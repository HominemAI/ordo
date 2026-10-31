//! The grant registry (8r's blocker), against real Postgres.
//!
//! Two things were impossible before this table existed, and only one of
//! them gets noticed: a grant could not be **revoked** (its only end was
//! expiry), and `std.space.callersOf` could not be **exhaustive**, because
//! `security-logging-design.md` §6 rule 4 requires the via to include
//! **token** and nothing recorded that a token had been issued.
//!
//! The tests below are ordered by that: the exhaustiveness property first,
//! because it is the one that made an audit surface unbuildable rather than
//! merely inconvenient.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::collections::BTreeSet;
use std::time::Duration;

use ordo_conformance::Pg;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::grants::PgGrantRegistry;
use ordo_identity::grants::{GrantRegistry, NewGrant};
use ordo_identity::{ClientId, GrantId, ManifestHash, OpPin, PrincipalId, SpaceId, Timestamp};

const NOW: Timestamp = Timestamp(1_000);
const LATER: Timestamp = Timestamp(9_000);

async fn registry(db: &str) -> PgGrantRegistry {
    let _ = Pg::named(db);
    let pool = Pool::connect(PoolConfig {
        db: db.into(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    let r = PgGrantRegistry::new(pool);
    r.ensure_table().await.expect("platform table");
    r
}

/// The registry takes one struct — named fields, because `space`,
/// `principal` and `client` are all string newtypes and a transposition
/// would record a grant against the wrong principal with nothing to catch
/// it. This keeps the tests' argument order readable at the call site.
#[allow(clippy::too_many_arguments)]
fn ng(
    space: &SpaceId,
    principal: &PrincipalId,
    client: Option<&ClientId>,
    id: &GrantId,
    ops: &BTreeSet<String>,
    manifest_hash: &ManifestHash,
    granted_at: Timestamp,
    expires: Timestamp,
) -> NewGrant {
    NewGrant {
        id: id.clone(),
        space: space.clone(),
        principal: principal.clone(),
        client: client.cloned(),
        // One pin per listed op, all carrying the same hash: this suite is
        // about the registry's own behaviour (revocation, exhaustiveness),
        // not about what a pin means, so the shapes are deliberately the
        // simplest legal ones.
        pins: ops
            .iter()
            .map(|op| {
                (
                    op.clone(),
                    OpPin {
                        manifest_hash: *manifest_hash,
                        unguarded: false,
                        entities: Vec::new(),
                    },
                )
            })
            .collect(),
        granted_at,
        expires,
    }
}

fn space() -> SpaceId {
    SpaceId("conformance".into())
}

fn ops(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn hash(seed: u8) -> ManifestHash {
    ManifestHash([seed; 32])
}

/// Clear rows for the ids a test uses. These databases outlive a test RUN,
/// and a previous run's live grant would be indistinguishable from this
/// run's — which is exactly the confusion the registry exists to remove.
async fn reset(db: &str, ids: &[&str]) {
    let _ = Pg::named(db);
    let pool = Pool::connect(PoolConfig {
        db: db.into(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    for id in ids {
        let _ = pool
            .rows(&format!(
                "DELETE FROM \"$capabilitygrant\" WHERE \"grant_id\" = '{id}'"
            ))
            .await;
    }
}

/// **The property 8r was blocked on.** `callersOf($op)` must name every
/// principal who can reach the op, and the token path was previously
/// invisible — so the op would have answered confidently while omitting
/// exactly the callers most worth auditing.
///
/// Asserted in both directions in one test, because "returns everyone" would
/// pass a test that only checked the positive half.
#[tokio::test(flavor = "multi_thread")]
async fn callers_of_finds_the_token_path_and_only_it() {
    let db = "ordo_grant_callers";
    reset(db, &["gr_a", "gr_b"]).await;
    let r = registry(db).await;

    r.record(&ng(
        &space(),
        &PrincipalId("agt_ellanor".into()),
        Some(&ClientId("cli_1".into())),
        &GrantId("gr_a".into()),
        &ops(&["openTasks", "closeTask"]),
        &hash(1),
        NOW,
        LATER,
    ))
    .await
    .expect("recorded");

    // A second principal, granted a DIFFERENT op — the separating case.
    r.record(&ng(
        &space(),
        &PrincipalId("agt_other".into()),
        None,
        &GrantId("gr_b".into()),
        &ops(&["taskBoard"]),
        &hash(2),
        NOW,
        LATER,
    ))
    .await
    .expect("recorded");

    let found = r.callers_of(&space(), "closeTask", NOW).await.unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].principal.0, "agt_ellanor");
    assert_eq!(
        found[0].client.as_ref().map(|c| c.0.as_str()),
        Some("cli_1"),
        "the via names the client the grant was consented to"
    );

    // …and the principal granted only `taskBoard` is not returned for
    // `closeTask`. Without this half, an implementation that ignored the op
    // entirely would pass.
    assert!(
        r.callers_of(&space(), "closeTask", NOW)
            .await
            .unwrap()
            .iter()
            .all(|g| g.principal.0 != "agt_other"),
        "a grant that does not include the op is not a caller of it"
    );
    assert_eq!(
        r.callers_of(&space(), "taskBoard", NOW)
            .await
            .unwrap()
            .len(),
        1
    );
}

/// A grant is scoped to exactly one space (§5, pinned: *"there is no
/// deployment-wide grant"*). The registry must not be the place that
/// quietly reintroduces one.
#[tokio::test(flavor = "multi_thread")]
async fn a_grant_does_not_leak_across_spaces() {
    let db = "ordo_grant_space";
    reset(db, &["gr_s"]).await;
    let r = registry(db).await;

    r.record(&ng(
        &space(),
        &PrincipalId("agt_ellanor".into()),
        None,
        &GrantId("gr_s".into()),
        &ops(&["openTasks"]),
        &hash(1),
        NOW,
        LATER,
    ))
    .await
    .expect("recorded");

    let other = SpaceId("somewhere_else".into());
    assert!(
        r.callers_of(&other, "openTasks", NOW)
            .await
            .unwrap()
            .is_empty(),
        "a grant in one space is not a caller in another"
    );
    assert!(
        !r.is_live(&other, &GrantId("gr_s".into()), NOW)
            .await
            .unwrap(),
        "and it is not even live there"
    );
}

/// Revocation: the thing that could not be done at all before, since a
/// grant's only end was its expiry.
#[tokio::test(flavor = "multi_thread")]
async fn revocation_ends_a_grant_before_its_expiry() {
    let db = "ordo_grant_revoke";
    reset(db, &["gr_r"]).await;
    let r = registry(db).await;
    let id = GrantId("gr_r".into());

    r.record(&ng(
        &space(),
        &PrincipalId("agt_ellanor".into()),
        None,
        &id,
        &ops(&["openTasks", "closeTask"]),
        &hash(1),
        NOW,
        LATER,
    ))
    .await
    .expect("recorded");
    assert!(r.is_live(&space(), &id, NOW).await.unwrap());

    r.revoke(&space(), &id, Timestamp(2_000))
        .await
        .expect("revoked");

    assert!(!r.is_live(&space(), &id, NOW).await.unwrap());
    assert!(
        r.callers_of(&space(), "closeTask", NOW)
            .await
            .unwrap()
            .is_empty(),
        "a revoked grant is not a caller — and the revocation reached EVERY \
         op of the grant, not just the first"
    );
    assert!(
        r.callers_of(&space(), "openTasks", NOW)
            .await
            .unwrap()
            .is_empty(),
        "the second op too"
    );
}

/// Revoking twice must not move the timestamp. That value is the audit
/// answer to *"when did this stop"*, and a user clicking twice is not a
/// mistake that should be allowed to rewrite it.
#[tokio::test(flavor = "multi_thread")]
async fn revoking_twice_keeps_the_first_answer() {
    let db = "ordo_grant_twice";
    reset(db, &["gr_t"]).await;
    let r = registry(db).await;
    let id = GrantId("gr_t".into());

    r.record(&ng(
        &space(),
        &PrincipalId("p".into()),
        None,
        &id,
        &ops(&["openTasks"]),
        &hash(1),
        NOW,
        LATER,
    ))
    .await
    .expect("recorded");
    r.revoke(&space(), &id, Timestamp(2_000)).await.unwrap();
    r.revoke(&space(), &id, Timestamp(5_000)).await.unwrap();

    let all = r
        .grants_of(&space(), &PrincipalId("p".into()))
        .await
        .unwrap();
    let rec = all.iter().find(|g| g.id == id).expect("still listed");
    assert_eq!(
        rec.revoked_at,
        Some(Timestamp(2_000)),
        "the first revocation is when it stopped"
    );
}

/// Expiry ends a grant without anyone acting — §5 pins *"no unexpiring
/// grants"*, and the registry has to agree with that rather than keeping a
/// caller alive in the audit view forever.
#[tokio::test(flavor = "multi_thread")]
async fn expiry_ends_a_grant_on_its_own() {
    let db = "ordo_grant_expiry";
    reset(db, &["gr_e"]).await;
    let r = registry(db).await;
    let id = GrantId("gr_e".into());

    r.record(&ng(
        &space(),
        &PrincipalId("p".into()),
        None,
        &id,
        &ops(&["openTasks"]),
        &hash(1),
        NOW,
        Timestamp(3_000),
    ))
    .await
    .expect("recorded");

    assert!(r.is_live(&space(), &id, Timestamp(2_999)).await.unwrap());
    assert!(
        !r.is_live(&space(), &id, Timestamp(3_000)).await.unwrap(),
        "expiry is exclusive: a grant is not live AT the instant it expires"
    );
    assert!(r
        .callers_of(&space(), "openTasks", Timestamp(4_000))
        .await
        .unwrap()
        .is_empty());
}

/// What a person is shown when they ask what they have consented to keeps
/// the withdrawn and expired entries. A list that silently drops them cannot
/// answer *"did I already turn that off?"* — which is the question people
/// actually have.
#[tokio::test(flavor = "multi_thread")]
async fn the_consent_list_keeps_what_was_withdrawn() {
    let db = "ordo_grant_list";
    reset(db, &["gr_l1", "gr_l2"]).await;
    let r = registry(db).await;
    let p = PrincipalId("agt_ellanor".into());

    for (id, op) in [("gr_l1", "openTasks"), ("gr_l2", "taskBoard")] {
        r.record(&ng(
            &space(),
            &p,
            None,
            &GrantId(id.into()),
            &ops(&[op]),
            &hash(1),
            NOW,
            LATER,
        ))
        .await
        .expect("recorded");
    }
    r.revoke(&space(), &GrantId("gr_l1".into()), Timestamp(2_000))
        .await
        .unwrap();

    let all = r.grants_of(&space(), &p).await.unwrap();
    assert_eq!(all.len(), 2, "the withdrawn one is still listed: {all:?}");
    assert!(all.iter().any(|g| g.revoked_at.is_some()));
    assert!(all.iter().any(|g| g.revoked_at.is_none()));
}

/// A grant is recorded whole or not at all. Half a grant in the registry is
/// worse than none: `callers_of` would answer for some of its ops and not
/// others, which is precisely the exhaustiveness failure the table exists to
/// close.
#[tokio::test(flavor = "multi_thread")]
async fn a_grant_lands_for_every_one_of_its_ops() {
    let db = "ordo_grant_whole";
    reset(db, &["gr_w"]).await;
    let r = registry(db).await;

    let names = ["a1", "b2", "c3", "d4"];
    r.record(&ng(
        &space(),
        &PrincipalId("p".into()),
        None,
        &GrantId("gr_w".into()),
        &ops(&names),
        &hash(1),
        NOW,
        LATER,
    ))
    .await
    .expect("recorded");

    for op in names {
        assert_eq!(
            r.callers_of(&space(), op, NOW).await.unwrap().len(),
            1,
            "`{op}` must be reachable through the recorded grant"
        );
    }
}
