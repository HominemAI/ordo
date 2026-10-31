//! A share link's password limits, in three layers, the way session settings
//! are layered ([`crate::session_policy`]; owner, 2026-10-21: "It is their
//! space"). The layering itself is [`crate::layered_limits`].
//!
//! | Layer | Sets | How |
//! |---|---|---|
//! | Deployment | the defaults and the absolute limits | `ORDO_SHARE_LINK_PASSWORD_TRIES`, `_LOCK_SECS`, `_MIN_LENGTH`, `_SLOW_AFTER`, `_SLOW_CAP_SECS`, and a `_LIMIT` for each |
//! | App developer | its own defaults, and optionally a narrower range per value | `std.deployment.setLinkPasswordLimits` |
//! | Tenant | its own values, looser or tighter, inside the app developer's range, or inside the deployment's limits when the app developer set none | `std.space.setLinkPasswordLimits` |
//!
//! The value in force is the tenant's, else the app developer's, else the
//! deployment's. A stored value that a later change left outside its range
//! is held to the nearest end of it.
//!
//! `tries` is **off by default** (owner, 2026-10-22: "Off by default. But per
//! link."). Off is the number 0, written `off` in a variable or a setter.
//! When off, a wrong password never locks. The count is still kept per link,
//! so a layer that turns it on locks a link by the tries that link has
//! missed. A range that starts above 0 (say `3-10`) shuts off for the layers
//! below it: the app developer can require a lock.
//!
//! Whether a tenant may set its own at all is the app developer's, through
//! the access template (`ORDO_ACCESS_TEMPLATE="std.space.setLinkPasswordLimits="`
//! closes it to every role), as for `std.space.setSessionPolicy`.
//!
//! What each `_LIMIT` bounds, and the platform's own end of each range:
//!
//! | Value | Looser is | `_LIMIT` is | Default, limit | Range |
//! |---|---|---|---|---|
//! | `tries` | more | the most tries | off, 100 | `off` (0), then 1 to the limit |
//! | `lockSecs` | shorter | the shortest lock | 900, 60 | the limit to 30 days |
//! | `minLength` | shorter | the shortest minimum | 8, 8 | the limit to 256 |
//! | `slowAfter` | more | the most wrong tries before slowing | 5, 100 | `off` (0), then 1 to the limit |
//! | `slowCapSecs` | shorter | the shortest longest wait | 60, 1 | the limit to 1 day |
//!
//! # The slowdown (owner, 2026-10-25)
//!
//! Separate from the lock, and **on by default**: after `slowAfter` wrong
//! passwords on one link, each further try waits for a doubling delay (1 s,
//! 2 s, 4 s, ...) up to `slowCapSecs`, and a try before it ends is refused
//! `429 too_many_attempts` with `Retry-After`. The right password resets it.
//! It never locks: the rightful holder waits at most `slowCapSecs`. The count
//! is kept in the control database, so every node sees it
//! ([`crate::guess_limits::Counter`]). A tenant may turn it off only where
//! the app developer's `slowAfterRange` starts at 0.
//!
//! # The shortest password (owner, 2026-10-25)
//!
//! 8 characters, and no layer may choose fewer unless the deployment owner
//! lowers `ORDO_SHARE_LINK_PASSWORD_MIN_LENGTH_LIMIT`. Only a password being
//! set is measured: a link given a shorter password before keeps it, and it
//! still opens the link.

use crate::guess_limits::{
    DEFAULT_SLOW_AFTER, DEFAULT_SLOW_AFTER_LIMIT, DEFAULT_SLOW_CAP_SECS,
    DEFAULT_SLOW_CAP_SECS_LIMIT, SLOW_AFTER_CEILING, SLOW_CAP_SECS_CEILING,
};
use crate::layered_limits::{self as layered, Def, Family, Looser};

pub use crate::layered_limits::{
    answer_fields, check_app, check_tenant, range_change, resolve, value_change, OFF as TRIES_OFF,
};

/// One of the values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Tries,
    LockSecs,
    MinLength,
    SlowAfter,
    SlowCapSecs,
}

pub const FIELDS: [Field; 5] = [
    Field::Tries,
    Field::LockSecs,
    Field::MinLength,
    Field::SlowAfter,
    Field::SlowCapSecs,
];

pub const TRIES_ENV: &str = "ORDO_SHARE_LINK_PASSWORD_TRIES";
pub const LOCK_SECS_ENV: &str = "ORDO_SHARE_LINK_PASSWORD_LOCK_SECS";
pub const MIN_LENGTH_ENV: &str = "ORDO_SHARE_LINK_PASSWORD_MIN_LENGTH";
pub const SLOW_AFTER_ENV: &str = "ORDO_SHARE_LINK_PASSWORD_SLOW_AFTER";
pub const SLOW_CAP_SECS_ENV: &str = "ORDO_SHARE_LINK_PASSWORD_SLOW_CAP_SECS";
pub const TRIES_LIMIT_ENV: &str = "ORDO_SHARE_LINK_PASSWORD_TRIES_LIMIT";
pub const LOCK_SECS_LIMIT_ENV: &str = "ORDO_SHARE_LINK_PASSWORD_LOCK_SECS_LIMIT";
pub const MIN_LENGTH_LIMIT_ENV: &str = "ORDO_SHARE_LINK_PASSWORD_MIN_LENGTH_LIMIT";
pub const SLOW_AFTER_LIMIT_ENV: &str = "ORDO_SHARE_LINK_PASSWORD_SLOW_AFTER_LIMIT";
pub const SLOW_CAP_SECS_LIMIT_ENV: &str = "ORDO_SHARE_LINK_PASSWORD_SLOW_CAP_SECS_LIMIT";

/// 0 is off: wrong tries never lock.
pub const DEFAULT_TRIES: u64 = 0;
pub const DEFAULT_LOCK_SECS: u64 = 900;
/// Owner, 2026-10-25: a link password is at least 8 characters.
pub const DEFAULT_MIN_LENGTH: u64 = 8;
/// The most tries any layer may choose, when the deployment sets no limit.
pub const DEFAULT_TRIES_LIMIT: u64 = 100;
/// The shortest lock any layer may choose, when the deployment sets no limit.
pub const DEFAULT_LOCK_SECS_LIMIT: u64 = 60;
/// The shortest minimum length any layer may choose, when the deployment
/// sets no limit.
pub const DEFAULT_MIN_LENGTH_LIMIT: u64 = 8;

/// The platform's own ends, which no setting moves.
pub const TRIES_CEILING: u64 = 1_000;
pub const LOCK_SECS_CEILING: u64 = 30 * 86_400;
/// A link password is at most this many characters.
pub const PASSWORD_MAX: u64 = 256;

static DEFS: [Def; 5] = [
    Def {
        word: "tries",
        range_word: "triesRange",
        from_word: "triesFrom",
        range_from_word: "triesRangeFrom",
        env: TRIES_ENV,
        limit_env: TRIES_LIMIT_ENV,
        column: "link_password_tries",
        default: DEFAULT_TRIES,
        default_limit: DEFAULT_TRIES_LIMIT,
        limit_span: (1, TRIES_CEILING),
        looser: Looser::More,
        off: true,
        // Owner, 2026-10-22: the lock is off by default, at every layer.
        tenant_off: true,
    },
    Def {
        word: "lockSecs",
        range_word: "lockSecsRange",
        from_word: "lockSecsFrom",
        range_from_word: "lockSecsRangeFrom",
        env: LOCK_SECS_ENV,
        limit_env: LOCK_SECS_LIMIT_ENV,
        column: "link_password_lock_secs",
        default: DEFAULT_LOCK_SECS,
        default_limit: DEFAULT_LOCK_SECS_LIMIT,
        limit_span: (1, LOCK_SECS_CEILING),
        looser: Looser::Less {
            ceiling: LOCK_SECS_CEILING,
        },
        off: false,
        tenant_off: false,
    },
    Def {
        word: "minLength",
        range_word: "minLengthRange",
        from_word: "minLengthFrom",
        range_from_word: "minLengthRangeFrom",
        env: MIN_LENGTH_ENV,
        limit_env: MIN_LENGTH_LIMIT_ENV,
        column: "link_password_min_length",
        default: DEFAULT_MIN_LENGTH,
        default_limit: DEFAULT_MIN_LENGTH_LIMIT,
        limit_span: (1, PASSWORD_MAX),
        looser: Looser::Less {
            ceiling: PASSWORD_MAX,
        },
        off: false,
        tenant_off: false,
    },
    Def {
        word: "slowAfter",
        range_word: "slowAfterRange",
        from_word: "slowAfterFrom",
        range_from_word: "slowAfterRangeFrom",
        env: SLOW_AFTER_ENV,
        limit_env: SLOW_AFTER_LIMIT_ENV,
        column: "link_password_slow_after",
        default: DEFAULT_SLOW_AFTER,
        default_limit: DEFAULT_SLOW_AFTER_LIMIT,
        limit_span: (1, SLOW_AFTER_CEILING),
        looser: Looser::More,
        off: true,
        tenant_off: false,
    },
    Def {
        word: "slowCapSecs",
        range_word: "slowCapSecsRange",
        from_word: "slowCapSecsFrom",
        range_from_word: "slowCapSecsRangeFrom",
        env: SLOW_CAP_SECS_ENV,
        limit_env: SLOW_CAP_SECS_LIMIT_ENV,
        column: "link_password_slow_cap_secs",
        default: DEFAULT_SLOW_CAP_SECS,
        default_limit: DEFAULT_SLOW_CAP_SECS_LIMIT,
        limit_span: (1, SLOW_CAP_SECS_CEILING),
        looser: Looser::Less {
            ceiling: SLOW_CAP_SECS_CEILING,
        },
        off: false,
        tenant_off: false,
    },
];

impl Family for Field {
    const ALL: &'static [Field] = &FIELDS;
    const BAD: &'static str = "link_bad_password_limit";
    const OUT_OF_RANGE: &'static str = "link_password_limit_out_of_range";
    const APP_OP: &'static str = "std.deployment.setLinkPasswordLimits";

    fn def(self) -> &'static Def {
        &DEFS[self.i()]
    }

    fn i(self) -> usize {
        match self {
            Field::Tries => 0,
            Field::LockSecs => 1,
            Field::MinLength => 2,
            Field::SlowAfter => 3,
            Field::SlowCapSecs => 4,
        }
    }
}

impl Field {
    /// The parameter and answer word.
    pub fn word(self) -> &'static str {
        self.def().word
    }

    pub fn range_word(self) -> &'static str {
        self.def().range_word
    }

    pub fn env(self) -> &'static str {
        self.def().env
    }

    pub fn limit_env(self) -> &'static str {
        self.def().limit_env
    }

    /// The tenant's column on `$admissionpolicy`.
    pub fn column(self) -> &'static str {
        self.def().column
    }
}

/// The deployment's layer: a default and an absolute limit per value.
pub type Deployment = layered::Deployment<Field>;
/// One stored layer. A tenant's has values only.
pub type Layer = layered::Layer<Field>;
/// The values in force for a tenant, and whose each is.
pub type Effective = layered::Effective<Field>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tenant_wins_inside_the_app_range_and_is_held_to_it() {
        let d = Deployment::default();
        let mut app = Layer::default();
        app.set_value(Field::Tries, Some(8));
        app.set_range(Field::Tries, Some((2, 10)));
        let mut t = Layer::default();
        t.set_value(Field::Tries, Some(9));
        let e = resolve(&d, &app, &t);
        assert_eq!(
            (e.get(Field::Tries), e.from_of(Field::Tries)),
            (9, "tenant")
        );
        assert_eq!(e.get(Field::LockSecs), 900);
        // Nothing set anywhere: the lock is off, the slowdown on.
        let e0 = resolve(&d, &Layer::default(), &Layer::default());
        assert_eq!(e0.get(Field::Tries), TRIES_OFF);
        assert_eq!(e0.get(Field::SlowAfter), 5);
        assert_eq!(e0.get(Field::SlowCapSecs), 60);
        assert_eq!(e0.get(Field::MinLength), 8);
        // A later, narrower range holds the stored value to it.
        app.set_range(Field::Tries, Some((2, 6)));
        assert_eq!(resolve(&d, &app, &t).get(Field::Tries), 6);
        assert!(check_tenant(&d, &app, &t).is_err());
    }

    #[test]
    fn with_no_app_range_the_deployment_limits_bound_the_tenant() {
        let d = Deployment::default().with_limit(Field::MinLength, 3);
        let mut t = Layer::default();
        t.set_value(Field::MinLength, Some(2));
        let e = check_tenant(&d, &Layer::default(), &t).unwrap_err();
        assert!(e.to_string().contains(MIN_LENGTH_LIMIT_ENV), "{e}");
        t.set_value(Field::MinLength, Some(3));
        assert!(check_tenant(&d, &Layer::default(), &t).is_ok());
    }

    #[test]
    fn a_tenant_turns_the_slowdown_off_only_inside_an_app_range_from_0() {
        let d = Deployment::default();
        let mut t = Layer::default();
        t.set_value(Field::SlowAfter, Some(TRIES_OFF));
        let e = check_tenant(&d, &Layer::default(), &t).unwrap_err();
        assert!(e.to_string().contains("slowAfterRange"), "{e}");
        // Held to 1 if it was stored before.
        assert_eq!(resolve(&d, &Layer::default(), &t).get(Field::SlowAfter), 1);
        let mut app = Layer::default();
        app.set_range(Field::SlowAfter, Some((0, 20)));
        assert!(check_tenant(&d, &app, &t).is_ok());
        assert_eq!(resolve(&d, &app, &t).get(Field::SlowAfter), TRIES_OFF);
        // The app developer may turn it off for everyone.
        let mut off = Layer::default();
        off.set_value(Field::SlowAfter, Some(TRIES_OFF));
        assert!(check_app(&d, &off).is_ok());
    }
}
