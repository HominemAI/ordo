//! P7's two done-when clauses, tested: `ordo check <dir>` works on a
//! directory, and `ordo plan` emits byte-stable artifact JSON.
//!
//! Regenerate the golden with:
//!
//! ```text
//! ORDO_UPDATE_GOLDEN=1 cargo test --workspace
//! ```
//!
//! then read the diff. See `ordo_conformance::golden`.

use std::path::{Path, PathBuf};

use ordo_cli::{check_sources, plan_artifact, Sources};
use ordo_conformance::{assert_golden, FIXTURE};

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

fn one(text: &str) -> Sources {
    Sources::from_parts(vec![("market.ordo".into(), text.to_string())])
}

// ── the artifact ─────────────────────────────────────────────────────────

/// P7: `ordo plan` emits byte-stable artifact JSON. The golden is the
/// market fixture's whole artifact — every entity, every op, the guard
/// sets with provenance, the flags, the SQL.
#[test]
fn market_fixture_artifact_is_byte_stable() {
    let out = plan_artifact(&one(FIXTURE), None).expect("the fixture plans");
    assert_golden(&golden("market.artifact.json"), &out);
}

/// The proof the golden cannot give on its own: the same program produces
/// the same bytes twice in one run, from two independent compilations.
///
/// Two compilations, not two serialisations of one document — the hazard
/// is `Registry`'s `HashMap`, and a single `Registry` walked twice can
/// give the same order twice while still being unordered by construction.
/// Two `Registry`s built from the same text are two different hash states.
#[test]
fn generating_twice_gives_identical_bytes() {
    let a = plan_artifact(&one(FIXTURE), None).expect("plans");
    let b = plan_artifact(&one(FIXTURE), None).expect("plans");
    assert_eq!(a, b, "the artifact is not byte-stable across two runs");
}

/// A concrete stability hazard, isolated: the op list must come out in
/// name order however the `HashMap` behind it feels today.
#[test]
fn ops_are_in_name_order() {
    let out = plan_artifact(&one(FIXTURE), None).expect("plans");
    let doc: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    let names: Vec<&str> = doc["ops"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["op"].as_str().unwrap())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "op order is not name order");
    assert!(names.len() > 50, "the fixture registers the whole corpus");
}

/// The contract's shape rule: every op object carries the same key set,
/// whatever its kind and whether or not it compiled. A key that appears
/// only sometimes turns "this op gained a property" and "this op changed"
/// into the same diff.
#[test]
fn every_op_object_carries_the_same_keys() {
    let out = plan_artifact(&one(FIXTURE), None).expect("plans");
    let doc: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    let ops = doc["ops"].as_array().unwrap();
    let keys = |v: &serde_json::Value| {
        v.as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<String>>()
    };
    let first = keys(&ops[0]);
    assert_eq!(
        first,
        vec![
            "bypassed_guards",
            "cost",
            "entities",
            "error",
            "flags",
            "guards",
            "kind",
            "mode",
            "op",
            "outputs",
            "params",
            "plan",
            "release",
            // The read's result-shape facts (anchor table, grouped,
            // statically single-row) — B1's descriptor reads them, and a
            // write op carries the all-negative form rather than no key.
            "result",
            // WHICH ROWS, beside `scope`'s which columns (8s / DL §8). A key
            // rather than a nesting inside `scope`, because the two are
            // derived by different walks over the same entity list and a
            // reader diffing a deploy needs to see which of them moved.
            "row_scope",
            "scope",
            "statements",
            // The plan tier, T0 (direct) for every op until plan tiers
            // land (`plan-tiers.md`, ruled 2026-10-23).
            "tier",
            "version",
        ]
    );
    for op in ops {
        assert_eq!(keys(op), first, "`{}` has a different key set", op["op"]);
    }
}

/// The fields the artifact exists for, on ops chosen because each is the
/// only one in the fixture that exercises its clause.
#[test]
fn the_artifact_states_what_a_deploy_review_needs() {
    let out = plan_artifact(&one(FIXTURE), None).expect("plans");
    let doc: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    let op = |n: &str| {
        doc["ops"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["op"] == n)
            .unwrap_or_else(|| panic!("no op `{n}`"))
            .clone()
    };

    // R-G8: the flag never travels alone. `allTasks` is unguarded over
    // guarded `tasks`, and the artifact names what it bypasses.
    let all_tasks = op("allTasks");
    assert_eq!(all_tasks["flags"]["unguarded"], true);
    assert_eq!(
        all_tasks["bypassed_guards"],
        serde_json::json!(["tasks.read"])
    );

    // Guard provenance, which is what makes a consent page honest. `cards`
    // inherits from `boards`, whose guard is acl-derived.
    let cards = op("cardList");
    let sources: Vec<&str> = cards["guards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["source"].as_str().unwrap())
        .collect();
    assert!(
        sources.iter().any(|s| s.starts_with("inherited from")),
        "{sources:?}"
    );

    // Composition reaches through the callee: `cheapActive` wraps
    // `activeProducts`, and the entity it actually reads is `products`.
    assert_eq!(
        op("cheapActive")["entities"],
        serde_json::json!(["products"])
    );

    // A bounded read states its bound as a number, not as prose.
    assert_eq!(op("top3Orders")["cost"]["row_bound"], 3);
    assert_eq!(
        op("activeProducts")["cost"]["row_bound"],
        serde_json::Value::Null
    );

    // Derived write mode, and the §6-rule-8 refusal the fixture carries.
    assert_eq!(op("addTask")["kind"], "write");
    assert_eq!(op("addTask")["mode"], "creates");
    assert_eq!(op("claimCoupon")["mode"], "ambiguous-identity");
    assert_eq!(op("dropCategory")["kind"], "delete");

    // A sealed output is marked from the plan's own host-open obligation,
    // never guessed.
    let memos = op("memoDigest");
    let sealed: Vec<&str> = memos["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["sealed"] == true)
        .map(|o| o["name"].as_str().unwrap())
        .collect();
    assert_eq!(sealed, vec!["memo"]);

    // A blind-index op withholds its statement — the digest is derived
    // under a per-space key and publishing it would be both unstable and
    // a membership oracle. Everything else about the op is still stated.
    let by_email = op("customerByEmail");
    assert_eq!(by_email["cost"]["class"], "blind-index");
    assert_eq!(by_email["plan"]["sql"], serde_json::Value::Null);
    assert_eq!(by_email["error"], serde_json::Value::Null);
    // Three outputs, not the two the author wrote: `name`, `tier`, and the
    // `$id` the executor attaches to every anchored read since 2026-09-16
    // (`ui-gen.md` §1). Withholding the STATEMENT and carrying row identity
    // are separate things — a blind-index op hides its digest, not its rows'
    // identity.
    assert_eq!(by_email["outputs"].as_array().unwrap().len(), 3);
    let names: Vec<&str> = by_email["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["name", "tier", "$id"]);

    // An op that does not compile is RECORDED, not fatal: a whole program
    // refusing to describe itself because one op is broken is the least
    // useful answer to "what changed?".
    let gift = op("giftOrders");
    assert_eq!(gift["error"]["code"], "sealed_unsupported");
    assert_eq!(gift["plan"], serde_json::Value::Null);
}

/// Filtering narrows `ops` and nothing else, so a one-op artifact is a
/// subsequence of the full one rather than a second format.
#[test]
fn filtering_by_op_keeps_the_document_shape() {
    let all: serde_json::Value =
        serde_json::from_str(&plan_artifact(&one(FIXTURE), None).unwrap()).unwrap();
    let one_op: serde_json::Value =
        serde_json::from_str(&plan_artifact(&one(FIXTURE), Some("activeProducts")).unwrap())
            .unwrap();
    assert_eq!(one_op["domain"], all["domain"]);
    assert_eq!(one_op["entities"], all["entities"]);
    assert_eq!(one_op["ops"].as_array().unwrap().len(), 1);
    assert_eq!(one_op["ops"][0], all["ops"].as_array().unwrap()[0]);

    assert_eq!(
        plan_artifact(&one(FIXTURE), Some("nosuchop")).unwrap_err(),
        "no op named `nosuchop`"
    );
}

/// The version hash is a digest of the op's own artifact object, so it
/// moves when and only when the contract does. In particular it must NOT
/// move when the declaration moves: a hash over the AST's debug form would
/// carry source spans, and reordering a file would bump every downstream
/// pin without changing anything a caller can observe.
#[test]
fn the_version_hash_ignores_where_the_op_sits_in_the_file() {
    let v = |text: &str, name: &str| -> String {
        let doc: serde_json::Value =
            serde_json::from_str(&plan_artifact(&one(text), Some(name)).unwrap()).unwrap();
        doc["ops"][0]["version"].as_str().unwrap().to_string()
    };
    let base = "entity a { n: text }\n\nop one unbounded =\n  a[n]\n\nop two unbounded =\n  a[n]\n  sort n\n";
    let moved = "entity a { n: text }\n\nop two unbounded =\n  a[n]\n  sort n\n\nop one unbounded =\n  a[n]\n";
    assert_eq!(v(base, "one"), v(moved, "one"));

    // And it DOES move when the op's meaning does.
    let changed = "entity a { n: text }\n\nop one unbounded =\n  a[n]\n  sort n\n\nop two unbounded =\n  a[n]\n  sort n\n";
    assert_ne!(v(base, "one"), v(changed, "one"));
}

// ── `ordo check <dir>` ───────────────────────────────────────────────────

/// A scratch directory named after the test, so two tests never share one
/// and a failure leaves its input behind to look at.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let d = std::env::temp_dir().join(format!("ordo-cli-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("scratch dir");
        Scratch(d)
    }
    fn write(&self, rel: &str, text: &str) {
        let p = self.0.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("scratch subdir");
        }
        std::fs::write(p, text).expect("scratch file");
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Split the fixture at top-level `op` declarations into `n` files, in
/// declaration order. Every line of every declaration stays with its
/// declaration; only the boundaries between them move.
fn split_fixture(n: usize) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut cur = String::new();
    let starts: Vec<usize> = FIXTURE
        .lines()
        .enumerate()
        .filter(|(_, l)| l.starts_with("op "))
        .map(|(i, _)| i)
        .collect();
    let every = starts.len() / n;
    let cut: Vec<usize> = (1..n).map(|k| starts[k * every]).collect();
    for (i, line) in FIXTURE.lines().enumerate() {
        if cut.contains(&i) {
            chunks.push(std::mem::take(&mut cur));
        }
        cur.push_str(line);
        cur.push('\n');
    }
    chunks.push(cur);
    chunks
}

/// The headline: a directory of `.ordo` files compiles to exactly the
/// artifact the same text compiles to as one file. Concatenation in sorted
/// relative-path order is the whole contract, and this is the only test
/// that can catch a boundary that silently loses or fuses a declaration.
#[test]
fn a_directory_of_files_is_the_same_program_as_one_file() {
    let s = Scratch::new("dir-equals-file");
    for (i, chunk) in split_fixture(3).into_iter().enumerate() {
        s.write(&format!("{i}-part.ordo"), &chunk);
    }
    let dir = Sources::read(s.path()).expect("reads the directory");
    assert_eq!(
        dir.labels(),
        vec!["0-part.ordo", "1-part.ordo", "2-part.ordo"]
    );

    let from_dir = plan_artifact(&dir, None).expect("the split fixture plans");
    let from_file = plan_artifact(&one(FIXTURE), None).expect("the whole fixture plans");
    assert_eq!(
        from_dir, from_file,
        "splitting a program across files changed its artifact"
    );
}

/// A single file still works, which is the other half of requirement 1 —
/// and it is the form every existing invocation uses.
#[test]
fn a_single_file_still_works() {
    let s = Scratch::new("single-file");
    s.write("market.ordo", FIXTURE);
    let src = Sources::read(&s.path().join("market.ordo")).expect("reads the file");
    // A single file is labelled by the path the caller gave, so a
    // diagnostic is something an editor can open.
    assert_eq!(src.labels().len(), 1);
    assert!(
        src.labels()[0].ends_with("market.ordo"),
        "{:?}",
        src.labels()
    );
    assert!(check_sources(&src)
        .expect("checks")
        .contains("domain market"));
}

/// File order is sorted relative path, not creation order and not
/// `read_dir` order — including into subdirectories, which is how a real
/// project is laid out.
#[test]
fn file_order_is_sorted_relative_path_not_creation_order() {
    let s = Scratch::new("ordering");
    // Written last-first, and one of them nested.
    s.write("zz.ordo", "op zz unbounded =\n  a[n]\n");
    s.write("nested/mm.ordo", "op mm unbounded =\n  a[n]\n");
    s.write("aa.ordo", "entity a { n: text }\n");
    // A file that is not `.ordo` is not part of the program.
    s.write("notes.txt", "entity ignored { n: text }\n");

    let src = Sources::read(s.path()).expect("reads");
    assert_eq!(src.labels(), vec!["aa.ordo", "nested/mm.ordo", "zz.ordo"]);

    let out = check_sources(&src).expect("checks");
    assert!(
        out.starts_with("SOURCES (3)\n  aa.ordo\n  nested/mm.ordo\n  zz.ordo\n"),
        "{out}"
    );
    assert!(
        !out.contains("ignored"),
        "a .txt file joined the program: {out}"
    );
}

/// A directory with nothing in it is an error, not an empty program. "It
/// checked nothing and passed" is how a CI step goes green on a wrong
/// path.
#[test]
fn an_empty_directory_is_refused() {
    let s = Scratch::new("empty");
    let e = Sources::read(s.path()).unwrap_err();
    assert!(e.contains("no .ordo files here"), "{e}");
}

/// The reason the line map exists: a diagnostic must name the file the
/// author wrote, at that file's own line, not an offset into the buffer
/// the CLI assembled.
#[test]
fn a_diagnostic_names_the_file_and_its_own_line() {
    let s = Scratch::new("diagnostic");
    s.write("a-entities.ordo", "entity a {\n  n: text\n}\n");
    // Line 3 of the SECOND file — line 6 of the concatenation.
    s.write(
        "b-ops.ordo",
        "op good unbounded =\n  a[n]\n\nop bad = a[nosuchfield\n",
    );
    let src = Sources::read(s.path()).expect("reads");
    let e = check_sources(&src).unwrap_err();
    // The buffer position is line 7; the file the author wrote says 4 (the
    // parser points at the line after the unterminated one, so the
    // assertion is on the rebase, not on the parser's choice of span).
    assert!(e.starts_with("b-ops.ordo:"), "{e}");
    let line: u32 = e
        .split(':')
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("{e}"));
    assert!(
        (4..=5).contains(&line),
        "not rebased into the second file: {e}"
    );
}

/// The line map itself, exactly, without a parser in the way.
#[test]
fn the_line_map_rebases_every_file_to_its_own_first_line() {
    let src = Sources::from_parts(vec![
        ("a.ordo".into(), "1\n2\n3\n".into()),
        // No trailing newline: the next file must still start on its own
        // line, and the map must account for the separator that inserts.
        ("b.ordo".into(), "1\n2".into()),
        ("c.ordo".into(), "1\n".into()),
    ]);
    let at = |line, col| src.at(ordo_syntax::Span::new(0, 0, line, col));
    assert_eq!(at(1, 1), "a.ordo:1:1");
    assert_eq!(at(3, 7), "a.ordo:3:7");
    assert_eq!(at(4, 1), "b.ordo:1:1");
    assert_eq!(at(5, 2), "b.ordo:2:2");
    assert_eq!(at(6, 1), "c.ordo:1:1");
}

/// The single-source form keeps its bare `line:col`, so an anonymous
/// source (a test, a playground buffer) does not grow a phantom filename.
#[test]
fn an_anonymous_source_reports_a_bare_line_and_column() {
    let e = ordo_cli::check("entity a {\n  n: nosuchtype\n}\n").unwrap_err();
    assert!(e.starts_with("3:1: parse error [parse_error]:"), "{e}");
    assert!(
        !e.contains(".ordo"),
        "an anonymous source grew a filename: {e}"
    );
}

/// 25e. A default the DDL cannot lower (`= now`) passed `ordo check` and
/// failed when the schema was applied. `check` now refuses what the DDL
/// refuses, so the program never gets as far as a deployment.
#[test]
fn a_default_the_ddl_cannot_lower_is_refused_by_check() {
    let refused = check_sources(&one(
        "entity notes {\n  title: text\n  created: datetime = now\n}\n",
    ))
    .expect_err("`= now` cannot be lowered to a column default");
    assert!(
        refused.contains("unsupported_default") && refused.contains("now"),
        "{refused}"
    );

    // A literal default is still fine.
    check_sources(&one(
        "entity notes {\n  title: text\n  stars: int = 0\n  state: text = \"open\"\n}\n",
    ))
    .expect("literal defaults lower");
}
