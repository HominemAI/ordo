//! A verified email address on an account (owner ruling 2026-10-22: a
//! person who signs in by passkey or an outside connection can add a
//! verified email, "yes - but app dev could make it required").
//!
//! # What it is for
//!
//! Mail from the app, such as a routine's `notify`, reaches a person only
//! at an address Ordo knows is theirs. Before this, that was only the
//! address of an emailed-code sign-in. Now a person adds one:
//!
//! 1. `POST /auth/email/begin {address}` with a session's bearer token. A
//!    code is mailed to the address, with the same ceremony, limits and
//!    settings as an emailed sign-in code ([`crate::email_code`]).
//! 2. `POST /auth/email/complete {challenge, binder, code}`. Only then is the
//!    address stored. A change mails the old address to say so.
//! 3. `POST /auth/email/remove` takes it off. The old address is told.
//!
//! `GET /auth/email` reads it back, with what the app developer requires.
//!
//! # Where it lives
//!
//! `$contactemail` in the control database: one row per person, with the
//! address, where it came from (`code`, or the connection that vouched for
//! it) and when. [`ContactEmails::address_of`] reads it, and falls back to the
//! address of an emailed-code sign-in, so the mail path asks one place.
//!
//! # An outside connection's verified address counts
//!
//! A sign-in whose provider marked the address verified (Discord's
//! `verified: true`, an OIDC `email_verified`) stores it, when the person
//! has none yet. It never replaces one the person chose.
//!
//! # Required, if the app developer says so
//!
//! Optional by default. Two layers, as the session settings have:
//!
//! | Layer | Sets | How |
//! |---|---|---|
//! | Deployment | the default | `ORDO_EMAIL_REQUIRED`: `off`, `all`, or a comma list of ops |
//! | App developer | its own value, over the deployment's | `std.deployment.setEmailPolicy {required}` |
//!
//! `all` is every op of the app: its own and the derived `std.<entity>.*`.
//! Platform ops (`std.space.*` and the rest) are never covered by `all`, so
//! a person can always manage their account. A named list covers exactly
//! the ops named. A signed-in person without a verified address who calls a
//! covered op is refused `403 email_required`, and the message names the
//! setting. A share link's or a delegated token is not a person signing in,
//! and is never asked.
//!
//! # A change or a removal asks for a passkey touch
//!
//! Owner, 2026-10-22: "Yes by default... But this is the app devs choice."
//! Changing the address, or removing it, needs a fresh passkey assertion
//! (`POST /auth/step-up`) by default, since an address is where recovery
//! and notices go. Adding a first address needs none. Two layers, as above:
//!
//! | Layer | Sets | How |
//! |---|---|---|
//! | Deployment | the default | `ORDO_EMAIL_CHANGE_TOUCH`: `on` (default) or `off` |
//! | App developer | its own value, over the deployment's | `std.deployment.setEmailPolicy {changeTouch}`: `on`, `off` or `default` |
//!
//! The refusal is `403 presence_required` and names the setting.
//!
//! # When the address table cannot be read
//!
//! A covered op is **refused**, not let through: `503 space_unavailable`
//! with `Retry-After`, naming the setting. The same call lands once the
//! table is back. Letting it through would drop a requirement the app
//! developer set.
//!
//! # Nothing leaks an address
//!
//! No log line and no security event carries an address. The events say
//! who, what and where it came from.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_exec::{sql_lit, Pool, PoolError};
use serde_json::{json, Value};

use crate::email_code::EmailCodeProvider;
use crate::mail::{Composer, MailAddress, MailError, MailJob, MailQueue, Message, QueuedMail};
use ordo_identity::StoreFault;

/// The deployment's setting: `off` (the default), `all`, or a comma list of
/// op names.
pub const REQUIRED_ENV: &str = "ORDO_EMAIL_REQUIRED";
/// The app developer's layer.
pub const SET_POLICY: &str = "std.deployment.setEmailPolicy";
/// Every op of this module.
pub const OPS: [&str; 1] = [SET_POLICY];
/// The output fields of [`SET_POLICY`].
pub const FIELDS: [&str; 6] = [
    "required",
    "source",
    "deployment",
    "app",
    "changeTouch",
    "changeTouchSource",
];

/// The deployment's setting: `on` (the default) or `off`. Whether changing
/// or removing a contact email asks for a fresh passkey touch.
pub const TOUCH_ENV: &str = "ORDO_EMAIL_CHANGE_TOUCH";

/// `on` or `off`; unset is `on`.
pub fn touch_from_vars(var: &dyn Fn(&str) -> Option<String>) -> Result<bool, String> {
    match var(TOUCH_ENV).as_deref().map(str::trim) {
        None | Some("") | Some("on") => Ok(true),
        Some("off") => Ok(false),
        Some(other) => Err(format!(
            "{TOUCH_ENV}: `{other}` is not on or off; the default is on"
        )),
    }
}
/// The outbox kind of the message that tells the old address.
pub const CHANGED_KIND: &str = "account.email-changed";
/// Its subject.
pub const CHANGED_SUBJECT: &str = "The email address on your account changed";
/// Where an address confirmed by an emailed code came from.
pub const SOURCE_CODE: &str = "code";
/// Where the address of an emailed-code sign-in came from.
pub const SOURCE_SIGN_IN: &str = "sign-in";

pub(crate) const APP_KEY: &str = "email_policy";
/// How long a node keeps the app developer's layer before reading it again.
const APP_CACHE_SECS: u64 = 5;

pub const DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$contactemail" (
  "principal"   text   NOT NULL,
  "address"     text   NOT NULL,
  "source"      text   NOT NULL,
  "verified_at" bigint NOT NULL,
  PRIMARY KEY ("principal")
)"#;

/// Create `$contactemail` and the settings table. Retry-once, as the other
/// `ensure_table`s are.
pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    for ddl in [crate::admission::SETTING_DDL, DDL] {
        if pool.rows(ddl).await.is_err() {
            pool.rows(ddl).await?;
        }
    }
    Ok(())
}

/// Which ops need a verified address.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Requirement {
    #[default]
    Off,
    /// Every op of the app; never a platform op.
    All,
    /// Exactly these ops.
    Ops(BTreeSet<String>),
}

impl Requirement {
    /// `off`, `all`, or a comma list of op names. Anything else names the
    /// setting it came from.
    pub fn parse(text: &str) -> Result<Requirement, String> {
        let t = text.trim();
        match t {
            "" | "off" => return Ok(Requirement::Off),
            "all" => return Ok(Requirement::All),
            _ => {}
        }
        let mut ops = BTreeSet::new();
        for name in t.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            let ok = name.len() <= 128
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-');
            if !ok {
                return Err(format!(
                    "`{name}` is not an op name: write off, all, or a comma list of op names"
                ));
            }
            ops.insert(name.to_string());
        }
        if ops.is_empty() {
            return Ok(Requirement::Off);
        }
        Ok(Requirement::Ops(ops))
    }

    pub fn word(&self) -> String {
        match self {
            Requirement::Off => "off".into(),
            Requirement::All => "all".into(),
            Requirement::Ops(ops) => ops.iter().cloned().collect::<Vec<_>>().join(","),
        }
    }

    pub fn is_off(&self) -> bool {
        *self == Requirement::Off
    }

    /// Does it cover `op`? `platform` says whether `op` is a platform op.
    pub fn covers(&self, op: &str, platform: bool) -> bool {
        match self {
            Requirement::Off => false,
            Requirement::All => !platform,
            Requirement::Ops(ops) => ops.contains(op),
        }
    }

    /// From the deployment's environment.
    pub fn from_vars(var: &dyn Fn(&str) -> Option<String>) -> Result<Requirement, String> {
        match var(REQUIRED_ENV) {
            None => Ok(Requirement::Off),
            Some(v) => Requirement::parse(&v).map_err(|e| format!("{REQUIRED_ENV}: {e}")),
        }
    }
}

/// A person's address, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub address: String,
    /// `code`, `sign-in`, or the name of the connection that vouched for it.
    pub source: String,
    pub verified_at: u64,
}

#[derive(Default)]
struct Memory {
    rows: BTreeMap<String, Stored>,
    /// The addresses of emailed-code sign-ins, by person. Tests only.
    sign_ins: BTreeMap<String, String>,
    app: AppPolicy,
    /// Tests only: make the address table unreadable.
    unreadable: bool,
}

/// The app developer's layer, as stored under `email_policy`. A field that
/// is `None` falls back to the deployment's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AppPolicy {
    required: Option<Requirement>,
    touch: Option<bool>,
}

impl AppPolicy {
    fn is_empty(&self) -> bool {
        self.required.is_none() && self.touch.is_none()
    }
}

enum Backing {
    Pg(Pool),
    Memory(Mutex<Memory>),
}

/// The addresses, the requirement, and the code ceremony that confirms an
/// address.
pub struct ContactEmails {
    deployment: Requirement,
    deployment_touch: bool,
    backing: Backing,
    /// The emailed-code provider that mails confirmation codes. `None` when
    /// the deployment sends no mail (`ORDO_MAIL_KIND` unset): nothing can be
    /// confirmed, and the routes say so.
    codes: Option<Arc<EmailCodeProvider>>,
    /// Where the message to an old address is queued.
    queue: Option<Arc<dyn MailQueue>>,
    /// The deployment's own space: the app developer's op answers there only.
    home: Option<String>,
    /// The platform ops of this build, which `all` never covers.
    platform: BTreeSet<String>,
    /// The app developer's layer, and when this node read it.
    app_cache: Mutex<Option<(u64, AppPolicy)>>,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
    stream: Option<Arc<ordo_secevent::Recorder>>,
}

impl ContactEmails {
    fn with_backing(deployment: Requirement, backing: Backing) -> ContactEmails {
        ContactEmails {
            deployment,
            deployment_touch: true,
            backing,
            codes: None,
            queue: None,
            home: None,
            platform: crate::platform_ops::specs()
                .into_iter()
                .map(|s| s.name)
                .collect(),
            app_cache: Mutex::new(None),
            now: Arc::new(crate::runtime::now_secs),
            stream: None,
        }
    }

    pub fn on_pool(deployment: Requirement, pool: Pool) -> ContactEmails {
        ContactEmails::with_backing(deployment, Backing::Pg(pool))
    }

    /// Held in memory: tests.
    pub fn in_memory(deployment: Requirement) -> ContactEmails {
        ContactEmails::with_backing(deployment, Backing::Memory(Mutex::new(Memory::default())))
    }

    /// Mail confirmation codes with `codes`, and tell old addresses through
    /// `queue`.
    pub fn with_mail(
        mut self,
        codes: Arc<EmailCodeProvider>,
        queue: Arc<dyn MailQueue>,
    ) -> ContactEmails {
        self.codes = Some(codes);
        self.queue = Some(queue);
        self
    }

    /// The deployment's `ORDO_EMAIL_CHANGE_TOUCH`.
    pub fn with_change_touch(mut self, on: bool) -> ContactEmails {
        self.deployment_touch = on;
        self
    }

    /// Make the address table unreadable or readable again. Tests only.
    pub fn set_unreadable(&self, on: bool) {
        if let Backing::Memory(m) = &self.backing {
            m.lock().expect("contact emails").unreadable = on;
        }
    }

    pub fn with_home(mut self, home: impl Into<String>) -> ContactEmails {
        self.home = Some(home.into());
        self
    }

    pub fn with_stream(mut self, stream: Arc<ordo_secevent::Recorder>) -> ContactEmails {
        self.stream = Some(stream);
        self
    }

    /// The provider that mails confirmation codes, if the deployment sends
    /// mail.
    pub fn codes(&self) -> Option<&Arc<EmailCodeProvider>> {
        self.codes.as_ref()
    }

    /// Note an emailed-code sign-in's address for `principal`, as the
    /// directory would hold it. Tests only.
    pub fn note_sign_in(&self, principal: &str, address: &str) {
        if let Backing::Memory(m) = &self.backing {
            m.lock()
                .expect("contact emails")
                .sign_ins
                .insert(principal.into(), address.into());
        }
    }

    /// The person's verified address: the one they added, else the one they
    /// sign in with by emailed code. `None` when Ordo knows none.
    pub async fn address_of(&self, principal: &str) -> Result<Option<Stored>, StoreFault> {
        match &self.backing {
            Backing::Memory(m) => {
                let m = m.lock().expect("contact emails");
                if m.unreadable {
                    return Err(StoreFault::unavailable(
                        "the account's email address could not be read",
                    ));
                }
                Ok(m.rows.get(principal).cloned().or_else(|| {
                    m.sign_ins.get(principal).map(|a| Stored {
                        address: a.clone(),
                        source: SOURCE_SIGN_IN.into(),
                        verified_at: 0,
                    })
                }))
            }
            Backing::Pg(pool) => {
                let rows = pool
                    .rows(&format!(
                        r#"SELECT "address", "source", "verified_at" FROM (
                             SELECT 0 AS "k", "address", "source", "verified_at"
                             FROM "$contactemail" WHERE "principal" = {p}
                             UNION ALL
                             SELECT 1, substr("authenticator", 7), 'sign-in', "linked_at"
                             FROM "$authenticator"
                             WHERE "principal" = {p} AND "authenticator" LIKE 'email:%'
                               AND "retired_case" = ''
                           ) s
                           ORDER BY "k", "verified_at" LIMIT 1"#,
                        p = sql_lit(principal)
                    ))
                    .await
                    .map_err(|e| {
                        crate::auth::store_failure::pool_fault(
                            e,
                            "the account's email address could not be read",
                        )
                    })?;
                Ok(rows.into_iter().next().and_then(|r| {
                    let mut it = r.into_iter();
                    let address = it.next().flatten()?;
                    let source = it.next().flatten().unwrap_or_default();
                    let verified_at = it
                        .next()
                        .flatten()
                        .and_then(|t| t.parse().ok())
                        .unwrap_or(0);
                    Some(Stored {
                        address,
                        source,
                        verified_at,
                    })
                }))
            }
        }
    }

    /// The address the person added, without the sign-in fallback.
    pub async fn added_address(&self, principal: &str) -> Result<Option<Stored>, StoreFault> {
        self.added(principal).await
    }

    async fn added(&self, principal: &str) -> Result<Option<Stored>, StoreFault> {
        match &self.backing {
            Backing::Memory(m) => Ok(m
                .lock()
                .expect("contact emails")
                .rows
                .get(principal)
                .cloned()),
            Backing::Pg(pool) => {
                let rows = pool
                    .rows(&format!(
                        r#"SELECT "address", "source", "verified_at" FROM "$contactemail"
                           WHERE "principal" = {}"#,
                        sql_lit(principal)
                    ))
                    .await
                    .map_err(|e| {
                        crate::auth::store_failure::pool_fault(
                            e,
                            "the account's email address could not be read",
                        )
                    })?;
                Ok(rows.into_iter().next().and_then(|r| {
                    let mut it = r.into_iter();
                    Some(Stored {
                        address: it.next().flatten()?,
                        source: it.next().flatten().unwrap_or_default(),
                        verified_at: it
                            .next()
                            .flatten()
                            .and_then(|t| t.parse().ok())
                            .unwrap_or(0),
                    })
                }))
            }
        }
    }

    async fn write(&self, principal: &str, s: &Stored) -> Result<(), StoreFault> {
        match &self.backing {
            Backing::Memory(m) => {
                m.lock()
                    .expect("contact emails")
                    .rows
                    .insert(principal.into(), s.clone());
                Ok(())
            }
            Backing::Pg(pool) => pool
                .rows(&format!(
                    r#"INSERT INTO "$contactemail" ("principal", "address", "source", "verified_at")
                       VALUES ({}, {}, {}, {})
                       ON CONFLICT ("principal") DO UPDATE SET "address" = EXCLUDED."address",
                         "source" = EXCLUDED."source", "verified_at" = EXCLUDED."verified_at""#,
                    sql_lit(principal),
                    sql_lit(&s.address),
                    sql_lit(&s.source),
                    s.verified_at
                ))
                .await
                .map(|_| ())
                .map_err(|e| {
                    crate::auth::store_failure::pool_fault(
                        e,
                        "the account's email address could not be written",
                    )
                }),
        }
    }

    /// Store a confirmed address for `principal`. Answers the address it
    /// replaced, if it was another one. The old address is told.
    pub async fn confirm(
        &self,
        principal: &str,
        address: &str,
        source: &str,
        at: u64,
    ) -> Result<Option<String>, StoreFault> {
        let old = self.added(principal).await?;
        self.write(
            principal,
            &Stored {
                address: address.to_string(),
                source: source.to_string(),
                verified_at: at,
            },
        )
        .await?;
        let replaced = old
            .map(|o| o.address)
            .filter(|o| !o.eq_ignore_ascii_case(address));
        let act = match &replaced {
            Some(old) => {
                self.tell_old(old, "changed", at).await;
                "contact-email-changed"
            }
            None => "contact-email-added",
        };
        self.record(act, principal, source, at);
        Ok(replaced)
    }

    /// Store an address an outside connection vouched for, when the person
    /// has none. Never replaces one. Answers whether it was stored.
    pub async fn adopt(
        &self,
        principal: &str,
        address: &str,
        connection: &str,
        at: u64,
    ) -> Result<bool, StoreFault> {
        let stored = match &self.backing {
            Backing::Memory(m) => {
                let mut m = m.lock().expect("contact emails");
                if m.rows.contains_key(principal) {
                    false
                } else {
                    m.rows.insert(
                        principal.into(),
                        Stored {
                            address: address.into(),
                            source: connection.into(),
                            verified_at: at,
                        },
                    );
                    true
                }
            }
            Backing::Pg(pool) => !pool
                .rows(&format!(
                    r#"INSERT INTO "$contactemail" ("principal", "address", "source", "verified_at")
                       VALUES ({}, {}, {}, {at})
                       ON CONFLICT ("principal") DO NOTHING
                       RETURNING "principal""#,
                    sql_lit(principal),
                    sql_lit(address),
                    sql_lit(connection),
                ))
                .await
                .map_err(|e| {
                    crate::auth::store_failure::pool_fault(
                        e,
                        "the account's email address could not be written",
                    )
                })?
                .is_empty(),
        };
        if stored {
            self.record("contact-email-added", principal, connection, at);
        }
        Ok(stored)
    }

    /// Put the person's added address back exactly as it was (`Some`), or
    /// take it off (`None`). Silent: no mail goes to either address and no
    /// event is recorded here; the recovery contest that calls it records the
    /// reset (owner, 2026-10-25).
    pub async fn restore(&self, principal: &str, to: Option<&Stored>) -> Result<(), StoreFault> {
        match to {
            Some(s) => self.write(principal, s).await,
            None => match &self.backing {
                Backing::Memory(m) => {
                    m.lock().expect("contact emails").rows.remove(principal);
                    Ok(())
                }
                Backing::Pg(pool) => pool
                    .rows(&format!(
                        r#"DELETE FROM "$contactemail" WHERE "principal" = {}"#,
                        sql_lit(principal)
                    ))
                    .await
                    .map(|_| ())
                    .map_err(|e| {
                        crate::auth::store_failure::pool_fault(
                            e,
                            "the account's email address could not be removed",
                        )
                    }),
            },
        }
    }

    /// Take the person's added address off. Answers whether there was one.
    /// The old address is told. An emailed-code sign-in's address stays: it
    /// is a way in, removed with `/auth/authenticators/unlink`.
    pub async fn remove(&self, principal: &str, at: u64) -> Result<bool, StoreFault> {
        let old = match &self.backing {
            Backing::Memory(m) => m
                .lock()
                .expect("contact emails")
                .rows
                .remove(principal)
                .map(|s| s.address),
            Backing::Pg(pool) => pool
                .rows(&format!(
                    r#"DELETE FROM "$contactemail" WHERE "principal" = {} RETURNING "address""#,
                    sql_lit(principal)
                ))
                .await
                .map_err(|e| {
                    crate::auth::store_failure::pool_fault(
                        e,
                        "the account's email address could not be removed",
                    )
                })?
                .into_iter()
                .next()
                .and_then(|r| r.into_iter().next().flatten()),
        };
        let Some(old) = old else {
            return Ok(false);
        };
        self.tell_old(&old, "removed", at).await;
        self.record("contact-email-removed", principal, "", at);
        Ok(true)
    }

    /// Queue the message that tells `address` it is no longer the account's.
    /// A queue that cannot take it costs the change nothing; the line says
    /// so without the address.
    async fn tell_old(&self, address: &str, what: &str, at: u64) {
        let Some(queue) = self.queue.clone() else {
            return;
        };
        let Ok(to) = MailAddress::parse(address) else {
            return;
        };
        let Ok(job) = MailJob::new(CHANGED_KIND, to, what) else {
            return;
        };
        let queued = tokio::task::spawn_blocking(move || queue.enqueue(&job, at)).await;
        if !matches!(queued, Ok(Ok(()))) {
            eprintln!("contact email: the message to the old address could not be queued");
        }
    }

    fn record(&self, act: &str, principal: &str, source: &str, at: u64) {
        if let Some(stream) = &self.stream {
            let detail = if source.is_empty() {
                format!("principal={principal}")
            } else {
                format!("principal={principal} source={source}")
            };
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act,
                    detail,
                    by: Some(principal),
                    tenant: None,
                    deployment: true,
                    at,
                },
            );
        }
    }

    // ── the requirement ────────────────────────────────────────────────

    async fn read_app(&self) -> Result<AppPolicy, ordo_identity::StoreFault> {
        match &self.backing {
            Backing::Memory(m) => Ok(m.lock().expect("contact emails").app.clone()),
            Backing::Pg(pool) => {
                let rows = pool
                    .rows(&format!(
                        r#"SELECT "value" FROM "$deploymentsetting" WHERE "key" = {}"#,
                        sql_lit(APP_KEY)
                    ))
                    .await
                    .map_err(|e| {
                        crate::auth::store_failure::pool_fault(
                            e,
                            "the app developer's email policy could not be read",
                        )
                    })?;
                let v = rows
                    .first()
                    .and_then(|r| r.first().cloned().flatten())
                    .and_then(|t| serde_json::from_str::<Value>(&t).ok());
                Ok(AppPolicy {
                    required: v
                        .as_ref()
                        .and_then(|v| v.get("required").and_then(Value::as_str))
                        .and_then(|w| Requirement::parse(w).ok()),
                    touch: v
                        .as_ref()
                        .and_then(|v| v.get("change_touch").and_then(Value::as_bool)),
                })
            }
        }
    }

    async fn write_app(
        &self,
        p: &AppPolicy,
        by: &str,
        at: u64,
    ) -> Result<(), ordo_identity::StoreFault> {
        match &self.backing {
            Backing::Memory(m) => {
                m.lock().expect("contact emails").app = p.clone();
            }
            Backing::Pg(pool) => {
                let sql = match p.is_empty() {
                    true => format!(
                        r#"DELETE FROM "$deploymentsetting" WHERE "key" = {}"#,
                        sql_lit(APP_KEY)
                    ),
                    false => format!(
                        r#"INSERT INTO "$deploymentsetting" ("key", "value", "updated_by", "updated_at")
                           VALUES ({}, {}, {}, {at})
                           ON CONFLICT ("key") DO UPDATE SET "value" = EXCLUDED."value",
                             "updated_by" = EXCLUDED."updated_by", "updated_at" = EXCLUDED."updated_at""#,
                        sql_lit(APP_KEY),
                        sql_lit(&{
                            let mut v = serde_json::Map::new();
                            if let Some(r) = &p.required {
                                v.insert("required".into(), json!(r.word()));
                            }
                            if let Some(t) = p.touch {
                                v.insert("change_touch".into(), json!(t));
                            }
                            Value::Object(v).to_string()
                        }),
                        sql_lit(by),
                    ),
                };
                pool.rows(&sql).await.map_err(|e| {
                    crate::auth::store_failure::pool_fault(
                        e,
                        "the app developer's email policy could not be written",
                    )
                })?;
            }
        }
        *self.app_cache.lock().expect("contact emails") = Some((at, p.clone()));
        Ok(())
    }

    /// The app developer's layer, read again every few seconds.
    async fn app(&self) -> Result<AppPolicy, ordo_identity::StoreFault> {
        let now = (self.now)();
        if let Some((at, r)) = self.app_cache.lock().expect("contact emails").clone() {
            if now < at + APP_CACHE_SECS && now >= at {
                return Ok(r);
            }
        }
        let r = self.read_app().await?;
        *self.app_cache.lock().expect("contact emails") = Some((now, r.clone()));
        Ok(r)
    }

    /// What is in force, and the setting that put it there.
    pub async fn requirement(&self) -> (Requirement, &'static str) {
        match self.app().await {
            Ok(AppPolicy {
                required: Some(r), ..
            }) => (r, SET_POLICY),
            _ => (self.deployment.clone(), REQUIRED_ENV),
        }
    }

    /// Does changing or removing the address need a fresh passkey touch, and
    /// the setting that decided it.
    pub async fn change_touch(&self) -> (bool, &'static str) {
        match self.app().await {
            Ok(AppPolicy { touch: Some(t), .. }) => (t, SET_POLICY),
            _ => (self.deployment_touch, TOUCH_ENV),
        }
    }

    /// Why `principal` may not call `op` yet, or `None`. A table that cannot
    /// be read **refuses** the call, retryable: letting it through would drop
    /// a requirement the app developer set (owner, 2026-10-22).
    pub async fn gate(&self, principal: &str, op: &str) -> Option<ordo_api::EmailGate> {
        let (r, setting) = self.requirement().await;
        if !r.covers(op, self.platform.contains(op)) {
            return None;
        }
        match self.address_of(principal).await {
            Ok(Some(_)) => None,
            Ok(None) => Some(ordo_api::EmailGate::missing(format!(
                "{op} needs a verified email address on your account, and it has none. Add one \
                 with POST /auth/email/begin and confirm the emailed code. The app developer \
                 requires it with {setting}"
            ))),
            Err(e) => {
                eprintln!("contact email: {e}; {op} was refused, to be tried again");
                Some(ordo_api::EmailGate::unreadable(format!(
                    "{op} needs a verified email address, and the table that holds addresses \
                     could not be read just now. Try again shortly. The app developer \
                     requires it with {setting}"
                )))
            }
        }
    }

    /// The refusal's message, whether the person has no address or the table
    /// could not be read.
    pub async fn refusal(&self, principal: &str, op: &str) -> Option<String> {
        self.gate(principal, op).await.map(|g| g.message)
    }

    // ── the app developer's op ─────────────────────────────────────────

    /// Run [`SET_POLICY`]. `required` is `off`, `all`, a comma list of ops,
    /// or `default` to fall back to the deployment's `ORDO_EMAIL_REQUIRED`.
    /// Without it, the op reads what is in force.
    pub async fn execute(
        &self,
        op: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        if op != SET_POLICY {
            return Err(ExecError::internal(format!(
                "`{op}` is not an email policy op"
            )));
        }
        if self.home.as_deref().is_some_and(|h| h != space) {
            return Err(ExecError::new(
                ErrorCode::NotFound,
                "the app developer's email policy is the deployment's setting and this op \
                 answers only in the deployment's own space",
            ));
        }
        let given = bound
            .0
            .get("required")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let touch_given = bound
            .0
            .get("changeTouch")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if given.is_some() || touch_given.is_some() {
            let now = (self.now)();
            let mut policy = self.app().await.map_err(ExecError::from)?;
            if let Some(word) = given {
                policy.required = if word == "default" {
                    None
                } else {
                    Some(Requirement::parse(word).map_err(|e| {
                        ExecError::new(ErrorCode::TenantRefused, format!("required: {e}"))
                    })?)
                };
            }
            if let Some(word) = touch_given {
                policy.touch = match word {
                    "default" => None,
                    "on" => Some(true),
                    "off" => Some(false),
                    other => {
                        return Err(ExecError::new(
                            ErrorCode::TenantRefused,
                            format!("changeTouch: `{other}` is not on, off or default"),
                        ))
                    }
                };
            }
            let next = policy.required.clone();
            if given.is_some() && next.as_ref().is_some_and(|r| !r.is_off()) && self.codes.is_none()
            {
                return Err(ExecError::new(
                    ErrorCode::TenantRefused,
                    format!(
                        "required: nobody could confirm an address, because this deployment \
                         sends no mail. The deployment owner sets {} (smtp, resend or sendgrid) \
                         first",
                        crate::mail::KIND_ENV
                    ),
                ));
            }
            self.write_app(&policy, caller, now)
                .await
                .map_err(ExecError::from)?;
            if let Some(stream) = &self.stream {
                crate::enrolment::record(
                    stream,
                    &crate::enrolment::Act {
                        act: "email-policy-set",
                        detail: format!(
                            "required={} change_touch={}",
                            policy
                                .required
                                .as_ref()
                                .map_or("default".into(), |r| r.word()),
                            policy
                                .touch
                                .map_or("default", |t| if t { "on" } else { "off" })
                        ),
                        by: Some(caller),
                        tenant: Some(space),
                        deployment: true,
                        at: now,
                    },
                );
            }
        }
        let app = self.app().await.map_err(ExecError::from)?;
        let (r, setting) = self.requirement().await;
        let (touch, touch_setting) = self.change_touch().await;
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "required": r.word(),
                "source": setting,
                "deployment": self.deployment.word(),
                "app": app.required.map(|a| a.word()),
                "changeTouch": if touch { "on" } else { "off" },
                "changeTouchSource": touch_setting,
            })],
            next_cursor: None,
        })
    }
}

/// Writes the message that tells an old address it is no longer the
/// account's. The payload is `changed` or `removed`. Nothing else from the
/// account is in it.
pub struct ChangedNotice;

impl Composer for ChangedNotice {
    fn compose(&self, job: &QueuedMail, _now: u64) -> Result<Option<Message>, MailError> {
        let what = if job.payload.trim() == "removed" {
            "was removed from"
        } else {
            "was replaced on"
        };
        let text = format!(
            "This address {what} an account. Mail from the app will no longer come here.\n\
             \n\
             If you did not make this change, sign in and add this address again, and tell\n\
             the app's support.\n"
        );
        Message::new(job.to.clone(), CHANGED_SUBJECT, &text).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_requirement_reads_off_all_and_a_list() {
        assert_eq!(Requirement::parse("off"), Ok(Requirement::Off));
        assert_eq!(Requirement::parse(""), Ok(Requirement::Off));
        assert_eq!(Requirement::parse("all"), Ok(Requirement::All));
        let r = Requirement::parse("setReminder, std.notes.create").expect("a list");
        assert!(r.covers("setReminder", false));
        assert!(!r.covers("createNote", false));
        assert_eq!(r.word(), "setReminder,std.notes.create");
        assert!(Requirement::parse("a b").is_err());
    }

    #[test]
    fn all_never_covers_a_platform_op() {
        assert!(Requirement::All.covers("createNote", false));
        assert!(!Requirement::All.covers("std.space.members", true));
        assert!(!Requirement::Off.covers("createNote", false));
    }

    #[test]
    fn a_bad_setting_names_itself() {
        let e = Requirement::from_vars(&|k| (k == REQUIRED_ENV).then(|| "a b".to_string()))
            .expect_err("refused");
        assert!(e.starts_with(REQUIRED_ENV), "{e}");
    }

    #[test]
    fn the_changed_notice_carries_nothing_but_the_fact() {
        let job = QueuedMail {
            id: 1,
            kind: CHANGED_KIND.into(),
            to: MailAddress::parse("old@example.org").expect("address"),
            payload: "changed".into(),
            attempt: 1,
            queued_at: 0,
        };
        let msg = ChangedNotice
            .compose(&job, 0)
            .expect("composed")
            .expect("a message");
        assert_eq!(msg.subject(), CHANGED_SUBJECT);
        assert!(msg.text().contains("was replaced on"));
    }
}
