//! The app developer's default per op: the **profile template** every
//! tenant starts from (owner ruling 2026-10-10, `op-role-limit` ask 2).
//!
//! # Where it lives, and why there
//!
//! The spec makes who may call which op runtime state, never syntax
//! (`std-library.md` §5 rule 2, `glossary.md` "profile / role / override"):
//! a domain ships a template, a space's profile is seeded from it, and the
//! space owns it afterwards. `domain-packages.md` §1 names a `profile`
//! entry in a package manifest whose format is not final, and there are no
//! packages yet. So the template is a deployment setting the app developer
//! ships beside the program, as `ORDO_DELEGABLE_OPS` is, and nothing about
//! it is written in the program text.
//!
//! ```text
//! ORDO_ACCESS_TEMPLATE="reportByProject=owner,manager; reportByPerson=owner,manager"
//! ```
//!
//! Each entry names one op and the roles that may call it by default.
//! `op=` with no role is closed to every role until a tenant opens it. An
//! op the setting does not name keeps the built-in default: every role may
//! call an authored read, editors and above an authored write, and the
//! derived `std.*` ops reach managers and owners.
//!
//! # What it may not name
//!
//! Platform ops (`std.space.*` and the rest of
//! [`crate::platform_ops::specs`]) and the deployment's own surfaces. Who
//! may call those is fixed by the platform, and a template that named one
//! could not be honoured, so naming one refuses the boot. Three are the
//! exception, a tenant's session settings and link password limits
//! ([`crate::session_policy::TEMPLATE_GOVERNED`]): the app developer may
//! close `std.space.setSessionPolicy` and `std.space.setLinkPasswordLimits` to
//! tenants (`std.space.setSessionPolicy=`)
//! and open `std.space.sessionPolicy` to every role. A name that is
//! not in the running program does nothing, because the program changes
//! with each deploy and the setting does not.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use ordo_identity::principal::Role;

/// The environment variable. Read once, at boot.
pub const ENV: &str = "ORDO_ACCESS_TEMPLATE";

/// The app developer's default roles for the ops it names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessTemplate {
    lines: BTreeMap<String, Vec<Role>>,
}

/// A role word, as `members.role` spells it.
pub fn role_of(word: &str) -> Option<Role> {
    match word {
        "owner" => Some(Role::Owner),
        "manager" => Some(Role::Manager),
        "editor" => Some(Role::Editor),
        "viewer" => Some(Role::Viewer),
        _ => None,
    }
}

impl AccessTemplate {
    /// The default: no op named, every op at the built-in default.
    pub fn none() -> AccessTemplate {
        AccessTemplate::default()
    }

    /// Read `op=role,role; op=role`. `platform` is the platform ops, which
    /// a template cannot name. Entries are split on `;` or a newline.
    pub fn parse(text: &str, platform: &[String]) -> Result<AccessTemplate, String> {
        let mut lines = BTreeMap::new();
        for entry in text
            .split([';', '\n'])
            .map(str::trim)
            .filter(|e| !e.is_empty())
        {
            let Some((op, roles)) = entry.split_once('=') else {
                return Err(format!(
                    "`{entry}`: write each entry as op=role,role (for example \
                     reportByProject=owner,manager)"
                ));
            };
            let op = op.trim();
            if op.is_empty() || op.contains('*') || op.contains(char::is_whitespace) {
                return Err(format!(
                    "`{entry}`: name one op exactly. A pattern would set the default of ops \
                     added later that nobody looked at"
                ));
            }
            // A tenant's session settings are the one platform surface the
            // app developer may open or close: whether tenants set their
            // own, and who reads them (owner, 2026-10-21).
            let governed = crate::session_policy::TEMPLATE_GOVERNED.contains(&op);
            if !governed && (platform.iter().any(|p| p == op) || reserved(op)) {
                return Err(format!(
                    "`{op}` is a platform op. Who may call it is fixed by the platform, so a \
                     template cannot set it; remove it from {ENV}"
                ));
            }
            let mut set: Vec<Role> = Vec::new();
            for word in roles.split(',').map(str::trim).filter(|w| !w.is_empty()) {
                let Some(role) = role_of(word) else {
                    return Err(format!(
                        "`{entry}`: `{word}` is not a role. The roles are owner, manager, editor \
                         and viewer; a tenant's own sets of people are groups, which a tenant \
                         makes and grants at runtime"
                    ));
                };
                if !set.contains(&role) {
                    set.push(role);
                }
            }
            set.sort();
            if lines.insert(op.to_string(), set).is_some() {
                return Err(format!("`{op}` is named twice in {ENV}; name it once"));
            }
        }
        Ok(AccessTemplate { lines })
    }

    /// From [`ENV`]. Unset is [`AccessTemplate::none`].
    pub fn from_env(platform: &[String]) -> Result<AccessTemplate, String> {
        match std::env::var(ENV) {
            Ok(text) => AccessTemplate::parse(&text, platform),
            Err(_) => Ok(AccessTemplate::none()),
        }
    }

    /// The roles the app developer named for `op`, if it named the op.
    pub fn roles_for(&self, op: &str) -> Option<&[Role]> {
        self.lines.get(op).map(Vec::as_slice)
    }

    /// Every op named, in name order.
    pub fn ops(&self) -> impl Iterator<Item = (&str, &[Role])> {
        self.lines.iter().map(|(o, r)| (o.as_str(), r.as_slice()))
    }

    /// One line for the boot census.
    pub fn describe(&self) -> String {
        if self.lines.is_empty() {
            format!("built-in defaults ({ENV} unset)")
        } else {
            let names: Vec<String> = self
                .lines
                .iter()
                .map(|(op, roles)| {
                    let words: Vec<&str> = roles.iter().map(|r| r.as_str()).collect();
                    format!("{op}={}", words.join(","))
                })
                .collect();
            format!("template: {}", names.join("; "))
        }
    }
}

/// The roles that reach a program op by the app developer's default: the
/// template's entry when it names the op, else the built-in default
/// (`std.*` reaches owner and manager; a write is closed to viewers).
pub fn default_roles(template: &AccessTemplate, name: &str, writes: bool) -> Vec<Role> {
    if let Some(roles) = template.roles_for(name) {
        return roles.to_vec();
    }
    let is_std = name.starts_with("std.");
    [Role::Viewer, Role::Editor, Role::Manager, Role::Owner]
        .into_iter()
        .filter(|role| !(writes && *role == Role::Viewer))
        .filter(|role| !(is_std && matches!(role, Role::Viewer | Role::Editor)))
        .collect()
}

/// The deployment's own surfaces and the blob gate: reserved names in the
/// op namespace that are not in [`crate::platform_ops::specs`].
pub fn reserved(op: &str) -> bool {
    op == crate::serve::HEALTH_DEEP_OP
        || op == crate::serve::METRICS_OP
        || op == crate::serve::ALERTS_OP
        || op == crate::serve::SECEVENTS_OP
        || op == crate::serve::USAGE_OP
        || op == ordo_api::blob::BLOB_READ_OP
        || op.starts_with("std.space.")
        || op.starts_with("std.person.")
        || op.starts_with("std.deployment.")
}

static INSTALLED: OnceLock<AccessTemplate> = OnceLock::new();

/// Install the deployment's template. Called once at boot, before any
/// engine is built; a second call is ignored, for
/// [`crate::presence::install`]'s reason.
pub fn install(template: AccessTemplate) {
    let _ = INSTALLED.set(template);
}

/// The template engines are built under: what was installed, or none.
pub fn current() -> AccessTemplate {
    INSTALLED.get().cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn platform() -> Vec<String> {
        vec!["std.space.callersOf".to_string()]
    }

    #[test]
    fn an_entry_names_an_op_and_its_roles() {
        let t = AccessTemplate::parse(
            "reportByProject=owner,manager; closed=\n purge = owner",
            &platform(),
        )
        .unwrap();
        assert_eq!(
            t.roles_for("reportByProject"),
            Some(&[Role::Owner, Role::Manager][..])
        );
        assert_eq!(t.roles_for("closed"), Some(&[][..]));
        assert_eq!(t.roles_for("purge"), Some(&[Role::Owner][..]));
        assert_eq!(t.roles_for("other"), None);
    }

    #[test]
    fn a_platform_op_a_pattern_and_an_unknown_role_refuse() {
        let e = AccessTemplate::parse("std.space.callersOf=viewer", &platform()).unwrap_err();
        assert!(e.contains("platform op"), "{e}");
        let e = AccessTemplate::parse("std.space.export=viewer", &platform()).unwrap_err();
        assert!(e.contains("platform op"), "{e}");
        let e = AccessTemplate::parse("report*=owner", &platform()).unwrap_err();
        assert!(e.contains("exactly"), "{e}");
        let e = AccessTemplate::parse("report=finance", &platform()).unwrap_err();
        assert!(e.contains("not a role") && e.contains("groups"), "{e}");
        let e = AccessTemplate::parse("report", &platform()).unwrap_err();
        assert!(e.contains("op=role"), "{e}");
        let e = AccessTemplate::parse("a=owner;a=viewer", &platform()).unwrap_err();
        assert!(e.contains("twice"), "{e}");
    }

    #[test]
    fn unset_is_the_built_in_default() {
        assert_eq!(
            AccessTemplate::parse("", &platform()).unwrap(),
            AccessTemplate::none()
        );
        assert!(AccessTemplate::none().describe().contains("unset"));
    }
}
