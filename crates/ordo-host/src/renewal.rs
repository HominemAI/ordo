//! Staying signed in across a reload (owner ruling 2026-10-21).
//!
//! Every sign-in also sets a **renewal cookie**: an opaque secret the page's
//! scripts cannot read (`HttpOnly`), sent only to `POST /auth/refresh`
//! (`Path=/auth/refresh`) and only from the same site (`SameSite=Strict`).
//! The session token itself stays in page memory, as before. On load the
//! page calls `/auth/refresh` with no bearer token, the browser adds the
//! cookie, and the door answers a fresh token.
//!
//! # What the door keeps
//!
//! Only the SHA-256 of each secret, beside the family it belongs to. A
//! family is one sign-in: its id **is** the session id that sign-in minted,
//! and every token a renewal mints carries that same id. So a logout of any
//! of the family's tokens revokes the id, and with it every token the family
//! ever minted.
//!
//! # Rotation, and reuse
//!
//! Each use spends the secret and sets a new one. A spent secret presented
//! again, after a short grace for two tabs loading at once
//! ([`GRACE_SECS`]), means two browsers hold the family: the real person and
//! a copy. The door cannot tell which is which, so it ends the family and
//! revokes its session. The person signs in again; the copy is dead.
//!
//! # What ends it
//!
//! [`RenewingRevocations`] wraps the deployment's one deny list, so every
//! act that ends a session ends its renewal too, with no caller remembering
//! to: sign-out (`revoke`), and every "end all sessions" (`end_all`), which
//! is what passkey removal, account recovery, a password change, account
//! deletion and sign-out everywhere call.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use ordo_exec::{sql_lit, Pool, PoolError};
use ordo_identity::principal::PrincipalId;
use ordo_identity::session::{RevocationStoreError, SessionId, SessionRevocations};
use ordo_identity::{StoreFault, Timestamp};
use sha2::{Digest, Sha256};

/// `ORDO_REMEMBER`: `on` (default) or `off`.
pub const SETTING: &str = "ORDO_REMEMBER";
/// `ORDO_REMEMBER_SECS`: how long a renewal lives unused. Each use renews it.
pub const SECS_SETTING: &str = "ORDO_REMEMBER_SECS";
/// 30 days.
pub const DEFAULT_SECS: u64 = 30 * 24 * 3600;
/// The cookie's name.
/// Over https it carries the `__Secure-` prefix, so a browser refuses it
/// from a plain-http answer or without `Secure`. Plain http to a loopback
/// name cannot carry the prefix and uses [`LOOPBACK_COOKIE`].
pub const COOKIE: &str = "__Secure-ordo_renew";
/// The name on plain http to a loopback name, where `Secure` is dropped.
pub const LOOPBACK_COOKIE: &str = "ordo_renew";
/// The only path the browser sends the cookie to.
pub const COOKIE_PATH: &str = "/auth/refresh";
/// How long a just-spent secret is still honoured, without a new cookie.
/// Two tabs reloaded together both send the same cookie; the second must
/// not end the family. Ten seconds is long enough for that race and too
/// short to be worth stealing a cookie for.
pub const GRACE_SECS: u64 = 10;

/// The deployment's setting: on or off, and the idle lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RememberConfig {
    pub on: bool,
    pub idle_secs: u64,
}

impl Default for RememberConfig {
    fn default() -> RememberConfig {
        RememberConfig {
            on: true,
            idle_secs: DEFAULT_SECS,
        }
    }
}

/// Read the two settings. A value that is not understood is refused,
/// never clamped.
pub fn from_vars(
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<RememberConfig, (&'static str, String)> {
    let on = match env(SETTING).as_deref().map(str::trim) {
        None | Some("") | Some("on") => true,
        Some("off") => false,
        Some(other) => {
            return Err((SETTING, format!("`{other}` is not a value (on | off)")));
        }
    };
    let idle_secs = match env(SECS_SETTING).as_deref().map(str::trim) {
        None | Some("") => DEFAULT_SECS,
        Some(v) => match v.parse::<u64>() {
            Ok(0) => {
                return Err((
                    SECS_SETTING,
                    format!(
                        "0 would end every renewal at once; set {SETTING}=off to turn it off, \
                         or unset this for the default, {DEFAULT_SECS}"
                    ),
                ))
            }
            Ok(n) => n,
            Err(_) => {
                return Err((
                    SECS_SETTING,
                    format!("`{v}` is not a number of seconds (default {DEFAULT_SECS})"),
                ))
            }
        },
    };
    Ok(RememberConfig { on, idle_secs })
}

/// One sign-in's renewals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Family {
    /// The session id the sign-in minted.
    pub family: String,
    pub principal: String,
    pub via: Option<String>,
    pub second_step: Option<String>,
    /// The tenant the session is for, when the sign-in named one.
    pub space: Option<String>,
    pub signed_in_at: u64,
}

/// What presenting a secret came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Turn {
    /// Spent, and the new secret stored.
    Fresh(Family),
    /// Spent a moment ago by another request. Honoured, with no new secret.
    Grace(Family),
    /// Spent long ago: the family has been ended.
    Reused(Family),
    /// Not known, or idle past its lifetime.
    Unknown,
}

/// Where renewals are kept. Synchronous, like the deny list it sits beside;
/// callers run it off the runtime's workers.
pub trait RenewalStore: Send + Sync {
    /// Store the first secret of a family.
    fn issue(&self, family: &Family, hash: &str, idle_until: u64) -> Result<(), StoreFault>;
    /// Present `hash`. When it is live, spend it and store `next_hash`,
    /// which lives until `now + idle_secs`.
    fn turn(
        &self,
        hash: &str,
        next_hash: &str,
        now: u64,
        idle_secs: u64,
    ) -> Result<Turn, StoreFault>;
    /// End one family.
    fn end_family(&self, family: &str) -> Result<(), StoreFault>;
    /// Drop one secret, spent or not.
    fn end_hash(&self, hash: &str) -> Result<(), StoreFault>;
    /// End every family a person has.
    fn end_principal(&self, principal: &str) -> Result<(), StoreFault>;
    /// Drop what has idled out.
    fn sweep(&self, now: u64) -> Result<(), StoreFault>;
}

/// The stored form of a secret.
pub fn hash_of(secret: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"ordo.renew.v1\0");
    h.update(secret.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// A new secret: 32 random bytes, base64url.
pub fn mint_secret() -> String {
    ordo_identity::federation::random_secret()
}

#[derive(Debug, Clone)]
struct Row {
    family: Family,
    idle_until: u64,
    spent_at: Option<u64>,
}

/// In memory, for tests.
#[derive(Default)]
pub struct MemoryRenewals {
    rows: Mutex<BTreeMap<String, Row>>,
}

impl MemoryRenewals {
    pub fn new() -> MemoryRenewals {
        MemoryRenewals::default()
    }

    /// How many secrets are stored, spent or not.
    pub fn len(&self) -> usize {
        self.rows.lock().expect("renewals").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl RenewalStore for MemoryRenewals {
    fn end_hash(&self, hash: &str) -> Result<(), StoreFault> {
        self.rows.lock().expect("renewals").remove(hash);
        Ok(())
    }

    fn issue(&self, family: &Family, hash: &str, idle_until: u64) -> Result<(), StoreFault> {
        self.rows.lock().expect("renewals").insert(
            hash.to_string(),
            Row {
                family: family.clone(),
                idle_until,
                spent_at: None,
            },
        );
        Ok(())
    }

    fn turn(
        &self,
        hash: &str,
        next_hash: &str,
        now: u64,
        idle_secs: u64,
    ) -> Result<Turn, StoreFault> {
        let mut rows = self.rows.lock().expect("renewals");
        let Some(row) = rows.get(hash).cloned() else {
            return Ok(Turn::Unknown);
        };
        if row.idle_until <= now {
            rows.remove(hash);
            return Ok(Turn::Unknown);
        }
        match row.spent_at {
            Some(at) if now.saturating_sub(at) <= GRACE_SECS => Ok(Turn::Grace(row.family)),
            Some(_) => {
                rows.retain(|_, r| r.family.family != row.family.family);
                Ok(Turn::Reused(row.family))
            }
            None => {
                if let Some(r) = rows.get_mut(hash) {
                    r.spent_at = Some(now);
                }
                rows.insert(
                    next_hash.to_string(),
                    Row {
                        family: row.family.clone(),
                        idle_until: now.saturating_add(idle_secs),
                        spent_at: None,
                    },
                );
                Ok(Turn::Fresh(row.family))
            }
        }
    }

    fn end_family(&self, family: &str) -> Result<(), StoreFault> {
        self.rows
            .lock()
            .expect("renewals")
            .retain(|_, r| r.family.family != family);
        Ok(())
    }

    fn end_principal(&self, principal: &str) -> Result<(), StoreFault> {
        self.rows
            .lock()
            .expect("renewals")
            .retain(|_, r| r.family.principal != principal);
        Ok(())
    }

    fn sweep(&self, now: u64) -> Result<(), StoreFault> {
        self.rows
            .lock()
            .expect("renewals")
            .retain(|_, r| r.idle_until > now);
        Ok(())
    }
}

/// The table. One row per secret, spent or live. A spent row is kept until
/// its own idle time, so a copy presented later is still recognised as
/// reuse. No secret is stored, only its hash.
pub const RENEWAL_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$renewal" (
  "hash"         text   NOT NULL,
  "family"       text   NOT NULL,
  "principal"    text   NOT NULL,
  "via"          text,
  "second_step"  text,
  "space"        text,
  "signed_in_at" bigint NOT NULL,
  "idle_until"   bigint NOT NULL,
  "spent_at"     bigint,
  PRIMARY KEY ("hash")
)"#;

const RENEWAL_INDEX_DDL: [&str; 3] = [
    r#"CREATE INDEX IF NOT EXISTS "$renewal$family" ON "$renewal" ("family")"#,
    r#"CREATE INDEX IF NOT EXISTS "$renewal$principal" ON "$renewal" ("principal")"#,
    r#"CREATE INDEX IF NOT EXISTS "$renewal$idle" ON "$renewal" ("idle_until")"#,
];

/// Create the table, at boot beside the deny list.
pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    for ddl in std::iter::once(RENEWAL_DDL).chain(RENEWAL_INDEX_DDL) {
        if pool.rows(ddl).await.is_err() {
            pool.rows(ddl).await?;
        }
    }
    Ok(())
}

/// Postgres.
pub struct PgRenewals {
    pool: Pool,
}

impl PgRenewals {
    pub fn new(pool: Pool) -> PgRenewals {
        PgRenewals { pool }
    }

    fn run(&self, sql: String) -> Result<Vec<Vec<Option<String>>>, StoreFault> {
        let pool = self.pool.clone();
        crate::sessions::block_on(async move { pool.rows(&sql).await }).map_err(|e| {
            crate::auth::store_failure::pool_fault(e, "the renewal table could not be read")
        })
    }
}

fn opt_lit(v: &Option<String>) -> String {
    match v {
        Some(s) => sql_lit(s),
        None => "NULL".to_string(),
    }
}

fn family_of(r: &[Option<String>]) -> Option<Family> {
    let col = |i: usize| r.get(i).cloned().flatten();
    Some(Family {
        family: col(0)?,
        principal: col(1)?,
        via: col(2),
        second_step: col(3),
        space: col(4),
        signed_in_at: col(5)?.parse().ok()?,
    })
}

const FAMILY_COLS: &str = r#""family", "principal", "via", "second_step", "space", "signed_in_at""#;

impl RenewalStore for PgRenewals {
    fn end_hash(&self, hash: &str) -> Result<(), StoreFault> {
        self.run(format!(
            r#"DELETE FROM "$renewal" WHERE "hash" = {}"#,
            sql_lit(hash)
        ))
        .map(|_| ())
    }

    fn issue(&self, f: &Family, hash: &str, idle_until: u64) -> Result<(), StoreFault> {
        self.run(format!(
            r#"INSERT INTO "$renewal" ("hash", {FAMILY_COLS}, "idle_until")
               VALUES ({}, {}, {}, {}, {}, {}, {}, {idle_until})"#,
            sql_lit(hash),
            sql_lit(&f.family),
            sql_lit(&f.principal),
            opt_lit(&f.via),
            opt_lit(&f.second_step),
            opt_lit(&f.space),
            f.signed_in_at,
        ))
        .map(|_| ())
    }

    fn turn(
        &self,
        hash: &str,
        next_hash: &str,
        now: u64,
        idle_secs: u64,
    ) -> Result<Turn, StoreFault> {
        // Spend it, if it is live: one statement, so two requests with the
        // same secret cannot both spend it.
        let spent = self.run(format!(
            r#"UPDATE "$renewal" SET "spent_at" = {now}
               WHERE "hash" = {} AND "spent_at" IS NULL AND "idle_until" > {now}
               RETURNING {FAMILY_COLS}"#,
            sql_lit(hash)
        ))?;
        if let Some(f) = spent.first().and_then(|r| family_of(r)) {
            self.issue(&f, next_hash, now.saturating_add(idle_secs))?;
            return Ok(Turn::Fresh(f));
        }
        let rows = self.run(format!(
            r#"SELECT {FAMILY_COLS}, "spent_at", "idle_until" FROM "$renewal"
               WHERE "hash" = {}"#,
            sql_lit(hash)
        ))?;
        let Some(r) = rows.first() else {
            return Ok(Turn::Unknown);
        };
        let Some(f) = family_of(r) else {
            return Ok(Turn::Unknown);
        };
        let spent_at = r
            .get(6)
            .cloned()
            .flatten()
            .and_then(|v| v.parse::<u64>().ok());
        let idle_until = r
            .get(7)
            .cloned()
            .flatten()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        if idle_until <= now {
            return Ok(Turn::Unknown);
        }
        match spent_at {
            Some(at) if now.saturating_sub(at) <= GRACE_SECS => Ok(Turn::Grace(f)),
            Some(_) => {
                self.end_family(&f.family)?;
                Ok(Turn::Reused(f))
            }
            None => Ok(Turn::Unknown),
        }
    }

    fn end_family(&self, family: &str) -> Result<(), StoreFault> {
        self.run(format!(
            r#"DELETE FROM "$renewal" WHERE "family" = {}"#,
            sql_lit(family)
        ))
        .map(|_| ())
    }

    fn end_principal(&self, principal: &str) -> Result<(), StoreFault> {
        self.run(format!(
            r#"DELETE FROM "$renewal" WHERE "principal" = {}"#,
            sql_lit(principal)
        ))
        .map(|_| ())
    }

    fn sweep(&self, now: u64) -> Result<(), StoreFault> {
        self.run(format!(
            r#"DELETE FROM "$renewal" WHERE "idle_until" <= {now}"#
        ))
        .map(|_| ())
    }
}

/// What the routes hold: the store, the deployment's setting, and the app
/// origins that may redeem the cookie.
pub struct Remember {
    pub store: Arc<dyn RenewalStore>,
    pub config: RememberConfig,
    /// The deployment's app origins: the list CORS uses
    /// (`ORDO_CORS_ORIGINS`). A cookie is redeemed only from a request whose
    /// `Origin` is on it. `None` redeems none. The door's own origin is not
    /// an app origin, so script served from the door cannot redeem it.
    pub app_origins: Option<ordo_api::cors::CorsPolicy>,
}

/// The setting that lists the origins a renewal cookie is redeemed from.
pub const ORIGINS_SETTING: &str = "ORDO_CORS_ORIGINS";

impl Remember {
    /// Whether a request carrying `origin` may redeem the cookie. A request
    /// with no `Origin` may not: a browser always sends one on a POST fetch.
    pub fn origin_allowed(&self, origin: Option<&str>) -> bool {
        match (origin, &self.app_origins) {
            (Some(o), Some(list)) => list.allows_origin(o.trim()),
            _ => false,
        }
    }
}

/// The deployment's deny list, with renewals ended beside every session.
///
/// Installed as **the** deny list, so every caller that ends a session ends
/// its renewal too. A renewal that could not be ended is reported as a
/// failed end, for the reason a failed logout is: the page would believe it
/// is signed out while a cookie could still sign it back in.
pub struct RenewingRevocations {
    inner: Arc<dyn SessionRevocations>,
    renewals: Arc<dyn RenewalStore>,
}

impl RenewingRevocations {
    pub fn new(
        inner: Arc<dyn SessionRevocations>,
        renewals: Arc<dyn RenewalStore>,
    ) -> RenewingRevocations {
        RenewingRevocations { inner, renewals }
    }
}

impl SessionRevocations for RenewingRevocations {
    fn is_revoked(&self, id: &SessionId) -> bool {
        self.inner.is_revoked(id)
    }

    fn revoke(&self, id: &SessionId, until: Timestamp) -> Result<(), RevocationStoreError> {
        self.inner.revoke(id, until)?;
        self.renewals
            .end_family(&id.0)
            .map_err(RevocationStoreError)
    }

    fn principal_ended(&self, principal: &PrincipalId, started_at: Timestamp) -> bool {
        self.inner.principal_ended(principal, started_at)
    }

    fn end_all(
        &self,
        principal: &PrincipalId,
        at: Timestamp,
        until: Timestamp,
    ) -> Result<(), RevocationStoreError> {
        self.inner.end_all(principal, at, until)?;
        self.renewals
            .end_principal(&principal.0)
            .map_err(RevocationStoreError)
    }
}

/// Whether the request reached the door on a loopback name over plain http,
/// where a `Secure` cookie would be dropped by some browsers.
fn loopback_http(headers: &axum::http::HeaderMap) -> bool {
    let https = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|p| p.trim().eq_ignore_ascii_case("https"));
    if https {
        return false;
    }
    let Some(host) = headers.get("host").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let host = host.trim().to_ascii_lowercase();
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or("").to_string()
    } else {
        host.rsplit_once(':')
            .map(|(h, _)| h.to_string())
            .unwrap_or(host)
    };
    name == "localhost" || name.ends_with(".localhost") || name == "127.0.0.1" || name == "::1"
}

/// The `Set-Cookie` value that hands the browser `secret`.
pub fn set_cookie(headers: &axum::http::HeaderMap, secret: &str, max_age: u64) -> String {
    let secure = if loopback_http(headers) {
        ""
    } else {
        "; Secure"
    };
    format!(
        "{}={secret}; Path={COOKIE_PATH}; Max-Age={max_age}; HttpOnly; SameSite=Strict{secure}",
        cookie_name(headers)
    )
}

/// The `Set-Cookie` value that removes the cookie.
pub fn clear_cookie(headers: &axum::http::HeaderMap) -> String {
    let secure = if loopback_http(headers) {
        ""
    } else {
        "; Secure"
    };
    format!(
        "{}=; Path={COOKIE_PATH}; Max-Age=0; HttpOnly; SameSite=Strict{secure}",
        cookie_name(headers)
    )
}

/// The cookie's name for this request.
fn cookie_name(headers: &axum::http::HeaderMap) -> &'static str {
    if loopback_http(headers) {
        LOOPBACK_COOKIE
    } else {
        COOKIE
    }
}

/// The renewal secret the browser sent, if any.
pub fn cookie_of(headers: &axum::http::HeaderMap) -> Option<String> {
    for value in headers.get_all("cookie") {
        let Ok(text) = value.to_str() else { continue };
        for part in text.split(';') {
            if let Some((name, v)) = part.trim().split_once('=') {
                if (name == COOKIE || name == LOOPBACK_COOKIE) && !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn defaults_are_on_for_thirty_days() {
        assert_eq!(from_vars(&env(&[])).unwrap(), RememberConfig::default());
        assert_eq!(DEFAULT_SECS, 2_592_000);
    }

    #[test]
    fn a_bad_value_names_the_setting() {
        let e = from_vars(&env(&[(SETTING, "yes")])).unwrap_err();
        assert_eq!(e.0, SETTING);
        let e = from_vars(&env(&[(SECS_SETTING, "0")])).unwrap_err();
        assert!(e.1.contains("ORDO_REMEMBER=off"), "{}", e.1);
    }

    #[test]
    fn loopback_http_drops_secure_and_nothing_else_does() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("host", "127.0.0.1:4000".parse().unwrap());
        assert!(!set_cookie(&h, "s", 1).contains("Secure"));
        h.insert("host", "localhost:5173".parse().unwrap());
        assert!(!set_cookie(&h, "s", 1).contains("Secure"));
        h.insert("x-forwarded-proto", "https".parse().unwrap());
        assert!(set_cookie(&h, "s", 1).starts_with("__Secure-ordo_renew=s;"));
        let mut h = axum::http::HeaderMap::new();
        h.insert("host", "ordo.example.com".parse().unwrap());
        assert!(set_cookie(&h, "s", 1).contains("Secure"));
    }

    #[test]
    fn the_cookie_is_read_among_others() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("cookie", "a=1; ordo_renew=xyz; b=2".parse().unwrap());
        assert_eq!(cookie_of(&h).as_deref(), Some("xyz"));
    }
}
