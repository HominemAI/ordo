//! **A person may see the list of their own tenants** (owner, 2026-09-29),
//! and nobody else may.
//!
//! `std.person.tenants` is a platform op with no parameter. The principal is
//! the one the door proved, so no call can name another person, and a tenant
//! is never shown which other tenants a person belongs to except to that
//! person. No database is needed: the engine is built from `members` rows as
//! the door builds it.

use std::sync::Arc;

use async_trait::async_trait;
use ordo_api::{AccessChecker, AccessDecision, BoundParams};
use ordo_conformance::Fixture;
use ordo_host::platform_ops::{self, PlatformOps, MY_TENANTS};
use ordo_host::IdentityAccess;
use ordo_identity::grants::{GrantError, GrantRecord, GrantRegistry, NewGrant};
use ordo_identity::principal::{GrantId, PrincipalId, SpaceId, Timestamp};
use ordo_identity::{TokenKey, TokenSpec};

const PROGRAM: &str = "\
entity notes (key: nte) {
  title: text
}

op listNotes =
  notes[id, title]
";
const SECRET: &[u8] = b"my-tenants-secret";
const NOW: u64 = 1_000;

fn row(space: &str, id: &str, role: &str, status: &str, principal: &str) -> Vec<Option<String>> {
    [space, id, role, status, principal]
        .into_iter()
        .map(|s| Some(s.to_string()))
        .collect()
}

/// Ana: editor in A, viewer in B, suspended in D. Bo: viewer in A, owner in
/// C. Cy: nowhere.
fn members() -> Vec<Vec<Option<String>>> {
    vec![
        row("spc_a", "mem_a_ana", "editor", "active", "usr_ana"),
        row("spc_b", "mem_b_ana", "viewer", "active", "usr_ana"),
        row("spc_d", "mem_d_ana", "manager", "suspended", "usr_ana"),
        row("spc_a", "mem_a_bo", "viewer", "active", "usr_bo"),
        row("spc_c", "mem_c_bo", "owner", "active", "usr_bo"),
    ]
}

fn engine() -> ordo_identity::AccessEngine {
    let f = Fixture::from_source(PROGRAM, "").expect("the program compiles");
    ordo_host::engine_from_members(&f, &members()).expect("the engine builds")
}

struct NoGrants;

#[async_trait]
impl GrantRegistry for NoGrants {
    async fn record(&self, _: &NewGrant) -> Result<(), GrantError> {
        Ok(())
    }
    async fn revoke(&self, _: &SpaceId, _: &GrantId, _: Timestamp) -> Result<(), GrantError> {
        Ok(())
    }
    async fn callers_of(
        &self,
        _: &SpaceId,
        _: &str,
        _: Timestamp,
    ) -> Result<Vec<GrantRecord>, GrantError> {
        Ok(Vec::new())
    }
    async fn grants_of(
        &self,
        _: &SpaceId,
        _: &PrincipalId,
    ) -> Result<Vec<GrantRecord>, GrantError> {
        Ok(Vec::new())
    }
    async fn is_live(&self, _: &SpaceId, _: &GrantId, _: Timestamp) -> Result<bool, GrantError> {
        Ok(true)
    }
}

/// What the op answers for `caller` (a member id) bound to `space`.
async fn tenants_as(space: &str, caller: &str) -> Vec<(String, String, String)> {
    let ops = PlatformOps::new(Arc::new(engine()), Arc::new(NoGrants), Arc::new(|| NOW));
    let spec = platform_ops::specs()
        .into_iter()
        .find(|s| s.name == MY_TENANTS)
        .expect("the op is defined");
    let out = ops
        .execute(&spec, space, &BoundParams::default(), caller)
        .await
        .expect("it answers");
    out.rows
        .iter()
        .map(|r| {
            (
                r["space"].as_str().unwrap().to_string(),
                r["role"].as_str().unwrap().to_string(),
                r["status"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn t(space: &str, role: &str, status: &str) -> (String, String, String) {
    (space.into(), role.into(), status.into())
}

/// A person sees every tenant they belong to, with the role held in each,
/// whichever of them the call was made in.
#[tokio::test]
async fn a_person_sees_their_own_tenants_with_their_role_in_each() {
    let ana = vec![
        t("spc_a", "editor", "active"),
        t("spc_b", "viewer", "active"),
        t("spc_d", "manager", "suspended"),
    ];
    assert_eq!(tenants_as("spc_a", "mem_a_ana").await, ana);
    assert_eq!(tenants_as("spc_b", "mem_b_ana").await, ana);
}

/// Nobody else sees it: another person's call names only that person's
/// tenants, so a tenant both belong to (A) never reveals the rest of either.
#[tokio::test]
async fn nobody_sees_anybody_elses_tenants() {
    let bo = tenants_as("spc_a", "mem_a_bo").await;
    assert_eq!(
        bo,
        vec![
            t("spc_a", "viewer", "active"),
            t("spc_c", "owner", "active")
        ]
    );
    for (space, _, _) in &bo {
        assert_ne!(space, "spc_b", "Bo is not shown Ana's tenants");
        assert_ne!(space, "spc_d", "Bo is not shown Ana's tenants");
    }
}

fn token(principal: &str, presence: bool) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: Timestamp(NOW + 3_600),
            client: None,
            user_presence_at: presence.then_some(Timestamp(NOW)),
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

fn door() -> IdentityAccess {
    IdentityAccess::new(
        engine(),
        TokenKey::from_secret(SECRET),
        Box::new(|| NOW),
        Arc::new(ordo_host::metrics::Metrics::new()),
    )
}

fn allowed(d: &IdentityAccess, who: &str, presence: bool, space: &str, op: &str) -> bool {
    matches!(
        d.check(&token(who, presence), space, op),
        AccessDecision::Allow { .. }
    )
}

/// The door lets a viewer call it with no presence at all (a password
/// session), refuses a person who is not a member of the tenant the call
/// names, and still keeps `callersOf` to managers behind step-up.
#[test]
fn the_door_serves_every_role_without_presence_and_refuses_strangers() {
    let d = door();
    assert!(
        allowed(&d, "usr_ana", false, "spc_b", MY_TENANTS),
        "a viewer, a password session"
    );
    assert!(
        allowed(&d, "usr_ana", false, "spc_a", MY_TENANTS),
        "an editor"
    );
    assert!(
        allowed(&d, "usr_bo", false, "spc_c", MY_TENANTS),
        "an owner"
    );
    assert!(
        !allowed(&d, "usr_cy", false, "spc_a", MY_TENANTS),
        "not a member there"
    );
    assert!(
        !allowed(&d, "usr_bo", false, "spc_b", MY_TENANTS),
        "not a member of that tenant"
    );
    assert!(!allowed(
        &d,
        "usr_ana",
        false,
        "spc_b",
        platform_ops::CALLERS_OF
    ));
}

/// **G6** (`plan-sharing.md`). A seat with no role is a guest, and the list
/// says so: `role` is null, where a member's is the word for their role. Dee
/// owns a tenant of her own (spc_own) and is a guest in Eli's (spc_eli).
#[tokio::test]
async fn a_guest_seat_is_listed_with_no_role_and_a_member_is_not_listed_as_a_guest() {
    let rows = vec![
        row("spc_own", "mem_own_dee", "owner", "active", "usr_dee"),
        // The guest seat: no role. `row` cannot say null, so it is spelt out.
        vec![
            Some("spc_eli".to_string()),
            Some("mem_eli_dee".to_string()),
            None,
            Some("active".to_string()),
            Some("usr_dee".to_string()),
        ],
        row("spc_eli", "mem_eli_eli", "owner", "active", "usr_eli"),
    ];
    let f = Fixture::from_source(PROGRAM, "").expect("the program compiles");
    let engine = ordo_host::engine_from_members(&f, &rows).expect("the engine builds");
    let ops = PlatformOps::new(Arc::new(engine), Arc::new(NoGrants), Arc::new(|| NOW));
    let spec = platform_ops::specs()
        .into_iter()
        .find(|s| s.name == MY_TENANTS)
        .expect("the op is defined");
    let out = ops
        .execute(&spec, "spc_own", &BoundParams::default(), "mem_own_dee")
        .await
        .expect("it answers");
    let listed: Vec<(String, Option<String>)> = out
        .rows
        .iter()
        .map(|r| {
            (
                r["space"].as_str().unwrap().to_string(),
                r["role"].as_str().map(str::to_string),
            )
        })
        .collect();
    assert_eq!(
        listed,
        vec![
            ("spc_eli".to_string(), None),
            ("spc_own".to_string(), Some("owner".to_string())),
        ],
        "the guest seat has a null role and the owned tenant names its role"
    );
    assert!(
        out.rows[0]["role"].is_null(),
        "null on the wire, not a word"
    );

    // Eli, who owns spc_eli, is not listed anywhere as a guest.
    let out = ops
        .execute(&spec, "spc_eli", &BoundParams::default(), "mem_eli_eli")
        .await
        .expect("it answers");
    assert_eq!(out.rows.len(), 1);
    assert_eq!(out.rows[0]["role"], "owner");
}
