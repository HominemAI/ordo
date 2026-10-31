//! What a child is started with, and the deployment's live spec on the
//! supervisor's own volume (`plan-identity-service.md` 3.2).
//!
//! A [`Spec`] is settings by name and value, and secrets: a value the
//! supervisor writes to a private file for each start, or a file the
//! deployment owner mounted. Either way the child reads `<NAME>_FILE`.
//!
//! The deployment's spec is a [`Revision`]. The live one and the last good
//! one before it are kept in `ORDO_SUPERVISOR_STATE_DIR`, each sealed with
//! `ORDO_SUPERVISOR_KEY_FILE` (the shared sealing module), written whole
//! and renamed into place, so a crash leaves the old file or the new one.
//! The supervisor must start the deployment before the console is up, so
//! it never reads the console's database.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};

use ordo_identity::sealing::SealingKey;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::KEY_FILE_ENV;
use crate::error::SupervisorError;

/// The revision label the deployment is started with (G16 reads it).
pub const REVISION_ENV: &str = "ORDO_RUNTIME_REVISION";
/// Who applied that revision (G16 reads it).
pub const APPLIED_BY_ENV: &str = "ORDO_RUNTIME_APPLIED_BY";
/// Where `ordo-host` listens; the supervisor polls `/health/ready` there.
pub const HOST_ADDR_ENV: &str = "ORDO_HOST_ADDR";
pub const DEFAULT_HOST_ADDR: &str = "127.0.0.1:8080";
/// The longest secret value the supervisor takes.
pub const MAX_SECRET_BYTES: usize = 64 * 1024;

/// One secret.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Secret {
    /// Held by the supervisor (sealed on its volume for the deployment, in
    /// memory for the others) and written to a private file per start.
    Value {
        value: String,
        #[serde(rename = "changedAt")]
        changed_at: u64,
    },
    /// A file the deployment owner mounted. The supervisor never reads it.
    File {
        path: String,
        #[serde(rename = "changedAt")]
        changed_at: u64,
    },
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Secret::Value { changed_at, .. } => write!(f, "Value(set, {changed_at})"),
            Secret::File { path, changed_at } => write!(f, "File({path}, {changed_at})"),
        }
    }
}

/// Settings and secrets for one child.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spec {
    pub settings: BTreeMap<String, String>,
    pub secrets: BTreeMap<String, Secret>,
}

impl Spec {
    /// Where the deployment's `/health/ready` is polled: its own address,
    /// with an every-interface address read as loopback.
    pub fn host_probe(&self) -> SocketAddr {
        probe_addr(
            self.settings.get(HOST_ADDR_ENV).map(String::as_str),
            DEFAULT_HOST_ADDR,
        )
    }

    /// The spec as the console's Runtime screen shows it: secrets by kind
    /// and when they changed, never a value.
    pub fn public_json(&self) -> Value {
        let secrets: Vec<Value> = self
            .secrets
            .iter()
            .map(|(name, s)| match s {
                Secret::Value { changed_at, .. } => {
                    json!({ "name": name, "kind": "value", "set": true, "changedAt": changed_at })
                }
                Secret::File { path, changed_at } => json!({
                    "name": name, "kind": "file", "set": true, "path": path,
                    "changedAt": changed_at }),
            })
            .collect();
        json!({ "settings": self.settings, "secrets": secrets })
    }

    /// The names of container variables that differ from this spec: once a
    /// live spec exists they are ignored, and the log says so.
    pub fn ignored_container_vars(&self, env: &[(String, String)]) -> Vec<String> {
        let mut out = Vec::new();
        for (k, v) in env {
            let same = match k.strip_suffix("_FILE").filter(|s| !s.is_empty()) {
                Some(stem) => matches!(
                    self.secrets.get(stem),
                    Some(Secret::File { path, .. }) if path == v
                ),
                None if ordo_env::is_sealed_secret(k) => matches!(
                    self.secrets.get(k),
                    Some(Secret::Value { value, .. }) if value == v
                ),
                None => self.settings.get(k) == Some(v),
            };
            if !same {
                out.push(k.clone());
            }
        }
        out
    }
}

/// An address to poll, with an every-interface address read as loopback.
pub fn probe_addr(text: Option<&str>, default: &str) -> SocketAddr {
    let mut addr: SocketAddr = text
        .unwrap_or(default)
        .trim()
        .parse()
        .unwrap_or_else(|_| default.parse().expect("the default parses"));
    if addr.ip().is_unspecified() {
        addr.set_ip(match addr.ip() {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
        });
    }
    addr
}

/// Where a revision came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    /// Revision 1, built from the container's variables at first boot.
    Container,
    /// Handed over by the console with `apply`.
    Console,
}

/// One deployment revision as the supervisor holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    pub number: u64,
    pub actor: String,
    pub reason: String,
    pub origin: Origin,
    pub at: u64,
    pub spec: Spec,
}

impl Revision {
    /// Revision 1, from the variables the container started with
    /// (`plan-console.md` 6.3, as the console built it). Every `ORDO_*`
    /// variable that is no other child's and not the supervisor's own:
    /// `<NAME>_FILE` becomes a file secret, a secret setting given as a
    /// value ([`ordo_env::is_sealed_secret`]) a sealed value, everything
    /// else a setting.
    pub fn from_container(env: &[(String, String)], now: u64) -> Revision {
        let mut spec = Spec::default();
        let mut values = Vec::new();
        for (k, v) in env.iter().filter(|(k, _)| is_deployment_var(k)) {
            match k.strip_suffix("_FILE").filter(|s| !s.is_empty()) {
                Some(stem) => {
                    spec.secrets.insert(
                        stem.to_string(),
                        Secret::File {
                            path: v.clone(),
                            changed_at: now,
                        },
                    );
                }
                None if ordo_env::is_sealed_secret(k) => values.push((k.clone(), v.clone())),
                None => {
                    spec.settings.insert(k.clone(), v.clone());
                }
            }
        }
        // A value and a file for one secret: the file wins, as the owner
        // mounted it on purpose.
        for (k, v) in values {
            spec.secrets.entry(k).or_insert(Secret::Value {
                value: v,
                changed_at: now,
            });
        }
        Revision {
            number: 1,
            actor: "system".into(),
            reason: "built from the container's ORDO_* variables at first boot".into(),
            origin: Origin::Container,
            at: now,
            spec,
        }
    }

    /// The revision without secret values, for `current`.
    pub fn public_json(&self) -> Value {
        let mut v = self.spec.public_json();
        v["revision"] = json!(self.number);
        v["actor"] = json!(self.actor);
        v["reason"] = json!(self.reason);
        v["origin"] = json!(self.origin);
        v["at"] = json!(self.at);
        v
    }
}

/// Whether a container variable is the deployment's: `ORDO_*`, and not the
/// console's, the identity service's or the supervisor's.
pub fn is_deployment_var(k: &str) -> bool {
    k.starts_with("ORDO_")
        && !k.starts_with("ORDO_CONSOLE_")
        && !k.starts_with("ORDO_IDENTITY_")
        && !k.starts_with("ORDO_SUPERVISOR_")
}

/// One secret in an `apply`: a value, a file, or the live one kept.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Keep the live revision's secret of this name as it is. The console
    /// does not hold every value: revision 1's were sealed here.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keep: bool,
}

/// What the console hands the supervisor to apply: the whole resolved
/// spec, numbered by the console's revision history.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyRequest {
    pub revision: u64,
    pub actor: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
    #[serde(default)]
    pub secrets: BTreeMap<String, SecretInput>,
}

fn refuse(msg: String) -> SupervisorError {
    SupervisorError::BadRequest(msg)
}

/// A deployment setting's name: `ORDO_`, then upper case, digits and
/// underscores, and none of the names the supervisor or another child owns.
///
/// The prefix is required. The console may be compromised, and a name such
/// as `LD_PRELOAD` would then run code of its choosing inside the
/// deployment, as the deployment's user. `ORDO_IDENTITY_*` is refused too:
/// it is the wiring that tells the door which identity service to trust,
/// and the supervisor alone sets it.
pub fn check_name(name: &str) -> Result<(), SupervisorError> {
    let ok = name.len() <= 128
        && name.len() > "ORDO_".len()
        && name.starts_with("ORDO_")
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    if !ok {
        return Err(refuse(format!(
            "`{name}` is not a deployment setting; a setting starts with ORDO_ and holds \
             upper-case letters, digits and underscores"
        )));
    }
    for (prefix, whose) in [
        (
            "ORDO_CONSOLE_",
            "the console's own setting; set it on the container",
        ),
        (
            "ORDO_SUPERVISOR_",
            "the supervisor's own setting; set it on the container",
        ),
        (
            "ORDO_IDENTITY_",
            "the identity wiring, which the supervisor sets for each start from the \
             container's settings",
        ),
    ] {
        if name.starts_with(prefix) {
            return Err(refuse(format!(
                "{name} is {whose}; it cannot be set on the deployment"
            )));
        }
    }
    if name == REVISION_ENV || name == APPLIED_BY_ENV {
        return Err(refuse(format!(
            "{name} is set by the supervisor for each start; it cannot be set"
        )));
    }
    Ok(())
}

impl ApplyRequest {
    /// Check the request on its own, and build the revision it asks for.
    /// `live` gives the secrets a `keep` refers to. `taken` are the ports
    /// the other children and the supervisor listen on.
    pub fn resolve(
        &self,
        live: &Revision,
        taken: &[(u16, &str)],
        now: u64,
    ) -> Result<Revision, SupervisorError> {
        if self.revision == 0 {
            return Err(refuse("revision: a number of 1 or more".into()));
        }
        if self.actor.trim().is_empty() {
            return Err(refuse(
                "actor: who applied it, as the audit names them".into(),
            ));
        }
        for (name, value) in &self.settings {
            check_name(name)?;
            if let Some(stem) = name.strip_suffix("_FILE") {
                return Err(refuse(format!(
                    "{name} names a secret file; send it under `secrets` as {stem} with \
                     {{\"file\": \"/path\"}}"
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
            if ordo_env::is_sealed_secret(name) {
                return Err(refuse(format!(
                    "{name} holds a secret; send it under `secrets` as {name} with \
                     {{\"value\": \"...\"}}"
                )));
            }
        }
        if let Some(addr) = self.settings.get(HOST_ADDR_ENV) {
            match addr.trim().parse::<SocketAddr>() {
                Ok(a) => {
                    if let Some((_, whose)) = taken.iter().find(|(p, _)| *p == a.port()) {
                        return Err(refuse(format!(
                            "{HOST_ADDR_ENV} is `{addr}`, the port {whose} listens on; give the \
                             deployment another port"
                        )));
                    }
                }
                Err(_) => {
                    return Err(refuse(format!(
                        "{HOST_ADDR_ENV} is `{addr}`, which is not an address and port, for \
                         example 0.0.0.0:8080"
                    )))
                }
            }
        }
        let mut spec = Spec {
            settings: self.settings.clone(),
            secrets: BTreeMap::new(),
        };
        for (name, input) in &self.secrets {
            check_name(name)?;
            if name.ends_with("_FILE") {
                return Err(refuse(format!(
                    "secret {name}: name it without `_FILE`; the supervisor adds it"
                )));
            }
            let secret = match (&input.value, &input.file, input.keep) {
                (Some(v), None, false) => {
                    if v.is_empty() || v.len() > MAX_SECRET_BYTES || v.contains('\0') {
                        return Err(refuse(format!(
                            "secret {name}: a value of 1 to {MAX_SECRET_BYTES} bytes, no NUL"
                        )));
                    }
                    match live.spec.secrets.get(name) {
                        // The same value keeps its date, so the diff is true.
                        Some(s @ Secret::Value { value, .. }) if value == v => s.clone(),
                        _ => Secret::Value {
                            value: v.clone(),
                            changed_at: now,
                        },
                    }
                }
                (None, Some(f), false) => {
                    if !f.starts_with('/') || f.contains('\0') {
                        return Err(refuse(format!(
                            "secret {name}: `file` must be an absolute path"
                        )));
                    }
                    match live.spec.secrets.get(name) {
                        Some(s @ Secret::File { path, .. }) if path == f => s.clone(),
                        _ => Secret::File {
                            path: f.clone(),
                            changed_at: now,
                        },
                    }
                }
                (None, None, true) => live.spec.secrets.get(name).cloned().ok_or_else(|| {
                    refuse(format!(
                        "secret {name}: `keep` names a secret the live revision {} does not \
                         hold",
                        live.number
                    ))
                })?,
                _ => {
                    return Err(refuse(format!(
                        "secret {name}: send exactly one of `value`, `file` or `keep`"
                    )))
                }
            };
            spec.secrets.insert(name.clone(), secret);
        }
        Ok(Revision {
            number: self.revision,
            actor: self.actor.trim().to_string(),
            reason: self.reason.trim().to_string(),
            origin: Origin::Console,
            at: now,
            spec,
        })
    }
}

/// What a change does, names only: the audit line and the answer to
/// `apply`. Secrets are named, never shown.
pub fn diff_summary(old: &Spec, new: &Spec) -> String {
    let mut added = Vec::new();
    let mut changed = Vec::new();
    for (k, v) in &new.settings {
        match old.settings.get(k) {
            None => added.push(k.as_str()),
            Some(o) if o != v => changed.push(k.as_str()),
            _ => {}
        }
    }
    let removed: Vec<&str> = old
        .settings
        .keys()
        .filter(|k| !new.settings.contains_key(*k))
        .map(String::as_str)
        .collect();
    let names: std::collections::BTreeSet<&String> =
        old.secrets.keys().chain(new.secrets.keys()).collect();
    let secrets: Vec<&str> = names
        .into_iter()
        .filter(|n| old.secrets.get(*n) != new.secrets.get(*n))
        .map(String::as_str)
        .collect();
    let mut parts = Vec::new();
    for (word, list) in [
        ("added", added),
        ("changed", changed),
        ("removed", removed),
        ("secrets", secrets),
    ] {
        if !list.is_empty() {
            parts.push(format!("{word}={}", list.join(",")));
        }
    }
    parts.join(" ")
}

/// The live and last good revisions on the supervisor's volume.
pub struct SpecStore {
    dir: PathBuf,
    key: SealingKey,
}

const ENTITY: &str = "ordo-supervisor.revision";
/// The live revision's slot on the volume.
pub const LIVE: &str = "live";
/// The last good revision's slot.
pub const GOOD: &str = "good";

impl SpecStore {
    /// Make the folder (0700) and read or make the key.
    pub fn open(dir: &Path, key_file: &Path) -> Result<SpecStore, SupervisorError> {
        private_dir(dir).map_err(|e| {
            SupervisorError::setting_caused(
                crate::config::STATE_DIR_ENV,
                format!(
                    "is `{}`, and the supervisor could not make it",
                    dir.display()
                ),
                format!(
                    "Mount a volume there that the supervisor's user (65530 in the image) owns, \
                     or set {} to one",
                    crate::config::STATE_DIR_ENV
                ),
                e,
            )
        })?;
        let loaded =
            SealingKey::load_or_create(key_file, KEY_FILE_ENV, "supervisor").map_err(|e| {
                SupervisorError::Setting {
                setting: KEY_FILE_ENV,
                problem: e.to_string(),
                fix: "Point it at the supervisor's key file, or at a path on its volume where it \
                      may make one"
                    .into(),
                cause: Some(Box::new(e)),
            }
            })?;
        if loaded.made {
            eprintln!(
                "ordo-supervisor: made a new sealing key at {}; back it up with the volume",
                key_file.display()
            );
        }
        Ok(SpecStore {
            dir: dir.to_path_buf(),
            key: loaded.key,
        })
    }

    fn path(&self, slot: &str) -> PathBuf {
        self.dir.join(format!("deployment-{slot}.sealed"))
    }

    /// Move a slot's file that does not open out of the way, kept beside
    /// it with the time in its name, so the owner can restore the key and
    /// read it again. Returns where it went, or why it could not move.
    pub fn set_aside(&self, slot: &str, now: u64) -> String {
        let from = self.path(slot);
        let to = self
            .dir
            .join(format!("deployment-{slot}.sealed.unopened-{now}"));
        match std::fs::rename(&from, &to) {
            Ok(()) => to.display().to_string(),
            Err(e) => format!("nowhere ({} could not be moved: {e})", from.display()),
        }
    }

    fn read(&self, slot: &str) -> Result<Option<Revision>, SupervisorError> {
        let path = self.path(slot);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(SupervisorError::Internal(format!(
                    "{} could not be read: {e}",
                    path.display()
                )))
            }
        };
        let plain = self.key.open(ENTITY, slot, text.trim()).map_err(|_| {
            SupervisorError::setting(
                KEY_FILE_ENV,
                format!(
                    "holds a key that does not open {}, so the live spec cannot be read",
                    path.display()
                ),
                "Restore the supervisor's key file from the backup made with this volume. \
                 Deleting the sealed file instead makes the supervisor build revision 1 from \
                 the container's variables again",
            )
        })?;
        serde_json::from_slice(&plain)
            .map(Some)
            .map_err(|e| SupervisorError::Internal(format!("{}: {e}", path.display())))
    }

    fn write(&self, slot: &str, rev: &Revision) -> Result<(), SupervisorError> {
        let plain =
            serde_json::to_vec(rev).map_err(|e| SupervisorError::Internal(e.to_string()))?;
        let sealed = self.key.seal(ENTITY, slot, &plain);
        let path = self.path(slot);
        write_atomic(&path, sealed.as_bytes()).map_err(|e| {
            SupervisorError::Internal(format!("{} could not be written: {e}", path.display()))
        })
    }

    pub fn live(&self) -> Result<Option<Revision>, SupervisorError> {
        self.read(LIVE)
    }

    pub fn good(&self) -> Result<Option<Revision>, SupervisorError> {
        self.read(GOOD)
    }

    pub fn set_live(&self, rev: &Revision) -> Result<(), SupervisorError> {
        self.write(LIVE, rev)
    }

    pub fn set_good(&self, rev: &Revision) -> Result<(), SupervisorError> {
        self.write(GOOD, rev)
    }
}

/// Make a folder only its owner may enter, mode 0700.
pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Write a file whole, mode 0600, then rename it into place and sync the
/// folder: a crash leaves the old file or the new one, never half of one.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let tmp = path.with_extension("tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    if let Some(dir) = path.parent() {
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn revision_one_takes_only_the_deployments_variables() {
        let e = env(&[
            ("ORDO_PG_HOST", "db"),
            ("ORDO_PG_PASSWORD_FILE", "/run/secrets/pg"),
            ("ORDO_TOKEN_KEY", "aa"),
            ("ORDO_CONSOLE_ORIGIN", "http://localhost:8081"),
            ("ORDO_IDENTITY_DATABASE_URL", "postgres://x"),
            ("ORDO_SUPERVISOR_LISTEN", "127.0.0.1:8079"),
            ("PATH", "/bin"),
        ]);
        let r = Revision::from_container(&e, 7);
        assert_eq!(r.number, 1);
        assert_eq!(r.origin, Origin::Container);
        assert_eq!(
            r.spec.settings.keys().collect::<Vec<_>>(),
            vec!["ORDO_PG_HOST"]
        );
        assert_eq!(
            r.spec.secrets.get("ORDO_PG_PASSWORD"),
            Some(&Secret::File {
                path: "/run/secrets/pg".into(),
                changed_at: 7
            })
        );
        assert!(matches!(
            r.spec.secrets.get("ORDO_TOKEN_KEY"),
            Some(Secret::Value { .. })
        ));
        assert!(!r.public_json().to_string().contains("\"aa\""));
        assert!(r.spec.ignored_container_vars(&e[..3]).is_empty());
    }

    #[test]
    fn an_apply_keeps_resolves_and_refuses_naming_where_it_goes() {
        let live =
            Revision::from_container(&env(&[("ORDO_PG_HOST", "db"), ("ORDO_TOKEN_KEY", "aa")]), 7);
        let mut req = ApplyRequest {
            revision: 2,
            actor: "p_ada".into(),
            reason: "more workers".into(),
            settings: [("ORDO_PG_HOST".to_string(), "db2".to_string())].into(),
            secrets: [(
                "ORDO_TOKEN_KEY".to_string(),
                SecretInput {
                    keep: true,
                    ..SecretInput::default()
                },
            )]
            .into(),
        };
        let r = req.resolve(&live, &[(8081, "the console")], 9).unwrap();
        assert_eq!(r.spec.secrets, live.spec.secrets);
        assert_eq!(diff_summary(&live.spec, &r.spec), "changed=ORDO_PG_HOST");

        req.settings
            .insert(HOST_ADDR_ENV.into(), "0.0.0.0:8081".into());
        let e = req.resolve(&live, &[(8081, "the console")], 9).unwrap_err();
        assert!(e.to_string().contains("the console"), "{e}");
        req.settings.remove(HOST_ADDR_ENV);

        for bad in [
            "ORDO_PG_PASSWORD",
            "ORDO_CONSOLE_MODE",
            REVISION_ENV,
            "x",
            // Review, step 7: a compromised console must not reach the
            // dynamic loader or the identity wiring.
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "PATH",
            "ORDO_",
            "ORDO_IDENTITY_URL",
            "ORDO_IDENTITY_PLAIN_HTTP",
            "ORDO_IDENTITY_DEPLOYMENT",
            "ORDO_IDENTITY_ORIGIN",
        ] {
            let mut r2 = req.clone();
            r2.settings.insert(bad.into(), "v".into());
            assert!(r2.resolve(&live, &[], 9).is_err(), "{bad}");
        }
        for bad in ["LD_PRELOAD", "ORDO_IDENTITY_URL"] {
            let mut r2 = req.clone();
            r2.secrets.insert(
                bad.into(),
                SecretInput {
                    value: Some("/tmp/x.so".into()),
                    ..SecretInput::default()
                },
            );
            assert!(r2.resolve(&live, &[], 9).is_err(), "secret {bad}");
        }
        let mut r3 = req.clone();
        r3.secrets.insert(
            "ORDO_PAGE_KEY".into(),
            SecretInput {
                keep: true,
                ..SecretInput::default()
            },
        );
        assert!(r3.resolve(&live, &[], 9).is_err());
    }

    #[test]
    fn the_store_seals_and_a_wrong_key_names_the_setting() {
        let dir = std::env::temp_dir().join(format!("ordo-sup-spec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let key = dir.join("k");
        let store = SpecStore::open(&dir.join("state"), &key).unwrap();
        assert!(store.live().unwrap().is_none());
        let r = Revision::from_container(&env(&[("ORDO_TOKEN_KEY", "very-secret")]), 7);
        store.set_live(&r).unwrap();
        let raw = std::fs::read_to_string(dir.join("state/deployment-live.sealed")).unwrap();
        assert!(!raw.contains("very-secret"));
        assert_eq!(store.live().unwrap(), Some(r));
        std::fs::write(&key, "00".repeat(32)).unwrap();
        let again = SpecStore::open(&dir.join("state"), &key).unwrap();
        let e = again.live().unwrap_err();
        assert_eq!(e.setting_name(), Some(KEY_FILE_ENV));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
