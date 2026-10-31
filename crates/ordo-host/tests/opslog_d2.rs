//! D2: the door's deny sites already carry the space the request bound
//! (`crates/ordo-host/src/lib.rs`'s `check` and `token_gate`, `:1192`,
//! `:1241`, `:1270`): `self.stream.deny(Some(&cspace), ...)` was already
//! there. What dropped the space was `Recorder::observe` (D1b, fixed in
//! `ordo-secevent`), so these three call sites needed **no change** to
//! start naming the space they probed. These are the regression tests for
//! that "no change needed" claim, exercised through the real door rather
//! than through the generic recorder.
//!
//! No database: `IdentityAccess::check` is synchronous and pure, and
//! `take_events` drains the in-process recorder directly.

use std::sync::Arc;

use ordo_api::{AccessChecker, AccessDecision};
use ordo_commandlog::SpaceId;
use ordo_host::metrics::Metrics;
use ordo_host::IdentityAccess;
use ordo_identity::principal::{PrincipalId, Timestamp};
use ordo_identity::{mint, AccessEngine, TokenKey, TokenSpec};
use ordo_secevent::{EventKind, RecordedDecision, Scope, Subject};

const SECRET: &[u8] = b"opslog-d2-test";

fn access() -> IdentityAccess {
    IdentityAccess::new(
        AccessEngine::new(),
        TokenKey::from_secret(SECRET),
        Box::new(|| 1_000),
        Arc::new(Metrics::new()),
    )
}

fn token(principal: &str) -> String {
    mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: Timestamp(9_999),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

/// A principal with no relationship to the space it named: `NoMembership`,
/// deployment scope, and now `about` naming the space it probed.
#[test]
fn a_non_member_denial_names_the_space_it_probed() {
    let access = access();
    let tok = token("usr_stranger");

    let decision = access.check(&tok, "spc_target", "openTasks");
    assert_eq!(decision, AccessDecision::Deny);

    let entries = access.take_events();
    assert_eq!(entries.len(), 1);
    let e = &entries[0];
    assert_eq!(e.scope, Scope::Deployment);
    assert_eq!(e.subject, Subject::member("usr_stranger"));
    assert_eq!(e.kind, EventKind::denied(RecordedDecision::NoMembership));
    assert_eq!(
        e.about,
        Some(SpaceId("spc_target".into())),
        "a non-member's denial must name the space it probed, not just say \
         it happened"
    );
}

/// A token that fails to verify at all: `Subject::Unverified`, deployment
/// scope by construction (§6.1's members-only rule can't apply to a subject
/// that proved nothing), and it too must name the space it targeted.
#[test]
fn an_unverified_caller_s_denial_names_the_space_it_probed() {
    let access = access();

    let decision = access.check("not-a-real-bearer-token", "spc_target", "openTasks");
    assert_eq!(decision, AccessDecision::Deny);

    let entries = access.take_events();
    assert_eq!(entries.len(), 1);
    let e = &entries[0];
    assert_eq!(e.scope, Scope::Deployment);
    assert_eq!(e.subject, Subject::Unverified);
    assert_eq!(
        e.kind,
        EventKind::denied(RecordedDecision::TokenUnverifiable)
    );
    assert_eq!(
        e.about,
        Some(SpaceId("spc_target".into())),
        "an unverifiable presentation still names the space it targeted"
    );
}

/// D1b/D2 touch the security *stream*; they must not perturb the counter
/// substrate the mandatory deny-volume alert reads. The alert is
/// aggregated per `(space, principal)` over ops, unaffected by whether the
/// stream now also carries `about`, so the door's existing many-ops,
/// one-space sweep still fires exactly as before.
#[test]
fn the_deny_volume_monitor_is_unchanged() {
    let access = access();
    let tok = token("usr_probe");

    // A hundred spaces, one denial each: the *stream* now shows a hundred
    // distinct entries (D1b's fix), but every one of them is one denial in
    // its own `(space, principal)` counter series, so none of them alone
    // crosses any threshold.
    for n in 0..100 {
        let _ = access.check(&tok, &format!("spc_sweep_{n:03}"), "openTasks");
    }
    assert!(
        access.metrics.evaluate_alerts(2_000).is_empty(),
        "a sweep across many spaces must not fire the per-space alert, \
         that gap is exactly why the deployment stream has to carry it"
    );

    // The control: many denials against ONE space still fires, unchanged.
    let tok2 = token("usr_burst");
    for i in 0..60 {
        let _ = access.check(&tok2, "spc_alpha", &format!("op{i}"));
    }
    let firing = access.metrics.evaluate_alerts(2_000);
    assert_eq!(
        firing.len(),
        1,
        "the mandatory single-space alert must still fire"
    );
    assert_eq!(firing[0].principal, "usr_burst");
    assert_eq!(firing[0].space, "spc_alpha");
}
