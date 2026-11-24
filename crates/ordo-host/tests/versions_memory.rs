//! **What serving many versions costs in memory** (`plan-versions.md`
//! §1.5, increment 3: "memory at 1,000+ spaces, measured and recorded").
//!
//! Each figure is this process's resident memory (`ps`), before and after.
//! The workspace forbids `unsafe`, so there is no counting allocator. For
//! each example program:
//!
//! - one `Loaded` (the compiled program, its registry and its manifests);
//! - one `SpaceView` per space, cut for 1,000 spaces, and the time a cut
//!   takes;
//! - the views evicted (`Loaded::evict_views`), so the cache is empty.
//!
//! The numbers are printed, and recorded in the plan. Run it in release,
//! alone:
//! `cargo test -p ordo-host --release --test versions_memory -- --nocapture`.
//! The asserts hold what the settings rely on: eviction empties the cache
//! and a cap keeps the most recently used. The figures are only printed.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`) for
//! the pool a view holds; no query runs.

use std::sync::Arc;

use ordo_conformance::{Fixture, Pg};
use ordo_exec::{Pool, PoolConfig};

/// This process's resident memory in bytes, as `ps` reports it on macOS
/// and Linux.
fn resident() -> i64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<i64>()
        .expect("a number of KB")
        * 1024
}

const SPACES: usize = 1_000;

struct Measured {
    program: &'static str,
    source_bytes: usize,
    loaded: i64,
    per_view: i64,
    cut_us: f64,
}

async fn measure(program: &'static str, source: &str, pool: &Pool) -> Measured {
    let before = resident();
    let fixture = Arc::new(Fixture::from_source(source, "").expect("compiles"));
    let exec = ordo_exec::Executor {
        pool: pool.clone(),
        schema: fixture.schema.clone(),
        registry: fixture.registry.clone(),
        keys: fixture.keys.clone(),
        space: ordo_conformance::fixture_space(),
    };
    let d = ordo_host::deploy::Deployment::pinned(program, fixture, exec);
    let loaded = resident() - before;

    let live = d.current();
    let start = resident();
    let t = std::time::Instant::now();
    for n in 0..SPACES {
        let _ = live.in_space(&format!("spc_{n:04}"), pool.clone());
    }
    let cut_us = t.elapsed().as_secs_f64() * 1e6 / SPACES as f64;
    let per_view = (resident() - start) / SPACES as i64;
    assert_eq!(live.view_count(), SPACES);

    // Evicted: every view no request holds.
    assert_eq!(live.evict_views(0, u64::MAX), SPACES, "every view evicted");
    assert_eq!(live.view_count(), 0);
    // And a cap keeps the most recently used.
    for n in 0..10 {
        let _ = live.in_space(&format!("spc_{n:04}"), pool.clone());
    }
    assert_eq!(live.evict_views(u64::MAX, 4), 6, "down to the cap");
    assert_eq!(live.view_count(), 4);
    Measured {
        program,
        source_bytes: source.len(),
        loaded,
        per_view,
        cut_us,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_per_version_and_per_space_view() {
    let pg = Pg::scoped("ordo_versions_memory");
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 1,
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool");
    let programs: [(&str, &str); 3] = [
        (
            "teamspace",
            include_str!("../../../examples/teamspace/teamspace.ordo"),
        ),
        (
            "shopfront",
            include_str!("../../../examples/shopfront/shopfront.ordo"),
        ),
        (
            "notes-v2",
            include_str!("../../../examples/notes-v2/notes.ordo"),
        ),
    ];
    let mut out = Vec::new();
    for (name, source) in programs {
        out.push(measure(name, source, &pool).await);
    }
    println!("\n| Program | Source | One Loaded | One view | 1,000 views | Cut |");
    println!("|---|---|---|---|---|---|");
    for m in &out {
        println!(
            "| {} | {} KB | {} KB | {} KB | {} MB | {:.0} µs |",
            m.program,
            m.source_bytes / 1024,
            m.loaded / 1024,
            m.per_view / 1024,
            m.per_view * SPACES as i64 / (1024 * 1024),
            m.cut_us,
        );
    }
    // No assert on the figures: resident memory is a report, not a
    // measure to hold. On Linux the allocator reuses the pages the last
    // program's evicted views gave back, so a later program can show 0.
}
