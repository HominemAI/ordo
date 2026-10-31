//! Reported fix 25p: `ordo check` exits 1 when an op does not compile.
//!
//! It used to print `DOES NOT COMPILE` and exit 0, so a script or CI step
//! that trusted the exit code passed a broken program. A regression reads
//! as exit 0 beside a `DOES NOT COMPILE` line, and fails here first.

mod support;

const MARKET: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../ordo-conformance/fixtures/market.ordo"
);

fn check(args: &[&str]) -> (i32, String) {
    let out = support::ordo()
        .arg("check")
        .args(args)
        .output()
        .expect("runs ordo");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The market fixture's `giftOrders` reads a sealed field with no index,
/// which waits on F0. Its blind-index ops, which need keys only at runtime,
/// are not counted.
#[test]
fn a_program_with_an_op_that_does_not_compile_exits_1() {
    let failed = ordo_cli::uncompiled_ops(
        &ordo_cli::Sources::read(std::path::Path::new(MARKET)).expect("reads"),
    )
    .expect("loads");
    assert_eq!(
        failed,
        vec![("giftOrders".to_string(), "sealed_unsupported".to_string())]
    );
    let (code, err) = check(&[MARKET]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("`giftOrders` does not compile"), "{err}");
}

#[test]
fn an_expected_failure_passes_and_a_stale_one_fails() {
    let (code, err) = check(&[MARKET, "--expect-fail", "giftOrders"]);
    assert_eq!(code, 0, "{err}");

    let (code, err) = check(&[
        MARKET,
        "--expect-fail",
        "giftOrders",
        "--expect-fail",
        "products",
    ]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("`products` was expected to fail and did not: it compiles"),
        "{err}"
    );

    // An allowance for a name that is not an op fails the same way: it is
    // stale too, and the message says both readings so a typo is not read
    // as a compile.
    let (code, err) = check(&[
        MARKET,
        "--expect-fail",
        "giftOrders",
        "--expect-fail",
        "noSuchOp",
    ]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("`noSuchOp` was expected to fail and did not")
            && err.contains("no op by that name is in this program"),
        "{err}"
    );
}

#[test]
fn a_program_that_compiles_exits_0() {
    let (code, err) = check(&[concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/notes/notes.ordo"
    )]);
    assert_eq!(code, 0, "{err}");
}

/// Bare `money` is `money(0)` and compiles; `ordo check` warns at each one
/// and still exits 0 (ruled 2026-10-05, `money-types` ask 1). Stated places,
/// `money(0)` among them, draw no warning.
#[test]
fn bare_money_warns_and_does_not_fail_the_check() {
    let dir = std::env::temp_dir().join(format!("ordo-bare-money-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("prices.ordo");
    std::fs::write(
        &file,
        "entity items {\n  name: text\n  price: money\n  cents: money(2)\n  yen: money(0)\n}\n\
         \nop cheap($cap: money) unbounded =\n  items(price < $cap)[name]\n",
    )
    .unwrap();
    let (code, err) = check(&[file.to_str().unwrap()]);
    let warnings: Vec<&str> = err.lines().filter(|l| l.starts_with("warning:")).collect();
    assert_eq!(code, 0, "{err}");
    assert_eq!(warnings.len(), 2, "{err}");
    assert!(warnings[0].contains("prices.ordo:3:10"), "{err}");
    assert!(warnings[1].contains("prices.ordo:8:16"), "{err}");
    assert!(warnings[0].contains("bare `money` is `money(0)`"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Board `guard-equality-uses-no-index`: `ordo check` warns when a guard's
/// walk compares no column an index leads with (each row checked scans the
/// walked table) and when a list sorts by a column no index leads with. It
/// still exits 0, and declared indexes silence both. A walk through an
/// ordinary ref never warns: every ref gets an index (reported fix 25y), so
/// a sealed ref, which gets none, is what leaves a walk unserved.
#[test]
fn a_guard_or_a_sort_no_index_serves_warns() {
    let dir = std::env::temp_dir().join(format!("ordo-index-warn-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let program = |seats: &str, docs: &str| {
        format!(
            "entity docs {{\n  title: text{docs}\n  \
             access read seats(person == $caller)\n  access write open\n}}\n\
             \nentity seats {{\n  doc -> docs {seats}\n  person: text\n}}\n\
             \nop myDocs unbounded =\n  docs[id, title]\n  sort title\n"
        )
    };
    let warn = |src: String| {
        let file = dir.join("docs.ordo");
        std::fs::write(&file, src).unwrap();
        let (code, err) = check(&[file.to_str().unwrap()]);
        assert_eq!(code, 0, "{err}");
        err.lines()
            .filter(|l| l.starts_with("warning:"))
            .map(str::to_string)
            .collect::<Vec<_>>()
    };

    let bare = warn(program("sealed", ""));
    assert_eq!(bare.len(), 2, "{bare:?}");
    assert!(
        bare[0].contains(
            "`myDocs` reads through a guard that compares `seats.doc` and `seats.person`"
        ),
        "{bare:?}"
    );
    assert!(bare[0].contains("Mark `seats.person` `index`"), "{bare:?}");
    assert!(
        bare[1].contains("`myDocs` sorts `docs` by `title`"),
        "{bare:?}"
    );

    // An ordinary ref is indexed, and a declared index serves the sort.
    let served = warn(program("", " index"));
    assert!(served.is_empty(), "{served:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Reported fix 25ap: a blind-index lookup that also returns the sealed field
/// compiles. Its class is `bounded-host`, because the payload is opened on
/// the host, and its compare still rides the digest, so the artifact
/// withholds the statement exactly as it does for a `blind-index` op.
///
/// It used to fail `ordo check`, `plan` and `apply` with
/// `unresolved_blind_index`: the artifact withheld the statement only when
/// the class was `blind-index`, planned this op, and refused the program.
#[test]
fn a_blind_index_lookup_that_returns_the_sealed_field_compiles() {
    let dir = std::env::temp_dir().join(format!("ordo-sealed-lookup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("customers.ordo");
    std::fs::write(
        &file,
        "entity customers {\n  name:  text\n  email: text sealed indexed\n}\n\
         \nop byEmail($e: text) = customers(email == $e)[name, email] take 1\n",
    )
    .unwrap();
    let sources = ordo_cli::Sources::read(&file).expect("reads");
    assert_eq!(
        ordo_cli::uncompiled_ops(&sources).expect("loads"),
        Vec::<(String, String)>::new()
    );

    let (schema, registry) = ordo_cli::load_sources(&sources).expect("loads");
    let doc = ordo_plan::artifact::program_artifact(&schema, &registry).expect("the artifact");
    let op = doc["ops"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["op"] == "byEmail")
        .expect("byEmail")
        .clone();
    assert_eq!(op["error"], serde_json::Value::Null, "{op}");
    assert_eq!(op["cost"]["class"], "bounded-host", "{op}");
    assert_eq!(op["plan"]["sql"], serde_json::Value::Null, "{op}");

    let (code, err) = check(&[file.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert!(!err.contains("does not compile"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}
