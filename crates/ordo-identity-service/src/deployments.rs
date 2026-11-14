//! Many deployments in one identity service (`plan-identity-service.md` 11,
//! owner's go 2026-11-14: "lets build the shared auth").
//!
//! The service has its own handle, `ORDO_IDENTITY_DEPLOYMENT`, called the
//! **home**. Its people are accounts: one person, one set of passkeys, one
//! session, whichever deployment they open. Other deployments register here
//! ([`register`]), each with:
//!
//! - its handle, so its tokens say `aud: ordo-deployment:<handle>`;
//! - its client origins: the pages that may ask for its tokens from a
//!   browser (its console);
//! - its return URLs: where sign-in may send a browser back to.
//!
//! **Who may have a token for which deployment.** A person holds the
//! home's tokens while their account has home access (`people.home`, true
//! for everyone before this slice). A person holds another deployment's
//! tokens only while they are a live member of it ([`is_member`]). So the
//! token says who, and each deployment's people say who may: a member of
//! `hal` cannot sign in to `ordo` with that membership.
//!
//! Tokens for a registered deployment are signed with the home's keys, so
//! every door that trusts this service fetches one key set.
//!
//! Every table row carries the home handle in `deployment`, as every other
//! table of this service does. The registered handle is `handle` in
//! `deployments` and `target` in `deployment_members` and `invites`.

use ordo_exec::{sql_lit, Pool, TxConn};
use ordo_identity::origin::Origin;
use serde_json::{json, Value};

use crate::audit::{self, Entry};
use crate::config::valid_handle;
use crate::error::IdentityError;
use crate::keys::KeyRing;
use crate::people::{Actor, Person};
use crate::store::{cell, dep, num, text};

/// A deployment registered with this service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registered {
    pub handle: String,
    /// The pages that may ask for this deployment's tokens from a browser.
    pub client_origins: Vec<Origin>,
    /// Where sign-in may send a browser back to: URL prefixes, each under
    /// one of the client origins.
    pub return_urls: Vec<String>,
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
            "createdBy": self.created_by,
            "createdAt": self.created_at,
        })
    }

    /// Whether `url` is one of this deployment's return URLs, or under one.
    pub fn allows_return(&self, url: &str) -> bool {
        return_allowed(&self.return_urls, url)
    }
}

/// What a deployment owner asks to register.
#[derive(Debug, Clone, Default)]
pub struct Registration {
    pub handle: String,
    pub client_origins: Vec<String>,
    pub return_urls: Vec<String>,
}

impl Registration {
    /// From a JSON body: `{handle, clientOrigins: [..], returnUrls?: [..]}`.
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
        })
    }

    /// Checked: a handle that is not the home's, at least 1 client origin,
    /// https unless loopback, and every return URL under a client origin.
    /// No return URL given means each client origin's root.
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
        if origins.is_empty() {
            return Err(IdentityError::BadRequest(
                "a deployment needs at least 1 client origin: its console's, such as \
                 https://console.example.com"
                    .into(),
            ));
        }
        let mut returns: Vec<String> = Vec::new();
        for url in self.return_urls.iter().map(|u| u.trim()) {
            let Some((origin, path)) = split_url(url) else {
                return Err(IdentityError::BadRequest(format!(
                    "return URL `{url}` is not an https:// URL with a path and no query"
                )));
            };
            if !origins.contains(&origin) {
                return Err(IdentityError::BadRequest(format!(
                    "return URL `{url}` is not under one of the client origins"
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
        Ok(Registered {
            handle: handle.to_string(),
            client_origins: origins,
            return_urls: returns,
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
    let path = match rest.find('/') {
        Some(i) => &rest[i..],
        None => "/",
    };
    if path.contains(['?', '#']) || unsafe_path(path) {
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
    }
}

const COLUMNS: &str = "handle, client_origins, return_urls, created_by, created_at";

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
/// registered again, keeping its members; a live one is refused.
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
    let done = tx
        .rows(&format!(
            "INSERT INTO deployments (deployment, handle, client_origins, return_urls, \
             created_by, created_at) VALUES ({}, {}, {}, {}, {}, {now}) \
             ON CONFLICT (deployment, handle) DO UPDATE SET client_origins = \
             EXCLUDED.client_origins, return_urls = EXCLUDED.return_urls, created_by = \
             EXCLUDED.created_by, created_at = EXCLUDED.created_at, ended_at = NULL, \
             ended_by = NULL WHERE deployments.ended_at IS NOT NULL RETURNING 1",
            sql_lit(home),
            sql_lit(&reg.handle),
            sql_lit(&origins),
            sql_lit(&returns),
            sql_lit(by.id),
        ))
        .await?;
    if done.is_empty() {
        return Err(IdentityError::DeploymentTaken(reg.handle));
    }
    let detail = format!("clients={origins} returns={returns}");
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

/// End a registration: no new token for it from now on. Its members stay,
/// so registering it again brings them back.
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
            detail: "",
        },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Whether `person` may hold `target`'s tokens: a live account with home
/// access for the home, a live member for any other.
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
             AND p.ended_at IS NULL",
            dep(home),
            sql_lit(target),
            sql_lit(person)
        )
    };
    Ok(!pool.rows(&sql).await?.is_empty())
}

/// Make `person` a member of `target`, inside `tx`. For the home, it
/// gives the account home access. Recorded.
pub async fn add_member(
    tx: &TxConn,
    home: &str,
    keys: &KeyRing,
    target: &str,
    person: &str,
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
        let already = !tx
            .rows(&format!(
                "SELECT 1 FROM deployment_members WHERE {} AND target = {} AND person_id = {} \
                 AND ended_at IS NULL",
                dep(home),
                sql_lit(target),
                sql_lit(person)
            ))
            .await?
            .is_empty();
        if already {
            return Ok(());
        }
        tx.rows(&format!(
            "INSERT INTO deployment_members (deployment, target, person_id, granted_by, \
             created_at) VALUES ({}, {}, {}, {}, {now})",
            sql_lit(home),
            sql_lit(target),
            sql_lit(person),
            sql_lit(by.id)
        ))
        .await?;
    }
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
            detail: target,
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
    let live: Vec<String> = tx
        .rows(&format!(
            "SELECT m.person_id FROM deployment_members m JOIN people p ON p.id = m.person_id \
             WHERE m.{} AND m.target = {} AND m.ended_at IS NULL AND p.ended_at IS NULL \
             ORDER BY m.person_id FOR UPDATE OF m",
            dep(home),
            sql_lit(target)
        ))
        .await?
        .iter()
        .map(|r| text(r, 0))
        .collect();
    if !live.iter().any(|p| p == person) {
        return Err(IdentityError::UnknownPerson);
    }
    if live.len() == 1 {
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

/// The people who are live members of `target`, oldest first. An ended
/// membership shows as ended.
pub async fn members(pool: &Pool, home: &str, target: &str) -> Result<Vec<Person>, IdentityError> {
    Ok(pool
        .rows(&format!(
            "SELECT DISTINCT ON (p.id) p.id, p.display_name, p.role, m.created_at, \
             COALESCE(p.ended_at, m.ended_at) FROM deployment_members m JOIN people p ON \
             p.id = m.person_id WHERE m.{} AND m.target = {} \
             ORDER BY p.id, m.ended_at IS NULL DESC, m.created_at DESC",
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
            reg("hal", &["https://hal.hominem.ai/"], &[]),
            reg(
                "hal",
                &["https://hal.hominem.ai"],
                &["https://evil.example.com/"],
            ),
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
        ] {
            assert!(bad.check("hominem").is_err(), "{bad:?}");
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
