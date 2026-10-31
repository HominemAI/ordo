//! The boot record (`plan-console.md` 7, G16): what started this
//! deployment, written to its own security record and shown on
//! `/health/deep`.
//!
//! The console can restart the deployment with any settings, including
//! ones that loosen security. This record is the deployment's side of that
//! power: a restart is never silent here, whoever caused it.
//!
//! | Field | From |
//! |---|---|
//! | `revision` | `ORDO_RUNTIME_REVISION`, set by the console's supervisor |
//! | `applied_by` | `ORDO_RUNTIME_APPLIED_BY`, the console owner who applied that revision |
//! | `image` | `ORDO_IMAGE_DIGEST`, when the platform sets it |
//! | `binary` | the SHA-256 of the running `ordo-host` binary, and its version |
//! | `settings` | a SHA-256 over every `ORDO_*` setting's name and value, with a secret's value left out |
//!
//! A deployment started without the console has no revision and no
//! applier, and says so: `revision=none applied_by=none`.

use std::sync::OnceLock;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The console's revision number for this start.
pub const REVISION_ENV: &str = "ORDO_RUNTIME_REVISION";
/// Who applied that revision, as the console names them (`owner:<id>`).
pub const APPLIED_BY_ENV: &str = "ORDO_RUNTIME_APPLIED_BY";
/// The image digest, when the platform passes it in.
pub const IMAGE_ENV: &str = "ORDO_IMAGE_DIGEST";

/// What started this process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootRecord {
    pub revision: Option<String>,
    pub applied_by: Option<String>,
    pub image: Option<String>,
    pub binary_sha256: Option<String>,
    pub version: String,
    pub settings_fingerprint: String,
    pub at: u64,
}

/// Keep a value safe for a `key=value` line: printable, no spaces, short.
fn clean(v: &str) -> Option<String> {
    let v: String = v
        .trim()
        .chars()
        .filter(|c| c.is_ascii_graphic() && *c != '=')
        .take(128)
        .collect();
    (!v.is_empty()).then_some(v)
}

/// A setting whose value is a secret: named in the fingerprint as set,
/// never hashed by value, since a hash of a short code can be guessed. The
/// one list, shared with the console (`ordo_env::hides_value`).
fn is_secret(name: &str) -> bool {
    ordo_env::hides_value(name)
}

/// The fingerprint of the deployment's settings: every `ORDO_*` variable,
/// sorted, one `NAME=value` line each; a secret's value is replaced by
/// `<set>`. The two variables this record reports on its own are left out,
/// so the same settings under a new revision number keep one fingerprint.
pub fn fingerprint(vars: &[(String, String)]) -> String {
    let mut lines: Vec<String> = vars
        .iter()
        .filter(|(k, _)| k.starts_with("ORDO_") && k != REVISION_ENV && k != APPLIED_BY_ENV)
        .map(|(k, v)| {
            if is_secret(k) {
                format!("{k}=<set>")
            } else {
                format!("{k}={v}")
            }
        })
        .collect();
    lines.sort();
    let mut h = Sha256::new();
    for l in &lines {
        h.update(l.as_bytes());
        h.update(b"\n");
    }
    hex(&h.finalize())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl BootRecord {
    /// From a set of variables, with the binary's hash already taken.
    pub fn from_vars(vars: &[(String, String)], binary_sha256: Option<String>, at: u64) -> Self {
        let get = |k: &str| {
            vars.iter()
                .find(|(n, _)| n == k)
                .and_then(|(_, v)| clean(v))
        };
        BootRecord {
            revision: get(REVISION_ENV),
            applied_by: get(APPLIED_BY_ENV),
            image: get(IMAGE_ENV),
            binary_sha256,
            version: env!("CARGO_PKG_VERSION").to_string(),
            settings_fingerprint: fingerprint(vars),
            at,
        }
    }

    /// From this process: its environment and its own binary.
    pub async fn of_this_process(at: u64) -> BootRecord {
        let vars: Vec<(String, String)> = std::env::vars().collect();
        let binary = tokio::task::spawn_blocking(|| {
            let path = std::env::current_exe().ok()?;
            let mut f = std::fs::File::open(path).ok()?;
            let mut h = Sha256::new();
            std::io::copy(&mut f, &mut h).ok()?;
            Some(hex(&h.finalize()))
        })
        .await
        .ok()
        .flatten();
        BootRecord::from_vars(&vars, binary, at)
    }

    /// The one line the security record carries.
    pub fn detail(&self) -> String {
        let or_none = |v: &Option<String>| v.clone().unwrap_or_else(|| "none".into());
        format!(
            "revision={} applied_by={} image={} binary={} version={} settings={}",
            or_none(&self.revision),
            or_none(&self.applied_by),
            or_none(&self.image),
            or_none(&self.binary_sha256),
            self.version,
            self.settings_fingerprint
        )
    }

    /// What `/health/deep` shows.
    pub fn json(&self) -> Value {
        json!({
            "revision": self.revision,
            "applied_by": self.applied_by,
            "image": self.image,
            "binary_sha256": self.binary_sha256,
            "version": self.version,
            "settings_fingerprint": self.settings_fingerprint,
            "at": self.at,
        })
    }

    /// Write it to the deployment's security record.
    pub fn record(&self, stream: &ordo_secevent::Recorder) {
        stream.observe(
            ordo_secevent::Observation {
                space: None,
                subject: ordo_secevent::Subject::platform("boot"),
                op: "deployment.boot".to_string(),
                kind: ordo_secevent::EventKind::DeploymentBooted {
                    detail: self.detail(),
                },
                at: ordo_commandlog::Timestamp(self.at),
                correlation: ordo_secevent::correlation::mint(),
            },
            ordo_secevent::Relationship::None,
        );
    }
}

static CURRENT: OnceLock<BootRecord> = OnceLock::new();

/// Record this boot once: in the security record, and for `/health/deep`.
pub fn install(record: BootRecord, stream: &ordo_secevent::Recorder) {
    record.record(stream);
    let _ = CURRENT.set(record);
}

/// The boot record, for `/health/deep`. `null` in a process that never
/// installed one (an embedded door, a test harness).
pub fn current_json() -> Value {
    CURRENT.get().map(BootRecord::json).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_secret_value_never_moves_the_fingerprint() {
        let a = fingerprint(&v(&[("ORDO_PG_PASSWORD", "one"), ("ORDO_PG_DB", "x")]));
        let b = fingerprint(&v(&[("ORDO_PG_PASSWORD", "two"), ("ORDO_PG_DB", "x")]));
        let c = fingerprint(&v(&[("ORDO_PG_DB", "x")]));
        let d = fingerprint(&v(&[("ORDO_PG_PASSWORD", "one"), ("ORDO_PG_DB", "y")]));
        assert_eq!(a, b, "a secret's value is not hashed");
        assert_ne!(a, c, "that it is set is");
        assert_ne!(a, d, "a plain setting's value is");
    }

    /// Security review, step 3 (finding 7): the claim code and a tenant
    /// credential were hashed by value, so a short one could be guessed
    /// from the fingerprint. Every name on the one list is hidden. The
    /// claim code is no longer read (owner, 2026-10-30), but an old
    /// environment may still hold one, so it stays hidden.
    #[test]
    fn a_claim_code_or_a_credential_never_moves_the_fingerprint() {
        for name in [
            "ORDO_AUTH_CLAIM_CODE",
            "ORDO_PG_TENANT_CREDENTIAL",
            "ORDO_MAIL_CREDENTIALS",
            "ORDO_PUBLIC_HUMAN_SECRET",
        ] {
            let a = fingerprint(&v(&[(name, "1234"), ("ORDO_PG_DB", "x")]));
            let b = fingerprint(&v(&[(name, "9999"), ("ORDO_PG_DB", "x")]));
            assert_eq!(a, b, "{name}'s value moved the fingerprint");
        }
        for name in ordo_env::SECRET_SETTINGS {
            assert!(is_secret(name), "{name}");
        }
    }

    #[test]
    fn the_revision_does_not_move_the_fingerprint_and_order_does_not_matter() {
        let a = fingerprint(&v(&[("ORDO_A", "1"), ("ORDO_B", "2"), (REVISION_ENV, "3")]));
        let b = fingerprint(&v(&[
            ("ORDO_B", "2"),
            (REVISION_ENV, "4"),
            ("ORDO_A", "1"),
            ("HOME", "/x"),
        ]));
        assert_eq!(a, b);
    }

    #[test]
    fn the_line_names_the_revision_and_who_applied_it() {
        let r = BootRecord::from_vars(
            &v(&[
                (REVISION_ENV, "7"),
                (APPLIED_BY_ENV, "owner:o1 extra=evil\nline"),
            ]),
            Some("ab".into()),
            5,
        );
        let line = r.detail();
        assert!(
            line.starts_with("revision=7 applied_by=owner:o1extraevilline "),
            "{line}"
        );
        assert!(!line.contains('\n'));
        let none = BootRecord::from_vars(&[], None, 5).detail();
        assert!(none.starts_with("revision=none applied_by=none image=none binary=none"));
    }
}
