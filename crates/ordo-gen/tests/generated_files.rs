//! The checked-in generated halves **are** what the generator emits.
//!
//! Without this, "generated from the implementation's own canonical tables,
//! never hand-maintained" is a claim about a command somebody ran once. A
//! keyword could be added, `spell.rs` given an arm, and `clients/ordo-cdk/
//! src/generated/lang.ts` left at yesterday's bytes — with both TypeScript
//! suites green, because they test the file that is there.
//!
//! So the whole emission is compared against the tree, file by file, and the
//! failure names `ordo gen ts`. This is the same discipline
//! `ordo-cli/tests/golden` uses for the artifact and the descriptor, and the
//! same regeneration variable.
//!
//! # The descriptor this runs against
//!
//! `crates/ordo-cli/tests/golden/market.interface.json` — the checked-in
//! descriptor of the conformance fixture, which `ordo-cli/tests/interface.rs`
//! already keeps joined to the compiler in both directions. Reading it here
//! rather than compiling the fixture is what keeps this crate free of a
//! schema, a registry and a binder: `ordo-gen` takes JSON in and text out,
//! for the reason `ordo_plan::interface` gives about itself.

use std::path::{Path, PathBuf};

use serde_json::Value;

const GOLDEN_DESCRIPTOR: &str =
    include_str!("../../../crates/ordo-cli/tests/golden/market.interface.json");

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

fn descriptor() -> Value {
    serde_json::from_str(GOLDEN_DESCRIPTOR).expect("the golden descriptor is JSON")
}

const HOW: &str = "\n\nregenerate with:\n  \
                   cargo run -p ordo-cli --bin ordo -- gen ts \
                   crates/ordo-conformance/fixtures/market.ordo\n\
                   then READ the diff. Both TypeScript packages' `src/generated/` \
                   halves are a pure function of the language's tables and this \
                   descriptor; an edit to one by hand is lost the next time \
                   anybody runs that command.";

#[test]
fn the_checked_in_generated_halves_are_what_the_generator_emits() {
    let root = repo_root();
    let files = ordo_gen::generate(&descriptor()).expect("the generator runs");
    assert!(
        files.len() >= 8,
        "the generator emitted {} files; both packages need theirs",
        files.len()
    );

    let mut stale = Vec::new();
    for f in &files {
        let path = root.join(&f.path);
        match std::fs::read_to_string(&path) {
            Ok(on_disk) if on_disk == f.contents => {}
            Ok(_) => stale.push(format!("{} is STALE", f.path)),
            Err(_) => stale.push(format!("{} is MISSING", f.path)),
        }
    }
    assert!(stale.is_empty(), "{}{HOW}", stale.join("\n  "));
}

/// Every generated file says it is generated, in its first line.
///
/// The split between a hand-written core and a generated surface is only real
/// if a reader who opens one file can tell which half they are in. Otherwise
/// the first person to fix a bug fixes it in the generated half and loses the
/// fix on the next run — silently, and in production.
#[test]
fn every_generated_file_announces_itself() {
    for f in ordo_gen::generate(&descriptor()).expect("the generator runs") {
        let first = f.contents.lines().next().unwrap_or_default();
        assert!(
            first.contains("GENERATED") && first.contains("ordo gen ts"),
            "{}: the first line must say the file is generated and how to \
             regenerate it, and it says {first:?}",
            f.path
        );
        assert!(
            f.contents.contains("src/core/"),
            "{}: the header must name the hand-written half, or the boundary \
             is only in the README",
            f.path
        );
    }
}

/// Nothing hand-written lives under a `generated/` directory.
///
/// The reverse of the test above: a file that IS in the generated directory
/// and is not emitted would survive every regeneration, be edited freely, and
/// look exactly like the rest.
#[test]
fn no_stowaways_in_the_generated_directories() {
    let root = repo_root();
    let emitted: std::collections::BTreeSet<String> = ordo_gen::generate(&descriptor())
        .expect("the generator runs")
        .into_iter()
        .map(|f| f.path)
        .collect();

    let mut stowaways = Vec::new();
    for dir in [ordo_gen::SDK_OUT, ordo_gen::CDK_OUT] {
        let full = root.join(dir);
        let entries = std::fs::read_dir(&full)
            .unwrap_or_else(|e| panic!("{}: {e} — has `ordo gen ts` ever run?", full.display()));
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let rel = format!("{dir}/{name}");
            if !emitted.contains(&rel) {
                stowaways.push(rel);
            }
        }
    }
    assert!(
        stowaways.is_empty(),
        "these files sit in a `generated/` directory and the generator does not \
         emit them, so they would survive every regeneration and be edited \
         freely while looking generated:\n  {}",
        stowaways.join("\n  ")
    );
}

/// The generator refuses a descriptor whose version it does not speak.
///
/// A descriptor version moves on a **changed meaning**, never on an addition
/// (`ordo_plan::interface::SCHEMA_VERSION` argues the case at length). So
/// generating against an unknown one would emit code that reads a key the way
/// it used to mean — the failure mode that is worse than a missing key,
/// because the caller gets a wrong answer rather than none.
#[test]
fn a_descriptor_from_another_version_is_refused() {
    let mut doc = descriptor();
    doc["schema_version"] = Value::String("ordo.interface/3".into());
    let err = ordo_gen::generate(&doc).expect_err("a /3 descriptor must be refused");
    assert!(
        err.contains("ordo.interface/3") && err.contains("ordo.interface/2"),
        "the refusal must name both versions, or nobody can tell which end is \
         behind: {err}"
    );
}

/// The version this generator speaks is the one the descriptor carries.
///
/// `ordo-gen` cannot depend on `ordo-plan` (that crate reaches a schema and a
/// binder, and this one must not), so the version string is restated here.
/// A restatement across a boundary that must not be crossed is exactly where
/// this repository puts a two-directional check instead of a comment.
#[test]
fn the_generator_speaks_the_descriptors_version() {
    let doc = descriptor();
    assert!(
        ordo_gen::generate(&doc).is_ok(),
        "the generator refuses the descriptor `ordo interface` actually emits: {}",
        doc["schema_version"]
    );
}
