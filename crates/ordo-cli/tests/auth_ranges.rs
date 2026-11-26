//! The `auth` block's platform ranges and defaults are the host's.
//!
//! `ordo_schema::auth` refuses a file value outside a range, and fills an
//! absent clause with a default. Both restate numbers the host enforces
//! (`session_policy`, `auth::claim`, `renewal`, `ordo_identity::session`).
//! If the host moved one and the compiler did not, a file would compile and
//! then be refused at run time, or the reverse. This crate sees both, so it
//! pins them.

use ordo_host::{auth::claim, renewal, session_policy};
use ordo_schema::auth::{
    Range, DEFAULT_LINKING_WINDOW_SECS, DEFAULT_REMEMBER_IDLE_SECS, DEFAULT_SESSION_MAX_SECS,
    DEFAULT_SESSION_TOKEN_SECS, LINKING_WINDOW, REMEMBER, SESSION_MAX, SESSION_TOKEN,
};

#[test]
fn the_platform_ranges_are_the_hosts() {
    let floor = session_policy::TOKEN_FLOOR;
    assert_eq!(
        SESSION_TOKEN,
        Range {
            min: floor,
            max: session_policy::DEFAULT_TOKEN_LIMIT
        }
    );
    assert_eq!(
        SESSION_MAX,
        Range {
            min: floor,
            max: session_policy::DEFAULT_MAX_LIMIT
        }
    );
    assert_eq!(
        REMEMBER,
        Range {
            min: floor,
            max: session_policy::DEFAULT_REMEMBER_LIMIT
        }
    );
    assert_eq!(
        LINKING_WINDOW,
        Range {
            min: claim::MIN_SECS,
            max: claim::MAX_SECS
        }
    );
}

#[test]
fn the_defaults_are_the_hosts() {
    assert_eq!(
        DEFAULT_SESSION_TOKEN_SECS,
        ordo_identity::session::DEFAULT_SESSION_SECS
    );
    assert_eq!(
        DEFAULT_SESSION_MAX_SECS,
        ordo_identity::session::DEFAULT_SESSION_MAX_SECS
    );
    assert_eq!(DEFAULT_LINKING_WINDOW_SECS, claim::DEFAULT_SECS);
    assert_eq!(DEFAULT_REMEMBER_IDLE_SECS, renewal::DEFAULT_SECS);
}
