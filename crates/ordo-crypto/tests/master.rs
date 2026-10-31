//! R2b — the sealed-master file: a master key that is not plaintext in the
//! process environment.
//!
//! Every test here writes its own file and its own passphrase, which is
//! half the reason the file source ships before any KMS: the whole ceremony
//! is testable in CI with no secret, no cloud account and no credential.

use std::path::PathBuf;

use ordo_crypto::master;
use ordo_crypto::{KeyError, KeyProvider};

const PASS: &[u8] = b"correct horse battery staple";

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ordo-master-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).expect("tmp");
    d.join("master.key")
}

/// The round trip, and the two properties the format exists for: the file
/// alone opens nothing, and the passphrase alone opens nothing.
#[test]
fn a_sealed_master_opens_with_its_passphrase_and_with_nothing_else() {
    let path = tmp("roundtrip");
    let _ = std::fs::remove_file(&path);
    let fp = master::seal_master(&path, "acme-2026-09", PASS).expect("sealed");
    assert_eq!(fp.len(), 16, "a fingerprint is eight bytes of hex");

    let master = master::open_sealed_master(&path, PASS).expect("opens");
    assert_eq!(master::fingerprint(&master), fp, "the file names its key");
    assert_eq!(
        master::key_id_of(&path).expect("id"),
        "acme-2026-09",
        "the id is readable without the passphrase — it exists to be printed"
    );

    // **A wrong passphrase is `Seal` and says nothing else.** No byte
    // position, no "the salt looked wrong", and no distinction from a
    // corrupted envelope: which byte failed is an oracle.
    let refused = master::open_sealed_master(&path, b"correct horse battery stapl");
    assert!(
        matches!(refused, Err(KeyError::Seal)),
        "a wrong passphrase must be an indistinct Seal: {refused:?}"
    );
    assert_eq!(
        refused.unwrap_err().to_string(),
        "sealed payload did not open",
        "the refusal text must not distinguish this failure from any other"
    );
    std::fs::remove_file(&path).ok();
}

/// The key id is the envelope's AAD, so editing it in the file breaks the
/// seal. An id that could be changed without invalidating the envelope
/// would be a label rather than a binding.
#[test]
fn editing_the_key_id_breaks_the_envelope() {
    let path = tmp("aad");
    let _ = std::fs::remove_file(&path);
    master::seal_master(&path, "before", PASS).expect("sealed");
    let raw = std::fs::read(&path).expect("read");
    let edited: Vec<u8> = String::from_utf8_lossy(&raw[..raw.len().min(64)])
        .replacen("before", "after!", 1)
        .into_bytes()
        .into_iter()
        .chain(raw[raw.len().min(64)..].iter().copied())
        .collect();
    let other = tmp("aad-edited");
    std::fs::write(&other, &edited).expect("write");
    assert!(
        matches!(
            master::open_sealed_master(&other, PASS),
            Err(KeyError::Seal)
        ),
        "the id is AAD; an edited id must not open"
    );
    std::fs::remove_file(&path).ok();
    std::fs::remove_file(&other).ok();
}

/// A master key file is never overwritten. Clobbering it erases every
/// space's custody, and there is no escrow and no recovery.
#[test]
fn sealing_over_an_existing_file_is_refused() {
    let path = tmp("no-clobber");
    let _ = std::fs::remove_file(&path);
    master::seal_master(&path, "first", PASS).expect("sealed");
    let refused = master::seal_master(&path, "second", PASS);
    let Err(KeyError::Store(text)) = refused else {
        panic!("overwriting a master key file must be refused");
    };
    assert!(
        text.reason().contains("never overwritten"),
        "the refusal does not say why: {text}"
    );
    // And the first file is untouched.
    assert_eq!(master::key_id_of(&path).expect("id"), "first");
    std::fs::remove_file(&path).ok();
}

/// Anything that is not the format is the same refusal, and it names the
/// format rather than the byte.
#[test]
fn a_file_that_is_not_the_format_is_refused_by_name() {
    let path = tmp("garbage");
    std::fs::write(&path, b"-----BEGIN SOMETHING ELSE-----\n").expect("write");
    let Err(KeyError::Store(text)) = master::open_sealed_master(&path, PASS) else {
        panic!("a foreign file must be refused");
    };
    assert!(text.reason().contains(master::MASTER_FILE_TAG), "{text}");
    std::fs::remove_file(&path).ok();
}

/// **The trap this API exists to close.** A sealed file holds the master
/// *itself*; `LocalKeyProvider::new` hashes what it is given. Feeding one
/// to the other would produce a provider whose master is
/// `HMAC(master)` — a provider that opens nothing, with no error until the
/// first read of a sealed field, and no way to tell it apart from a wrong
/// passphrase.
///
/// So the source's bytes go to `from_master`, and this pins the difference.
#[test]
fn a_sealed_master_goes_to_from_master_not_to_new() {
    let path = tmp("from-master");
    let _ = std::fs::remove_file(&path);
    master::seal_master(&path, "acme", PASS).expect("sealed");
    let m = master::open_sealed_master(&path, PASS).expect("opens");
    let space = ordo_crypto::SpaceId("acme".into());
    let store = SharedStore::default();

    // Provision under a provider built from the master itself…
    let a = ordo_crypto::LocalKeyProvider::from_master(m, Box::new(store.clone()));
    a.create_space(&space).expect("custody");
    let lease = ordo_crypto::KeyProvider::release(&a, &space, ordo_crypto::Purpose::Write, "r2b")
        .expect("lease");
    let sealed = ordo_crypto::KeyProvider::seal_at(
        &a,
        &lease,
        "orders",
        "memo",
        ordo_crypto::Anchor::Row("r1"),
        b"kept",
    )
    .expect("sealed");

    // …and a second process, same file, same passphrase, same store.
    let m2 = master::open_sealed_master(&path, PASS).expect("opens");
    let b = ordo_crypto::LocalKeyProvider::from_master(m2, Box::new(store.clone()));
    let read = ordo_crypto::KeyProvider::release(&b, &space, ordo_crypto::Purpose::Read, "r2b")
        .expect("lease");
    assert_eq!(
        ordo_crypto::KeyProvider::open_at(
            &b,
            &read,
            "orders",
            "memo",
            ordo_crypto::Anchor::Row("r1"),
            &sealed
        )
        .expect("opens"),
        b"kept"
    );

    // The wrong constructor: `new` hashes again, so its master is a
    // different key and the SMK row does not unwrap.
    //
    // Asked with `open`, not with `current_epoch` — see
    // `a_wrong_master_is_invisible_to_the_presence_checks` below for why
    // that distinction is load-bearing.
    let wrong = ordo_crypto::LocalKeyProvider::new(&m, Box::new(store.clone()));
    let wrong_lease =
        ordo_crypto::KeyProvider::release(&wrong, &space, ordo_crypto::Purpose::Read, "r2b")
            .expect("the SPACE exists — that is not the secret");
    assert!(
        matches!(
            ordo_crypto::KeyProvider::open_at(
                &wrong,
                &wrong_lease,
                "orders",
                "memo",
                ordo_crypto::Anchor::Row("r1"),
                &sealed
            ),
            Err(KeyError::Seal)
        ),
        "`new` over a master must not silently produce a working provider"
    );
}

/// **A wrong master key is invisible to `release` and `current_epoch`.**
///
/// Both are presence checks over the store: `release` asks whether an SMK
/// row exists, `current_epoch` takes the maximum epoch of the DEK rows.
/// Neither unwraps anything, so a deployment holding the *wrong* master
/// passes both and fails on the first sealed read.
///
/// That matters beyond this crate: `/health/ready`'s KeyProvider probe was
/// `current_epoch`, so a replica booted with a mistyped `ORDO_MASTER_KEY`
/// would report **ready** and then fail every read of every sealed field.
/// `blind_index` is the cheap question that actually unwraps — it derives
/// the index key from the SMK — and it is what the binary and readiness now
/// ask. Pinned here so the property cannot quietly regress.
#[test]
fn a_wrong_master_is_invisible_to_the_presence_checks() {
    let space = ordo_crypto::SpaceId("acme".into());
    let store = SharedStore::default();
    ordo_crypto::LocalKeyProvider::new(b"the-right-master-key-for-this", Box::new(store.clone()))
        .create_space(&space)
        .expect("custody");

    let wrong = ordo_crypto::LocalKeyProvider::new(
        b"a-completely-different-master",
        Box::new(store.clone()),
    );
    // The presence checks pass…
    ordo_crypto::KeyProvider::release(&wrong, &space, ordo_crypto::Purpose::Read, "probe")
        .expect("release is a presence check");
    assert_eq!(
        ordo_crypto::KeyProvider::current_epoch(&wrong, &space).expect("so is current_epoch"),
        1
    );
    // …and the one that unwraps does not.
    assert!(
        matches!(
            ordo_crypto::KeyProvider::blind_index(
                &wrong,
                &space,
                "$custody",
                "probe",
                ordo_crypto::BlindValue::Null,
            ),
            Err(KeyError::Seal)
        ),
        "blind_index must unwrap the SMK, or nothing cheap validates the master"
    );
}

/// The env source returns the **derived** master, not the raw secret — the
/// same derivation `LocalKeyProvider::new` performs, so a deployment can
/// move between the two sources without re-wrapping anything.
///
/// Asserted through `derive_from_secret` rather than by setting the
/// variable: `std::env::set_var` is `unsafe` and this workspace forbids
/// `unsafe`, which is the right trade — a process-wide mutation to test a
/// pure function was never the honest test anyway.
#[test]
fn the_env_source_derives_what_the_provider_derives() {
    let secret = b"a-master-key-long-enough-to-pass";
    let derived = master::derive_from_secret(secret);
    assert_ne!(
        derived.as_slice(),
        secret.as_slice(),
        "the source must return the derived master, not the raw secret"
    );
    // One spelling: the provider's own constructor goes through this
    // function, so a provider over the secret and a provider over the
    // derived master are the same provider.
    let space = ordo_crypto::SpaceId("acme".into());
    let store = SharedStore::default();
    let a = ordo_crypto::LocalKeyProvider::new(secret, Box::new(store.clone()));
    a.create_space(&space).expect("custody");
    let b = ordo_crypto::LocalKeyProvider::from_master(derived, Box::new(store.clone()));
    // `blind_index` rather than `current_epoch`: only an operation that
    // unwraps can tell two masters apart.
    let digest = |p: &ordo_crypto::LocalKeyProvider| {
        ordo_crypto::KeyProvider::blind_index(
            p,
            &space,
            "customers",
            "email",
            ordo_crypto::BlindValue::Text("ada@example.com"),
        )
        .expect("the same master")
    };
    assert_eq!(digest(&a), digest(&b));
}

/// A shareable in-memory store, so two providers can be pointed at the same
/// wrapped rows — which is the only way to ask "are these two masters the
/// same" without a provider handing its master out.
#[derive(Default, Clone)]
struct SharedStore(std::sync::Arc<ordo_crypto::MemoryKeyStore>);

impl ordo_crypto::WrappedKeyStore for SharedStore {
    fn smk(&self, s: &ordo_crypto::SpaceId) -> Result<Option<Vec<u8>>, KeyError> {
        self.0.smk(s)
    }
    fn put_smk(&self, s: &ordo_crypto::SpaceId, w: &[u8]) -> Result<(), KeyError> {
        self.0.put_smk(s, w)
    }
    fn deks(&self, s: &ordo_crypto::SpaceId) -> Result<Vec<(u8, Vec<u8>)>, KeyError> {
        self.0.deks(s)
    }
    fn put_dek(&self, s: &ordo_crypto::SpaceId, e: u8, w: &[u8]) -> Result<(), KeyError> {
        self.0.put_dek(s, e, w)
    }
    fn drop_dek(&self, s: &ordo_crypto::SpaceId, e: u8) -> Result<(), KeyError> {
        self.0.drop_dek(s, e)
    }
    fn drop_space(&self, s: &ordo_crypto::SpaceId) -> Result<(), KeyError> {
        self.0.drop_space(s)
    }
}

/// The argon2id profile this crate pins and the one `ordo-identity` pins
/// are the same three numbers.
///
/// They are deliberately *restated* rather than shared — neither crate
/// depends on a sibling, and coupling two leaves to share three integers
/// would cost more than it buys — so the parity needs an assertion, or the
/// two drift the first time somebody updates one against a new OWASP
/// recommendation.
#[test]
fn the_two_argon2_profiles_agree() {
    assert_eq!(
        (
            ordo_crypto::master::profile::M_COST_KIB,
            ordo_crypto::master::profile::T_COST,
            ordo_crypto::master::profile::P_COST,
        ),
        (
            ordo_identity::provider::ARGON2ID_M_COST_KIB,
            ordo_identity::provider::ARGON2ID_T_COST,
            ordo_identity::provider::ARGON2ID_P_COST,
        ),
        "the master-key KEK and the password hasher must use one profile"
    );
}
