//! How a test runs the CLI without the developer's own sign-in leaking in.
//!
//! The door commands take a token from `ORDO_TOKEN`, a machine key, or the
//! session `ordo login` stored in the config folder. A test must see none
//! of the developer's. So every test that runs a command goes through here:
//!
//! - `operator::run` is `ordo_cli::operator::run` after [`isolate`];
//! - [`ordo`] is the binary with the same settings on its own environment.
//!
//! [`isolate`] runs once per test binary: it points `ORDO_CONFIG_DIR` at an
//! empty folder of its own and clears the sign-in settings.

#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// The settings a developer's shell may hold that would sign a test in.
pub const SIGN_IN_SETTINGS: &[&str] = &[
    "ORDO_TOKEN",
    "ORDO_MACHINE_KEY",
    "ORDO_MACHINE_KEY_FILE",
    "ORDO_IDENTITY_URL",
    "ORDO_IDENTITY_DEPLOYMENT",
];

/// An empty config folder for this test binary.
pub fn config_dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = std::env::temp_dir().join(format!("ordo-cli-test-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("a test config folder");
        d
    })
}

/// Point this process at the empty config folder and clear the sign-in
/// settings. Once per test binary, before any command reads them.
pub fn isolate() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        std::env::set_var(ordo_cli::signin::CONFIG_DIR_ENV, config_dir());
        for k in SIGN_IN_SETTINGS {
            std::env::remove_var(k);
        }
    });
}

/// The `ordo` binary, isolated the same way.
pub fn ordo() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_ordo"));
    c.env(ordo_cli::signin::CONFIG_DIR_ENV, config_dir());
    for k in SIGN_IN_SETTINGS {
        c.env_remove(k);
    }
    c
}

/// `ordo_cli::operator`, with `run` isolated.
pub mod operator {
    pub use ordo_cli::operator::*;

    pub fn run(cmd: &str, args: &[String]) -> Outcome {
        super::isolate();
        ordo_cli::operator::run(cmd, args)
    }

    pub fn run_with_input(cmd: &str, args: &[String], input: &[u8]) -> Outcome {
        super::isolate();
        ordo_cli::operator::run_with_input(cmd, args, input)
    }
}
