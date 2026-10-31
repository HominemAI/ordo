//! `ordo console recover`: the lockout failsafe (`plan-console.md` 5.3
//! item 7, owner 2026-10-23, ask 17).
//!
//! Run inside the image (`docker exec`, `kubectl exec`). It prints a
//! one-time setup code that enrols a new passkey, and writes an audit row
//! with the time and the unix user who ran it. When the code is used, every
//! console session of the deployment ends (`Store::enrol_with_code`).
//!
//! It adds no power: whoever can run it already controls the deployment.
//!
//! `ordo console forget-door-key` forgets the console key (G14) this
//! console holds for the door. A console with `ORDO_IDENTITY_URL` set
//! refuses to boot while it holds one (`crate::server`), because unsetting
//! the setting later would bring the key back into use without anyone
//! deciding it. The person runs this once, and it is audited as them.

use crate::codes::{display_code, mint_code, setup_code_hash, Clock};
use crate::config::{DatabaseSettings, DEFAULT_DEPLOYMENT, DEPLOYMENT_ENV};
use crate::error::ConsoleError;
use crate::server::RECOVER_TTL_SECS;
use crate::store::{CodeKind, Store};

/// What `recover` printed.
#[derive(Debug, Clone)]
pub struct Recovered {
    /// The code, in its display form.
    pub code: String,
    /// The owner it enrols for; `None` when the console has no owner yet.
    pub owner: Option<String>,
    pub expires_at: u64,
}

/// Issue a recover code.
pub async fn recover(
    db: &DatabaseSettings,
    deployment: &str,
    owner: Option<&str>,
    unix_user: &str,
    clock: &Clock,
) -> Result<Recovered, ConsoleError> {
    let now = clock.now();
    let store = Store::open(db).await?;
    store.migrate(now).await?;
    let actor = format!("unix:{unix_user}");
    let result = issue(&store, deployment, owner, &actor, now).await;
    let (outcome, detail) = match &result {
        Ok(r) => (
            "ok".to_string(),
            format!("owner={}", r.owner.as_deref().unwrap_or("none yet")),
        ),
        Err(e) => (format!("refused:{}", e.code()), String::new()),
    };
    store
        .audit(
            deployment,
            &actor,
            "console.recover",
            &outcome,
            &detail,
            now,
        )
        .await?;
    result
}

async fn issue(
    store: &Store,
    deployment: &str,
    owner: Option<&str>,
    actor: &str,
    now: u64,
) -> Result<Recovered, ConsoleError> {
    if !store.deployment_exists(deployment).await? {
        return Err(ConsoleError::setting(
            DEPLOYMENT_ENV,
            format!("is `{deployment}`, and the console has never run for that deployment"),
            format!("Set {DEPLOYMENT_ENV} to the handle the console runs with"),
        ));
    }
    let owners = store.owners(deployment).await?;
    let target = match owner {
        Some(id) if owners.iter().any(|o| o.id == id) => Some(id.to_string()),
        Some(_) => return Err(ConsoleError::UnknownOwner),
        None => match owners.as_slice() {
            [] => None,
            [only] => Some(only.id.clone()),
            many => {
                return Err(ConsoleError::OwnerNotNamed {
                    owners: many.iter().map(|o| o.id.clone()).collect(),
                })
            }
        },
    };
    let code = mint_code();
    let expires_at = now + RECOVER_TTL_SECS;
    store
        .issue_setup_code(
            deployment,
            CodeKind::Recover,
            &setup_code_hash(&code),
            target.as_deref(),
            Some(expires_at),
            actor,
            now,
        )
        .await?;
    Ok(Recovered {
        code: display_code(&code),
        owner: target,
        expires_at,
    })
}

/// What `forget-door-key` forgot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Forgotten {
    /// The key's id at the door.
    Key(String),
    /// The console held no console key.
    Nothing,
}

/// Forget the console key the console holds for the door, and write an
/// audit row naming the unix user. The door's own record of the key is
/// not touched: the door refuses console key sign-in while its
/// `ORDO_IDENTITY_URL` is set, and the row is dropped in a later release.
pub async fn forget_door_key(
    db: &DatabaseSettings,
    deployment: &str,
    unix_user: &str,
    clock: &Clock,
) -> Result<Forgotten, ConsoleError> {
    let now = clock.now();
    let store = Store::open(db).await?;
    store.migrate(now).await?;
    let actor = format!("unix:{unix_user}");
    // The delete and its audit row are one transaction.
    match store
        .forget_credential_audited(deployment, &actor, "console.forget-door-key", now)
        .await?
    {
        Some(key) => Ok(Forgotten::Key(key)),
        None => Ok(Forgotten::Nothing),
    }
}

/// Who ran the command: `$USER`, `$LOGNAME`, or the uid the kernel reports.
pub fn unix_user() -> String {
    for var in ["USER", "LOGNAME"] {
        if let Ok(v) = std::env::var(var) {
            if !v.trim().is_empty() {
                return v;
            }
        }
    }
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("Uid:"))
                .and_then(|l| l.split_whitespace().next().map(|u| format!("uid {u}")))
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn run_forget() -> (i32, String) {
    let get = |k: &str| std::env::var(k).ok();
    let db = match DatabaseSettings::from_vars(&get) {
        Ok(db) => db,
        Err(e) => return (2, format!("ordo console forget-door-key: {e}\n")),
    };
    let deployment = get(DEPLOYMENT_ENV).unwrap_or_else(|| DEFAULT_DEPLOYMENT.to_string());
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            return (
                1,
                format!("ordo console forget-door-key: no runtime: {e}\n"),
            )
        }
    };
    let user = unix_user();
    match rt.block_on(forget_door_key(&db, &deployment, &user, &Clock::system())) {
        Ok(Forgotten::Key(id)) => (
            0,
            format!(
                "Forgot console key {id}. The console now calls the door only as the person \
                 signed in at the identity service. The door refuses this key while its \
                 ORDO_IDENTITY_URL is set. This command is in the console's audit log as unix \
                 user `{user}`.\n"
            ),
        ),
        Ok(Forgotten::Nothing) => (0, "This console holds no console key.\n".to_string()),
        Err(e) => (1, format!("ordo console forget-door-key: {e}\n")),
    }
}

/// `ordo console <args>`: the exit code and what to print.
pub fn run_cli(args: &[String]) -> (i32, String) {
    let usage = "usage: ordo console recover [--owner <id>]\n  \
         prints a one-time setup code that enrols a new console passkey. Run it\n  \
         inside the image. Reads ORDO_CONSOLE_DATABASE_URL (or\n  \
         ORDO_CONSOLE_DATABASE_URL_FILE) and ORDO_CONSOLE_DEPLOYMENT. When the code is used, every console session ends.\n\
       usage: ordo console forget-door-key\n  \
         forgets the console key this console holds for the door, so the console\n  \
         can start with ORDO_IDENTITY_URL set. Run it inside the image; same settings.\n"
        .to_string();
    let [sub, rest @ ..] = args else {
        return (2, usage);
    };
    if sub == "forget-door-key" && rest.is_empty() {
        return run_forget();
    }
    if sub != "recover" {
        return (2, usage);
    }
    let owner = match rest {
        [] => None,
        [flag, id] if flag == "--owner" => Some(id.clone()),
        _ => return (2, usage),
    };
    let get = |k: &str| std::env::var(k).ok();
    let db = match DatabaseSettings::from_vars(&get) {
        Ok(db) => db,
        Err(e) => return (2, format!("ordo console recover: {e}\n")),
    };
    let deployment = get(DEPLOYMENT_ENV).unwrap_or_else(|| DEFAULT_DEPLOYMENT.to_string());
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return (1, format!("ordo console recover: no runtime: {e}\n")),
    };
    let user = unix_user();
    match rt.block_on(recover(
        &db,
        &deployment,
        owner.as_deref(),
        &user,
        &Clock::system(),
    )) {
        Ok(r) => (
            0,
            format!(
                "Setup code: {}\n\nEnter it on the console's sign-in page and enrol a new \
                 passkey{}. It works once, for one hour. When it is used, every console \
                 session ends. This command is in the console's audit log as unix user `{user}`.\n",
                r.code,
                match &r.owner {
                    Some(o) => format!(" for owner {o}"),
                    None => " as the first console owner".to_string(),
                }
            ),
        ),
        Err(e) => (1, format!("ordo console recover: {e}\n")),
    }
}
