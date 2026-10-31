//! Tenant-owned sign-in providers (`plan-tenant-providers.md`,
//! `plan-auth-federated.md` L1).
//!
//! # The ruling
//!
//! Owner, 2026-10-09: *"A tenant given the opportunity should have the
//! ability to setup their own provider as long as it falls within the oauth /
//! oidc standard protocol."* Off by default; the app developer switches it
//! on with [`SETTING`]. A tenant's owner adds a provider, with presence.
//!
//! # What lives here
//!
//! - The four platform ops: [`ADD`], [`UPDATE`], [`RETIRE`], [`LIST`].
//! - [`TenantConnections`]: the rows in each tenant's own database, the
//!   sealed client secret, and the registry that changes while serving.
//!   Each `begin` and callback reads the row, so a retired connection stops
//!   new sign-ins on the next request, on every replica. A built provider is
//!   cached by the row's revision and key.
//! - The originating-admin rule ([`TenantConnections::guarded`]): a
//!   tenant always keeps one active owner who signs in without its own
//!   providers.
//!
//! The routes are `crate::auth::tenant`'s.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ordo_api::{
    BoundParams, ErrorCode, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec,
    ParamSpec, ParamTy, RefineTier, Syncability,
};
use ordo_crypto::{Anchor, KeyProvider, Purpose};
use ordo_egress::HttpsUrl;
use ordo_exec::{sql_lit, Pool, TxConn};
use ordo_identity::federation::ClientSecret;
use ordo_identity::oauth2::{valid_connection_name, AvatarRule, OAuth2Connection, OAuth2Provider};
use ordo_identity::oidc::{OidcConnection, OidcEndpoints, OidcProvider, TransportFactory};
use ordo_identity::upstream::{tenant_key, tenant_key_prefix, PresenceMode, PresenceRule};
use ordo_identity::{AuthenticatorId, CeremonyStore, DirectoryStore, PrincipalId, SpaceId};
use serde_json::{json, Map, Value};

use crate::member_ops::{MemberOps, ROLES};
use crate::platform_ops::PLATFORM_VERSION;

/// The deployment setting: `off` (the default) or `on`.
pub const SETTING: &str = "ORDO_AUTH_TENANT_PROVIDERS";

pub const ADD: &str = "std.space.addConnection";
pub const UPDATE: &str = "std.space.updateConnection";
pub const RETIRE: &str = "std.space.retireConnection";
pub const LIST: &str = "std.space.connections";
/// Every op of this module, for registration and the owner-only list.
pub const OPS: [&str; 4] = [ADD, UPDATE, RETIRE, LIST];

const TABLE: &str = "$tenantconnection";
const DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$tenantconnection" (
  "$space"     text   NOT NULL,
  "name"       text   NOT NULL,
  "key"        text   NOT NULL,
  "kind"       text   NOT NULL,
  "settings"   text   NOT NULL,
  "secret"     text   NOT NULL,
  "state"      text   NOT NULL DEFAULT 'active',
  "rev"        bigint NOT NULL DEFAULT 1,
  "updated_by" text   NOT NULL,
  "updated_at" bigint NOT NULL,
  PRIMARY KEY ("$space", "name"))"#;

/// Read [`SETTING`]. Unset is off.
pub fn enabled_from_vars(var: &dyn Fn(&str) -> Option<String>) -> Result<bool, String> {
    match var(SETTING).map(|v| v.trim().to_string()).as_deref() {
        None | Some("") | Some("off") => Ok(false),
        Some("on") => Ok(true),
        Some(_) => Err(format!("{SETTING} is `on` or `off`; unset it for off")),
    }
}

/// The refusal while the setting is off. Names the setting and the value.
pub fn off_message() -> String {
    format!(
        "tenant-owned sign-in providers are off on this deployment; the app developer sets \
         {SETTING}=on to allow them"
    )
}

fn refused(msg: impl Into<String>) -> ExecError {
    ExecError::new(ErrorCode::TenantRefused, msg)
}

fn unavailable(e: impl crate::Faulty) -> ExecError {
    crate::op_store_unavailable(e)
}

/// A key custody failure as the answer to the caller, read from its class: a
/// busy key store is `server_busy` (the same call can be sent again), anything
/// else is the one fixed sentence. The cause is never put in the answer.
fn custody_fault(e: &ordo_crypto::KeyError, what: &str) -> ExecError {
    if e.is_busy() {
        ExecError::server_busy()
    } else {
        unavailable(what)
    }
}

/// The same, for the provider build path that carries its reason as text.
fn custody_words(e: &ordo_crypto::KeyError, what: &str) -> String {
    if e.is_busy() {
        format!("{what}: the server is busy; try again shortly")
    } else {
        what.to_string()
    }
}

/// The refusal when a change would leave no owner who signs in without
/// the tenant's own providers.
pub fn no_originating_admin() -> ExecError {
    refused(
        "that would leave this tenant with no active owner who signs in without its own \
         providers, and then nobody could sign in to fix them. Make another member an owner \
         who has a passkey, a password or another sign-in first",
    )
}

/// `oidc` or `oauth2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Oidc,
    OAuth2,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Oidc => "oidc",
            Kind::OAuth2 => "oauth2",
        }
    }

    fn parse(w: &str) -> Option<Kind> {
        match w {
            "oidc" => Some(Kind::Oidc),
            "oauth2" => Some(Kind::OAuth2),
            _ => None,
        }
    }
}

/// A connection's settings, without its secret. Stored as JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub kind: Kind,
    /// OIDC: the issuer. Fixed once added.
    pub issuer: Option<String>,
    /// OIDC: all three pin the endpoints and skip discovery. OAuth2: the
    /// first two are required.
    pub authorize_url: Option<String>,
    pub token_url: Option<String>,
    pub jwks_url: Option<String>,
    /// OAuth2: required.
    pub userinfo_url: Option<String>,
    pub client_id: String,
    pub scope: String,
    pub display_claims: Vec<String>,
    pub avatar_claim: Option<String>,
    /// OIDC: keep a verified address.
    pub use_email: bool,
    /// OAuth2: the claim that names the person, never changing.
    pub subject_claim: String,
    pub email_claim: Option<String>,
    pub email_verified_claim: Option<String>,
    /// The role a newcomer is admitted at.
    pub role: String,
}

impl Settings {
    fn to_json(&self) -> Value {
        json!({
            "kind": self.kind.as_str(),
            "issuer": self.issuer,
            "authorizeUrl": self.authorize_url,
            "tokenUrl": self.token_url,
            "jwksUrl": self.jwks_url,
            "userinfoUrl": self.userinfo_url,
            "clientId": self.client_id,
            "scope": self.scope,
            "displayClaims": self.display_claims.join(","),
            "avatarClaim": self.avatar_claim,
            "useEmail": self.use_email,
            "subjectClaim": self.subject_claim,
            "emailClaim": self.email_claim,
            "emailVerifiedClaim": self.email_verified_claim,
            "role": self.role,
        })
    }

    fn from_json(v: &Value) -> Option<Settings> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Some(Settings {
            kind: Kind::parse(v.get("kind")?.as_str()?)?,
            issuer: s("issuer"),
            authorize_url: s("authorizeUrl"),
            token_url: s("tokenUrl"),
            jwks_url: s("jwksUrl"),
            userinfo_url: s("userinfoUrl"),
            client_id: s("clientId")?,
            scope: s("scope")?,
            display_claims: list(&s("displayClaims").unwrap_or_default()),
            avatar_claim: s("avatarClaim"),
            use_email: v.get("useEmail").and_then(Value::as_bool).unwrap_or(true),
            subject_claim: s("subjectClaim").unwrap_or_else(|| "sub".into()),
            email_claim: s("emailClaim"),
            email_verified_claim: s("emailVerifiedClaim"),
            role: s("role").unwrap_or_else(|| crate::admission::DEFAULT_ROLE.into()),
        })
    }
}

fn list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string)
        .collect()
}

/// One stored connection.
#[derive(Debug, Clone)]
struct Row {
    name: String,
    key: String,
    settings: Settings,
    secret: String,
    state: String,
    rev: i64,
}

/// A connection, built and ready to serve.
#[derive(Clone)]
pub enum Live {
    Oidc(Arc<OidcProvider>),
    OAuth2(Arc<OAuth2Provider>),
}

impl Live {
    /// The provider, as the sign-in routes call it.
    pub fn provider(&self) -> Arc<dyn ordo_identity::AuthProvider> {
        match self {
            Live::Oidc(p) => Arc::new(p.clone()),
            Live::OAuth2(p) => Arc::new(p.clone()),
        }
    }
}

/// A tenant's connection, resolved for one sign-in.
#[derive(Clone)]
pub struct Resolved {
    pub space: String,
    pub name: String,
    pub key: String,
    pub role: String,
    pub live: Live,
}

/// What a change to a tenant's owners takes away, for
/// [`TenantConnections::guarded`].
#[derive(Debug, Clone, Copy)]
pub enum Without<'a> {
    /// Nothing: is there such an owner now?
    Nothing,
    /// A member row, by its id: removed, suspended or no longer owner.
    Member(&'a str),
    /// A principal, leaving.
    Principal(&'a str),
    /// One authenticator of one principal, unlinked.
    Authenticator(&'a str, &'a AuthenticatorId),
    /// Every password of a member row's principal: a credential reset.
    MemberPasswords(&'a str),
}

/// How long a guarded change waits for its tenant's lock, by default, and
/// the pause between asks. The lock is asked for without waiting inside
/// Postgres, so a change that is waiting holds no connection the change it
/// waits on may need. `ORDO_AUTH_TENANT_LOCK_WAIT_MS` sets the wait.
pub const LOCK_WAIT_ENV: &str = "ORDO_AUTH_TENANT_LOCK_WAIT_MS";
pub const DEFAULT_LOCK_WAIT_MS: u64 = 10_000;
pub const MIN_LOCK_WAIT_MS: u64 = 100;
pub const MAX_LOCK_WAIT_MS: u64 = 120_000;
const LOCK_PAUSE_MS: u64 = 25;

/// A built provider, with the revision and key it was built from.
type Cached = (i64, String, Live);

/// The tenants' own connections.
pub struct TenantConnections {
    /// [`SETTING`].
    pub enabled: bool,
    /// Which database a tenant's rows are in, and its members.
    pub members: Arc<MemberOps>,
    /// Who holds which authenticators, for the originating-admin rule.
    pub directory: Arc<dyn DirectoryStore>,
    /// The tenant's key custody: the secret is sealed under the tenant's key.
    pub keys: Arc<dyn KeyProvider>,
    /// The ceremony table every redirect sign-in files in.
    pub ceremonies: Arc<dyn CeremonyStore>,
    /// The road to a tenant's provider: the egress kernel under
    /// `DenyList::for_tenant_providers`.
    pub transports: Arc<dyn TransportFactory>,
    /// `ORDO_PUBLIC_URL`, for the callback address.
    pub public_url: String,
    /// `ORDO_AUTH_RETURN_URLS`.
    pub returns: ordo_identity::origin::OriginList,
    /// The deployment's own space, where these ops are refused.
    pub home: String,
    /// The admission the arrival runs through.
    pub admission: Option<Arc<crate::admission::Admission>>,
    /// The deployment's OIDC layer (`ORDO_AUTH_OIDC_*`): cache, clock skew,
    /// algorithms and stale key grace. A tenant's connection carries no
    /// value of its own for these, so it takes the deployment's.
    pub oidc: crate::oidc::OidcDefaults,
    /// `ORDO_AUTH_TENANT_LOCK_WAIT_MS`: how long a guarded change waits for
    /// its tenant's lock.
    pub lock_wait_ms: u64,
    ensured: Mutex<HashSet<String>>,
    cache: Mutex<HashMap<(String, String), Cached>>,
}

impl TenantConnections {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        enabled: bool,
        members: Arc<MemberOps>,
        directory: Arc<dyn DirectoryStore>,
        keys: Arc<dyn KeyProvider>,
        ceremonies: Arc<dyn CeremonyStore>,
        transports: Arc<dyn TransportFactory>,
        public_url: &str,
        returns: ordo_identity::origin::OriginList,
        home: &str,
    ) -> TenantConnections {
        TenantConnections {
            enabled,
            members,
            directory,
            keys,
            ceremonies,
            transports,
            public_url: public_url.trim_end_matches('/').to_string(),
            returns,
            home: home.to_string(),
            admission: None,
            oidc: crate::oidc::OidcDefaults::default(),
            lock_wait_ms: DEFAULT_LOCK_WAIT_MS,
            ensured: Mutex::new(HashSet::new()),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Give a tenant's connections the deployment's OIDC layer.
    pub fn with_oidc_defaults(mut self, oidc: crate::oidc::OidcDefaults) -> Self {
        self.oidc = oidc;
        self
    }

    /// Set [`TenantConnections::lock_wait_ms`].
    pub fn with_lock_wait_ms(mut self, ms: u64) -> Self {
        self.lock_wait_ms = ms;
        self
    }

    pub fn with_admission(mut self, admission: Arc<crate::admission::Admission>) -> Self {
        self.admission = Some(admission);
        self
    }

    /// The address to register at the provider:
    /// `<ORDO_PUBLIC_URL>/auth/callback/t/<tenant>/<name>`.
    pub fn callback_url(&self, space: &str, name: &str) -> String {
        format!("{}/auth/callback/t/{space}/{name}", self.public_url)
    }

    async fn pool(&self, space: &str) -> Result<Pool, ExecError> {
        let pool = self.members.pool_of(space).await?;
        let fresh = !self
            .ensured
            .lock()
            .expect("tenant connections")
            .contains(space);
        if fresh {
            pool.rows(DDL).await.map_err(unavailable)?;
            self.ensured
                .lock()
                .expect("tenant connections")
                .insert(space.to_string());
        }
        Ok(pool)
    }

    async fn rows(&self, space: &str, name: Option<&str>) -> Result<Vec<Row>, ExecError> {
        let pool = self.pool(space).await?;
        let mut sql = format!(
            r#"SELECT "name", "key", "settings", "secret", "state", "rev" FROM "{TABLE}"
               WHERE "$space" = {}"#,
            sql_lit(space)
        );
        if let Some(n) = name {
            sql.push_str(&format!(r#" AND "name" = {}"#, sql_lit(n)));
        }
        sql.push_str(r#" ORDER BY "name""#);
        let got = pool.rows(&sql).await.map_err(unavailable)?;
        Ok(got
            .into_iter()
            .filter_map(|r| {
                let cell = |i: usize| r.get(i).cloned().flatten();
                Some(Row {
                    name: cell(0)?,
                    key: cell(1)?,
                    settings: Settings::from_json(&serde_json::from_str(&cell(2)?).ok()?)?,
                    secret: cell(3)?,
                    state: cell(4)?,
                    rev: cell(5)?.parse().ok()?,
                })
            })
            .collect())
    }

    // ── sealing ──────────────────────────────────────────────────────────

    fn seal(&self, space: &str, key: &str, plain: &str) -> Result<String, ExecError> {
        let sid = ordo_crypto::SpaceId(space.to_string());
        let lease = self
            .keys
            .release(&sid, Purpose::Write, &format!("tenant-connection:{key}"))
            .map_err(|e| custody_fault(&e, "the tenant's key custody could not release a key"))?;
        let sealed = self
            .keys
            .seal_at(
                &lease,
                TABLE,
                "client_secret",
                Anchor::Row(key),
                plain.as_bytes(),
            )
            .map_err(|e| custody_fault(&e, "the client secret could not be sealed"))?;
        Ok(sealed.iter().map(|b| format!("{b:02x}")).collect())
    }

    fn open(&self, space: &str, key: &str, hex: &str) -> Result<String, String> {
        let bytes: Option<Vec<u8>> = (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok())
            .collect();
        let bytes = bytes.ok_or("a sealed secret is not hex")?;
        let sid = ordo_crypto::SpaceId(space.to_string());
        let lease = self
            .keys
            .release(&sid, Purpose::Read, &format!("tenant-connection:{key}"))
            .map_err(|e| custody_words(&e, "the tenant's key custody could not release a key"))?;
        let plain = self
            .keys
            .open_at(&lease, TABLE, "client_secret", Anchor::Row(key), &bytes)
            .map_err(|e| custody_words(&e, "the client secret could not be opened"))?;
        String::from_utf8(plain).map_err(|_| "the client secret is not text".to_string())
    }

    // ── building a provider ──────────────────────────────────────────────

    fn oidc_connection(
        &self,
        space: &str,
        name: &str,
        key: &str,
        s: &Settings,
        secret: String,
    ) -> Result<OidcConnection, String> {
        let endpoints = match (&s.authorize_url, &s.token_url, &s.jwks_url) {
            (None, None, None) => None,
            (Some(a), Some(t), Some(j)) => Some(OidcEndpoints {
                authorize_url: a.clone(),
                token_url: t.clone(),
                jwks_url: j.clone(),
                iss_parameter_required: false,
            }),
            _ => {
                return Err(
                    "`authorizeUrl`, `tokenUrl` and `jwksUrl` are given together or not \
                            at all; give none to use discovery"
                        .into(),
                )
            }
        };
        let conn = OidcConnection {
            name: key.to_string(),
            issuer: s
                .issuer
                .clone()
                .ok_or("an `oidc` connection needs `issuer`")?,
            client_id: s.client_id.clone(),
            client_secret: ClientSecret::new(secret),
            scope: s.scope.clone(),
            endpoints,
            use_email: s.use_email,
            display_claims: s.display_claims.clone(),
            avatar_claim: s.avatar_claim.clone(),
            skew_secs: self.oidc.skew_secs,
            algs: self.oidc.algs.clone(),
            cache_secs: self.oidc.cache_secs,
            stale_grace_secs: self.oidc.stale_grace_secs,
            redirect_uri: self.callback_url(space, name),
            returns: self.returns.clone(),
            // Never the deployment's `on`: a tenant's connection counts only
            // where its own tenant lists its key in `presenceFrom` (§7).
            presence: PresenceRule {
                mode: PresenceMode::Off,
                ..PresenceRule::default()
            },
        };
        conn.check().map_err(|e| e.to_string())?;
        HttpsUrl::parse(&conn.discovery_url())
            .map_err(|e| format!("`issuer`: {}", e.disclosable()))?;
        if let Some(e) = &conn.endpoints {
            for (k, url) in [
                ("authorizeUrl", &e.authorize_url),
                ("tokenUrl", &e.token_url),
                ("jwksUrl", &e.jwks_url),
            ] {
                HttpsUrl::parse(url).map_err(|e| format!("`{k}`: {}", e.disclosable()))?;
            }
        }
        Ok(conn)
    }

    fn oauth2_connection(
        &self,
        space: &str,
        name: &str,
        key: &str,
        s: &Settings,
        secret: String,
    ) -> Result<OAuth2Connection, String> {
        let need = |v: &Option<String>, k: &str| {
            v.clone()
                .ok_or_else(|| format!("an `oauth2` connection needs `{k}`"))
        };
        let conn = OAuth2Connection {
            name: key.to_string(),
            client_id: s.client_id.clone(),
            client_secret: ClientSecret::new(secret),
            authorize_url: need(&s.authorize_url, "authorizeUrl")?,
            token_url: need(&s.token_url, "tokenUrl")?,
            userinfo_url: need(&s.userinfo_url, "userinfoUrl")?,
            scope: s.scope.clone(),
            subject_claim: s.subject_claim.clone(),
            email_claim: s.email_claim.clone(),
            email_verified_claim: s.email_verified_claim.clone(),
            display_claims: s.display_claims.clone(),
            avatar: match &s.avatar_claim {
                Some(c) => AvatarRule::Claim(c.clone()),
                None => AvatarRule::None,
            },
            redirect_uri: self.callback_url(space, name),
            returns: self.returns.clone(),
            mutable_subject_acknowledged: false,
        };
        conn.check().map_err(|e| e.to_string())?;
        for (k, url) in [
            ("authorizeUrl", &conn.authorize_url),
            ("tokenUrl", &conn.token_url),
            ("userinfoUrl", &conn.userinfo_url),
        ] {
            HttpsUrl::parse(url).map_err(|e| format!("`{k}`: {}", e.disclosable()))?;
        }
        Ok(conn)
    }

    /// Check settings by building the connection they describe.
    fn check(&self, space: &str, name: &str, s: &Settings) -> Result<(), String> {
        let key = format!("{}0000000000", tenant_key_prefix(&SpaceId(space.into())));
        match s.kind {
            Kind::Oidc => self
                .oidc_connection(space, name, &key, s, "x".into())
                .map(|_| ()),
            Kind::OAuth2 => self
                .oauth2_connection(space, name, &key, s, "x".into())
                .map(|_| ()),
        }
    }

    fn build(&self, space: &str, row: &Row) -> Result<Live, String> {
        let secret = self.open(space, &row.key, &row.secret)?;
        match row.settings.kind {
            Kind::Oidc => {
                let conn =
                    self.oidc_connection(space, &row.name, &row.key, &row.settings, secret)?;
                OidcProvider::new(conn, self.ceremonies.clone(), self.transports.clone())
                    .map(|p| Live::Oidc(Arc::new(p)))
                    .map_err(|e| e.to_string())
            }
            Kind::OAuth2 => {
                let conn =
                    self.oauth2_connection(space, &row.name, &row.key, &row.settings, secret)?;
                let mut hosts: Vec<String> =
                    [&conn.authorize_url, &conn.token_url, &conn.userinfo_url]
                        .iter()
                        .filter_map(|u| ordo_identity::oauth2::host_of(u))
                        .collect();
                hosts.sort();
                hosts.dedup();
                let transport = self.transports.for_hosts(&hosts);
                OAuth2Provider::new(conn, self.ceremonies.clone(), transport)
                    .map(|p| Live::OAuth2(Arc::new(p)))
                    .map_err(|e| e.to_string())
            }
        }
    }

    /// The active connection `name` of tenant `space`, built. `Ok(None)` for
    /// a tenant or a name that has none, and for a retired one. Read from the
    /// tenant's database on every call; built again only when its revision
    /// or key changed.
    pub async fn resolve(
        &self,
        space: &str,
        name: &str,
    ) -> Result<Option<Resolved>, ordo_identity::StoreFault> {
        if !self.enabled
            || space == self.home
            || !ordo_api::well_formed_space(space)
            || !valid_connection_name(name)
        {
            return Ok(None);
        }
        let rows = self
            .rows(space, Some(name))
            .await
            .map_err(crate::auth::store_failure::exec_fault)?;
        let Some(row) = rows.into_iter().find(|r| r.state == "active") else {
            return Ok(None);
        };
        let ck = (space.to_string(), name.to_string());
        let cached = self
            .cache
            .lock()
            .expect("tenant connections")
            .get(&ck)
            .filter(|(rev, key, _)| *rev == row.rev && *key == row.key)
            .map(|(_, _, live)| live.clone());
        let live = match cached {
            Some(live) => live,
            None => {
                let live = self
                    .build(space, &row)
                    .map_err(ordo_identity::StoreFault::unavailable)?;
                self.cache
                    .lock()
                    .expect("tenant connections")
                    .insert(ck, (row.rev, row.key.clone(), live.clone()));
                live
            }
        };
        Ok(Some(Resolved {
            space: space.to_string(),
            name: row.name,
            key: row.key,
            role: row.settings.role,
            live,
        }))
    }

    /// The tenant's active connections, as a sign-in page shows them:
    /// `(name, kind)` by name. **Empty for every tenant that has none**,
    /// including one that does not exist, the home space, a badly formed
    /// name and a database that cannot be read, so the answer does not
    /// tell a name that exists from one that does not. Never a secret, a
    /// client id or an issuer.
    pub async fn public_list(&self, space: &str) -> Vec<(String, Kind)> {
        if !self.enabled || space == self.home || !ordo_api::well_formed_space(space) {
            return Vec::new();
        }
        match self.rows(space, None).await {
            Ok(rows) => rows
                .into_iter()
                .filter(|r| r.state == "active")
                .map(|r| (r.name, r.settings.kind))
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    // ── the originating admin ────────────────────────────────────────────

    /// Is `id` a sign-in through one of `space`'s own connections?
    pub fn tenant_owned(id: &AuthenticatorId, space: &str) -> bool {
        let prefix = tenant_key_prefix(&SpaceId(space.to_string()));
        let mut parts = id.0.splitn(3, ':');
        matches!(
            (parts.next(), parts.next()),
            (Some("oidc" | "oauth2"), Some(conn)) if conn.starts_with(&prefix)
        )
    }

    /// Would `space` still have an active owner who signs in without its own
    /// providers, after `without`? Always `true` for a tenant with no active
    /// connection, unless `force`. Read on `db`, the transaction holding the
    /// tenant's lock ([`TenantConnections::guarded`]).
    async fn keeps_on(
        &self,
        db: &TxConn,
        space: &str,
        without: Without<'_>,
        force: bool,
    ) -> Result<bool, ExecError> {
        if !force {
            let active = db
                .rows(&format!(
                    r#"SELECT count(*) FROM "{TABLE}" WHERE "$space" = {} AND "state" = 'active'"#,
                    sql_lit(space)
                ))
                .await
                .map_err(unavailable)?;
            let n = active.first().and_then(|r| r.first().cloned().flatten());
            if n.as_deref() == Some("0") {
                return Ok(true);
            }
        }
        let owners = db
            .rows(&format!(
                r#"SELECT "id", "principal" FROM "members"
                   WHERE "$space" = {} AND "role" = 'owner' AND "status" = 'active'
                     AND "kind" = 'principal'"#,
                sql_lit(space)
            ))
            .await
            .map_err(unavailable)?;
        let owners: Vec<(String, String)> = owners
            .into_iter()
            .filter_map(|r| Some((r.first().cloned()??, r.get(1).cloned()??)))
            .filter(|(member, principal)| match without {
                Without::Member(m) => member != m,
                Without::Principal(p) => principal != p,
                _ => true,
            })
            .collect();
        let directory = self.directory.clone();
        let space2 = space.to_string();
        let (gone_p, gone_a, reset) = match without {
            Without::Authenticator(p, a) => (Some(p.to_string()), Some(a.clone()), None),
            Without::MemberPasswords(m) => (None, None, Some(m.to_string())),
            _ => (None, None, None),
        };
        tokio::task::spawn_blocking(move || {
            for (member, principal) in owners {
                let held = directory
                    .authenticators_of(&PrincipalId(principal.clone()))
                    .map_err(|e| e.to_string())?;
                let qualifies = held.iter().any(|a| {
                    let unlinked =
                        gone_p.as_deref() == Some(principal.as_str()) && gone_a.as_ref() == Some(a);
                    let reset =
                        reset.as_deref() == Some(member.as_str()) && a.0.starts_with("password:");
                    !unlinked && !reset && !TenantConnections::tenant_owned(a, &space2)
                });
                if qualifies {
                    return Ok(true);
                }
            }
            Ok::<bool, String>(false)
        })
        .await
        .map_err(|_| unavailable("the directory could not be read"))?
        .map_err(unavailable)
    }

    /// Take `space`'s lock: a transaction holding an advisory lock in the
    /// tenant's own database, so every replica sees it. Asked for without
    /// waiting inside Postgres, and asked again after a pause, so a change
    /// that waits holds no connection between asks.
    ///
    /// The wait is [`TenantConnections::lock_wait_ms`] of wall-clock time,
    /// measured from the call. The first ask always runs, so a wait of 0
    /// asks once and does not wait. Every later ask, and every pause, is
    /// cut at the deadline, so a slow database cannot stretch the wait: the
    /// refusal comes within the setting plus about one round trip.
    pub async fn lock(&self, space: &str) -> Result<TxConn, ExecError> {
        let deadline = Instant::now() + Duration::from_millis(self.lock_wait_ms);
        // The table first, on its own connection, before the lock is held.
        let pool = self.pool(space).await?;
        let key = sql_lit(&format!("ordo-tenant-admin:{space}"));
        let ask = format!("SELECT pg_try_advisory_xact_lock(hashtext({key}))");
        let mut first = true;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if !first && left.is_zero() {
                break;
            }
            let once = Self::ask_once(&pool, &ask);
            let got = if first {
                once.await
            } else {
                // Cut at the deadline. A cancelled ask drops its
                // transaction, which closes its connection and so lets go
                // of anything it took.
                match tokio::time::timeout(left, once).await {
                    Ok(got) => got,
                    Err(_) => break,
                }
            };
            first = false;
            if let Some(tx) = got? {
                return Ok(tx);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            tokio::time::sleep(left.min(Duration::from_millis(LOCK_PAUSE_MS))).await;
        }
        Err(unavailable(format!(
            "another change to this tenant's owners is in progress; ask again. The wait is \
             {LOCK_WAIT_ENV}={}",
            self.lock_wait_ms
        )))
    }

    /// One ask for the lock. `Some` holds it. `None` lost, and its
    /// connection has gone back to the pool.
    async fn ask_once(pool: &Pool, ask: &str) -> Result<Option<TxConn>, ExecError> {
        let tx = pool.begin().await.map_err(unavailable)?;
        match tx.rows(ask).await {
            Ok(r) => {
                let won = r.first().and_then(|c| c.first().cloned().flatten());
                if won.as_deref() == Some("t") {
                    return Ok(Some(tx));
                }
                let _ = tx.rollback().await;
                Ok(None)
            }
            Err(e) => {
                let _ = tx.rollback().await;
                Err(unavailable(e))
            }
        }
    }

    /// Let go of locks [`TenantConnections::lock`] took.
    pub async fn release(held: Vec<TxConn>) {
        for tx in held {
            let _ = tx.commit().await;
        }
    }

    /// Run `write` only if, after it, `space` keeps an owner who signs in
    /// without its own providers (`plan-tenant-providers.md` §8). The check
    /// and the write happen under the tenant's lock, so two changes at the
    /// same moment are taken one after the other: the second is checked
    /// against what the first did.
    pub async fn guarded<T, F, Fut>(
        &self,
        space: &str,
        without: Without<'_>,
        force: bool,
        write: F,
    ) -> Result<T, ExecError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, ExecError>>,
    {
        let tx = self.lock(space).await?;
        match self.keeps_on(&tx, space, without, force).await {
            Ok(true) => {}
            Ok(false) => {
                let _ = tx.rollback().await;
                return Err(no_originating_admin());
            }
            Err(e) => {
                let _ = tx.rollback().await;
                return Err(e);
            }
        }
        let out = write().await;
        let _ = tx.commit().await;
        out
    }

    /// Lock `space` and check it keeps its originating admin after
    /// `without`. The lock is the caller's to [`TenantConnections::release`]
    /// once its write is done. `Ok(None)` when it would not keep one.
    pub async fn hold_if_kept(
        &self,
        space: &str,
        without: Without<'_>,
    ) -> Result<Option<TxConn>, ExecError> {
        let tx = self.lock(space).await?;
        match self.keeps_on(&tx, space, without, false).await {
            Ok(true) => Ok(Some(tx)),
            Ok(false) => {
                let _ = tx.rollback().await;
                Ok(None)
            }
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        }
    }

    /// What a membership command takes away, when it can take away an
    /// owner. `None` for one that cannot.
    pub fn member_change<'a>(op: &str, bound: &'a BoundParams) -> Option<Without<'a>> {
        use crate::member_ops::{REMOVE, SET_ROLE, SUSPEND};
        let text = |n: &str| bound.0.get(n).and_then(Value::as_str).map(str::trim);
        let member = text("member")?;
        (op == REMOVE || op == SUSPEND || (op == SET_ROLE && text("role") != Some("owner")))
            .then_some(Without::Member(member))
    }

    // ── the arrival ──────────────────────────────────────────────────────

    /// A person signed in through `resolved`: admit them to its tenant at
    /// its role, under the deployment's ceiling. The words `/auth/complete`
    /// answers.
    pub async fn arrive(&self, principal: &str, resolved: &Resolved) -> &'static str {
        use crate::admission::Outcome;
        let Some(admission) = &self.admission else {
            return "refused";
        };
        match admission
            .admit_by_connection(principal, &resolved.space, &resolved.role)
            .await
        {
            Ok(Outcome::Admitted { .. }) => "admitted",
            Ok(Outcome::AlreadyMember { .. }) => "already-member",
            _ => "refused",
        }
    }

    // ── the ops ──────────────────────────────────────────────────────────

    /// Run one of the four ops.
    pub async fn execute(
        &self,
        op: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        if !self.enabled {
            return Err(refused(off_message()));
        }
        if space == self.home {
            return Err(refused(
                "the deployment's own space signs in through the app developer's settings \
                 (ORDO_AUTH_METHODS and ORDO_AUTH_<NAME>_*), not through a tenant's connection",
            ));
        }
        match op {
            ADD => self.add(space, bound, caller).await,
            UPDATE => self.update(space, bound, caller).await,
            RETIRE => self.retire(space, bound, caller).await,
            LIST => self.list(space).await,
            other => Err(ExecError::internal(format!(
                "`{other}` is not a tenant connection op"
            ))),
        }
    }

    fn answer_row(&self, space: &str, r: &Row) -> Value {
        let mut v = r.settings.to_json();
        v["name"] = json!(r.name);
        v["key"] = json!(r.key);
        v["state"] = json!(r.state);
        v["secret"] = json!("set");
        v["callback"] = json!(self.callback_url(space, &r.name));
        v
    }

    fn one(&self, space: &str, r: &Row, outcome: &str) -> ExecResult {
        let mut v = self.answer_row(space, r);
        v["outcome"] = json!(outcome);
        ExecResult {
            position: None,
            rows: vec![v],
            next_cursor: None,
        }
    }

    async fn add(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let p = &bound.0;
        let name = text(p, "name").ok_or_else(|| refused("`name` is required"))?;
        if !valid_connection_name(&name) {
            return Err(refused(
                "`name` is 1 to 32 of a-z, 0-9 and -: what a page passes to /auth/begin",
            ));
        }
        let kind = Kind::parse(&text(p, "kind").unwrap_or_default())
            .ok_or_else(|| refused("`kind` is `oidc` or `oauth2`"))?;
        let secret =
            text(p, "clientSecret").ok_or_else(|| refused("`clientSecret` is required"))?;
        let settings = Settings {
            kind,
            issuer: text(p, "issuer"),
            authorize_url: text(p, "authorizeUrl"),
            token_url: text(p, "tokenUrl"),
            jwks_url: text(p, "jwksUrl"),
            userinfo_url: text(p, "userinfoUrl"),
            client_id: text(p, "clientId").ok_or_else(|| refused("`clientId` is required"))?,
            scope: text(p, "scope").unwrap_or_else(|| match kind {
                Kind::Oidc => "openid email profile".into(),
                Kind::OAuth2 => String::new(),
            }),
            display_claims: text(p, "displayClaims")
                .map(|t| list(&t))
                .unwrap_or_else(|| vec!["name".into(), "preferred_username".into()]),
            avatar_claim: text(p, "avatarClaim"),
            use_email: p.get("useEmail").and_then(Value::as_bool).unwrap_or(true),
            subject_claim: text(p, "subjectClaim").unwrap_or_else(|| "sub".into()),
            email_claim: text(p, "emailClaim"),
            email_verified_claim: text(p, "emailVerifiedClaim"),
            role: role_of(p)?.unwrap_or_else(|| crate::admission::DEFAULT_ROLE.into()),
        };
        self.check(space, &name, &settings).map_err(refused)?;
        // One owner stays able to sign in without the tenant's providers,
        // checked under the tenant's lock with the insert.
        self.guarded(space, Without::Nothing, true, || {
            self.insert(space, &name, kind, &settings, &secret, caller)
        })
        .await
    }

    async fn insert(
        &self,
        space: &str,
        name: &str,
        kind: Kind,
        settings: &Settings,
        secret: &str,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let existing = self.rows(space, Some(name)).await?;
        if existing.iter().any(|r| r.state == "active") {
            return Err(ExecError::new(
                ErrorCode::Duplicate,
                format!(
                    "this tenant already has a connection `{name}`; change it with {UPDATE} or \
                     retire it with {RETIRE}"
                ),
            ));
        }
        let rev = existing.first().map(|r| r.rev + 1).unwrap_or(1);
        let mut random = [0u8; 5];
        ordo_crypto::fill_random(&mut random);
        let random: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let key = tenant_key(&SpaceId(space.into()), &random)
            .ok_or_else(|| ExecError::internal("a tenant connection key could not be made"))?;
        let sealed = self.seal(space, &key, secret)?;
        let pool = self.pool(space).await?;
        let at = self.members.at();
        pool.rows(&format!(
            r#"INSERT INTO "{TABLE}" ("$space","name","key","kind","settings","secret","state","rev","updated_by","updated_at")
               VALUES ({s},{n},{k},{kind},{set},{sec},'active',{rev},{by},{at})
               ON CONFLICT ("$space","name") DO UPDATE SET
                 "key" = EXCLUDED."key", "kind" = EXCLUDED."kind",
                 "settings" = EXCLUDED."settings", "secret" = EXCLUDED."secret",
                 "state" = 'active', "rev" = EXCLUDED."rev",
                 "updated_by" = EXCLUDED."updated_by", "updated_at" = EXCLUDED."updated_at"
               WHERE "{TABLE}"."state" <> 'active'"#,
            s = sql_lit(space),
            n = sql_lit(name),
            k = sql_lit(&key),
            kind = sql_lit(kind.as_str()),
            set = sql_lit(&settings.to_json().to_string()),
            sec = sql_lit(&sealed),
            by = sql_lit(caller),
        ))
        .await
        .map_err(unavailable)?;
        let row = self
            .rows(space, Some(name))
            .await?
            .into_iter()
            .find(|r| r.key == key)
            .ok_or_else(|| {
                ExecError::new(
                    ErrorCode::Duplicate,
                    format!("this tenant already has a connection `{name}`"),
                )
            })?;
        self.members.note(
            space,
            caller,
            "connection-added",
            format!(
                "name={name} key={key} kind={} role={}",
                kind.as_str(),
                row.settings.role,
            ),
        );
        Ok(self.one(space, &row, "added"))
    }

    async fn active_row(&self, space: &str, bound: &BoundParams) -> Result<Row, ExecError> {
        let name = text(&bound.0, "name").ok_or_else(|| refused("`name` is required"))?;
        self.rows(space, Some(&name))
            .await?
            .into_iter()
            .find(|r| r.state == "active")
            .ok_or_else(|| {
                ExecError::new(
                    ErrorCode::NotFound,
                    format!("this tenant has no active connection `{name}`"),
                )
            })
    }

    async fn update(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let p = &bound.0;
        let row = self.active_row(space, bound).await?;
        for fixed in ["kind", "issuer"] {
            if let Some(v) = text(p, fixed) {
                let same = match fixed {
                    "kind" => v == row.settings.kind.as_str(),
                    _ => row.settings.issuer.as_deref() == Some(v.as_str()),
                };
                if !same {
                    return Err(refused(format!(
                        "`{fixed}` cannot change: a subject id is unique only within its issuer, \
                         so people would sign in as somebody else. Add a new connection with \
                         {ADD} and retire this one"
                    )));
                }
            }
        }
        let mut s = row.settings.clone();
        let set = |field: &mut Option<String>, k: &str| {
            if let Some(v) = p.get(k).and_then(Value::as_str) {
                let v = v.trim();
                *field = (!v.is_empty()).then(|| v.to_string());
            }
        };
        set(&mut s.authorize_url, "authorizeUrl");
        set(&mut s.token_url, "tokenUrl");
        set(&mut s.jwks_url, "jwksUrl");
        set(&mut s.userinfo_url, "userinfoUrl");
        set(&mut s.avatar_claim, "avatarClaim");
        set(&mut s.email_claim, "emailClaim");
        set(&mut s.email_verified_claim, "emailVerifiedClaim");
        if let Some(v) = text(p, "clientId") {
            s.client_id = v;
        }
        if let Some(v) = text(p, "scope") {
            s.scope = v;
        }
        if let Some(v) = text(p, "displayClaims") {
            s.display_claims = list(&v);
        }
        if let Some(v) = text(p, "subjectClaim") {
            s.subject_claim = v;
        }
        if let Some(v) = p.get("useEmail").and_then(Value::as_bool) {
            s.use_email = v;
        }
        if let Some(r) = role_of(p)? {
            s.role = r;
        }
        self.check(space, &row.name, &s).map_err(refused)?;
        let secret = match text(p, "clientSecret") {
            Some(plain) => self.seal(space, &row.key, &plain)?,
            None => row.secret.clone(),
        };
        let pool = self.pool(space).await?;
        let done = pool
            .rows(&format!(
                r#"UPDATE "{TABLE}" SET "settings" = {set}, "secret" = {sec},
                     "rev" = "rev" + 1, "updated_by" = {by}, "updated_at" = {at}
                   WHERE "$space" = {sp} AND "name" = {n} AND "key" = {k} AND "state" = 'active'
                   RETURNING "rev""#,
                set = sql_lit(&s.to_json().to_string()),
                sec = sql_lit(&secret),
                by = sql_lit(caller),
                at = self.members.at(),
                sp = sql_lit(space),
                n = sql_lit(&row.name),
                k = sql_lit(&row.key),
            ))
            .await
            .map_err(unavailable)?;
        if done.is_empty() {
            return Err(ExecError::new(
                ErrorCode::NotFound,
                format!("this tenant has no active connection `{}`", row.name),
            ));
        }
        self.members.note(
            space,
            caller,
            "connection-updated",
            format!(
                "name={} key={} secret_replaced={}",
                row.name,
                row.key,
                text(p, "clientSecret").is_some()
            ),
        );
        let fresh = Row {
            settings: s,
            rev: row.rev + 1,
            ..row
        };
        Ok(self.one(space, &fresh, "updated"))
    }

    async fn retire(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let row = self.active_row(space, bound).await?;
        let pool = self.pool(space).await?;
        pool.rows(&format!(
            r#"UPDATE "{TABLE}" SET "state" = 'retired', "rev" = "rev" + 1,
                 "updated_by" = {by}, "updated_at" = {at}
               WHERE "$space" = {sp} AND "name" = {n} AND "key" = {k}"#,
            by = sql_lit(caller),
            at = self.members.at(),
            sp = sql_lit(space),
            n = sql_lit(&row.name),
            k = sql_lit(&row.key),
        ))
        .await
        .map_err(unavailable)?;
        self.cache
            .lock()
            .expect("tenant connections")
            .remove(&(space.to_string(), row.name.clone()));
        self.members.note(
            space,
            caller,
            "connection-retired",
            format!("name={} key={}", row.name, row.key),
        );
        let gone = Row {
            state: "retired".into(),
            rev: row.rev + 1,
            ..row
        };
        Ok(self.one(space, &gone, "retired"))
    }

    async fn list(&self, space: &str) -> Result<ExecResult, ExecError> {
        let rows = self.rows(space, None).await?;
        Ok(ExecResult {
            position: None,
            rows: rows.iter().map(|r| self.answer_row(space, r)).collect(),
            next_cursor: None,
        })
    }
}

fn text(p: &Map<String, Value>, k: &str) -> Option<String> {
    p.get(k)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn role_of(p: &Map<String, Value>) -> Result<Option<String>, ExecError> {
    match text(p, "role") {
        None => Ok(None),
        Some(r) if ROLES.contains(&r.as_str()) => Ok(Some(r)),
        Some(_) => Err(refused(
            "`role` is the role a newcomer is admitted at: owner, manager, editor or viewer",
        )),
    }
}

/// The specs of this module's ops.
pub fn specs() -> Vec<OpSpec> {
    let param = |name: &str, ty: ParamTy, required: bool| ParamSpec {
        name: name.to_string(),
        ty,
        required,
        bounds: None,
    };
    let text = |name: &str, max: u32, required: bool| {
        param(name, ParamTy::Text { max_len: Some(max) }, required)
    };
    let out: Vec<FieldSpec> = [
        "name",
        "key",
        "kind",
        "state",
        "issuer",
        "authorizeUrl",
        "tokenUrl",
        "jwksUrl",
        "userinfoUrl",
        "clientId",
        "secret",
        "scope",
        "displayClaims",
        "avatarClaim",
        "useEmail",
        "subjectClaim",
        "emailClaim",
        "emailVerifiedClaim",
        "role",
        "callback",
        "outcome",
    ]
    .iter()
    .map(|f| FieldSpec::new(*f))
    .collect();
    let spec = |name: &str, kind: OpKind, params: Vec<ParamSpec>| OpSpec {
        name: name.to_string(),
        version_hash: PLATFORM_VERSION.to_string(),
        sync: if kind == OpKind::Read {
            Syncability::NotSyncable(NotSyncable::NoAnchor)
        } else {
            Syncability::NotSyncable(NotSyncable::NotARead)
        },
        kind,
        params,
        output_fields: out.clone(),
        live_capable: false,
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    let settings = |name_required: bool, kind_required: bool, secret_required: bool| {
        vec![
            text("name", 32, name_required),
            text("kind", 8, kind_required),
            text("issuer", 2048, false),
            text("authorizeUrl", 2048, false),
            text("tokenUrl", 2048, false),
            text("jwksUrl", 2048, false),
            text("userinfoUrl", 2048, false),
            text("clientId", 1024, kind_required),
            text("clientSecret", 4096, secret_required),
            text("scope", 1024, false),
            text("displayClaims", 512, false),
            text("avatarClaim", 128, false),
            param("useEmail", ParamTy::Bool, false),
            text("subjectClaim", 128, false),
            text("emailClaim", 128, false),
            text("emailVerifiedClaim", 128, false),
            text("role", 32, false),
        ]
    };
    vec![
        spec(ADD, OpKind::Write, settings(true, true, true)),
        spec(UPDATE, OpKind::Write, settings(true, false, false)),
        spec(RETIRE, OpKind::Write, vec![text("name", 32, true)]),
        spec(LIST, OpKind::Read, Vec::new()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_is_off_unless_on_and_names_itself_when_wrong() {
        let none = |_: &str| None;
        assert_eq!(enabled_from_vars(&none), Ok(false));
        let on = |k: &str| (k == SETTING).then(|| "on".to_string());
        assert_eq!(enabled_from_vars(&on), Ok(true));
        let bad = |k: &str| (k == SETTING).then(|| "yes".to_string());
        assert!(enabled_from_vars(&bad).unwrap_err().contains(SETTING));
        assert!(off_message().contains("ORDO_AUTH_TENANT_PROVIDERS=on"));
    }

    #[test]
    fn only_this_tenants_keys_are_its_own_sign_ins() {
        let acme = SpaceId("spc_acme".into());
        let key = tenant_key(&acme, "abcdef0123").unwrap();
        let other = tenant_key(&SpaceId("spc_other".into()), "abcdef0123").unwrap();
        let id = |s: String| AuthenticatorId(s);
        assert!(TenantConnections::tenant_owned(
            &id(format!("oidc:{key}:sub-1")),
            "spc_acme"
        ));
        assert!(TenantConnections::tenant_owned(
            &id(format!("oauth2:{key}:sub-1")),
            "spc_acme"
        ));
        assert!(!TenantConnections::tenant_owned(
            &id(format!("oidc:{other}:sub-1")),
            "spc_acme"
        ));
        assert!(!TenantConnections::tenant_owned(
            &id("oidc:corp:sub-1".into()),
            "spc_acme"
        ));
        assert!(!TenantConnections::tenant_owned(
            &id("passkey:abc".into()),
            "spc_acme"
        ));
    }

    #[test]
    fn settings_round_trip_through_their_json() {
        let s = Settings {
            kind: Kind::Oidc,
            issuer: Some("https://sso.acme.example".into()),
            authorize_url: None,
            token_url: None,
            jwks_url: None,
            userinfo_url: None,
            client_id: "ordo".into(),
            scope: "openid email".into(),
            display_claims: vec!["name".into()],
            avatar_claim: None,
            use_email: false,
            subject_claim: "sub".into(),
            email_claim: None,
            email_verified_claim: None,
            role: "editor".into(),
        };
        assert_eq!(Settings::from_json(&s.to_json()), Some(s.clone()));
        assert!(!s.to_json().to_string().contains("secret"));
    }
}
