//! The identity service's settings, read once at boot
//! (`plan-identity-service.md` 2.3, with the owner's answers of section 10).
//! No I/O but reading a `_FILE` setting, so every refusal happens before the
//! first dial, and every refusal names the setting and the value to set.
//!
//! Read from a lookup function rather than the process environment, so a
//! test hands in the variables it means.

use std::net::SocketAddr;
use std::path::PathBuf;

use ordo_exec::dburl::{parse_database_url, same_database, DatabaseUrlError};
use ordo_exec::PoolConfig;
use ordo_identity::origin::Origin;

use crate::error::IdentityError;
use crate::throttle::{Cidr, Proxy};

pub const LISTEN_ENV: &str = "ORDO_IDENTITY_LISTEN";
pub const ORIGIN_ENV: &str = "ORDO_IDENTITY_ORIGIN";
/// Where browsers reach the service under its origin, such as `/identity`
/// when the console passes it through on the console's own origin (board,
/// `upgrading-with-an-old-compose-file`, ask 1). Empty is the root. The
/// service answers at the root as well, for the door and the console on
/// loopback.
pub const BASE_PATH_ENV: &str = "ORDO_IDENTITY_BASE_PATH";
pub const RP_ID_ENV: &str = "ORDO_IDENTITY_RP_ID";
pub const DATABASE_URL_ENV: &str = "ORDO_IDENTITY_DATABASE_URL";
pub const DATABASE_URL_FILE_ENV: &str = "ORDO_IDENTITY_DATABASE_URL_FILE";
pub const DATABASE_POOL_ENV: &str = "ORDO_IDENTITY_DATABASE_POOL_SIZE";
/// Opt-in, `false` by default (owner, 2026-10-30: "Database needs to be
/// explicit, or there is an optional ENV / setting that enables using the
/// same db server"). `true`: with no [`DATABASE_URL_ENV`], the service makes
/// its own database beside the console's, on the console's Postgres
/// server, with the console's role, at first start. `false`: no database
/// named refuses the boot.
pub const DATABASE_BESIDE_CONSOLE_ENV: &str = "ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER";
/// The database the service makes beside the console's.
pub const BESIDE_CONSOLE_DB: &str = "ordo_identity";
pub const IMPORT_CONSOLE_URL_ENV: &str = "ORDO_IDENTITY_IMPORT_CONSOLE_URL";
pub const IMPORT_CONSOLE_URL_FILE_ENV: &str = "ORDO_IDENTITY_IMPORT_CONSOLE_URL_FILE";
pub const KEY_FILE_ENV: &str = "ORDO_IDENTITY_KEY_FILE";
/// The service's volume: its key file by default, and the first invite.
pub const DATA_DIR_ENV: &str = "ORDO_IDENTITY_DATA_DIR";
pub const CLIENT_ORIGINS_ENV: &str = "ORDO_IDENTITY_CLIENT_ORIGINS";
pub const DEPLOYMENT_ENV: &str = "ORDO_IDENTITY_DEPLOYMENT";
pub const PROVISIONER_KEY_ENV: &str = "ORDO_IDENTITY_PROVISIONER_KEY";
pub const INVITE_DAYS_ENV: &str = "ORDO_IDENTITY_INVITE_DAYS";
pub const TOKEN_SECS_ENV: &str = "ORDO_IDENTITY_TOKEN_SECS";
pub const SESSION_IDLE_ENV: &str = "ORDO_IDENTITY_SESSION_IDLE_HOURS";
pub const SESSION_MAX_ENV: &str = "ORDO_IDENTITY_SESSION_MAX_HOURS";
pub const GUESSES_PER_ADDRESS_ENV: &str = "ORDO_IDENTITY_GUESSES_PER_ADDRESS";
pub const GUESSES_SHARED_ENV: &str = "ORDO_IDENTITY_GUESSES_SHARED";
/// The door's proxy-mode settings, read the same way (`ordo-host` `proxy.rs`).
pub const PROXY_HEADER_ENV: &str = "ORDO_PROXY_HEADER";
pub const PROXY_TRUSTED_ENV: &str = "ORDO_PROXY_TRUSTED";
pub const KEY_ROTATE_DAYS_ENV: &str = "ORDO_IDENTITY_KEY_ROTATE_DAYS";

/// The console's own database setting: boot refuses to share it.
pub const CONSOLE_DATABASE_URL_ENV: &str = "ORDO_CONSOLE_DATABASE_URL";
/// The deployment's own database settings: boot refuses to share it.
pub const DEPLOYMENT_PG_HOST_ENV: &str = "ORDO_PG_HOST";
pub const DEPLOYMENT_PG_PORT_ENV: &str = "ORDO_PG_PORT";
pub const DEPLOYMENT_PG_DB_ENV: &str = "ORDO_PG_DB";

pub const DEFAULT_LISTEN: &str = "127.0.0.1:8082";
pub const DEFAULT_DATA_DIR: &str = "/var/lib/ordo-identity";
/// The console's `DEFAULT_DEPLOYMENT`.
pub const DEFAULT_DEPLOYMENT: &str = "main";
pub const DEFAULT_POOL_SIZE: usize = 4;

/// A whole-number setting: its default and its range (Q9).
#[derive(Debug, Clone, Copy)]
pub struct Range {
    pub default: u64,
    pub min: u64,
    pub max: u64,
    pub unit: &'static str,
}

pub const INVITE_DAYS: Range = Range {
    default: 7,
    min: 1,
    max: 30,
    unit: "days",
};
pub const TOKEN_SECS: Range = Range {
    default: 600,
    min: 60,
    max: 3600,
    unit: "seconds",
};
/// 7 days. The owner expects 24 or 12 to be common, so the unit is hours.
pub const SESSION_IDLE_HOURS: Range = Range {
    default: 168,
    min: 1,
    max: 720,
    unit: "hours",
};
/// 30 days, measured from sign-in; never below the idle value.
pub const SESSION_MAX_HOURS: Range = Range {
    default: 720,
    min: 1,
    max: 2160,
    unit: "hours",
};
/// Tries of one kind of secret one address may make per minute.
pub const GUESSES_PER_ADDRESS: Range = Range {
    default: 30,
    min: 1,
    max: 100_000,
    unit: "tries per minute",
};
/// Tries of one kind everyone together may make per minute: 20 times the
/// per-address default. It only stops a flood.
pub const GUESSES_SHARED: Range = Range {
    default: 600,
    min: 1,
    max: 10_000_000,
    unit: "tries per minute",
};

/// The guessing limits (`throttle`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub per_address: u64,
    pub shared: u64,
}

/// 0 turns scheduled rotation off; `rotate-keys` still works.
pub const KEY_ROTATE_DAYS: Range = Range {
    default: 90,
    min: 0,
    max: 365,
    unit: "days",
};

/// Everything the service reads at boot.
#[derive(Clone, Debug)]
pub struct IdentityConfig {
    pub database: PoolConfig,
    /// The console's database, when [`IdentityConfig::database`] was made
    /// from it ([`DATABASE_BESIDE_CONSOLE_ENV`]): the service creates its
    /// own database on that server before it opens it.
    pub create_beside: Option<PoolConfig>,
    pub listen: SocketAddr,
    /// `ORDO_IDENTITY_ORIGIN`, as written once checked: the token's `iss`.
    pub origin: String,
    /// [`BASE_PATH_ENV`], checked: empty, or `/` and plain segments.
    pub base_path: String,
    pub rp_id: String,
    pub data_dir: PathBuf,
    pub key_file: PathBuf,
    /// The console's database, read once at first start (6.1). `None`
    /// means no import.
    pub import_console: Option<PoolConfig>,
    /// Pages that may ask for a token from a browser, with credentials.
    pub client_origins: Vec<Origin>,
    /// The deployment handle tokens name in `aud`.
    pub deployment: String,
    /// The provisioning service's Ed25519 public key (5.1).
    pub provisioner_key: Option<[u8; 32]>,
    pub invite_days: u64,
    pub token_secs: u64,
    pub session_idle_hours: u64,
    pub session_max_hours: u64,
    pub key_rotate_days: u64,
    pub limits: Limits,
    /// Proxy mode; `None` is off and the socket's address is the visitor's.
    pub proxy: Option<Proxy>,
}

impl IdentityConfig {
    /// From the process environment.
    pub fn from_env() -> Result<IdentityConfig, IdentityError> {
        IdentityConfig::from_vars(&|k| std::env::var(k).ok())
    }

    /// From any lookup.
    pub fn from_vars(
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<IdentityConfig, IdentityError> {
        let pool_size = pool_size(get)?;
        let own = database_setting(
            get,
            DATABASE_URL_ENV,
            DATABASE_URL_FILE_ENV,
            pool_size,
            "postgres://ordo_identity@db.internal:5432/ordo_identity",
        )?;
        let import_console = database_setting(
            get,
            IMPORT_CONSOLE_URL_ENV,
            IMPORT_CONSOLE_URL_FILE_ENV,
            1,
            "postgres://ordo_console@db.internal:5432/ordo_console",
        )?;
        let beside = match get(DATABASE_BESIDE_CONSOLE_ENV).as_deref().map(str::trim) {
            None | Some("") | Some("false") => false,
            Some("true") => true,
            Some(other) => {
                return Err(IdentityError::setting(
                    DATABASE_BESIDE_CONSOLE_ENV,
                    format!("is `{other}`, which is not `true` or `false`"),
                    format!(
                        "Set {DATABASE_BESIDE_CONSOLE_ENV} to `true` or `false`, or unset it \
                         for `false`"
                    ),
                ))
            }
        };
        let (database, create_beside) = match (own, &import_console) {
            (Some(db), _) => (db, None),
            // Opted in: the service makes its own database beside the
            // console's, with the console's role, on the console's server.
            (None, Some(console)) if beside => (
                PoolConfig {
                    db: BESIDE_CONSOLE_DB.to_string(),
                    size: pool_size,
                    ..console.clone()
                },
                Some(console.clone()),
            ),
            (None, _) => {
                return Err(IdentityError::setting(
                    DATABASE_URL_ENV,
                    "is not set, and the identity service has no default database",
                    format!(
                        "Set {DATABASE_URL_ENV} (or {DATABASE_URL_FILE_ENV}, the file form) to a \
                         database of the identity service's own, for example \
                         postgres://ordo_identity@db.internal:5432/ordo_identity, or set \
                         {DATABASE_BESIDE_CONSOLE_ENV}=true (with {IMPORT_CONSOLE_URL_ENV}) so \
                         the service makes {BESIDE_CONSOLE_DB} on the console's database server"
                    ),
                ))
            }
        };
        refuse_shared_database(&database, import_console.as_ref(), get)?;

        let origin_text = get(ORIGIN_ENV)
            .map(|o| o.trim().to_string())
            .filter(|o| !o.is_empty())
            .ok_or_else(|| {
                IdentityError::setting(
                    ORIGIN_ENV,
                    "is not set, and passkeys and the token's issuer are bound to the address \
                     people open the identity service at",
                    format!(
                        "Set {ORIGIN_ENV} to that address, for example \
                         https://console.example.com:8082"
                    ),
                )
            })?;
        let origin = Origin::parse(&origin_text).ok_or_else(|| {
            IdentityError::setting(
                ORIGIN_ENV,
                format!("is `{origin_text}`, which is not scheme://host[:port]"),
                format!("Set {ORIGIN_ENV} to an origin such as https://console.example.com:8082"),
            )
        })?;
        // Over plain http the session cookie loses `Secure` and `__Host-`,
        // so only a loopback host (local development) may use it.
        if origin.scheme() == "http" && !origin.is_loopback() {
            return Err(IdentityError::setting(
                ORIGIN_ENV,
                format!(
                    "is `{origin_text}`, which is http:// on a host that is not loopback, and \
                     the session cookie would lose Secure and __Host-"
                ),
                format!(
                    "Set {ORIGIN_ENV} to an https:// origin (a TLS proxy in front of the \
                     service), or use http://localhost for local development"
                ),
            ));
        }
        let rp_id = get(RP_ID_ENV)
            .map(|r| r.trim().to_ascii_lowercase())
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| origin.host().to_string());
        refuse_origin_outside_rp_id(&origin, &rp_id)?;
        let base_text = get(BASE_PATH_ENV).unwrap_or_default();
        let base_path = ordo_identity::origin::base_path(&base_text).map_err(|why| {
            IdentityError::setting(
                BASE_PATH_ENV,
                format!("is `{base_text}`, and {why}"),
                format!(
                    "Set {BASE_PATH_ENV} to a path such as /identity, or unset it to serve at \
                     the root"
                ),
            )
        })?;

        let listen_text = get(LISTEN_ENV).unwrap_or_else(|| DEFAULT_LISTEN.to_string());
        let listen = listen_text.trim().parse::<SocketAddr>().map_err(|e| {
            IdentityError::setting_caused(
                LISTEN_ENV,
                format!("is `{listen_text}`, which is not an address and port"),
                format!(
                    "Set {LISTEN_ENV} to an address such as 0.0.0.0:8082, or unset it for \
                     {DEFAULT_LISTEN}"
                ),
                e,
            )
        })?;
        let data_dir = PathBuf::from(
            get(DATA_DIR_ENV)
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_DATA_DIR.into()),
        );
        let key_file = get(KEY_FILE_ENV)
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("identity.key"));
        let client_origins = client_origins(get)?;
        let deployment = get(DEPLOYMENT_ENV)
            .map(|d| d.trim().to_string())
            .unwrap_or_else(|| DEFAULT_DEPLOYMENT.into());
        if !valid_handle(&deployment) {
            return Err(IdentityError::setting(
                DEPLOYMENT_ENV,
                format!("is `{deployment}`, which is not a deployment handle"),
                format!(
                    "Set {DEPLOYMENT_ENV} to lowercase letters, digits and dashes, at most 63, \
                     or unset it for `{DEFAULT_DEPLOYMENT}`"
                ),
            ));
        }
        let provisioner_key = provisioner_key(get)?;
        let session_idle_hours = whole(get, SESSION_IDLE_ENV, SESSION_IDLE_HOURS)?;
        let session_max_hours = whole(get, SESSION_MAX_ENV, SESSION_MAX_HOURS)?;
        if session_max_hours < session_idle_hours {
            return Err(IdentityError::setting(
                SESSION_MAX_ENV,
                format!(
                    "is {session_max_hours}, which is below {SESSION_IDLE_ENV} \
                     ({session_idle_hours}), and a session cannot end later for being unused \
                     than it ends anyway"
                ),
                format!(
                    "Set {SESSION_MAX_ENV} to {session_idle_hours} or more, or lower \
                     {SESSION_IDLE_ENV}"
                ),
            ));
        }
        let per_address = whole(get, GUESSES_PER_ADDRESS_ENV, GUESSES_PER_ADDRESS)?;
        let shared = whole(get, GUESSES_SHARED_ENV, GUESSES_SHARED)?;
        if shared < per_address {
            return Err(IdentityError::setting(
                GUESSES_SHARED_ENV,
                format!(
                    "is {shared}, which is below {GUESSES_PER_ADDRESS_ENV} ({per_address}), \
                     and the ceiling over everyone would refuse an address before its own \
                     limit"
                ),
                format!(
                    "Set {GUESSES_SHARED_ENV} to {per_address} or more (20 times it is \
                     typical), or lower {GUESSES_PER_ADDRESS_ENV}"
                ),
            ));
        }
        let proxy = proxy(get)?;
        Ok(IdentityConfig {
            database,
            create_beside,
            listen,
            origin: origin.to_string(),
            base_path,
            rp_id,
            data_dir,
            key_file,
            import_console,
            client_origins,
            deployment,
            provisioner_key,
            invite_days: whole(get, INVITE_DAYS_ENV, INVITE_DAYS)?,
            token_secs: whole(get, TOKEN_SECS_ENV, TOKEN_SECS)?,
            session_idle_hours,
            session_max_hours,
            key_rotate_days: whole(get, KEY_ROTATE_DAYS_ENV, KEY_ROTATE_DAYS)?,
            limits: Limits {
                per_address,
                shared,
            },
            proxy,
        })
    }

    /// Where a browser opens the service: the origin and the base path.
    /// Invite links and the page's own links start here.
    pub fn public_url(&self) -> String {
        format!("{}{}", self.origin.trim_end_matches('/'), self.base_path)
    }

    /// Whether the service is served over https, which decides the
    /// cookie's `Secure` flag and `__Host-` prefix.
    pub fn secure_origin(&self) -> bool {
        self.origin.starts_with("https://")
    }

    pub fn session_idle_secs(&self) -> u64 {
        self.session_idle_hours * 3600
    }

    pub fn session_max_secs(&self) -> u64 {
        self.session_max_hours * 3600
    }

    /// The `aud` this service signs: `ordo-deployment:<handle>`.
    pub fn audience(&self) -> String {
        ordo_identity::deployment_token::audience(&self.deployment)
    }
}

fn pool_size(get: &dyn Fn(&str) -> Option<String>) -> Result<usize, IdentityError> {
    match get(DATABASE_POOL_ENV) {
        None => Ok(DEFAULT_POOL_SIZE),
        Some(v) => match v.trim().parse::<usize>() {
            Ok(n) if n >= 1 => Ok(n),
            _ => Err(IdentityError::setting(
                DATABASE_POOL_ENV,
                format!("is `{v}`, which is not a connection count"),
                format!(
                    "Set {DATABASE_POOL_ENV} to a whole number of 1 or more, or unset it for \
                     {DEFAULT_POOL_SIZE}"
                ),
            )),
        },
    }
}

/// A database setting in either form, `None` when unset.
fn database_setting(
    get: &dyn Fn(&str) -> Option<String>,
    var: &'static str,
    file_var: &'static str,
    size: usize,
    example: &str,
) -> Result<Option<PoolConfig>, IdentityError> {
    let url = ordo_env::secret_from(var, get, &|p| std::fs::read_to_string(p))
        .map_err(|e| {
            IdentityError::setting_caused(
                file_var,
                e.to_string(),
                format!(
                    "Set only one of {var} and {file_var}, and point {file_var} at a readable \
                     file that holds the URL"
                ),
                e,
            )
        })?
        .filter(|v| !v.trim().is_empty());
    let Some(url) = url else {
        return Ok(None);
    };
    parse_database_url(&url, size).map(Some).map_err(|e| {
        let fix = match &e {
            DatabaseUrlError::NotAConnectionString(_) => format!(
                "Set {var} to a URL such as {example}, or to key=value pairs (host=... \
                 port=... user=... dbname=...)"
            ),
            DatabaseUrlError::NoHost => format!("Add the database host to {var}"),
            DatabaseUrlError::NoDatabase => format!("Add the database name to {var}"),
            DatabaseUrlError::NoUser => format!("Add the database role to {var}"),
        };
        IdentityError::setting_caused(var, e.to_string(), fix, e)
    })
}

/// Boot refuses a database that is the deployment's or the console's own
/// (2.2). The service may share their cluster, never their database.
fn refuse_shared_database(
    own: &PoolConfig,
    console: Option<&PoolConfig>,
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<(), IdentityError> {
    let fix = format!(
        "Create a database of the identity service's own, for example `CREATE DATABASE \
         ordo_identity` on the same cluster, and point {DATABASE_URL_ENV} at it"
    );
    if let Some(db) = get(DEPLOYMENT_PG_DB_ENV) {
        let host = get(DEPLOYMENT_PG_HOST_ENV).unwrap_or_default();
        let port = get(DEPLOYMENT_PG_PORT_ENV)
            .and_then(|p| p.trim().parse::<u16>().ok())
            .unwrap_or(5432);
        if same_database(&db, &host, port, own) {
            return Err(IdentityError::setting(
                DATABASE_URL_ENV,
                format!(
                    "names database `{}` on {}:{}, which is the deployment's own database \
                     ({DEPLOYMENT_PG_DB_ENV})",
                    own.db, own.host, own.port
                ),
                fix,
            ));
        }
    }
    // The console's database, as the import names it and as the console's
    // own setting names it when this process can see it.
    let console_own = ordo_env::secret_from(CONSOLE_DATABASE_URL_ENV, get, &|p| {
        std::fs::read_to_string(p)
    })
    .ok()
    .flatten()
    .and_then(|u| parse_database_url(&u, 1).ok());
    for (named_by, c) in [
        (IMPORT_CONSOLE_URL_ENV, console),
        (CONSOLE_DATABASE_URL_ENV, console_own.as_ref()),
    ] {
        if let Some(c) = c {
            if same_database(&c.db, &c.host, c.port, own) {
                return Err(IdentityError::setting(
                    DATABASE_URL_ENV,
                    format!(
                        "names database `{}` on {}:{}, which is the console's own database \
                         ({named_by})",
                        own.db, own.host, own.port
                    ),
                    fix,
                ));
            }
        }
    }
    Ok(())
}

/// A passkey made under an RP ID asserts only for a page whose host is that
/// id or under it (6.2). The port is free.
fn refuse_origin_outside_rp_id(origin: &Origin, rp_id: &str) -> Result<(), IdentityError> {
    let host = origin.host();
    if host == rp_id || host.ends_with(&format!(".{rp_id}")) {
        return Ok(());
    }
    Err(IdentityError::setting(
        ORIGIN_ENV,
        format!(
            "has host `{host}`, which is not {RP_ID_ENV} (`{rp_id}`) or a host under it, so \
             no passkey made for `{rp_id}` would sign in there"
        ),
        format!(
            "Set {ORIGIN_ENV} to an address whose host is `{rp_id}` (any port, for example \
             https://{rp_id}:8082) or under it (https://id.{rp_id}). To move to another host \
             instead, set {RP_ID_ENV} to it and send every person a new invite: their old \
             passkeys will not sign in there"
        ),
    ))
}

fn client_origins(get: &dyn Fn(&str) -> Option<String>) -> Result<Vec<Origin>, IdentityError> {
    let Some(text) = get(CLIENT_ORIGINS_ENV) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in text.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let origin = Origin::parse(entry).ok_or_else(|| {
            IdentityError::setting(
                CLIENT_ORIGINS_ENV,
                format!("holds `{entry}`, which is not scheme://host[:port]"),
                format!(
                    "Set {CLIENT_ORIGINS_ENV} to the console's origin, for example \
                     https://console.example.com, comma-separated when there are more"
                ),
            )
        })?;
        if origin.scheme() == "http" && !origin.is_loopback() {
            return Err(IdentityError::setting(
                CLIENT_ORIGINS_ENV,
                format!("holds `{entry}`, which is http:// on a host that is not loopback"),
                format!("Use the https:// origin in {CLIENT_ORIGINS_ENV}"),
            ));
        }
        out.push(origin);
    }
    Ok(out)
}

/// 64 hex digits, or 43 base64url characters: the 32 bytes of an Ed25519
/// public key.
fn provisioner_key(
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<[u8; 32]>, IdentityError> {
    let Some(text) = get(PROVISIONER_KEY_ENV).map(|t| t.trim().to_string()) else {
        return Ok(None);
    };
    if text.is_empty() {
        return Ok(None);
    }
    let bytes = ordo_identity::sealing::unhex(&text)
        .or_else(|| ordo_identity::webauthn::b64url_decode(&text).ok());
    let key = bytes
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .filter(|b| ed25519_dalek::VerifyingKey::from_bytes(b).is_ok());
    match key {
        Some(k) => Ok(Some(k)),
        None => Err(IdentityError::setting(
            PROVISIONER_KEY_ENV,
            "is not an Ed25519 public key",
            format!(
                "Set {PROVISIONER_KEY_ENV} to the provisioning service's public key, 64 hex \
                 digits or 43 base64url characters, or unset it"
            ),
        )),
    }
}

/// Proxy mode, as the door reads it: both settings or neither.
fn proxy(get: &dyn Fn(&str) -> Option<String>) -> Result<Option<Proxy>, IdentityError> {
    let read = |k| {
        get(k)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let (header, trusted) = match (read(PROXY_HEADER_ENV), read(PROXY_TRUSTED_ENV)) {
        (None, None) => return Ok(None),
        (Some(_), None) => {
            return Err(IdentityError::setting(
                PROXY_TRUSTED_ENV,
                format!("is unset while {PROXY_HEADER_ENV} is set"),
                format!(
                    "List the proxies trusted to set the header, such as \
                     {PROXY_TRUSTED_ENV}=10.0.0.0/8, or unset {PROXY_HEADER_ENV}"
                ),
            ))
        }
        (None, Some(_)) => {
            return Err(IdentityError::setting(
                PROXY_HEADER_ENV,
                format!("is unset while {PROXY_TRUSTED_ENV} is set"),
                format!(
                    "Name the header your proxy sets, such as \
                     {PROXY_HEADER_ENV}=CF-Connecting-IP, or unset {PROXY_TRUSTED_ENV}"
                ),
            ))
        }
        (Some(h), Some(t)) => (h, t),
    };
    if axum::http::HeaderName::from_bytes(header.as_bytes()).is_err() {
        return Err(IdentityError::setting(
            PROXY_HEADER_ENV,
            format!("is `{header}`, which is not a header name"),
            format!("Set {PROXY_HEADER_ENV} to a header name such as CF-Connecting-IP"),
        ));
    }
    let mut blocks = Vec::new();
    for entry in trusted.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        blocks.push(Cidr::parse(entry).map_err(|problem| {
            IdentityError::setting(
                PROXY_TRUSTED_ENV,
                format!("holds {problem}"),
                format!("List addresses or CIDR blocks, such as {PROXY_TRUSTED_ENV}=10.0.0.0/8"),
            )
        })?);
    }
    Ok(Some(Proxy {
        header,
        trusted: blocks,
    }))
}

fn whole(
    get: &dyn Fn(&str) -> Option<String>,
    var: &'static str,
    r: Range,
) -> Result<u64, IdentityError> {
    match get(var) {
        None => Ok(r.default),
        Some(v) => match v.trim().parse::<u64>() {
            Ok(n) if (r.min..=r.max).contains(&n) => Ok(n),
            _ => Err(IdentityError::setting(
                var,
                format!(
                    "is `{v}`, which is not a whole number of {} from {} to {}",
                    r.unit, r.min, r.max
                ),
                format!(
                    "Set {var} to {} to {} {}, or unset it for {}",
                    r.min, r.max, r.unit, r.default
                ),
            )),
        },
    }
}

/// A deployment handle: lowercase letters, digits and dashes, at most 63.
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
    use std::collections::HashMap;

    fn with(extra: &[(&str, &str)]) -> Result<IdentityConfig, IdentityError> {
        let mut v: HashMap<String, String> = HashMap::from([
            (
                DATABASE_URL_ENV.to_string(),
                "host=/tmp user=identity dbname=ordo_identity".to_string(),
            ),
            (
                ORIGIN_ENV.to_string(),
                "https://console.example.com:8082".to_string(),
            ),
        ]);
        for (k, val) in extra {
            v.insert(k.to_string(), val.to_string());
        }
        IdentityConfig::from_vars(&|k| v.get(k).cloned())
    }

    fn refused(extra: &[(&str, &str)]) -> &'static str {
        with(extra)
            .expect_err("refused")
            .setting_name()
            .expect("names a setting")
    }

    #[test]
    fn the_defaults_are_the_owners_answers() {
        let c = with(&[]).unwrap();
        assert_eq!(c.session_idle_hours, 168);
        assert_eq!(c.session_max_hours, 720);
        assert_eq!(c.invite_days, 7);
        assert_eq!(c.token_secs, 600);
        assert_eq!(c.key_rotate_days, 90);
        assert_eq!(c.deployment, "main");
        assert_eq!(c.audience(), "ordo-deployment:main");
        assert_eq!(c.listen.to_string(), "127.0.0.1:8082");
        assert_eq!(c.rp_id, "console.example.com");
        assert_eq!(c.origin, "https://console.example.com:8082");
        assert_eq!(
            c.key_file,
            PathBuf::from("/var/lib/ordo-identity/identity.key")
        );
        assert!(c.import_console.is_none());
    }

    #[test]
    fn each_out_of_range_value_names_its_setting() {
        for (var, bad) in [
            (SESSION_IDLE_ENV, "0"),
            (SESSION_IDLE_ENV, "721"),
            (SESSION_MAX_ENV, "2161"),
            (INVITE_DAYS_ENV, "31"),
            (TOKEN_SECS_ENV, "59"),
            (TOKEN_SECS_ENV, "3601"),
            (KEY_ROTATE_DAYS_ENV, "soon"),
            (LISTEN_ENV, "nowhere"),
            (DEPLOYMENT_ENV, "Main"),
            (PROVISIONER_KEY_ENV, "abc"),
            (CLIENT_ORIGINS_ENV, "http://console.example.com"),
            (DATABASE_POOL_ENV, "0"),
        ] {
            assert_eq!(refused(&[(var, bad)]), var, "{var}={bad}");
        }
        assert_eq!(
            with(&[(KEY_ROTATE_DAYS_ENV, "0")]).unwrap().key_rotate_days,
            0
        );
        assert_eq!(
            with(&[(SESSION_IDLE_ENV, "12"), (SESSION_MAX_ENV, "24")])
                .unwrap()
                .session_max_hours,
            24
        );
    }

    #[test]
    fn a_max_below_idle_is_refused_naming_the_max() {
        assert_eq!(
            refused(&[(SESSION_IDLE_ENV, "48"), (SESSION_MAX_ENV, "24")]),
            SESSION_MAX_ENV
        );
    }

    #[test]
    fn the_origin_must_sit_at_or_under_the_rp_id_and_the_refusal_names_both() {
        assert!(with(&[
            (ORIGIN_ENV, "https://id.console.example.com"),
            (RP_ID_ENV, "console.example.com")
        ])
        .is_ok());
        assert!(with(&[
            (ORIGIN_ENV, "http://localhost:8082"),
            (RP_ID_ENV, "localhost")
        ])
        .is_ok());
        let e = with(&[
            (ORIGIN_ENV, "https://id.example.com"),
            (RP_ID_ENV, "console.example.com"),
        ])
        .unwrap_err();
        assert_eq!(e.setting_name(), Some(ORIGIN_ENV));
        assert!(e.to_string().contains(RP_ID_ENV), "{e}");
        // `evilconsole.example.com` is not under `console.example.com`.
        assert_eq!(
            refused(&[
                (ORIGIN_ENV, "https://evilconsole.example.com"),
                (RP_ID_ENV, "console.example.com")
            ]),
            ORIGIN_ENV
        );
        assert_eq!(
            refused(&[(ORIGIN_ENV, "http://id.example.com")]),
            ORIGIN_ENV
        );
    }

    #[test]
    fn the_database_is_required_and_never_the_deployments_or_the_consoles() {
        let e = IdentityConfig::from_vars(&|k| {
            (k == ORIGIN_ENV).then(|| "https://console.example.com".to_string())
        })
        .unwrap_err();
        assert_eq!(e.setting_name(), Some(DATABASE_URL_ENV));
        assert_eq!(
            refused(&[
                (DEPLOYMENT_PG_DB_ENV, "ordo_identity"),
                (DEPLOYMENT_PG_HOST_ENV, "/tmp"),
                (DEPLOYMENT_PG_PORT_ENV, "5432")
            ]),
            DATABASE_URL_ENV
        );
        let e = with(&[(
            IMPORT_CONSOLE_URL_ENV,
            "host=/tmp user=console dbname=ordo_identity",
        )])
        .unwrap_err();
        assert_eq!(e.setting_name(), Some(DATABASE_URL_ENV));
        assert!(e.to_string().contains(IMPORT_CONSOLE_URL_ENV), "{e}");
        assert_eq!(
            refused(&[(
                CONSOLE_DATABASE_URL_ENV,
                "host=/tmp user=console dbname=ordo_identity"
            )]),
            DATABASE_URL_ENV
        );
        let ok = with(&[(
            IMPORT_CONSOLE_URL_ENV,
            "host=/tmp user=console dbname=ordo_console",
        )])
        .unwrap();
        assert_eq!(ok.import_console.unwrap().db, "ordo_console");
    }
}
