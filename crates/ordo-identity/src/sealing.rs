//! A sealing key in a file, and the values sealed with it.
//!
//! Shared by the console (`ORDO_CONSOLE_KEY_FILE`: managed secrets and the
//! console key) and the identity service (`ORDO_IDENTITY_KEY_FILE`: its
//! signing keys at rest), `plan-identity-service.md` 2.1. The key is 32
//! random bytes written as 64 hex digits, made at first boot, mode 0600. A
//! copy of the database alone opens nothing.
//!
//! A value is sealed with XChaCha20-Poly1305 (`ordo_crypto::seal_at_with`),
//! bound to an entity word and a row anchor, so a sealed value moved to
//! another row or read as another kind does not open.

use std::io::Write as _;
use std::path::Path;

use ordo_crypto::custody::Anchor;

const FIELD: &str = "value";
const EPOCH: u8 = 1;

/// A sealing key, read from or made at its file.
#[derive(Clone)]
pub struct SealingKey([u8; 32]);

impl std::fmt::Debug for SealingKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SealingKey(..)")
    }
}

/// The key file could not be read or made. Names the setting that points
/// at it, and what to do.
#[derive(Debug)]
pub struct KeyFileError {
    pub setting: &'static str,
    pub problem: String,
    pub fix: String,
    pub cause: Option<std::io::Error>,
}

impl std::fmt::Display for KeyFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}. {}", self.setting, self.problem, self.fix)
    }
}

impl std::error::Error for KeyFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_ref()
            .map(|c| c as &(dyn std::error::Error + 'static))
    }
}

/// The key did not open a sealed value: another key, another row, another
/// entity, or damaged bytes. One answer for all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotOpened;

impl std::fmt::Display for NotOpened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the sealing key did not open this value")
    }
}

impl std::error::Error for NotOpened {}

/// What [`SealingKey::load_or_create`] found.
#[derive(Debug)]
pub struct Loaded {
    pub key: SealingKey,
    /// The file was not there and a new key was written.
    pub made: bool,
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

impl SealingKey {
    /// Read the key at `path`, or make it when the file is not there yet.
    /// `setting` is the variable that names the file, and `who` the service
    /// that holds it ("console", "identity service"), for the refusals.
    pub fn load_or_create(
        path: &Path,
        setting: &'static str,
        who: &str,
    ) -> Result<Loaded, KeyFileError> {
        let shown = path.display().to_string();
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let bytes = unhex(text.trim())
                    .filter(|b| b.len() == 32)
                    .ok_or_else(|| KeyFileError {
                        setting,
                        problem: format!("names `{shown}`, which does not hold a 64-digit hex key"),
                        fix: format!(
                            "Restore the {who}'s key file from its backup, or point {setting} at \
                             it. A new key opens none of the values sealed with the old one"
                        ),
                        cause: None,
                    })?;
                let mut key = [0u8; 32];
                key.copy_from_slice(&bytes);
                Ok(Loaded {
                    key: SealingKey(key),
                    made: false,
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut key = [0u8; 32];
                getrandom::getrandom(&mut key).map_err(|e| KeyFileError {
                    setting,
                    problem: format!(
                        "could not be made: the operating system gave no randomness ({e})"
                    ),
                    fix: "Start the service on a host whose random number generator answers"
                        .to_string(),
                    cause: None,
                })?;
                write_new(path, &hex(&key)).map_err(|e| KeyFileError {
                    setting,
                    problem: format!(
                        "names `{shown}`, and the {who} could not write its key there"
                    ),
                    fix: format!(
                        "Set {setting} to a file on the {who}'s own volume that the {who} can \
                         write"
                    ),
                    cause: Some(e),
                })?;
                Ok(Loaded {
                    key: SealingKey(key),
                    made: true,
                })
            }
            Err(e) => Err(KeyFileError {
                setting,
                problem: format!("names `{shown}`, which the {who} could not read"),
                fix: format!("Let the {who} read {setting}, or point it at the key's backup"),
                cause: Some(e),
            }),
        }
    }

    /// A key from fixed bytes, for tests.
    pub fn from_bytes(key: [u8; 32]) -> SealingKey {
        SealingKey(key)
    }

    /// Seal a value as hex for its row, bound to `entity` and `anchor`.
    pub fn seal(&self, entity: &str, anchor: &str, plain: &[u8]) -> String {
        hex(&ordo_crypto::seal_at_with(
            &self.0,
            EPOCH,
            entity,
            FIELD,
            Anchor::Row(anchor),
            plain,
        ))
    }

    /// Open a value [`SealingKey::seal`] made with the same entity and
    /// anchor.
    pub fn open(&self, entity: &str, anchor: &str, sealed_hex: &str) -> Result<Vec<u8>, NotOpened> {
        let bytes = unhex(sealed_hex).ok_or(NotOpened)?;
        ordo_crypto::open_at_with(&self.0, entity, FIELD, Anchor::Row(anchor), &bytes)
            .map_err(|_| NotOpened)
    }
}

fn write_new(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    writeln!(f, "{text}")?;
    f.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_made_once_and_opens_only_what_it_sealed_there() {
        let dir = std::env::temp_dir().join(format!("ordo-sealing-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("svc.key");
        let k1 = SealingKey::load_or_create(&path, "X_KEY_FILE", "service").expect("made");
        assert!(k1.made);
        let k2 = SealingKey::load_or_create(&path, "X_KEY_FILE", "service").expect("read");
        assert!(!k2.made);
        let sealed = k1.key.seal("thing", "main/a", b"hunter2");
        assert!(!sealed.contains("hunter2"));
        assert_eq!(k2.key.open("thing", "main/a", &sealed).unwrap(), b"hunter2");
        assert_eq!(k2.key.open("thing", "main/b", &sealed), Err(NotOpened));
        assert_eq!(k2.key.open("other", "main/a", &sealed), Err(NotOpened));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::write(&path, "not a key\n").unwrap();
        let e = SealingKey::load_or_create(&path, "X_KEY_FILE", "service").unwrap_err();
        assert_eq!(e.setting, "X_KEY_FILE");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
