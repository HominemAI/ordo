//! Many deployments in one identity service (`plan-identity-service.md` 11,
//! owner's go 2026-11-14: "lets build the shared auth").
//!
//! The service has its own handle, `ORDO_IDENTITY_DEPLOYMENT`, called the
//! **home**. Its people are accounts: one person, one set of passkeys, one
//! session, whichever deployment they open. Other deployments register here
//! ([`register`]), each with:
//!
//! - its handle, so its tokens say `aud: ordo-deployment:<handle>`;
//! - its client origins: the pages that may call the service from a
//!   browser with the session cookie and ask for its tokens (its console).
//!   Each is trusted: CORS with credentials, and the reauth may run there;
//! - its return URLs: where sign-in may send a browser back to, and, as an
//!   OIDC client, its exact redirect URIs. A return URL may sit on any
//!   origin and makes that origin no caller. An app's callback is
//!   registered here, never as a client origin (review of PRs 26 and 27,
//!   2026-11-19);
//! - its sign-up mode ([`SignUp`]): by invite only, the default, or open,
//!   where anyone may make an account and join it (owner, 2026-11-17).
//!
//! **Who may have a token for which deployment.** A person holds the
//! home's tokens while their account has home access (`people.home`, true
//! for everyone before this slice). A person holds another deployment's
//! tokens only while they are a live member of it ([`is_member`]). So the
//! token says who, and each deployment's people say who may: a member of
//! `hal` cannot sign in to `ordo` with that membership.
//!
//! **Two kinds of member.** An invite, or `member add`, makes an admin
//! member ([`ROLE_ADMIN`]): they hold the deployment's tokens, which are
//! admin tokens (`role: admin` is the only role a token carries). Open
//! sign-up makes an app member ([`ROLE_MEMBER`]): they hold an account and
//! a membership, and no token for the deployment, so a stranger never
//! becomes its admin. App members sign in to apps once the service is an
//! OIDC provider (`plan-identity-service.md` 11, the next step).
//!
//! Tokens for a registered deployment are signed with the home's keys, so
//! every door that trusts this service fetches one key set.
//!
//! Every table row carries the home handle in `deployment`, as every other
//! table of this service does. The registered handle is `handle` in
//! `deployments` and `target` in `deployment_members` and `invites`.

use ordo_exec::{sql_lit, Pool, TxConn};
pub use ordo_identity::deployment_token::ROLE_ADMIN;
use ordo_identity::origin::Origin;
use serde_json::{json, Value};

use crate::audit::{self, Entry};
use crate::config::valid_handle;
use crate::error::IdentityError;
use crate::keys::KeyRing;
use crate::people::{Actor, Person};
use crate::store::{cell, dep, num, text};

/// A member who joined by open sign-up: an account and a membership, and
/// no token for the deployment.
pub const ROLE_MEMBER: &str = "member";

/// How people come to be members of a registered deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SignUp {
    /// Only by an invite, or `member add`. Every row made before open
    /// sign-up reads as this.
    #[default]
    Invite,
    /// Anyone may make an account with a passkey, or join with the one
    /// they hold, and becomes an app member ([`ROLE_MEMBER`]).
    Open,
}

impl SignUp {
    /// The word stored and shown: `invite` or `open`.
    pub fn as_str(self) -> &'static str {
        match self {
            SignUp::Invite => "invite",
            SignUp::Open => "open",
        }
    }

    /// `invite` or `open`.
    pub fn parse(word: &str) -> Result<SignUp, IdentityError> {
        match word.trim() {
            "invite" => Ok(SignUp::Invite),
            "open" => Ok(SignUp::Open),
            other => Err(IdentityError::BadRequest(format!(
                "sign-up mode `{other}` is not one there is; use `invite` or `open`"
            ))),
        }
    }

    /// `signUp` in a JSON body, when it is there.
    pub fn from_body(b: &Value) -> Result<Option<SignUp>, IdentityError> {
        match b.get("signUp") {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(w)) => SignUp::parse(w).map(Some),
            Some(_) => Err(IdentityError::BadRequest(
                "`signUp` is `invite` or `open`, as a string".into(),
            )),
        }
    }
}

/// A deployment registered with this service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registered {
    pub handle: String,
    /// The pages that may call the service with the session cookie and
    /// ask for this deployment's tokens from a browser: its console.
    pub client_origins: Vec<Origin>,
    /// Where sign-in may send a browser back to: URL prefixes on any
    /// origin. The OIDC flow takes one only exactly
    /// ([`Registered::allows_redirect`]).
    pub return_urls: Vec<String>,
    /// How people join it.
    pub sign_up: SignUp,
    pub created_by: String,
    pub created_at: u64,
}

impl Registered {
    pub fn json(&self) -> Value {
        json!({
            "handle": self.handle,
            "audience": ordo_identity::deployment_token::audience(&self.handle),
            "clientOrigins": self.client_origins.iter().map(Origin::to_string).collect::<Vec<_>>(),
            "returnUrls": self.return_urls,
            "trustedAppOrigins": self.trusted_app_origins().iter().map(Origin::to_string).collect::<Vec<_>>(),
            "signUp": self.sign_up.as_str(),
            "createdBy": self.created_by,
            "createdAt": self.created_at,
        })
    }

    /// Client origins that also hold an app's OIDC callback
    /// (`/auth/callback/`), as registrations made before 2026-11-19 did. A
    /// client origin may spend the session cookie, which an app's page
    /// must not; `deployment list` warns about each.
    pub fn trusted_app_origins(&self) -> Vec<Origin> {
        self.client_origins
            .iter()
            .filter(|o| {
                self.return_urls.iter().any(|u| {
                    split_url(u)
                        .is_some_and(|(uo, path)| &uo == *o && path.contains("/auth/callback/"))
                })
            })
            .cloned()
            .collect()
    }

    /// Whether `url` is one of this deployment's return URLs, or under one.
    /// The page's sign-in return (`POST /sign-in/return`) uses this rule.
    pub fn allows_return(&self, url: &str) -> bool {
        return_allowed(&self.return_urls, url)
    }

    /// Whether `uri` is exactly one of this deployment's return URLs: the
    /// same scheme, host, port and path, with no query or fragment. The
    /// OIDC flow's `redirect_uri` uses this rule.
    pub fn allows_redirect(&self, uri: &str) -> bool {
        redirect_allowed(&self.return_urls, uri)
    }
}

/// What a deployment owner asks to register.
#[derive(Debug, Clone, Default)]
pub struct Registration {
    pub handle: String,
    pub client_origins: Vec<String>,
    pub return_urls: Vec<String>,
    /// `invite` unless asked.
    pub sign_up: SignUp,
}

impl Registration {
    /// From a JSON body: `{handle, clientOrigins: [..], returnUrls?: [..],
    /// signUp?: "invite" | "open"}`.
    pub fn from_body(b: &Value) -> Result<Registration, IdentityError> {
        let list = |name: &str| -> Result<Vec<String>, IdentityError> {
            match b.get(name) {
                None | Some(Value::Null) => Ok(Vec::new()),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|v| {
                        v.as_str().map(str::to_string).ok_or_else(|| {
                            IdentityError::BadRequest(format!("`{name}` is a list of strings"))
                        })
                    })
                    .collect(),
                Some(_) => Err(IdentityError::BadRequest(format!(
                    "`{name}` is a list of strings"
                ))),
            }
        };
        Ok(Registration {
            handle: b
                .get("handle")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    IdentityError::BadRequest("`handle` is required, as a string".into())
                })?
                .to_string(),
            client_origins: list("clientOrigins")?,
            return_urls: list("returnUrls")?,
            sign_up: SignUp::from_body(b)?.unwrap_or_default(),
        })
    }

    /// Checked: a handle that is not the home's, at least 1 client origin
    /// or 1 return URL, each https unless loopback. A return URL may sit on
    /// any origin. No return URL given means each client origin's root.
    pub fn check(&self, home: &str) -> Result<Registered, IdentityError> {
        let handle = self.handle.trim();
        if !valid_handle(handle) {
            return Err(IdentityError::BadRequest(format!(
                "`{handle}` is not a deployment handle: lowercase letters, digits and dashes, \
                 at most 63"
            )));
        }
        if handle == home {
            return Err(IdentityError::BadRequest(format!(
                "`{handle}` is this identity service's own handle (ORDO_IDENTITY_DEPLOYMENT); \
                 its clients are ORDO_IDENTITY_CLIENT_ORIGINS"
            )));
        }
        let mut origins: Vec<Origin> = Vec::new();
        for entry in self.client_origins.iter().map(|o| o.trim()) {
            let o = Origin::parse(entry).ok_or_else(|| {
                IdentityError::BadRequest(format!(
                    "client origin `{entry}` is not scheme://host[:port]"
                ))
            })?;
            if o.scheme() == "http" && !o.is_loopback() {
                return Err(IdentityError::BadRequest(format!(
                    "client origin `{entry}` is http:// on a host that is not loopback; use \
                     its https:// origin"
                )));
            }
            if !origins.contains(&o) {
                origins.push(o);
            }
        }
        let mut returns: Vec<String> = Vec::new();
        for url in self.return_urls.iter().map(|u| u.trim()) {
            let Some((origin, path)) = split_url(url) else {
                return Err(IdentityError::BadRequest(format!(
                    "return URL `{url}` is not an https:// URL with a path and no query"
                )));
            };
            if origin.scheme() == "http" && !origin.is_loopback() {
                return Err(IdentityError::BadRequest(format!(
                    "return URL `{url}` is http:// on a host that is not loopback; use its \
                     https:// address"
                )));
            }
            let text = format!("{origin}{path}");
            if !returns.contains(&text) {
                returns.push(text);
            }
        }
        if returns.is_empty() {
            returns = origins.iter().map(|o| format!("{o}/")).collect();
        }
        if returns.is_empty() {
            return Err(IdentityError::BadRequest(
                "a deployment needs at least 1 client origin, its console's (such as \
                 https://console.example.com), or 1 return URL, an app's callback (such as \
                 https://notes.example.com/auth/callback/accounts)"
                    .into(),
            ));
        }
        Ok(Registered {
            handle: handle.to_string(),
            client_origins: origins,
            return_urls: returns,
            sign_up: self.sign_up,
            created_by: String::new(),
            created_at: 0,
        })
    }
}

/// A URL as an origin and a path: `scheme://host[:port]/path`, with no
/// query, fragment, `..` segment or backslash. `None` otherwise.
fn split_url(url: &str) -> Option<(Origin, String)> {
    let origin = Origin::of_url(url)?;
    let (_, rest) = url.split_once("://")?;
    // Where the authority ends, as `Origin::of_url` finds it: the first
    // `/`, `?` or `#`. A query or fragment, before a path or after it, is
    // refused.
    let path = match rest.find(['/', '?', '#']) {
        Some(i) => &rest[i..],
        None => "/",
    };
    if !path.starts_with('/') || path.contains(['?', '#']) || unsafe_path(path) {
        return None;
    }
    Some((origin, path.to_string()))
}

/// A path a browser would rewrite before it lands: `..` or `.` segments,
/// or their escaped forms, or a backslash.
fn unsafe_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains('\\')
        || lower.contains("%2e")
        || lower.contains("%5c")
        || lower.contains("%2f")
        || path.split('/').any(|s| s == ".." || s == ".")
}

/// Whether `asked` is one of `listed` or under one. A listed URL ending in
/// `/` covers everything under it; one that does not covers itself, and
/// itself followed by `/`, `?` or `#`.
pub fn return_allowed(listed: &[String], asked: &str) -> bool {
    let Some(origin) = Origin::of_url(asked) else {
        return false;
    };
    let Some((_, rest)) = asked.split_once("://") else {
        return false;
    };
    let after_host = match rest.find(['/', '?', '#']) {
        Some(i) => &rest[i..],
        None => "",
    };
    let path_end = after_host.find(['?', '#']).unwrap_or(after_host.len());
    let path = match &after_host[..path_end] {
        "" => "/",
        p => p,
    };
    if unsafe_path(path) {
        return false;
    }
    listed.iter().any(|l| {
        let Some((lo, lp)) = split_url(l) else {
            return false;
        };
        if lo != origin {
            return false;
        }
        if lp.ends_with('/') {
            path.starts_with(&lp)
        } else {
            path == lp || path.starts_with(&format!("{lp}/"))
        }
    })
}

/// Whether `asked` is exactly one of `listed`, once both are read as an
/// origin and a path: a query, a fragment or a longer path is refused.
pub fn redirect_allowed(listed: &[String], asked: &str) -> bool {
    if !ordo_identity::origin::plain_url_text(asked) {
        return false;
    }
    let Some((origin, path)) = split_url(asked) else {
        return false;
    };
    let text = format!("{origin}{path}");
    listed.contains(&text)
}

fn row(r: &ordo_exec::TextRow) -> Registered {
    let words = |i: usize| -> Vec<String> {
        text(r, i)
            .split(' ')
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect()
    };
    Registered {
        handle: text(r, 0),
        client_origins: words(1).iter().filter_map(|o| Origin::parse(o)).collect(),
        return_urls: words(2),
        created_by: text(r, 3),
        created_at: num(r, 4).unwrap_or(0),
        sign_up: if text(r, 5) == SignUp::Open.as_str() {
            SignUp::Open
        } else {
            SignUp::Invite
        },
    }
}

const COLUMNS: &str = "handle, client_origins, return_urls, created_by, created_at, sign_up";

/// Every live deployment registered with the service, by handle.
pub async fn list(pool: &Pool, home: &str) -> Result<Vec<Registered>, IdentityError> {
    Ok(pool
        .rows(&format!(
            "SELECT {COLUMNS} FROM deployments WHERE {} AND ended_at IS NULL ORDER BY handle",
            dep(home)
        ))
        .await?
        .iter()
        .map(row)
        .collect())
}

/// One live registered deployment.
pub async fn get(
    pool: &Pool,
    home: &str,
    handle: &str,
) -> Result<Option<Registered>, IdentityError> {
    Ok(pool
        .rows(&format!(
            "SELECT {COLUMNS} FROM deployments WHERE {} AND handle = {} AND ended_at IS NULL",
            dep(home),
            sql_lit(handle)
        ))
        .await?
        .first()
        .map(row))
}

/// Register a deployment. A handle registered before and ended is
/// registered again with no members; a live one is refused, and so is an
/// ended one that still has live members (left from before endings ended
/// them), so nobody comes back under new origins unasked.
pub async fn register(
    pool: &Pool,
    home: &str,
    keys: &KeyRing,
    asked: &Registration,
    by: Actor<'_>,
    now: u64,
) -> Result<Registered, IdentityError> {
    let mut reg = asked.check(home)?;
    let origins = reg
        .client_origins
        .iter()
        .map(Origin::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    let returns = reg.return_urls.join(" ");
    let tx = pool.begin().await?;
    let left = tx
        .rows(&format!(
            "SELECT count(*) FROM deployment_members WHERE {} AND target = {} \
             AND ended_at IS NULL",
            dep(home),
            sql_lit(&reg.handle)
        ))
        .await?
        .first()
        .and_then(|r| num(r, 0))
        .unwrap_or(0);
    if left > 0 {
        return Err(IdentityError::DeploymentHasMembers {
            handle: reg.handle,
            live: left,
        });
    }
    let done = tx
        .rows(&format!(
            "INSERT INTO deployments (deployment, handle, client_origins, return_urls, \
             created_by, created_at, sign_up) VALUES ({}, {}, {}, {}, {}, {now}, {}) \
             ON CONFLICT (deployment, handle) DO UPDATE SET client_origins = \
             EXCLUDED.client_origins, return_urls = EXCLUDED.return_urls, created_by = \
             EXCLUDED.created_by, created_at = EXCLUDED.created_at, sign_up = \
             EXCLUDED.sign_up, ended_at = NULL, ended_by = NULL \
             WHERE deployments.ended_at IS NOT NULL RETURNING 1",
            sql_lit(home),
            sql_lit(&reg.handle),
            sql_lit(&origins),
            sql_lit(&returns),
            sql_lit(by.id),
            sql_lit(reg.sign_up.as_str()),
        ))
        .await?;
    if done.is_empty() {
        return Err(IdentityError::DeploymentTaken(reg.handle));
    }
    let detail = format!(
        "clients={origins} returns={returns} sign-up={}",
        reg.sign_up.as_str()
    );
    audit::append(
        &tx,
        home,
        keys,
        &Entry {
            actor: by.id,
            action: "deployment.registered",
            subject: &reg.handle,
            outcome: "ok",
            client: by.client,
            detail: &detail,
        },
        now,
    )
    .await?;
    tx.commit().await?;
    reg.created_by = by.id.to_string();
    reg.created_at = now;
    Ok(reg)
}

/// Change how people join a registered deployment. The service's own
/// deployment takes people by invite only, so it has no mode to change.
/// Recorded.
pub async fn set_sign_up(
    pool: &Pool,
    home: &str,
    keys: &KeyRing,
    handle: &str,
    mode: SignUp,
    by: Actor<'_>,
    now: u64,
) -> Result<(), IdentityError> {
    if handle == home {
        return Err(IdentityError::HomeInviteOnly(home.to_string()));
    }
    let tx = pool.begin().await?;
    let done = tx
        .rows(&format!(
            "UPDATE deployments SET sign_up = {} WHERE {} AND handle = {} AND ended_at IS NULL \
             RETURNING 1",
            sql_lit(mode.as_str()),
            dep(home),
            sql_lit(handle)
        ))
        .await?;
    if done.is_empty() {
        return Err(IdentityError::UnknownDeployment(handle.to_string()));
    }
    audit::append(
        &tx,
        home,
        keys,
        &Entry {
            actor: by.id,
            action: "deployment.sign-up",
            subject: handle,
            outcome: "ok",
            client: by.client,
            detail: mode.as_str(),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// What [`set_addresses`] replaces: `None` keeps the list as it is.
#[derive(Debug, Clone, Default)]
pub struct Addresses {
    pub client_origins: Option<Vec<String>>,
    pub return_urls: Option<Vec<String>>,
}

impl Addresses {
    /// From a JSON body: `{clientOrigins?: [..], returnUrls?: [..]}`.
    pub fn from_body(b: &Value) -> Result<Addresses, IdentityError> {
        let list = |name: &str| -> Result<Option<Vec<String>>, IdentityError> {
            match b.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|v| {
                        v.as_str().map(str::to_string).ok_or_else(|| {
                            IdentityError::BadRequest(format!("`{name}` is a list of strings"))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(Some),
                Some(_) => Err(IdentityError::BadRequest(format!(
                    "`{name}` is a list of strings"
                ))),
            }
        };
        let out = Addresses {
            client_origins: list("clientOrigins")?,
            return_urls: list("returnUrls")?,
        };
        if out.client_origins.is_none() && out.return_urls.is_none() {
            return Err(IdentityError::BadRequest(
                "give `clientOrigins`, `returnUrls` or both".into(),
            ));
        }
        Ok(out)
    }
}

/// Replace a registered deployment's client origins, its return URLs, or
/// both, checked as registering checks them. Members and the sign-up mode
/// stay. This is how an app registered as a client origin before
/// 2026-11-19 becomes a return address only. Recorded.
pub async fn set_addresses(
    pool: &Pool,
    home: &str,
    keys: &KeyRing,
    handle: &str,
    asked: &Addresses,
    by: Actor<'_>,
    now: u64,
) -> Result<Registered, IdentityError> {
    let Some(was) = get(pool, home, handle).await? else {
        return Err(IdentityError::UnknownDeployment(handle.to_string()));
    };
    let next = Registration {
        handle: handle.to_string(),
        client_origins: asked
            .client_origins
            .clone()
            .unwrap_or_else(|| was.client_origins.iter().map(Origin::to_string).collect()),
        return_urls: asked
            .return_urls
            .clone()
            .unwrap_or_else(|| was.return_urls.clone()),
        sign_up: was.sign_up,
    };
    let mut reg = next.check(home)?;
    let origins = reg
        .client_origins
        .iter()
        .map(Origin::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    let returns = reg.return_urls.join(" ");
    let tx = pool.begin().await?;
    let done = tx
        .rows(&format!(
            "UPDATE deployments SET client_origins = {}, return_urls = {} WHERE {} \
             AND handle = {} AND ended_at IS NULL RETURNING 1",
            sql_lit(&origins),
            sql_lit(&returns),
            dep(home),
            sql_lit(handle)
        ))
        .await?;
    if done.is_empty() {
        return Err(IdentityError::UnknownDeployment(handle.to_string()));
    }
    let detail = format!("clients={origins} returns={returns}");
    audit::append(
        &tx,
        home,
        keys,
        &Entry {
            actor: by.id,
            action: "deployment.addresses",
            subject: handle,
            outcome: "ok",
            client: by.client,
            detail: &detail,
        },
        now,
    )
    .await?;
    tx.commit().await?;
    reg.created_by = was.created_by;
    reg.created_at = was.created_at;
    Ok(reg)
}

/// End a registration: no new token for it from now on, and every live
/// membership ends with it. Registering the handle again starts with no
/// members.
pub async fn end(
    pool: &Pool,
    home: &str,
    keys: &KeyRing,
    handle: &str,
    by: Actor<'_>,
    now: u64,
) -> Result<(), IdentityError> {
    let tx = pool.begin().await?;
    let done = tx
        .rows(&format!(
            "UPDATE deployments SET ended_at = {now}, ended_by = {} WHERE {} AND handle = {} \
             AND ended_at IS NULL RETURNING 1",
            sql_lit(by.id),
            dep(home),
            sql_lit(handle)
        ))
        .await?;
    if done.is_empty() {
        return Err(IdentityError::UnknownDeployment(handle.to_string()));
    }
    let members = tx
        .rows(&format!(
            "UPDATE deployment_members SET ended_at = {now}, ended_by = {} WHERE {} \
             AND target = {} AND ended_at IS NULL RETURNING 1",
            sql_lit(by.id),
            dep(home),
            sql_lit(handle)
        ))
        .await?
        .len();
    let detail = format!("members={members}");
    audit::append(
        &tx,
        home,
        keys,
        &Entry {
            actor: by.id,
            action: "deployment.ended",
            subject: handle,
            outcome: "ok",
            client: by.client,
            detail: &detail,
        },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Whether `person` may hold `target`'s tokens: a live account with home
/// access for the home, a live admin member for any other. An app member
/// ([`ROLE_MEMBER`]) holds none: see [`role_of`].
pub async fn is_member(
    pool: &Pool,
    home: &str,
    target: &str,
    person: &str,
) -> Result<bool, IdentityError> {
    let sql = if target == home {
        format!(
            "SELECT 1 FROM people WHERE {} AND id = {} AND ended_at IS NULL AND home",
            dep(home),
            sql_lit(person)
        )
    } else {
        format!(
            "SELECT 1 FROM deployment_members m JOIN people p ON p.id = m.person_id \
             WHERE m.{} AND m.target = {} AND m.person_id = {} AND m.ended_at IS NULL \
             AND p.ended_at IS NULL AND m.role = {}",
            dep(home),
            sql_lit(target),
            sql_lit(person),
            sql_lit(ROLE_ADMIN)
        )
    };
    Ok(!pool.rows(&sql).await?.is_empty())
}

/// The role of `person`'s live membership of the registered deployment
/// `target`: [`ROLE_ADMIN`] or [`ROLE_MEMBER`], or `None` with none.
pub async fn role_of(
    pool: &Pool,
    home: &str,
    target: &str,
    person: &str,
) -> Result<Option<String>, IdentityError> {
    Ok(pool
        .rows(&format!(
            "SELECT m.role FROM deployment_members m JOIN people p ON p.id = m.person_id \
             WHERE m.{} AND m.target = {} AND m.person_id = {} AND m.ended_at IS NULL \
             AND p.ended_at IS NULL",
            dep(home),
            sql_lit(target),
            sql_lit(person)
        ))
        .await?
        .first()
        .map(|r| text(r, 0)))
}

/// Make `person` a member of `target` with `role` ([`ROLE_ADMIN`] or
/// [`ROLE_MEMBER`]), inside `tx`. For the home, it gives the account home
/// access. An admin asked of an app member makes them an admin; a member
/// asked of an admin changes nothing. Recorded.
#[allow(clippy::too_many_arguments)]
pub async fn add_member(
    tx: &TxConn,
    home: &str,
    keys: &KeyRing,
    target: &str,
    person: &str,
    role: &str,
    by: Actor<'_>,
    now: u64,
) -> Result<(), IdentityError> {
    let live = !tx
        .rows(&format!(
            "SELECT 1 FROM people WHERE {} AND id = {} AND ended_at IS NULL",
            dep(home),
            sql_lit(person)
        ))
        .await?
        .is_empty();
    if !live {
        return Err(IdentityError::UnknownPerson);
    }
    if target == home {
        tx.rows(&format!(
            "UPDATE people SET home = true WHERE {} AND id = {}",
            dep(home),
            sql_lit(person)
        ))
        .await?;
    } else {
        let registered = !tx
            .rows(&format!(
                "SELECT 1 FROM deployments WHERE {} AND handle = {} AND ended_at IS NULL",
                dep(home),
                sql_lit(target)
            ))
            .await?
            .is_empty();
        if !registered {
            return Err(IdentityError::UnknownDeployment(target.to_string()));
        }
        let held = tx
            .rows(&format!(
                "SELECT role FROM deployment_members WHERE {} AND target = {} AND person_id = {} \
                 AND ended_at IS NULL FOR UPDATE",
                dep(home),
                sql_lit(target),
                sql_lit(person)
            ))
            .await?
            .first()
            .map(|r| text(r, 0));
        match held.as_deref() {
            Some(r) if r == role || r == ROLE_ADMIN => return Ok(()),
            Some(_) => {
                tx.rows(&format!(
                    "UPDATE deployment_members SET role = {}, granted_by = {} WHERE {} \
                     AND target = {} AND person_id = {} AND ended_at IS NULL",
                    sql_lit(role),
                    sql_lit(by.id),
                    dep(home),
                    sql_lit(target),
                    sql_lit(person)
                ))
                .await?;
            }
            None => {
                tx.rows(&format!(
                    "INSERT INTO deployment_members (deployment, target, person_id, granted_by, \
                     created_at, role) VALUES ({}, {}, {}, {}, {now}, {})",
                    sql_lit(home),
                    sql_lit(target),
                    sql_lit(person),
                    sql_lit(by.id),
                    sql_lit(role)
                ))
                .await?;
            }
        }
    }
    let detail = if target == home {
        target.to_string()
    } else {
        format!("{target} role={role}")
    };
    audit::append(
        tx,
        home,
        keys,
        &Entry {
            actor: by.id,
            action: "member.added",
            subject: person,
            outcome: "ok",
            client: by.client,
            detail: &detail,
        },
        now,
    )
    .await?;
    Ok(())
}

/// End `person`'s membership of `target` (never the home: ending home
/// access is ending the person, `people::end`). Refuses the last live
/// member, as the home refuses its last admin. Their sessions stay: they
/// may still hold other deployments' tokens.
pub async fn end_member(
    pool: &Pool,
    home: &str,
    keys: &KeyRing,
    target: &str,
    person: &str,
    by: Actor<'_>,
    now: u64,
) -> Result<(), IdentityError> {
    let tx = pool.begin().await?;
    let live: Vec<(String, String)> = tx
        .rows(&format!(
            "SELECT m.person_id, m.role FROM deployment_members m JOIN people p \
             ON p.id = m.person_id WHERE m.{} AND m.target = {} AND m.ended_at IS NULL \
             AND p.ended_at IS NULL ORDER BY m.person_id FOR UPDATE OF m",
            dep(home),
            sql_lit(target)
        ))
        .await?
        .iter()
        .map(|r| (text(r, 0), text(r, 1)))
        .collect();
    let Some((_, role)) = live.iter().find(|(p, _)| p == person) else {
        return Err(IdentityError::UnknownPerson);
    };
    let admins = live.iter().filter(|(_, r)| r == ROLE_ADMIN).count();
    // The last admin of a live deployment stays; an ended one may be
    // emptied, so it can be registered again. App members come and go.
    let registered = !tx
        .rows(&format!(
            "SELECT 1 FROM deployments WHERE {} AND handle = {} AND ended_at IS NULL",
            dep(home),
            sql_lit(target)
        ))
        .await?
        .is_empty();
    if registered && role == ROLE_ADMIN && admins == 1 {
        return Err(IdentityError::LastAdmin);
    }
    tx.rows(&format!(
        "UPDATE deployment_members SET ended_at = {now}, ended_by = {} WHERE {} AND target = {} \
         AND person_id = {} AND ended_at IS NULL",
        sql_lit(by.id),
        dep(home),
        sql_lit(target),
        sql_lit(person)
    ))
    .await?;
    audit::append(
        &tx,
        home,
        keys,
        &Entry {
            actor: by.id,
            action: "member.ended",
            subject: person,
            outcome: "ok",
            client: by.client,
            detail: target,
        },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// The people who are live members of `target`, oldest first, each with
/// their membership's role. An ended membership shows as ended.
pub async fn members(pool: &Pool, home: &str, target: &str) -> Result<Vec<Person>, IdentityError> {
    Ok(pool
        .rows(&format!(
            "SELECT id, display_name, role, since, ended FROM (\
             SELECT DISTINCT ON (p.id) p.id, p.display_name, m.role, m.created_at AS since, \
             COALESCE(p.ended_at, m.ended_at) AS ended FROM deployment_members m JOIN people p \
             ON p.id = m.person_id WHERE m.{} AND m.target = {} \
             ORDER BY p.id, m.ended_at IS NULL DESC, m.created_at DESC) latest \
             ORDER BY since, id",
            dep(home),
            sql_lit(target)
        ))
        .await?
        .iter()
        .map(|r| Person {
            id: text(r, 0),
            display_name: text(r, 1),
            role: text(r, 2),
            created_at: num(r, 3).unwrap_or(0),
            ended_at: cell(r, 4).and_then(|v| v.parse::<u64>().ok()),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(handle: &str, origins: &[&str], returns: &[&str]) -> Registration {
        Registration {
            handle: handle.into(),
            client_origins: origins.iter().map(|s| s.to_string()).collect(),
            return_urls: returns.iter().map(|s| s.to_string()).collect(),
            sign_up: SignUp::Invite,
        }
    }

    #[test]
    fn a_registration_is_checked_and_defaults_its_return_urls() {
        let r = reg("hal", &["https://hal.hominem.ai"], &[])
            .check("hominem")
            .unwrap();
        assert_eq!(r.return_urls, vec!["https://hal.hominem.ai/".to_string()]);
        for bad in [
            reg("Hal", &["https://hal.hominem.ai"], &[]),
            reg("hominem", &["https://hal.hominem.ai"], &[]),
            reg("hal", &[], &[]),
            reg("hal", &["http://hal.hominem.ai"], &[]),
            reg("hal", &[], &["http://hal.hominem.ai/cb"]),
            reg("hal", &["https://hal.hominem.ai/"], &[]),
            reg(
                "hal",
                &["https://hal.hominem.ai"],
                &["https://hal.hominem.ai/a?b"],
            ),
            reg(
                "hal",
                &["https://hal.hominem.ai"],
                &["https://hal.hominem.ai/a/../b"],
            ),
            // A query or fragment before any path: the slash inside it is
            // not a path.
            reg(
                "hal",
                &["https://hal.hominem.ai"],
                &["https://hal.hominem.ai?next=/admin"],
            ),
            reg(
                "hal",
                &["https://hal.hominem.ai"],
                &["https://hal.hominem.ai#/admin"],
            ),
        ] {
            assert!(bad.check("hominem").is_err(), "{bad:?}");
        }
    }

    /// An app is an OIDC client with a return URL and no client origin. A
    /// return URL on an origin no client origin names makes it no caller.
    #[test]
    fn a_return_url_needs_no_client_origin() {
        let app = reg(
            "notes",
            &[],
            &["https://api.notes.example.com/auth/callback/accounts"],
        )
        .check("hominem")
        .unwrap();
        assert!(app.client_origins.is_empty());
        assert_eq!(
            app.return_urls,
            vec!["https://api.notes.example.com/auth/callback/accounts".to_string()]
        );
        let both = reg(
            "hal",
            &["https://hal.hominem.ai"],
            &["https://app.hal.example.com/cb"],
        )
        .check("hominem")
        .unwrap();
        assert_eq!(both.client_origins.len(), 1);
        assert_eq!(both.return_urls, vec!["https://app.hal.example.com/cb"]);
    }

    #[test]
    fn a_redirect_uri_must_be_exactly_a_listed_one() {
        let listed = vec![
            "https://notes.example.com/auth/callback/accounts".to_string(),
            "https://hal.hominem.ai/".to_string(),
        ];
        for ok in [
            "https://notes.example.com/auth/callback/accounts",
            "https://hal.hominem.ai/",
            "https://hal.hominem.ai",
        ] {
            assert!(redirect_allowed(&listed, ok), "{ok}");
        }
        for refused in [
            "https://notes.example.com/auth/callback/accounts/x",
            "https://notes.example.com/auth/callback/accounts?x=1",
            "https://notes.example.com/auth/callback/accounts?",
            "https://notes.example.com/auth/callback/accounts#x",
            "https://notes.example.com/auth/callback/account",
            "https://notes.example.com/auth/callback/",
            "https://hal.hominem.ai/d/hal",
            "http://notes.example.com/auth/callback/accounts",
            "https://notes.example.com:444/auth/callback/accounts",
            "https://notes.example.com/auth/callback/x/../accounts",
            "https://notes.example.com/auth/callback/\taccounts",
        ] {
            assert!(!redirect_allowed(&listed, refused), "{refused}");
        }
    }

    #[test]
    fn a_return_url_must_sit_under_a_listed_one() {
        let listed = vec![
            "https://hal.hominem.ai/".to_string(),
            "https://ordo.example.com/console".to_string(),
        ];
        for ok in [
            "https://hal.hominem.ai/",
            "https://hal.hominem.ai",
            "https://hal.hominem.ai/d/hal/settings?tab=1#x",
            "https://ordo.example.com/console",
            "https://ordo.example.com/console/d/main",
            "https://ordo.example.com/console?x=1",
        ] {
            assert!(return_allowed(&listed, ok), "{ok}");
        }
        for refused in [
            "https://hal.hominem.ai.evil.com/",
            "https://evil.com/?https://hal.hominem.ai/",
            "http://hal.hominem.ai/",
            "https://hal.hominem.ai:444/",
            "https://ordo.example.com/consoleevil",
            "https://ordo.example.com/",
            "https://ordo.example.com/console/../evil",
            "https://ordo.example.com/console/%2e%2e/evil",
            "https://user@hal.hominem.ai/",
            "javascript:alert(1)",
            "//hal.hominem.ai/",
        ] {
            assert!(!return_allowed(&listed, refused), "{refused}");
        }
    }
}
