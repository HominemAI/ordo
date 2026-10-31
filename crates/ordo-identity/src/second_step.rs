//! The second step: what it is, and when a sign-in owes one
//! (`plan-second-factor.md` §5).
//!
//! A second step proves possession of a second thing after a first thing was
//! proved. It changes who can **sign in**. It does not change who can **step
//! up**: presence comes only from a first-factor passkey, and nothing in this
//! module can put presence anywhere.
//!
//! # Mechanisms here, decisions elsewhere
//!
//! This module holds the rule that turns the levels into an answer. It holds
//! no default the app developer cannot move: the deployment's mode is an input,
//! read by the host from its configuration, and so is a tenant's and a
//! person's. Where a level is tighter than the one above it, the tighter one
//! wins. A level can never loosen the one above: a tenant cannot require what
//! the deployment turned `off`, and cannot relax what the deployment requires.

use crate::principal::AuthMethodKind;

/// Which second factor proved a sign-in. Written into the token's session
/// claim so the door can tell a session that took the second step from one
/// that did not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SecondStepKind {
    /// A code from an authenticator app or a code-generating token (RFC 6238).
    Totp,
    /// A WebAuthn security key registered as a second factor. It never signs
    /// in alone and never carries presence.
    SecurityKey,
    /// One of the person's one-time backup codes.
    BackupCode,
}

impl SecondStepKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SecondStepKind::Totp => "totp",
            SecondStepKind::SecurityKey => "security-key",
            SecondStepKind::BackupCode => "backup-code",
        }
    }

    pub fn parse(s: &str) -> Option<SecondStepKind> {
        match s {
            "totp" => Some(SecondStepKind::Totp),
            "security-key" => Some(SecondStepKind::SecurityKey),
            "backup-code" => Some(SecondStepKind::BackupCode),
            _ => None,
        }
    }
}

/// What the app developer set for the deployment (`ORDO_AUTH_SECOND_FACTOR`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeploymentMode {
    /// No second step anywhere, and no factor can be added.
    Off,
    /// A person may add a factor and is then asked for it. Nobody is forced.
    #[default]
    Enabled,
    /// Every sign-in needs a second step, passkeys included.
    Required,
    /// Every password, code or provider sign-in needs one. A passkey does not.
    RequiredForNonPasskey,
}

impl DeploymentMode {
    pub fn parse(word: &str) -> Option<DeploymentMode> {
        match word {
            "off" => Some(DeploymentMode::Off),
            "enabled" => Some(DeploymentMode::Enabled),
            "required" => Some(DeploymentMode::Required),
            "required-for-non-passkey" => Some(DeploymentMode::RequiredForNonPasskey),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DeploymentMode::Off => "off",
            DeploymentMode::Enabled => "enabled",
            DeploymentMode::Required => "required",
            DeploymentMode::RequiredForNonPasskey => "required-for-non-passkey",
        }
    }
}

/// What a tenant chose for its own sign-ins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TenantMode {
    /// Whatever the deployment says.
    #[default]
    Inherit,
    Required,
    RequiredForNonPasskey,
}

impl TenantMode {
    pub fn parse(word: &str) -> Option<TenantMode> {
        match word {
            "inherit" => Some(TenantMode::Inherit),
            "required" => Some(TenantMode::Required),
            "required-for-non-passkey" => Some(TenantMode::RequiredForNonPasskey),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TenantMode::Inherit => "inherit",
            TenantMode::Required => "required",
            TenantMode::RequiredForNonPasskey => "required-for-non-passkey",
        }
    }
}

impl TenantMode {
    /// Whether a tenant may choose this under the deployment's setting.
    /// Tightening only: a tenant cannot require what the app developer turned
    /// off (nobody could then enrol), and cannot loosen what the app developer
    /// requires of every sign-in.
    pub fn within(self, deployment: DeploymentMode) -> Result<(), String> {
        match (deployment, self) {
            (_, TenantMode::Inherit) => Ok(()),
            (DeploymentMode::Off, _) => Err(
                "the app developer turned the second step off for this deployment, so a \
                 tenant cannot require it"
                    .to_string(),
            ),
            (DeploymentMode::Required, TenantMode::RequiredForNonPasskey) => Err(
                "the app developer requires a second step of every sign-in, passkeys \
                 included, and a tenant cannot loosen that"
                    .to_string(),
            ),
            _ => Ok(()),
        }
    }
}

/// What a person chose for themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PersonMode {
    /// Any sign-in that is not a passkey gets the step once a factor exists.
    #[default]
    WhenEnrolled,
    /// A passkey sign-in gets the step too.
    AlsoForPasskeys,
}

impl PersonMode {
    pub fn parse(word: &str) -> Option<PersonMode> {
        match word {
            "when-enrolled" => Some(PersonMode::WhenEnrolled),
            "also-for-passkeys" => Some(PersonMode::AlsoForPasskeys),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PersonMode::WhenEnrolled => "when-enrolled",
            PersonMode::AlsoForPasskeys => "also-for-passkeys",
        }
    }
}

/// Everything the decision reads.
#[derive(Debug, Clone)]
pub struct Facts<'a> {
    pub deployment: DeploymentMode,
    /// The tenant named at sign-in, when one was and it has said something.
    pub tenant: TenantMode,
    pub person: PersonMode,
    /// The first factor that just succeeded.
    pub first: AuthMethodKind,
    /// The factors the person holds and has not had disabled. Backup codes are
    /// not a factor: they stand in for one.
    pub enrolled: &'a [SecondStepKind],
    /// Whether the person holds unused backup codes.
    pub has_backup_codes: bool,
}

/// What a sign-in owes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owes {
    /// Mint the token.
    Nothing,
    /// Prove one of these before a token is minted.
    Step(Vec<SecondStepKind>),
    /// A second step is required and the person has no factor: enrol one
    /// first. Nobody is signed in without a factor, and nobody is locked out
    /// for lacking one.
    Enrol,
}

/// The rule.
///
/// * `off` at the deployment ends it: nothing is owed and nothing can be added.
/// * A **passkey** sign-in owes a step only when a level says so for
///   passkeys: the deployment's `required`, a tenant's `required`, or the
///   person's `also-for-passkeys`. Default off, because a passkey already needs
///   the device and its unlock.
/// * Any other first factor owes a step when the person holds a factor, and
///   owes an enrolment when a level requires one and they hold none.
pub fn decide(f: &Facts<'_>) -> Owes {
    if f.deployment == DeploymentMode::Off {
        return Owes::Nothing;
    }
    let is_passkey = f.first == AuthMethodKind::Passkey;
    let required = match (f.deployment, f.tenant) {
        (DeploymentMode::Required, _) | (_, TenantMode::Required) => true,
        (DeploymentMode::RequiredForNonPasskey, _) | (_, TenantMode::RequiredForNonPasskey) => {
            !is_passkey
        }
        _ => false,
    };
    let holds = !f.enrolled.is_empty();
    let applies = if is_passkey {
        required || (holds && f.person == PersonMode::AlsoForPasskeys)
    } else {
        required || holds
    };
    if !applies {
        return Owes::Nothing;
    }
    if !holds {
        return Owes::Enrol;
    }
    let mut methods = f.enrolled.to_vec();
    methods.sort();
    methods.dedup();
    if f.has_backup_codes {
        methods.push(SecondStepKind::BackupCode);
    }
    Owes::Step(methods)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(
        deployment: DeploymentMode,
        tenant: TenantMode,
        person: PersonMode,
        first: AuthMethodKind,
        enrolled: &'a [SecondStepKind],
    ) -> Facts<'a> {
        Facts {
            deployment,
            tenant,
            person,
            first,
            enrolled,
            has_backup_codes: false,
        }
    }

    #[test]
    fn a_tenant_can_only_tighten() {
        use DeploymentMode as D;
        use TenantMode as T;
        assert!(T::Inherit.within(D::Off).is_ok());
        assert!(T::Required.within(D::Off).is_err());
        assert!(T::RequiredForNonPasskey.within(D::Off).is_err());
        assert!(T::Required.within(D::Enabled).is_ok());
        assert!(T::RequiredForNonPasskey.within(D::Enabled).is_ok());
        assert!(T::Required.within(D::RequiredForNonPasskey).is_ok());
        assert!(T::RequiredForNonPasskey.within(D::Required).is_err());
        assert!(T::Required.within(D::Required).is_ok());
    }

    #[test]
    fn off_at_the_deployment_ends_it_whatever_a_tenant_or_a_person_says() {
        let owes = decide(&facts(
            DeploymentMode::Off,
            TenantMode::Required,
            PersonMode::AlsoForPasskeys,
            AuthMethodKind::Password,
            &[SecondStepKind::Totp],
        ));
        assert_eq!(owes, Owes::Nothing);
    }

    #[test]
    fn a_password_with_a_factor_owes_a_step_and_one_without_owes_nothing() {
        let with = decide(&facts(
            DeploymentMode::Enabled,
            TenantMode::Inherit,
            PersonMode::WhenEnrolled,
            AuthMethodKind::Password,
            &[SecondStepKind::Totp],
        ));
        assert_eq!(with, Owes::Step(vec![SecondStepKind::Totp]));
        let without = decide(&facts(
            DeploymentMode::Enabled,
            TenantMode::Inherit,
            PersonMode::WhenEnrolled,
            AuthMethodKind::Password,
            &[],
        ));
        assert_eq!(without, Owes::Nothing);
    }

    #[test]
    fn a_passkey_is_exempt_unless_a_level_says_otherwise() {
        let on = [SecondStepKind::Totp];
        let d = |dep, ten, per| {
            decide(&facts(dep, ten, per, AuthMethodKind::Passkey, &on)) != Owes::Nothing
        };
        assert!(!d(
            DeploymentMode::Enabled,
            TenantMode::Inherit,
            PersonMode::WhenEnrolled
        ));
        assert!(!d(
            DeploymentMode::RequiredForNonPasskey,
            TenantMode::RequiredForNonPasskey,
            PersonMode::WhenEnrolled
        ));
        assert!(d(
            DeploymentMode::Enabled,
            TenantMode::Inherit,
            PersonMode::AlsoForPasskeys
        ));
        assert!(d(
            DeploymentMode::Enabled,
            TenantMode::Required,
            PersonMode::WhenEnrolled
        ));
        assert!(d(
            DeploymentMode::Required,
            TenantMode::Inherit,
            PersonMode::WhenEnrolled
        ));
    }

    #[test]
    fn a_required_level_with_no_factor_owes_an_enrolment() {
        let owes = decide(&facts(
            DeploymentMode::RequiredForNonPasskey,
            TenantMode::Inherit,
            PersonMode::WhenEnrolled,
            AuthMethodKind::Password,
            &[],
        ));
        assert_eq!(owes, Owes::Enrol);
        // A passkey under required-for-non-passkey owes nothing.
        let owes = decide(&facts(
            DeploymentMode::RequiredForNonPasskey,
            TenantMode::Inherit,
            PersonMode::WhenEnrolled,
            AuthMethodKind::Passkey,
            &[],
        ));
        assert_eq!(owes, Owes::Nothing);
    }

    #[test]
    fn backup_codes_are_offered_beside_a_factor_and_never_instead_of_one() {
        let mut f = facts(
            DeploymentMode::Enabled,
            TenantMode::Inherit,
            PersonMode::WhenEnrolled,
            AuthMethodKind::Password,
            &[],
        );
        f.has_backup_codes = true;
        assert_eq!(decide(&f), Owes::Nothing, "codes alone are not a factor");
        let on = [SecondStepKind::SecurityKey];
        f.enrolled = &on;
        assert_eq!(
            decide(&f),
            Owes::Step(vec![
                SecondStepKind::SecurityKey,
                SecondStepKind::BackupCode
            ])
        );
    }
}
