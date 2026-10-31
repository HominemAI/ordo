//! The console's settings, read once at boot (`plan-console.md` 5.2, 5.3,
//! 9.4). No I/O here, so every refusal happens before the first dial, and
//! every refusal names the setting and the value to set.
//!
//! Read from a lookup function rather than the process environment, so a
//! test hands in the variables it means without touching the process's.

use std::net::SocketAddr;
use std::path::PathBuf;

use ordo_exec::dburl::DatabaseUrlError;
use ordo_exec::PoolConfig;

use crate::error::ConsoleError;

/// The console's own database. Required: there is no default database.
pub const DATABASE_URL_ENV: &str = "ORDO_CONSOLE_DATABASE_URL";
/// The file form of [`DATABASE_URL_ENV`]: the URL holds the role's
/// password, so a secret store mounts it here. Setting both is refused.
pub const DATABASE_URL_FILE_ENV: &str = "ORDO_CONSOLE_DATABASE_URL_FILE";
/// How many connections the console holds to its database.
pub const DATABASE_POOL_ENV: &str = "ORDO_CONSOLE_DATABASE_POOL_SIZE";
/// The origin the console page is served from, `https://host[:port]`.
pub const ORIGIN_ENV: &str = "ORDO_CONSOLE_ORIGIN";
/// The console's own WebAuthn relying party id. Defaults to the origin's host.
pub const RP_ID_ENV: &str = "ORDO_CONSOLE_RP_ID";
/// Where the server listens.
pub const LISTEN_ENV: &str = "ORDO_CONSOLE_LISTEN";
/// The console's volume: the first-run setup code is written here.
pub const DATA_DIR_ENV: &str = "ORDO_CONSOLE_DATA_DIR";
/// Where the built page is: `index.html` and `assets/`.
pub const PAGE_DIR_ENV: &str = "ORDO_CONSOLE_PAGE_DIR";
/// The handle of the one deployment open source manages (`plan-console.md` 11).
pub const DEPLOYMENT_ENV: &str = "ORDO_CONSOLE_DEPLOYMENT";
/// The deployment's door, when the console has been told it.
pub const DOOR_URL_ENV: &str = "ORDO_CONSOLE_DOOR_URL";
/// `on` lets the console reach the door over plain http to an address that
/// is not loopback. Off by default.
pub const DOOR_PLAIN_HTTP_ENV: &str = "ORDO_CONSOLE_DOOR_PLAIN_HTTP";
/// The deployment's own space, where the console key and the claim live.
/// Defaults to the deployment's `ORDO_DEPLOYMENT_SPACE`.
pub const DOOR_SPACE_ENV: &str = "ORDO_CONSOLE_DOOR_SPACE";
/// `ordo-host`'s own default space, when nothing names one.
pub const DEFAULT_DOOR_SPACE: &str = "conformance";
/// The door in the one image when nothing names it: the deployment's
/// address beside the console.
pub const DEFAULT_SUPERVISED_DOOR: &str = "http://127.0.0.1:8080";
/// How long a session may sit unused before it ends.
pub const SESSION_IDLE_ENV: &str = "ORDO_CONSOLE_SESSION_IDLE_SECS";
/// How long a session may live, used or not.
pub const SESSION_MAX_ENV: &str = "ORDO_CONSOLE_SESSION_MAX_SECS";
/// No longer read for anything but a note (step 7): the mode follows
/// [`SUPERVISOR_SOCKET_ENV`]. Kept so an old compose file that sets it is
/// told so, and boots.
pub const MODE_ENV: &str = "ORDO_CONSOLE_MODE";
/// The supervisor's socket (`plan-identity-service.md` 3.2). Set, the
/// console manages the deployment through it (supervise mode); unset, it
/// reaches the deployment through its door alone (door-only mode). The
/// supervisor sets it for the console it runs.
pub const SUPERVISOR_SOCKET_ENV: &str = "ORDO_CONSOLE_SUPERVISOR_SOCKET";
/// Days after which the console key is due for rotation (`plan-console.md`
/// 8.2). A rotation needs the owner's touch at the deployment, so the
/// console says the key is due and asks; it never rotates alone.
pub const KEY_ROTATE_DAYS_ENV: &str = "ORDO_CONSOLE_KEY_ROTATE_DAYS";
/// 30 days.
pub const DEFAULT_KEY_ROTATE_DAYS: u64 = 30;
/// The longest a deployment owner may set: a year.
pub const MAX_KEY_ROTATE_DAYS: u64 = 365;
/// The console's sealing key: managed secrets are sealed with it.
pub const KEY_FILE_ENV: &str = "ORDO_CONSOLE_KEY_FILE";

/// The deployment's own database settings, as `ordo-host` reads them. The
/// console reads them only to refuse pointing at the same database.
pub const DEPLOYMENT_PG_HOST_ENV: &str = "ORDO_PG_HOST";
pub const DEPLOYMENT_PG_PORT_ENV: &str = "ORDO_PG_PORT";
pub const DEPLOYMENT_PG_DB_ENV: &str = "ORDO_PG_DB";

pub const DEFAULT_LISTEN: &str = "127.0.0.1:8081";
pub const DEFAULT_DATA_DIR: &str = "/var/lib/ordo-console";
/// Where the image puts the built page (`docker/Dockerfile`).
pub const DEFAULT_PAGE_DIR: &str = "/usr/share/ordo-console/page";
pub const DEFAULT_DEPLOYMENT: &str = "main";
pub const DEFAULT_POOL_SIZE: usize = 4;
/// Thirty minutes unused ends a session.
pub const DEFAULT_SESSION_IDLE_SECS: u64 = 30 * 60;
/// Twelve hours ends a session however busy it is.
pub const DEFAULT_SESSION_MAX_SECS: u64 = 12 * 60 * 60;
/// A runtime or credential change needs a passkey touch this recent, as at
/// the door (`plan-console.md` 5.3 item 5). Not a setting.
pub const STEP_UP_WINDOW_SECS: u64 = 300;

/// The console's own database: where it is, and what it is called.
#[derive(Clone, Debug)]
pub struct DatabaseSettings {
    pub pool: PoolConfig,
}

impl DatabaseSettings {
    /// Read `ORDO_CONSOLE_DATABASE_URL` and refuse one that names the
    /// deployment's database.
    pub fn from_vars(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, ConsoleError> {
        let url = ordo_env::secret_from(DATABASE_URL_ENV, get, &|p| std::fs::read_to_string(p))
            .map_err(|e| {
                ConsoleError::setting_caused(
                    DATABASE_URL_FILE_ENV,
                    e.to_string(),
                    format!(
                        "Set only one of {DATABASE_URL_ENV} and {DATABASE_URL_FILE_ENV}, and \
                         point {DATABASE_URL_FILE_ENV} at a readable file that holds the URL"
                    ),
                    e,
                )
            })?
            .filter(|v| !v.trim().is_empty());
        let Some(url) = url else {
            return Err(ConsoleError::setting(
                DATABASE_URL_ENV,
                "is not set, and the console has no default database",
                format!(
                    "Set {DATABASE_URL_ENV} (or {DATABASE_URL_FILE_ENV}, the file form) to a \
                     database of the console's own, for example \
                     postgres://ordo_console@db.internal:5432/ordo_console"
                ),
            ));
        };
        let size = match get(DATABASE_POOL_ENV) {
            None => DEFAULT_POOL_SIZE,
            Some(v) => match v.trim().parse::<usize>() {
                Ok(n) if n >= 1 => n,
                _ => {
                    return Err(ConsoleError::setting(
                        DATABASE_POOL_ENV,
                        format!("is `{v}`, which is not a connection count"),
                        format!(
                            "Set {DATABASE_POOL_ENV} to a whole number of 1 or more, or unset \
                             it for {DEFAULT_POOL_SIZE}"
                        ),
                    ))
                }
            },
        };
        let pool = ordo_exec::dburl::parse_database_url(&url, size).map_err(|e| {
            let fix = match &e {
                DatabaseUrlError::NotAConnectionString(_) => format!(
                    "Set {DATABASE_URL_ENV} to postgres://user@host:port/database, or to \
                     key=value pairs (host=... port=... user=... dbname=...)"
                ),
                DatabaseUrlError::NoHost => {
                    format!("Add the database host to {DATABASE_URL_ENV}")
                }
                DatabaseUrlError::NoDatabase => format!(
                    "Add the database name to {DATABASE_URL_ENV}, for example .../ordo_console"
                ),
                DatabaseUrlError::NoUser => {
                    format!("Add the console's own database role to {DATABASE_URL_ENV}")
                }
            };
            ConsoleError::setting_caused(DATABASE_URL_ENV, e.to_string(), fix, e)
        })?;
        refuse_deployment_database(&pool, get)?;
        Ok(DatabaseSettings { pool })
    }
}

/// Boot refuses a console database that is the deployment's own
/// (`plan-console.md` 3): the console never opens the deployment's data.
///
/// Compared on what both settings say: the database name, the host and the
/// port. A loopback host is one host whatever it is called.
fn refuse_deployment_database(
    console: &PoolConfig,
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<(), ConsoleError> {
    let Some(db) = get(DEPLOYMENT_PG_DB_ENV) else {
        return Ok(());
    };
    let host = get(DEPLOYMENT_PG_HOST_ENV).unwrap_or_default();
    let port = get(DEPLOYMENT_PG_PORT_ENV)
        .and_then(|p| p.trim().parse::<u16>().ok())
        .unwrap_or(5432);
    if ordo_exec::dburl::same_database(&db, &host, port, console) {
        return Err(ConsoleError::setting(
            DATABASE_URL_ENV,
            format!(
                "names database `{}` on {}:{}, which is the deployment's own database \
                 ({DEPLOYMENT_PG_DB_ENV})",
                console.db, console.host, console.port
            ),
            format!(
                "Create a database of the console's own, for example `CREATE DATABASE \
                 ordo_console`, and point {DATABASE_URL_ENV} at it"
            ),
        ));
    }
    Ok(())
}

/// Whether the console holds runtime control (`plan-identity-service.md`
/// 3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// [`SUPERVISOR_SOCKET_ENV`] is set: the console manages the deployment
    /// through the supervisor (the one image).
    Supervise,
    /// No socket: the console reaches the deployment through its door
    /// alone, and holds no way to start, stop or change it.
    DoorOnly,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Supervise => "supervise",
            Mode::DoorOnly => "door-only",
        }
    }
}

/// What the runtime reads: the console's sealing key, and where the
/// supervisor's socket is.
#[derive(Clone, Debug)]
pub struct RuntimeSettings {
    pub key_file: PathBuf,
    /// [`SUPERVISOR_SOCKET_ENV`]; `None` is door-only mode.
    pub socket: Option<PathBuf>,
}

/// Everything the console server reads at boot.
#[derive(Clone, Debug)]
pub struct ConsoleConfig {
    pub database: DatabaseSettings,
    pub listen: SocketAddr,
    pub origin: String,
    pub rp_id: String,
    pub data_dir: PathBuf,
    pub page_dir: PathBuf,
    pub deployment: String,
    pub door_url: Option<String>,
    /// The door, checked: `door_url`, or in supervise mode the child's
    /// default address. `None` in door-only mode with no address set.
    pub door: Option<crate::door::DoorUrl>,
    /// [`DOOR_SPACE_ENV`].
    pub door_space: String,
    /// The identity service (`plan-identity-service.md` 5.4), when
    /// `ORDO_IDENTITY_URL` is set: people sign in there, and the console
    /// verifies the tokens they bring. `None` keeps the console's own
    /// sign-in.
    pub identity: Option<crate::identity::IdentitySettings>,
    pub session_idle_secs: u64,
    pub session_max_secs: u64,
    /// [`KEY_ROTATE_DAYS_ENV`].
    pub key_rotate_days: u64,
    pub mode: Mode,
    pub runtime: RuntimeSettings,
}

impl ConsoleConfig {
    /// From the process environment.
    pub fn from_env() -> Result<ConsoleConfig, ConsoleError> {
        let mut cfg = ConsoleConfig::from_vars(&|k| std::env::var(k).ok())?;
        if std::env::var(DOOR_SPACE_ENV).is_err() {
            if let Ok(space) = std::env::var("ORDO_DEPLOYMENT_SPACE") {
                cfg.door_space = space;
            }
        }
        Ok(cfg)
    }

    /// From any lookup.
    pub fn from_vars(get: &dyn Fn(&str) -> Option<String>) -> Result<ConsoleConfig, ConsoleError> {
        let database = DatabaseSettings::from_vars(get)?;
        let Some(origin) = get(ORIGIN_ENV).map(|o| o.trim().trim_end_matches('/').to_string())
        else {
            return Err(ConsoleError::setting(
                ORIGIN_ENV,
                "is not set, and passkeys are bound to the origin the page is served from",
                format!("Set {ORIGIN_ENV} to the console's address, for example https://console.example.com"),
            ));
        };
        let host = origin_host(&origin).ok_or_else(|| {
            ConsoleError::setting(
                ORIGIN_ENV,
                format!("is `{origin}`, which is not scheme://host[:port]"),
                format!("Set {ORIGIN_ENV} to an origin such as https://console.example.com"),
            )
        })?;
        // Over plain http the session cookie loses `Secure` and `__Host-`,
        // so only a loopback host (local development) may use it.
        if origin.starts_with("http://") && !is_loopback_host(&host) {
            return Err(ConsoleError::setting(
                ORIGIN_ENV,
                format!(
                    "is `{origin}`, which is http:// on a host that is not loopback, and the \
                     session cookie would lose Secure and __Host-"
                ),
                format!(
                    "Set {ORIGIN_ENV} to an https:// origin such as https://console.example.com \
                     (a TLS proxy in front of the console), or use http://localhost for local \
                     development"
                ),
            ));
        }
        let rp_id = get(RP_ID_ENV).unwrap_or(host);
        let listen_text = get(LISTEN_ENV).unwrap_or_else(|| DEFAULT_LISTEN.to_string());
        let listen = listen_text.trim().parse::<SocketAddr>().map_err(|e| {
            ConsoleError::setting_caused(
                LISTEN_ENV,
                format!("is `{listen_text}`, which is not an address and port"),
                format!("Set {LISTEN_ENV} to an address such as 0.0.0.0:8081, or unset it for {DEFAULT_LISTEN}"),
                e,
            )
        })?;
        let data_dir = PathBuf::from(get(DATA_DIR_ENV).unwrap_or_else(|| DEFAULT_DATA_DIR.into()));
        let page_dir = PathBuf::from(get(PAGE_DIR_ENV).unwrap_or_else(|| DEFAULT_PAGE_DIR.into()));
        let deployment = get(DEPLOYMENT_ENV).unwrap_or_else(|| DEFAULT_DEPLOYMENT.into());
        if !valid_handle(&deployment) {
            return Err(ConsoleError::setting(
                DEPLOYMENT_ENV,
                format!("is `{deployment}`, which is not a deployment handle"),
                format!(
                    "Set {DEPLOYMENT_ENV} to lowercase letters, digits and dashes, at most 63, \
                     or unset it for `{DEFAULT_DEPLOYMENT}`"
                ),
            ));
        }
        let identity = crate::identity::IdentitySettings::from_vars(get, &deployment)?;
        let session_idle_secs = secs(get, SESSION_IDLE_ENV, DEFAULT_SESSION_IDLE_SECS)?;
        let session_max_secs = secs(get, SESSION_MAX_ENV, DEFAULT_SESSION_MAX_SECS)?;
        let key_rotate_days = match get(KEY_ROTATE_DAYS_ENV) {
            None => DEFAULT_KEY_ROTATE_DAYS,
            Some(v) => match v.trim().parse::<u64>() {
                Ok(n) if (1..=MAX_KEY_ROTATE_DAYS).contains(&n) => n,
                _ => {
                    return Err(ConsoleError::setting(
                        KEY_ROTATE_DAYS_ENV,
                        format!("is `{v}`, which is not a whole number of days from 1 to {MAX_KEY_ROTATE_DAYS}"),
                        format!(
                            "Set {KEY_ROTATE_DAYS_ENV} to 1 to {MAX_KEY_ROTATE_DAYS}, or unset it \
                             for {DEFAULT_KEY_ROTATE_DAYS}"
                        ),
                    ))
                }
            },
        };
        // The one place the mode is settled: the supervisor's socket.
        let socket = get(SUPERVISOR_SOCKET_ENV)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        let mode = if socket.is_some() {
            Mode::Supervise
        } else {
            Mode::DoorOnly
        };
        if let Some(m) = get(MODE_ENV).filter(|m| !m.trim().is_empty()) {
            eprintln!(
                "ordo-console: {MODE_ENV} is `{}` and is no longer read: the console is in {} \
                 mode because {SUPERVISOR_SOCKET_ENV} is {}. Remove the line",
                m.trim(),
                mode.as_str(),
                if socket.is_some() { "set" } else { "unset" }
            );
        }
        let door_url = get(DOOR_URL_ENV).filter(|v| !v.trim().is_empty());
        let plain_ok = match get(DOOR_PLAIN_HTTP_ENV).as_deref().map(str::trim) {
            None | Some("") | Some("off") => false,
            Some("on") => true,
            Some(other) => {
                return Err(ConsoleError::setting(
                    DOOR_PLAIN_HTTP_ENV,
                    format!("is `{other}`"),
                    format!("Set {DOOR_PLAIN_HTTP_ENV} to `on` or `off`, or unset it for `off`"),
                ))
            }
        };
        let door = match (&door_url, mode) {
            (Some(u), _) => Some(crate::door::DoorUrl::parse(u, plain_ok)?),
            (None, Mode::Supervise) => Some(crate::door::DoorUrl::parse(
                DEFAULT_SUPERVISED_DOOR,
                plain_ok,
            )?),
            (None, Mode::DoorOnly) => None,
        };
        let door_space = get(DOOR_SPACE_ENV)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_DOOR_SPACE.to_string());
        let runtime = RuntimeSettings {
            key_file: get(KEY_FILE_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(|| data_dir.join("console.key")),
            socket,
        };
        Ok(ConsoleConfig {
            database,
            listen,
            origin,
            rp_id,
            data_dir,
            page_dir,
            deployment,
            door_url,
            door,
            door_space,
            identity,
            session_idle_secs,
            session_max_secs,
            key_rotate_days,
            mode,
            runtime,
        })
    }

    /// Whether the page is served over https, which decides the cookie's
    /// `Secure` flag and `__Host-` prefix.
    pub fn secure_origin(&self) -> bool {
        self.origin.starts_with("https://")
    }
}

fn secs(
    get: &dyn Fn(&str) -> Option<String>,
    var: &'static str,
    default: u64,
) -> Result<u64, ConsoleError> {
    match get(var) {
        None => Ok(default),
        Some(v) => match v.trim().parse::<u64>() {
            Ok(n) if n >= 60 => Ok(n),
            _ => Err(ConsoleError::setting(
                var,
                format!("is `{v}`, which is not a number of seconds of 60 or more"),
                format!(
                    "Set {var} to a number of seconds of 60 or more, or unset it for {default}"
                ),
            )),
        },
    }
}

/// `https://host[:port]` to `host`.
fn origin_host(origin: &str) -> Option<String> {
    let rest = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))?;
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    let host = match rest.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => rest,
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// `localhost`, an IPv4 address in 127.0.0.0/8, or IPv6 `::1`.
fn is_loopback_host(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    bare == "localhost"
        || bare.ends_with(".localhost")
        || bare
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// A deployment handle: what a route carries (`/d/<handle>/...`).
pub fn valid_handle(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 63
        && h.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !h.starts_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(rotate: Option<&str>) -> Result<ConsoleConfig, ConsoleError> {
        let vars = std::collections::HashMap::from([
            (DATABASE_URL_ENV, "host=/tmp user=console dbname=c"),
            (ORIGIN_ENV, "https://console.example.com"),
            (MODE_ENV, "door-only"),
        ]);
        ConsoleConfig::from_vars(&|k| {
            if k == KEY_ROTATE_DAYS_ENV {
                return rotate.map(str::to_string);
            }
            vars.get(k).map(|v| v.to_string())
        })
    }

    fn with_origin(origin: &str) -> Result<ConsoleConfig, ConsoleError> {
        ConsoleConfig::from_vars(&|k| match k {
            k if k == DATABASE_URL_ENV => Some("host=/tmp user=console dbname=c".into()),
            k if k == ORIGIN_ENV => Some(origin.into()),
            k if k == MODE_ENV => Some("door-only".into()),
            _ => None,
        })
    }

    #[test]
    fn http_origin_is_refused_unless_loopback() {
        for ok in [
            "https://console.example.com",
            "http://localhost:8081",
            "http://127.0.0.1:8081",
            "http://[::1]:8081",
        ] {
            assert!(with_origin(ok).is_ok(), "{ok} should be accepted");
        }
        for bad in [
            "http://console.example.com",
            "http://10.0.0.5:8081",
            "http://localhost.example.com",
        ] {
            let err = with_origin(bad).expect_err(bad).to_string();
            assert!(err.contains(ORIGIN_ENV), "{bad}: {err}");
        }
    }

    fn db(vars: &[(&str, &str)], files: &[(&str, &str)]) -> Result<DatabaseSettings, ConsoleError> {
        let mut vars: std::collections::HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let dir = std::env::temp_dir().join(format!("ordo-console-dburl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in files {
            let path = dir.join(name);
            std::fs::write(&path, text).unwrap();
            vars.insert(name.to_string(), path.display().to_string());
        }
        DatabaseSettings::from_vars(&|k| vars.get(k).cloned())
    }

    #[test]
    fn the_database_url_file_form_works_and_refuses_naming_the_file_setting() {
        let url = "postgres://console:pw@console-db:5432/ordo_console\n";
        let ok = db(&[], &[(DATABASE_URL_FILE_ENV, url)]).unwrap();
        assert_eq!(ok.pool.db, "ordo_console");
        assert_eq!(ok.pool.host, "console-db");
        let both = db(
            &[(DATABASE_URL_ENV, "host=/tmp user=c dbname=c")],
            &[(DATABASE_URL_FILE_ENV, url)],
        )
        .unwrap_err();
        assert_eq!(both.setting_name(), Some(DATABASE_URL_FILE_ENV));
        let empty = db(&[], &[(DATABASE_URL_FILE_ENV, "\n")]).unwrap_err();
        assert_eq!(empty.setting_name(), Some(DATABASE_URL_FILE_ENV));
        let missing = db(&[(DATABASE_URL_FILE_ENV, "/nonexistent/url")], &[]).unwrap_err();
        assert_eq!(missing.setting_name(), Some(DATABASE_URL_FILE_ENV));
    }

    #[test]
    fn the_rotation_schedule_defaults_to_30_days_and_names_its_setting() {
        assert_eq!(with(None).unwrap().key_rotate_days, 30);
        assert_eq!(with(Some("7")).unwrap().key_rotate_days, 7);
        for bad in ["0", "366", "soon"] {
            let e = with(Some(bad)).unwrap_err();
            assert_eq!(e.setting_name(), Some(KEY_ROTATE_DAYS_ENV), "{bad}");
        }
    }
}
