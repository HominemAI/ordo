//! Owner ruling 2026-10-27: when the door's read of a space's database fails
//! for a passing reason, it keeps serving that space's last good member list
//! for a bounded time (`ORDO_DOOR_STALE_GRACE_SECS`), and denies past the
//! bound or on a lasting error.
//!
//! The failure is injected into the door's cache ([`Cache::force_failure`]);
//! the space is attached to the control database itself, so a good read is a
//! real read of real member rows.

use std::time::Duration;

use ordo_conformance::{Fixture, Pg};
use ordo_crypto::SpaceId;
use ordo_exec::route::Installed;
use ordo_exec::spacedb::{self, DbAttachment, DbCustody};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::directory::{stale_grace_from, Cache, Directory, Scope};

struct World {
    f: Fixture,
    control: Pool,
    template: PoolConfig,
    space: SpaceId,
}

async fn world(db: &str) -> World {
    let f = Fixture::load().expect("fixture");
    f.reset(&Pg::named(db)).expect("seeded");
    let template = PoolConfig {
        db: db.into(),
        size: 3,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    };
    let control = Pool::connect(template.clone()).await.expect("pool");
    spacedb::ensure_table(&control).await.expect("table");
    control.rows("DELETE FROM \"$spacedb\"").await.unwrap();
    let space = ordo_conformance::fixture_space();
    spacedb::attach(
        &control,
        &space,
        &DbAttachment {
            host: template.host.clone(),
            port: template.port,
            user: template.user.clone(),
            db: db.into(),
            credential: "local".into(),
            custody: DbCustody::PlatformManaged,
            attached_at: 1,
            attached_by: "test:stale-grace".into(),
        },
    )
    .await
    .expect("attached");
    World {
        f,
        control,
        template,
        space,
    }
}

async fn build(w: &World, cache: &mut Cache) -> Directory {
    let creds = Installed::new().as_configured("local");
    let reads = ordo_host::directory::prefetch(&w.control, &creds, &w.template, Scope::All).await;
    cache
        .build(reads, &w.f, &w.control, &creds, &w.template)
        .await
        .expect("the directory builds")
}

fn members(d: &Directory, w: &World) -> usize {
    d.engine
        .members_of(&ordo_identity::principal::SpaceId(w.space.0.clone()))
        .len()
}

#[tokio::test]
async fn a_passing_failure_keeps_the_members_within_the_bound_and_denies_after_it() {
    let w = world("ordo_door_grace_keep").await;
    let mut cache = Cache::with_grace(Duration::from_secs(60));
    let good = build(&w, &mut cache).await;
    let served = members(&good, &w);
    assert!(served > 0, "the fixture space has members");
    assert!(good.kept_stale.is_empty() && good.unread_spaces.is_empty());

    cache.force_failure(Some(true));
    let kept = build(&w, &mut cache).await;
    assert_eq!(members(&kept, &w), served, "the last good list is served");
    assert!(kept.unread_spaces.is_empty());
    assert_eq!(kept.kept_stale.len(), 1);
    assert_eq!(kept.kept_stale[0].0, w.space.0);

    cache.advance(Duration::from_secs(30));
    let still = build(&w, &mut cache).await;
    assert_eq!(members(&still, &w), served, "still inside the bound");

    cache.advance(Duration::from_secs(31));
    let past = build(&w, &mut cache).await;
    assert_eq!(members(&past, &w), 0, "past the bound the space is denied");
    assert!(past.kept_stale.is_empty());
    assert_eq!(past.unread_spaces.len(), 1);

    cache.force_failure(None);
    let healed = build(&w, &mut cache).await;
    assert_eq!(members(&healed, &w), served, "a good read serves again");
    assert!(healed.kept_stale.is_empty() && healed.unread_spaces.is_empty());
}

#[tokio::test]
async fn a_lasting_error_denies_at_once() {
    let w = world("ordo_door_grace_lasting").await;
    let mut cache = Cache::with_grace(Duration::from_secs(60));
    assert!(members(&build(&w, &mut cache).await, &w) > 0);
    cache.force_failure(Some(false));
    let d = build(&w, &mut cache).await;
    assert_eq!(members(&d, &w), 0);
    assert!(d.kept_stale.is_empty());
    assert_eq!(d.unread_spaces.len(), 1);
    // A passing failure after a lasting one has nothing old to serve.
    cache.force_failure(Some(true));
    let d = build(&w, &mut cache).await;
    assert_eq!(members(&d, &w), 0);
    assert_eq!(d.unread_spaces.len(), 1);
}

#[tokio::test]
async fn a_grace_of_zero_denies_at_once_as_before() {
    let w = world("ordo_door_grace_zero").await;
    let mut cache = Cache::with_grace(Duration::ZERO);
    assert!(members(&build(&w, &mut cache).await, &w) > 0);
    cache.force_failure(Some(true));
    let d = build(&w, &mut cache).await;
    assert_eq!(members(&d, &w), 0);
    assert!(d.kept_stale.is_empty());
    assert_eq!(d.unread_spaces.len(), 1);
}

#[test]
fn the_setting_parses_and_a_bad_value_names_it() {
    assert_eq!(stale_grace_from(None), Ok(Duration::from_secs(60)));
    assert_eq!(stale_grace_from(Some(" ")), Ok(Duration::from_secs(60)));
    assert_eq!(stale_grace_from(Some("0")), Ok(Duration::ZERO));
    assert_eq!(stale_grace_from(Some("15")), Ok(Duration::from_secs(15)));
    for bad in ["-1", "soon", "1.5", "999999999"] {
        let e = stale_grace_from(Some(bad)).unwrap_err();
        assert!(e.contains(bad), "{e}");
    }
}
