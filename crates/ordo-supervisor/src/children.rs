//! The container's variables, split among the 3 children
//! (`plan-identity-service.md` 3.1), and the identity defaults filled from
//! the console's settings.
//!
//! Each child gets a cleared environment holding only its own settings.
//! Its secrets reach it as files in its own folder under the run folder,
//! never as values in its environment.
//!
//! - The identity service: `ORDO_IDENTITY_*`, the proxy settings it shares
//!   with the door, and the deployment's database address (it refuses to
//!   share that database). Its database URL and the console's, for the
//!   import, are secrets.
//! - The console: `ORDO_CONSOLE_*` without the runtime settings it no
//!   longer reads, the identity wiring, and the deployment's database
//!   address. Its database URL is a secret.
//! - The deployment: its live revision ([`crate::spec`]), plus the identity
//!   wiring when the revision does not set it.
//!
//! **The identity defaults** (3.1, 6.2). When the deployment owner leaves
//! them unset, the supervisor fills:
//! - where browsers reach the service, by `ORDO_SUPERVISOR_IDENTITY_ADDRESS`
//!   (board `upgrading-with-an-old-compose-file`, ask 1, recommended):
//!   `console-path`, the default, makes `ORDO_IDENTITY_ORIGIN` the console's
//!   own origin and `ORDO_IDENTITY_BASE_PATH` `/identity`, and the console
//!   passes that path through, so an upgraded compose file needs no new
//!   port; the service's guessing limits then read the visitor from
//!   `X-Forwarded-For`, trusting the console on loopback. `own-port` makes
//!   it `ORDO_CONSOLE_ORIGIN` with port 8082, as the plan first said;
//! - `ORDO_IDENTITY_RP_ID` from `ORDO_CONSOLE_RP_ID`, or the console
//!   origin's host, so a passkey made at the console still signs in;
//! - `ORDO_IDENTITY_IMPORT_CONSOLE_URL` from the console's database URL,
//!   so the service moves the console's owners in at its first start;
//! - `ORDO_IDENTITY_CLIENT_ORIGINS` from `ORDO_CONSOLE_ORIGIN`, so the
//!   console's page may ask the service for tokens. The plan names 3; this
//!   fourth is what makes console sign-in work with no new line.
//!
//! A value the deployment owner set always wins. An upgraded compose file
//! therefore needs no identity line but the service's own database.

use std::collections::BTreeMap;
use std::path::Path;

use crate::config::ChildName;
use crate::spec::{Secret, Spec};

pub const CONSOLE_ORIGIN: &str = "ORDO_CONSOLE_ORIGIN";
pub const CONSOLE_RP_ID: &str = "ORDO_CONSOLE_RP_ID";
pub const CONSOLE_DATABASE_URL: &str = "ORDO_CONSOLE_DATABASE_URL";
pub const CONSOLE_MODE: &str = "ORDO_CONSOLE_MODE";
pub const CONSOLE_DOOR_URL: &str = "ORDO_CONSOLE_DOOR_URL";
/// Where the console finds the supervisor's socket; set by the supervisor.
pub const CONSOLE_SUPERVISOR_SOCKET: &str = "ORDO_CONSOLE_SUPERVISOR_SOCKET";

pub const IDENTITY_URL: &str = "ORDO_IDENTITY_URL";
pub const IDENTITY_ORIGIN: &str = "ORDO_IDENTITY_ORIGIN";
pub const IDENTITY_RP_ID: &str = "ORDO_IDENTITY_RP_ID";
pub const IDENTITY_LISTEN: &str = "ORDO_IDENTITY_LISTEN";
pub const IDENTITY_DEPLOYMENT: &str = "ORDO_IDENTITY_DEPLOYMENT";
pub const IDENTITY_PLAIN_HTTP: &str = "ORDO_IDENTITY_PLAIN_HTTP";
pub const IDENTITY_CLIENT_ORIGINS: &str = "ORDO_IDENTITY_CLIENT_ORIGINS";
pub const IDENTITY_DATABASE_URL: &str = "ORDO_IDENTITY_DATABASE_URL";
pub const IDENTITY_IMPORT_CONSOLE_URL: &str = "ORDO_IDENTITY_IMPORT_CONSOLE_URL";
pub const IDENTITY_BASE_PATH: &str = "ORDO_IDENTITY_BASE_PATH";
/// `console-path` (the default) or `own-port` (module docs).
pub const IDENTITY_ADDRESS: &str = "ORDO_SUPERVISOR_IDENTITY_ADDRESS";
/// The base path the console passes through in `console-path` mode.
pub const DEFAULT_BASE_PATH: &str = "/identity";
pub const PROXY_HEADER: &str = "ORDO_PROXY_HEADER";
pub const PROXY_TRUSTED: &str = "ORDO_PROXY_TRUSTED";
/// The console, as the identity service sees it in `console-path` mode.
const LOOPBACK: &str = "127.0.0.1/32,::1/128";

/// The identity service's port in the image (2.3).
pub const IDENTITY_PORT: u16 = 8082;

/// The console's runtime settings: it no longer supervises, so they are
/// the supervisor's now ([`crate::config`] reads the old names).
const CONSOLE_RUNTIME: [&str; 6] = [
    "ORDO_CONSOLE_CHILD_USER",
    "ORDO_CONSOLE_CHILD_GRACE_SECS",
    "ORDO_CONSOLE_CHILD_READY_SECS",
    "ORDO_CONSOLE_HOST_BIN",
    "ORDO_CONSOLE_RUN_DIR",
    CONSOLE_MODE,
];

/// The deployment's database address. The console and the identity
/// service read it only to refuse to share that database.
const DEPLOYMENT_DB: [&str; 3] = ["ORDO_PG_HOST", "ORDO_PG_PORT", "ORDO_PG_DB"];

/// One value the supervisor filled, and what it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filled {
    pub name: &'static str,
    pub value: String,
    pub from: String,
}

/// Every child's start, as built at boot. The deployment's settings come
/// from its live revision; `wiring` is what it is given on top.
#[derive(Clone, Debug)]
pub struct Split {
    pub identity: Spec,
    pub console: Spec,
    /// The identity wiring the door and the console read.
    pub wiring: BTreeMap<String, String>,
    pub filled: Vec<Filled>,
    /// What the deployment owner should know: a file the supervisor could
    /// not read, a setting it overrode.
    pub notes: Vec<String>,
    /// The identity service has a database, so it is started and wired.
    pub identity_enabled: bool,
}

fn lookup<'a>(env: &'a [(String, String)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |k: &str| {
        env.iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.clone())
            .filter(|v| !v.trim().is_empty())
    }
}

/// `scheme`, `host` (with brackets for IPv6) and port of an origin.
fn split_origin(origin: &str) -> Option<(&str, &str)> {
    let o = origin.trim().trim_end_matches('/');
    let (scheme, rest) = o.split_once("://")?;
    if rest.is_empty() || rest.contains(['/', '@', '?', '#']) {
        return None;
    }
    let host = if rest.starts_with('[') {
        let end = rest.find(']')?;
        &rest[..=end]
    } else {
        rest.split(':').next()?
    };
    (!host.is_empty()).then_some((scheme, host))
}

/// The identity defaults (module docs). Each only when unset.
pub fn identity_defaults(env: &[(String, String)]) -> Vec<Filled> {
    let get = lookup(env);
    let mut out = Vec::new();
    let console_origin = get(CONSOLE_ORIGIN);
    let parts = console_origin.as_deref().and_then(split_origin);
    let under_console = get(IDENTITY_ADDRESS).as_deref().map(str::trim) != Some("own-port");
    if get(IDENTITY_ORIGIN).is_none() {
        match (parts, &console_origin) {
            (Some(_), Some(o)) if under_console => {
                out.push(Filled {
                    name: IDENTITY_ORIGIN,
                    value: o.trim().trim_end_matches('/').to_string(),
                    from: format!("{CONSOLE_ORIGIN}, as {IDENTITY_ADDRESS} is console-path"),
                });
                if get(IDENTITY_BASE_PATH).is_none() {
                    out.push(Filled {
                        name: IDENTITY_BASE_PATH,
                        value: DEFAULT_BASE_PATH.to_string(),
                        from: format!("{IDENTITY_ADDRESS}, console-path"),
                    });
                }
                // The console passes each visitor through from loopback:
                // the service reads the visitor from X-Forwarded-For.
                if get(PROXY_HEADER).is_none() {
                    out.push(Filled {
                        name: PROXY_HEADER,
                        value: "X-Forwarded-For".to_string(),
                        from: format!("{IDENTITY_ADDRESS}, console-path"),
                    });
                }
                out.push(Filled {
                    name: PROXY_TRUSTED,
                    value: match get(PROXY_TRUSTED) {
                        Some(t) => format!("{},{LOOPBACK}", t.trim().trim_end_matches(',')),
                        None => LOOPBACK.to_string(),
                    },
                    from: format!("{IDENTITY_ADDRESS}, console-path, with the console on loopback"),
                });
            }
            (Some((scheme, host)), _) => {
                out.push(Filled {
                    name: IDENTITY_ORIGIN,
                    value: format!("{scheme}://{host}:{IDENTITY_PORT}"),
                    from: format!("{CONSOLE_ORIGIN}, port {IDENTITY_PORT}"),
                });
            }
            _ => {}
        }
    }
    if get(IDENTITY_RP_ID).is_none() {
        if let Some(rp) = get(CONSOLE_RP_ID) {
            out.push(Filled {
                name: IDENTITY_RP_ID,
                value: rp.trim().to_string(),
                from: CONSOLE_RP_ID.to_string(),
            });
        } else if let Some((_, host)) = parts {
            out.push(Filled {
                name: IDENTITY_RP_ID,
                value: host
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_string(),
                from: format!("the host of {CONSOLE_ORIGIN}"),
            });
        }
    }
    if get(IDENTITY_CLIENT_ORIGINS).is_none() {
        if let Some(o) = &console_origin {
            out.push(Filled {
                name: IDENTITY_CLIENT_ORIGINS,
                value: o.trim().trim_end_matches('/').to_string(),
                from: CONSOLE_ORIGIN.to_string(),
            });
        }
    }
    let import_set = get(IDENTITY_IMPORT_CONSOLE_URL).is_some()
        || get(&format!("{IDENTITY_IMPORT_CONSOLE_URL}_FILE")).is_some();
    if !import_set {
        if get(CONSOLE_DATABASE_URL).is_some() {
            out.push(Filled {
                name: IDENTITY_IMPORT_CONSOLE_URL,
                value: String::new(),
                from: CONSOLE_DATABASE_URL.to_string(),
            });
        } else if get(&format!("{CONSOLE_DATABASE_URL}_FILE")).is_some() {
            out.push(Filled {
                name: IDENTITY_IMPORT_CONSOLE_URL,
                value: String::new(),
                from: format!("{CONSOLE_DATABASE_URL}_FILE"),
            });
        }
    }
    out
}

/// Where the door and the console reach the identity service: loopback,
/// on the port it listens on.
fn identity_url(get: &dyn Fn(&str) -> Option<String>) -> String {
    let port = get(IDENTITY_LISTEN)
        .and_then(|l| l.trim().parse::<std::net::SocketAddr>().ok())
        .map(|a| a.port())
        .unwrap_or(IDENTITY_PORT);
    format!("http://127.0.0.1:{port}")
}

/// A secret for a child: the value when it is given, or the file's content
/// copied into the child's own folder. `read` reads a file the owner
/// mounted; when the supervisor may not read it, the path is passed on as
/// it is and a note says so.
fn secret_of(
    get: &dyn Fn(&str) -> Option<String>,
    name: &str,
    read: &dyn Fn(&Path) -> std::io::Result<String>,
    now: u64,
    notes: &mut Vec<String>,
) -> Option<Secret> {
    if let Some(v) = get(name) {
        return Some(Secret::Value {
            value: v,
            changed_at: now,
        });
    }
    let file_var = format!("{name}_FILE");
    let path = get(&file_var)?;
    match read(Path::new(path.trim())) {
        Ok(text) => Some(Secret::Value {
            value: text.trim().to_string(),
            changed_at: now,
        }),
        Err(e) => {
            notes.push(format!(
                "{file_var} is `{path}`, which the supervisor could not read ({e}); the child \
                 is given the path as it is. Make the file readable by the supervisor's user \
                 (65530 in the image), so each child gets a private copy"
            ));
            Some(Secret::File {
                path,
                changed_at: now,
            })
        }
    }
}

/// The database URL files the children read, by their `_FILE` settings.
pub const SECRET_FILE_SETTINGS: [&str; 3] = [
    "ORDO_CONSOLE_DATABASE_URL_FILE",
    "ORDO_IDENTITY_DATABASE_URL_FILE",
    "ORDO_IDENTITY_IMPORT_CONSOLE_URL_FILE",
];

/// The files among [`SECRET_FILE_SETTINGS`] the supervisor's own user may
/// not read: each is then read as a child's user, through the launcher.
pub fn unreadable_files(
    env: &[(String, String)],
    read: &dyn Fn(&Path) -> std::io::Result<Vec<u8>>,
) -> Vec<std::path::PathBuf> {
    let get = lookup(env);
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    for var in SECRET_FILE_SETTINGS {
        if let Some(p) = get(var) {
            let path = std::path::PathBuf::from(p.trim());
            let denied =
                matches!(read(&path), Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied);
            if denied && !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

/// A reader that answers first from the files already read as a child's
/// user, then from the file system.
pub fn reader<'a>(
    handed: &'a BTreeMap<std::path::PathBuf, String>,
    direct: &'a dyn Fn(&Path) -> std::io::Result<String>,
) -> impl Fn(&Path) -> std::io::Result<String> + 'a {
    move |p: &Path| match handed.get(p) {
        Some(text) => Ok(text.clone()),
        None => direct(p),
    }
}

/// Split the container's variables (module docs).
pub fn split(
    env: &[(String, String)],
    read: &dyn Fn(&Path) -> std::io::Result<String>,
    now: u64,
) -> Split {
    let get = lookup(env);
    let mut notes = Vec::new();
    let filled = identity_defaults(env);
    let value_of = |name: &str| -> Option<String> {
        get(name).or_else(|| {
            filled
                .iter()
                .find(|f| f.name == name && !f.value.is_empty())
                .map(|f| f.value.clone())
        })
    };

    let mut wiring = BTreeMap::new();
    wiring.insert(
        IDENTITY_URL.to_string(),
        get(IDENTITY_URL).unwrap_or_else(|| identity_url(&get)),
    );
    for name in [IDENTITY_ORIGIN, IDENTITY_DEPLOYMENT, IDENTITY_PLAIN_HTTP] {
        if let Some(v) = value_of(name) {
            wiring.insert(name.to_string(), v);
        }
    }

    // The identity service.
    let mut identity = Spec::default();
    for (k, v) in env {
        let secret_stem = [IDENTITY_DATABASE_URL, IDENTITY_IMPORT_CONSOLE_URL]
            .iter()
            .any(|s| k == s || k.strip_suffix("_FILE") == Some(s));
        if secret_stem || k == IDENTITY_URL || k == IDENTITY_PLAIN_HTTP {
            continue;
        }
        if k.starts_with("ORDO_IDENTITY_")
            || k == "ORDO_PROXY_HEADER"
            || k == "ORDO_PROXY_TRUSTED"
            || DEPLOYMENT_DB.contains(&k.as_str())
        {
            identity.settings.insert(k.clone(), v.clone());
        }
    }
    for f in &filled {
        if !f.value.is_empty() {
            identity
                .settings
                .insert(f.name.to_string(), f.value.clone());
        }
    }
    if let Some(s) = secret_of(&get, IDENTITY_DATABASE_URL, read, now, &mut notes) {
        identity.secrets.insert(IDENTITY_DATABASE_URL.into(), s);
    }
    let import_from = if filled.iter().any(|f| f.name == IDENTITY_IMPORT_CONSOLE_URL) {
        CONSOLE_DATABASE_URL
    } else {
        IDENTITY_IMPORT_CONSOLE_URL
    };
    if let Some(s) = secret_of(&get, import_from, read, now, &mut notes) {
        identity
            .secrets
            .insert(IDENTITY_IMPORT_CONSOLE_URL.into(), s);
    }

    // The console.
    let mut console = Spec::default();
    for (k, v) in env {
        if k == CONSOLE_DATABASE_URL
            || k.strip_suffix("_FILE") == Some(CONSOLE_DATABASE_URL)
            || CONSOLE_RUNTIME.contains(&k.as_str())
        {
            continue;
        }
        if k.starts_with("ORDO_CONSOLE_")
            || DEPLOYMENT_DB.contains(&k.as_str())
            || k == "ORDO_DEPLOYMENT_SPACE"
        {
            console.settings.insert(k.clone(), v.clone());
        }
    }
    if let Some(m) = get(CONSOLE_MODE) {
        notes.push(format!(
            "{CONSOLE_MODE} is `{m}` and is no longer read: the supervisor runs the \
             deployment, and the console manages it through the supervisor's socket \
             ({CONSOLE_SUPERVISOR_SOCKET}). Remove the line"
        ));
    }
    if get(CONSOLE_DOOR_URL).is_none() {
        let port = get(crate::spec::HOST_ADDR_ENV)
            .and_then(|a| a.trim().parse::<std::net::SocketAddr>().ok())
            .map(|a| a.port())
            .unwrap_or(8080);
        console
            .settings
            .insert(CONSOLE_DOOR_URL.into(), format!("http://127.0.0.1:{port}"));
    }
    for (k, v) in &wiring {
        console.settings.insert(k.clone(), v.clone());
    }
    // The console passes the base path through, so it reads it too.
    if let Some(b) = value_of(IDENTITY_BASE_PATH) {
        console.settings.insert(IDENTITY_BASE_PATH.into(), b);
    }
    if let Some(s) = secret_of(&get, CONSOLE_DATABASE_URL, read, now, &mut notes) {
        console.secrets.insert(CONSOLE_DATABASE_URL.into(), s);
    }

    // The identity service runs only with a database it was given, or
    // the opt-in to make one on the console's server (owner, 2026-10-30).
    // An upgraded deployment with neither runs as before: no identity
    // service, no wiring, and the console keeps its own sign-in.
    let identity_enabled = identity_database_named(&get);
    if !identity_enabled {
        notes.push(identity_off_note());
        if get(IDENTITY_URL).is_none() {
            wiring.clear();
            console
                .settings
                .retain(|k, _| !k.starts_with("ORDO_IDENTITY_"));
        }
    }

    Split {
        identity,
        console,
        wiring,
        filled,
        notes,
        identity_enabled,
    }
}

/// The opt-in to make the identity database on the console's server.
pub const SHARE_CONSOLE_DB_SERVER: &str = "ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER";

/// Whether the identity service has a database: named, in either form, or
/// the opt-in set to `true`.
fn identity_database_named(get: &dyn Fn(&str) -> Option<String>) -> bool {
    get(IDENTITY_DATABASE_URL).is_some()
        || get(&format!("{IDENTITY_DATABASE_URL}_FILE")).is_some()
        || get(SHARE_CONSOLE_DB_SERVER).as_deref().map(str::trim) == Some("true")
}

/// What the boot log says when the identity service is not started.
pub fn identity_off_note() -> String {
    format!(
        "the identity service is not started: neither {IDENTITY_DATABASE_URL} (or \
         {IDENTITY_DATABASE_URL}_FILE) nor {SHARE_CONSOLE_DB_SERVER}=true is set. The console \
         keeps its own sign-in, as before the one image. Set one of them to move sign-in to \
         the identity service; a later, announced release will require one"
    )
}

/// A child's environment from its spec: its settings, and `<NAME>_FILE`
/// for each secret. A value is written to `dir` (made empty first), mode
/// 0600 and `user`'s; a mounted file is named as it is.
pub fn write_env(
    spec: &Spec,
    dir: &Path,
    user: Option<crate::config::ChildUser>,
) -> std::io::Result<BTreeMap<String, String>> {
    crate::child::fresh_dir(dir)?;
    let mut env = spec.settings.clone();
    for (name, secret) in &spec.secrets {
        let path = match secret {
            Secret::File { path, .. } => path.clone(),
            Secret::Value { value, .. } => {
                let file = dir.join(name);
                crate::child::write_secret(&file, value.as_bytes(), user)?;
                file.display().to_string()
            }
        };
        env.insert(format!("{name}_FILE"), path);
    }
    Ok(env)
}

impl Split {
    pub fn fixed(&self, name: ChildName) -> Option<&Spec> {
        match name {
            ChildName::Identity => Some(&self.identity),
            ChildName::Console => Some(&self.console),
            ChildName::Deployment => None,
        }
    }

    /// One log line per filled default, naming its source, never a value
    /// that holds a secret.
    pub fn describe_filled(&self) -> Vec<String> {
        self.filled
            .iter()
            .map(|f| {
                if f.value.is_empty() {
                    format!("{} unset: filled from {}", f.name, f.from)
                } else {
                    format!("{} unset: filled as `{}` from {}", f.name, f.value, f.from)
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn no_file(_: &Path) -> std::io::Result<String> {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "denied",
        ))
    }

    /// The live notes v2 case (6.2): a console on localhost:8081 and
    /// nothing set for the service. The service is reached at
    /// localhost:8081/identity, through the console (board
    /// `upgrading-with-an-old-compose-file`, ask 1), under RP ID
    /// `localhost`, imports from the console's database, accepts the
    /// console's page, and trusts the console on loopback to name each
    /// visitor.
    #[test]
    fn the_identity_defaults_come_from_the_consoles_settings() {
        let e = env(&[
            (CONSOLE_ORIGIN, "http://localhost:8081"),
            (
                "ORDO_CONSOLE_DATABASE_URL_FILE",
                "/run/secrets/console-database-url",
            ),
            (
                IDENTITY_DATABASE_URL,
                "postgres://identity@db/ordo_identity",
            ),
        ]);
        let read = |p: &Path| {
            assert_eq!(p, Path::new("/run/secrets/console-database-url"));
            Ok("postgres://console@db/ordo_console\n".to_string())
        };
        let s = split(&e, &read, 5);
        let id = &s.identity.settings;
        assert_eq!(id[IDENTITY_ORIGIN], "http://localhost:8081");
        assert_eq!(id[IDENTITY_BASE_PATH], "/identity");
        assert_eq!(id[PROXY_HEADER], "X-Forwarded-For");
        assert_eq!(id[PROXY_TRUSTED], "127.0.0.1/32,::1/128");
        assert_eq!(s.console.settings[IDENTITY_BASE_PATH], "/identity");
        assert_eq!(id[IDENTITY_RP_ID], "localhost");
        assert_eq!(id[IDENTITY_CLIENT_ORIGINS], "http://localhost:8081");
        assert!(!id.contains_key(IDENTITY_IMPORT_CONSOLE_URL));
        assert_eq!(
            s.identity.secrets[IDENTITY_IMPORT_CONSOLE_URL],
            Secret::Value {
                value: "postgres://console@db/ordo_console".into(),
                changed_at: 5
            }
        );
        assert_eq!(
            s.console.secrets[CONSOLE_DATABASE_URL],
            s.identity.secrets[IDENTITY_IMPORT_CONSOLE_URL]
        );
        assert!(matches!(
            s.identity.secrets[IDENTITY_DATABASE_URL],
            Secret::Value { .. }
        ));
        assert_eq!(s.wiring[IDENTITY_URL], "http://127.0.0.1:8082");
        assert_eq!(s.wiring[IDENTITY_ORIGIN], "http://localhost:8081");
        assert!(!s.console.settings.contains_key(CONSOLE_MODE));
        assert_eq!(
            s.console.settings[CONSOLE_DOOR_URL],
            "http://127.0.0.1:8080"
        );
        assert!(s.notes.is_empty(), "{:?}", s.notes);
        // No secret value reaches a settings map.
        let text = format!("{:?} {:?}", s.identity.settings, s.console.settings);
        assert!(!text.contains("postgres://"), "{text}");
    }

    /// `ORDO_SUPERVISOR_IDENTITY_ADDRESS=own-port`, should the owner's
    /// answer flip ask 1: the service on its own port, as the plan first
    /// said. Under the console or on its own port, the RP ID is the
    /// console's, so the passkeys still sign in.
    #[test]
    fn a_console_rp_id_and_an_https_origin_keep_the_passkeys_relying_party() {
        let e = env(&[
            (CONSOLE_ORIGIN, "https://console.example.com"),
            (CONSOLE_RP_ID, "example.com"),
            (IDENTITY_ADDRESS, "own-port"),
        ]);
        let f = identity_defaults(&e);
        let get = |n: &str| f.iter().find(|x| x.name == n).map(|x| x.value.clone());
        assert_eq!(
            get(IDENTITY_ORIGIN).as_deref(),
            Some("https://console.example.com:8082")
        );
        assert_eq!(get(IDENTITY_BASE_PATH), None);
        assert_eq!(get(PROXY_TRUSTED), None);
        assert_eq!(get(IDENTITY_RP_ID).as_deref(), Some("example.com"));
        let v6 = identity_defaults(&env(&[
            (CONSOLE_ORIGIN, "http://[::1]:8081"),
            (IDENTITY_ADDRESS, "own-port"),
        ]));
        assert_eq!(v6[0].value, "http://[::1]:8082");
        assert_eq!(v6[1].value, "::1");

        // The default: under the console, the owner's own proxy list kept.
        let f = identity_defaults(&env(&[
            (CONSOLE_ORIGIN, "https://console.example.com/"),
            (CONSOLE_RP_ID, "example.com"),
            (PROXY_HEADER, "X-Forwarded-For"),
            (PROXY_TRUSTED, "10.0.0.0/8"),
        ]));
        let get = |n: &str| f.iter().find(|x| x.name == n).map(|x| x.value.clone());
        assert_eq!(
            get(IDENTITY_ORIGIN).as_deref(),
            Some("https://console.example.com")
        );
        assert_eq!(get(IDENTITY_BASE_PATH).as_deref(), Some("/identity"));
        assert_eq!(get(PROXY_HEADER), None, "the owner's header stands");
        assert_eq!(
            get(PROXY_TRUSTED).as_deref(),
            Some("10.0.0.0/8,127.0.0.1/32,::1/128")
        );
        assert_eq!(get(IDENTITY_RP_ID).as_deref(), Some("example.com"));
    }

    #[test]
    fn a_value_the_owner_set_always_wins() {
        let e = env(&[
            (CONSOLE_ORIGIN, "https://console.example.com"),
            (IDENTITY_ORIGIN, "https://id.console.example.com"),
            (IDENTITY_RP_ID, "console.example.com"),
            (IDENTITY_CLIENT_ORIGINS, "https://other.example.com"),
            (IDENTITY_IMPORT_CONSOLE_URL, "postgres://elsewhere/c"),
            (CONSOLE_DATABASE_URL, "postgres://console/c"),
        ]);
        assert!(identity_defaults(&e).is_empty());
        let s = split(&e, &no_file, 1);
        assert_eq!(
            s.identity.settings[IDENTITY_ORIGIN],
            "https://id.console.example.com"
        );
        assert_eq!(
            s.identity.secrets[IDENTITY_IMPORT_CONSOLE_URL],
            Secret::Value {
                value: "postgres://elsewhere/c".into(),
                changed_at: 1
            }
        );
    }

    /// Upgrade (step 7): the old image's setup left console-database-url
    /// readable by the console alone (65532, mode 0400). The supervisor
    /// finds it denied, the launcher reads it as the console's user, and
    /// the console and the identity service's import each get a private
    /// copy of the value, never the path to the console-only file.
    #[test]
    fn a_url_only_the_console_may_read_is_read_as_the_console_and_handed_on() {
        let e = env(&[
            (CONSOLE_ORIGIN, "http://localhost:8081"),
            (
                "ORDO_CONSOLE_DATABASE_URL_FILE",
                "/run/secrets/console-database-url",
            ),
            (SHARE_CONSOLE_DB_SERVER, "true"),
        ]);
        let denied = |p: &Path| -> std::io::Result<Vec<u8>> {
            assert_eq!(p, Path::new("/run/secrets/console-database-url"));
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "mode 0400, owner 65532",
            ))
        };
        let files = unreadable_files(&e, &denied);
        assert_eq!(
            files,
            vec![std::path::PathBuf::from(
                "/run/secrets/console-database-url"
            )]
        );
        // As the console's user, through the launcher.
        let handed: BTreeMap<std::path::PathBuf, String> = [(
            files[0].clone(),
            "postgres://console:pw@console-db/ordo_console\n".to_string(),
        )]
        .into();
        let read = reader(&handed, &no_file);
        let s = split(&e, &read, 3);
        let want = Secret::Value {
            value: "postgres://console:pw@console-db/ordo_console".into(),
            changed_at: 3,
        };
        assert_eq!(s.console.secrets[CONSOLE_DATABASE_URL], want);
        assert_eq!(s.identity.secrets[IDENTITY_IMPORT_CONSOLE_URL], want);
        assert!(s.notes.is_empty(), "{:?}", s.notes);
        // Another error than a refusal is not read as anyone else.
        let missing = |_: &Path| -> std::io::Result<Vec<u8>> {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"))
        };
        assert!(unreadable_files(&e, &missing).is_empty());
    }

    /// Owner, 2026-10-30: the identity database is explicit, or opted
    /// into. An upgraded compose file with neither runs as before: the
    /// identity service is not started, nothing is wired to it, the console
    /// keeps its own sign-in, and the note names both settings.
    #[test]
    fn with_no_identity_database_named_nothing_is_wired_and_both_settings_are_named() {
        let old = env(&[
            (CONSOLE_ORIGIN, "http://localhost:8081"),
            (CONSOLE_DATABASE_URL, "postgres://console@db/ordo_console"),
        ]);
        let s = split(&old, &no_file, 1);
        assert!(!s.identity_enabled);
        assert!(s.wiring.is_empty(), "{:?}", s.wiring);
        assert!(
            !s.console
                .settings
                .keys()
                .any(|k| k.starts_with("ORDO_IDENTITY_")),
            "{:?}",
            s.console.settings
        );
        let note = s.notes.iter().find(|n| n.contains("not started")).unwrap();
        assert!(note.contains(IDENTITY_DATABASE_URL), "{note}");
        assert!(note.contains(SHARE_CONSOLE_DB_SERVER), "{note}");
        assert!(note.contains("announced release"), "{note}");

        for (k, v) in [
            (SHARE_CONSOLE_DB_SERVER, "true"),
            (
                IDENTITY_DATABASE_URL,
                "postgres://identity@db/ordo_identity",
            ),
            (
                "ORDO_IDENTITY_DATABASE_URL_FILE",
                "/run/secrets/identity-database-url",
            ),
        ] {
            let mut e = old.clone();
            e.push((k.to_string(), v.to_string()));
            let s = split(&e, &no_file, 1);
            assert!(s.identity_enabled, "{k}");
            assert_eq!(
                s.console.settings[IDENTITY_URL], "http://127.0.0.1:8082",
                "{k}"
            );
        }
        let mut e = old.clone();
        e.push((SHARE_CONSOLE_DB_SERVER.to_string(), "false".to_string()));
        assert!(!split(&e, &no_file, 1).identity_enabled);
    }

    #[test]
    fn an_unreadable_file_is_passed_on_and_said() {
        let e = env(&[
            (CONSOLE_ORIGIN, "http://localhost:8081"),
            ("ORDO_CONSOLE_DATABASE_URL_FILE", "/run/secrets/c"),
            (CONSOLE_MODE, "supervise"),
            ("ORDO_CONSOLE_CHILD_USER", "65533:65533"),
        ]);
        let s = split(&e, &no_file, 1);
        assert_eq!(
            s.console.secrets[CONSOLE_DATABASE_URL],
            Secret::File {
                path: "/run/secrets/c".into(),
                changed_at: 1
            }
        );
        assert_eq!(s.notes.len(), 4, "{:?}", s.notes);
        assert!(s.notes.iter().any(|n| n.contains(CONSOLE_MODE)));
        assert!(!s.console.settings.contains_key("ORDO_CONSOLE_CHILD_USER"));
        assert!(!s.console.settings.contains_key(CONSOLE_MODE));
    }
}
