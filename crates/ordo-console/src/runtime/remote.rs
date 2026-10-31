//! The console's side of the supervisor's socket (`plan-identity-service.md`
//! 3.2).
//!
//! Every runtime action is one call on the socket, through
//! [`SupervisorClient`]; every failure is a [`ConsoleError`] that keeps the
//! supervisor's own typed error. The console files a revision row before
//! it asks, and records how the supervisor answered.
//!
//! **Numbering.** The supervisor numbers nothing: the console hands it each
//! revision's number. The console's history always holds a row for the
//! supervisor's live revision ([`Runtime::sync`] files a copy when it has
//! none, such as revision 1, which the supervisor built from the
//! container's variables), so the next number the console files is above
//! it.
//!
//! **The upgrade** (owner, 2026-10-27: an upgrade affects nothing). Before
//! the one image the console ran the deployment itself and kept the live
//! revision in its own database. The new supervisor starts on an empty
//! volume and builds revision 1 from the container's variables. So at its
//! first sync the console looks: when the supervisor runs revision 1 built
//! from the container, and the console's own live revision is a later one
//! that came up (`good`), the console applies that revision through the
//! socket, under its own number, audited. A secret the console holds a
//! value for is sent as that value; one it has none for is kept as the
//! supervisor holds it.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ordo_supervisor::spec::{ApplyRequest, SecretInput as WireSecret};
use ordo_supervisor::{ChildName, Kind, SupervisorClient, SupervisorError};
use serde_json::{json, Value};

use super::spec::{self, Diff, Patch, SecretRef, Spec};
use super::{Applied, Status};
use crate::codes::Clock;
use crate::error::ConsoleError;
use crate::sealing::ConsoleKey;
use crate::store::Store;

/// The audit action of the upgrade's carry-over.
pub const CARRY_OVER_ACTION: &str = "runtime.carry-over";
/// How many of the deployment's log lines the Runtime screen shows.
const LOG_LINES: usize = 200;

/// What a sync did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Synced {
    /// The supervisor's live revision is in the console's history.
    InStep { live: u64 },
    /// The supervisor ran revision 1 from the container; the console's own
    /// live revision was applied over it.
    CarriedOver { revision: u64 },
    /// The carry-over was refused or did not come up; the supervisor still
    /// runs revision 1. Said in the audit and the log.
    CarryOverFailed { revision: u64, why: String },
}

/// The handle the server holds in supervise mode. Cheap to clone.
#[derive(Clone)]
pub struct Runtime(Arc<Core>);

struct Core {
    dep: String,
    store: Store,
    clock: Clock,
    key: ConsoleKey,
    client: SupervisorClient,
    console_port: u16,
    apply_lock: tokio::sync::Mutex<()>,
    synced: AtomicBool,
}

impl From<SupervisorError> for ConsoleError {
    fn from(e: SupervisorError) -> ConsoleError {
        match e {
            SupervisorError::SettingsRefused { revision, output } => {
                ConsoleError::SettingsRefused { revision, output }
            }
            SupervisorError::RolledBack { revision, to, why } => {
                ConsoleError::RolledBack { revision, to, why }
            }
            SupervisorError::BadRequest(why) => ConsoleError::BadRequest(why),
            other => ConsoleError::Supervisor(other),
        }
    }
}

impl Runtime {
    /// The handle; nothing is called yet. [`Runtime::sync`] is the first
    /// call, at boot.
    pub fn new(
        dep: &str,
        store: Store,
        clock: Clock,
        key: ConsoleKey,
        client: SupervisorClient,
        console_port: u16,
    ) -> Runtime {
        Runtime(Arc::new(Core {
            dep: dep.to_string(),
            store,
            clock,
            key,
            client,
            console_port,
            apply_lock: tokio::sync::Mutex::new(()),
            synced: AtomicBool::new(false),
        }))
    }

    pub fn client(&self) -> &SupervisorClient {
        &self.0.client
    }

    /// Whether a sync has completed in this boot.
    pub fn synced(&self) -> bool {
        self.0.synced.load(Ordering::SeqCst)
    }

    /// Bring the console's history in step with the supervisor, and carry
    /// the console's own live revision over at the upgrade (module docs).
    /// Safe to run again: a second run finds the rows in step.
    pub async fn sync(&self) -> Result<Synced, ConsoleError> {
        let core = &self.0;
        let _one = core.apply_lock.lock().await;
        let current = core.client.current().await?;
        let (live, cur) = spec_of(&current)?;
        let from_container = current["origin"] == "container";
        let now = core.clock.now();
        if from_container && live == 1 {
            if let Some(n) = core
                .store
                .live_revision(&core.dep)
                .await?
                .filter(|n| *n > 1)
            {
                if let Some(row) = core.store.revision(&core.dep, n).await? {
                    if row.outcome.as_deref() == Some("good") {
                        let spec = Spec::from_row(&row)?;
                        let reason = format!(
                            "carried over at the upgrade to the one image: the console's live \
                             revision {n}"
                        );
                        let req = self.request(n, "system", &reason, &spec, &cur).await?;
                        let out = match core.client.apply(&req).await {
                            Ok(_) => Synced::CarriedOver { revision: n },
                            Err(e) => Synced::CarryOverFailed {
                                revision: n,
                                why: e.to_string(),
                            },
                        };
                        let (outcome, detail) = match &out {
                            Synced::CarriedOver { .. } => ("ok", format!("revision={n}")),
                            Synced::CarryOverFailed { why, .. } => {
                                ("failed", format!("revision={n} {why}"))
                            }
                            Synced::InStep { .. } => unreachable!(),
                        };
                        eprintln!("ordo-console: {CARRY_OVER_ACTION}: {outcome} {detail}");
                        core.store
                            .audit(
                                &core.dep,
                                "system",
                                CARRY_OVER_ACTION,
                                outcome,
                                &detail,
                                now,
                            )
                            .await?;
                        if matches!(out, Synced::CarriedOver { .. }) {
                            core.store.set_live_revision(&core.dep, n).await?;
                            core.synced.store(true, Ordering::SeqCst);
                            return Ok(out);
                        }
                        // The supervisor runs revision 1; file it beside
                        // the history so the screen says so.
                        self.mirror(live, &current, &cur, now).await?;
                        core.synced.store(true, Ordering::SeqCst);
                        return Ok(out);
                    }
                }
            }
        }
        self.mirror(live, &current, &cur, now).await?;
        core.synced.store(true, Ordering::SeqCst);
        Ok(Synced::InStep { live })
    }

    /// File the supervisor's live revision in the console's history when
    /// it has no row of that number, and mark it live.
    async fn mirror(
        &self,
        live: u64,
        current: &Value,
        cur: &Spec,
        now: u64,
    ) -> Result<(), ConsoleError> {
        let core = &self.0;
        if core.store.revision(&core.dep, live).await?.is_none() {
            let actor = current["actor"].as_str().unwrap_or("system");
            let reason = current["reason"].as_str().unwrap_or_default();
            core.store
                .mirror_revision(
                    &core.dep,
                    live,
                    &cur.settings_json(),
                    &cur.secrets_json(),
                    actor,
                    reason,
                    now,
                )
                .await?;
        }
        core.store.set_live_revision(&core.dep, live).await
    }

    /// The deployment's status, as the supervisor reports it.
    pub async fn status(&self) -> Result<Status, ConsoleError> {
        Ok(self.0.client.status(ChildName::Deployment).await?)
    }

    /// The live revision and its spec. A secret the supervisor holds as a
    /// value is [`SecretRef::Held`]: the console never gets it back.
    pub async fn current(&self) -> Result<(u64, Spec), ConsoleError> {
        let v = self.0.client.current().await?;
        spec_of(&v)
    }

    /// Everything the Runtime screen reads: the deployment's status, its
    /// live revision and its last lines, and the other 2 children's state.
    pub async fn describe(&self) -> Result<Value, ConsoleError> {
        let c = &self.0.client;
        let status = c.status(ChildName::Deployment).await?;
        let (live, spec) = self.current().await?;
        let logs = c.logs(ChildName::Deployment, LOG_LINES).await?;
        let mut current = spec.public_json();
        current["revision"] = json!(live);
        let mut children = serde_json::Map::new();
        for child in [ChildName::Identity, ChildName::Console] {
            let s = match c.status(child).await {
                Ok(s) => json!(s),
                Err(e) => json!({ "error": e.to_string() }),
            };
            children.insert(child.as_str().to_string(), s);
        }
        Ok(json!({
            "mode": "supervise",
            "privilege": status.privilege,
            "status": status,
            "containerVariablesIgnored": [],
            "secretSettings": spec::SECRET_SETTINGS,
            "current": current,
            "logs": logs,
            "children": children,
        }))
    }

    /// Apply a change to the live spec (`plan-console.md` 6.1): filed as a
    /// revision here, then handed to the supervisor whole.
    pub async fn apply(&self, patch: &Patch, actor: &str) -> Result<Applied, ConsoleError> {
        let core = &self.0;
        patch.check(core.console_port)?;
        let _one = core.apply_lock.lock().await;
        let (_, old) = self.current().await?;
        let now = core.clock.now();
        let mut new = old.clone();
        for (k, v) in &patch.set {
            new.settings.insert(k.clone(), v.clone());
        }
        for k in &patch.unset {
            new.settings.remove(k);
        }
        for (name, input) in &patch.secrets {
            match input {
                None => {
                    new.secrets.remove(name);
                }
                Some(spec::SecretInput {
                    value: Some(v),
                    file: None,
                }) => {
                    let sealed = core.key.seal(&core.dep, name, v.as_bytes());
                    let version = core.store.put_secret(&core.dep, name, &sealed, now).await?;
                    new.secrets.insert(
                        name.clone(),
                        SecretRef::Managed {
                            version,
                            changed_at: now,
                        },
                    );
                }
                Some(spec::SecretInput {
                    value: None,
                    file: Some(f),
                }) => {
                    new.secrets.insert(
                        name.clone(),
                        SecretRef::Referenced {
                            path: f.clone(),
                            changed_at: now,
                        },
                    );
                }
                Some(_) => unreachable!("Patch::check allows one of value or file"),
            }
        }
        self.apply_spec(&old, &new, actor, patch.reason.trim())
            .await
    }

    /// Roll back: an earlier revision's spec, applied as a new revision.
    pub async fn rollback(
        &self,
        to: u64,
        reason: &str,
        actor: &str,
    ) -> Result<Applied, ConsoleError> {
        let core = &self.0;
        let _one = core.apply_lock.lock().await;
        let (_, old) = self.current().await?;
        let row = core
            .store
            .revision(&core.dep, to)
            .await?
            .ok_or(ConsoleError::UnknownRevision(to))?;
        let new = Spec::from_row(&row)?;
        let reason = if reason.trim().is_empty() {
            format!("roll back to revision {to}")
        } else {
            format!("roll back to revision {to}: {}", reason.trim())
        };
        self.apply_spec(&old, &new, actor, &reason).await
    }

    async fn apply_spec(
        &self,
        old: &Spec,
        new: &Spec,
        actor: &str,
        reason: &str,
    ) -> Result<Applied, ConsoleError> {
        let core = &self.0;
        let diff = Diff::between(old, new);
        if diff.is_empty() {
            return Err(ConsoleError::BadRequest(
                "that matches the live revision; there is nothing to apply (restart starts a \
                 new process with the same settings)"
                    .into(),
            ));
        }
        let now = core.clock.now();
        let number = core
            .store
            .insert_revision(
                &core.dep,
                &new.settings_json(),
                &new.secrets_json(),
                actor,
                reason,
                now,
            )
            .await?;
        let req = match self.request(number, actor, reason, new, old).await {
            Ok(r) => r,
            Err(e) => {
                core.store
                    .decide_revision(&core.dep, number, "refused", &e.to_string(), now)
                    .await?;
                return Err(e);
            }
        };
        let answer = core.client.apply(&req).await;
        let decided = core.clock.now();
        match answer {
            Ok(_) => {
                core.store
                    .decide_revision(&core.dep, number, "good", "", decided)
                    .await?;
                core.store.set_live_revision(&core.dep, number).await?;
                Ok(Applied {
                    revision: number,
                    diff,
                })
            }
            Err(e) => {
                let outcome = match e.kind() {
                    Kind::SettingsRefused | Kind::BadRequest => "refused",
                    _ => "failed",
                };
                core.store
                    .decide_revision(&core.dep, number, outcome, &e.to_string(), decided)
                    .await?;
                Err(e.into())
            }
        }
    }

    /// The same settings, a new process.
    pub async fn restart(&self) -> Result<(), ConsoleError> {
        Ok(self.0.client.restart(ChildName::Deployment).await?)
    }

    /// The whole request for one revision: settings as they are, each
    /// secret as a value (opened from the console's sealed copy), a file,
    /// or kept as the supervisor holds it. A sealed copy that does not
    /// open is kept when the live revision `live` holds that name.
    async fn request(
        &self,
        number: u64,
        actor: &str,
        reason: &str,
        spec: &Spec,
        live: &Spec,
    ) -> Result<ApplyRequest, ConsoleError> {
        let core = &self.0;
        let keep = || WireSecret {
            keep: true,
            ..WireSecret::default()
        };
        let mut secrets = BTreeMap::new();
        for (name, r) in &spec.secrets {
            let input = match r {
                SecretRef::Referenced { path, .. } => WireSecret {
                    file: Some(path.clone()),
                    ..WireSecret::default()
                },
                SecretRef::Held { .. } => keep(),
                SecretRef::Managed { version, .. } => {
                    let opened = match core.store.secret(&core.dep, name, *version).await? {
                        Some(sealed) => core.key.open(&core.dep, name, &sealed).ok(),
                        None => None,
                    };
                    match opened.map(String::from_utf8) {
                        Some(Ok(value)) => WireSecret {
                            value: Some(value),
                            ..WireSecret::default()
                        },
                        _ if live.secrets.contains_key(name) => {
                            eprintln!(
                                "ordo-console: {name} version {version} does not open with the \
                                 console's key; revision {number} keeps the value the \
                                 supervisor holds"
                            );
                            keep()
                        }
                        _ => {
                            return Err(ConsoleError::Internal(format!(
                                "secret {name} version {version} does not open with the \
                                 console's key, and the supervisor holds no value for it; send \
                                 it again under `secrets`"
                            )))
                        }
                    }
                }
            };
            secrets.insert(name.clone(), input);
        }
        Ok(ApplyRequest {
            revision: number,
            actor: actor.to_string(),
            reason: reason.to_string(),
            settings: spec.settings.clone(),
            secrets,
        })
    }
}

/// The supervisor's `current` answer as a spec: its number, its settings,
/// and each secret as a file or as a value it holds.
fn spec_of(v: &Value) -> Result<(u64, Spec), ConsoleError> {
    let bad = |what: &str| {
        ConsoleError::Supervisor(SupervisorError::Internal(format!(
            "the supervisor's live revision has no {what}"
        )))
    };
    let number = v["revision"].as_u64().ok_or_else(|| bad("number"))?;
    let settings: BTreeMap<String, String> =
        serde_json::from_value(v["settings"].clone()).map_err(|_| bad("settings"))?;
    let mut secrets = BTreeMap::new();
    for s in v["secrets"].as_array().ok_or_else(|| bad("secrets"))? {
        let name = s["name"].as_str().ok_or_else(|| bad("secret name"))?;
        let changed_at = s["changedAt"].as_u64().unwrap_or(0);
        let r = match (s["kind"].as_str(), s["path"].as_str()) {
            (Some("file"), Some(path)) => SecretRef::Referenced {
                path: path.to_string(),
                changed_at,
            },
            _ => SecretRef::Held { changed_at },
        };
        secrets.insert(name.to_string(), r);
    }
    Ok((number, Spec { settings, secrets }))
}
