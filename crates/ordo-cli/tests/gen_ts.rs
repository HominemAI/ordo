//! `ordo gen ts` — the command the generator-first architecture rests on.
//!
//! `ordo-gen/tests/generated_files.rs` checks what the generator *emits*;
//! this checks the command that *writes* it — that it compiles a real
//! program end to end (source text, not a checked-in descriptor), lands the
//! files where both packages expect them, and is idempotent.
//!
//! Idempotence is the load-bearing one. "Every iteration it's just a quick
//! gen button" is only true if pressing the button twice is a no-op; a
//! generator with any run-to-run variation — a hash-map iteration order, a
//! timestamp — turns every regeneration into a diff nobody can review, which
//! is how a team learns to stop reading them.

use std::path::Path;

use ordo_cli::{generate_ts, Sources};
use ordo_conformance::FIXTURE;

fn sources() -> Sources {
    Sources::from_parts(vec![("market.ordo".to_string(), FIXTURE.to_string())])
}

#[test]
fn it_writes_both_packages_generated_halves() {
    let dir = tempdir("gen-ts-writes");
    let report = generate_ts(&sources(), &dir).expect("the fixture compiles and generates");

    for expected in [
        "clients/ordo-sdk/src/generated/ids.ts",
        "clients/ordo-sdk/src/generated/specs.ts",
        "clients/ordo-sdk/src/generated/ops.ts",
        "clients/ordo-sdk/src/generated/index.ts",
        "clients/ordo-cdk/src/generated/vocabulary.ts",
        "clients/ordo-cdk/src/generated/lang.ts",
        "clients/ordo-cdk/src/generated/schema.ts",
        "clients/ordo-cdk/src/generated/index.ts",
    ] {
        assert!(
            report.contains(expected),
            "`ordo gen ts` did not report {expected}; it reported:\n{report}"
        );
        assert!(
            dir.join(expected).is_file(),
            "{expected} was reported and not written"
        );
    }

    // One command, both packages. A package whose two halves came from two
    // commands is a package that ships with one of them stale.
    assert!(report.contains("ordo-sdk") && report.contains("ordo-cdk"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn generating_twice_gives_identical_bytes() {
    let a = tempdir("gen-ts-a");
    let b = tempdir("gen-ts-b");
    generate_ts(&sources(), &a).expect("first run");
    generate_ts(&sources(), &b).expect("second run");

    let mut compared = 0;
    for rel in [
        "clients/ordo-sdk/src/generated/ops.ts",
        "clients/ordo-sdk/src/generated/specs.ts",
        "clients/ordo-sdk/src/generated/ids.ts",
        "clients/ordo-cdk/src/generated/lang.ts",
        "clients/ordo-cdk/src/generated/schema.ts",
        "clients/ordo-cdk/src/generated/vocabulary.ts",
    ] {
        let x = std::fs::read_to_string(a.join(rel)).expect(rel);
        let y = std::fs::read_to_string(b.join(rel)).expect(rel);
        assert_eq!(
            x, y,
            "{rel} differs between two runs over the same source. \
             Regeneration has to be a no-op when nothing changed, or every \
             `ordo gen ts` is a diff nobody can review."
        );
        assert!(!x.is_empty(), "{rel} is empty");
        compared += 1;
    }
    assert_eq!(compared, 6);
    std::fs::remove_dir_all(&a).ok();
    std::fs::remove_dir_all(&b).ok();
}

/// The language layer does not depend on the deployment.
///
/// `vocabulary.ts` and `lang.ts` are a projection of the parser's tables, so
/// two different programs must produce the same bytes for them — otherwise
/// something about the schema has leaked into the language layer, and the
/// two-layer story in the README is wrong.
#[test]
fn the_language_layer_is_the_same_for_every_program() {
    let a = tempdir("gen-ts-lang-a");
    let b = tempdir("gen-ts-lang-b");
    generate_ts(&sources(), &a).expect("market");
    generate_ts(
        &Sources::from_parts(vec![(
            "tiny.ordo".to_string(),
            "domain tiny\n\nentity notes {\n  body: text\n}\n\n\
             op allNotes unbounded =\n  notes[body]\n"
                .to_string(),
        )]),
        &b,
    )
    .expect("a one-entity program");

    for rel in [
        "clients/ordo-cdk/src/generated/lang.ts",
        "clients/ordo-cdk/src/generated/vocabulary.ts",
    ] {
        assert_eq!(
            std::fs::read_to_string(a.join(rel)).expect(rel),
            std::fs::read_to_string(b.join(rel)).expect(rel),
            "{rel} differs between two programs, so the LANGUAGE layer is \
             carrying something about the SCHEMA"
        );
    }
    // …and the schema layer must differ, or the test above is vacuous.
    let sa = std::fs::read_to_string(a.join("clients/ordo-cdk/src/generated/schema.ts")).unwrap();
    let sb = std::fs::read_to_string(b.join("clients/ordo-cdk/src/generated/schema.ts")).unwrap();
    assert_ne!(
        sa, sb,
        "the schema layer did not notice a different program"
    );
    assert!(sb.contains("notes") && !sb.contains("orderItems"));

    std::fs::remove_dir_all(&a).ok();
    std::fs::remove_dir_all(&b).ok();
}

/// DL §2 Parameters, `filled`: "the generated client fills it when the
/// caller does not". A filled `date` or `datetime` is optional in the
/// params type and filled from the caller's clock before the call is built;
/// an unmarked one stays required.
#[test]
fn a_filled_parameter_is_filled_by_the_generated_client() {
    let dir = tempdir("gen-ts-filled");
    generate_ts(
        &Sources::from_parts(vec![(
            "due.ordo".to_string(),
            "domain due\n\nentity tasks {\n  title: text\n  due: date\n  at: datetime\n}\n\n\
             op dueBy($today: date filled, $now: datetime filled, $until: date) =\n  \
             tasks(due >= $today and due <= $until and at <= $now)[title]\n  sort title\n"
                .to_string(),
        )]),
        &dir,
    )
    .expect("a filled parameter compiles and generates");
    let ops =
        std::fs::read_to_string(dir.join("clients/ordo-sdk/src/generated/ops.ts")).expect("ops.ts");
    assert!(ops.contains("readonly today?: string;"), "{ops}");
    assert!(ops.contains("readonly now?: string;"), "{ops}");
    assert!(
        ops.contains("readonly until: string;"),
        "an unmarked date stays required:\n{ops}"
    );
    assert!(
        ops.contains("{ ...params, today: params.today ?? filledDate(), now: params.now ?? filledDatetime() }"),
        "{ops}"
    );
    assert!(ops.contains("function filledDate(): string"), "{ops}");
    assert!(ops.contains("function filledDatetime(): string"), "{ops}");
    std::fs::remove_dir_all(&dir).ok();
}

/// 28b: a `json` field and a `json` parameter are `JsonValue` in the
/// generated client, imported from the SDK's core, and the CDK has
/// `t.json()`.
#[test]
fn the_generated_ts_type_of_a_json_field_is_json_value() {
    let dir = tempdir("gen-ts-json");
    generate_ts(
        &Sources::from_parts(vec![(
            "pages.ordo".to_string(),
            "domain pages\n\nentity pages {\n  title: text\n  doc: json = {}\n  draft: json?\n}\n\n\
             op savePage($title: text, $doc: json) =\n  set pages { title: $title, doc: $doc }\n\n\
             op allPages unbounded =\n  pages[title, doc, draft]\n  sort title\n"
                .to_string(),
        )]),
        &dir,
    )
    .expect("a json field compiles and generates");
    let ops =
        std::fs::read_to_string(dir.join("clients/ordo-sdk/src/generated/ops.ts")).expect("ops.ts");
    assert!(ops.contains("readonly doc: JsonValue;"), "{ops}");
    assert!(ops.contains("readonly draft: JsonValue;"), "{ops}");
    assert!(
        ops.contains("import type { JsonValue } from '../core/json.js';"),
        "{ops}"
    );
    let specs = std::fs::read_to_string(dir.join("clients/ordo-sdk/src/generated/specs.ts"))
        .expect("specs.ts");
    assert!(specs.contains("type: 'json'"), "{specs}");
    let lang = std::fs::read_to_string(dir.join("clients/ordo-cdk/src/generated/lang.ts"))
        .expect("lang.ts");
    assert!(lang.contains("json: (): TypeSpec =>"), "{lang}");
    std::fs::remove_dir_all(&dir).ok();
}

/// A program that does not compile generates nothing.
///
/// Half a generated package is worse than none: the next `tsc` fails
/// somewhere unrelated, and the author looks for the bug in TypeScript.
#[test]
fn a_program_that_does_not_compile_generates_nothing() {
    let dir = tempdir("gen-ts-refuses");
    let err = generate_ts(
        &Sources::from_parts(vec![(
            "bad.ordo".to_string(),
            "entity notes {\n  body: nosuchtype\n}\n".to_string(),
        )]),
        &dir,
    )
    .expect_err("an unknown type must refuse");
    assert!(
        err.contains("nosuchtype"),
        "the refusal names the mistake: {err}"
    );
    assert!(
        !dir.join("clients").exists(),
        "a refused program wrote files anyway"
    );
    std::fs::remove_dir_all(&dir).ok();
}

fn tempdir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ordo-{tag}-{}-{}",
        std::process::id(),
        // Distinct per call within a process, so two directories in one test
        // cannot collide.
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp directory");
    assert!(Path::new(&dir).is_dir());
    dir
}
