//! The console server (`plan-console.md` 5.1 and 5.3): health, strict
//! headers, and the owner's sign-in.
//!
//! Every route sits under a deployment handle, `/d/<handle>/...`
//! (`plan-console.md` 11); `/` redirects to the one open source holds.
//! Every state-changing call is a POST that must carry the console's own
//! `Origin`, and every one is written to the audit log with who, what, when
//! and the outcome.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, COOKIE, ORIGIN, REFERRER_POLICY, SET_COOKIE,
    X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use ordo_identity::passkey::PasskeyProvider;
use ordo_identity::webauthn::{b64url_decode, b64url_encode};
use ordo_identity::{
    Argon2idHasher, AuthError, AuthProvider, ChallengeId, CompleteRequest, PasswordHasher,
    PrincipalId, Timestamp,
};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::codes::{
    display_code, mint_code, mint_session_token, normalise_code, session_hash, setup_code_hash,
    Clock, LOOKUP_LEN,
};
use crate::config::{ConsoleConfig, DATA_DIR_ENV, DOOR_URL_ENV, LISTEN_ENV, STEP_UP_WINDOW_SECS};
use crate::door::DoorClient;
use crate::error::{ConsoleError, SessionEnd};
use crate::identity::IdentityClient;
use crate::link::{DoorAs, DoorLink, LinkKind};
use crate::page::Page;
use crate::runtime::spec::Patch;
use crate::runtime::Runtime;
use crate::sealing::ConsoleKey;
use crate::store::{CodeKind, NewSession, RecoveryRow, RevisionRow, SessionRow, Store};
use ordo_identity::deployment_token::{DeploymentClaims, Kind};

/// Launch holds at most two console owners (`plan-console.md` 5.3 item 6).
pub const MAX_OWNERS: usize = 2;
/// How long a second owner's invitation stays usable.
pub const INVITE_TTL_SECS: u64 = 24 * 60 * 60;
/// How long a code from `ordo console recover` stays usable.
pub const RECOVER_TTL_SECS: u64 = 60 * 60;
/// The file in the data directory the first-run setup code is written to.
pub const SETUP_CODE_FILE: &str = "setup-code";

/// What a console under the supervisor says in its boot log when no
/// identity service runs (owner, 2026-10-30): the database is explicit, or
/// opted into. Its old sign-in stays until a later, announced release.
pub const IDENTITY_OFF_NOTE: &str = "people sign in at this console, as before the one image: \
     no identity service runs, because neither ORDO_IDENTITY_DATABASE_URL (or its _FILE form) \
     nor ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER=true is set. Set one to move sign-in to the \
     identity service; a later, announced release will require one";

/// The same, as the banner on every console page and the `notice` of
/// `/api/status`, for the deployment owner who reads the console rather
/// than the log.
pub const IDENTITY_OFF_BANNER: &str = "Sign-in still happens at this console, as before. To \
     move it to the identity service, set ORDO_IDENTITY_DATABASE_URL (or \
     ORDO_IDENTITY_DATABASE_URL_FILE), or set ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER=true. A \
     later, announced release will require one of them.";

/// The page's policy: nothing from anywhere but the console itself, and no
/// framing.
pub const CSP: &str =
    "default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

/// The page's policy when people sign in at the identity service: the same,
/// and the page may also call the service at `origin` for its tokens and
/// touches (`plan-identity-service.md` 5.4).
pub fn csp_with_identity(origin: &str) -> String {
    format!(
        "default-src 'self'; connect-src 'self' {origin}; frame-ancestors 'none'; \
         base-uri 'none'; form-action 'self'"
    )
}

/// What a boot can be handed besides its settings.
pub struct BootOptions {
    pub clock: Clock,
    /// How recovery codes are hashed: argon2id.
    pub hasher: Arc<dyn PasswordHasher>,
}

impl Default for BootOptions {
    fn default() -> BootOptions {
        BootOptions {
            clock: Clock::system(),
            hasher: Arc::new(Argon2idHasher::new()),
        }
    }
}

/// The running console. Cheap to clone.
#[derive(Clone)]
pub struct Console(Arc<Inner>);

struct Inner {
    cfg: ConsoleConfig,
    store: Store,
    clock: Clock,
    hasher: Arc<dyn PasswordHasher>,
    providers: HashMap<String, Arc<PasskeyProvider>>,
    page: Page,
    /// The deployment's runtime through the supervisor's socket, in
    /// supervise mode only. Door-only mode holds none, and so no way to
    /// start, stop or change the deployment.
    runtime: Option<Runtime>,
    /// The link to the deployment's door (`crate::link`). `None` when no
    /// door address is known (door-only mode without `ORDO_CONSOLE_DOOR_URL`).
    link: Option<DoorLink>,
    /// The one client to the identity service (`crate::identity`), when
    /// `ORDO_IDENTITY_URL` is set. Then people sign in there, every call
    /// carries its token as a bearer, and the console's own sign-in and
    /// console key routes answer `moved_to_identity_service`.
    identity: Option<Arc<IdentityClient>>,
    /// The page's Content-Security-Policy.
    csp: HeaderValue,
}

/// Who is calling and what the audit row says about it.
struct Call {
    deployment: String,
    actor: String,
    detail: String,
}

/// A successful answer: a JSON body, and a cookie to set or clear.
struct Reply {
    body: Value,
    cookie: Option<String>,
}

impl Reply {
    fn json(body: Value) -> Reply {
        Reply { body, cookie: None }
    }
}

impl IntoResponse for Reply {
    fn into_response(self) -> Response {
        let mut res = (StatusCode::OK, Json(self.body)).into_response();
        if let Some(c) = self.cookie.and_then(|c| HeaderValue::from_str(&c).ok()) {
            res.headers_mut().insert(SET_COOKIE, c);
        }
        res
    }
}

/// What a call needs from the session.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Need {
    /// Any live session, including one a recovery code opened.
    Enrol,
    /// A full session.
    Signed,
    /// A full session with a passkey touch in the step-up window.
    Touch,
}

/// A checked session.
struct Authed {
    hash: String,
    row: SessionRow,
}

/// Who a call is from: an owner with a console session (the console's own
/// sign-in), or a deployment person with a token from the identity service.
enum Who {
    Owner(Authed),
    Person {
        token: String,
        claims: DeploymentClaims,
    },
}

impl Who {
    /// The audit's name for them.
    fn actor(&self) -> String {
        match self {
            Who::Owner(a) => format!("owner:{}", a.row.owner_id),
            Who::Person { claims, .. } => format!("{}:{}", claims.kind.as_str(), claims.sub),
        }
    }

    fn is_person(&self) -> bool {
        matches!(self, Who::Person { .. })
    }

    /// The door, called as them: the console key for an owner, their own
    /// token for a person. A person's call never uses the console key.
    fn door<'a>(&self, c: &'a Console) -> Result<DoorAs<'a>, ConsoleError> {
        let link = door_link(c)?;
        Ok(match self {
            Who::Owner(_) => link.as_key(),
            Who::Person { token, .. } => link.as_person(token),
        })
    }
}

/// The bearer token a call carries, if any.
fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// How far a token's `touched_at` may sit ahead of the console's clock.
const TOUCH_LEEWAY_SECS: u64 = ordo_identity::deployment_token::DEFAULT_LEEWAY_SECS;

/// Was the person's last touch within the step-up window? A touch from the
/// future past the leeway is not a touch, as at the door.
fn fresh_token_touch(touched_at: Option<u64>, now: u64) -> bool {
    touched_at.is_some_and(|t| {
        t <= now.saturating_add(TOUCH_LEEWAY_SECS) && now.saturating_sub(t) <= STEP_UP_WINDOW_SECS
    })
}

fn parse<T: DeserializeOwned>(body: &[u8]) -> Result<T, ConsoleError> {
    let body = if body.iter().all(u8::is_ascii_whitespace) {
        b"{}".as_slice()
    } else {
        body
    };
    serde_json::from_slice(body).map_err(|e| ConsoleError::BadRequest(e.to_string()))
}

fn b64(field: &str, v: &str) -> Result<Vec<u8>, ConsoleError> {
    b64url_decode(v).map_err(|_| ConsoleError::BadRequest(format!("`{field}` is not base64url")))
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ConsoleError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ConsoleError::Internal(format!("a blocking task did not finish: {e}")))
}

use ordo_identity::passkey::{creation_json, request_json};

#[derive(Deserialize)]
struct CodeBody {
    code: String,
}

#[derive(Deserialize)]
struct Registration {
    challenge: String,
    #[serde(rename = "clientDataJSON")]
    client_data_json: String,
    #[serde(rename = "attestationObject")]
    attestation_object: String,
}

#[derive(Deserialize)]
struct SetupFinish {
    code: String,
    label: Option<String>,
    #[serde(flatten)]
    registration: Registration,
}

#[derive(Deserialize)]
struct Assertion {
    challenge: String,
    #[serde(rename = "credentialId")]
    credential_id: String,
    #[serde(rename = "clientDataJSON")]
    client_data_json: String,
    #[serde(rename = "authenticatorData")]
    authenticator_data: String,
    signature: String,
}

#[derive(Deserialize)]
struct OwnerBody {
    owner: String,
}

impl Assertion {
    fn request(&self) -> Result<CompleteRequest, ConsoleError> {
        Ok(CompleteRequest::PasskeyAssertion {
            challenge: ChallengeId(self.challenge.clone()),
            credential_id: b64("credentialId", &self.credential_id)?,
            client_data_json: b64("clientDataJSON", &self.client_data_json)?,
            authenticator_data: b64("authenticatorData", &self.authenticator_data)?,
            signature: b64("signature", &self.signature)?,
        })
    }
}

/// A POST that changes something: origin checked, audited.
macro_rules! action {
    ($name:ident, $action:literal, $inner:ident) => {
        async fn $name(
            State(c): State<Console>,
            Path(handle): Path<String>,
            headers: HeaderMap,
            body: Bytes,
        ) -> Response {
            let mut call = Call {
                deployment: handle,
                actor: "anonymous".into(),
                detail: String::new(),
            };
            let result = match c.admit(&call, &headers, true) {
                Ok(()) => $inner(&c, &mut call, &headers, &body).await,
                Err(e) => Err(e),
            };
            c.finish(&call, $action, result).await
        }
    };
}

/// A GET that reads.
macro_rules! read {
    ($name:ident, $inner:ident) => {
        async fn $name(
            State(c): State<Console>,
            Path(handle): Path<String>,
            headers: HeaderMap,
        ) -> Response {
            let mut call = Call {
                deployment: handle,
                actor: "anonymous".into(),
                detail: String::new(),
            };
            let result = match c.admit(&call, &headers, false) {
                Ok(()) => $inner(&c, &mut call, &headers).await,
                Err(e) => Err(e),
            };
            match result {
                Ok(r) => r.into_response(),
                Err(e) => e.into_response(),
            }
        }
    };
}

action!(setup_begin_route, "setup.begin", setup_begin);
action!(setup_finish_route, "setup.finish", setup_finish);
action!(signin_begin_route, "signin.begin", signin_begin);
action!(signin_finish_route, "signin.finish", signin_finish);
action!(stepup_begin_route, "stepup.begin", stepup_begin);
action!(stepup_finish_route, "stepup.finish", stepup_finish);
action!(recovery_route, "recovery.redeem", recovery_redeem);
action!(passkeys_begin_route, "passkeys.begin", passkeys_begin);
action!(passkeys_finish_route, "passkeys.finish", passkeys_finish);
action!(recovery_renew_route, "recovery_codes.renew", recovery_renew);
action!(invite_route, "owners.invite", owners_invite);
action!(remove_route, "owners.remove", owners_remove);
action!(signout_route, "signout", signout);
read!(session_route, session_get);
read!(owners_route, owners_list);
read!(runtime_route, runtime_get);
read!(revisions_route, revisions_list);
action!(apply_route, "runtime.apply", runtime_apply);
read!(door_route, door_status);
action!(link_begin_route, "door.link.begin", door_link_begin);
action!(link_finish_route, "door.link.finish", door_link_finish);
action!(
    door_stepup_begin_route,
    "door.stepup.begin",
    door_stepup_begin
);
action!(
    door_stepup_finish_route,
    "door.stepup.finish",
    door_stepup_finish
);
action!(door_rotate_route, "door.rotate", door_rotate);
action!(door_unlink_route, "door.unlink", door_unlink);
read!(door_keys_route, door_keys);
read!(door_health_route, door_health);
action!(door_revoke_route, "door.keys.revoke", door_revoke);
action!(door_forget_route, "door.forget", door_forget);
action!(rollback_route, "runtime.rollback", runtime_rollback);
action!(restart_route, "runtime.restart", runtime_restart);

// The Deploys screen (step 4), in its own file.
/// The Deployment people screen's routes (identity service, step 6).
mod admins;
mod deploys;
/// The People screen's routes (step 8).
mod people;
/// The settings screens' route (step 6).
mod settings;
mod shares; // Shares and links, and the security log (step 9).
/// The Tenants screen's routes (step 5).
mod tenants;
mod usage; // The Usage screen (step 7).

impl Console {
    /// Open the database, run migrations, file the deployment, and print
    /// the first-run setup code when there is no owner yet.
    pub async fn boot(cfg: ConsoleConfig, opts: BootOptions) -> Result<Console, ConsoleError> {
        let now = opts.clock.now();
        let store = Store::open(&cfg.database).await?;
        store.migrate(now).await?;
        store
            .ensure_deployment(
                &cfg.deployment,
                cfg.door_url.as_deref(),
                cfg.mode.as_str(),
                now,
            )
            .await?;
        // With the identity service the console calls the door as the
        // signed-in person. A console key it still holds would come back
        // into use the day `ORDO_IDENTITY_URL` is unset, without anyone
        // deciding it, so it is refused here until a person forgets it.
        // Forgetting it at boot instead would destroy a credential on a
        // setting change that may be a trial; the refusal keeps that a
        // decision, made once and audited under its unix user.
        if cfg.identity.is_some() {
            if let Some(row) = store.credential(&cfg.deployment).await? {
                return Err(ConsoleError::setting(
                    crate::identity::URL_ENV,
                    format!(
                        "is set, and this console still holds console key {} for the door. \
                         People sign in at the identity service now and the console calls the \
                         door as them, so the key must go",
                        row.key_id
                    ),
                    format!(
                        "Run `ordo-console forget-door-key` (or `ordo console forget-door-key`) \
                         inside the image, then start the console again. To keep the console's \
                         own sign-in and its key instead, unset {}",
                        crate::identity::URL_ENV
                    ),
                ));
            }
        }
        let provider =
            crate::passkeys::provider(store.pool(), &cfg.deployment, &cfg.rp_id, &cfg.origin)?;
        let mut providers = HashMap::new();
        providers.insert(cfg.deployment.clone(), Arc::new(provider));
        // No identity service under the supervisor: the old sign-in stays,
        // and every page says so (owner, 2026-10-30).
        let identity_off = cfg.identity.is_none() && cfg.runtime.socket.is_some();
        let mut page = Page::load(&cfg.page_dir);
        if identity_off {
            page = page.with_notice(IDENTITY_OFF_BANNER);
        }
        let identity = match &cfg.identity {
            None => None,
            Some(settings) => {
                let client = Arc::new(IdentityClient::new(settings.clone(), opts.clock.clone()));
                // Not fatal: the service may start after the console. Until
                // it answers, tokens are refused with `token_refused`, and
                // the first one with a key the console lacks fetches again.
                if let Err(e) = client.refresh().await {
                    eprintln!(
                        "ordo-console: the identity service's keys were not fetched at boot: {e}"
                    );
                }
                client.spawn_refresher();
                eprintln!(
                    "ordo-console: people sign in at the identity service at {}",
                    settings.issuer
                );
                Some(client)
            }
        };
        let csp = match &cfg.identity {
            None => HeaderValue::from_static(CSP),
            Some(s) => HeaderValue::from_str(&csp_with_identity(&s.issuer)).map_err(|_| {
                ConsoleError::setting(
                    crate::identity::ISSUER_ENV,
                    format!("is `{}`, which cannot be put in a header", s.issuer),
                    format!(
                        "Set {} to an origin such as https://console.example.com:8082",
                        crate::identity::ISSUER_ENV
                    ),
                )
            })?,
        };
        // The console's sealing key, made here at first boot in either mode:
        // the console key it signs in to the deployment with is sealed with
        // it (G14).
        let sealing = ConsoleKey::load_or_create(&cfg.runtime.key_file)?;
        let link = cfg.door.clone().map(|url| {
            DoorLink::new(
                &cfg.deployment,
                &cfg.door_space,
                DoorClient::new(url),
                store.clone(),
                sealing.clone(),
                opts.clock.clone(),
            )
            .with_rotation_days(cfg.key_rotate_days)
        });
        let runtime = cfg.runtime.socket.clone().map(|socket| {
            Runtime::new(
                &cfg.deployment,
                store.clone(),
                opts.clock.clone(),
                sealing.clone(),
                ordo_supervisor::SupervisorClient::new(socket),
                cfg.listen.port(),
            )
        });
        if let Some(rt) = &runtime {
            sync_until_done(rt.clone()).await;
            if cfg.identity.is_none() {
                eprintln!("ordo-console: {}", IDENTITY_OFF_NOTE);
            }
        }
        let console = Console(Arc::new(Inner {
            page,
            cfg,
            store,
            clock: opts.clock,
            hasher: opts.hasher,
            providers,
            runtime,
            link,
            identity,
            csp,
        }));
        console.first_run(now).await?;
        let dep = console.0.cfg.deployment.clone();
        console
            .0
            .store
            .audit(&dep, "system", "console.boot", "ok", "", now)
            .await?;
        Ok(console)
    }

    pub fn clock(&self) -> &Clock {
        &self.0.clock
    }

    pub fn store(&self) -> &Store {
        &self.0.store
    }

    /// The link to the deployment, when a door address is known.
    pub fn link(&self) -> Option<&DoorLink> {
        self.0.link.as_ref()
    }

    pub fn config(&self) -> &ConsoleConfig {
        &self.0.cfg
    }

    /// The deployment's runtime, in supervise mode.
    pub fn runtime(&self) -> Option<&Runtime> {
        self.0.runtime.as_ref()
    }

    /// Nothing to stop: the supervisor runs the deployment, and stops it
    /// itself. Kept so a caller that ends a console reads the same.
    pub async fn shutdown(&self) {}

    /// Where the first-run setup code is written.
    pub fn setup_code_path(&self) -> PathBuf {
        self.0.cfg.data_dir.join(SETUP_CODE_FILE)
    }

    async fn first_run(&self, now: u64) -> Result<(), ConsoleError> {
        let dep = &self.0.cfg.deployment;
        let path = self.setup_code_path();
        if self.0.identity.is_some() {
            // People sign in at the identity service, whose first admin
            // comes from provisioning or its own first boot
            // (`plan-identity-service.md` 5.1, 5.2). No console setup code.
            let _ = std::fs::remove_file(&path);
            return Ok(());
        }
        if !self.0.store.owners(dep).await?.is_empty() {
            let _ = std::fs::remove_file(&path);
            return Ok(());
        }
        let code = mint_code();
        write_secret_file(&path, &display_code(&code)).map_err(|e| {
            ConsoleError::setting_caused(
                DATA_DIR_ENV,
                format!(
                    "is `{}`, and the setup code could not be written there",
                    self.0.cfg.data_dir.display()
                ),
                format!(
                    "Set {DATA_DIR_ENV} to a directory the console can write, on its own volume"
                ),
                e,
            )
        })?;
        self.0
            .store
            .issue_setup_code(
                dep,
                CodeKind::FirstRun,
                &setup_code_hash(&code),
                None,
                None,
                "system",
                now,
            )
            .await?;
        eprintln!(
            "ordo-console: no console owner yet. Setup code: {} (also written to {}). \
             The first person to enter it and enrol a passkey becomes the console owner; \
             the code then stops working.",
            display_code(&code),
            path.display()
        );
        self.0
            .store
            .audit(
                dep,
                "system",
                "setup_code.issued",
                "ok",
                "kind=first-run",
                now,
            )
            .await?;
        Ok(())
    }

    /// The routes.
    pub fn router(&self) -> Router {
        // The console's own sign-in and the console key link (G14, G15).
        // With the identity service they answer `moved_to_identity_service`
        // and do nothing (`plan-identity-service.md` 5.4, 6).
        let own_sign_in = Router::new()
            .route("/d/{handle}/api/owners", get(owners_route))
            .route("/d/{handle}/api/setup/begin", post(setup_begin_route))
            .route("/d/{handle}/api/setup/finish", post(setup_finish_route))
            .route("/d/{handle}/api/signin/begin", post(signin_begin_route))
            .route("/d/{handle}/api/signin/finish", post(signin_finish_route))
            .route("/d/{handle}/api/stepup/begin", post(stepup_begin_route))
            .route("/d/{handle}/api/stepup/finish", post(stepup_finish_route))
            .route("/d/{handle}/api/recovery", post(recovery_route))
            .route("/d/{handle}/api/passkeys/begin", post(passkeys_begin_route))
            .route(
                "/d/{handle}/api/passkeys/finish",
                post(passkeys_finish_route),
            )
            .route("/d/{handle}/api/recovery-codes", post(recovery_renew_route))
            .route("/d/{handle}/api/owners/invite", post(invite_route))
            .route("/d/{handle}/api/owners/remove", post(remove_route))
            .route("/d/{handle}/api/signout", post(signout_route))
            .route("/d/{handle}/api/door/link/begin", post(link_begin_route))
            .route("/d/{handle}/api/door/link/finish", post(link_finish_route))
            .route(
                "/d/{handle}/api/door/stepup/begin",
                post(door_stepup_begin_route),
            )
            .route(
                "/d/{handle}/api/door/stepup/finish",
                post(door_stepup_finish_route),
            )
            .route("/d/{handle}/api/door/rotate", post(door_rotate_route))
            .route("/d/{handle}/api/door/unlink", post(door_unlink_route))
            .route("/d/{handle}/api/door/keys", get(door_keys_route))
            .route("/d/{handle}/api/door/keys/revoke", post(door_revoke_route))
            .route("/d/{handle}/api/door/forget", post(door_forget_route))
            .route_layer(axum::middleware::from_fn_with_state(
                self.clone(),
                own_sign_in_gate,
            ));
        Router::new()
            .route("/health/live", get(live))
            .route("/health/ready", get(ready))
            .route("/", get(root))
            .route("/assets/{*path}", get(asset))
            .route("/d/{handle}", get(to_page))
            .route("/d/{handle}/", get(page))
            .route("/d/{handle}/{*rest}", get(page_deep))
            .route("/d/{handle}/api/status", get(status))
            .route("/d/{handle}/api/session", get(session_route))
            .route("/d/{handle}/api/runtime", get(runtime_route))
            .route("/d/{handle}/api/runtime/revisions", get(revisions_route))
            .route("/d/{handle}/api/runtime/apply", post(apply_route))
            .route("/d/{handle}/api/runtime/rollback", post(rollback_route))
            .route("/d/{handle}/api/runtime/restart", post(restart_route))
            .route("/d/{handle}/api/door", get(door_route))
            .route("/d/{handle}/api/door/health", get(door_health_route))
            .merge(own_sign_in)
            .merge(deploys::routes())
            .merge(tenants::routes())
            .merge(people::routes())
            .merge(settings::routes())
            .merge(usage::routes())
            .merge(shares::routes())
            .merge(admins::routes())
            .layer(axum::middleware::map_response_with_state(
                self.clone(),
                strict_headers,
            ))
            .merge(self.identity_passthrough())
            .with_state(self.clone())
    }

    /// The identity service under the console's own origin, when its base
    /// path is set (`ORDO_IDENTITY_BASE_PATH`): `<base>` and everything
    /// under it pass through to the service as they came, its answers with
    /// its own headers (its pages carry their own policy). Outside the
    /// console's strict headers on purpose. No routes otherwise.
    fn identity_passthrough(&self) -> Router<Console> {
        let base = match &self.0.identity {
            Some(i) if !i.settings().base_path.is_empty() => i.settings().base_path.clone(),
            _ => return Router::new(),
        };
        Router::new()
            .route(&base, axum::routing::any(identity_passthrough))
            .route(
                &format!("{base}/"),
                axum::routing::any(identity_passthrough),
            )
            .route(
                &format!("{base}/{{*rest}}"),
                axum::routing::any(identity_passthrough),
            )
    }

    /// Listen and serve until interrupted.
    pub async fn serve(self) -> Result<(), ConsoleError> {
        let addr = self.0.cfg.listen;
        let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
            ConsoleError::setting_caused(
                LISTEN_ENV,
                format!("is {addr}, and the console could not listen there"),
                format!("Set {LISTEN_ENV} to a free address and port"),
                e,
            )
        })?;
        eprintln!(
            "ordo-console: listening on {addr} for deployment `{}`",
            self.0.cfg.deployment
        );
        // The visitor's address reaches the identity service's guessing
        // limits through a passed-through call (`identity_passthrough`).
        let served = axum::serve(
            listener,
            self.router()
                .into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(stop_signal())
        .await
        .map_err(|e| ConsoleError::Internal(format!("the server stopped: {e}")));
        self.shutdown().await;
        served
    }

    fn now(&self) -> u64 {
        self.0.clock.now()
    }

    fn provider(&self, dep: &str) -> Result<Arc<PasskeyProvider>, ConsoleError> {
        self.0
            .providers
            .get(dep)
            .cloned()
            .ok_or_else(|| ConsoleError::UnknownDeployment(dep.to_string()))
    }

    /// The deployment is one this console manages, and a write comes from
    /// the console's own page.
    fn admit(&self, call: &Call, headers: &HeaderMap, write: bool) -> Result<(), ConsoleError> {
        if !self.0.providers.contains_key(&call.deployment) {
            return Err(ConsoleError::UnknownDeployment(call.deployment.clone()));
        }
        if write {
            let origin = headers.get(ORIGIN).and_then(|v| v.to_str().ok());
            if origin != Some(self.0.cfg.origin.as_str()) {
                return Err(ConsoleError::OriginRefused);
            }
        }
        Ok(())
    }

    /// Write the audit row, then answer.
    async fn finish(
        &self,
        call: &Call,
        action: &str,
        result: Result<Reply, ConsoleError>,
    ) -> Response {
        let outcome = match &result {
            Ok(_) => "ok".to_string(),
            Err(e) => format!("refused:{}", e.code()),
        };
        if let Err(e) = self
            .0
            .store
            .audit(
                &call.deployment,
                &call.actor,
                action,
                &outcome,
                &call.detail,
                self.now(),
            )
            .await
        {
            eprintln!(
                "ordo-console: the audit row for {action} by {} ({outcome}) was not written: {e}",
                call.actor
            );
        }
        match result {
            Ok(r) => r.into_response(),
            Err(e) => e.into_response(),
        }
    }

    fn cookie_name(&self) -> &'static str {
        if self.0.cfg.secure_origin() {
            "__Host-ordo-console"
        } else {
            "ordo-console"
        }
    }

    fn set_cookie(&self, token: &str) -> String {
        let secure = if self.0.cfg.secure_origin() {
            "; Secure"
        } else {
            ""
        };
        format!(
            "{}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{secure}",
            self.cookie_name(),
            self.0.cfg.session_max_secs
        )
    }

    fn clear_cookie(&self) -> String {
        let secure = if self.0.cfg.secure_origin() {
            "; Secure"
        } else {
            ""
        };
        format!(
            "{}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure}",
            self.cookie_name()
        )
    }

    fn token(&self, headers: &HeaderMap) -> Option<String> {
        let name = self.cookie_name();
        headers
            .get_all(COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(';'))
            .filter_map(|kv| kv.trim().split_once('='))
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.to_string())
            .filter(|v| !v.is_empty())
    }

    /// Check the session a call carries.
    async fn session(
        &self,
        call: &mut Call,
        headers: &HeaderMap,
        need: Need,
    ) -> Result<Authed, ConsoleError> {
        let token = self.token(headers).ok_or(ConsoleError::NotSignedIn)?;
        let hash = session_hash(&token);
        let row = self
            .0
            .store
            .session(&hash)
            .await?
            .filter(|r| r.deployment == call.deployment)
            .ok_or(ConsoleError::NotSignedIn)?;
        call.actor = format!("owner:{}", row.owner_id);
        if row.ended || !row.owner_active {
            return Err(ConsoleError::SessionEnded(SessionEnd::Ended));
        }
        let now = self.now();
        let cfg = &self.0.cfg;
        if now.saturating_sub(row.created_at) > cfg.session_max_secs {
            self.0
                .store
                .end_session(&hash, "absolute limit", now)
                .await?;
            return Err(ConsoleError::SessionEnded(SessionEnd::Absolute {
                limit_secs: cfg.session_max_secs,
            }));
        }
        if now.saturating_sub(row.last_seen_at) > cfg.session_idle_secs {
            self.0.store.end_session(&hash, "idle limit", now).await?;
            return Err(ConsoleError::SessionEnded(SessionEnd::Idle {
                limit_secs: cfg.session_idle_secs,
            }));
        }
        self.0.store.seen(&hash, now).await?;
        if row.must_enrol && need != Need::Enrol {
            return Err(ConsoleError::MustEnrolPasskey);
        }
        if need == Need::Touch && !fresh_touch(&row, now) {
            return Err(ConsoleError::StepUpNeeded {
                window_secs: STEP_UP_WINDOW_SECS,
            });
        }
        Ok(Authed { hash, row })
    }

    /// Who a call is from. With the identity service: the bearer token,
    /// verified with its published keys (`iss`, `aud`, `exp`, signature),
    /// from a person, and with a touch in the step-up window when `need`
    /// asks for one. Without it: the console's own session cookie.
    async fn who(
        &self,
        call: &mut Call,
        headers: &HeaderMap,
        need: Need,
    ) -> Result<Who, ConsoleError> {
        let Some(identity) = &self.0.identity else {
            return Ok(Who::Owner(self.session(call, headers, need).await?));
        };
        let token = bearer(headers).ok_or(ConsoleError::NotSignedIn)?;
        let claims = identity
            .verify(&token)
            .await
            .map_err(ConsoleError::TokenRefused)?;
        call.actor = format!("{}:{}", claims.kind.as_str(), claims.sub);
        // A person only. A machine key has no page, and a break-glass
        // token is for the host's own `ordo` CLI (plan 5.8): neither uses
        // the console.
        if claims.kind != Kind::Person {
            return Err(ConsoleError::PersonOnly);
        }
        if need == Need::Touch && !fresh_token_touch(claims.touched_at, self.now()) {
            return Err(ConsoleError::StepUpNeeded {
                window_secs: STEP_UP_WINDOW_SECS,
            });
        }
        Ok(Who::Person { token, claims })
    }

    /// Where people sign in, when at the identity service: its origin and
    /// its base path.
    fn identity_origin(&self) -> Option<String> {
        self.0.identity.as_ref().map(|i| i.settings().public_url())
    }

    async fn owner_credentials(
        &self,
        dep: &str,
        owner: &str,
    ) -> Result<Vec<Vec<u8>>, ConsoleError> {
        Ok(self
            .0
            .store
            .owner_credentials(dep, owner)
            .await?
            .iter()
            .filter_map(|c| b64url_decode(c).ok())
            .collect())
    }

    /// Ten fresh recovery codes: as shown once, and as stored.
    async fn mint_recovery_set(&self) -> Result<(Vec<String>, Vec<RecoveryRow>), ConsoleError> {
        let hasher = self.0.hasher.clone();
        blocking(move || {
            let (shown, rows) = ordo_identity::codes::mint_recovery_set(hasher.as_ref());
            let rows = rows
                .into_iter()
                .map(|r| RecoveryRow {
                    lookup: r.lookup,
                    hash: r.hash,
                })
                .collect();
            (shown, rows)
        })
        .await
    }

    /// A new session: its token, and the cookie that carries it.
    fn new_token(&self) -> (String, String) {
        let token = mint_session_token();
        (session_hash(&token), self.set_cookie(&token))
    }
}

/// Ctrl-C, or SIGTERM: what `docker stop` and an orchestrator send.
async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn fresh_touch(row: &SessionRow, now: u64) -> bool {
    row.touched_at
        .is_some_and(|t| now.saturating_sub(t) <= STEP_UP_WINDOW_SECS)
}

fn write_secret_file(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    writeln!(f, "{text}")
}

/// In front of the console's own sign-in and console key routes: with the
/// identity service, they answer `moved_to_identity_service` and run
/// nothing.
async fn own_sign_in_gate(
    State(c): State<Console>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    match c.identity_origin() {
        Some(origin) => ConsoleError::MovedToIdentityService { origin }.into_response(),
        None => next.run(req).await,
    }
}

/// One browser request passed through to the identity service (see
/// `Console::identity_passthrough`). A service that does not answer is the
/// console's own `identity_service_not_answering`, which says to retry.
async fn identity_passthrough(State(c): State<Console>, req: axum::extract::Request) -> Response {
    let Some(client) = c.0.identity.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let peer = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip());
    let method = req.method().as_str().to_string();
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".into());
    let headers: Vec<(String, Vec<u8>)> = req
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.as_bytes().to_vec()))
        .collect();
    let body = match axum::body::to_bytes(req.into_body(), crate::identity::MAX_PASS_BYTES).await {
        Ok(b) => b.to_vec(),
        Err(_) => {
            return ConsoleError::BadRequest(format!(
                "a request to the identity service is at most {} bytes",
                crate::identity::MAX_PASS_BYTES
            ))
            .into_response()
        }
    };
    match client
        .forward(&method, &path_and_query, &headers, peer, body)
        .await
    {
        Ok(p) => {
            let mut res = Response::new(axum::body::Body::from(p.body));
            *res.status_mut() = StatusCode::from_u16(p.status).unwrap_or(StatusCode::BAD_GATEWAY);
            for (k, v) in p.headers {
                if let (Ok(name), Ok(value)) = (
                    axum::http::HeaderName::from_bytes(k.as_bytes()),
                    HeaderValue::from_bytes(&v),
                ) {
                    res.headers_mut().append(name, value);
                }
            }
            res
        }
        Err(e) => ConsoleError::Identity(e).into_response(),
    }
}

async fn strict_headers(State(c): State<Console>, mut res: Response) -> Response {
    let h = res.headers_mut();
    h.insert(CONTENT_SECURITY_POLICY, c.0.csp.clone());
    h.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    if !h.contains_key(CACHE_CONTROL) {
        h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    h.insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
    res
}

async fn live() -> Json<Value> {
    Json(json!({ "status": "live" }))
}

/// Ready once booted. It never asks the deployment; it reports whether the
/// console's own database answers, and stays ready when it does not, so
/// the sign-in page keeps answering (`plan-console.md` 5.2). It stays ready
/// while the child is down or crash-looping too (6.2), and says which.
async fn ready(State(c): State<Console>) -> Json<Value> {
    let db = tokio::time::timeout(Duration::from_secs(1), c.0.store.pool().rows("SELECT 1")).await;
    let database = match db {
        Ok(Ok(_)) => "ok",
        _ => "unreachable",
    };
    // Asked of the supervisor, briefly: the console's readiness never
    // waits on it.
    let deployment = match &c.0.runtime {
        Some(s) => match tokio::time::timeout(Duration::from_secs(1), s.status()).await {
            Ok(Ok(st)) => json!(st.state),
            _ => json!("unknown"),
        },
        None => json!("door-only"),
    };
    Json(json!({ "status": "ready", "consoleDatabase": database, "deployment": deployment }))
}

async fn root(State(c): State<Console>) -> Redirect {
    Redirect::to(&format!("/d/{}/", c.0.cfg.deployment))
}

async fn to_page(Path(handle): Path<String>) -> Redirect {
    Redirect::to(&format!("/d/{handle}/"))
}

/// The page shell for a deployment this console manages. The page routes
/// itself from the path.
async fn page(State(c): State<Console>, Path(handle): Path<String>) -> Response {
    if !c.0.providers.contains_key(&handle) {
        return ConsoleError::UnknownDeployment(handle).into_response();
    }
    c.0.page.index()
}

/// A deeper page route (`/d/<handle>/owners`). An unknown `api/` path is a
/// JSON 404, never the page.
async fn page_deep(
    State(c): State<Console>,
    Path((handle, rest)): Path<(String, String)>,
) -> Response {
    if rest == "api" || rest.starts_with("api/") {
        return (
            StatusCode::NOT_FOUND,
            Json(
                json!({ "error": "not_found", "message": "no such console call", "retry": false }),
            ),
        )
            .into_response();
    }
    page(State(c), Path(handle)).await
}

async fn asset(State(c): State<Console>, Path(path): Path<String>) -> Response {
    c.0.page.asset(&path).await
}

/// What the page needs before anyone signs in: whether the console has an
/// owner yet, so it opens on setup or on sign-in. Says nothing else.
async fn status(State(c): State<Console>, Path(handle): Path<String>) -> Response {
    if !c.0.providers.contains_key(&handle) {
        return ConsoleError::UnknownDeployment(handle).into_response();
    }
    if let Some(i) = &c.0.identity {
        // People sign in at the identity service: the page asks it for a
        // token for `audience`, and sends the browser to `origin` to sign
        // in when it has none.
        return Json(json!({
            "deployment": handle,
            "hasOwner": true,
            "identity": {
                "origin": i.settings().public_url(),
                "audience": i.settings().audience,
            },
        }))
        .into_response();
    }
    match c.0.store.owners(&handle).await {
        Ok(owners) => {
            let mut body = json!({
                "deployment": handle,
                "hasOwner": !owners.is_empty(),
            });
            // Under the supervisor with no identity service: the banner's
            // words, for a page or a script reading the status.
            if c.0.runtime.is_some() {
                body["notice"] = json!(IDENTITY_OFF_BANNER);
            }
            Json(body).into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn setup_begin(
    c: &Console,
    call: &mut Call,
    _headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let req: CodeBody = parse(body)?;
    let code = normalise_code(&req.code).ok_or(ConsoleError::CodeRefused)?;
    let hash = setup_code_hash(&code);
    let now = c.now();
    let live =
        c.0.store
            .live_setup_code(&call.deployment, &hash, now)
            .await?
            .ok_or(ConsoleError::CodeRefused)?;
    call.detail = format!("kind={}", live.kind.as_str());
    let exclude = match &live.owner_id {
        Some(o) => c.owner_credentials(&call.deployment, o).await?,
        None => Vec::new(),
    };
    let provider = c.provider(&call.deployment)?;
    let binding = format!("setup:{hash}");
    let opts = blocking(move || {
        provider.begin_registration_bound(Timestamp(now), &exclude, Some(&binding))
    })
    .await??;
    Ok(Reply::json(creation_json(
        &opts,
        "Ordo console",
        "Console owner",
    )))
}

async fn setup_finish(
    c: &Console,
    call: &mut Call,
    _headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let req: SetupFinish = parse(body)?;
    let code = normalise_code(&req.code).ok_or(ConsoleError::CodeRefused)?;
    let hash = setup_code_hash(&code);
    let now = c.now();
    let provider = c.provider(&call.deployment)?;
    let r = &req.registration;
    let challenge = ChallengeId(r.challenge.clone());
    let client_data = b64("clientDataJSON", &r.client_data_json)?;
    let attestation = b64("attestationObject", &r.attestation_object)?;
    let pending = blocking(move || {
        provider.verify_registration(&challenge, &client_data, &attestation, Timestamp(now))
    })
    .await??;
    if pending.binding() != Some(format!("setup:{hash}").as_str()) {
        return Err(ConsoleError::CodeRefused);
    }
    let label = req
        .label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .unwrap_or("Console owner")
        .chars()
        .take(64)
        .collect::<String>();
    let (codes, rows) = c.mint_recovery_set().await?;
    let (session, cookie) = c.new_token();
    let enrolled =
        c.0.store
            .enrol_with_code(
                &call.deployment,
                &hash,
                &label,
                pending.stored(),
                &rows,
                &session,
                MAX_OWNERS,
                now,
            )
            .await?;
    call.actor = format!("owner:{}", enrolled.owner_id);
    call.detail = format!(
        "kind={} sessions_ended={}",
        enrolled.kind.as_str(),
        enrolled.sessions_ended
    );
    if enrolled.kind == CodeKind::FirstRun {
        let _ = std::fs::remove_file(c.setup_code_path());
    }
    Ok(Reply {
        body: json!({
            "owner": enrolled.owner_id,
            "kind": enrolled.kind.as_str(),
            "recoveryCodes": codes,
            "sessionsEnded": enrolled.sessions_ended,
        }),
        cookie: Some(cookie),
    })
}

async fn signin_begin(
    c: &Console,
    call: &mut Call,
    _headers: &HeaderMap,
    _body: &[u8],
) -> Result<Reply, ConsoleError> {
    let provider = c.provider(&call.deployment)?;
    let now = c.now();
    let opts = blocking(move || provider.begin_assertion(Timestamp(now), &[])).await??;
    Ok(Reply::json(request_json(&opts)))
}

async fn signin_finish(
    c: &Console,
    call: &mut Call,
    _headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let req: Assertion = parse(body)?;
    let complete = req.request()?;
    let provider = c.provider(&call.deployment)?;
    let now = c.now();
    let subject = blocking(move || provider.complete(&complete, Timestamp(now))).await??;
    let credential = subject
        .authenticator
        .0
        .strip_prefix("passkey:")
        .unwrap_or_default()
        .to_string();
    let owner =
        c.0.store
            .passkey_owner(&call.deployment, &credential)
            .await?
            .ok_or(ConsoleError::Passkey(AuthError::BadCredentials))?;
    call.actor = format!("owner:{owner}");
    let (session, cookie) = c.new_token();
    c.0.store
        .create_session(
            &call.deployment,
            &NewSession {
                hash: &session,
                owner_id: &owner,
                touched: true,
                must_enrol: false,
            },
            now,
        )
        .await?;
    Ok(Reply {
        body: json!({ "owner": owner }),
        cookie: Some(cookie),
    })
}

async fn stepup_begin(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    _body: &[u8],
) -> Result<Reply, ConsoleError> {
    let s = c.session(call, headers, Need::Signed).await?;
    let allow = c
        .owner_credentials(&call.deployment, &s.row.owner_id)
        .await?;
    let provider = c.provider(&call.deployment)?;
    let now = c.now();
    let principal = PrincipalId(s.row.owner_id.clone());
    let opts =
        blocking(move || provider.begin_step_up(Timestamp(now), &principal, &allow)).await??;
    Ok(Reply::json(request_json(&opts)))
}

async fn stepup_finish(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let s = c.session(call, headers, Need::Signed).await?;
    let req: Assertion = parse(body)?;
    let CompleteRequest::PasskeyAssertion {
        challenge,
        credential_id,
        client_data_json,
        authenticator_data,
        signature,
    } = req.request()?
    else {
        return Err(ConsoleError::BadRequest("not an assertion".into()));
    };
    let provider = c.provider(&call.deployment)?;
    let now = c.now();
    let (subject, principal) = blocking(move || {
        provider.complete_step_up(
            &challenge,
            &credential_id,
            &client_data_json,
            &authenticator_data,
            &signature,
            Timestamp(now),
        )
    })
    .await??;
    let credential = subject
        .authenticator
        .0
        .strip_prefix("passkey:")
        .unwrap_or_default()
        .to_string();
    let holder =
        c.0.store
            .passkey_owner(&call.deployment, &credential)
            .await?;
    if principal.0 != s.row.owner_id || holder.as_deref() != Some(s.row.owner_id.as_str()) {
        return Err(ConsoleError::Passkey(AuthError::BadCredentials));
    }
    c.0.store.touched(&s.hash, now).await?;
    Ok(Reply::json(json!({
        "touchedAt": now,
        "freshUntil": now + STEP_UP_WINDOW_SECS,
    })))
}

async fn recovery_redeem(
    c: &Console,
    call: &mut Call,
    _headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let req: CodeBody = parse(body)?;
    let code = normalise_code(&req.code).ok_or(ConsoleError::CodeRefused)?;
    let (lookup, secret) = code.split_at(LOOKUP_LEN);
    let (owner, hash) =
        c.0.store
            .live_recovery(&call.deployment, lookup)
            .await?
            .ok_or(ConsoleError::CodeRefused)?;
    let hasher = c.0.hasher.clone();
    let secret = secret.to_string();
    if !blocking(move || hasher.verify(&secret, &hash)).await? {
        return Err(ConsoleError::CodeRefused);
    }
    call.actor = format!("owner:{owner}");
    let now = c.now();
    let (session, cookie) = c.new_token();
    if !c
        .0
        .store
        .use_recovery(&call.deployment, lookup, &owner, &session, now)
        .await?
    {
        return Err(ConsoleError::CodeRefused);
    }
    Ok(Reply {
        body: json!({ "owner": owner, "mustEnrol": true }),
        cookie: Some(cookie),
    })
}

async fn passkeys_begin(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    _body: &[u8],
) -> Result<Reply, ConsoleError> {
    let s = c.session(call, headers, Need::Enrol).await?;
    let now = c.now();
    if !s.row.must_enrol && !fresh_touch(&s.row, now) {
        return Err(ConsoleError::StepUpNeeded {
            window_secs: STEP_UP_WINDOW_SECS,
        });
    }
    let exclude = c
        .owner_credentials(&call.deployment, &s.row.owner_id)
        .await?;
    let provider = c.provider(&call.deployment)?;
    let binding = format!("session:{}", s.hash);
    let opts = blocking(move || {
        provider.begin_registration_bound(Timestamp(now), &exclude, Some(&binding))
    })
    .await??;
    Ok(Reply::json(creation_json(
        &opts,
        "Ordo console",
        "Console owner",
    )))
}

async fn passkeys_finish(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let s = c.session(call, headers, Need::Enrol).await?;
    let now = c.now();
    if !s.row.must_enrol && !fresh_touch(&s.row, now) {
        return Err(ConsoleError::StepUpNeeded {
            window_secs: STEP_UP_WINDOW_SECS,
        });
    }
    let r: Registration = parse(body)?;
    let provider = c.provider(&call.deployment)?;
    let challenge = ChallengeId(r.challenge.clone());
    let client_data = b64("clientDataJSON", &r.client_data_json)?;
    let attestation = b64("attestationObject", &r.attestation_object)?;
    let pending = blocking(move || {
        provider.verify_registration(&challenge, &client_data, &attestation, Timestamp(now))
    })
    .await??;
    if pending.binding() != Some(format!("session:{}", s.hash).as_str()) {
        return Err(ConsoleError::BadRequest(
            "that passkey ceremony was begun by another session".into(),
        ));
    }
    let owner = s.row.owner_id.clone();
    if s.row.must_enrol {
        let (codes, rows) = c.mint_recovery_set().await?;
        c.0.store
            .add_passkey(
                &call.deployment,
                &owner,
                pending.stored(),
                Some(&rows),
                &s.hash,
                now,
            )
            .await?;
        call.detail = "after a recovery code; new recovery set".into();
        return Ok(Reply::json(
            json!({ "owner": owner, "recoveryCodes": codes }),
        ));
    }
    c.0.store
        .add_passkey(
            &call.deployment,
            &owner,
            pending.stored(),
            None,
            &s.hash,
            now,
        )
        .await?;
    Ok(Reply::json(json!({ "owner": owner })))
}

async fn recovery_renew(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    _body: &[u8],
) -> Result<Reply, ConsoleError> {
    let s = c.session(call, headers, Need::Touch).await?;
    let (codes, rows) = c.mint_recovery_set().await?;
    c.0.store
        .renew_recovery(&call.deployment, &s.row.owner_id, &rows, c.now())
        .await?;
    Ok(Reply::json(json!({ "recoveryCodes": codes })))
}

async fn owners_invite(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    _body: &[u8],
) -> Result<Reply, ConsoleError> {
    let s = c.session(call, headers, Need::Touch).await?;
    if c.0.store.owners(&call.deployment).await?.len() >= MAX_OWNERS {
        return Err(ConsoleError::OwnerLimit { max: MAX_OWNERS });
    }
    let now = c.now();
    let code = mint_code();
    let expires = now + INVITE_TTL_SECS;
    c.0.store
        .issue_setup_code(
            &call.deployment,
            CodeKind::Invite,
            &setup_code_hash(&code),
            None,
            Some(expires),
            &format!("owner:{}", s.row.owner_id),
            now,
        )
        .await?;
    Ok(Reply::json(
        json!({ "code": display_code(&code), "expiresAt": expires }),
    ))
}

async fn owners_remove(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let _ = c.session(call, headers, Need::Touch).await?;
    let req: OwnerBody = parse(body)?;
    call.detail = format!("owner={}", req.owner);
    c.0.store
        .remove_owner(&call.deployment, &req.owner, c.now())
        .await?;
    Ok(Reply::json(json!({ "removed": req.owner })))
}

async fn signout(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    _body: &[u8],
) -> Result<Reply, ConsoleError> {
    let s = c.session(call, headers, Need::Enrol).await?;
    c.0.store
        .end_session(&s.hash, "signed out", c.now())
        .await?;
    Ok(Reply {
        body: json!({ "signedOut": true }),
        cookie: Some(c.clear_cookie()),
    })
}

async fn session_get(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let s = match c.who(call, headers, Need::Enrol).await? {
        Who::Owner(s) => s,
        Who::Person { claims, .. } => {
            return Ok(Reply::json(json!({
                "owner": claims.sub,
                "person": claims.sub,
                "kind": claims.kind.as_str(),
                "sid": claims.sid,
                "mustEnrol": false,
                "createdAt": claims.iat,
                "idleEndsAt": claims.exp,
                "endsAt": claims.exp,
                "touchFreshUntil": claims.touched_at.map(|t| t + STEP_UP_WINDOW_SECS),
            })))
        }
    };
    let now = c.now();
    let cfg = &c.0.cfg;
    Ok(Reply::json(json!({
        "owner": s.row.owner_id,
        "mustEnrol": s.row.must_enrol,
        "createdAt": s.row.created_at,
        "idleEndsAt": now + cfg.session_idle_secs,
        "endsAt": s.row.created_at + cfg.session_max_secs,
        "touchFreshUntil": s.row.touched_at.map(|t| t + STEP_UP_WINDOW_SECS),
    })))
}

async fn owners_list(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let _ = c.session(call, headers, Need::Signed).await?;
    let owners = c.0.store.owners(&call.deployment).await?;
    Ok(Reply::json(json!({ "owners": owners })))
}

/// The runtime, or the door-only refusal that names the setting.
fn supervisor(c: &Console) -> Result<&Runtime, ConsoleError> {
    c.0.runtime.as_ref().ok_or(ConsoleError::DoorOnly)
}

/// The first sync with the supervisor (`crate::runtime`), at boot. The
/// supervisor binds its socket before it starts any child, so it answers
/// at once in the one image; should it not, boot goes on and the sync is
/// tried again every 5 s in the background, said in the log.
async fn sync_until_done(rt: Runtime) {
    match rt.sync().await {
        Ok(s) => eprintln!("ordo-console: runtime in step with the supervisor: {s:?}"),
        Err(e) => {
            eprintln!(
                "ordo-console: the supervisor did not answer the first sync ({e}); trying \
                 again every 5 s"
            );
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    match rt.sync().await {
                        Ok(s) => {
                            eprintln!("ordo-console: runtime in step with the supervisor: {s:?}");
                            return;
                        }
                        Err(e) => eprintln!("ordo-console: the sync failed again: {e}"),
                    }
                }
            });
        }
    }
}

fn revision_json(r: &RevisionRow) -> Value {
    let settings: Value = serde_json::from_str(&r.settings).unwrap_or_else(|_| json!({}));
    let secrets: Value = serde_json::from_str(&r.secret_refs).unwrap_or_else(|_| json!([]));
    json!({
        "number": r.number,
        "settings": settings,
        "secrets": secrets,
        "actor": r.actor,
        "reason": r.reason,
        "outcome": r.outcome,
        "detail": r.detail,
        "createdAt": r.created_at,
        "decidedAt": r.decided_at,
    })
}

/// The Runtime screen: status, the live revision, the last log lines. In
/// door-only mode it says the console has no runtime control.
async fn runtime_get(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    match &c.0.runtime {
        Some(s) => Ok(Reply::json(s.describe().await?)),
        None => Ok(Reply::json(door_only_runtime(c, &who).await)),
    }
}

/// The Runtime screen in door-only mode (`plan-console.md` 9.2): read-only,
/// what the deployment reports through its door (`/health/ready`, and
/// `/health/deep` with its boot record, G16), and nothing about the process,
/// which the console does not see. A silent door is "not answering", no more.
async fn door_only_runtime(c: &Console, who: &Who) -> Value {
    let message = ConsoleError::DoorOnly.to_string();
    let Ok(door) = who.door(c) else {
        return json!({
            "mode": "door-only",
            "message": message,
            "answering": false,
            "reported": null,
            "doorError": format!(
                "no door address is set; set {DOOR_URL_ENV} to the deployment's door"
            ),
        });
    };
    let health = door.health().await;
    let answering = health["ready"]["error"] != "deployment_not_answering";
    let boot = health["deep"].get("boot").cloned().unwrap_or(Value::Null);
    json!({
        "mode": "door-only",
        "message": message,
        "answering": answering,
        "reported": { "ready": health["ready"], "deep": health["deep"], "boot": boot },
        "doorError": null,
    })
}

async fn revisions_list(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let _ = c.who(call, headers, Need::Signed).await?;
    let rows = c.0.store.revisions(&call.deployment, 100).await?;
    let live = c.0.store.live_revision(&call.deployment).await?;
    Ok(Reply::json(json!({
        "live": live,
        "revisions": rows.iter().map(revision_json).collect::<Vec<_>>(),
    })))
}

async fn runtime_apply(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let sup = supervisor(c)?;
    let patch: Patch = parse(body)?;
    let actor = who.actor();
    let result = sup.apply(&patch, &actor).await;
    call.detail = match &result {
        Ok(a) => format!("revision={} {}", a.revision, a.diff.summary()),
        Err(ConsoleError::SettingsRefused { revision, .. }) => format!("revision={revision}"),
        Err(ConsoleError::RolledBack { revision, to, .. }) => {
            format!("revision={revision} rolled_back_to={to}")
        }
        Err(_) => String::new(),
    };
    let a = result?;
    Ok(Reply::json(json!({
        "revision": a.revision,
        "diff": a.diff,
        "status": sup.status().await?,
    })))
}

#[derive(Deserialize)]
struct RollbackBody {
    revision: u64,
    #[serde(default)]
    reason: String,
}

async fn runtime_rollback(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let sup = supervisor(c)?;
    let req: RollbackBody = parse(body)?;
    let actor = who.actor();
    let result = sup.rollback(req.revision, &req.reason, &actor).await;
    call.detail = match &result {
        Ok(a) => format!("to={} revision={}", req.revision, a.revision),
        Err(ConsoleError::RolledBack { revision, to, .. }) => {
            format!(
                "to={} revision={revision} rolled_back_to={to}",
                req.revision
            )
        }
        Err(_) => format!("to={}", req.revision),
    };
    let a = result?;
    Ok(Reply::json(json!({
        "revision": a.revision,
        "diff": a.diff,
        "status": sup.status().await?,
    })))
}

async fn runtime_restart(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    _body: &[u8],
) -> Result<Reply, ConsoleError> {
    let _ = c.who(call, headers, Need::Touch).await?;
    let sup = supervisor(c)?;
    sup.restart().await?;
    let status = sup.status().await?;
    call.detail = format!("revision={}", status.live);
    Ok(Reply::json(json!({ "status": status })))
}

/// The link to the deployment, or the refusal that names the setting.
fn door_link(c: &Console) -> Result<&DoorLink, ConsoleError> {
    c.0.link.as_ref().ok_or_else(|| {
        ConsoleError::setting(
            DOOR_URL_ENV,
            "is not set, so the console does not know where the deployment is",
            format!(
                "Set {DOOR_URL_ENV} to the deployment's address, such as https://app.example.com"
            ),
        )
    })
}

/// The console's link to the deployment, for the page. The console key
/// itself never leaves the server.
async fn door_status(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    if who.is_person() {
        // No console key and no link: the console calls the door as the
        // person. All the screen needs is whether the door answers.
        let link = door_link(c)?;
        let reachable = who.door(c)?.reachable().await;
        return Ok(Reply::json(json!({
            "linked": false,
            "via": "identity-service",
            "door": link.client().url().display(),
            "reachable": reachable.is_ok(),
            "reachError": reachable.err().map(|e| e.to_string()),
        })));
    }
    Ok(Reply::json(door_link(c)?.status().await?))
}

#[derive(Deserialize)]
struct LinkBegin {
    kind: String,
}

#[derive(Deserialize)]
struct LinkFinish {
    kind: String,
    credential: Value,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct DoorAssertion {
    credential: Value,
    /// The `session` the begin answered.
    session: String,
}

#[derive(Deserialize, Default)]
struct KeyName {
    #[serde(default)]
    name: Option<String>,
}

/// The console key's name at the deployment, from the page or the
/// console's own address.
fn key_name(c: &Console, asked: Option<String>) -> Result<String, ConsoleError> {
    let name = asked
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("console {}", c.0.cfg.origin));
    if name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err(ConsoleError::BadRequest(
            "`name` must be 1 to 64 printable characters".into(),
        ));
    }
    Ok(name)
}

/// Linking, leg one (G14, 8.2): the deployment's passkey options. Needs
/// the console owner's own touch: linking hands the console the
/// deployment.
async fn door_link_begin(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let _ = c.session(call, headers, Need::Touch).await?;
    let req: LinkBegin = parse(body)?;
    let kind = LinkKind::parse(&req.kind)?;
    call.detail = format!("kind={}", req.kind);
    Ok(Reply::json(door_link(c)?.link_begin(kind).await?))
}

/// Linking, leg two: the owner's credential at the deployment, then the
/// console key.
async fn door_link_finish(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let _ = c.session(call, headers, Need::Touch).await?;
    let req: LinkFinish = parse(body)?;
    let kind = LinkKind::parse(&req.kind)?;
    let name = key_name(c, req.name)?;
    let key = door_link(c)?
        .link_finish(kind, &req.credential, &name)
        .await?;
    call.detail = format!("kind={} key={key}", req.kind);
    Ok(Reply::json(json!({ "linked": true, "key": key })))
}

/// G15, leg one: the deployment's passkey options for the console's
/// session there.
async fn door_stepup_begin(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    _body: &[u8],
) -> Result<Reply, ConsoleError> {
    let _ = c.session(call, headers, Need::Signed).await?;
    Ok(Reply::json(door_link(c)?.step_up_begin().await?))
}

/// G15, leg two: the owner's assertion, relayed to the deployment.
async fn door_stepup_finish(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let _ = c.session(call, headers, Need::Signed).await?;
    let req: DoorAssertion = parse(body)?;
    Ok(Reply::json(
        door_link(c)?
            .step_up_finish(&req.credential, &req.session)
            .await?,
    ))
}

/// A new console key, enrolled with the deployment session the owner just
/// touched for; the old one is revoked.
async fn door_rotate(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let _ = c.session(call, headers, Need::Touch).await?;
    let req: KeyName = parse(body)?;
    let name = key_name(c, req.name)?;
    let key = door_link(c)?.rotate(&name).await?;
    call.detail = format!("key={key}");
    Ok(Reply::json(json!({ "key": key })))
}

/// Revoke the console key at the deployment and forget it here.
async fn door_unlink(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    _body: &[u8],
) -> Result<Reply, ConsoleError> {
    let _ = c.session(call, headers, Need::Touch).await?;
    let key = door_link(c)?.unlink().await?;
    call.detail = format!("key={key}");
    Ok(Reply::json(json!({ "linked": false, "revoked": key })))
}

/// The console keys the deployment holds, read with the console key.
async fn door_keys(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let _ = c.session(call, headers, Need::Signed).await?;
    Ok(Reply::json(json!({ "keys": door_link(c)?.keys().await? })))
}

/// Health from the door (`plan-console.md` 4.1).
async fn door_health(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let link = door_link(c)?;
    let mut out = who.door(c)?.health().await;
    out["link"] = if who.is_person() {
        json!({"linked": false, "via": "identity-service", "door": link.client().url().display()})
    } else {
        link.status().await?
    };
    Ok(Reply::json(out))
}

#[derive(Deserialize)]
struct KeyId {
    id: String,
}

/// Revoke another console's key at the deployment, with the owner's touch
/// there.
async fn door_revoke(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let _ = c.session(call, headers, Need::Touch).await?;
    let req: KeyId = parse(body)?;
    call.detail = format!("key={}", req.id);
    let row = door_link(c)?.revoke_key(&req.id).await?;
    Ok(Reply::json(json!({ "revoked": row })))
}

/// Forget a link the deployment revoked from its side.
async fn door_forget(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    _body: &[u8],
) -> Result<Reply, ConsoleError> {
    let _ = c.session(call, headers, Need::Touch).await?;
    let key = door_link(c)?.forget().await?;
    call.detail = format!("key={key}");
    Ok(Reply::json(json!({ "linked": false, "forgotten": key })))
}

/// The base64url form a test or a page sends a credential id in.
pub fn credential_b64(id: &[u8]) -> String {
    b64url_encode(id)
}
