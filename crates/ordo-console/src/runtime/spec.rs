//! A runtime spec (`plan-console.md` 6.1): the deployment's settings as
//! name and value, and its secrets by reference, never by value.
//!
//! Every applied spec is a revision row in the console's database. The
//! supervisor (`ordo-supervisor`) holds the live one and runs it; the
//! console keeps the history, the diff and the person's touch
//! (`plan-identity-service.md` 3.2). This module holds no I/O: it parses a
//! stored row, checks a requested change and says what it would change.

use std::collections::BTreeMap;
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::ConsoleError;
use crate::store::RevisionRow;

/// The revision label the child is started with (G16 reads it).
pub const REVISION_ENV: &str = "ORDO_RUNTIME_REVISION";
/// Who applied that revision, as the audit log names them (G16 reads it).
pub const APPLIED_BY_ENV: &str = "ORDO_RUNTIME_APPLIED_BY";
/// Where `ordo-host` listens; the supervisor polls `/health/ready` there.
pub const HOST_ADDR_ENV: &str = "ORDO_HOST_ADDR";
/// `ordo-host`'s own default.
pub const DEFAULT_HOST_ADDR: &str = "127.0.0.1:8080";
/// The longest secret value the console takes.
pub const MAX_SECRET_BYTES: usize = 64 * 1024;

/// The deployment settings that hold a secret (step 2b): the one list in
/// `ordo-env`, which the deployment's boot record reads too. Each is sealed
/// in the console database and reaches the child as `<NAME>_FILE`, never as
/// a value in a revision row or the child's environment.
pub use ordo_env::SECRET_SETTINGS;

/// Whether `name` is a setting the console treats as a secret
/// (`ordo_env::is_sealed_secret`).
pub fn is_secret(name: &str) -> bool {
    ordo_env::is_sealed_secret(name)
}

/// How a secret reaches the child. Either way the child gets `<NAME>_FILE`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SecretRef {
    /// Sealed in the console database; written to a private file per start.
    Managed {
        version: u64,
        #[serde(rename = "changedAt")]
        changed_at: u64,
    },
    /// A file the deployment owner mounted. The console never reads it.
    Referenced {
        path: String,
        #[serde(rename = "changedAt")]
        changed_at: u64,
    },
    /// A value the supervisor holds and the console has no copy of:
    /// revision 1's secrets, built by the supervisor from the container's
    /// variables. An apply keeps it as it is.
    Held {
        #[serde(rename = "changedAt")]
        changed_at: u64,
    },
}

#[derive(Serialize, Deserialize)]
struct SecretEntry {
    name: String,
    #[serde(flatten)]
    secret: SecretRef,
}

/// Settings and secret references.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Spec {
    pub settings: BTreeMap<String, String>,
    pub secrets: BTreeMap<String, SecretRef>,
}

impl Spec {
    pub fn settings_json(&self) -> String {
        serde_json::to_string(&self.settings).unwrap_or_else(|_| "{}".into())
    }

    pub fn secrets_json(&self) -> String {
        let list: Vec<SecretEntry> = self
            .secrets
            .iter()
            .map(|(name, secret)| SecretEntry {
                name: name.clone(),
                secret: secret.clone(),
            })
            .collect();
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())
    }

    /// Read a stored revision.
    pub fn from_row(row: &RevisionRow) -> Result<Spec, ConsoleError> {
        let settings: BTreeMap<String, String> =
            serde_json::from_str(&row.settings).map_err(|e| {
                ConsoleError::Internal(format!("revision {} settings: {e}", row.number))
            })?;
        let list: Vec<SecretEntry> = serde_json::from_str(&row.secret_refs).map_err(|e| {
            ConsoleError::Internal(format!("revision {} secret references: {e}", row.number))
        })?;
        Ok(Spec {
            settings,
            secrets: list.into_iter().map(|e| (e.name, e.secret)).collect(),
        })
    }

    /// The spec as the Runtime screen shows it: secrets as set, with their
    /// kind and when they changed, never a value.
    pub fn public_json(&self) -> Value {
        let secrets: Vec<Value> = self
            .secrets
            .iter()
            .map(|(name, s)| match s {
                SecretRef::Managed {
                    version,
                    changed_at,
                } => json!({ "name": name, "kind": "managed", "set": true,
                             "version": version, "changedAt": changed_at }),
                SecretRef::Referenced { path, changed_at } => json!({
                    "name": name, "kind": "referenced", "set": true,
                    "path": path, "changedAt": changed_at }),
                SecretRef::Held { changed_at } => json!({
                    "name": name, "kind": "held", "set": true, "changedAt": changed_at }),
            })
            .collect();
        json!({ "settings": self.settings, "secrets": secrets })
    }
}

/// A secret in a requested change: a value the console seals, or a file
/// the deployment owner mounted.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretInput {
    pub value: Option<String>,
    pub file: Option<String>,
}

/// A requested change to the live spec.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    /// Settings to add or change.
    #[serde(default)]
    pub set: BTreeMap<String, String>,
    /// Settings to remove.
    #[serde(default)]
    pub unset: Vec<String>,
    /// Secrets to set, or to remove (`null`).
    #[serde(default)]
    pub secrets: BTreeMap<String, Option<SecretInput>>,
    /// Why, for the revision history.
    #[serde(default)]
    pub reason: String,
}

fn refuse(msg: String) -> ConsoleError {
    ConsoleError::BadRequest(msg)
}

/// A deployment setting's name, by the supervisor's own rule
/// (`ordo_supervisor::spec::check_name`): `ORDO_` and upper case, never the
/// console's, the supervisor's or the identity wiring's. Refused here
/// first, so nothing is sealed or filed for a change the supervisor would
/// refuse.
fn check_name(name: &str) -> Result<(), ConsoleError> {
    ordo_supervisor::spec::check_name(name).map_err(|e| refuse(e.to_string()))
}

impl Patch {
    /// Check the change on its own, before anything is sealed or stored.
    /// `console_port` is the port the console listens on, which the
    /// deployment may not take.
    pub fn check(&self, console_port: u16) -> Result<(), ConsoleError> {
        for (name, value) in &self.set {
            check_name(name)?;
            if let Some(stem) = name.strip_suffix("_FILE") {
                return Err(refuse(format!(
                    "{name} names a secret file; send it under `secrets` as {stem} with \
                     {{\"file\": \"/path\"}}, and the console passes {name} to the deployment"
                )));
            }
            if value.contains('\0') {
                return Err(refuse(format!("{name} holds a NUL byte")));
            }
            if self.secrets.contains_key(name) {
                return Err(refuse(format!(
                    "{name} is sent as both a setting and a secret; send it once"
                )));
            }
            if is_secret(name) {
                return Err(refuse(format!(
                    "{name} holds a secret; send it under `secrets` as {name} with \
                     {{\"value\": \"...\"}}, and the console seals it and passes {name}_FILE"
                )));
            }
        }
        for name in &self.unset {
            check_name(name)?;
            if is_secret(name) {
                return Err(refuse(format!(
                    "{name} holds a secret; remove it under `secrets` as {name}: null"
                )));
            }
        }
        if let Some(addr) = self.set.get(HOST_ADDR_ENV) {
            match addr.trim().parse::<SocketAddr>() {
                Ok(a) if a.port() == console_port => {
                    return Err(refuse(format!(
                        "{HOST_ADDR_ENV} is `{addr}`, the console's own port; give the \
                         deployment another port"
                    )))
                }
                Ok(_) => {}
                Err(_) => {
                    return Err(refuse(format!(
                        "{HOST_ADDR_ENV} is `{addr}`, which is not an address and port, for \
                         example 0.0.0.0:8080"
                    )))
                }
            }
        }
        for (name, input) in &self.secrets {
            check_name(name)?;
            if name.ends_with("_FILE") {
                return Err(refuse(format!(
                    "secret {name}: name it without `_FILE`; the console adds it"
                )));
            }
            let Some(input) = input else { continue };
            match (&input.value, &input.file) {
                (Some(v), None) => {
                    if v.is_empty() || v.len() > MAX_SECRET_BYTES {
                        return Err(refuse(format!(
                            "secret {name}: a value of 1 to {MAX_SECRET_BYTES} bytes"
                        )));
                    }
                }
                (None, Some(f)) => {
                    if !f.starts_with('/') || f.contains('\0') {
                        return Err(refuse(format!(
                            "secret {name}: `file` must be an absolute path"
                        )));
                    }
                }
                _ => {
                    return Err(refuse(format!(
                        "secret {name}: send exactly one of `value` or `file`"
                    )))
                }
            }
        }
        Ok(())
    }
}

/// What a change does, for the screen and the audit row. Secrets are named,
/// never shown.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Diff {
    pub added: BTreeMap<String, String>,
    pub changed: BTreeMap<String, (String, String)>,
    pub removed: Vec<String>,
    #[serde(rename = "secretsChanged")]
    pub secrets_changed: Vec<String>,
}

impl Diff {
    pub fn between(old: &Spec, new: &Spec) -> Diff {
        let mut d = Diff::default();
        for (k, v) in &new.settings {
            match old.settings.get(k) {
                None => {
                    d.added.insert(k.clone(), v.clone());
                }
                Some(o) if o != v => {
                    d.changed.insert(k.clone(), (o.clone(), v.clone()));
                }
                _ => {}
            }
        }
        d.removed = old
            .settings
            .keys()
            .filter(|k| !new.settings.contains_key(*k))
            .cloned()
            .collect();
        let names: std::collections::BTreeSet<&String> =
            old.secrets.keys().chain(new.secrets.keys()).collect();
        d.secrets_changed = names
            .into_iter()
            .filter(|n| old.secrets.get(*n) != new.secrets.get(*n))
            .cloned()
            .collect();
        d
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.changed.is_empty()
            && self.removed.is_empty()
            && self.secrets_changed.is_empty()
    }

    /// One line for the audit row: names only.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        let names = |m: Vec<&String>| m.into_iter().cloned().collect::<Vec<_>>().join(",");
        if !self.added.is_empty() {
            parts.push(format!("added={}", names(self.added.keys().collect())));
        }
        if !self.changed.is_empty() {
            parts.push(format!("changed={}", names(self.changed.keys().collect())));
        }
        if !self.removed.is_empty() {
            parts.push(format!("removed={}", self.removed.join(",")));
        }
        if !self.secrets_changed.is_empty() {
            parts.push(format!("secrets={}", self.secrets_changed.join(",")));
        }
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_refuses_a_secret_as_a_setting_naming_where_it_goes() {
        let p = Patch {
            set: [("ORDO_PG_PASSWORD".to_string(), "x".to_string())].into(),
            ..Patch::default()
        };
        let e = p.check(8081).unwrap_err().to_string();
        assert!(
            e.contains("ORDO_PG_PASSWORD") && e.contains("secrets"),
            "{e}"
        );
        let p = Patch {
            unset: vec!["ORDO_TOKEN_KEY".into()],
            ..Patch::default()
        };
        assert!(p.check(8081).is_err());
        for name in SECRET_SETTINGS {
            assert!(is_secret(name));
        }
        assert!(!is_secret("ORDO_PG_HOST") && !is_secret("ORDO_AUTH_HUMAN_CHECK_SITE_KEY"));
    }

    /// Console step 6: the claim code is a secret. A change sends it under
    /// `secrets`, and no revision row holds it.
    #[test]
    fn a_revision_row_never_holds_the_claim_code() {
        let p = Patch {
            set: [("ORDO_AUTH_CLAIM_CODE".to_string(), "required".to_string())].into(),
            ..Patch::default()
        };
        let e = p.check(8081).unwrap_err().to_string();
        assert!(e.contains("secrets"), "{e}");
    }

    #[test]
    fn a_patch_refuses_console_settings_files_and_the_console_port() {
        let p = |set: &[(&str, &str)]| Patch {
            set: set
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Patch::default()
        };
        assert!(p(&[("ORDO_CONSOLE_MODE", "x")]).check(8081).is_err());
        assert!(p(&[("ORDO_PG_PASSWORD_FILE", "/x")]).check(8081).is_err());
        assert!(p(&[(REVISION_ENV, "3")]).check(8081).is_err());
        assert!(p(&[(APPLIED_BY_ENV, "owner:x")]).check(8081).is_err());
        assert!(p(&[("lower", "x")]).check(8081).is_err());
        // Review, step 7: the supervisor's rule, refused here first.
        assert!(p(&[("LD_PRELOAD", "/tmp/x.so")]).check(8081).is_err());
        assert!(p(&[("ORDO_IDENTITY_URL", "http://x")]).check(8081).is_err());
        assert!(p(&[(HOST_ADDR_ENV, "0.0.0.0:8081")]).check(8081).is_err());
        assert!(p(&[(HOST_ADDR_ENV, "0.0.0.0:8080")]).check(8081).is_ok());
    }

    #[test]
    fn the_diff_names_secrets_without_values() {
        let old = Spec::default();
        let mut new = Spec::default();
        new.settings.insert("ORDO_A".into(), "1".into());
        new.secrets.insert(
            "ORDO_S".into(),
            SecretRef::Managed {
                version: 1,
                changed_at: 1,
            },
        );
        let d = Diff::between(&old, &new);
        assert_eq!(d.summary(), "added=ORDO_A secrets=ORDO_S");
        assert!(Diff::between(&new, &new).is_empty());
    }
}
