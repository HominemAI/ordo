//! Security levels for passkey touches (owner, 2026-11-05).
//!
//! One setting, `ORDO_SECURITY_LEVEL` = `low` | `standard` | `high`, sets
//! which situations ask a **person** for a passkey touch. Standard is the
//! default. A system (a machine key) is never asked at any level, and
//! sign-up never asks.
//!
//! | Situation | Low | Standard | High |
//! |---|---|---|---|
//! | Deploying, versions, deployment settings | off | off | on |
//! | Creating a tenant | off | off | on |
//! | Deleting or reaping a tenant | off | on | on |
//! | Console keys | on | on | on |
//! | Export and import | off | on | on |
//! | Membership and admission | off | off | on |
//! | Recovery and contests | on | on | on |
//! | Tenant storage and attestation | off | on | on |
//! | Platform reads | off | off | off |
//! | Console acting inside a tenant | on | on | on |
//! | Approving a delegation | off | on | on |
//! | Account security on the app's sign-in | off | on | on |
//! | Changing a contact email | off | on | on |
//! | Identity service account page | on | on | on |
//!
//! The table is [`asks`]. Which platform op falls in which situation is
//! [`situation_of`]. Every `std.deployment.*` op but the console keys is
//! the first row. Any other platform write it does not know asks at every
//! level, so an op added later asks until somebody places it.
//!
//! **The per-action settings still override the level** (*"again the dev
//! decides"*): `ORDO_STEP_UP_EXEMPT_OPS` takes a touch off named ops,
//! `ORDO_STEP_UP_REQUIRED_OPS` puts one on, `ORDO_PRESENCE_ON_READS=on`
//! asks on reads, and `ORDO_EMAIL_CHANGE_TOUCH` (and the app developer's
//! `setEmailPolicy`) decide the contact email. Platform reads are the
//! strict setting's alone. Console keys, recovery, a console acting inside a
//! tenant and the identity service's account page ask at every level, so
//! the level does not reach them.
//!
//! **A tenant may choose its own touches** where the app developer exposes
//! them (owner, 2026-11-05, [`crate::tenant_touches`]): only the situations
//! [`Situation::tenant_choosable`] names. The door reads the per-action
//! settings first, then the tenant's choice, then the level.

use std::sync::OnceLock;

/// The environment variable. Read once, at boot.
pub const ENV: &str = "ORDO_SECURITY_LEVEL";

/// How many situations ask a person for a touch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum SecurityLevel {
    /// Only console permissions and recovery ask.
    Low,
    /// The default: a touch where a stolen session does lasting harm that
    /// is hard to undo.
    #[default]
    Standard,
    /// Every platform change asks.
    High,
}

impl SecurityLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            SecurityLevel::Low => "low",
            SecurityLevel::Standard => "standard",
            SecurityLevel::High => "high",
        }
    }

    /// Read a value. Unset or empty is standard; any other word is a
    /// refusal that names the setting and its values.
    pub fn parse(value: Option<&str>) -> Result<SecurityLevel, String> {
        match value.map(str::trim) {
            None | Some("") | Some("standard") => Ok(SecurityLevel::Standard),
            Some("low") => Ok(SecurityLevel::Low),
            Some("high") => Ok(SecurityLevel::High),
            Some(other) => Err(format!(
                "`{other}` is not a level. Set {ENV}=low, standard (the default) or high"
            )),
        }
    }

    /// From a variable reader, as the boot reads every setting.
    pub fn from_vars(var: &dyn Fn(&str) -> Option<String>) -> Result<SecurityLevel, String> {
        SecurityLevel::parse(var(ENV).as_deref())
    }

    /// From the process environment.
    pub fn from_env() -> Result<SecurityLevel, String> {
        SecurityLevel::from_vars(&|k| std::env::var(k).ok())
    }

    /// Does this level ask a person for a touch in `situation`?
    pub fn asks(self, situation: Situation) -> bool {
        asks(self, situation)
    }
}

/// The situations of the ruling's table, in its order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Situation {
    Deploying,
    CreatingTenant,
    DeletingTenant,
    ConsoleKeys,
    ExportImport,
    Membership,
    Recovery,
    TenantStorage,
    PlatformReads,
    ConsoleInTenant,
    ApprovingDelegation,
    AccountSecurity,
    ContactEmail,
    IdentityAccountPage,
}

impl Situation {
    /// Every situation, in the table's order.
    pub const ALL: [Situation; 14] = [
        Situation::Deploying,
        Situation::CreatingTenant,
        Situation::DeletingTenant,
        Situation::ConsoleKeys,
        Situation::ExportImport,
        Situation::Membership,
        Situation::Recovery,
        Situation::TenantStorage,
        Situation::PlatformReads,
        Situation::ConsoleInTenant,
        Situation::ApprovingDelegation,
        Situation::AccountSecurity,
        Situation::ContactEmail,
        Situation::IdentityAccountPage,
    ];

    /// The situation's one word, as the door, the CLI and the console
    /// name it (`ordo touches expose membership both`).
    pub fn word(self) -> &'static str {
        match self {
            Situation::Deploying => "deploying",
            Situation::CreatingTenant => "creating-tenant",
            Situation::DeletingTenant => "deleting-tenant",
            Situation::ConsoleKeys => "console-keys",
            Situation::ExportImport => "export-import",
            Situation::Membership => "membership",
            Situation::Recovery => "recovery",
            Situation::TenantStorage => "tenant-storage",
            Situation::PlatformReads => "platform-reads",
            Situation::ConsoleInTenant => "console-in-tenant",
            Situation::ApprovingDelegation => "approving-delegation",
            Situation::AccountSecurity => "account-security",
            Situation::ContactEmail => "contact-email",
            Situation::IdentityAccountPage => "identity-account-page",
        }
    }

    /// A situation from its [`Situation::word`].
    pub fn parse(word: &str) -> Option<Situation> {
        Situation::ALL.into_iter().find(|s| s.word() == word.trim())
    }

    /// May the app developer let a tenant choose this touch for its own
    /// space (owner, 2026-11-05, `tenant-chooses-its-touches`)? Only the
    /// situations whose ops act inside one tenant, through the door.
    ///
    /// Never: the acts on the deployment (deploying, creating, deleting or
    /// reaping a tenant, console keys, a console inside a tenant, the
    /// identity service's page), recovery (a stolen session must not be able
    /// to lower the touch that guards taking an account over), platform
    /// reads (the strict setting's alone), and a person's own contact email
    /// (one person, many tenants: no one tenant decides it).
    pub fn tenant_choosable(self) -> bool {
        matches!(
            self,
            Situation::ExportImport
                | Situation::Membership
                | Situation::TenantStorage
                | Situation::ApprovingDelegation
                | Situation::AccountSecurity
        )
    }

    /// The table's words, for the boot log.
    pub fn label(self) -> &'static str {
        match self {
            Situation::Deploying => "deploying, versions, deployment settings",
            Situation::CreatingTenant => "creating a tenant",
            Situation::DeletingTenant => "deleting or reaping a tenant",
            Situation::ConsoleKeys => "console keys",
            Situation::ExportImport => "export and import",
            Situation::Membership => "membership and admission",
            Situation::Recovery => "recovery and contests",
            Situation::TenantStorage => "tenant storage and attestation",
            Situation::PlatformReads => "platform reads",
            Situation::ConsoleInTenant => "console acting inside a tenant",
            Situation::ApprovingDelegation => "approving a delegation",
            Situation::AccountSecurity => "account security on the app's sign-in",
            Situation::ContactEmail => "changing a contact email",
            Situation::IdentityAccountPage => "identity service account page",
        }
    }
}

/// The ruling's table.
pub fn asks(level: SecurityLevel, situation: Situation) -> bool {
    use SecurityLevel::*;
    use Situation::*;
    match situation {
        ConsoleKeys | Recovery | ConsoleInTenant | IdentityAccountPage => true,
        PlatformReads => false,
        Deploying | CreatingTenant | Membership => level == High,
        DeletingTenant | ExportImport | TenantStorage | ApprovingDelegation | AccountSecurity
        | ContactEmail => level >= Standard,
    }
}

/// The situation a platform op that changes something falls in. `None` for
/// an op this table does not place: it asks at every level.
pub fn situation_of(op: &str) -> Option<Situation> {
    use Situation::*;
    let (family, name) = op.rsplit_once('.')?;
    Some(match (family, name) {
        ("std.deployment", "addConsoleKey" | "revokeConsoleKey") => ConsoleKeys,
        // Every other act on the deployment is deploying, versions or the
        // deployment's settings, the ruling's first row. Moving a tenant's
        // version (`pin`) asks no touch at any level
        // (`deploy_ops::NO_PRESENCE`).
        ("std.deployment", _) => Deploying,
        // A tenant's own move off a revoked version, when the developer
        // grants it. Like every move it asks no touch (`NO_PRESENCE`).
        ("std.space", "moveOffRevoked") => Deploying,
        ("std.space", "create") => CreatingTenant,
        ("std.space", "delete" | "reap") => DeletingTenant,
        ("std.space", "export" | "import" | "download") => ExportImport,
        ("std.space", "attachBucket" | "detachBucket" | "attestTo") => TenantStorage,
        (
            "std.space",
            "recoverMember"
            | "approveMemberRecovery"
            | "setRecoveryPolicy"
            | "resetCredential"
            | "setTemporaryPassword"
            | "objectToCase",
        ) => Recovery,
        ("std.identity", _) => Recovery,
        ("std.delegation", "approve") => ApprovingDelegation,
        // Who is in the tenant, what they may call, who may come in and how,
        // what is shared, and the tenant's own settings.
        (
            "std.space",
            "admit"
            | "setRole"
            | "remove"
            | "invite"
            | "revokeInvitation"
            | "suspend"
            | "reinstate"
            | "leave"
            | "setAdmission"
            | "createGroup"
            | "deleteGroup"
            | "addToGroup"
            | "removeFromGroup"
            | "createPerson"
            | "setMemberAccess"
            | "setOpAccess"
            | "resetOpAccess"
            | "share"
            | "shareLink"
            | "revokeShare"
            | "revokeLink"
            | "setLinkMaxOpens"
            | "setLinkPassword"
            | "setShareLinkLifetime"
            | "setLinkPasswordLimits"
            | "setPasswordSlowdown"
            | "setSessionPolicy"
            | "pause"
            | "resume",
        ) => Membership,
        // A tenant's own ways in and the pages that may call it. A stolen
        // session that adds a provider or an origin keeps a way in after the
        // session ends: lasting harm, so it reads as account security.
        ("std.space", "addConnection" | "updateConnection" | "retireConnection" | "setOrigins") => {
            AccountSecurity
        }
        // A tenant's own touches: lowering one is lasting harm a stolen
        // session could do, so it reads as account security. The tenant's
        // own choice never reaches this op (`tenant_touches::NEVER_CHOSEN`).
        ("std.space", "setTouch") => AccountSecurity,
        // Taking over a tenant's ownership is the succession half of
        // recovery (owner succession, 2026-10-24).
        ("std.space", "succeedOwner") => Recovery,
        _ => return None,
    })
}

/// Does `level` ask a person for a touch on this platform write? An op
/// [`situation_of`] does not place asks.
pub fn asks_for_op(level: SecurityLevel, op: &str) -> bool {
    situation_of(op).is_none_or(|s| asks(level, s))
}

/// The boot log's line: the level and what it asks for.
pub fn describe(level: SecurityLevel) -> String {
    let (on, off): (Vec<Situation>, Vec<Situation>) =
        Situation::ALL.iter().partition(|s| asks(level, **s));
    let words = |v: &[Situation]| v.iter().map(|s| s.label()).collect::<Vec<_>>().join("; ");
    format!(
        "{} ({ENV}); a touch asked for: {}; not asked for: {}; a system is never asked",
        level.as_str(),
        words(&on),
        words(&off)
    )
}

static INSTALLED: OnceLock<SecurityLevel> = OnceLock::new();

/// Install the deployment's level. Once, at boot, before any engine is
/// built, for [`crate::presence::install`]'s reason.
pub fn install(level: SecurityLevel) {
    let _ = INSTALLED.set(level);
}

/// The installed level, or standard.
pub fn current() -> SecurityLevel {
    INSTALLED.get().copied().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_values_and_the_refusal() {
        assert_eq!(SecurityLevel::parse(None), Ok(SecurityLevel::Standard));
        assert_eq!(SecurityLevel::parse(Some(" low ")), Ok(SecurityLevel::Low));
        assert_eq!(SecurityLevel::parse(Some("high")), Ok(SecurityLevel::High));
        let err = SecurityLevel::parse(Some("paranoid")).unwrap_err();
        assert!(err.contains("ORDO_SECURITY_LEVEL=low, standard (the default) or high"));
    }

    #[test]
    fn the_table_row_by_row() {
        use SecurityLevel::*;
        use Situation::*;
        let row = |s| [asks(Low, s), asks(Standard, s), asks(High, s)];
        assert_eq!(row(Deploying), [false, false, true]);
        assert_eq!(row(CreatingTenant), [false, false, true]);
        assert_eq!(row(DeletingTenant), [false, true, true]);
        assert_eq!(row(ConsoleKeys), [true, true, true]);
        assert_eq!(row(ExportImport), [false, true, true]);
        assert_eq!(row(Membership), [false, false, true]);
        assert_eq!(row(Recovery), [true, true, true]);
        assert_eq!(row(TenantStorage), [false, true, true]);
        assert_eq!(row(PlatformReads), [false, false, false]);
        assert_eq!(row(ConsoleInTenant), [true, true, true]);
        assert_eq!(row(ApprovingDelegation), [false, true, true]);
        assert_eq!(row(AccountSecurity), [false, true, true]);
        assert_eq!(row(ContactEmail), [false, true, true]);
        assert_eq!(row(IdentityAccountPage), [true, true, true]);
    }

    #[test]
    fn every_platform_write_is_placed() {
        let unplaced: Vec<String> = crate::platform_ops::specs()
            .into_iter()
            .filter(|s| s.kind != ordo_api::OpKind::Read)
            .filter(|s| situation_of(&s.name).is_none())
            .filter(|s| {
                !crate::consent::any_member(&s.name) || crate::consent::asks_presence(&s.name)
            })
            .map(|s| s.name)
            .collect();
        assert!(unplaced.is_empty(), "place these: {unplaced:?}");
    }

    #[test]
    fn an_unplaced_op_asks_at_every_level() {
        for level in [
            SecurityLevel::Low,
            SecurityLevel::Standard,
            SecurityLevel::High,
        ] {
            assert!(asks_for_op(level, "std.space.somethingNew"));
        }
        assert!(!asks_for_op(
            SecurityLevel::Standard,
            "std.deployment.apply"
        ));
        assert!(asks_for_op(SecurityLevel::Standard, "std.space.delete"));
        assert!(asks_for_op(
            SecurityLevel::Low,
            "std.deployment.addConsoleKey"
        ));
    }

    #[test]
    fn only_situations_inside_a_tenant_are_choosable() {
        use Situation::*;
        let choosable: Vec<Situation> = Situation::ALL
            .into_iter()
            .filter(|s| s.tenant_choosable())
            .collect();
        assert_eq!(
            choosable,
            [
                ExportImport,
                Membership,
                TenantStorage,
                ApprovingDelegation,
                AccountSecurity
            ]
        );
        for s in Situation::ALL {
            assert_eq!(Situation::parse(s.word()), Some(s));
        }
        assert_eq!(Situation::parse("everything"), None);
    }

    #[test]
    fn the_boot_line_names_the_level() {
        let line = describe(SecurityLevel::Low);
        assert!(line.starts_with("low (ORDO_SECURITY_LEVEL)"), "{line}");
        assert!(line.contains("a system is never asked"));
    }
}
