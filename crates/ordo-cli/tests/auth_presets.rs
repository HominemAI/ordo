//! The `auth` block's presets are the presets the host ships.
//!
//! `ordo_syntax::AUTH_PRESETS` is what the compiler accepts after
//! `preset`. `ordo_identity` holds the presets the host can run. A preset
//! in one list and not the other would compile and then fail at sign-in,
//! or ship and never be writable. This crate sees both, so it pins them.

use std::collections::BTreeSet;

use ordo_identity::oauth2::PRESETS;
use ordo_identity::oidc::OIDC_PRESETS;
use ordo_syntax::{ProviderKind, AUTH_PRESETS};

#[test]
fn the_auth_presets_are_the_hosts_presets_with_their_kinds() {
    let file: BTreeSet<(&str, &str)> = AUTH_PRESETS.iter().map(|(n, k)| (*n, k.as_str())).collect();
    let host: BTreeSet<(&str, &str)> = PRESETS
        .iter()
        .map(|n| (*n, ProviderKind::Oauth2.as_str()))
        .chain(
            OIDC_PRESETS
                .iter()
                .map(|n| (*n, ProviderKind::Oidc.as_str())),
        )
        .collect();
    assert_eq!(
        file, host,
        "AUTH_PRESETS in ordo-syntax and the host's PRESETS and OIDC_PRESETS \
         in ordo-identity disagree; change both together"
    );
}
