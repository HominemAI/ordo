//! Restore across versions (`plan-versions.md` §2.9, §4 row 17), with the
//! owner's ruling of 2026-11-23 on Q13.
//!
//! A tenant's own database is restored from a snapshot taken on X while the
//! deployment serves Y. `Versions::restore_pin` moves the tenant's pin to
//! the version the restored data is at, so it serves X:
//!
//! - X stored and served: pinned to X, cause `restore`, a new epoch;
//! - already on X: nothing moves;
//! - X not stored: refused, naming the version and how to store it, and
//!   the pin does not move; newer than every stored version says so;
//! - X revoked or below `versions.minimum` (Q13): the restore stands, the
//!   pin stays ahead of the data and the answer names the fix;
//! - a restore in the middle of a move: the move goes on from X, so the
//!   database's own cursor is where it resumes.
//!
//! A regression looks like a pin that stays ahead of data it could serve,
//! a pin moved to a version the deployment does not hold, or a move lost.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_conformance::Pg;
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op;
use ordo_host::tenants;
use ordo_host::versions::{RestorePin, VersionRefusal, VersionSetting, Versions};

const ADA: &str = "p_ada";

const PROGRAM: &str = "domain notes @ 0.1.0\n\n\
    entity notes {\n  title: text\n  created: datetime\n}\n\n\
    op allNotes unbounded =\n  notes[title, created]\n";

/// The program at a later version, each adding one field.
fn program_at(version: &str) -> String {
    let extra = match version {
        "0.2.0" => "  starred: bool = false\n",
        "0.3.0" => "  starred: bool = false\n  color: text = \"\"\n",
        _ => "",
    };
    PROGRAM.replace("@ 0.1.0", &format!("@ {version}")).replace(
        "  created: datetime\n",
        &format!("  created: datetime\n{extra}"),
    )
}

async fn fresh(stem: &str) -> (Pool, Pg) {
    let pg = Pg::scoped(stem);
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 4,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool");
    tenants::ensure_table(&pool).await.expect("$spaces");
    all_at_once(&pool).await;
    (pool, pg)
}

async fn apply(pool: &Pool, source: &str) {
    apply_op::run(
        pool,
        &SinglePool::new(pool.clone()),
        source,
        &apply_op::CodeFiles::new(),
        ADA,
        "t",
        true,
    )
    .await
    .expect("applied");
}

/// A deployment storing 0.1.0, 0.2.0 and 0.3.0, the last active, and one
/// tenant pinned to `pinned`.
async fn deployment(stem: &str, pinned: &str) -> (Pool, Pg, String) {
    let (pool, pg) = fresh(stem).await;
    apply(&pool, PROGRAM).await;
    apply(&pool, &program_at("0.2.0")).await;
    apply(&pool, &program_at("0.3.0")).await;
    let id = tenants::mint_space_id();
    tenants::reserve(&pool, &id, "acme", ADA, 100, None)
        .await
        .unwrap();
    Versions::on(&pool)
        .pin(&id, pinned, ADA, 100)
        .await
        .expect("pinned");
    tenants::activate(&pool, &id, 100).await.unwrap();
    (pool, pg, id)
}

async fn pin(pool: &Pool, id: &str) -> (Option<String>, i64) {
    let p = Versions::on(pool).pin_of(id).await.unwrap().unwrap();
    (p.version, p.epoch)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restore_pins_the_tenant_to_the_version_its_data_is_at() {
    let (pool, _pg, id) = deployment("ordo_restore_v1", "0.3.0").await;
    let (_, epoch) = pin(&pool, &id).await;
    let v = Versions::on(&pool);

    let done = v.restore_pin(&id, "0.1.0", ADA, 200).await.expect("pinned");
    let RestorePin::Pinned { change } = done else {
        panic!("expected a pin: {done:?}");
    };
    assert_eq!(change.from.as_deref(), Some("0.3.0"));
    assert_eq!(change.to, "0.1.0");
    assert_eq!(change.cause, "restore");
    let (now, after) = pin(&pool, &id).await;
    assert_eq!(now.as_deref(), Some("0.1.0"));
    assert_eq!(after, epoch + 1, "a restore's pin is a new epoch");
    let last = v.pin_history(&id).await.unwrap().pop().unwrap();
    assert_eq!((last.cause.as_str(), last.actor.as_str()), ("restore", ADA));

    // Run again (the marker's `--again`): already there, nothing moves.
    let again = v.restore_pin(&id, "0.1.0", ADA, 300).await.unwrap();
    assert_eq!(
        again,
        RestorePin::Unchanged {
            version: "0.1.0".into()
        }
    );
    assert_eq!(pin(&pool, &id).await.1, epoch + 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_version_not_stored_is_refused_naming_it_and_the_pin_stays() {
    let (pool, _pg, id) = deployment("ordo_restore_v2", "0.3.0").await;
    let v = Versions::on(&pool);

    let e = v.restore_pin(&id, "0.1.5", ADA, 200).await.unwrap_err();
    assert!(
        matches!(e, VersionRefusal::RestoreNotStored { newer: false, .. }),
        "{e:?}"
    );
    let text = e.to_string();
    assert!(text.contains("0.1.5"), "{text}");
    assert!(text.contains("ordo promote 0.1.5"), "{text}");
    assert!(text.contains("ordo restore --record"), "{text}");

    let e = v.restore_pin(&id, "1.0.0", ADA, 200).await.unwrap_err();
    assert!(
        matches!(e, VersionRefusal::RestoreNotStored { newer: true, .. }),
        "{e:?}"
    );
    assert!(e.to_string().contains("newer than every version"), "{e}");
    // The check alone refuses the same way, and nothing moved.
    assert!(v.restore_check(&id, "0.1.5").await.is_err());
    assert_eq!(pin(&pool, &id).await.0.as_deref(), Some("0.3.0"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_or_below_minimum_version_holds_the_tenant_and_names_the_fix() {
    let (pool, _pg, id) = deployment("ordo_restore_v3", "0.3.0").await;
    let v = Versions::on(&pool);

    v.revoke("0.1.0", "a data bug", None, ADA, 150)
        .await
        .expect("revoked");
    let held = v.restore_pin(&id, "0.1.0", ADA, 200).await.unwrap();
    let RestorePin::Held { data, pin: at, why } = held else {
        panic!("expected a hold: {held:?}");
    };
    assert_eq!((data.as_str(), at.as_deref()), ("0.1.0", Some("0.3.0")));
    assert!(why.contains("ordo versions unrevoke 0.1.0"), "{why}");
    assert_eq!(pin(&pool, &id).await.0.as_deref(), Some("0.3.0"));

    v.set_setting(
        VersionSetting::Minimum,
        Some("0.3.0"),
        ADA,
        "lag bound",
        160,
    )
    .await
    .expect("minimum");
    let held = v.restore_pin(&id, "0.2.0", ADA, 200).await.unwrap();
    let RestorePin::Held { why, .. } = held else {
        panic!("expected a hold: {held:?}");
    };
    assert!(why.contains("0.3.0"), "{why}");
    assert_eq!(pin(&pool, &id).await.0.as_deref(), Some("0.3.0"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restore_in_the_middle_of_a_move_goes_on_from_the_restored_version() {
    let (pool, _pg, id) = deployment("ordo_restore_v4", "0.2.0").await;
    let v = Versions::on(&pool);
    let open = |from: &str| {
        format!(
            "INSERT INTO \"$moves\" (\"space\", \"from\", \"to\", \"phase\", \"started_at\", \
             \"reason\") VALUES ('{id}', '{from}', '0.3.0', 'waiting', 150, 'backfilling')"
        )
    };
    pool.rows(&open("0.2.0")).await.unwrap();

    // Restored to 0.1.0: the move to 0.3.0 stays open, now from 0.1.0.
    v.restore_pin(&id, "0.1.0", ADA, 200).await.expect("pinned");
    let moves = v.moves().await.unwrap();
    let m = moves
        .iter()
        .find(|m| m.space == id)
        .expect("the move stays");
    assert_eq!((m.from.as_str(), m.to.as_str()), ("0.1.0", "0.3.0"));
    assert_eq!(m.phase.as_str(), "expanding", "the catch-up tries it again");
    assert!(m.reason.contains("restored at 0.1.0"), "{}", m.reason);

    // Restored to the move's own target: no move is left.
    v.restore_pin(&id, "0.3.0", ADA, 300).await.expect("pinned");
    assert!(v.moves().await.unwrap().iter().all(|m| m.space != id));
    assert_eq!(pin(&pool, &id).await.0.as_deref(), Some("0.3.0"));
}

/// `rollout.strategy` `all` on `pool`. With it unset, a deployment that is
/// not dev rolls an apply out a batch at a time (ruled 2026-11-25); these
/// tests apply and expect every tenant on the new version at once.
async fn all_at_once(pool: &ordo_exec::Pool) {
    ordo_host::rollouts::Rollouts::on(pool)
        .set_setting(
            "rollout.strategy",
            Some("all"),
            "test",
            "every tenant at once",
            0,
        )
        .await
        .expect("rollout.strategy all");
}
