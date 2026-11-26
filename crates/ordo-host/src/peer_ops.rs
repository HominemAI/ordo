//! What one Ordo deployment tells another about the ops it calls
//! (`plan-federation-grants.md` 4, step G4).
//!
//! The consumer's file lists the holder's ops it calls:
//!
//! ```text
//! peer notes {
//!   std.notes.list
//!   addComment
//! }
//! ```
//!
//! # At the consumer's apply ([`check_uses`])
//!
//! 1. For each `peer` block, read the holder's grant for this deployment
//!    and the shape of each op it grants: [`GRANT`], as itself, through
//!    [`crate::peer_client`].
//! 2. Refuse the apply, naming the peer and the op, for each listed op the
//!    holder does not grant.
//! 3. When nothing is refused, record the use with each holder:
//!    [`RECORD_USE`]. Recorded before the version is stored, so a holder
//!    always knows; an apply that then fails leaves a use recorded for a
//!    version that did not land, which only makes the holder warn more.
//!
//! # At the holder
//!
//! - [`GRANT`] answers the calling consumer its expanded grants for the
//!   serving version, and each granted op's `op`, `kind` and `params`.
//! - [`RECORD_USE`] keeps what each consumer calls, one row per consumer in
//!   [`USES_TABLE`], with the Ordo release the consumer runs, and answers
//!   the holder's own. Each side keeps the other's release
//!   ([`HOLDERS_TABLE`] on the consumer), and the release check reads it
//!   there: the release a neighbour runs comes through its door.
//! - At the holder's plan and apply, [`breaks`] names each consumer a
//!   change would break ("note_viewer calls archiveEntry"). A warning: the
//!   holder's own apply goes on (owner question, G4 report).
//!
//! Both holder ops answer only another deployment calling as itself, in
//! the holder's own space, and only one the holder grants something
//! ([`crate::peer_door`]).
//!
//! [`REGISTER_KEY`] is the consumer's: it registers this deployment's
//! consumer key at the identity service with a token for its own admin.

use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

use ordo_api::{
    BoundParams, ErrorCode, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec,
    ParamSpec, ParamTy, RefineTier, Syncability,
};
use ordo_exec::{sql_lit, Pool, PoolError};
use serde_json::{json, Value};

use crate::consumer_grants::{client_env, Expanded};
use crate::peer_client::{PeerClient, PeerClientError};

/// The holder's grant for the calling consumer, and each granted op's
/// shape. A read.
pub const GRANT: &str = "std.peer.grant";
/// The holder records which of its ops the calling consumer's version
/// calls. A write.
pub const RECORD_USE: &str = "std.peer.recordUse";
/// Register this deployment's consumer key at the identity service, with a
/// token for this deployment's own admin. A write, the deployment owner's.
pub const REGISTER_KEY: &str = "std.peer.registerKey";

/// Every op of this module.
pub const OPS: [&str; 3] = [GRANT, RECORD_USE, REGISTER_KEY];
/// The ops another deployment calls, as itself.
pub const CONSUMER_OPS: [&str; 2] = [GRANT, RECORD_USE];

/// What each consumer calls: one row per consumer.
pub const USES_TABLE: &str = "$peeruses";
/// The most ops one record holds.
pub const MAX_OPS: usize = 1000;

const DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$peeruses" (
  "consumer"    text   PRIMARY KEY,
  "version"     text   NOT NULL,
  "ops"         text   NOT NULL,
  "recorded_at" bigint NOT NULL
)"#;

/// The consumer's Ordo release, added after the table.
const RELEASE_COLUMN: &str =
    r#"ALTER TABLE "$peeruses" ADD COLUMN IF NOT EXISTS "release" text"#;

/// On the consumer: what each holder answered at the last apply that
/// recorded a use there, one row per `peer` block.
pub const HOLDERS_TABLE: &str = "$peerholders";

const HOLDERS_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$peerholders" (
  "peer"        text   PRIMARY KEY,
  "handle"      text   NOT NULL,
  "version"     text   NOT NULL,
  "release"     text   NOT NULL,
  "recorded_at" bigint NOT NULL
)"#;

/// The apply's refusal class for a `peer` block the holder does not back.
pub const NOT_GRANTED: &str = "peer_op_not_granted";
/// The apply's refusal class for a holder that could not be asked.
pub const UNREACHABLE: &str = "peer_unreachable";

/// The specs of the ops.
pub fn specs() -> Vec<OpSpec> {
    let spec = |name: &str, kind: OpKind, params: Vec<ParamSpec>, fields: &[&str]| OpSpec {
        name: name.to_string(),
        version_hash: crate::platform_ops::PLATFORM_VERSION.to_string(),
        kind,
        params,
        output_fields: fields.iter().map(|f| FieldSpec::new(*f)).collect(),
        live_capable: false,
        reads_external: false,
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
    let text = |name: &str, max: u32| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required: true,
        bounds: None,
    };
    vec![
        spec(
            GRANT,
            OpKind::Read,
            vec![],
            &["consumer", "version", "release", "grants", "ops"],
        ),
        spec(
            RECORD_USE,
            OpKind::Write,
            // `ops` is a JSON array of op names; `release` the consumer's
            // Ordo release.
            vec![
                text("version", 128),
                text("ops", 128 * 1024),
                ParamSpec {
                    required: false,
                    ..text("release", 64)
                },
            ],
            &["consumer", "version", "ops", "release", "holderVersion"],
        ),
        spec(
            REGISTER_KEY,
            OpKind::Write,
            // A token for an admin of this deployment at the identity
            // service. Held for the one request and never stored.
            vec![text("token", 16 * 1024)],
            &["handle", "consumerKey"],
        ),
    ]
}

// ── what each consumer calls ──────────────────────────────────────────────

/// One consumer's recorded use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedUse {
    /// The consumer's handle at the identity service.
    pub consumer: String,
    /// Its version that calls these ops.
    pub version: String,
    pub ops: Vec<String>,
    /// The Ordo release it ran when it recorded them. `None` for a row from
    /// a release before it was sent.
    pub release: Option<String>,
    pub recorded_at: u64,
}

/// Keep `ops` as what `consumer` calls, replacing what it recorded before.
pub async fn record(pool: &Pool, used: &RecordedUse) -> Result<(), PoolError> {
    pool.rows(DDL).await?;
    pool.rows(RELEASE_COLUMN).await?;
    let text = serde_json::to_string(&used.ops).unwrap_or_else(|_| "[]".into());
    pool.rows(&format!(
        r#"INSERT INTO "$peeruses" ("consumer", "version", "ops", "release", "recorded_at")
           VALUES ({c}, {v}, {o}, {r}, {at})
           ON CONFLICT ("consumer") DO UPDATE SET "version" = EXCLUDED."version",
             "ops" = EXCLUDED."ops", "release" = EXCLUDED."release",
             "recorded_at" = EXCLUDED."recorded_at""#,
        c = sql_lit(&used.consumer),
        v = sql_lit(&used.version),
        o = sql_lit(&text),
        r = used.release.as_deref().map(sql_lit).unwrap_or_else(|| "NULL".into()),
        at = used.recorded_at,
    ))
    .await
    .map(|_| ())
}

/// Every consumer's recorded use, by consumer. None before the first.
pub async fn recorded(pool: &Pool) -> Result<Vec<RecordedUse>, PoolError> {
    let read = |release: &str| {
        format!(
            r#"SELECT "consumer", "version", "ops", "recorded_at"::text, {release}
               FROM "$peeruses" ORDER BY "consumer""#
        )
    };
    let rows = match pool.rows(&read(r#""release""#)).await {
        Ok(rows) => rows,
        Err(e) if e.is_missing_table() => return Ok(Vec::new()),
        // A table from before the release column.
        Err(e) if e.is_missing_column() => pool.rows(&read("NULL::text")).await?,
        Err(e) => return Err(e),
    };
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let mut it = r.into_iter();
            let consumer = it.next()??;
            let version = it.next()??;
            let ops: Vec<String> = serde_json::from_str(&it.next()??).ok()?;
            let recorded_at = it.next()??.parse().ok()?;
            let release = it.next().flatten();
            Some(RecordedUse {
                consumer,
                version,
                ops,
                release,
                recorded_at,
            })
        })
        .collect())
}

/// What one holder answered at the consumer's last apply that recorded a
/// use there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HolderSeen {
    /// The `peer` block's nickname.
    pub peer: String,
    /// The holder's handle at the identity service.
    pub handle: String,
    /// The version the holder served.
    pub version: String,
    /// The Ordo release the holder ran.
    pub release: String,
    pub recorded_at: u64,
}

/// Keep what `seen` says, on the consumer, replacing the peer's last row.
pub async fn record_holder(pool: &Pool, seen: &HolderSeen) -> Result<(), PoolError> {
    pool.rows(HOLDERS_DDL).await?;
    pool.rows(&format!(
        r#"INSERT INTO "$peerholders" ("peer", "handle", "version", "release", "recorded_at")
           VALUES ({p}, {h}, {v}, {r}, {at})
           ON CONFLICT ("peer") DO UPDATE SET "handle" = EXCLUDED."handle",
             "version" = EXCLUDED."version", "release" = EXCLUDED."release",
             "recorded_at" = EXCLUDED."recorded_at""#,
        p = sql_lit(&seen.peer),
        h = sql_lit(&seen.handle),
        v = sql_lit(&seen.version),
        r = sql_lit(&seen.release),
        at = seen.recorded_at,
    ))
    .await
    .map(|_| ())
}

/// Every holder this consumer recorded, by peer. None before the first.
pub async fn holders(pool: &Pool) -> Result<Vec<HolderSeen>, PoolError> {
    let rows = match pool
        .rows(
            r#"SELECT "peer", "handle", "version", "release", "recorded_at"::text
               FROM "$peerholders" ORDER BY "peer""#,
        )
        .await
    {
        Ok(rows) => rows,
        Err(e) if e.is_missing_table() => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let mut it = r.into_iter();
            Some(HolderSeen {
                peer: it.next()??,
                handle: it.next()??,
                version: it.next()??,
                release: it.next()??,
                recorded_at: it.next()??.parse().ok()?,
            })
        })
        .collect())
}

/// Does `g` cover `consumer`: a grant tied to it by
/// `ORDO_CONSUMER_<NAME>_CLIENT`, or `grant provider` (on behalf of a
/// person).
fn covers(g: &Expanded, consumer: &str, var: &dyn Fn(&str) -> Option<String>) -> bool {
    g.provider || var(&client_env(&g.name)).is_some_and(|id| id.trim() == consumer)
}

/// **The consumers a holder's change would break**: for each recorded use,
/// each op the new version's grants no longer open to that consumer, named
/// by the grant that ties it ("note_viewer calls archiveEntry"). `new` and
/// `serving` are the expanded grants of the version being applied and the
/// one serving; `ops` are the new version's own ops. Warnings: the apply
/// goes on.
pub fn breaks(
    new: &[Expanded],
    serving: &[Expanded],
    ops: &[String],
    uses: &[RecordedUse],
    var: &dyn Fn(&str) -> Option<String>,
) -> Vec<String> {
    let mut out = Vec::new();
    for u in uses {
        let tied: Vec<&Expanded> = new.iter().filter(|g| covers(g, &u.consumer, var)).collect();
        let open: BTreeSet<&str> = tied
            .iter()
            .flat_map(|g| g.ops.iter().map(String::as_str))
            .collect();
        // The name the holder knows the consumer by: its grant here, or the
        // one it had in the serving version, or its handle.
        let named = |list: &[Expanded]| {
            list.iter()
                .find(|g| !g.provider && covers(g, &u.consumer, var))
                .map(|g| g.name.clone())
        };
        let name = named(new)
            .or_else(|| named(serving))
            .unwrap_or_else(|| u.consumer.clone());
        for op in &u.ops {
            if open.contains(op.as_str()) {
                continue;
            }
            let why = if ops.iter().any(|o| o == op) {
                "and this version does not grant it to that deployment"
            } else {
                "and this version has no such op"
            };
            let fix = match tied.iter().find(|g| !g.provider) {
                Some(g) => format!("keep `{op}` in `{}`", g.header()),
                None => format!(
                    "tie a grant to it with {}={}",
                    client_env(&name),
                    u.consumer
                ),
            };
            out.push(format!(
                "warning     breaks {name}: {name} calls {op} (consumer `{}`, its version {}), \
                 {why}, so its calls will be refused. This apply goes on. Tell that \
                 deployment's owner first, or {fix}",
                u.consumer, u.version
            ));
        }
    }
    out
}

// ── at the consumer's apply ───────────────────────────────────────────────

/// What [`check_uses`] found.
#[derive(Debug, Clone, Default)]
pub struct UsesCheck {
    /// One line per refusal, leading with its class and ending with the
    /// fix.
    pub refusals: Vec<String>,
    /// What the plan shows: each holder's ops and how each is granted.
    pub notes: Vec<String>,
    /// What each holder answered when the use was recorded there; the
    /// apply keeps it ([`record_holder`]).
    pub holders: Vec<HolderSeen>,
}

/// Check each `peer` block of a program against its holder, and record the
/// use with each holder when nothing is refused and `record` is set. With
/// no client, every block is refused, naming the settings.
pub async fn check_uses(
    client: Option<&PeerClient>,
    uses: &[ordo_schema::PeerUses],
    version: &str,
    record: bool,
) -> UsesCheck {
    let mut out = UsesCheck::default();
    if uses.is_empty() {
        return out;
    }
    let Some(client) = client else {
        for u in uses {
            out.refusals.push(format!(
                "{UNREACHABLE} in `peer {}`: this deployment has no client for another \
                 deployment's door. Fix: set ORDO_IDENTITY_URL and ORDO_IDENTITY_ORIGIN, so it \
                 can ask the identity service for a token, and restart",
                u.name
            ));
        }
        return out;
    };
    for u in uses {
        let grant = match client.grant(&u.name).await {
            Ok(g) => g,
            Err(e) => {
                out.refusals.push(format!(
                    "{UNREACHABLE} in `peer {}`: its grant could not be read. {e}",
                    u.name
                ));
                continue;
            }
        };
        let granted = grant.granted();
        let me = client.handle().unwrap_or("this deployment").to_string();
        let mut shown = Vec::new();
        for op in &u.ops {
            if !granted.contains(op.as_str()) {
                out.refusals.push(format!(
                    "{NOT_GRANTED} in `peer {}`: `{op}` is not granted to this deployment \
                     (`{me}`) by that deployment's file. Fix: its owner adds `{op}` to the \
                     grant tied to `{me}`, or remove the line `{op}` here",
                    u.name
                ));
                continue;
            }
            let how: Vec<String> = grant
                .grants_of(op)
                .iter()
                .map(|g| {
                    if g.as_itself {
                        format!("{} (as itself or on behalf)", g.header())
                    } else {
                        format!("{} (on behalf of a person)", g.header())
                    }
                })
                .collect();
            let kind = grant
                .shape(op)
                .and_then(|s| s["kind"].as_str())
                .unwrap_or("op");
            shown.push(format!("{op} ({kind}, {})", how.join(", ")));
        }
        if !shown.is_empty() {
            out.notes.push(format!(
                "peer        {}: {} op(s) granted: {}",
                u.name,
                shown.len(),
                shown.join("; ")
            ));
        }
    }
    if record && out.refusals.is_empty() {
        for u in uses {
            match client.record_use(&u.name, version, &u.ops).await {
                Ok(seen) => {
                    out.notes.push(format!(
                        "peer        {}: told it this version calls {}; it runs Ordo {}",
                        u.name,
                        u.ops.join(", "),
                        seen.release
                    ));
                    out.holders.push(seen);
                }
                Err(e) => out.refusals.push(format!(
                    "{UNREACHABLE} in `peer {}`: the use could not be recorded there, so it \
                     could not warn before a change breaks this deployment. {e}",
                    u.name
                )),
            }
        }
    }
    out
}

// ── the ops ───────────────────────────────────────────────────────────────

/// The state the ops run on. The runtime gives it the door and the client
/// once each is built.
pub struct PeerOps {
    /// The control database: where `$peeruses` and the versions are.
    pub pool: Pool,
    /// The deployment's own space.
    pub home: String,
    pub door: OnceLock<Arc<crate::peer_door::PeerDoor>>,
    pub client: OnceLock<Arc<PeerClient>>,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl PeerOps {
    pub fn new(pool: Pool, home: &str, now: Arc<dyn Fn() -> u64 + Send + Sync>) -> Arc<PeerOps> {
        Arc::new(PeerOps {
            pool,
            home: home.to_string(),
            door: OnceLock::new(),
            client: OnceLock::new(),
            now,
        })
    }

    /// Run one of [`OPS`].
    pub async fn execute(
        &self,
        name: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        if space != self.home {
            return Err(ExecError::new(
                ErrorCode::NotFound,
                format!("`{name}` answers only in this deployment's own space"),
            ));
        }
        let row = match name {
            REGISTER_KEY => self.register_key(bound).await?,
            _ => {
                let consumer = caller.strip_prefix("consumer:").ok_or_else(|| {
                    ExecError::new(
                        ErrorCode::NotFound,
                        format!(
                            "`{name}` answers another Ordo deployment calling as itself, with a \
                             token from the identity service"
                        ),
                    )
                })?;
                if name == GRANT {
                    self.grant(consumer).await?
                } else {
                    self.record_use(consumer, bound).await?
                }
            }
        };
        Ok(ExecResult {
            position: None,
            rows: vec![row],
            next_cursor: None,
        })
    }

    async fn grant(&self, consumer: &str) -> Result<Value, ExecError> {
        let door = self
            .door
            .get()
            .ok_or_else(|| ExecError::internal("the door for other deployments is not wired"))?;
        let (version, grants) = door
            .grants_for(consumer)
            .await
            .map_err(|e| ExecError::new(ErrorCode::ServerBusy, e))?;
        let open: BTreeSet<&str> = grants
            .iter()
            .flat_map(|g| g.ops.iter().map(String::as_str))
            .collect();
        // Each granted op's shape, from the serving version's document.
        let doc: Value = crate::schema_apply::deployed_artifact(&self.pool)
            .await
            .and_then(|a| serde_json::from_str(&a.artifact).ok())
            .unwrap_or(Value::Null);
        let ops: Vec<Value> = doc["ops"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|o| o["op"].as_str().is_some_and(|n| open.contains(n)))
            .map(|o| json!({ "op": o["op"], "kind": o["kind"], "params": o["params"] }))
            .collect();
        Ok(json!({
            "consumer": consumer,
            "version": version,
            "release": crate::release_check::release(),
            "grants": grants.iter().map(|g| json!({
                "name": g.name,
                "provider": g.provider,
                "asItself": g.as_itself,
                "ops": g.ops,
            })).collect::<Vec<_>>(),
            "ops": ops,
        }))
    }

    async fn record_use(&self, consumer: &str, bound: &BoundParams) -> Result<Value, ExecError> {
        let text = |k: &str| {
            bound
                .0
                .get(k)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        let bad = |why: &str| ExecError::new(ErrorCode::InvalidParam, why.to_string());
        let version = text("version");
        if version.is_empty() {
            return Err(bad(
                "`version` names the consumer's version that calls the ops",
            ));
        }
        let ops: Vec<String> = serde_json::from_str(&text("ops"))
            .map_err(|_| bad("`ops` is a JSON array of op names"))?;
        if ops.len() > MAX_OPS
            || ops
                .iter()
                .any(|o| o.is_empty() || o.len() > 128 || o.contains(char::is_whitespace))
        {
            return Err(bad(
                "`ops` holds at most 1000 op names, each at most 128 characters with no space",
            ));
        }
        let release = Some(text("release")).filter(|r| !r.is_empty());
        let used = RecordedUse {
            consumer: consumer.to_string(),
            version: version.clone(),
            ops: ops.clone(),
            release: release.clone(),
            recorded_at: (self.now)(),
        };
        record(&self.pool, &used)
            .await
            .map_err(|e| ExecError::new(ErrorCode::ServerBusy, e.to_string()))?;
        let serving = crate::schema_apply::deployed_artifact(&self.pool)
            .await
            .map(|a| a.version)
            .unwrap_or_default();
        Ok(json!({
            "consumer": consumer, "version": version, "ops": ops,
            "release": crate::release_check::release(), "holderVersion": serving,
        }))
    }

    async fn register_key(&self, bound: &BoundParams) -> Result<Value, ExecError> {
        let client = self.client.get().ok_or_else(|| {
            PeerClientError::Setting {
                setting: crate::identity_keys::URL_ENV.into(),
                why: "is unset, or the consumer key could not be derived, so this deployment \
                      has no client for another deployment's door"
                    .into(),
            }
            .to_exec_error()
        })?;
        let token = bound
            .0
            .get("token")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        let key = client
            .register_key(token)
            .await
            .map_err(|e| e.to_exec_error())?;
        Ok(json!({ "handle": client.handle(), "consumerKey": key }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(name: &str, provider: bool, ops: &[&str]) -> Expanded {
        Expanded {
            name: name.into(),
            provider,
            as_itself: false,
            ops: ops.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn a_removal_names_the_consumer_it_breaks() {
        let var = |k: &str| (k == client_env("note_viewer")).then(|| "viewer".to_string());
        let uses = [RecordedUse {
            consumer: "viewer".into(),
            version: "1.0.0".into(),
            ops: vec!["std.notes.list".into(), "archiveNote".into()],
            release: None,
            recorded_at: 0,
        }];
        let serving = [grant(
            "note_viewer",
            false,
            &["std.notes.list", "archiveNote"],
        )];
        let ops = ["std.notes.list".to_string(), "archiveNote".to_string()];
        assert!(breaks(&serving, &serving, &ops, &uses, &var).is_empty());

        let new = [grant("note_viewer", false, &["std.notes.list"])];
        let w = breaks(&new, &serving, &ops, &uses, &var);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("note_viewer calls archiveNote"), "{}", w[0]);
        assert!(
            w[0].contains("keep `archiveNote` in `grant note_viewer`"),
            "{}",
            w[0]
        );

        // The op itself gone, and the grant gone: still named by its old
        // grant, with the setting that would tie a new one.
        let w = breaks(&[], &serving, &ops[..1], &uses, &var);
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w[1].contains("this version has no such op"), "{}", w[1]);
        let tie = format!("{}=viewer", client_env("note_viewer"));
        assert!(w[1].contains(&tie), "{}", w[1]);

        // A provider grant covers a consumer with none of its own.
        let provider = [grant("ids", true, &["std.notes.list", "archiveNote"])];
        assert!(breaks(&provider, &[], &ops, &uses, &|_| None).is_empty());
    }
}
