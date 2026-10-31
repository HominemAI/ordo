//! `ORDO_TENANT_CREDENTIALS`: the credential provider `$spacedb` and
//! `$spacebucket` labels both resolve through.
//!
//! `plan-space-routing.md` §9.1 R-C3 named this provider `ORDO_DB_CREDENTIALS`
//! and `plan-tenant-storage.md` §3 B-K1 renamed it `ORDO_TENANT_CREDENTIALS`,
//! arguing that object storage takes the *same* provider as the database
//! rather than one of its own: two providers would be two rotation
//! procedures, two boot refusals and two label lists on one node, and
//! §9.7 C-M3 needs one list a fleet-wide move can check before it copies
//! anything. The later name is the one this module and every call site use;
//! `ORDO_DB_CREDENTIALS` never shipped.
//!
//! # One directory, two shapes
//!
//! [`Dir`] reads one mounted directory once, at boot. Each entry names a
//! label:
//!
//! - **A file** is a Postgres password. Its trimmed contents are the
//!   password, and an **empty file is passwordless** (peer, trust or
//!   certificate authentication), which is the `Ok(None)` [`crate::route::
//!   DbCredentials::password`] already carries for exactly that case.
//! - **A directory** is an S3 key pair: it holds exactly two files,
//!   `access_key_id` and `secret_access_key`. Two files rather than one
//!   two-line one, for `sealed-master`'s stated reason that the pieces are
//!   kept separable, and because that is the layout a Kubernetes secret
//!   mounts by key.
//!
//! A label can only be one shape, because a file and a directory cannot
//! share one name in one directory, so the filesystem enforces the "flat,
//! shared namespace" B-K1 asks for without this code doing anything extra.
//!
//! # "Empty" means opposite things on purpose
//!
//! **A Postgres label with empty material authenticates; an S3 label with
//! empty material refuses.** This looks inconsistent and is not:
//! `PoolConfig`'s password field already treats "nothing configured" as
//! "ask the server to authenticate some other way", because peer, trust and
//! certificate authentication are all legitimate and all look like "no
//! password" from here. There is no equivalent legitimate reading on the
//! S3 side. `ordo_blob::s3::S3Config::new` already refuses an empty
//! `access_key_id` or `secret_access_key` rather than attempting anonymous
//! access, and says why: *"anonymous access to a bucket holding customer
//! content is not a configuration this refuses to guess at."* An empty key
//! half here is refused at boot for the identical reason, before any
//! request has a chance to silently downgrade a tenant's bucket to public.
//! The asymmetry is the point: one shape has a safe empty reading and the
//! other does not, and treating them the same would either refuse a
//! passwordless Postgres cluster that has always worked, or let a bucket
//! quietly go anonymous.
//!
//! # Refused at boot, not at first use
//!
//! The whole directory is read once, here, rather than lazily per label.
//! `plan-tenant-storage.md` B-K3: a bucket credential that is wrong should
//! cost milliseconds at boot rather than a tenant's first upload, which has
//! already consumed the client's bytes by the time it fails. The database
//! side inherits the same argument from R-C3.
//!
//! # Mode: refuse group- or world-readable, on every label's own material
//!
//! Every regular file is refused if its mode has any of `0o077` set,
//! matching the `0o600` the master key is written with
//! (`ordo_crypto::master::write_new`). **An S3 label's own directory is
//! checked the same way, not only the two files inside it**: a
//! world-readable label directory holding two mode-0600 files still lets
//! another local user (or process) add or replace entries in it regardless
//! of what mode the files themselves carry, because directory permissions
//! govern the directory's own contents rather than the files' declared
//! mode. `0o077` is checked on the *resolved* file (see below), because a
//! symlink's own mode bits are meaningless on Linux, where the kernel
//! ignores them, so checking them would check nothing.
//!
//! # Symlinks are followed, not refused, and then checked for escape
//!
//! The plans do not say what a symlink inside the directory should do, so
//! this is a decision rather than an inherited one. **A symlink is
//! followed rather than refused outright**, because refusing every symlink
//! would refuse the primary deployment shape R-C3 was written for: a
//! Kubernetes Secret or ConfigMap volume publishes each key as a symlink
//! into a versioned `..data` directory and swaps it atomically on update,
//! and Vault Agent's template sidecar does the same. A provider that
//! refused to boot on a Kubernetes cluster because its own secret mount is
//! made of symlinks would be a provider nobody could actually use in the
//! deployment shape this design targets.
//!
//! What *is* refused is a symlink whose resolved target lands **outside**
//! the credential directory: this code canonicalizes the directory once at
//! open and canonicalizes every entry's resolved path, refusing any whose
//! canonical path does not start with the directory's own. That is the
//! actual risk a symlink poses here: a way to make this process read a
//! file the operator did not mount as credential material, such as
//! `/etc/shadow` or another tenant's key file elsewhere on the same
//! filesystem. Canonicalizing both sides catches it regardless of how many
//! symlinks are chained, while leaving an ordinary atomic-swap mount
//! untouched, because every hop of that swap still resolves to a path
//! under the mounted directory.
//!
//! This is a boot-time check, not a continuously enforced invariant: the
//! target could change between the canonicalize and the read that follows
//! it, the same TOCTOU margin every other file this codebase reads
//! (`ordo_crypto::master`'s sealed file included) already lives with. What
//! it buys is exactly what boot-time checks buy elsewhere in this
//! codebase: a misconfigured or tampered mount costs a refused boot rather
//! than a quietly wrong answer served for the life of the process.
//!
//! # Trailing newlines are trimmed, nothing else is
//!
//! A credential file written by `echo "secret" > file` ends in `\n`, and a
//! provider that treated that byte as part of the password would
//! authenticate against a string nobody typed. `PoolConfig::from_env`
//! already makes this call for `ORDO_PG_PASSWORD_FILE`: `s.trim_end_
//! matches(['\n', '\r'])`, trailing newline and carriage return only,
//! nothing else. This module uses the identical trim for every file it
//! reads, database password and S3 key halves alike, so the two shapes
//! agree on what "the file's contents" means. Leading or interior
//! whitespace is left alone: a password or a key half is not text a human
//! is expected to have padded, and trimming more than the one byte a shell
//! redirection adds would silently change a credential nobody asked to
//! have changed.
//!
//! # Values never leave this module
//!
//! [`Dir`]'s hand-written [`std::fmt::Debug`] prints the labels it holds
//! and nothing else, the same posture `PoolConfig` and `ordo_blob::s3::
//! S3Config` already take for their one secret field each. Every error
//! this module returns names a label, never a value and never a path
//! inside a label's material, so a `{:?}`, a log line, a panic message or
//! an error chain that formats one of these can carry a label but never a
//! credential.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::route::DbCredentials;

/// The two file names an S3 label's directory must hold, and nothing else.
const ACCESS_KEY_FILE: &str = "access_key_id";
const SECRET_KEY_FILE: &str = "secret_access_key";

/// A label resolves to an S3 key pair.
///
/// Kept apart from [`DbCredentials`] rather than merged into it: the two
/// traits answer different questions (a password versus a key pair) with
/// different shapes, and every existing caller of `DbCredentials`
/// (`AttachedPools`, `directory::routed_engine`, `provision::prepare_target`)
/// takes `dyn DbCredentials` and has no reason to grow a method it never
/// calls. [`Dir`] implements both, which is what makes it "one provider":
/// one directory, one boot read, one label namespace, answering two
/// questions through two traits over the same data.
pub trait BucketCredentials: Send + Sync {
    /// `(access_key_id, secret_access_key)` for `label`.
    ///
    /// `Err` for a label this node holds no bucket material under, whether
    /// nothing is installed under that name or it names a Postgres
    /// password instead, and the two are one answer for the same
    /// reason `BucketOps::attach`'s own refusal collapses "not installed"
    /// and "not for you": what this node holds is deployment
    /// configuration, not something a caller's guess should be able to
    /// probe.
    fn key_pair(&self, label: &str) -> Result<(String, String), String>;
}

/// The `dir` provider: every label this node holds, read once from one
/// mounted directory at boot. See the module doc for the shape, the
/// asymmetry, the mode and symlink rules, and the newline trim.
pub struct Dir {
    passwords: HashMap<String, Option<String>>,
    key_pairs: HashMap<String, (String, String)>,
}

impl Dir {
    /// Read `path` and build the provider, or refuse.
    ///
    /// Every refusal names the label (or, for a problem with the directory
    /// itself, the path an operator configured) and never any material.
    pub fn open(path: &Path) -> Result<Dir, String> {
        let root = fs::canonicalize(path).map_err(|e| {
            format!(
                "{}: {e} (ORDO_TENANT_CREDENTIALS_DIR must name a directory this process can \
                 read)",
                path.display()
            )
        })?;
        if !root.is_dir() {
            return Err(format!("{}: not a directory", root.display()));
        }
        // The mount itself is not mode-checked, only what is under it: an
        // operator's mount point (a Kubernetes secret volume, a Docker
        // secret directory) is commonly `0755` for traversal, and that is
        // fine, because it holds no material directly: the labels are
        // file *names*, already the intended disclosure of `/health/ready`
        // (§9.7 C-M3), and nothing at this level is a value. What must be
        // `0o600`-shaped is the material one level down: a label's file, or
        // an S3 label's own subdirectory (checked below on each entry).

        let mut passwords = HashMap::new();
        let mut key_pairs = HashMap::new();

        let entries = fs::read_dir(&root).map_err(|e| format!("{}: {e}", root.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("{}: {e}", root.display()))?;
            let name = entry.file_name();
            let label = name
                .to_str()
                .ok_or_else(|| "a credential entry's name is not valid UTF-8".to_string())?
                .to_string();

            let resolved = resolve_within(&root, &entry.path(), &label)?;
            let meta = fs::metadata(&resolved).map_err(|e| format!("label `{label}`: {e}"))?;

            if meta.is_file() {
                check_mode(&resolved, &format!("label `{label}`"))?;
                let raw = fs::read(&resolved).map_err(|e| format!("label `{label}`: {e}"))?;
                let text = String::from_utf8(raw)
                    .map_err(|_| format!("label `{label}`: credential file is not valid UTF-8"))?;
                let trimmed = trim_newline(&text);
                passwords.insert(
                    label,
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed)
                    },
                );
            } else if meta.is_dir() {
                check_mode(&resolved, &format!("label `{label}`"))?;
                let pair = read_key_pair(&resolved, &label)?;
                key_pairs.insert(label, pair);
            } else {
                return Err(format!(
                    "label `{label}`: neither a file nor a directory, refused rather than \
                     guessed at"
                ));
            }
        }

        Ok(Dir {
            passwords,
            key_pairs,
        })
    }

    /// Every label this node holds, names only, never a value. What
    /// `/health/ready` prints for §9.7 C-M3, so a fleet-wide move can check
    /// every node's labels before it copies anything.
    pub fn labels(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .passwords
            .keys()
            .chain(self.key_pairs.keys())
            .cloned()
            .collect();
        out.sort();
        out
    }
}

/// Written by hand so a `{:?}` (a panic message, a `dbg!`, a log line)
/// carries the labels this node holds and never the material behind them.
/// The same posture `PoolConfig` and `ordo_blob::s3::S3Config` take for
/// their one secret field each; this type has a map of them; the answer is
/// unchanged either way.
impl std::fmt::Debug for Dir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dir")
            .field("labels", &self.labels())
            .finish()
    }
}

impl DbCredentials for Dir {
    fn password(&self, label: &str) -> Result<Option<String>, String> {
        self.passwords.get(label).cloned().ok_or_else(|| {
            format!("no database credential is installed under `{label}` on this node")
        })
    }
}

impl BucketCredentials for Dir {
    fn key_pair(&self, label: &str) -> Result<(String, String), String> {
        self.key_pairs.get(label).cloned().ok_or_else(|| {
            format!("no bucket credential is installed under `{label}` on this node")
        })
    }
}

/// Read an S3 label's directory: exactly `access_key_id` and
/// `secret_access_key`, nothing more and nothing fewer, neither empty.
///
/// Strict about the directory's contents: a third file or a missing half
/// refuses rather than being ignored, for the same "cost milliseconds at
/// boot" reason the mode check exists. A directory an operator half-wrote
/// should be caught here, not discovered as a resolve failure at the first
/// upload it was never going to serve correctly anyway.
fn read_key_pair(dir: &Path, label: &str) -> Result<(String, String), String> {
    let mut seen: Vec<String> = Vec::new();
    let mut access_key_id: Option<String> = None;
    let mut secret_access_key: Option<String> = None;

    let entries = fs::read_dir(dir).map_err(|e| format!("label `{label}`: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("label `{label}`: {e}"))?;
        let name = entry
            .file_name()
            .to_str()
            .ok_or_else(|| format!("label `{label}`: a file name is not valid UTF-8"))?
            .to_string();
        if name != ACCESS_KEY_FILE && name != SECRET_KEY_FILE {
            return Err(format!(
                "label `{label}`: `{name}` is not part of a key pair (expected only \
                 `{ACCESS_KEY_FILE}` and `{SECRET_KEY_FILE}`)"
            ));
        }
        seen.push(name.clone());

        let resolved = resolve_within(dir, &entry.path(), label)?;
        let meta = fs::metadata(&resolved).map_err(|e| format!("label `{label}`: {e}"))?;
        if !meta.is_file() {
            return Err(format!("label `{label}`: `{name}` is not a regular file"));
        }
        check_mode(&resolved, &format!("label `{label}` (`{name}`)"))?;
        let raw = fs::read(&resolved).map_err(|e| format!("label `{label}`: {e}"))?;
        let text = String::from_utf8(raw)
            .map_err(|_| format!("label `{label}`: `{name}` is not valid UTF-8"))?;
        let trimmed = trim_newline(&text);
        // **Refused, never anonymous.** See the module doc's "'Empty' means
        // opposite things on purpose": the database shape's empty file is
        // a legitimate passwordless configuration, and there is no
        // equivalent legitimate reading here. `ordo_blob::s3::S3Config::
        // new` already refuses an empty half rather than attempting
        // anonymous access to a bucket holding customer content, and this
        // is the same refusal at the point the material is read, so a
        // deployment never gets as far as building an `S3Config` from a
        // half-installed label.
        if trimmed.is_empty() {
            return Err(format!(
                "label `{label}`: `{name}` is empty; an S3 credential half cannot be empty. \
                 Empty means passwordless for a database label and means refused for a bucket \
                 label, because there is no anonymous-access reading of it that is safe to guess"
            ));
        }
        match name.as_str() {
            ACCESS_KEY_FILE => access_key_id = Some(trimmed),
            SECRET_KEY_FILE => secret_access_key = Some(trimmed),
            _ => unreachable!("checked above"),
        }
    }

    match (access_key_id, secret_access_key) {
        (Some(a), Some(s)) => Ok((a, s)),
        _ => {
            let missing: Vec<&str> = [ACCESS_KEY_FILE, SECRET_KEY_FILE]
                .into_iter()
                .filter(|f| !seen.iter().any(|s| s == f))
                .collect();
            Err(format!(
                "label `{label}`: missing {}. An S3 label needs both halves, or neither file \
                 (which is not a bucket label at all)",
                missing.join(" and ")
            ))
        }
    }
}

/// Canonicalize `entry_path` and refuse if it resolves outside `root`. See
/// the module doc's symlink section for why following is preferred to a
/// blanket refusal, and why an escape is refused regardless.
fn resolve_within(root: &Path, entry_path: &Path, label: &str) -> Result<PathBuf, String> {
    let resolved = fs::canonicalize(entry_path).map_err(|e| format!("label `{label}`: {e}"))?;
    if !resolved.starts_with(root) {
        return Err(format!(
            "label `{label}`: resolves outside the credential directory, refused whether that \
             is a symlink escaping the mount or something stranger"
        ));
    }
    Ok(resolved)
}

/// Refuse a mode with any of `0o077` set: group- or world-readable,
/// -writable or -executable. Matches the `0o600` the master key is written
/// with (`ordo_crypto::master::write_new`). A no-op on a non-Unix target,
/// the same split `write_new` itself takes, because there is no portable
/// mode to check.
fn check_mode(path: &Path, what: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::metadata(path).map_err(|e| format!("{what}: {e}"))?;
        let mode = meta.mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(format!(
                "{what}: mode {mode:04o} is group- or world-accessible, refused at boot, \
                 matching the 0600 the master key is written with"
            ));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, what);
    }
    Ok(())
}

/// Trim exactly the bytes a shell redirection adds, and nothing else. See
/// the module doc's "Trailing newlines" section; identical to
/// `PoolConfig::from_env`'s `ORDO_PG_PASSWORD_FILE` handling.
fn trim_newline(s: &str) -> String {
    s.trim_end_matches(['\n', '\r']).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::Permissions;
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn write_mode(path: &Path, contents: &str, mode: u32) {
        fs::write(path, contents).expect("write credential file");
        fs::set_permissions(path, Permissions::from_mode(mode)).expect("chmod");
    }

    /// A label the operator never installed refuses the moment it is
    /// asked for, with no I/O of its own, which is what lets a caller
    /// like `AttachedPools::target_config` refuse a target before it ever
    /// dials, rather than discovering the missing credential mid-connect.
    ///
    /// Falsified by first asserting on a provider built with the label
    /// present (`Ok`), confirming the assertion can fail, then removing it
    /// from the fixture.
    #[test]
    fn a_label_with_no_file_refuses_before_any_dial() {
        let tmp = tempdir();
        let dir = Dir::open(tmp.path()).expect("empty directory opens");
        let err = dir.password("db1").unwrap_err();
        assert!(err.contains("db1"));
        assert!(
            !err.contains("password"),
            "must not describe what db1 would have held"
        );
    }

    /// A password file that is group- or world-readable refuses the whole
    /// boot, before this node ever answers a request against the label it
    /// names.
    ///
    /// Falsified by first writing the same file at `0o600` and confirming
    /// `Dir::open` succeeds, so the refusal below is provably about the
    /// mode and not about the file's presence or content.
    #[test]
    fn a_world_readable_credential_file_refuses_at_boot() {
        let tmp = tempdir();
        let file = tmp.path().join("db1");

        write_mode(&file, "s3kr3t\n", 0o600);
        assert!(Dir::open(tmp.path()).is_ok(), "0600 must open cleanly");

        write_mode(&file, "s3kr3t\n", 0o644);
        let err = Dir::open(tmp.path()).unwrap_err();
        assert!(err.contains("db1"));
        assert!(!err.contains("s3kr3t"));
    }

    /// The directory holding an S3 label's two files is checked the same
    /// way as the files themselves (B-K3): a world-readable directory
    /// lets another local actor replace a 0600 file's contents regardless
    /// of the file's own mode.
    ///
    /// Falsified by first confirming `0o700` opens, so the refusal is
    /// provably about the directory's own mode and not about anything
    /// else in the fixture.
    #[test]
    fn a_world_readable_bucket_directory_refuses_at_boot_even_with_mode_0600_files() {
        let tmp = tempdir();
        let bucket_dir = tmp.path().join("s3-1");
        fs::create_dir(&bucket_dir).expect("mkdir");
        write_mode(&bucket_dir.join(ACCESS_KEY_FILE), "AKIA\n", 0o600);
        write_mode(&bucket_dir.join(SECRET_KEY_FILE), "shh\n", 0o600);
        fs::set_permissions(&bucket_dir, Permissions::from_mode(0o700)).expect("chmod dir");
        assert!(
            Dir::open(tmp.path()).is_ok(),
            "0700 dir with 0600 files must open"
        );

        fs::set_permissions(&bucket_dir, Permissions::from_mode(0o755)).expect("chmod dir");
        let err = Dir::open(tmp.path()).unwrap_err();
        assert!(err.contains("s3-1"));
        assert!(!err.contains("AKIA") && !err.contains("shh"));
    }

    /// An empty password file is a passwordless label (peer, trust or
    /// certificate authentication), which `DbCredentials::password`
    /// already expresses as `Ok(None)`.
    ///
    /// Falsified by first writing a non-empty file and asserting `Ok(Some
    /// (_))`, so the `Ok(None)` below is provably about emptiness and not
    /// about the label being unrecognized.
    #[test]
    fn an_empty_credential_file_is_passwordless() {
        let tmp = tempdir();
        write_mode(&tmp.path().join("db1"), "hunter2", 0o600);
        let dir = Dir::open(tmp.path()).unwrap();
        assert_eq!(dir.password("db1").unwrap(), Some("hunter2".to_string()));

        write_mode(&tmp.path().join("db2"), "", 0o600);
        let dir = Dir::open(tmp.path()).unwrap();
        assert_eq!(dir.password("db2").unwrap(), None);

        // A file holding only the newline `echo` adds is the same as an
        // empty file, per the module doc's trim rule.
        write_mode(&tmp.path().join("db3"), "\n", 0o600);
        let dir = Dir::open(tmp.path()).unwrap();
        assert_eq!(dir.password("db3").unwrap(), None);
    }

    /// A database label, a plain file, resolves to a password through
    /// `DbCredentials::password`.
    #[test]
    fn a_database_label_resolves_to_a_password() {
        let tmp = tempdir();
        write_mode(&tmp.path().join("prod-db"), "correct-horse\n", 0o600);
        let dir = Dir::open(tmp.path()).unwrap();
        assert_eq!(
            dir.password("prod-db").unwrap(),
            Some("correct-horse".to_string())
        );
    }

    /// A bucket label, a directory holding the two halves, resolves to a
    /// key pair through `BucketCredentials::key_pair`.
    #[test]
    fn a_bucket_label_resolves_to_a_key_pair() {
        let tmp = tempdir();
        let bucket_dir = tmp.path().join("prod-bucket");
        fs::create_dir(&bucket_dir).unwrap();
        write_mode(&bucket_dir.join(ACCESS_KEY_FILE), "AKIAEXAMPLE\n", 0o600);
        write_mode(&bucket_dir.join(SECRET_KEY_FILE), "wJalrXUtnFEMI\n", 0o600);
        fs::set_permissions(&bucket_dir, Permissions::from_mode(0o700)).unwrap();

        let dir = Dir::open(tmp.path()).unwrap();
        assert_eq!(
            dir.key_pair("prod-bucket").unwrap(),
            ("AKIAEXAMPLE".to_string(), "wJalrXUtnFEMI".to_string())
        );
    }

    /// A label with no material under it refuses `key_pair` the same way
    /// an unconfigured database label refuses `password`: before any S3
    /// request is attempted, not at the first upload.
    ///
    /// Falsified by first resolving a label that IS installed (`Ok`), then
    /// asking for one that never was.
    #[test]
    fn a_label_with_no_material_refuses_before_any_request() {
        let tmp = tempdir();
        let bucket_dir = tmp.path().join("real-bucket");
        fs::create_dir(&bucket_dir).unwrap();
        write_mode(&bucket_dir.join(ACCESS_KEY_FILE), "AKIA\n", 0o600);
        write_mode(&bucket_dir.join(SECRET_KEY_FILE), "shh\n", 0o600);
        fs::set_permissions(&bucket_dir, Permissions::from_mode(0o700)).unwrap();
        let dir = Dir::open(tmp.path()).unwrap();

        assert!(dir.key_pair("real-bucket").is_ok());
        let err = dir.key_pair("no-such-label").unwrap_err();
        assert!(err.contains("no-such-label"));

        // A database label is also "no material" from the bucket side.
        write_mode(&tmp.path().join("some-db"), "pw\n", 0o600);
        let dir = Dir::open(tmp.path()).unwrap();
        assert!(dir.key_pair("some-db").is_err());
    }

    /// The node reports the labels it holds, sorted and deduplicated
    /// across the two shapes, and `Debug` carries the same list and never
    /// a value. This is what `/health/ready` prints for §9.7 C-M3.
    #[test]
    fn the_health_surface_names_labels_and_never_values() {
        let tmp = tempdir();
        write_mode(&tmp.path().join("db1"), "s3kr3t-pw\n", 0o600);
        let bucket_dir = tmp.path().join("bucket1");
        fs::create_dir(&bucket_dir).unwrap();
        write_mode(&bucket_dir.join(ACCESS_KEY_FILE), "AKIA-VALUE\n", 0o600);
        write_mode(&bucket_dir.join(SECRET_KEY_FILE), "SECRET-VALUE\n", 0o600);
        fs::set_permissions(&bucket_dir, Permissions::from_mode(0o700)).unwrap();

        let dir = Dir::open(tmp.path()).unwrap();
        assert_eq!(dir.labels(), vec!["bucket1".to_string(), "db1".to_string()]);

        let printed = format!("{dir:?}");
        assert!(printed.contains("db1") && printed.contains("bucket1"));
        assert!(!printed.contains("s3kr3t-pw"));
        assert!(!printed.contains("AKIA-VALUE"));
        assert!(!printed.contains("SECRET-VALUE"));
    }

    /// An S3 half that is present but empty refuses at boot rather than
    /// being read as "no credential", which would let `S3Config::new`
    /// later refuse it as an anonymous binding, or worse not refuse it at
    /// all if that check were ever relaxed. Refusing here means a half-written
    /// label never becomes a provider answer at all.
    ///
    /// Falsified by first writing both halves non-empty (`Ok`), then
    /// emptying one, proving the refusal tracks emptiness and not the
    /// label or the other half.
    #[test]
    fn an_empty_access_key_refuses_rather_than_going_anonymous() {
        let tmp = tempdir();
        let bucket_dir = tmp.path().join("bucket1");
        fs::create_dir(&bucket_dir).unwrap();
        write_mode(&bucket_dir.join(ACCESS_KEY_FILE), "AKIA\n", 0o600);
        write_mode(&bucket_dir.join(SECRET_KEY_FILE), "shh\n", 0o600);
        fs::set_permissions(&bucket_dir, Permissions::from_mode(0o700)).unwrap();
        assert!(Dir::open(tmp.path()).is_ok());

        write_mode(&bucket_dir.join(ACCESS_KEY_FILE), "", 0o600);
        let err = Dir::open(tmp.path()).unwrap_err();
        assert!(err.contains("bucket1"));
        assert!(err.contains("empty"));
        assert!(!err.contains("shh"));

        // The other half, alone, is refused the same way.
        write_mode(&bucket_dir.join(ACCESS_KEY_FILE), "AKIA\n", 0o600);
        write_mode(&bucket_dir.join(SECRET_KEY_FILE), "", 0o600);
        let err = Dir::open(tmp.path()).unwrap_err();
        assert!(err.contains("bucket1"));
        assert!(!err.contains("AKIA"));
    }

    /// A symlink whose target resolves outside the credential directory
    /// refuses at boot: the module doc argues for following a symlink
    /// rather than blanket-refusing it (so a Kubernetes-style mount still
    /// works), but never allowing it to point this process at a file the
    /// operator did not mount as credential material.
    ///
    /// Falsified by first pointing the symlink at a label file *inside*
    /// the directory and confirming that opens and resolves normally
    /// (proving symlinks are not blanket-refused), then repointing it
    /// outside.
    #[test]
    fn a_symlink_that_escapes_the_credential_directory_refuses_at_boot() {
        let tmp = tempdir();
        let outside = tempdir();
        let real_secret = outside.path().join("real-secret");
        write_mode(&real_secret, "not-for-this-process\n", 0o600);

        // Inside the directory: a symlink to another label already inside
        // it, the Kubernetes-style `..data` pattern. This must work.
        write_mode(&tmp.path().join("db1"), "hunter2\n", 0o600);
        let inside_link = tmp.path().join("db2");
        symlink(tmp.path().join("db1"), &inside_link).expect("symlink");
        let dir = Dir::open(tmp.path()).expect("a symlink pointing inside the directory opens");
        assert_eq!(dir.password("db2").unwrap(), Some("hunter2".to_string()));

        // Outside the directory: refused.
        fs::remove_file(&inside_link).unwrap();
        symlink(&real_secret, &inside_link).expect("symlink outside");
        let err = Dir::open(tmp.path()).unwrap_err();
        assert!(err.contains("db2"));
        assert!(!err.contains("not-for-this-process"));
    }

    /// **A sibling directory whose name merely starts with the credential
    /// directory's is outside it**, and the containment check has to know
    /// that.
    ///
    /// This pins the one line the whole symlink argument rests on.
    /// `resolve_within` compares with `Path::starts_with`, which is
    /// component-wise: `/creds-evil` does not start with `/creds`. The
    /// same expression written over strings, which is the obvious
    /// "simplification" for somebody tidying this later, compares
    /// byte-wise and lets `/creds-evil/db1` through as though it were
    /// inside `/creds`. Nothing else in the file would notice, because
    /// every other test puts its escape somewhere with an unrelated name.
    #[test]
    fn a_sibling_directory_sharing_a_name_prefix_is_still_outside() {
        let tmp = tempdir();
        // A sibling of the credential directory whose path is a byte-wise
        // prefix match and a component-wise miss.
        let mut sibling = tmp.path().as_os_str().to_os_string();
        sibling.push("-evil");
        let sibling = std::path::PathBuf::from(sibling);
        fs::create_dir_all(&sibling).expect("the sibling");
        let planted = sibling.join("planted");
        write_mode(&planted, "material-from-next-door", 0o600);

        write_mode(&tmp.path().join("db1"), "hunter2", 0o600);
        symlink(&planted, tmp.path().join("db2")).expect("symlink to the sibling");

        let err = Dir::open(tmp.path()).unwrap_err();
        assert!(
            err.contains("db2") && err.contains("outside"),
            "a sibling directory sharing a name prefix was treated as inside: {err}"
        );
        assert!(
            !err.contains("material-from-next-door"),
            "the refusal carries the material it refused: {err}"
        );
        let _ = fs::remove_dir_all(&sibling);
    }

    fn tempdir() -> TempDir {
        TempDir::new()
    }

    /// A minimal owned temp directory, removed on drop. `ordo-exec` has no
    /// dev-dependency on a temp-file crate, and this module's tests are the
    /// first here to need one.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> TempDir {
            // pid + nanos alone collided under `--test-threads=4`: two
            // threads calling this within the same clock tick got the same
            // name and corrupted each other's fixtures. The counter is
            // what actually guarantees uniqueness within one process.
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut p = std::env::temp_dir();
            let unique = format!(
                "ordo-creds-test-{}-{}-{n}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            p.push(unique);
            fs::create_dir_all(&p).expect("create temp dir");
            TempDir(p)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
