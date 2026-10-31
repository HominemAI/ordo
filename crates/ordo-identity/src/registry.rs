//! Config-driven provider set — the runtime mirror of the spec's
//! `auth { methods … }` block. Each configured method name (`passkey`,
//! `password`, `google`, `corp-sso`, …) maps to exactly one provider
//! instance; the login endpoint routes by method name and never knows which
//! family is behind it.

use std::collections::BTreeMap;

use crate::principal::AuthMethodKind;
use crate::provider::AuthProvider;

/// Registry failures. Duplicate registration is a deploy-time config error,
/// not a shadowing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegistryError {
    DuplicateMethod(String),
    UnknownMethod(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::DuplicateMethod(n) => {
                write!(f, "auth method `{n}` is already configured")
            }
            RegistryError::UnknownMethod(n) => {
                write!(f, "no auth method named `{n}` is configured")
            }
        }
    }
}

impl std::error::Error for RegistryError {}

/// Method name → provider. Built once from deployment config; immutable at
/// serve time.
#[derive(Default)]
pub struct ProviderRegistry {
    providers: BTreeMap<String, Box<dyn AuthProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        ProviderRegistry::default()
    }

    pub fn register(
        &mut self,
        name: &str,
        provider: Box<dyn AuthProvider>,
    ) -> Result<(), RegistryError> {
        if self.providers.contains_key(name) {
            return Err(RegistryError::DuplicateMethod(name.to_string()));
        }
        self.providers.insert(name.to_string(), provider);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<&dyn AuthProvider, RegistryError> {
        self.providers
            .get(name)
            .map(|b| b.as_ref())
            .ok_or_else(|| RegistryError::UnknownMethod(name.to_string()))
    }

    /// Configured methods, for the login-page inventory: (name, family).
    pub fn methods(&self) -> impl Iterator<Item = (&str, AuthMethodKind)> {
        self.providers.iter().map(|(n, p)| (n.as_str(), p.kind()))
    }
}
