//! Sign-in through an outside provider, the deployment's half
//! (`plan-auth-federated.md` §3.4, §3.5, §5.3).
//!
//! Three things live here, each the host side of something
//! `ordo_identity::oauth2` defines:
//!
//! - [`EgressTransport`]: the provider's only way to the network, over the
//!   egress kernel's `fetch`. Its allowlist is exactly the connection's token
//!   and userinfo hosts (pin 6), and the deny list is the deployment's
//!   (`ORDO_EGRESS_DENY_CIDRS`), so a connection whose endpoint resolves
//!   inside the perimeter is refused like any sink.
//! - [`connection_from_vars`]: a connection from its settings, with a preset
//!   filling what the app developer did not set. The client secret is read
//!   from a file and never from a variable's value.
//! - `$authconnection` ([`ensure_fingerprint`]) and `$identityattr`
//!   ([`IdentityAttrStore`]): the fingerprint that stops a connection's
//!   endpoints from changing under its name, and the profile a person can read
//!   back through `GET /auth/identities`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ordo_egress::{Allowlist, DenyList, FetchMethod, FetchRequest, HttpsUrl, TlsRoots};
use ordo_exec::{sql_lit, Pool, PoolError, TextRow};
use ordo_identity::federation::{
    form_encode, ClientSecret, FederationMethod, FederationRequest, FederationResponse,
    FederationTransport, ProviderError,
};
use ordo_identity::oauth2::{preset, valid_connection_name, AvatarRule, OAuth2Connection, PRESETS};
use ordo_identity::{AuthenticatedSubject, PrincipalId, Timestamp};

use crate::passkeys::{scrub_fault, Source, SyncPg};

/// How long one request to a provider may take by default, connect to last
/// byte (`ORDO_AUTH_PROVIDER_REQUEST_SECS`).
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

static FETCH_SECS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(FETCH_TIMEOUT.as_secs());

/// Set the provider request timeout from the deployment's setting. Called
/// once at boot.
pub fn set_fetch_timeout_secs(secs: u64) {
    FETCH_SECS.store(secs, std::sync::atomic::Ordering::Relaxed);
}

/// The provider request timeout in force.
pub fn fetch_timeout() -> Duration {
    Duration::from_secs(FETCH_SECS.load(std::sync::atomic::Ordering::Relaxed))
}

/// The provider's road to the network: the egress kernel, with this
/// connection's hosts as the whole allowlist.
pub struct EgressTransport {
    roots: TlsRoots,
    deny: DenyList,
    allow: Allowlist,
    timeout: Duration,
}

impl EgressTransport {
    pub fn new(roots: TlsRoots, deny: DenyList, hosts: &[String]) -> EgressTransport {
        EgressTransport {
            roots,
            deny,
            allow: Allowlist::of(hosts),
            timeout: fetch_timeout(),
        }
    }

    async fn go(
        &self,
        req: &FederationRequest<'_>,
    ) -> Result<FederationResponse, ordo_egress::Refusal> {
        let url = HttpsUrl::parse(req.url)?;
        let dest = ordo_egress::resolve_and_pin(&url, &self.allow, &self.deny).await?;
        // The client secret joins the form here, per call, and is held by
        // nothing (pin 5).
        let mut form = req.form.clone();
        if let Some(secret) = req.client_secret {
            form.push(("client_secret", secret.reveal_to_wire()));
        }
        let body = form_encode(&form);
        let headers: Vec<(&str, String)> = match req.bearer {
            Some(t) => vec![("authorization", format!("Bearer {t}"))],
            None => Vec::new(),
        };
        let method = match req.method {
            FederationMethod::Get => FetchMethod::Get,
            FederationMethod::PostForm => FetchMethod::PostForm,
            _ => {
                return Err(ordo_egress::Refusal::NotRepresentable {
                    detail: "method".into(),
                })
            }
        };
        let fetched = ordo_egress::fetch(
            dest,
            &self.roots,
            FetchRequest {
                method,
                body: body.as_bytes(),
                accept: "application/json",
                headers: &headers,
            },
            req.cap,
            self.timeout,
        )
        .await?;
        let max_age = fetched.max_age();
        Ok(FederationResponse::new(fetched.into_verifier_input()).with_max_age(max_age))
    }
}

impl FederationTransport for EgressTransport {
    /// Blocks on the egress future. Providers run inside `spawn_blocking`, so
    /// this thread may block; outside a runtime it is a refusal.
    fn fetch(&self, req: &FederationRequest<'_>) -> Result<FederationResponse, ProviderError> {
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| ProviderError::unreachable("no-runtime"))?;
        handle
            .block_on(self.go(req))
            .map_err(|r| provider_error(&r))
    }
}

/// The egress refusal as a provider's error, keeping the refusal as its
/// `source()`, the status, the kernel's own retry verdict
/// (`Refusal::retryable`) and the provider's `Retry-After`, so a 5xx or a
/// 429 from the provider is "try again", never a refused sign-in.
pub fn provider_error(r: &ordo_egress::Refusal) -> ProviderError {
    use ordo_egress::Refusal;
    let base = match r {
        Refusal::Rejected { status, .. } => ProviderError::answered(*status, r.retryable()),
        Refusal::MalformedResponse { .. } => ProviderError::bad_response(r.kind()),
        other if other.retryable() => ProviderError::unreachable(other.kind()),
        other => ProviderError::broken(other.kind()),
    };
    base.with_retry_after(r.retry_after())
        .with_source(r.clone())
}

/// The environment prefix of a connection's settings: `ORDO_AUTH_<NAME>_`,
/// the name uppercased with `-` as `_`.
pub fn prefix(name: &str) -> String {
    format!("ORDO_AUTH_{}_", name.to_ascii_uppercase().replace('-', "_"))
}

/// Is `name` a connection rather than a built-in method word? It is when
/// its settings name a preset or a kind.
pub fn names_a_connection(name: &str, var: &dyn Fn(&str) -> Option<String>) -> bool {
    let p = prefix(name);
    var(&format!("{p}PRESET")).is_some() || var(&format!("{p}KIND")).is_some()
}

/// Build one OAuth2 connection from its settings.
///
/// | Setting | |
/// |---|---|
/// | `ORDO_AUTH_<NAME>_PRESET` | `discord` or `github`: fills every endpoint and claim |
/// | `ORDO_AUTH_<NAME>_KIND` | `oauth2`, when there is no preset |
/// | `ORDO_AUTH_<NAME>_CLIENT_ID` | required |
/// | `ORDO_AUTH_<NAME>_CLIENT_SECRET_FILE` | required; a file holding the secret |
/// | `_AUTHORIZE_URL`, `_TOKEN_URL`, `_USERINFO_URL`, `_SCOPE`, `_SUBJECT_CLAIM`, `_EMAIL_CLAIM`, `_EMAIL_VERIFIED_CLAIM`, `_DISPLAY_CLAIMS`, `_AVATAR_CLAIM` | override the preset, or required without one |
/// | `ORDO_AUTH_<NAME>_MUTABLE_SUBJECT` | `acknowledged`, to accept a subject claim that can change (A3) |
///
/// `read_file` reads the secret file. Every error names a setting and never
/// a value, so nothing a deployment owner put in a secret file can reach a
/// boot log.
pub fn connection_from_vars(
    name: &str,
    var: &dyn Fn(&str) -> Option<String>,
    read_file: &dyn Fn(&str) -> Result<String, String>,
    public_url: &str,
    returns: &ordo_identity::origin::OriginList,
) -> Result<OAuth2Connection, String> {
    if !valid_connection_name(name) {
        return Err(format!(
            "`{name}` is not a connection name: 1 to 32 of a-z, 0-9 and -"
        ));
    }
    let p = prefix(name);
    let get = |k: &str| var(&format!("{p}{k}")).filter(|v| !v.trim().is_empty());
    let preset = match get("PRESET") {
        Some(word) => Some(preset(word.trim()).ok_or_else(|| {
            format!(
                "{p}PRESET `{}` is not a preset ({})",
                word.trim(),
                PRESETS.join(", ")
            )
        })?),
        None => None,
    };
    match get("KIND").as_deref().map(str::trim) {
        None if preset.is_some() => {}
        Some("oauth2") => {}
        Some(other @ ("oidc" | "saml")) => {
            return Err(format!(
                "{p}KIND `{other}` is not an OAuth2 kind: an `oidc` connection is read by \
                 crate::oidc, and `saml` is not built yet"
            ))
        }
        Some(other) => return Err(format!("{p}KIND `{other}` is not a kind (oauth2)")),
        None => return Err(format!("{p}PRESET or {p}KIND is required")),
    }
    let pick = |k: &str, from_preset: Option<&str>| -> Result<String, String> {
        get(k)
            .map(|v| v.trim().to_string())
            .or_else(|| from_preset.map(str::to_string))
            .ok_or_else(|| format!("{p}{k} is required"))
    };
    let optional = |k: &str, from_preset: Option<&str>| -> Option<String> {
        get(k)
            .map(|v| v.trim().to_string())
            .or_else(|| from_preset.map(str::to_string))
    };
    let client_id = pick("CLIENT_ID", None)?;
    let secret_file = pick("CLIENT_SECRET_FILE", None)?;
    if var(&format!("{p}CLIENT_SECRET")).is_some() {
        return Err(format!(
            "{p}CLIENT_SECRET is not read: a secret in a variable's value shows in a process \
             listing; put it in a file and name the file in {p}CLIENT_SECRET_FILE"
        ));
    }
    let secret =
        read_file(&secret_file).map_err(|_| format!("{p}CLIENT_SECRET_FILE could not be read"))?;
    let pr = preset.as_ref();
    let display_claims = match get("DISPLAY_CLAIMS") {
        Some(list) => list
            .split(',')
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect(),
        None => pr
            .map(|x| x.display_claims.iter().map(|c| c.to_string()).collect())
            .unwrap_or_default(),
    };
    let avatar = match get("AVATAR_CLAIM") {
        Some(c) => AvatarRule::Claim(c.trim().to_string()),
        None => pr.map(|x| x.avatar.rule()).unwrap_or(AvatarRule::None),
    };
    let mutable_subject_acknowledged = match get("MUTABLE_SUBJECT").as_deref().map(str::trim) {
        None => false,
        Some("acknowledged") => true,
        Some(_) => return Err(format!("{p}MUTABLE_SUBJECT is `acknowledged` or unset")),
    };
    let conn = OAuth2Connection {
        name: name.to_string(),
        client_id,
        client_secret: ClientSecret::new(secret),
        authorize_url: pick("AUTHORIZE_URL", pr.map(|x| x.authorize_url))?,
        token_url: pick("TOKEN_URL", pr.map(|x| x.token_url))?,
        userinfo_url: pick("USERINFO_URL", pr.map(|x| x.userinfo_url))?,
        scope: pick("SCOPE", pr.map(|x| x.scope))?,
        subject_claim: pick("SUBJECT_CLAIM", pr.map(|x| x.subject_claim))?,
        email_claim: optional("EMAIL_CLAIM", pr.and_then(|x| x.email_claim)),
        email_verified_claim: optional(
            "EMAIL_VERIFIED_CLAIM",
            pr.and_then(|x| x.email_verified_claim),
        ),
        display_claims,
        avatar,
        redirect_uri: redirect_uri(public_url, name),
        returns: returns.clone(),
        mutable_subject_acknowledged,
    };
    conn.check()
        .map_err(|e| format!("connection `{name}`: {e}"))?;
    for (k, url) in [
        ("AUTHORIZE_URL", &conn.authorize_url),
        ("TOKEN_URL", &conn.token_url),
        ("USERINFO_URL", &conn.userinfo_url),
    ] {
        HttpsUrl::parse(url).map_err(|e| format!("{p}{k}: {}", e.disclosable()))?;
    }
    Ok(conn)
}

/// The one redirect URI of a connection: `<ORDO_PUBLIC_URL>/auth/callback/<name>`.
pub fn redirect_uri(public_url: &str, name: &str) -> String {
    format!("{}/auth/callback/{name}", public_url.trim_end_matches('/'))
}

/// Parse `ORDO_AUTH_RETURN_URLS`: comma-separated `https://` addresses (or
/// `http://localhost` for a page served on the developer's own machine), no
/// fragment. Each is matched **by origin, with any path** (owner ruling
/// 2026-10-09, `subdomain-signin` ask 2), so an address written before the
/// ruling still works and its siblings on the same origin now do too.
/// `wildcards` is `ORDO_WILDCARD_ORIGINS`: with it on, `https://*.example.com`
/// matches one label under `example.com`; with it off, such an entry is
/// refused by name.
pub fn parse_return_urls(
    text: &str,
    wildcards: bool,
) -> Result<ordo_identity::origin::OriginList, String> {
    let list = ordo_identity::origin::OriginList::parse(text, true, wildcards)?;
    for p in list.entries() {
        if let ordo_identity::origin::OriginPattern::Exact(o) = p {
            if o.scheme() == "http" && !o.is_loopback() {
                return Err(format!(
                    "`{o}` is not a return address: https:// (or http://localhost), no fragment"
                ));
            }
        }
    }
    Ok(list)
}

// ── $authconnection ─────────────────────────────────────────────────────

pub const CONNECTION_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$authconnection" (
  "name"        text   NOT NULL,
  "kind"        text   NOT NULL,
  "fingerprint" text   NOT NULL,
  "first_seen"  bigint NOT NULL,
  PRIMARY KEY ("name")
)"#;

/// Record a connection's fingerprint the first time its name is served, and
/// refuse a boot that finds a different one (§3.5).
///
/// A changed issuer or endpoint set under the same name would re-point every
/// `oauth2:<name>:<sub>` link at another provider's subjects, where a `sub`
/// can collide with, or be chosen to match, one of the old ones.
pub async fn ensure_fingerprint(
    pool: &Pool,
    name: &str,
    kind: &str,
    fingerprint: &str,
    at: u64,
) -> Result<(), String> {
    if pool.rows(CONNECTION_DDL).await.is_err() {
        pool.rows(CONNECTION_DDL).await.map_err(|e| e.to_string())?;
    }
    pool.rows(&format!(
        r#"INSERT INTO "$authconnection" ("name", "kind", "fingerprint", "first_seen")
           VALUES ({}, {}, {}, {at}) ON CONFLICT ("name") DO NOTHING"#,
        sql_lit(name),
        sql_lit(kind),
        sql_lit(fingerprint),
    ))
    .await
    .map_err(|e| e.to_string())?;
    let rows = pool
        .rows(&format!(
            r#"SELECT "kind", "fingerprint" FROM "$authconnection" WHERE "name" = {}"#,
            sql_lit(name)
        ))
        .await
        .map_err(|e| e.to_string())?;
    let row = rows.first().ok_or("the connection row did not persist")?;
    let same = row.first().and_then(|c| c.as_deref()) == Some(kind)
        && row.get(1).and_then(|c| c.as_deref()) == Some(fingerprint);
    if !same {
        return Err(format!(
            "connection `{name}` was first served with other endpoints or another subject \
             claim. Every account linked through it names a subject of that provider, so \
             it keeps its endpoints for life: give the new provider a new connection name"
        ));
    }
    Ok(())
}

// ── $identityattr ───────────────────────────────────────────────────────

pub const IDENTITY_ATTR_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$identityattr" (
  "authenticator" text   NOT NULL,
  "principal"     text   NOT NULL,
  "display"       text,
  "avatar"        text,
  "email"         text,
  "linked_at"     bigint NOT NULL,
  "seen_at"       bigint NOT NULL,
  PRIMARY KEY ("authenticator")
)"#;

/// Create `$identityattr`. Retry-once, as the other `ensure_table`s are.
pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    if pool.rows(IDENTITY_ATTR_DDL).await.is_err() {
        pool.rows(IDENTITY_ATTR_DDL).await?;
    }
    Ok(())
}

/// One linked identity as its owner reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityRow {
    pub authenticator: String,
    pub display: Option<String>,
    pub avatar: Option<String>,
    pub email: Option<String>,
    pub linked_at: u64,
}

/// What the store could not do. Names the table, never a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityAttrError(pub ordo_identity::StoreFault);

/// The profile an outside provider vouched for, beside the link. Written at
/// each sign-in, so it is as fresh as the last one. Read only by the person
/// it describes (`GET /auth/identities`).
pub trait IdentityAttrStore: Send + Sync {
    /// Keep what `subject` carries for `principal`, at `at`. The first
    /// instant is kept as `linked_at`.
    fn note(
        &self,
        principal: &PrincipalId,
        subject: &AuthenticatedSubject,
        at: Timestamp,
    ) -> Result<(), IdentityAttrError>;

    /// Every row for `principal`, sorted by authenticator.
    fn of(&self, principal: &PrincipalId) -> Result<Vec<IdentityRow>, IdentityAttrError>;
}

/// Process-local. **Tests only.**
#[derive(Default)]
pub struct MemoryIdentityAttrs {
    rows: Mutex<std::collections::BTreeMap<String, (String, IdentityRow)>>,
}

impl IdentityAttrStore for MemoryIdentityAttrs {
    fn note(
        &self,
        principal: &PrincipalId,
        subject: &AuthenticatedSubject,
        at: Timestamp,
    ) -> Result<(), IdentityAttrError> {
        let mut rows = self.rows.lock().expect("identity attrs");
        let key = subject.authenticator.0.clone();
        let linked_at = rows.get(&key).map(|(_, r)| r.linked_at).unwrap_or(at.0);
        let a = &subject.attributes;
        rows.insert(
            key.clone(),
            (
                principal.0.clone(),
                IdentityRow {
                    authenticator: key,
                    display: a.display.clone(),
                    avatar: a.avatar.clone(),
                    email: a.email.as_ref().map(|e| e.address().to_string()),
                    linked_at,
                },
            ),
        );
        Ok(())
    }

    fn of(&self, principal: &PrincipalId) -> Result<Vec<IdentityRow>, IdentityAttrError> {
        Ok(self
            .rows
            .lock()
            .expect("identity attrs")
            .values()
            .filter(|(p, _)| *p == principal.0)
            .map(|(_, r)| r.clone())
            .collect())
    }
}

/// The Postgres store, over the control database.
pub struct PgIdentityAttrs {
    db: SyncPg,
}

impl PgIdentityAttrs {
    /// On a connection the deployment already holds. Refuses when the table
    /// does not exist.
    pub fn open_on(pool: Pool) -> Result<PgIdentityAttrs, IdentityAttrError> {
        let db = SyncPg::start(Source::Shared(pool), "ordo-identityattr")
            .map_err(|e| IdentityAttrError(e.into()))?;
        let store = PgIdentityAttrs { db };
        store.sql(r#"SELECT 1 FROM "$identityattr" LIMIT 1"#)?;
        Ok(store)
    }

    fn sql(&self, sql: &str) -> Result<Vec<TextRow>, IdentityAttrError> {
        self.db
            .sql(sql)
            .map_err(|e| IdentityAttrError(scrub_fault("$identityattr", e)))
    }
}

fn lit_or_null(v: Option<&str>) -> String {
    v.map(sql_lit).unwrap_or_else(|| "NULL".to_string())
}

impl IdentityAttrStore for PgIdentityAttrs {
    fn note(
        &self,
        principal: &PrincipalId,
        subject: &AuthenticatedSubject,
        at: Timestamp,
    ) -> Result<(), IdentityAttrError> {
        let a = &subject.attributes;
        let email = a.email.as_ref().map(|e| e.address().to_string());
        self.sql(&format!(
            r#"INSERT INTO "$identityattr"
                 ("authenticator", "principal", "display", "avatar", "email", "linked_at", "seen_at")
               VALUES ({}, {}, {}, {}, {}, {at}, {at})
               ON CONFLICT ("authenticator") DO UPDATE SET
                 "principal" = EXCLUDED."principal", "display" = EXCLUDED."display",
                 "avatar" = EXCLUDED."avatar", "email" = EXCLUDED."email",
                 "seen_at" = EXCLUDED."seen_at""#,
            sql_lit(&subject.authenticator.0),
            sql_lit(&principal.0),
            lit_or_null(a.display.as_deref()),
            lit_or_null(a.avatar.as_deref()),
            lit_or_null(email.as_deref()),
            at = at.0,
        ))?;
        Ok(())
    }

    fn of(&self, principal: &PrincipalId) -> Result<Vec<IdentityRow>, IdentityAttrError> {
        let rows = self.sql(&format!(
            r#"SELECT "authenticator", "display", "avatar", "email", "linked_at"
               FROM "$identityattr" WHERE "principal" = {} ORDER BY "authenticator""#,
            sql_lit(&principal.0)
        ))?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                Some(IdentityRow {
                    authenticator: r.first()?.clone()?,
                    display: r.get(1).cloned().flatten(),
                    avatar: r.get(2).cloned().flatten(),
                    email: r.get(3).cloned().flatten(),
                    linked_at: r.get(4)?.as_deref()?.parse().ok()?,
                })
            })
            .collect())
    }
}

/// The transport a deployment builds for one connection: the deployment's
/// trust anchors (the public root program, plus `ORDO_AUTH_PROVIDER_CA_FILE`
/// when set), and its deny list.
pub fn deployment_transport(
    conn: &OAuth2Connection,
    roots: TlsRoots,
    deny: DenyList,
) -> Arc<dyn FederationTransport> {
    Arc::new(EgressTransport::new(roots, deny, &conn.hosts()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_egress_refusal_keeps_its_status_retry_after_and_source() {
        use ordo_egress::Refusal;
        use ordo_identity::federation::ProviderClass;
        use std::error::Error as _;
        let r = Refusal::Rejected {
            status: 429,
            retry_after_secs: Some(30),
        };
        let e = super::provider_error(&r);
        assert_eq!(e.class(), ProviderClass::Busy);
        assert_eq!(e.status(), Some(429));
        assert_eq!(e.retry_after(), Some(std::time::Duration::from_secs(30)));
        assert!(e.source().is_some());
        let r = Refusal::Rejected {
            status: 403,
            retry_after_secs: None,
        };
        assert_eq!(super::provider_error(&r).class(), ProviderClass::Refused);
        let r = Refusal::NotHttps { detail: "x".into() };
        assert_eq!(super::provider_error(&r).class(), ProviderClass::Broken);
        let r = Refusal::MalformedResponse { detail: "x".into() };
        assert_eq!(
            super::provider_error(&r).class(),
            ProviderClass::BadResponse
        );
    }

    #[test]
    fn return_urls_accept_a_local_subdomain_and_nothing_broader() {
        let ok = super::parse_return_urls(
            "http://login.localhost:5180, http://shop.localhost:5181/x, http://localhost:5182",
            false,
        )
        .expect("local origins");
        assert!(ok.allows_url("http://shop.localhost:5181/anywhere"));
        for bad in [
            "http://a.b.localhost:5181",
            "http://shop.localhost.evil.com",
            "http://example.com",
        ] {
            assert!(super::parse_return_urls(bad, false).is_err(), "{bad}");
        }
    }

    use super::*;
    use std::collections::BTreeMap;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| m.get(k).cloned()
    }

    fn secret(_: &str) -> Result<String, String> {
        Ok("the-secret\n".into())
    }

    fn returns() -> ordo_identity::origin::OriginList {
        parse_return_urls("https://app.example.com/signed-in", false).expect("a list")
    }

    #[test]
    fn a_preset_and_a_client_id_and_a_secret_file_are_a_whole_connection() {
        let v = vars(&[
            ("ORDO_AUTH_DISCORD_PRESET", "discord"),
            ("ORDO_AUTH_DISCORD_CLIENT_ID", "1234"),
            (
                "ORDO_AUTH_DISCORD_CLIENT_SECRET_FILE",
                "/run/secrets/discord",
            ),
        ]);
        let c = connection_from_vars(
            "discord",
            &v,
            &secret,
            "https://api.example.com/",
            &returns(),
        )
        .expect("a connection");
        assert_eq!(
            c.redirect_uri,
            "https://api.example.com/auth/callback/discord"
        );
        assert_eq!(c.client_secret.reveal_to_wire(), "the-secret");
        assert_eq!(c.subject_claim, "id");
        assert_eq!(c.hosts(), vec!["discord.com".to_string()]);
    }

    #[test]
    fn a_secret_in_a_value_and_an_unreadable_file_refuse_without_saying_the_secret() {
        let v = vars(&[
            ("ORDO_AUTH_GH_PRESET", "github"),
            ("ORDO_AUTH_GH_CLIENT_ID", "1"),
            ("ORDO_AUTH_GH_CLIENT_SECRET_FILE", "/nope"),
            ("ORDO_AUTH_GH_CLIENT_SECRET", "hunter2"),
        ]);
        let e = connection_from_vars("gh", &v, &secret, "https://a.example", &returns())
            .err()
            .expect("refused");
        assert!(!e.contains("hunter2"), "{e}");
        let v = vars(&[
            ("ORDO_AUTH_GH_PRESET", "github"),
            ("ORDO_AUTH_GH_CLIENT_ID", "1"),
            ("ORDO_AUTH_GH_CLIENT_SECRET_FILE", "/nope"),
        ]);
        let unreadable = |_: &str| -> Result<String, String> { Err("secret-ish detail".into()) };
        let e = connection_from_vars("gh", &v, &unreadable, "https://a.example", &returns())
            .err()
            .expect("refused");
        assert!(
            e.contains("CLIENT_SECRET_FILE") && !e.contains("secret-ish"),
            "{e}"
        );
    }

    #[test]
    fn oidc_and_saml_are_refused_by_name_until_built() {
        let v = vars(&[
            ("ORDO_AUTH_CORP_KIND", "oidc"),
            ("ORDO_AUTH_CORP_CLIENT_ID", "1"),
            ("ORDO_AUTH_CORP_CLIENT_SECRET_FILE", "/s"),
        ]);
        let e = connection_from_vars("corp", &v, &secret, "https://a.example", &returns())
            .err()
            .expect("refused");
        assert!(e.contains("oidc"), "{e}");
    }

    #[test]
    fn return_addresses_are_https_or_a_local_page() {
        assert!(parse_return_urls("https://a.example/x, http://localhost:5173/cb", false).is_ok());
        assert!(parse_return_urls("http://a.example/x", false).is_err());
        assert!(parse_return_urls("http://localhost.evil.example/x", false).is_err());
        assert!(parse_return_urls("https://a.example/x#y", false).is_err());
        let off = parse_return_urls("https://*.a.example", false).unwrap_err();
        assert!(off.contains("ORDO_WILDCARD_ORIGINS=on"), "{off}");
        assert!(parse_return_urls("https://*.a.example", true).is_ok());
    }
}
