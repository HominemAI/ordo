//! `plan-versions.md` §1.7: release and sealing in the sharing rule
//! (review V-10), from compiled programs through the access engine a node
//! builds.
//!
//! - A share made in a tenant on one version and called after the tenant
//!   moved to a version that seals a field the op reads is refused as
//!   `versions_disagree`, naming the field. Both ways.
//! - The same for a `release member` effect on a field the op reads.
//! - Two versions that agree on both keep sharing, whatever else in the
//!   source moved.
//! - The pin rides in the grant's encoding (`pins.v3`) and is judged the
//!   same after a round trip.
//!
//! A regression looks like a share that keeps reading a field one version
//! seals and the other stores in plaintext, or a refusal across 2 versions
//! that agree.
//!
//! Needs no database: the engine is built from the compiled programs and a
//! directory as the node reads it.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ordo_conformance::Fixture;
use ordo_host::Served;
use ordo_identity::principal::{GrantId, SpaceId, Timestamp};
use ordo_identity::{AccessEngine, Decision, DenyReason};

const SPACE: &str = "spc_share";
const OP: &str = "std.memos.list";

/// One program, with `body` sealed or not and `owner`'s release effect as
/// given. Everything else is the same text.
fn program(version: &str, sealed: bool, release: &str) -> String {
    format!(
        "domain memo @ {version}

entity memos (key: memo) {{
  owner -> members
  title: text
  body:  text{}
}}

{release}
",
        if sealed { " sealed" } else { "" }
    )
}

fn seat(space: &str, id: &str, role: &str) -> Vec<Option<String>> {
    vec![
        Some(space.into()),
        Some(id.into()),
        Some(role.into()),
        Some("active".into()),
        Some(id.into()),
        Some("principal".into()),
    ]
}

type Pins = Arc<Mutex<std::collections::BTreeMap<String, String>>>;

/// An engine serving 0.1.0 (active) and 0.2.0, as `directory.rs` builds
/// it, with the tenant's version read from `pins`.
fn engine(v1: &str, v2: &str) -> (AccessEngine, Pins) {
    let f1 = Fixture::from_source(v1, "").expect("0.1.0 compiles");
    let f2 = Arc::new(Fixture::from_source(v2, "").expect("0.2.0 compiles"));
    let served = Served {
        active: Some("0.1.0".into()),
        others: vec![("0.2.0".into(), f2)],
    };
    let e = ordo_host::engine_from_directory_served(
        &f1,
        &served,
        &[seat(SPACE, "mem_o", "owner")],
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &ordo_host::delegable::DelegablePolicy::closed(),
        &ordo_host::access_template::AccessTemplate::none(),
    )
    .expect("engine");
    let pins: Pins = Default::default();
    let read = pins.clone();
    e.set_tenant_versions(Arc::new(move |s: &SpaceId| {
        read.lock().unwrap().get(&s.0).cloned()
    }));
    (e, pins)
}

fn move_to(pins: &Pins, version: &str) {
    pins.lock().unwrap().insert(SPACE.into(), version.into());
}

/// A grant over `OP` made in the tenant now, then the tenant moved to
/// `then`, and what the door decides. The grant also goes through its
/// encoding, as it does in a token.
fn share_then_move(e: &AccessEngine, pins: &Pins, made: &str, then: &str) -> Decision {
    move_to(pins, made);
    let grant = e
        .grant_for(
            GrantId("shr_1".into()),
            &SpaceId(SPACE.into()),
            BTreeSet::from([OP.to_string()]),
            Timestamp(u64::MAX / 2),
        )
        .expect("the op is deployed in both");
    let held = &grant.pins()[OP];
    assert_eq!(held.version.as_deref(), Some(made));
    assert!(held.contract, "the conversion records sealing and release");
    let text = ordo_identity::encode_pins(grant.pins());
    assert!(text.starts_with("7:pins.v3"), "{text}");
    assert_eq!(
        ordo_identity::decode_pins(&text).as_ref(),
        Some(grant.pins())
    );
    let claims = ordo_identity::TokenClaims {
        principal: ordo_identity::principal::PrincipalId("prn_guest".into()),
        capability: Some(grant),
        client: None,
        user_presence_at: None,
        delegation: None,
        session: None,
    };
    let space = SpaceId(SPACE.into());
    assert!(
        e.check(&claims, &space, OP, Timestamp(1)).is_allow(),
        "the share reads on the version it was made under"
    );
    move_to(pins, then);
    e.check(&claims, &space, OP, Timestamp(1))
}

fn refused(d: Decision) -> String {
    match d {
        Decision::Deny {
            reason:
                DenyReason::VersionsDisagree {
                    pinned,
                    serving,
                    entity,
                    detail,
                    ..
                },
            ..
        } => {
            assert_eq!(entity.as_deref(), Some("memos"));
            format!("{pinned}->{serving}: {detail}")
        }
        other => panic!("expected versions_disagree, got {other:?}"),
    }
}

/// 0.2.0 seals `body`: a share made on 0.1.0 is refused on 0.2.0, and one
/// made on 0.2.0 is refused on 0.1.0, each naming the field.
#[test]
fn a_share_across_versions_that_differ_in_sealing_is_refused_by_name() {
    let (e, pins) = engine(&program("0.1.0", false, ""), &program("0.2.0", true, ""));
    let said = refused(share_then_move(&e, &pins, "0.1.0", "0.2.0"));
    assert!(said.starts_with("0.1.0->0.2.0"), "{said}");
    assert!(said.contains("`body` is sealed in one version"), "{said}");
    let said = refused(share_then_move(&e, &pins, "0.2.0", "0.1.0"));
    assert!(said.contains("`body`"), "{said}");
}

/// 0.2.0 removes a leaving member's memos where 0.1.0 kept them: a share
/// is refused naming `owner`'s release effect. 0.3.0's reassign differs
/// from 0.2.0's remove the same way.
#[test]
fn a_share_across_versions_that_differ_in_release_is_refused_by_name() {
    let (e, pins) = engine(
        &program("0.1.0", false, "release member open"),
        &program(
            "0.2.0",
            false,
            "release member open {\n  remove memos.owner\n}",
        ),
    );
    let said = refused(share_then_move(&e, &pins, "0.1.0", "0.2.0"));
    assert!(
        said.contains("`owner`'s `release member` effect differs"),
        "{said}"
    );
    let (e, pins) = engine(
        &program(
            "0.1.0",
            false,
            "release member open {\n  reassign memos.owner to owner\n}",
        ),
        &program(
            "0.2.0",
            false,
            "release member open {\n  remove memos.owner\n}",
        ),
    );
    assert!(refused(share_then_move(&e, &pins, "0.2.0", "0.1.0")).contains("release member"));
}

/// The control: 2 versions that seal the same fields and release the same
/// way keep sharing, though their text (and so each op's manifest hash
/// over the domain's version) differs.
#[test]
fn a_share_across_versions_that_agree_on_sealing_and_release_reads() {
    let rule = "release member when not memos(owner == $member) {\n  remove memos.owner\n}";
    let (e, pins) = engine(&program("0.1.0", true, rule), &program("0.2.0", true, rule));
    assert!(share_then_move(&e, &pins, "0.1.0", "0.2.0").is_allow());
    assert!(share_then_move(&e, &pins, "0.2.0", "0.1.0").is_allow());
}
