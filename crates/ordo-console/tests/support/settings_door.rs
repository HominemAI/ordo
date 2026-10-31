//! `std.deployment.settings` (G7) for a stand-in door, shared by
//! `tests/settings.rs` and the end-to-end door (`examples/e2e_door.rs`).
//!
//! The answer is `ordo-host`'s own (`ordo_host::settings::effective`), over
//! the environment the supervised stand-in host wrote at its last start
//! (`ORDO_FAKE_ENV_OUT`) and the values a platform op would have stored. So
//! what the door says is what the running child really took.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use ordo_host::settings::{self, StoredRow};
use serde_json::Value;

/// The op's name.
pub const SETTINGS: &str = settings::SETTINGS;

pub struct SettingsDoor {
    /// The file the child wrote its environment to. `None`: nothing started.
    env_file: Mutex<Option<PathBuf>>,
    /// What platform ops stored, by `$deploymentsetting` key.
    stored: Mutex<BTreeMap<String, StoredRow>>,
}

impl SettingsDoor {
    pub fn new(env_file: Option<PathBuf>) -> SettingsDoor {
        SettingsDoor {
            env_file: Mutex::new(env_file),
            stored: Mutex::new(BTreeMap::new()),
        }
    }

    /// Read the child's environment from another file from now on.
    #[allow(dead_code)]
    pub fn read_from(&self, file: PathBuf) {
        *self.env_file.lock().unwrap() = Some(file);
    }

    /// Store a value as its platform op would.
    pub fn store(&self, key: &str, value: &str, by: &str, at: u64) {
        self.stored.lock().unwrap().insert(
            key.to_string(),
            StoredRow {
                value: value.to_string(),
                by: by.to_string(),
                at,
            },
        );
    }

    /// The op's rows.
    pub fn rows(&self) -> Vec<Value> {
        let file = self.env_file.lock().unwrap().clone();
        let text = file
            .and_then(|f| std::fs::read_to_string(f).ok())
            .unwrap_or_default();
        let vars: Vec<(String, String)> = text
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let stored = self.stored.lock().unwrap().clone();
        settings::effective(&settings::held(vars), &stored)
    }
}
