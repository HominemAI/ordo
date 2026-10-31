//! `plan-schema-evolution.md` §1.1(a), as identity.
//!
//! # The failure this is about
//!
//! `registry::store` decides artifact identity on the version hash, and the
//! version hash was the digest of the artifact document alone. The document
//! emitted `name`, `type`, `optional`, `unique`, `sealed` and `indexed` per
//! field and `key`, `tree`, `acl_levels` and guards per entity — and **no
//! field default, no `normalize`, no `enum_values`, no composite
//! `unique_keys` and no declared `indexes`**.
//!
//! Each of those five changes the generated DDL or the field's meaning and
//! changed nothing in the document. So a program that added
//! `index (account, date)` produced byte-identical artifact bytes, the same
//! program version, `Applied::AlreadyStored`, `Activated::AlreadyActive` —
//! and the new DDL, which *did* differ, was discarded. `ordo apply` printed
//! "already present, byte-identical" and "already active; nothing
//! re-pointed", and the operator's change was gone.
//!
//! # What is asserted, and in which order
//!
//! Each case below is **two programs differing only in one schema
//! declaration**, and each is asserted four ways:
//!
//! 1. whether the change reaches the generated DDL at all — `normalize` does
//!    not, which is why the document was its only possible home;
//! 2. the version moves — the fix;
//! 3. the *document* differs — so the change is visible in a deploy diff and
//!    not only inside a hash;
//! 4. and, with the five new keys stripped back out, the two programs
//!    declare **byte-identical entities** — the old failure, reconstructed,
//!    so the test proves the bug was real rather than only that the fix is
//!    present.
//!
//! Assertion 4 is the one that would survive somebody deleting the fix: it
//! fails loudly if the five keys ever stop being what separates these
//! programs.
//!
//! # One correction to the plan, found by writing this
//!
//! §1.1(a) names `unique (a, b)` as the worse example — *"the operator now
//! believes a uniqueness constraint is deployed and it is not"*. The
//! consequence is right and the premise is not: a composite unique key
//! derives `std.<entity>.put`, so the document already moved on that change.
//! See [`a_composite_unique_moves_the_version`], which asserts the
//! correction. The genuinely silent cases were `index`, a changed default,
//! and `normalize`.

use ordo_cli::{load_sources, Sources};
use ordo_plan::artifact::{program_artifact, program_version, to_bytes};
use serde_json::Value;

/// The five keys defect (a) found missing, and the only ones any case here
/// is allowed to differ by.
const THE_FIVE: &[&str] = &[
    "default",
    "normalize",
    "enum_values",
    "unique_keys",
    "indexes",
];

const BASE: &str = r#"
entity a {
  name:   text
  n:      int
  email:  text?  sealed indexed
  active: bool = true
}

op listA unbounded =
  a[name, n]
"#;

fn one(text: &str) -> Sources {
    Sources::from_parts(vec![("p.ordo".into(), text.to_string())])
}

/// Compile, and return the three things a deploy is keyed on.
fn deployable(text: &str) -> (Value, String, String) {
    let (schema, registry) = load_sources(&one(text)).expect("compiles");
    let doc = program_artifact(&schema, &registry).expect("artifact");
    let ddl = ordo_schema::ddl_for(&schema).expect("ddl");
    let version = program_version(&doc, &ddl);
    (doc, ddl, version)
}

/// The document as it would have been before the fix: the five keys removed
/// wherever they appear.
fn without_the_five(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(k, _)| !THE_FIVE.contains(&k.as_str()))
                .map(|(k, x)| (k.clone(), without_the_five(x)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(without_the_five).collect()),
        other => other.clone(),
    }
}

/// Does the change reach `ddl_for`'s output?
#[derive(PartialEq)]
enum Ddl {
    Moves,
    /// `normalize` is the one that does not: it changes the plaintext that
    /// is HMACed into a blind index and emits no SQL at all.
    Unchanged,
}

/// One declaration changed, four assertions.
fn only_the_declaration_moved(what: &str, changed: &str, ddl_expect: Ddl) {
    let (base_doc, base_ddl, base_v) = deployable(BASE);
    let (doc, ddl, v) = deployable(changed);

    match ddl_expect {
        Ddl::Moves => assert_ne!(base_ddl, ddl, "{what}: the generated DDL should differ"),
        Ddl::Unchanged => assert_eq!(
            base_ddl, ddl,
            "{what}: this declaration emits no SQL, so the DOCUMENT is the only \
             place a deploy can ever find it — which is why leaving it out was \
             not a cosmetic omission"
        ),
    }
    assert_ne!(
        base_v, v,
        "{what}: the program version did not move, so `store` answers \
         AlreadyStored, `activate` answers AlreadyActive, and the change is \
         discarded — §1.1(a) exactly"
    );
    assert_ne!(
        to_bytes(&base_doc),
        to_bytes(&doc),
        "{what}: the version moved but the document did not, so a deploy diff \
         shows a reviewer nothing but a changed hash"
    );
    // The **entities** half, which is the half defect (a) is about. Compared
    // separately from `ops` because some declarations legitimately move an
    // op too — see `a_composite_unique_moves_the_version`, where the finding
    // is that the plan's own example was not the silent case it named.
    assert_eq!(
        to_bytes(&without_the_five(&base_doc["entities"])),
        to_bytes(&without_the_five(&doc["entities"])),
        "{what}: with the five declarations removed these two programs declare \
         the SAME entities — which is what the artifact used to emit, and is \
         why this deploy used to be a silent no-op. If this fails, the case is \
         no longer isolating one declaration and the assertions above prove \
         less than they claim."
    );
}

// ── the three the task names, and the two beside them ────────────────────

/// `unique (a, b)` — and a correction to the plan, found here.
///
/// §1.1(a) names this one as *"worse"* than `index (a, b)` because *"the
/// operator now believes a uniqueness constraint is deployed and it is
/// not."* The consequence is right; the premise is not. A composite unique
/// key is what `writes::set_mode` reads to decide that an unconditioned
/// `set` **addresses** a row, so declaring one derives `std.<entity>.put`,
/// which the entity did not have before. The document therefore already
/// moved on this change, and it already moved on a single-column `unique`
/// too, because `Field::unique` was always emitted.
///
/// So the version moves here for two reasons now and moved for one before,
/// and the silent cases were the other three: a declared `index`, which
/// constrains nothing and derives nothing; a changed default; and
/// `normalize`. The fix is right and one of its stated examples was not.
#[test]
fn a_composite_unique_moves_the_version() {
    const WITH: &str = r#"
entity a {
  name:   text
  n:      int
  email:  text?  sealed indexed
  active: bool = true
  unique (name, n)
}

op listA unbounded =
  a[name, n]
"#;
    only_the_declaration_moved("unique (name, n)", WITH, Ddl::Moves);

    // The correction, asserted rather than asserted-about: the op list is
    // not the same on the two sides, so this was never the byte-identical
    // case the plan called it.
    let names = |d: &Value| -> Vec<String> {
        d["ops"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["op"].as_str().unwrap().to_string())
            .collect()
    };
    let (base_doc, ..) = deployable(BASE);
    let (with_doc, ..) = deployable(WITH);
    assert!(!names(&base_doc).contains(&"std.a.put".to_string()));
    assert!(
        names(&with_doc).contains(&"std.a.put".to_string()),
        "a composite unique is what makes an unconditioned `set` address a \
         row, so it derives an op — which is why the document was never \
         byte-identical across this change"
    );
}

/// A changed default. Not cosmetic: the default decides what a create
/// without the field writes, so it is a contract fact and not only a DDL
/// rendering.
#[test]
fn a_changed_default_moves_the_version() {
    only_the_declaration_moved(
        "active: bool = false",
        r#"
entity a {
  name:   text
  n:      int
  email:  text?  sealed indexed
  active: bool = false
}

op listA unbounded =
  a[name, n]
"#,
        Ddl::Moves,
    );
}

/// The plan's own worked example: `index (account, date)`.
#[test]
fn a_declared_index_moves_the_version() {
    only_the_declaration_moved(
        "index (name, n)",
        r#"
entity a {
  name:   text
  n:      int
  email:  text?  sealed indexed
  active: bool = true
  index (name, n)
}

op listA unbounded =
  a[name, n]
"#,
        Ddl::Moves,
    );
}

/// `normalize(lower)` changes no column and no constraint — it changes the
/// plaintext that is HMACed into a blind index. So it is invisible to
/// `information_schema`, and the *only* place a diff can find it is the
/// document. It had to be there.
#[test]
fn a_normalization_moves_the_version() {
    only_the_declaration_moved(
        "normalize(lower)",
        r#"
entity a {
  name:   text
  n:      int
  email:  text?  sealed indexed normalize(lower)
  active: bool = true
}

op listA unbounded =
  a[name, n]
"#,
        Ddl::Unchanged,
    );
}

/// An added `enum` value, reached at the `Schema` grain because that is
/// where it lives.
///
/// `enum_values` has no declaration form in DL yet — it is the pin the `acl`
/// derivation puts on `level` — so the two programs cannot differ in source
/// by it alone. They can differ in `Schema` by it alone, which is the same
/// claim one layer down and isolates it exactly: everything else about the
/// two compilations is one text.
#[test]
fn an_added_enum_value_moves_the_version() {
    let build = |values: &[&str]| {
        let (mut schema, registry) = load_sources(&one(BASE)).expect("compiles");
        let e = schema
            .entities
            .iter_mut()
            .find(|e| e.name == "a")
            .expect("entity a");
        let f = e
            .fields
            .iter_mut()
            .find(|f| f.name == "name")
            .expect("field name");
        f.enum_values = Some(values.iter().map(|v| v.to_string()).collect());
        let doc = program_artifact(&schema, &registry).expect("artifact");
        let ddl = ordo_schema::ddl_for(&schema).expect("ddl");
        let version = program_version(&doc, &ddl);
        (doc, ddl, version)
    };
    let (a_doc, a_ddl, a_v) = build(&["read", "write"]);
    let (b_doc, b_ddl, b_v) = build(&["read", "write", "admin"]);

    assert_ne!(a_ddl, b_ddl, "the generated CHECK names the set");
    assert_ne!(
        a_v, b_v,
        "a value set is a refusal class a caller can hit; a deploy that adds \
         one must not be a no-op"
    );
    assert_ne!(to_bytes(&a_doc), to_bytes(&b_doc));
    assert_eq!(
        to_bytes(&without_the_five(&a_doc)),
        to_bytes(&without_the_five(&b_doc)),
        "with `enum_values` removed these are one document — the old failure"
    );
}

// ── the general form, which outlives the five ────────────────────────────

/// The document is not the whole of a deployable state, and the version says
/// so.
///
/// The five keys close the *specific* bug. This closes the general one: any
/// future declaration that reaches `ddl_for` and not the document moves the
/// version anyway, so the next omission is a diff that reads oddly rather
/// than a deploy that silently does nothing.
#[test]
fn the_version_covers_the_ddl_even_when_the_document_cannot() {
    let (doc, ddl, v) = deployable(BASE);
    let tampered = format!("{ddl}\nCREATE INDEX \"a$hand$ix\" ON a (\"n\");\n");
    assert_ne!(
        v,
        program_version(&doc, &tampered),
        "one version must mean one deployable state, and the DDL is half of it"
    );
}

/// The separator is a NUL byte, which occurs in neither input, so no
/// document-plus-DDL pair can be re-cut into a different one.
#[test]
fn the_document_and_the_ddl_cannot_be_re_cut_into_each_other() {
    let doc = serde_json::json!({ "x": "1" });
    let a = program_version(&doc, "SELECT 2;");
    // A document whose bytes end where the other's DDL begins would collide
    // under naive concatenation.
    let doc2 = serde_json::json!({ "x": "1SELECT" });
    let b = program_version(&doc2, " 2;");
    assert_ne!(a, b, "the two inputs are not separably framed");
}

/// The consequence for grants, stated so it is a decision rather than a
/// discovery: **none of the five moves an op's version.**
///
/// An op's version is the digest of its own artifact object — kind, params,
/// outputs, scope, guards, plan — and an entity's declarations are not in
/// it. So a deploy that adds `unique (a, b)` moves the program version and
/// kills no capability grant, which is right: nothing an op discloses or
/// touches has changed. The report's GRANTS section counts zero, and it is
/// counting correctly.
#[test]
fn the_five_move_the_program_version_and_no_op_version() {
    let versions = |doc: &Value| -> Vec<(String, String)> {
        doc["ops"]
            .as_array()
            .expect("ops")
            .iter()
            .map(|o| {
                (
                    o["op"].as_str().unwrap().to_string(),
                    o["version"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };
    let (base_doc, ..) = deployable(BASE);
    let (with_index, ..) = deployable(
        r#"
entity a {
  name:   text
  n:      int
  email:  text?  sealed indexed
  active: bool = true
  index (name, n)
}

op listA unbounded =
  a[name, n]
"#,
    );
    assert_eq!(
        versions(&base_doc),
        versions(&with_index),
        "an index constrains nothing and discloses nothing, so no op's contract \
         moved and no grant should die"
    );
}

// ── the registry, which is where the silence actually happened ───────────

/// The end of the story, against a real registry: a schema-only change is
/// **stored**, not swallowed.
///
/// The three assertions above are about hashes. This one is about the
/// sentence an operator reads. `registry::store` answers `AlreadyStored` on
/// a version it already holds, `ordo apply` prints *"already present,
/// byte-identical"*, and the differing DDL never reaches the column it is
/// stored in — so the deploy is a no-op with a success message on it. The
/// `index (name, n)` case is used because it is the one that was genuinely
/// silent.
#[tokio::test(flavor = "multi_thread")]
async fn a_schema_only_change_is_stored_rather_than_swallowed() {
    use ordo_commandlog::Timestamp;
    use ordo_exec::registry::{self, Applied, Artifact};
    use ordo_exec::{Pool, PoolConfig};

    const WITH_INDEX: &str = r#"
entity a {
  name:   text
  n:      int
  email:  text?  sealed indexed
  active: bool = true
  index (name, n)
}

op listA unbounded =
  a[name, n]
"#;

    let pg = ordo_conformance::Pg::named("ordo_mig_identity");
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("reset");
    let pool = Pool::connect(PoolConfig {
        db: "ordo_mig_identity".into(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool boots");
    registry::ensure_tables(&pool).await.expect("registry");

    let row = |src: &str| {
        let (doc, ddl, version) = deployable(src);
        Artifact {
            version,
            artifact: to_bytes(&doc),
            source: src.to_string(),
            ddl,
            applied_at: Timestamp(1),
            actor: "test".into(),
        }
    };
    let before = row(BASE);
    let after = row(WITH_INDEX);
    assert_ne!(before.ddl, after.ddl, "the DDL is what changed");

    assert!(
        matches!(
            registry::store(&pool, &before).await,
            Ok(Applied::Stored(_))
        ),
        "the first apply stores"
    );
    assert!(
        matches!(
            registry::store(&pool, &before).await,
            Ok(Applied::AlreadyStored(_))
        ),
        "re-applying the SAME program is idempotent, which is the behaviour \
         the defect hid behind"
    );
    match registry::store(&pool, &after).await {
        Ok(Applied::Stored(v)) => assert_eq!(v, after.version),
        other => panic!(
            "a program that adds `index (name, n)` must land as a new version. \
             Before the fix this answered AlreadyStored, `activate` answered \
             AlreadyActive, and the index was never created. Got: {other:?}"
        ),
    }

    // And the DDL that reached the column is the new one, which is the fact
    // the whole registry exists to hold: `verify_candidate` builds its
    // shadow from this text.
    let stored = registry::get(&pool, &after.version)
        .await
        .expect("read")
        .expect("present");
    assert_eq!(stored.ddl, after.ddl);
    assert!(stored.ddl.contains("a$name$n$ix"));
}

// ── routines ─────────────────────────────────────────────────────────────

const WITH_ROUTINE: &str = r#"
domain shop @ 1.0.0

entity orders {
  placed: datetime
  status: text = "new"
}

op placeOrder($placed: datetime) =
  set orders { placed: $placed }

routine sweep in shop {
  trigger every 15m
  run($now: datetime):
    set orders(status == "new" and placed < $now - 7d) { status: "expired" }
}
"#;

/// A routine is part of the deployed program: the server rebuilds its
/// trigger wiring from it, and a run is attributed `routine:<name>@<version>`.
/// So an edit that touches only a routine must be a new version.
#[test]
fn a_change_to_only_a_routine_changes_the_programs_version() {
    let (_, _, base) = deployable(WITH_ROUTINE);
    let (_, _, again) = deployable(WITH_ROUTINE);
    assert_eq!(base, again, "the same text is the same version");

    let variants = [
        (
            "its schedule",
            WITH_ROUTINE.replace("every 15m", "every 30m"),
        ),
        (
            "its body",
            WITH_ROUTINE.replace("{ status: \"expired\" }", "{ status: \"stale\" }"),
        ),
        (
            "its trigger kind",
            WITH_ROUTINE.replace("every 15m", "when shop.orders.created"),
        ),
    ];
    for (what, text) in variants {
        let (_, _, v) = deployable(&text);
        assert_ne!(v, base, "changing only {what} must change the version");
    }

    // Layout and comments are not the program.
    let spaced = WITH_ROUTINE.replace("  trigger every 15m", "  # nightly\n  trigger   every 15m");
    let (_, _, v) = deployable(&spaced);
    assert_eq!(v, base, "a comment and spacing leave the version alone");
}

// ── secrets ──────────────────────────────────────────────────────────────

/// A secret's binding is part of the program: moving a host decides where
/// the egress kernel will send the key, so it must be a new version. A
/// program with no secrets keeps the version it had before secrets existed.
#[test]
fn a_secret_binding_is_part_of_the_programs_version() {
    let (_, _, base) = deployable(BASE);
    let with = format!("secret openaiKey for api.openai.com\n{BASE}");
    let (doc, _, bound) = deployable(&with);
    assert_ne!(bound, base, "declaring a secret is a new version");
    assert_eq!(
        doc["secrets"],
        serde_json::json!([{ "name": "openaiKey", "hosts": ["api.openai.com"] }])
    );
    let moved = with.replace("api.openai.com", "api.openai.com, eu.openai.com");
    let (_, _, v) = deployable(&moved);
    assert_ne!(v, bound, "adding a host is a new version");
    let (doc, _, _) = deployable(BASE);
    assert!(doc.get("secrets").is_none(), "no secrets, no key");

    let twice = format!("secret k for a.example.com\nsecret k for b.example.com\n{BASE}");
    let err = load_sources(&one(&twice)).err().expect("refused");
    assert!(format!("{err:?}").contains("duplicate_secret"), "{err:?}");
}
