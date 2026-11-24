//! `ordo fix <dir> [--write]` (`plan-versions.md` §2.8 step R4): rewrite a
//! source from an old language edition to the current one.
//!
//! Only edition 1 is released, so each run that needs an old edition makes
//! edition 2 current with the test build's flag `ORDO_TEST_EDITION_2_SINCE`.
//!
//! A regression looks like a dry run that writes, a write whose program
//! compiles to something else (a new version on apply), a rewrite that
//! leaves bare `money`, or an unknown flag taken as a dry run.

mod support;

use std::path::{Path, PathBuf};

use ordo_plan::edition::{TEST_EDITION_2_SINCE, TEST_RELEASE_DATE};

/// Edition 1 over 2 files: the header in one, a bare `money` in each.
const SHOP: &str = "# The shop.

domain shop @ 0.1.0

entity items {
  name:  text
  price: money
}

op itemList =
  items[name, price]
  sort name
  take 50
";

const ORDERS: &str = "entity orders {
  item -> items
  total: money = 0
}
";

fn program(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ordo-fix-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a_shop.ordo"), SHOP).unwrap();
    std::fs::write(dir.join("b_orders.ordo"), ORDERS).unwrap();
    dir
}

/// `ordo <args>`, with edition 2 current when `edition_2` is set.
fn ordo(args: &[&str], edition_2: bool) -> (i32, String, String) {
    let mut c = support::ordo();
    c.args(args).env_remove(TEST_RELEASE_DATE);
    if edition_2 {
        c.env(TEST_EDITION_2_SINCE, "2026-01-01");
    } else {
        c.env_remove(TEST_EDITION_2_SINCE);
    }
    let out = c.output().expect("runs ordo");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn read(dir: &Path, file: &str) -> String {
    std::fs::read_to_string(dir.join(file)).unwrap()
}

/// The dry run shows the diff and writes nothing; `--write` writes it, and
/// the program compiles to the same artifact as before.
#[test]
fn a_dry_run_shows_the_diff_then_write_rewrites_to_the_current_edition() {
    let dir = program("write");
    let d = dir.to_str().unwrap();
    let (_, before, err) = ordo(&["plan", d], true);
    assert!(!before.is_empty(), "{err}");

    let (code, out, err) = ordo(&["fix", d], true);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("edition 1 to edition 2"), "{out}");
    assert!(out.contains("+edition 2"), "{out}");
    assert!(out.contains("-  price: money\n+  price: money(0)"), "{out}");
    assert!(
        out.contains("-  total: money = 0\n+  total: money(0) = 0"),
        "{out}"
    );
    assert!(out.contains("dry run: nothing written"), "{out}");
    assert!(out.contains("--write"), "{out}");
    assert_eq!(read(&dir, "a_shop.ordo"), SHOP);
    assert_eq!(read(&dir, "b_orders.ordo"), ORDERS);

    let (code, out, err) = ordo(&["fix", d, "--write"], true);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("wrote "), "{out}");
    let shop = read(&dir, "a_shop.ordo");
    assert!(shop.contains("domain shop @ 0.1.0\nedition 2\n"), "{shop}");
    assert!(shop.contains("price: money(0)"), "{shop}");
    assert!(!read(&dir, "b_orders.ordo").contains("edition"));
    assert!(read(&dir, "b_orders.ordo").contains("total: money(0) = 0"));

    // The same program: the artifact is byte-identical.
    let (code, after, err) = ordo(&["plan", d], true);
    assert_eq!(code, 0, "{err}");
    assert_eq!(after, before);

    // Nothing left to change.
    let (code, out, _) = ordo(&["fix", d], true);
    assert_eq!(code, 0);
    assert!(
        out.contains("is at edition 2, the current edition"),
        "{out}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Under the released editions there is nothing to rewrite, and an edition
/// this release does not know is refused naming the way out.
#[test]
fn a_source_at_the_current_edition_is_left_alone() {
    let dir = program("current");
    let d = dir.to_str().unwrap();
    let (code, out, err) = ordo(&["fix", d, "--write"], false);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("is at edition 1, the current edition"),
        "{out}"
    );
    assert_eq!(read(&dir, "a_shop.ordo"), SHOP);

    std::fs::write(
        dir.join("a_shop.ordo"),
        SHOP.replace("@ 0.1.0\n", "@ 0.1.0\nedition 2\n"),
    )
    .unwrap();
    let (code, _, err) = ordo(&["fix", d], false);
    assert_eq!(code, 1);
    assert!(err.contains("newer than this Ordo release knows"), "{err}");
    let (code, _, err) = ordo(&["check", d], false);
    assert_eq!(code, 1);
    assert!(err.contains("edition_unknown"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A mistyped flag is refused, never taken as a dry run.
#[test]
fn an_unknown_flag_is_refused() {
    let dir = program("flag");
    let (code, _, err) = ordo(&["fix", dir.to_str().unwrap(), "--wirte"], true);
    assert_eq!(code, 2);
    assert!(err.contains("`--write`"), "{err}");
    assert_eq!(read(&dir, "a_shop.ordo"), SHOP);
    let _ = std::fs::remove_dir_all(&dir);
}
