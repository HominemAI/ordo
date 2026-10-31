//! The reserved plan tier (`plan-tiers.md`, "Build placement"; owner ruling
//! 2026-10-23). Every op artifact carries `tier`, T0 for every op today, and
//! the version hash covers it, so a later tier is a new value of a key that
//! is already hashed rather than a new key that moves every version.

use ordo_plan::artifact::{op_artifact, op_version, Tier};
use ordo_sema::Registry;
use serde_json::json;

const SRC: &str = "\
entity tasks {
  title: text
  done:  bool
}

op openTasks =
  tasks(done == false)[title]
  sort title
  take 50

op doneTasks =
  tasks(done == true)[title]
  take 10
";

fn artifact(name: &str) -> serde_json::Value {
    let program = ordo_syntax::parse_program(SRC).expect("parses");
    let schema = ordo_schema::build(&program).expect("builds");
    let registry = Registry::from_program(&program);
    op_artifact(&schema, &registry, name).expect("compiles")
}

#[test]
fn every_op_artifact_carries_the_direct_tier() {
    for name in ["openTasks", "doneTasks"] {
        let art = artifact(name);
        assert_eq!(
            art["tier"],
            json!({ "id": "T0", "kind": "direct" }),
            "{name}"
        );
        assert_eq!(art["tier"], Tier::Direct.json());
    }
}

#[test]
fn the_version_hash_covers_the_tier_and_is_stable_for_a_given_tier() {
    let art = artifact("openTasks");
    let version = art["version"].as_str().expect("version").to_string();

    // The stamped version is the recomputable one, and compiling again gives
    // the same bytes.
    assert_eq!(op_version(&art), version);
    assert_eq!(artifact("openTasks")["version"], json!(version));

    // The same tier written again keeps the version.
    let mut same = art.clone();
    same["tier"] = Tier::Direct.json();
    assert_eq!(op_version(&same), version);

    // A different tier is a different artifact with a different version.
    let mut other = art.clone();
    other["tier"] = json!({ "id": "T1", "kind": "index-assisted" });
    assert_ne!(op_version(&other), version);
}
