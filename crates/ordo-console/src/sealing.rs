//! The console's sealing key and the managed secrets it seals
//! (`plan-console.md` 6.3 and 8.2).
//!
//! The key is a file on the console's own volume (`ORDO_CONSOLE_KEY_FILE`,
//! default `<data dir>/console.key`), made at first boot, mode 0600. It is
//! never in the child's environment. A copy of the console database alone
//! does not open a secret.
//!
//! The file and the seal are `ordo_identity::sealing`, shared with the
//! identity service (`plan-identity-service.md` 2.1). This wrapper keeps
//! the console's entity words and its refusals.

use std::path::Path;

use ordo_identity::sealing::{KeyFileError, SealingKey};

use crate::config::KEY_FILE_ENV;
use crate::error::ConsoleError;

const ENTITY: &str = "console_secret";
/// The console key's private half (G14), sealed apart from managed
/// secrets so neither can be opened as the other.
const CREDENTIAL_ENTITY: &str = "console_credential";

/// The console's sealing key.
#[derive(Clone, Debug)]
pub struct ConsoleKey(SealingKey);

fn refused(e: KeyFileError) -> ConsoleError {
    match e.cause {
        Some(cause) => ConsoleError::setting_caused(e.setting, e.problem, e.fix, cause),
        None => ConsoleError::setting(e.setting, e.problem, e.fix),
    }
}

impl ConsoleKey {
    /// Read the key, or make it when the file is not there yet.
    pub fn load_or_create(path: &Path) -> Result<ConsoleKey, ConsoleError> {
        let loaded = SealingKey::load_or_create(path, KEY_FILE_ENV, "console").map_err(refused)?;
        if loaded.made {
            eprintln!(
                "ordo-console: made a new sealing key at {}. Back it up with the console \
                 database: managed secrets do not open without it.",
                path.display()
            );
        }
        Ok(ConsoleKey(loaded.key))
    }

    fn anchor(dep: &str, name: &str) -> String {
        format!("{dep}/{name}")
    }

    /// Seal one secret value, as hex for its row.
    pub fn seal(&self, dep: &str, name: &str, plain: &[u8]) -> String {
        self.0.seal(ENTITY, &Self::anchor(dep, name), plain)
    }

    /// Open one sealed secret.
    pub fn open(&self, dep: &str, name: &str, sealed_hex: &str) -> Result<Vec<u8>, ConsoleError> {
        self.0
            .open(ENTITY, &Self::anchor(dep, name), sealed_hex)
            .map_err(|_| {
                ConsoleError::setting(
                    KEY_FILE_ENV,
                    format!("does not open the managed secret {name}"),
                    format!(
                        "Point {KEY_FILE_ENV} at the key this console database was sealed with, \
                         or set {name} again on the Runtime screen"
                    ),
                )
            })
    }

    /// Seal the console key's private half for one deployment, bound to
    /// its key id.
    pub fn seal_credential(&self, dep: &str, key_id: &str, plain: &[u8]) -> String {
        self.0
            .seal(CREDENTIAL_ENTITY, &Self::anchor(dep, key_id), plain)
    }

    /// Open it.
    pub fn open_credential(
        &self,
        dep: &str,
        key_id: &str,
        sealed_hex: &str,
    ) -> Result<Vec<u8>, ConsoleError> {
        self.0
            .open(CREDENTIAL_ENTITY, &Self::anchor(dep, key_id), sealed_hex)
            .map_err(|_| {
                ConsoleError::setting(
                    KEY_FILE_ENV,
                    "does not open the console key this console signs in to the deployment with",
                    format!(
                        "Point {KEY_FILE_ENV} at the key this console database was sealed with, \
                         or link the console to the deployment again"
                    ),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_made_once_and_opens_what_it_sealed() {
        let dir = std::env::temp_dir().join(format!("ordo-console-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("console.key");
        let k1 = ConsoleKey::load_or_create(&path).expect("made");
        let k2 = ConsoleKey::load_or_create(&path).expect("read");
        let sealed = k1.seal("main", "ORDO_PG_PASSWORD", b"hunter2");
        assert!(!sealed.contains("hunter2"));
        assert_eq!(
            k2.open("main", "ORDO_PG_PASSWORD", &sealed).unwrap(),
            b"hunter2"
        );
        assert!(
            k2.open("main", "OTHER", &sealed).is_err(),
            "bound to its name"
        );
        assert!(
            k2.open_credential("main", "ORDO_PG_PASSWORD", &sealed)
                .is_err(),
            "a managed secret does not open as the console key"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
