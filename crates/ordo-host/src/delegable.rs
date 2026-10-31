//! Which ops a delegated token may call: a deployment setting with a safe
//! default (SH-20, `plan-delegation-surface.md` §6, checklist 11-4d).
//!
//! # The default
//!
//! A token that acts for a person (a worker, an agent, a server function
//! holding a delegation) may call what its allowlist names and its delegator
//! could call, **except**:
//!
//! - **every op that deletes a row** ([`ordo_api::OpSpec::deletes_rows`]:
//!   the app's own, the derived `std.<entity>.delete`, and an op whose body
//!   deletes beside a `set`). SH-20: *"destructive ops non-delegable by
//!   default, opt-in per op listed individually"*.
//! - **every platform op that writes** (`std.space.*`, `std.person.*` and
//!   the rest of [`crate::platform_ops::specs`]). These write membership and
//!   tenant state through their own paths, which record the delegator alone.
//!   They stay closed until those paths can name the agent too, so this
//!   setting cannot open them.
//!
//! Platform ops also require presence by default, which no delegated token
//! can ever carry. That is a second, independent refusal, not the reason.
//!
//! # The setting
//!
//! The app developer opens a delete op to delegation by naming it:
//!
//! ```text
//! ORDO_DELEGABLE_OPS=purgeDone,std.tasks.delete
//! ```
//!
//! Exact names only, with no prefixes: opening a delete to an agent is a
//! decision about one op, and a pattern would open ops added later that
//! nobody looked at. Unset or empty is the default above. A name that is
//! not in the running program does nothing, because the program changes
//! with each deploy and the setting does not. A name that is a platform op
//! refuses the boot, because it cannot be honoured.

use std::sync::OnceLock;

/// The environment variable. Read once, at boot.
pub const ENV: &str = "ORDO_DELEGABLE_OPS";

/// The delete ops the app developer opened to delegated tokens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DelegablePolicy {
    open: Vec<String>,
}

impl DelegablePolicy {
    /// The default: no delete op is open to delegation.
    pub fn closed() -> DelegablePolicy {
        DelegablePolicy::default()
    }

    /// Read a list of op names. `platform` is the platform ops, which this
    /// setting cannot open; naming one is a refusal that says why.
    pub fn parse(list: &str, platform: &[String]) -> Result<DelegablePolicy, String> {
        let mut open = Vec::new();
        for entry in list.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            if entry.contains('*') {
                return Err(format!(
                    "`{entry}`: name each op exactly. Opening a delete to an agent is a \
                     decision about one op, so patterns are not accepted"
                ));
            }
            if platform.iter().any(|p| p == entry) {
                return Err(format!(
                    "`{entry}` is a platform op. Platform writes stay closed to delegated \
                     tokens, because their record would name only the delegator"
                ));
            }
            if !open.iter().any(|e| e == entry) {
                open.push(entry.to_string());
            }
        }
        Ok(DelegablePolicy { open })
    }

    /// From [`ENV`]. Unset is the default.
    pub fn from_env(platform: &[String]) -> Result<DelegablePolicy, String> {
        match std::env::var(ENV) {
            Ok(list) => DelegablePolicy::parse(&list, platform),
            Err(_) => Ok(DelegablePolicy::closed()),
        }
    }

    /// Did the app developer open this delete op to delegation?
    pub fn opens(&self, op: &str) -> bool {
        self.open.iter().any(|o| o == op)
    }

    /// The names as written, for the census.
    pub fn open(&self) -> &[String] {
        &self.open
    }

    /// One line for the boot census.
    pub fn describe(&self) -> String {
        if self.open.is_empty() {
            "no delete open to delegated tokens (ORDO_DELEGABLE_OPS unset)".to_string()
        } else {
            format!("deletes open to delegated tokens: {}", self.open.join(", "))
        }
    }
}

/// Close to delegated tokens what this policy leaves closed: every delete
/// among `app` it does not name, and every platform op in `platform` that
/// is not a read. Reads and ordinary writes stay delegable, bounded as
/// always by the token's allowlist and the delegator's reach.
pub fn close(
    engine: &mut ordo_identity::AccessEngine,
    app: &[ordo_api::OpSpec],
    platform: &[ordo_api::OpSpec],
    policy: &DelegablePolicy,
) {
    for s in app {
        // Every op that deletes a row, not only the ones whose every
        // statement is a `del`: an op that sets one row and deletes another
        // is a `Write` on the wire, and it stayed open (audit 2026-10-08).
        if s.deletes_rows() && !policy.opens(&s.name) {
            engine.forbid_delegation(&s.name);
        }
    }
    for s in platform {
        // The consent ops, reads included: an agent never reviews, approves
        // or lists a delegation in its delegator's name (11-4b).
        if s.kind != ordo_api::OpKind::Read || crate::consent::any_member(&s.name) {
            engine.forbid_delegation(&s.name);
        }
    }
}

static INSTALLED: OnceLock<DelegablePolicy> = OnceLock::new();

/// Install the deployment's policy. Called once at boot, before any engine
/// is built; a second call is ignored, for [`crate::presence::install`]'s
/// reason.
pub fn install(policy: DelegablePolicy) {
    let _ = INSTALLED.set(policy);
}

/// The policy engines are built under: what was installed, or the default.
pub fn current() -> DelegablePolicy {
    INSTALLED.get().cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn platform() -> Vec<String> {
        vec!["std.space.leave".to_string()]
    }

    #[test]
    fn unset_opens_nothing() {
        assert!(!DelegablePolicy::closed().opens("purgeDone"));
    }

    #[test]
    fn a_named_delete_is_opened_and_nothing_else() {
        let p = DelegablePolicy::parse(" purgeDone , std.tasks.delete,", &platform()).unwrap();
        assert!(p.opens("purgeDone"));
        assert!(p.opens("std.tasks.delete"));
        assert!(!p.opens("std.cards.delete"));
    }

    #[test]
    fn a_platform_op_or_a_pattern_refuses_the_boot() {
        assert!(DelegablePolicy::parse("std.space.leave", &platform()).is_err());
        assert!(DelegablePolicy::parse("std.*", &platform()).is_err());
    }

    /// SH-20 reaches every op that deletes a row, not only the ops whose
    /// every statement is a `del`. An op that sets one row and deletes
    /// another is a `Write` on the wire, and it used to stay open to a
    /// delegated token by default (audit 2026-10-08).
    #[test]
    fn an_op_that_sets_and_deletes_is_closed_to_delegation_like_a_delete() {
        let src = "domain shop\n\
                   entity orders {\n  status: text\n}\n\
                   entity drafts {\n  status: text\n}\n\
                   op archive($o: id, $d: id) =\n  \
                     set orders(id == $o) { status: \"archived\" }\n  \
                     del drafts(id == $d)\n\
                   op purge($d: id) =\n  del drafts(id == $d)\n\
                   op rename($o: id) =\n  set orders(id == $o) { status: \"renamed\" }\n";
        let f = ordo_conformance::Fixture::from_source(src, "").expect("compiles");
        let specs = crate::build_specs(&f);
        let mut engine = ordo_identity::AccessEngine::new();
        close(&mut engine, &specs, &[], &DelegablePolicy::closed());
        assert!(!engine.delegable("purge"), "a pure delete is closed");
        assert!(
            !engine.delegable("archive"),
            "an op with a del statement among its sets is closed too"
        );
        assert!(engine.delegable("rename"), "an ordinary write stays open");

        let mut engine = ordo_identity::AccessEngine::new();
        let opened = DelegablePolicy::parse("archive", &[]).unwrap();
        close(&mut engine, &specs, &[], &opened);
        assert!(engine.delegable("archive"), "opened by name");
        assert!(!engine.delegable("purge"), "only the op named");
    }
}
