//! The security stream's checkpoint key (`plan-identity-service.md`
//! section 7, board ask 4): made at first boot when
//! `ORDO_SECURITY_CHECKPOINT_KEY` is unset, instead of running with no
//! checkpoints.
//!
//! - **Set wins.** A set `ORDO_SECURITY_CHECKPOINT_KEY` (or its `_FILE`) is
//!   used as before, and nothing is made or read from the database. A set
//!   value that does not parse is still the refusal it was: the feature is
//!   off and the drain says why, and no key is made in its place.
//! - **Unset makes one, once.** 32 random bytes, sealed under the
//!   deployment's custody (the master key, through the system space's key)
//!   and stored in `$securitykey` in the control database. Every later boot,
//!   and every replica, opens the same row. Two replicas making one at the
//!   same moment both keep the row that landed first.
//! - **Published like a set key.** A verifier needs the key id and the
//!   public half, the pair `ORDO_SECURITY_CHECKPOINT_KEY_ID` and
//!   `ORDO_SECURITY_CHECKPOINT_PUBKEY` name. The boot prints that pair, and
//!   `/health/deep` carries it under `security_stream.checkpoint_key`.
//!
//! [`SecurityKeyError`] is the one error, and each variant says what the
//! caller can do.

use std::sync::{Arc, Mutex};

use ordo_commandlog::{Ed25519Signer, Signer, SignerError};
use ordo_crypto::{Anchor, KeyProvider, Purpose, SpaceId};
use ordo_exec::{sql_lit, Pool};
use ordo_identity::StoreFault;

const DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$securitykey" (
  "name"    text   PRIMARY KEY,
  "key_id"  text   NOT NULL,
  "sealed"  text   NOT NULL,
  "made_at" bigint NOT NULL
)"#;

/// The one row's name, and the anchor its seal is bound to.
const ROW: &str = "checkpoint";
const ENTITY: &str = "$securitykey";
const FIELD: &str = "seed";

/// Where the key came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// `ORDO_SECURITY_CHECKPOINT_KEY` was set.
    Set,
    /// Made at an earlier boot and opened from `$securitykey`.
    Stored,
    /// Made at this boot.
    Made,
}

impl KeySource {
    pub fn word(self) -> &'static str {
        match self {
            KeySource::Set => "set",
            KeySource::Stored => "made at first boot",
            KeySource::Made => "made at this boot",
        }
    }
}

/// Why there is no checkpoint key.
#[derive(Debug)]
#[non_exhaustive]
pub enum SecurityKeyError {
    /// `ORDO_SECURITY_CHECKPOINT_KEY` is set and does not parse, or its
    /// key id is missing. Fix the setting; no key is made in its place.
    Set(SignerError),
    /// `$securitykey` could not be read or written. Retry; the boot goes on
    /// without checkpoints and says so.
    Store(StoreFault),
    /// The custody would not seal a new seed or open the stored one (a
    /// master key other than the one it was sealed under). Restore that
    /// master key, or set `ORDO_SECURITY_CHECKPOINT_KEY`.
    Custody(&'static str),
}

impl std::fmt::Display for SecurityKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecurityKeyError::Set(e) => write!(f, "{e}"),
            SecurityKeyError::Store(e) => {
                write!(f, "the security checkpoint key could not be stored: {e}")
            }
            SecurityKeyError::Custody(why) => write!(
                f,
                "the security checkpoint key made at first boot {why}: restore the master \
                 key it was sealed under, or set {}",
                ordo_secevent::SECURITY_KEY_ENV
            ),
        }
    }
}

impl std::error::Error for SecurityKeyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SecurityKeyError::Set(e) => Some(e),
            SecurityKeyError::Store(e) => Some(e),
            SecurityKeyError::Custody(_) => None,
        }
    }
}

enum Backend {
    Pg(Pool),
    Memory(Mutex<Option<(String, String)>>),
}

/// `$securitykey` in the control database: the made key, sealed.
#[derive(Clone)]
pub struct SecurityKeyStore(Arc<Backend>);

fn fault(e: ordo_exec::PoolError) -> StoreFault {
    crate::auth::store_failure::pool_fault(
        e,
        "the security checkpoint key could not be read or written",
    )
}

impl SecurityKeyStore {
    /// Over the control database.
    pub fn pg(pool: Pool) -> SecurityKeyStore {
        SecurityKeyStore(Arc::new(Backend::Pg(pool)))
    }

    /// In memory. For tests.
    pub fn memory() -> SecurityKeyStore {
        SecurityKeyStore(Arc::new(Backend::Memory(Mutex::new(None))))
    }

    /// The stored `(key id, sealed seed hex)`.
    async fn load(&self) -> Result<Option<(String, String)>, StoreFault> {
        match self.0.as_ref() {
            Backend::Memory(m) => Ok(m.lock().expect("security key memory").clone()),
            Backend::Pg(pool) => {
                pool.rows(DDL).await.map_err(fault)?;
                let rows = pool
                    .rows(&format!(
                        r#"SELECT "key_id", "sealed" FROM "$securitykey" WHERE "name" = {}"#,
                        sql_lit(ROW)
                    ))
                    .await
                    .map_err(fault)?;
                Ok(rows.first().and_then(|r| {
                    Some((r.first().cloned().flatten()?, r.get(1).cloned().flatten()?))
                }))
            }
        }
    }

    /// Keep `(key_id, sealed)` unless a row is already there.
    async fn insert(&self, key_id: &str, sealed: &str, at: u64) -> Result<(), StoreFault> {
        match self.0.as_ref() {
            Backend::Memory(m) => {
                let mut m = m.lock().expect("security key memory");
                if m.is_none() {
                    *m = Some((key_id.to_string(), sealed.to_string()));
                }
                Ok(())
            }
            Backend::Pg(pool) => {
                pool.rows(&format!(
                    r#"INSERT INTO "$securitykey" ("name", "key_id", "sealed", "made_at")
                       VALUES ({n}, {k}, {s}, {at})
                       ON CONFLICT ("name") DO NOTHING"#,
                    n = sql_lit(ROW),
                    k = sql_lit(key_id),
                    s = sql_lit(sealed),
                ))
                .await
                .map_err(fault)?;
                Ok(())
            }
        }
    }
}

/// The resolved key and where it came from.
pub struct CheckpointKey {
    pub signer: Arc<Ed25519Signer>,
    pub source: KeySource,
}

impl CheckpointKey {
    /// The pair a verifier sets, as one line.
    pub fn published(&self) -> String {
        format!(
            "{}={} {}={}",
            ordo_secevent::SECURITY_KEY_ID_ENV,
            self.signer.key_id(),
            ordo_secevent::SECURITY_PUBKEY_ENV,
            self.signer.verifier().public_hex()
        )
    }
}

/// The checkpoint key: `from_env` when the setting is there, else the one
/// stored in `store`, else a new one made, sealed and stored.
///
/// `from_env` is `ordo_secevent::security_signer_from_env()`, passed in so
/// a test can say "set" or "unset" without touching the process
/// environment.
pub async fn resolve(
    from_env: Result<Ed25519Signer, SignerError>,
    store: &SecurityKeyStore,
    keys: &dyn KeyProvider,
    now: u64,
) -> Result<CheckpointKey, SecurityKeyError> {
    match from_env {
        Ok(signer) => {
            return Ok(CheckpointKey {
                signer: Arc::new(signer),
                source: KeySource::Set,
            })
        }
        // Only the seed itself missing means unset. A missing key id beside
        // a set seed, or a seed that does not parse, is the setting's own
        // refusal.
        Err(SignerError::Missing(var)) if var == ordo_secevent::SECURITY_KEY_ENV => {}
        Err(e) => return Err(SecurityKeyError::Set(e)),
    }
    if let Some(stored) = store.load().await.map_err(SecurityKeyError::Store)? {
        return open(&stored, keys, KeySource::Stored);
    }
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed)
        .map_err(|_| SecurityKeyError::Custody("could not be made: no randomness"))?;
    let key_id = key_id_for(&seed);
    let sealed = seal(keys, &seed)?;
    store
        .insert(&key_id, &sealed, now)
        .await
        .map_err(SecurityKeyError::Store)?;
    // Read back: a replica that made one at the same moment may have won.
    let stored =
        store
            .load()
            .await
            .map_err(SecurityKeyError::Store)?
            .ok_or(SecurityKeyError::Store(StoreFault::unavailable(
                "the security checkpoint key was written and is not there",
            )))?;
    let source = if stored.0 == key_id {
        KeySource::Made
    } else {
        KeySource::Stored
    };
    open(&stored, keys, source)
}

/// `sec-` and the first 16 hex characters of SHA-256 of the public key, so
/// the id names the key and a second key never reuses it.
fn key_id_for(seed: &[u8; 32]) -> String {
    use sha2::{Digest, Sha256};
    let public = ed25519_dalek::SigningKey::from_bytes(seed).verifying_key();
    let digest = Sha256::digest(public.as_bytes());
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("sec-{hex}")
}

fn system() -> SpaceId {
    SpaceId(crate::provision::SYSTEM_SPACE.to_string())
}

fn seal(keys: &dyn KeyProvider, seed: &[u8; 32]) -> Result<String, SecurityKeyError> {
    let lease = keys
        .release(&system(), Purpose::Write, "security-checkpoint-key")
        .map_err(|_| SecurityKeyError::Custody("could not be sealed: no key was released"))?;
    let sealed = keys
        .seal_at(&lease, ENTITY, FIELD, Anchor::Row(ROW), seed)
        .map_err(|_| SecurityKeyError::Custody("could not be sealed"))?;
    Ok(sealed.iter().map(|b| format!("{b:02x}")).collect())
}

fn open(
    (key_id, hex): &(String, String),
    keys: &dyn KeyProvider,
    source: KeySource,
) -> Result<CheckpointKey, SecurityKeyError> {
    let bytes: Option<Vec<u8>> = (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok())
        .collect();
    let bytes = bytes.ok_or(SecurityKeyError::Custody("is not hex in `$securitykey`"))?;
    let lease = keys
        .release(&system(), Purpose::Read, "security-checkpoint-key")
        .map_err(|_| SecurityKeyError::Custody("could not be opened: no key was released"))?;
    let plain = keys
        .open_at(&lease, ENTITY, FIELD, Anchor::Row(ROW), &bytes)
        .map_err(|_| SecurityKeyError::Custody("could not be opened"))?;
    let seed: [u8; 32] = plain
        .as_slice()
        .try_into()
        .map_err(|_| SecurityKeyError::Custody("is not 32 bytes"))?;
    Ok(CheckpointKey {
        signer: Arc::new(Ed25519Signer::from_seed(key_id.clone(), &seed)),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ordo_commandlog::{ChainHash, CheckpointVerifier, Timestamp};
    use ordo_crypto::TestProvider;
    use ordo_secevent::{checkpoint_at, Scope};

    fn unset() -> Result<Ed25519Signer, SignerError> {
        Err(SignerError::Missing(ordo_secevent::SECURITY_KEY_ENV.into()))
    }

    /// The verifier a reader builds from the published line alone.
    fn from_published(line: &str) -> CheckpointVerifier {
        let value = |name: &str| {
            line.split(' ')
                .find_map(|kv| kv.strip_prefix(&format!("{name}=")))
                .expect(name)
                .to_string()
        };
        CheckpointVerifier::from_public_hex(
            value(ordo_secevent::SECURITY_KEY_ID_ENV),
            &value(ordo_secevent::SECURITY_PUBKEY_ENV),
        )
        .expect("a public key")
    }

    #[tokio::test]
    async fn unset_makes_one_and_a_restart_reuses_it() {
        let store = SecurityKeyStore::memory();
        let keys = TestProvider::new();
        let made = resolve(unset(), &store, &keys, 1).await.expect("made");
        assert_eq!(made.source, KeySource::Made);
        assert!(made.signer.key_id().starts_with("sec-"));

        let again = resolve(unset(), &store, &keys, 2).await.expect("stored");
        assert_eq!(again.source, KeySource::Stored);
        assert_eq!(again.signer.key_id(), made.signer.key_id());
        assert_eq!(again.published(), made.published());
    }

    #[tokio::test]
    async fn a_set_key_wins_over_a_stored_one() {
        let store = SecurityKeyStore::memory();
        let keys = TestProvider::new();
        let made = resolve(unset(), &store, &keys, 1).await.expect("made");
        let set = Ed25519Signer::from_seed("sec-set", &[9u8; 32]);
        let got = resolve(Ok(set), &store, &keys, 2).await.expect("set");
        assert_eq!(got.source, KeySource::Set);
        assert_eq!(got.signer.key_id(), "sec-set");
        assert_ne!(got.published(), made.published());
    }

    #[tokio::test]
    async fn a_set_key_that_does_not_parse_makes_nothing() {
        let store = SecurityKeyStore::memory();
        let keys = TestProvider::new();
        let bad = Err(SignerError::BadKey("seed is not hexadecimal".into()));
        assert!(matches!(
            resolve(bad, &store, &keys, 1).await,
            Err(SecurityKeyError::Set(_))
        ));
        let id_missing = Err(SignerError::Missing(
            ordo_secevent::SECURITY_KEY_ID_ENV.into(),
        ));
        assert!(matches!(
            resolve(id_missing, &store, &keys, 1).await,
            Err(SecurityKeyError::Set(_))
        ));
        assert!(store.load().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_checkpoint_verifies_with_the_published_key() {
        let store = SecurityKeyStore::memory();
        let keys = TestProvider::new();
        let key = resolve(unset(), &store, &keys, 1).await.expect("made");
        let verifier = from_published(&key.published());
        let cp = checkpoint_at(
            &Scope::Deployment,
            3,
            ChainHash([7u8; 32]),
            key.signer.as_ref(),
            Timestamp(100),
        );
        assert!(cp.verify(&verifier));
        // Another deployment's made key does not verify it.
        let other = resolve(unset(), &SecurityKeyStore::memory(), &keys, 1)
            .await
            .expect("made");
        assert!(!cp.verify(&from_published(&other.published())));
    }

    #[tokio::test]
    async fn the_seed_is_sealed_not_stored_in_the_clear() {
        let store = SecurityKeyStore::memory();
        let keys = TestProvider::new();
        let key = resolve(unset(), &store, &keys, 1).await.expect("made");
        let (_, sealed) = store.load().await.unwrap().expect("a row");
        assert_ne!(sealed.len(), 64);
        let opened = open(&("x".into(), sealed), &keys, KeySource::Stored).expect("opens");
        assert_eq!(
            opened.signer.verifier().public_hex(),
            key.signer.verifier().public_hex()
        );
    }
}
