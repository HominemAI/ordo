//! The one reader of a deployment setting that holds a secret.
//!
//! A secret setting `<NAME>` can be given two ways:
//!
//! - `<NAME>_FILE`: the path of a file that holds the value. This is the
//!   form a secret store mounts, and the form the console's supervisor
//!   always uses (`plan-console.md` 6.3): a value in a variable shows in
//!   `/proc/<pid>/environ` and in every `docker inspect`.
//! - `<NAME>`: the value itself, for a laptop.
//!
//! Setting both is refused, so a stale value never wins silently. Trailing
//! line ends are trimmed from a file, since editors add one.
//!
//! Every crate that reads a key, a password or a token from the environment
//! calls [`secret`], so each one takes the file form the same way, and its
//! own error names the variable to fix.

use std::fmt;
use std::path::Path;

/// The deployment settings that hold a secret: the one list (console step
/// 2b, review of step 3a). The console seals each in its database and hands
/// it to the child as `<NAME>_FILE`; `ordo-host` reads each through
/// [`secret`], so it takes the file form. The boot record never hashes one
/// by value ([`hides_value`]).
///
/// Names the host refuses in plain form (the mail, human-check and CDN
/// secrets) are here too: the console seals them all the same.
///
/// `ORDO_AUTH_CLAIM_CODE` is here since console step 6: it reached the
/// deployment as a plain setting in a revision row before. The host reads it
/// through [`secret`], so the file form works, and the console seals it.
pub const SECRET_SETTINGS: &[&str] = &[
    "ORDO_PG_PASSWORD",
    "ORDO_TOKEN_KEY",
    "ORDO_PAGE_KEY",
    "ORDO_MASTER_KEY",
    "ORDO_MASTER_KEY_PASSPHRASE",
    "ORDO_CHECKPOINT_KEY",
    "ORDO_SECURITY_CHECKPOINT_KEY",
    "ORDO_BLOB_S3_SECRET_ACCESS_KEY",
    "ORDO_MAIL_KEY",
    "ORDO_MAIL_CREDENTIALS",
    "ORDO_MAIL_PASSWORD",
    "ORDO_PUBLIC_HUMAN_SECRET",
    "ORDO_PUBLIC_CDN_PURGE_SECRET",
    "ORDO_AUTH_HUMAN_CHECK_SECRET",
    "ORDO_AUTH_CLAIM_CODE",
];

/// Endings that make a setting a sealed secret beyond [`SECRET_SETTINGS`]:
/// a sign-in provider's `ORDO_AUTH_<P>_CLIENT_SECRET`, and any later
/// password or passphrase. The host reads every one in file form.
pub const SEALED_ENDINGS: &[&str] = &["_SECRET", "_PASSWORD", "_PASSPHRASE"];

/// Endings whose value is never shown, logged or hashed: the sealed ones,
/// and settings such as `ORDO_PG_TENANT_CREDENTIAL` that the host reads in
/// plain form. Erring wide here costs nothing: a public value left out of a
/// fingerprint is still named as set.
pub const HIDDEN_ENDINGS: &[&str] = &[
    "_SECRET",
    "_PASSWORD",
    "_PASSPHRASE",
    "_KEY",
    "_CREDENTIALS",
    "_CREDENTIAL",
    "_CODE",
];

/// Whether the console seals `name` and hands it over as `<NAME>_FILE`.
pub fn is_sealed_secret(name: &str) -> bool {
    SECRET_SETTINGS.contains(&name) || SEALED_ENDINGS.iter().any(|e| name.ends_with(e))
}

/// Whether `name`'s value must never be shown, logged or hashed by value.
/// Every sealed secret, and more.
pub fn hides_value(name: &str) -> bool {
    is_sealed_secret(name) || HIDDEN_ENDINGS.iter().any(|e| name.ends_with(e))
}

/// Why a secret setting could not be read. Each case names the variable a
/// deployment owner changes.
#[derive(Debug)]
pub enum SecretError {
    /// `<NAME>_FILE` names a file that could not be read.
    Unreadable {
        var: String,
        path: String,
        cause: std::io::Error,
    },
    /// `<NAME>_FILE` names an empty file.
    Empty { var: String, path: String },
    /// Both `<NAME>` and `<NAME>_FILE` are set.
    Both { name: String },
}

impl SecretError {
    /// The variable the deployment owner changes.
    pub fn var(&self) -> String {
        match self {
            SecretError::Unreadable { var, .. } | SecretError::Empty { var, .. } => var.clone(),
            SecretError::Both { name } => format!("{name}_FILE"),
        }
    }
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretError::Unreadable { var, path, cause } => write!(
                f,
                "{var} names `{path}`, which could not be read ({cause}); point {var} at a \
                 file this process can read"
            ),
            SecretError::Empty { var, path } => write!(
                f,
                "{var} names `{path}`, which is empty; put the value in the file"
            ),
            SecretError::Both { name } => write!(
                f,
                "{name} and {name}_FILE are both set; unset one (the file form is the safer: \
                 a value in a variable shows in a process listing)"
            ),
        }
    }
}

impl std::error::Error for SecretError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SecretError::Unreadable { cause, .. } => Some(cause),
            _ => None,
        }
    }
}

/// Read the secret setting `name` from this process's environment:
/// `Ok(None)` when neither form is set.
pub fn secret(name: &str) -> Result<Option<String>, SecretError> {
    secret_from(name, &|k| std::env::var(k).ok(), &|p| {
        std::fs::read_to_string(p)
    })
}

/// [`secret`] over any variable source and file reader, for tests and for
/// a caller that holds its variables in a map.
pub fn secret_from(
    name: &str,
    get: &dyn Fn(&str) -> Option<String>,
    read: &dyn Fn(&Path) -> std::io::Result<String>,
) -> Result<Option<String>, SecretError> {
    let var = format!("{name}_FILE");
    let file = get(&var).filter(|v| !v.trim().is_empty());
    let value = get(name);
    match (file, value) {
        (Some(_), Some(_)) => Err(SecretError::Both {
            name: name.to_string(),
        }),
        (None, value) => Ok(value),
        (Some(path), None) => {
            let path = path.trim().to_string();
            let text = read(Path::new(&path)).map_err(|cause| SecretError::Unreadable {
                var: var.clone(),
                path: path.clone(),
                cause,
            })?;
            let text = text.trim_end_matches(['\n', '\r']).to_string();
            if text.is_empty() {
                return Err(SecretError::Empty { var, path });
            }
            Ok(Some(text))
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_sealed_secret_hides_its_value_and_so_do_codes_and_credentials() {
        for name in super::SECRET_SETTINGS {
            assert!(super::is_sealed_secret(name) && super::hides_value(name));
        }
        for name in [
            "ORDO_AUTH_CLAIM_CODE",
            "ORDO_PG_TENANT_CREDENTIAL",
            "ORDO_AUTH_X_CLIENT_SECRET",
        ] {
            assert!(super::hides_value(name), "{name}");
        }
        // Sealed since console step 6: a revision row never holds it.
        assert!(super::is_sealed_secret("ORDO_AUTH_CLAIM_CODE"));
        assert!(!super::hides_value("ORDO_PG_HOST"));
    }

    use super::*;
    use std::collections::HashMap;

    fn run(vars: &[(&str, &str)], files: &[(&str, &str)]) -> Result<Option<String>, SecretError> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let files: HashMap<String, String> = files
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        secret_from("ORDO_X_KEY", &|k| vars.get(k).cloned(), &|p| {
            files
                .get(&p.display().to_string())
                .cloned()
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
        })
    }

    #[test]
    fn the_file_form_is_read_and_its_line_end_trimmed() {
        let got = run(&[("ORDO_X_KEY_FILE", "/s/k")], &[("/s/k", "abc\n")]).unwrap();
        assert_eq!(got.as_deref(), Some("abc"));
    }

    #[test]
    fn the_value_form_and_neither() {
        assert_eq!(
            run(&[("ORDO_X_KEY", "v")], &[]).unwrap().as_deref(),
            Some("v")
        );
        assert_eq!(run(&[], &[]).unwrap(), None);
    }

    #[test]
    fn each_refusal_names_the_variable_to_change() {
        let e = run(&[("ORDO_X_KEY_FILE", "/nope")], &[]).unwrap_err();
        assert_eq!(e.var(), "ORDO_X_KEY_FILE");
        assert!(e.to_string().contains("/nope"), "{e}");
        let e = run(&[("ORDO_X_KEY_FILE", "/e")], &[("/e", "\n")]).unwrap_err();
        assert!(matches!(e, SecretError::Empty { .. }), "{e}");
        let e = run(&[("ORDO_X_KEY_FILE", "/s/k"), ("ORDO_X_KEY", "v")], &[]).unwrap_err();
        assert!(
            e.to_string().contains("ORDO_X_KEY and ORDO_X_KEY_FILE"),
            "{e}"
        );
    }
}
