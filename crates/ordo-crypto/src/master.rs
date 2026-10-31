//! Where the deployment master key comes from.
//!
//! `LocalKeyProvider::new(secret, store)` already takes bytes, so nothing
//! below is a fifth implementation of a fifteen-method trait: it is a
//! **source** for those bytes. That is the whole reason R2b stayed small.
//!
//! # What `ORDO_MASTER_KEY` costs, stated once
//!
//! [`crate::LocalKeyProvider::from_env`] is not a stub and is not deleted —
//! a built-in provider has to exist so small deployments are not gated on a
//! KMS, and it is a first-class implementation that does not pretend: it
//! cannot protect keys from someone holding the machine. What is refused is
//! **the environment as the only way to supply the master key**, for five
//! mechanical reasons:
//!
//! 1. `/proc/<pid>/environ` — any process running as the same uid reads it,
//!    which in a container is every sidecar sharing the process namespace.
//! 2. `docker inspect` prints it, as does every orchestrator's pod
//!    description, and both are readable by roles never granted key access.
//! 3. Crash reporters dump the environment, and a core dump of the serving
//!    process is a file somebody puts in a ticket.
//! 4. Children inherit it. The operator tool forks `psql`, so the
//!    deployment master key sits in the environment of every `psql` process
//!    an `ordo apply` or `ordo prune` spawns.
//! 5. **The stretch is one HMAC.** `LocalKeyProvider::new` computes
//!    `hmac_sha256(b"ordo.master.v1", secret)` and `from_env` enforces only
//!    a sixteen-character floor. A sixteen-character passphrase behind one
//!    HMAC is a sixteen-character passphrase.
//!
//! (5) is what decides the design here, and the workspace already had the
//! answer: argon2id at the OWASP profile `ordo-identity` has pinned as
//! constants since the password provider landed.
//!
//! # The file format
//!
//! ```text
//! "ordo-master-v1" \n key_id \n salt(16) ‖ seal_with(kek, 0, "master", key_id, master32)
//! ```
//!
//! where `kek = argon2id(passphrase, salt)` at the pinned profile.
//! `seal_with`/`open_with` are this crate's own, and the wrap/unwrap pattern
//! is exactly what `LocalKeyProvider::wrap` already does for SMKs and DEKs.
//! **No new crypto is designed here, and that is the point:** a key file
//! whose format was invented for the occasion is a key file nobody can
//! audit.
//!
//! `key_id` is the envelope's AAD field, so a file whose id was edited does
//! not open — the id is what tells two master keys apart in a log, and an id
//! that could be changed without invalidating the envelope would be a label
//! rather than a binding.
//!
//! # Why the file source ships before any KMS
//!
//! - **A KMS at boot makes a KMS outage a boot outage**, and boot is exactly
//!   when a fleet is rolling. This needs no network, and the platform's
//!   stated dependency floor is that Postgres is the only stateful
//!   dependency that must exist.
//! - **It is testable in CI without a secret** — a file and a passphrase the
//!   test writes, against a cloud account, a credential and a rotation
//!   obligation.
//! - **Self-host is first-class by ruling**, and a custody story whose only
//!   non-plaintext option is a cloud KMS makes it second-class.
//! - **It is the same file a KMS source later opens.** Moving to a KMS is
//!   then a provider swap plus a re-wrap, never a migration, because the
//!   envelope, the store, the epochs and every digest are unchanged.
//!
//! And the disclosure a KMS envelope will owe when it arrives, written now
//! so it is not written later by somebody with a reason to soften it: the
//! KMS would wrap the *master*, the master is unwrapped once at boot and
//! stays resident in process memory, and per-space keys are unwrapped from
//! the store as needed. A KMS-*native* provider would mean one network call
//! per sealed field per row, which is not affordable at the write path's
//! rate. **A KMS here buys "the key is not on the disk and not in the
//! environment"; it does not buy "the key never exists in this host's RAM."**

use std::path::{Path, PathBuf};

use ordo_fault::FaultClass;

use crate::custody::KeyError;

/// The format tag. Frozen: changing it is a new format, not a new version
/// of this one.
pub const MASTER_FILE_TAG: &str = "ordo-master-v1";

/// The AAD entity every master envelope is bound to.
const MASTER_ENTITY: &str = "master";

const SALT_LEN: usize = 16;

/// The OWASP argon2id profile.
///
/// `ordo-identity` pins the same three numbers for password hashing and
/// depends on no sibling crate; `ordo-crypto` depends on none either, and
/// making one depend on the other to share three integers would buy a
/// coupling for nothing. They are equal because they are the same published
/// recommendation, not because one is derived from the other.
pub mod profile {
    /// Memory, in KiB.
    pub const M_COST_KIB: u32 = 19_456;
    /// Iterations.
    pub const T_COST: u32 = 2;
    /// Lanes.
    pub const P_COST: u32 = 1;
}

/// The variable that marks a process as development. It is the flag the
/// server already had for `ORDO_KEY_PROVIDER=test`, and it gates the
/// `local` provider too (S6-009, owner ruling 2026-09-28): one signal for
/// "this deployment holds nothing worth protecting", not two.
pub const DEV_FLAG: &str = "ORDO_DEV_UNSAFE_TEST_KEYS";

/// Whether [`DEV_FLAG`] is set to a true value in this process.
pub fn dev_unsafe_from_env() -> bool {
    matches!(std::env::var(DEV_FLAG).as_deref(), Ok("1" | "true" | "yes"))
}

/// The refusal for the env master outside development, stated once so the
/// server and the `ordo keys` commands say the same thing.
///
/// The env source stretches the secret with one HMAC, so anyone holding a
/// database backup (which carries the wrapped keys by design) can guess a
/// short secret offline. The sealed file uses argon2id and has no such
/// weakness.
pub fn env_master_refusal() -> String {
    format!(
        "the env master key (ORDO_MASTER_KEY) is stretched with one HMAC, so a short secret \
         can be guessed offline from a database backup, and it is refused outside \
         development. Use `sealed-master`: run `ordo keys seal-master` to write the file, \
         then set ORDO_KEY_PROVIDER=sealed-master, ORDO_MASTER_KEY_FILE and \
         ORDO_MASTER_KEY_PASSPHRASE_FILE (the passphrase file in the secret store). For \
         development only, {DEV_FLAG}=1 allows it"
    )
}

/// Where the master key comes from.
pub enum MasterKeySource {
    /// `ORDO_MASTER_KEY`. Kept, first-class for a laptop and a small
    /// self-host, and printed as `local (env)` in the startup log so the
    /// posture is never invisible.
    Env,
    /// `ORDO_MASTER_KEY_FILE` + `ORDO_MASTER_KEY_PASSPHRASE_FILE`.
    ///
    /// **Two things, deliberately separable.** The file may live on the
    /// volume or in the image; the passphrase belongs in the orchestrator's
    /// secret store. Neither alone opens anything, which is what makes it
    /// safe for the file to travel with the deployment.
    SealedFile { path: PathBuf, passphrase: PathBuf },
}

impl MasterKeySource {
    /// The 32 bytes, however they were held.
    pub fn open(&self) -> Result<[u8; 32], KeyError> {
        match self {
            MasterKeySource::Env => {
                // `ORDO_MASTER_KEY`, or `ORDO_MASTER_KEY_FILE` naming a
                // file that holds the same secret (`ordo_env::secret`): the
                // form the console's supervisor passes. Under
                // `sealed-master` the same variable names the sealed file;
                // either way it names the file the master key comes from.
                let secret = ordo_env::secret("ORDO_MASTER_KEY")
                    .map_err(|e| KeyError::refused(e.to_string()))?
                    .ok_or_else(|| KeyError::refused(crate::MASTER_KEY_UNSET))?;
                if secret.len() < 16 {
                    return Err(KeyError::refused(crate::MASTER_KEY_SHORT));
                }
                Ok(derive_from_secret(secret.as_bytes()))
            }
            MasterKeySource::SealedFile { path, passphrase } => {
                let pass = read_passphrase_file(passphrase)?;
                open_sealed_master(path, pass.as_bytes())
            }
        }
    }

    /// What the startup log prints. A kind, never material.
    pub fn label(&self) -> &'static str {
        match self {
            MasterKeySource::Env => "local (env master key)",
            MasterKeySource::SealedFile { .. } => "sealed-master (file)",
        }
    }
}

/// Stretch a passphrase-shaped secret into the deployment master key.
///
/// **One HMAC, and that is the disclosure.** This is what
/// [`crate::LocalKeyProvider::new`] has always done — it is named here so
/// the two spellings cannot drift, and so the sealed-file source can be
/// clear that it does something *different*: a sealed file's 32 bytes are
/// already the master and must be handed to
/// [`crate::LocalKeyProvider::from_master`], not run through this again.
/// Hashing them twice would produce a provider that opens nothing an
/// `ORDO_MASTER_KEY` deployment wrote, with no error until the first read.
pub fn derive_from_secret(secret: &[u8]) -> [u8; 32] {
    crate::hmac_sha256(b"ordo.master.v1", secret)
}

/// A passphrase file, with its trailing newline removed.
///
/// Trailing newline stripped because every editor and every `echo` adds one,
/// and a passphrase that silently differs from what the operator typed is an
/// unopenable file with no diagnosis. Leading whitespace is **kept**: it is
/// a passphrase, and trimming both ends would quietly narrow the space of
/// passphrases somebody can choose.
fn read_passphrase_file(path: &Path) -> Result<String, KeyError> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        KeyError::caused_by(
            FaultClass::Broken,
            format!("passphrase file {}: {e}", path.display()),
            e,
        )
    })?;
    let pass = raw.trim_end_matches(['\n', '\r']).to_string();
    if pass.is_empty() {
        return Err(KeyError::refused(format!(
            "passphrase file {} is empty",
            path.display()
        )));
    }
    Ok(pass)
}

/// The key-encryption key: argon2id over the passphrase and the file's own
/// salt, at the pinned profile.
///
/// **The profile is not in the file.** Reading cost parameters out of a file
/// the attacker also holds means an attacker who wants a cheap cracking
/// target can hand you one. A future profile change is a new format tag,
/// which is a re-wrap, which is one command.
fn kek(passphrase: &[u8], salt: &[u8]) -> Result<[u8; 32], KeyError> {
    use argon2::{Algorithm, Argon2, Params, Version};
    let params = Params::new(
        profile::M_COST_KIB,
        profile::T_COST,
        profile::P_COST,
        Some(32),
    )
    .map_err(|e| KeyError::broken(format!("argon2 params: {e}")))?;
    let a2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; 32];
    a2.hash_password_into(passphrase, salt, &mut out)
        .map_err(|e| KeyError::broken(format!("argon2: {e}")))?;
    Ok(out)
}

/// Mint a fresh master key and write the sealed file.
///
/// Returns the **fingerprint** — `sha256("ordo.master.fp.v1" ‖ master)[..8]`,
/// hex — which is enough to tell two master keys apart in a log and useless
/// for recovering either. The master itself is returned to nobody and
/// written nowhere but the envelope.
///
/// Refuses to overwrite: a ceremony that clobbers the previous master is a
/// ceremony that erases every space's custody, and `create_new` makes that
/// one syscall rather than a check-then-write window.
pub fn seal_master(path: &Path, key_id: &str, passphrase: &[u8]) -> Result<String, KeyError> {
    let master = random32();
    let bytes = encode_sealed_master(key_id, &master, passphrase)?;
    write_new(path, &bytes)?;
    Ok(fingerprint(&master))
}

/// The file's bytes, for a caller that writes them itself — a re-wrap, which
/// must not leave a half-written file where the old one was.
pub fn encode_sealed_master(
    key_id: &str,
    master: &[u8; 32],
    passphrase: &[u8],
) -> Result<Vec<u8>, KeyError> {
    if key_id.is_empty() || key_id.contains('\n') || key_id.len() > 128 {
        return Err(KeyError::refused(
            "a master key id is 1..=128 bytes and contains no newline",
        ));
    }
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&random32()[..SALT_LEN]);
    let kek = kek(passphrase, &salt)?;
    // `key_id` as the AAD field: an id edited in the file does not open.
    let envelope = crate::seal_with(&kek, 0, MASTER_ENTITY, key_id, master);
    let mut out = Vec::with_capacity(64 + envelope.len());
    out.extend_from_slice(MASTER_FILE_TAG.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(key_id.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(&salt);
    out.extend_from_slice(&envelope);
    Ok(out)
}

/// Read a sealed master file and open it.
///
/// **A wrong passphrase is `KeyError::Seal` and says nothing else** — no
/// "the salt looked wrong", no byte position, no distinction from a
/// corrupted envelope. That is `SealError`'s existing posture: which byte
/// failed is an oracle, so every bad envelope is the same error.
pub fn open_sealed_master(path: &Path, passphrase: &[u8]) -> Result<[u8; 32], KeyError> {
    let raw = std::fs::read(path).map_err(|e| {
        KeyError::caused_by(
            FaultClass::Broken,
            format!("master key file {}: {e}", path.display()),
            e,
        )
    })?;
    let (key_id, salt, envelope) = parse_sealed_master(&raw)?;
    let kek = kek(passphrase, &salt)?;
    let plain = crate::open_with(&kek, MASTER_ENTITY, &key_id, envelope)?;
    if plain.len() != 32 {
        return Err(KeyError::Seal);
    }
    let mut master = [0u8; 32];
    master.copy_from_slice(&plain);
    Ok(master)
}

/// The key id a file carries, without opening it.
///
/// For a log line and for `rewrap-master`. The id is not secret — it exists
/// to be printed.
pub fn key_id_of(path: &Path) -> Result<String, KeyError> {
    let raw = std::fs::read(path).map_err(|e| {
        KeyError::caused_by(
            FaultClass::Broken,
            format!("master key file {}: {e}", path.display()),
            e,
        )
    })?;
    Ok(parse_sealed_master(&raw)?.0)
}

/// A public-safe fingerprint: `sha256("ordo.master.fp.v1" ‖ master)[..8]`.
///
/// Eight bytes, hex. Enough to tell two master keys apart in a log or in a
/// ticket; useless for recovering either. Not the key id — the id is what an
/// operator chose, the fingerprint is what the bytes are, and a re-wrap
/// keeps the second while it may change the first.
pub fn fingerprint(master: &[u8; 32]) -> String {
    let d = crate::hmac_sha256(b"ordo.master.fp.v1", master);
    crate::hex(&d[..8])
}

type Parsed<'a> = (String, [u8; SALT_LEN], &'a [u8]);

fn parse_sealed_master(raw: &[u8]) -> Result<Parsed<'_>, KeyError> {
    let malformed = || KeyError::refused(format!("not an {MASTER_FILE_TAG} file"));
    let nl1 = raw.iter().position(|b| *b == b'\n').ok_or_else(malformed)?;
    if &raw[..nl1] != MASTER_FILE_TAG.as_bytes() {
        return Err(malformed());
    }
    let rest = &raw[nl1 + 1..];
    let nl2 = rest
        .iter()
        .position(|b| *b == b'\n')
        .ok_or_else(malformed)?;
    let key_id = std::str::from_utf8(&rest[..nl2])
        .map_err(|_| malformed())?
        .to_string();
    let body = &rest[nl2 + 1..];
    if body.len() < SALT_LEN + 1 {
        return Err(malformed());
    }
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&body[..SALT_LEN]);
    Ok((key_id, salt, &body[SALT_LEN..]))
}

/// Write, refusing an existing file, owner-only.
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), KeyError> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // 0600 at creation, not chmod-after: a file that is world-readable
        // for the microseconds between the two is a file that was
        // world-readable.
        opts.mode(0o600);
    }
    let mut f = opts.open(path).map_err(|e| {
        KeyError::caused_by(FaultClass::Broken, format!(
            "{}: {e} (a master key file is never overwritten — it would erase every space's custody)",
            path.display()
        ), e)
    })?;
    f.write_all(bytes).map_err(|e| {
        KeyError::caused_by(FaultClass::Broken, format!("{}: {e}", path.display()), e)
    })?;
    f.sync_all()
        .map_err(|e| KeyError::caused_by(FaultClass::Broken, format!("{}: {e}", path.display()), e))
}

fn random32() -> [u8; 32] {
    use chacha20poly1305::aead::{AeadCore, OsRng};
    use chacha20poly1305::XChaCha20Poly1305;
    let a = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let b = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let mut out = [0u8; 32];
    out[..24].copy_from_slice(&a);
    out[24..].copy_from_slice(&b[..8]);
    out
}
