//! Checklist 9u, proved where it matters: against a real server.
//!
//! The point of NFC-on-write is not that a Rust function composes strings —
//! that is `unicode-normalization`'s job and its own tests cover it. The
//! point is that **the database sees one form**, so that two spellings of one
//! text sort together, compare equal, and cannot both occupy a `unique`
//! column. None of those three is observable from Rust alone, so all three
//! are asserted here.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::time::Duration;

use ordo_conformance::Pg;
use ordo_exec::nfc::{collisions, unnormalized};
use ordo_exec::{Pool, PoolConfig};
use ordo_syntax::nfc;

const COMPOSED: &str = "\u{00e9}"; // é
const DECOMPOSED: &str = "e\u{0301}"; // e + combining acute

async fn pool(db: &str) -> Pool {
    // `Pg::named` creates the database; the pool then connects to it.
    let _ = Pg::named(db);
    Pool::connect(PoolConfig {
        db: db.into(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot")
}

/// The premise. If Postgres considered these equal on its own there would be
/// nothing to fix, and the rest of this file would be theatre.
#[tokio::test(flavor = "multi_thread")]
async fn postgres_does_not_do_this_for_us() {
    let pool = pool("ordo_nfc_premise").await;
    let rows = pool
        .rows(&format!("SELECT '{COMPOSED}' = '{DECOMPOSED}'"))
        .await
        .unwrap();
    assert_eq!(
        rows[0][0].as_deref(),
        Some("f"),
        "the two spellings are unequal to the server — which is the bug 9u closes"
    );

    // And they straddle a third value under the byte order G11 pinned, so
    // this is an ORDERING defect as much as an equality one: a keyset page
    // seeking past `f` would skip one spelling and not the other.
    let rows = pool
        .rows(&format!(
            "SELECT x FROM (VALUES ('{DECOMPOSED}'), ('f'), ('{COMPOSED}')) t(x) \
             ORDER BY x COLLATE \"C\""
        ))
        .await
        .unwrap();
    let got: Vec<&str> = rows.iter().map(|r| r[0].as_deref().unwrap()).collect();
    assert_eq!(got, vec![DECOMPOSED, "f", COMPOSED]);
}

/// The fix, end to end: normalized on the way in, the server sees one value.
#[tokio::test(flavor = "multi_thread")]
async fn one_text_is_one_value_to_the_server() {
    let pool = pool("ordo_nfc_equal").await;
    let rows = pool
        .rows(&format!(
            "SELECT '{}' = '{}'",
            nfc(COMPOSED),
            nfc(DECOMPOSED)
        ))
        .await
        .unwrap();
    assert_eq!(rows[0][0].as_deref(), Some("t"));
}

/// A `unique` column is where the defect stops being cosmetic: today both
/// spellings fit, which is the language telling one lie in two places — the
/// column claims the values are distinct and the rule says they are one text.
#[tokio::test(flavor = "multi_thread")]
async fn a_unique_column_admits_both_spellings_until_it_does_not() {
    let pool = pool("ordo_nfc_unique").await;
    pool.rows("DROP TABLE IF EXISTS u").await.unwrap();
    pool.rows("CREATE TABLE u (v text UNIQUE)").await.unwrap();

    // Un-normalized: both land. This is the pre-9u deployment.
    pool.rows(&format!("INSERT INTO u VALUES ('{COMPOSED}')"))
        .await
        .unwrap();
    pool.rows(&format!("INSERT INTO u VALUES ('{DECOMPOSED}')"))
        .await
        .expect("both spellings fit a unique column today — the defect");

    // Normalized, the second is what it always was: the same text.
    pool.rows("DROP TABLE IF EXISTS n").await.unwrap();
    pool.rows("CREATE TABLE n (v text UNIQUE)").await.unwrap();
    pool.rows(&format!("INSERT INTO n VALUES ('{}')", nfc(COMPOSED)))
        .await
        .unwrap();
    pool.rows(&format!("INSERT INTO n VALUES ('{}')", nfc(DECOMPOSED)))
        .await
        .expect_err("after 9u the second spelling is a duplicate, and says so");
}

/// The migration probe, on the table the previous test built by hand: it must
/// find the exposure AND name the collision **before** anything is written,
/// because the alternative is discovering it mid-migration against a
/// constraint, with half the table converted.
#[tokio::test(flavor = "multi_thread")]
async fn the_probe_finds_the_hazard_before_a_migration_hits_it() {
    let pool = pool("ordo_nfc_probe").await;
    pool.rows("DROP TABLE IF EXISTS legacy").await.unwrap();
    pool.rows("CREATE TABLE legacy (v text)").await.unwrap();
    for v in [COMPOSED, DECOMPOSED, "plain", "also_plain"] {
        pool.rows(&format!("INSERT INTO legacy VALUES ('{v}')"))
            .await
            .unwrap();
    }

    let found = unnormalized(&pool, &[("legacy", "v")]).await.unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(
        found[0].rows, 1,
        "exactly the decomposed row — the composed one and the ASCII ones are \
         already in their stored form and are not work"
    );

    let clashes = collisions(&pool, &[("legacy", "v")]).await.unwrap();
    assert_eq!(clashes.len(), 1, "{clashes:?}");
    assert_eq!(clashes[0].value, nfc(COMPOSED));
    assert_eq!(clashes[0].rows, 2, "two rows would become one value");

    // …and it reports rather than repairs. A probe that fixed what it found
    // would be deciding which of the two rows survives, which is the
    // deployment's call and not the platform's.
    let still = unnormalized(&pool, &[("legacy", "v")]).await.unwrap();
    assert_eq!(still, found, "the probe wrote nothing");
}

/// The safe case, so the probe is useful rather than merely alarming: a
/// column with work to do and nothing to lose reports the first and not the
/// second, which is what lets a migration run unattended.
#[tokio::test(flavor = "multi_thread")]
async fn work_without_collisions_is_distinguishable_from_danger() {
    let pool = pool("ordo_nfc_safe").await;
    pool.rows("DROP TABLE IF EXISTS safe").await.unwrap();
    pool.rows("CREATE TABLE safe (v text)").await.unwrap();
    for v in [DECOMPOSED, "plain"] {
        pool.rows(&format!("INSERT INTO safe VALUES ('{v}')"))
            .await
            .unwrap();
    }

    assert_eq!(
        unnormalized(&pool, &[("safe", "v")]).await.unwrap().len(),
        1
    );
    assert!(
        collisions(&pool, &[("safe", "v")])
            .await
            .unwrap()
            .is_empty(),
        "nothing would merge, so this column converts unattended"
    );
}
