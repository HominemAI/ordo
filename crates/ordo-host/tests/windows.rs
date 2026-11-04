//! Maintenance windows (owner 2026-11-04, board
//! `program-versions-the-core-under-schema` asks 19, 21, 22 and 24).
//!
//! - A window is a range or a cron with a length, with a message, for
//!   every tenant, only some, or every tenant except some.
//! - Set replaces by id; delete removes it and its skips.
//! - A skip suppresses a cron window's times over a range, and is marked
//!   on each time in the list.
//! - The list spans 12 months back to 12 months ahead. Filtered by a
//!   tenant it gives the windows that cover it, `except` included for a
//!   tenant created after the window.
//! - Unknown tenants are refused, by name. Erasing a tenant drops it from
//!   the windows that list it (`exit_ops.rs`).
//! - The door answers as the client does, deployment owner only.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_api::BoundParams;
use ordo_conformance::Pg;
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op;
use ordo_host::tenants;
use ordo_host::window_ops;
use ordo_host::windows::{
    format_utc, parse_utc, Listing, Schedule, Target, WindowRefusal, WindowSpec, Windows,
};

async fn fresh(stem: &str) -> Pool {
    let pg = Pg::scoped(stem);
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool");
    tenants::ensure_table(&pool).await.expect("$spaces");
    pool
}

async fn tenant(pool: &Pool, id: &str) {
    assert!(tenants::reserve(pool, id, id, "dev:ada", 10, None)
        .await
        .expect("reserved"));
}

fn t(s: &str) -> i64 {
    parse_utc(s).expect("a time")
}

fn ids(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

fn spec(id: &str, target: Target, schedule: Schedule) -> WindowSpec {
    WindowSpec {
        id: Some(id.into()),
        target,
        schedule,
        message: "**Upgrade.** Back by 04:00.".into(),
    }
}

fn times(l: &Listing, window: &str) -> Vec<(String, Option<String>)> {
    l.occurrences
        .iter()
        .filter(|o| o.window == window)
        .map(|o| (format_utc(o.start), o.skipped_by.clone()))
        .collect()
}

const NOW: &str = "2026-11-04T12:00Z";

#[tokio::test(flavor = "multi_thread")]
async fn windows_are_set_listed_skipped_and_deleted() {
    let pool = fresh("ordo_windows_crud").await;
    tenant(&pool, "spc_a").await;
    tenant(&pool, "spc_b").await;
    let w = Windows::on(&pool);
    let now = t(NOW);

    // A one-off range for spc_a only.
    let range = w
        .set(
            &spec(
                "upgrade",
                Target::parse("only", &ids(&["spc_a"])).unwrap(),
                Schedule::Range {
                    start: t("2026-11-20T02:00Z"),
                    end: t("2026-11-20T04:00Z"),
                },
            ),
            "dev:ada",
            now,
        )
        .await
        .expect("set");
    assert_eq!(range.message, "**Upgrade.** Back by 04:00.");

    // Sundays at 02:00 for 2 hours, for everyone but spc_b.
    w.set(
        &spec(
            "weekly",
            Target::parse("except", &ids(&["spc_b"])).unwrap(),
            Schedule::Cron {
                cron: "0 2 * * sun".into(),
                length: 7200,
            },
        ),
        "dev:ada",
        now,
    )
    .await
    .expect("set");

    let all = w.list(None, now).await.expect("listed");
    assert_eq!(format_utc(all.from), "2025-11-04T12:00:00Z");
    assert_eq!(format_utc(all.to), "2027-11-04T12:00:00Z");
    assert_eq!(all.windows.len(), 2);
    assert_eq!(times(&all, "upgrade").len(), 1);
    // About 104 Sundays in 24 months.
    let weekly = times(&all, "weekly");
    assert!((103..=106).contains(&weekly.len()), "{}", weekly.len());
    assert!(all.occurrences.iter().all(|o| o.target
        == if o.window == "weekly" {
            "except"
        } else {
            "only"
        }));
    assert!(all.occurrences.windows(2).all(|p| p[0].start <= p[1].start));

    // Per tenant: what covers it. A tenant made after the window is in
    // `except`'s rule.
    let a = w.list(Some("spc_a"), now).await.unwrap();
    assert_eq!(a.windows.len(), 2);
    let b = w.list(Some("spc_b"), now).await.unwrap();
    assert!(b.windows.is_empty() && b.occurrences.is_empty());
    tenant(&pool, "spc_later").await;
    let later = w.list(Some("spc_later"), now).await.unwrap();
    assert_eq!(later.windows.len(), 1);
    assert_eq!(later.windows[0].id, "weekly");

    // Skip the two Sundays of an incident; each is kept and marked.
    let skip = w
        .skip(
            "weekly",
            Some("incident"),
            t("2026-11-14T00:00Z"),
            t("2026-11-23T00:00Z"),
            Some("frozen during the incident"),
            "dev:ada",
            now,
        )
        .await
        .expect("skipped");
    assert_eq!(skip.window, "weekly");
    let listed = w.list(None, now).await.unwrap();
    let marked: Vec<_> = times(&listed, "weekly")
        .into_iter()
        .filter(|(_, s)| s.is_some())
        .map(|(at, _)| at)
        .collect();
    assert_eq!(marked, vec!["2026-11-15T02:00:00Z", "2026-11-22T02:00:00Z"]);
    assert_eq!(listed.skips.len(), 1);

    // A range has no times to skip.
    let e = w
        .skip(
            "upgrade",
            None,
            t("2026-11-20T00:00Z"),
            t("2026-11-21T00:00Z"),
            None,
            "dev:ada",
            now,
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("ordo windows delete upgrade"), "{e}");

    // The skip goes; then the window, by id, replaced and then deleted.
    w.delete_skip("incident").await.expect("deleted");
    assert!(matches!(
        w.delete_skip("incident").await,
        Err(WindowRefusal::NotFound { what: "skip", .. })
    ));
    w.set(
        &spec(
            "weekly",
            Target::All,
            Schedule::Range {
                start: t("2026-12-01T00:00Z"),
                end: t("2026-12-01T01:00Z"),
            },
        ),
        "dev:bo",
        now,
    )
    .await
    .expect("replaced");
    let replaced = w.get("weekly").await.unwrap().unwrap();
    assert_eq!(replaced.target, Target::All);
    assert_eq!(replaced.set_by, "dev:bo");
    w.delete("weekly").await.expect("deleted");
    assert!(w.get("weekly").await.unwrap().is_none());
    let e = w.delete("weekly").await.unwrap_err();
    assert!(e.to_string().contains("ordo windows list"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_windows_are_refused_saying_what_to_change() {
    let pool = fresh("ordo_windows_refused").await;
    tenant(&pool, "spc_a").await;
    let w = Windows::on(&pool);
    let now = t(NOW);
    let cron = |c: &str, l: i64| Schedule::Cron {
        cron: c.into(),
        length: l,
    };

    let e = w
        .set(
            &spec(
                "x",
                Target::parse("only", &ids(&["spc_a", "spc_ghost"])).unwrap(),
                cron("0 2 * * *", 60),
            ),
            "dev:ada",
            now,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&e, WindowRefusal::UnknownTenants(t) if t == &ids(&["spc_ghost"])),
        "{e}"
    );
    let e = w.list(Some("spc_ghost"), now).await.unwrap_err();
    assert!(e.to_string().contains("spc_ghost"), "{e}");

    let e = w
        .set(
            &spec("x", Target::All, cron("0 2 * * *", 86_400)),
            "dev:ada",
            now,
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("shortest gap"), "{e}");
    let e = w
        .set(&spec("x", Target::All, cron("0 2 * *", 60)), "dev:ada", now)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("5 fields"), "{e}");

    let mut long = spec("x", Target::All, cron("0 2 * * *", 60));
    long.message = "m".repeat(2_001);
    let e = w.set(&long, "dev:ada", now).await.unwrap_err();
    assert!(e.to_string().contains("at most 2000"), "{e}");

    let mut bad_id = spec("x", Target::All, cron("0 2 * * *", 60));
    bad_id.id = Some("Not An Id".into());
    assert!(w.set(&bad_id, "dev:ada", now).await.is_err());
    assert!(w.list(None, now).await.unwrap().windows.is_empty());
}

/// The deploy acts the door wrote to the security record.
fn acts(stream: &ordo_secevent::Recorder) -> Vec<String> {
    stream
        .drain_all(ordo_commandlog::Timestamp(u64::MAX / 2))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::Deploy { act, .. } => Some(act),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_door_sets_and_lists_as_the_client_does() {
    let pool = fresh("ordo_windows_door").await;
    tenant(&pool, "spc_a").await;
    let stream = std::sync::Arc::new(ordo_secevent::Recorder::default());
    let ops = apply_op::ApplyOps::recording(
        pool.clone(),
        std::sync::Arc::new(SinglePool::new(pool.clone())),
        "home",
        Some(stream.clone()),
    );
    let bound = |v: serde_json::Value| BoundParams(v.as_object().unwrap().clone());

    let set = window_ops::execute(
        &ops,
        window_ops::SET_WINDOW,
        "home",
        &bound(serde_json::json!({
            "id": "nightly",
            "target": "only",
            "tenants": ["spc_a"],
            "cron": "0 1 * * *",
            "lengthSecs": 1800,
            "message": "nightly work",
        })),
        "owner",
    )
    .await
    .expect("set");
    let row = &set.rows[0];
    assert_eq!(row["id"], "nightly");
    assert_eq!(row["target"], "only");
    assert_eq!(row["tenants"], serde_json::json!(["spc_a"]));
    assert_eq!(row["setBy"], "owner");

    let both = window_ops::execute(
        &ops,
        window_ops::SET_WINDOW,
        "home",
        &bound(serde_json::json!({
            "target": "all",
            "starts": "2026-12-01T00:00:00Z",
            "ends": "2026-12-01T01:00:00Z",
            "cron": "0 1 * * *",
            "lengthSecs": 60,
            "message": "m",
        })),
        "owner",
    )
    .await
    .unwrap_err();
    assert_eq!(both.code, ordo_api::ErrorCode::InvalidParam);
    assert!(
        both.message.contains("one or the other"),
        "{}",
        both.message
    );

    let skip = window_ops::execute(
        &ops,
        window_ops::SKIP_WINDOW,
        "home",
        &bound(serde_json::json!({
            "window": "nightly",
            "starts": "2026-01-01T00:00:00Z",
            "ends": "2026-01-02T00:00:00Z",
        })),
        "owner",
    )
    .await
    .expect("skipped");
    assert!(skip.rows[0]["id"].as_str().unwrap().starts_with("skp_"));

    let listed = window_ops::execute(
        &ops,
        window_ops::WINDOWS,
        "home",
        &bound(serde_json::json!({ "tenant": "spc_a" })),
        "owner",
    )
    .await
    .expect("listed");
    let back = Listing::from_json(&listed.rows[0]).expect("reads back");
    let direct = Windows::on(&pool)
        .list(Some("spc_a"), back.from + 365 * 86_400)
        .await
        .unwrap();
    assert_eq!(back.windows, direct.windows);
    assert_eq!(back.skips, direct.skips);
    assert_eq!(back.tenant.as_deref(), Some("spc_a"));
    assert!(back.occurrences.iter().all(|o| o.target == "only"));

    // Each change leaves a record on the security stream (G12).
    for (op, params) in [
        (
            window_ops::DELETE_SKIP,
            serde_json::json!({ "id": skip.rows[0]["id"].clone() }),
        ),
        (
            window_ops::DELETE_WINDOW,
            serde_json::json!({ "id": "nightly" }),
        ),
    ] {
        window_ops::execute(&ops, op, "home", &bound(params), "owner")
            .await
            .expect(op);
    }
    assert_eq!(
        acts(&stream),
        [
            "window-set",
            "window-skipped",
            "skip-deleted",
            "window-deleted"
        ]
    );

    // Bound to a tenant's space, none of them is there.
    for op in window_ops::OPS {
        let e = window_ops::execute(&ops, op, "spc_a", &bound(serde_json::json!({})), "owner")
            .await
            .unwrap_err();
        assert_eq!(e.code, ordo_api::ErrorCode::NotFound, "{op}");
    }
}
