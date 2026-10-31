//! Where a platform op requires presence: a deployment setting.
//!
//! # The ruling
//!
//! Owner, 2026-09-29, first *"THis is not ours to decide - that is for the
//! developer"* about the membership and admission ops, then *"What is
//! enforced is app design ... not our design"* about all of them. So which
//! platform ops demand a fresh passkey assertion is the app developer's
//! choice, for every one of them, including `std.deployment.apply`,
//! `std.space.create`, `std.space.import`, `std.space.export` and
//! `std.space.delete`. `docs/security-logging-design.md` §2.3 records it.
//!
//! **What stays the platform's.** Step-up is still the one mechanism for
//! "not delegable", presence is still minted only by a verified passkey
//! assertion (`UserPresence` has no other producer), and the freshness
//! window is still the engine's. This setting says only *where presence is
//! required*.
//!
//! **A provider's two-step claim** (`plan-auth-federated.md` §8.2, step 10)
//! is the one other thing that satisfies it, and only where the app
//! developer's `ORDO_AUTH_<NAME>_PRESENCE=on` or, for its own data, the
//! tenant's `presenceFrom` say so. A tenant may name any connection (ask 10). [`acceptance`] builds what the engine asks, and
//! [`DEPLOYMENT_ACTS`] are the ops a tenant's choice never reaches.
//!
//! # The shape
//!
//! One environment list, of the ops that **do not** require presence:
//!
//! ```text
//! ORDO_STEP_UP_EXEMPT_OPS=std.space.admit,std.space.setRole,std.space.remove
//! ORDO_STEP_UP_EXEMPT_OPS=std.space.*
//! ```
//!
//! An entry is an exact op name, or a prefix ending `.*`, which names every
//! platform op that begins with it. **Unset or empty is the default:
//! presence required on every platform op that changes something.** Chosen over a per-op table of
//! values because the default has to be the safe one and a list of
//! exceptions says exactly what was loosened: reading the variable is
//! reading the departure from the default. A list of exceptions also cannot
//! silently tighten or loosen an op added later: a new platform op requires
//! presence until somebody names it.
//!
//! # Signed in means authenticated
//!
//! Owner, 2026-10-22: *"imagine having to touch your passkey 30 times
//! everytime you refresh the page. If you are signed in = that passkey was
//! your credentials - you are authenticated."* So a platform op that only
//! reads asks for no touch. Read or change is the op's own declaration, its
//! [`ordo_api::OpKind`]: the same field that makes it a `GET`, so a read
//! cannot change state without also being served on the wrong verb. A
//! change keeps the rules above.
//!
//! A strict deployment asks for presence on reads too:
//!
//! ```text
//! ORDO_PRESENCE_ON_READS=on
//! ```
//!
//! A read freed this way is still not delegable. Before this ruling the
//! touch is what kept a capability token off `callersOf` and its kin, so
//! the engine is told so directly (`deployment_engine`'s loop). A read the
//! app developer names in [`ENV`] is delegable, as it always was.
//!
//! **Two ops are never exempt**: `std.deployment.addConsoleKey` and
//! `std.deployment.revokeConsoleKey` ([`crate::console_keys::NEVER_EXEMPT`]).
//! Who holds a console key decides who can sign in as the deployment owner,
//! so it takes the owner's own touch whatever the list says. A prefix such
//! as `std.deployment.*` leaves them out; naming one exactly refuses the
//! boot and says why.
//!
//! An entry that names no platform op refuses the boot, listing the names
//! that exist. A misspelt exemption would otherwise leave the op requiring
//! presence while its owner believes it does not.
//!
//! # What it does not touch
//!
//! The role gate is unchanged. An exempt op is still refused to a caller the
//! engine does not list for it, and the owner-only ops stay owner-only.
//! Only the second half of the check, the presence, is removed. **An exempt
//! op is also one a capability token may be granted**, because the
//! structural rule that keeps a delegation from satisfying step-up has
//! nothing to say about an op that asks for none.

use std::sync::OnceLock;

/// The environment variable. Read once, at boot.
pub const ENV: &str = "ORDO_STEP_UP_EXEMPT_OPS";

/// The strict setting: `on` asks for presence on reads too. Default `off`.
pub const READS_ENV: &str = "ORDO_PRESENCE_ON_READS";

/// Where platform ops require presence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PresencePolicy {
    exempt: Vec<String>,
    /// Presence on reads too ([`READS_ENV`]). Off by default.
    reads: bool,
}

fn matches(pattern: &str, op: &str) -> bool {
    match pattern.strip_suffix(".*") {
        Some(prefix) => {
            op.len() > prefix.len() + 1
                && op.starts_with(prefix)
                && op[prefix.len()..].starts_with('.')
        }
        None => pattern == op,
    }
}

impl PresencePolicy {
    /// Every platform op requires presence, reads included: the strict
    /// deployment, `ORDO_PRESENCE_ON_READS=on` with no exemptions.
    pub fn everything_required() -> PresencePolicy {
        PresencePolicy {
            exempt: Vec::new(),
            reads: true,
        }
    }

    /// What a deployment that sets nothing gets: every change requires
    /// presence, no read does.
    pub fn standard() -> PresencePolicy {
        PresencePolicy::default()
    }

    /// The same policy with presence on reads set to `on` or `off`.
    pub fn with_reads(mut self, on: bool) -> PresencePolicy {
        self.reads = on;
        self
    }

    /// Read a list of entries, checking each against the platform ops that
    /// exist (`known`). An entry that names none is a refusal that says what
    /// does exist. Reads are left free; see [`PresencePolicy::with_reads`].
    pub fn parse(list: &str, known: &[String]) -> Result<PresencePolicy, String> {
        let mut exempt = Vec::new();
        for entry in list.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            if crate::console_keys::NEVER_EXEMPT.contains(&entry) {
                return Err(format!(
                    "`{entry}` can never be exempt: who holds a console key decides who can \
                     sign in as the deployment owner, so it always takes the owner's touch. \
                     Remove `{entry}` from {ENV}"
                ));
            }
            if !known.iter().any(|k| matches(entry, k)) {
                let mut names = known.to_vec();
                names.sort();
                return Err(format!(
                    "`{entry}` names no platform op. Use an exact name or a prefix ending \
                     `.*`. The platform ops are: {}",
                    names.join(", ")
                ));
            }
            if !exempt.iter().any(|e| e == entry) {
                exempt.push(entry.to_string());
            }
        }
        Ok(PresencePolicy {
            exempt,
            reads: false,
        })
    }

    /// From [`ENV`]. Unset is the default. [`READS_ENV`] is read apart, by
    /// [`reads_from_env`], so each refusal names its own setting.
    pub fn from_env(known: &[String]) -> Result<PresencePolicy, String> {
        match std::env::var(ENV) {
            Ok(list) => PresencePolicy::parse(&list, known),
            Err(_) => Ok(PresencePolicy::standard()),
        }
    }

    /// Does this platform op demand a fresh passkey assertion? A read asks
    /// for none unless the deployment is strict.
    pub fn requires(&self, op: &str, kind: ordo_api::OpKind) -> bool {
        if kind == ordo_api::OpKind::Read && !self.reads {
            return false;
        }
        !self.exempts(op)
    }

    /// Does the app developer's list name this op? Never for
    /// [`crate::console_keys::NEVER_EXEMPT`].
    pub fn exempts(&self, op: &str) -> bool {
        !crate::console_keys::NEVER_EXEMPT.contains(&op)
            && self.exempt.iter().any(|p| matches(p, op))
    }

    /// Is presence asked on reads too?
    pub fn reads_require_presence(&self) -> bool {
        self.reads
    }

    /// The entries as written, for the census.
    pub fn exempt(&self) -> &[String] {
        &self.exempt
    }

    /// The boot banner's line.
    pub fn describe(&self) -> String {
        let reads = if self.reads {
            "required on reads too (ORDO_PRESENCE_ON_READS=on)"
        } else {
            "not required on reads (ORDO_PRESENCE_ON_READS=on requires it)"
        };
        if self.exempt.is_empty() {
            format!("required on every platform op that changes something; {reads}")
        } else {
            format!(
                "NOT required on: {} (ORDO_STEP_UP_EXEMPT_OPS); required on every other \
                 platform op that changes something; {reads}",
                self.exempt.join(", ")
            )
        }
    }
}

/// [`READS_ENV`]: `on` or `off`; unset is `off`. Any other value is a
/// refusal that names the setting's values.
pub fn reads_from_env() -> Result<bool, String> {
    match std::env::var(READS_ENV) {
        Err(_) => Ok(false),
        Ok(v) => match v.trim() {
            "on" => Ok(true),
            "off" | "" => Ok(false),
            other => Err(format!(
                "`{other}` is not a value. Set {READS_ENV}=on to ask for presence on \
                 platform reads too, or off (the default) to ask on changes only"
            )),
        },
    }
}

static INSTALLED: OnceLock<PresencePolicy> = OnceLock::new();

/// Install the deployment's policy. Called once by `assemble`, before any
/// engine is built; a second call is ignored, because an engine built under
/// one policy and rebuilt under another is a presence requirement that
/// changes while the server runs.
pub fn install(policy: PresencePolicy) {
    let _ = INSTALLED.set(policy);
}

/// The policy engines are built under: what was installed, or the default.
pub fn current() -> PresencePolicy {
    INSTALLED.get().cloned().unwrap_or_default()
}

// ── a provider's two-step claim (`plan-auth-federated.md` §8.2) ─────────

/// The acts on the deployment itself. A tenant's acceptance of a provider's
/// two-step claim never reaches them, in whatever space they are called;
/// only the app developer's `on` does. Every op in the deployment's own
/// space is one as well.
pub const DEPLOYMENT_ACTS: &[&str] = &[
    "std.deployment.*",
    "std.space.create",
    "std.space.delete",
    "std.space.reap",
];

/// The acceptance every engine is built with, from the OIDC connections'
/// settings. A connection not configured is `off`.
pub fn acceptance<'a>(
    connections: impl IntoIterator<Item = &'a ordo_identity::oidc::OidcConnection>,
    deployment_space: &str,
) -> ordo_identity::UpstreamAcceptance {
    let connections: Vec<&ordo_identity::oidc::OidcConnection> = connections.into_iter().collect();
    // One deployment-wide window (`ORDO_AUTH_UPSTREAM_PRESENCE_SECS`), read
    // into every connection's rule. The shortest wins if they ever differ.
    let max_age_secs = connections
        .iter()
        .map(|c| c.presence.max_age_secs)
        .min()
        .unwrap_or(ordo_identity::upstream::DEFAULT_MAX_AGE_SECS);
    let mut a = ordo_identity::UpstreamAcceptance::new()
        .with_deployment_space(ordo_identity::SpaceId(deployment_space.to_string()))
        .with_deployment_ops(DEPLOYMENT_ACTS)
        .with_max_age(max_age_secs);
    for c in &connections {
        a = a.with_connection(&c.name, c.presence.mode);
    }
    a
}

static UPSTREAM: OnceLock<std::sync::Arc<ordo_identity::UpstreamAcceptance>> = OnceLock::new();

/// Install the deployment's acceptance. Once, by `assemble`, before any
/// engine is built, for the reason [`install`] gives.
pub fn install_upstream(acceptance: std::sync::Arc<ordo_identity::UpstreamAcceptance>) {
    let _ = UPSTREAM.set(acceptance);
}

/// The acceptance engines are built with. `None`, the default, is no
/// provider's claim counting anywhere.
pub fn upstream() -> Option<std::sync::Arc<ordo_identity::UpstreamAcceptance>> {
    UPSTREAM.get().cloned()
}
