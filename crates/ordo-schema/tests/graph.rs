//! Unit tests for the schema graph and its DDL.
//!
//! These assert on the graph and the emitted DDL directly, without going
//! through the conformance harness — so a failure points at P2 rather than at
//! whichever corpus case happened to notice.

use ordo_schema::ddl::{convert_duration, unit_ms};
use ordo_schema::{build, ddl_for, FieldTy};
use ordo_syntax::parse_program;
use ordo_syntax::token::DurationUnit::*;

fn schema_of(src: &str) -> ordo_schema::Schema {
    build(&parse_program(src).expect("parses")).expect("builds")
}

fn ddl_of(src: &str) -> String {
    ddl_for(&schema_of(src)).expect("ddl")
}

fn err_code(src: &str) -> &'static str {
    match parse_program(src) {
        Ok(p) => build(&p).expect_err("expected a schema error").code,
        Err(e) => panic!("expected a schema error, got a parse error: {}", e.message),
    }
}

// ── implicit fields ──────────────────────────────────────────────────────

#[test]
fn every_entity_gets_id_and_rev() {
    let s = schema_of("entity a { name: text }");
    let a = s.entity("a").unwrap();
    assert!(a.field("id").unwrap().implicit);
    assert!(a.field("rev").unwrap().implicit);
    // `[*]` means what the author wrote, so implicit columns stay out of it.
    let declared: Vec<&str> = a.declared().map(|f| f.name.as_str()).collect();
    assert_eq!(declared, ["name"]);
}

#[test]
fn depth_appears_only_on_tree_entities() {
    let s = schema_of("entity a { parent -> a? tree }\nentity b { name: text }");
    assert!(s.entity("a").unwrap().is_tree);
    assert!(s.entity("a").unwrap().field("depth").is_some());
    assert!(!s.entity("b").unwrap().is_tree);
    assert!(s.entity("b").unwrap().field("depth").is_none());
}

#[test]
fn declaring_a_platform_field_is_an_error() {
    assert_eq!(err_code("entity a { id: text }"), "reserved_field_name");
    assert_eq!(err_code("entity a { rev: int }"), "reserved_field_name");
    assert_eq!(
        err_code("entity a { parent -> a? tree\n depth: int }"),
        "reserved_field_name"
    );
}

// ── ref resolution ───────────────────────────────────────────────────────

#[test]
fn refs_resolve_forward_and_backward() {
    // `a` points at `b`, declared later — resolution runs over the completed
    // name set, so declaration order is not a constraint on the author.
    let s = schema_of("entity a { b -> b }\nentity b { name: text }");
    assert!(matches!(
        &s.entity("a").unwrap().field("b").unwrap().ty,
        FieldTy::Ref { target, .. } if target == "b"
    ));
}

#[test]
fn parallel_edges_to_one_target_are_both_kept() {
    // Two refs at the same entity is legal and is what makes dot-paths
    // unambiguous while multi-source match inference is not.
    let s = schema_of("entity p { name: text }\nentity q { lead -> p\n second -> p? }");
    let q = s.entity("q").unwrap();
    for f in ["lead", "second"] {
        assert!(matches!(
            &q.field(f).unwrap().ty,
            FieldTy::Ref { target, .. } if target == "p"
        ));
    }
    assert!(q.field("second").unwrap().optional);
}

#[test]
fn structural_errors_have_stable_codes() {
    assert_eq!(err_code("entity a { b -> nope }"), "unknown_ref_target");
    assert_eq!(
        err_code("entity a { name: text }\nentity a { x: int }"),
        "duplicate_entity"
    );
    assert_eq!(err_code("entity a { x: text\n x: int }"), "duplicate_field");
    assert_eq!(
        err_code("entity a { b -> b tree }\nentity b { n: text }"),
        "tree_not_self_ref"
    );
    // TWO owned refs is a link entity (G7) and legal; THREE is nothing.
    assert_eq!(
        err_code("entity p { n: text }\nentity a { x -> p owned\n y -> p owned\n z -> p owned }"),
        "multiple_owners"
    );
    assert_eq!(
        err_code("entity a { x: text indexed }"),
        "indexed_without_sealed"
    );
    // The same reasoning one marker over: an OPEN field already accepts
    // refinement conditions, so `refinable` on one is decoration. The marker
    // exists to open a *sealed* field to them, which is a disclosure and a
    // cost (`std-library.md` §2.1 rule 1).
    assert_eq!(
        err_code("entity a { x: text refinable }"),
        "refinable_without_sealed"
    );
}

/// The `sealed indexed refinable` opt-in reaches the schema, and a sealed
/// field without it does not.
///
/// This is the fact the whole sealed-refinement gate is keyed on — the door
/// cannot tell an opted-in sealed field from any other one without it, and
/// before it was carried the parser read the marker and the schema dropped
/// it on the floor, which made `std-library.md` §2.1 rules 1–4 unenforceable
/// against any spec the platform produced.
#[test]
fn sealed_refinability_is_opt_in_and_survives_the_build() {
    let s = schema_of(
        "entity a {\n  open:  text\n  quiet: text sealed\n  found: text sealed indexed refinable\n}",
    );
    let f = |n: &str| {
        s.entity("a")
            .expect("entity a")
            .field(n)
            .unwrap_or_else(|| panic!("field {n}"))
    };
    assert!(f("open").refinable, "open fields are refinable");
    assert!(
        !f("quiet").refinable,
        "a sealed field is NOT refinable by default — the capability is authored"
    );
    assert!(f("found").refinable && f("found").indexed);
}

/// `$caller` in a shape default (G4 / R-G2, GC069). The class was in the
/// inventory with nothing that could raise it, and the probe found the
/// compiler *accepting* the exact declaration the spec forbids.
///
/// A default is evaluated with no session in scope — at seeds, backfills
/// and scheduled writes as much as at a member's create — so `$caller`
/// there either bakes one deployment's creating principal into the schema
/// or makes a static declaration mean different things to different
/// writers. Both are worse than the error.
#[test]
fn caller_may_not_appear_in_a_shape_default() {
    assert_eq!(
        err_code("entity m { n: text }\nentity drafts { author -> m = $caller\n body: text }"),
        "caller_in_default",
        "the GC069 declaration verbatim"
    );
    // On a scalar field too — nothing about this is specific to refs.
    assert_eq!(
        err_code("entity drafts { who: id = $caller }"),
        "caller_in_default"
    );
    // And through a function, which is the same mistake wearing a hat.
    assert_eq!(
        err_code(r#"entity drafts { who: id = coalesce($caller, "anon") }"#),
        "caller_in_default"
    );

    // Other parameters are not this error: a default naming `$tenant` is
    // unresolvable for a different reason, and the DDL layer says so with
    // `unsupported_default`. Only `$caller` is the authorization mistake
    // this class exists to name.
    let s = schema_of("entity drafts { who: id = $tenant }");
    assert!(ddl_for(&s).is_err(), "still not a lowerable default");

    // `$caller` on a write's right-hand side stays legal — that is where
    // "whoever is creating this row" is sayable (R-G2), and it is the fix
    // the error message names.
    schema_of("entity m { n: text }\nentity drafts { author -> m\n body: text }");
}

// ── DDL ──────────────────────────────────────────────────────────────────

#[test]
fn type_mapping_never_produces_a_float() {
    let d = ddl_of(
        "entity a { t: text\n i: int\n m: money\n b: bool\n d: date\n dt: datetime\n du: duration(s)\n x: id }",
    );
    for want in [
        r#""t" text"#,
        r#""i" bigint"#,
        r#""m" bigint"#,
        r#""b" boolean"#,
        r#""d" date"#,
        r#""dt" timestamptz"#,
        r#""du" bigint"#,
        r#""x" text"#,
    ] {
        assert!(d.contains(want), "missing {want} in:\n{d}");
    }
    let lower = d.to_lowercase();
    assert!(!lower.contains("double precision") && !lower.contains("real"));
}

#[test]
fn optionality_becomes_not_null() {
    let d = ddl_of("entity a { req: text\n opt: text? }");
    assert!(d.contains(r#""req" text NOT NULL"#), "{d}");
    assert!(d.contains(r#""opt" text,"#), "{d}");
}

#[test]
fn sealed_fields_are_bytea_and_index_to_a_separate_column() {
    let d = ddl_of(
        "entity a { e: text? sealed indexed unique normalize(lower)\n n: text = \"\" sealed }",
    );
    assert!(d.contains(r#""e" bytea"#), "{d}");
    assert!(d.contains(r#""e$bidx" bytea"#), "{d}");
    // Uniqueness is on the digest, and the SPACE leads it (9h): there is
    // no deployment-wide uniqueness and no way to ask for one.
    assert!(
        d.contains(r#"UNIQUE ("$space", "e$bidx")"#),
        "uniqueness is on the digest, per space:\n{d}"
    );
    // A sealed field's default is plaintext; it cannot become a column default.
    assert!(!d.contains(r#""n" bytea NOT NULL DEFAULT"#), "{d}");
}

/// `unique (a, b)` and `index (a, b)` — DL §3, declared rather than
/// derived (9e). Every consumer of `Entity::unique_keys` predates the
/// syntax: the `acl` derivation pinned it, so this is the surface arriving
/// on a pin that was already load-bearing end to end.
#[test]
fn declared_composite_unique_keys_and_indexes_reach_the_ddl() {
    let d = ddl_of(
        "entity properties { name: text }\n\
         entity locations {\n  \
           property -> properties owned\n  \
           name: text\n  \
           kind: text\n  \
           index (property, kind)\n\
         }\n\
         entity units {\n  \
           property -> properties owned\n  \
           name: text\n  \
           unique (property, name)\n\
         }\n",
    );
    assert!(
        d.contains(
            r#"ALTER TABLE "units" ADD CONSTRAINT "units$property$name$uq" UNIQUE NULLS NOT DISTINCT ("$space", "property", "name");"#
        ),
        "{d}"
    );
    assert!(
        d.contains(
            r#"CREATE INDEX "locations$property$kind$ix" ON "locations" ("$space", "property", "kind");"#
        ),
        "an index leads with the space too — every statement this compiler emits \
         filters on it, so one that did not would answer no query:\n{d}"
    );
    // The schema carries them as names, which is what `SetMode` reads to
    // decide converge-or-create.
    let s = schema_of("entity units { name: text\n code: text\n unique (name, code) }");
    assert_eq!(
        s.entity("units").unwrap().unique_keys,
        vec![vec!["name".to_string(), "code".to_string()]]
    );
}

/// `index` as a field modifier (reported fix 25w): the single-column form of
/// `index (a, b)`, lowered into the same list and emitted by the same DDL,
/// so the diff and the apply path need nothing new. It leads with the
/// space, as every index does.
#[test]
fn a_field_modifier_index_is_a_single_column_index() {
    let src = "entity people { name: text }\n\
               entity tickets {\n  \
                 status: text = \"open\" index\n  \
                 owner -> people index\n  \
                 due:    date? index\n  \
                 kind:   text\n  \
                 index (kind, due)\n\
               }\n";
    let s = schema_of(src);
    assert_eq!(
        s.entity("tickets").unwrap().indexes,
        vec![
            vec!["status".to_string()],
            vec!["owner".to_string()],
            vec!["due".to_string()],
            vec!["kind".to_string(), "due".to_string()],
        ]
    );
    let d = ddl_of(src);
    for stmt in [
        r#"CREATE INDEX "tickets$status$ix" ON "tickets" ("$space", "status");"#,
        r#"CREATE INDEX "tickets$owner$ix" ON "tickets" ("$space", "owner");"#,
        r#"CREATE INDEX "tickets$due$ix" ON "tickets" ("$space", "due");"#,
        r#"CREATE INDEX "tickets$kind$due$ix" ON "tickets" ("$space", "kind", "due");"#,
    ] {
        assert!(d.contains(stmt), "{stmt}\n{d}");
    }

    // A sealed field's index is the blind index, and each refusal names
    // the other spelling.
    let e = match parse_program("entity x { a: text sealed index }") {
        Ok(p) => build(&p).expect_err("a sealed field takes no plain index"),
        Err(e) => panic!("{}", e.message),
    };
    assert_eq!(e.code, "index_on_sealed");
    assert!(e.message.contains("`a: … sealed indexed`"), "{}", e.message);
    let e = match parse_program("entity x { a: text indexed }") {
        Ok(p) => build(&p).expect_err("an open field takes no blind index"),
        Err(e) => panic!("{}", e.message),
    };
    assert_eq!(e.code, "indexed_without_sealed");
    assert!(e.message.contains("`a: … index`"), "{}", e.message);
    assert_eq!(
        err_code("entity x { f: blob? index }"),
        "blob_not_indexable"
    );
}

/// DL §3: "Ref fields index automatically" (reported fix 25y). One
/// `(space, ref)` index per ref, named as the field modifier names it, and
/// none where another index already leads with the ref.
#[test]
fn ref_fields_index_automatically() {
    let src = "entity boards { name: text }\n\
               entity people { name: text }\n\
               entity tasks {\n  \
                 board -> boards\n  \
                 owner -> people index\n  \
                 reviewer -> people?\n  \
                 title: text\n\
               }\n\
               entity seats {\n  \
                 board -> boards\n  \
                 member -> people\n  \
                 unique (board, member)\n\
               }\n\
               entity notes {\n  \
                 board -> boards\n  \
                 at: date\n  \
                 index (board, at)\n\
               }\n";
    let s = schema_of(src);
    // The artifact keeps what the author declared.
    assert_eq!(
        s.entity("tasks").unwrap().indexes,
        vec![vec!["owner".to_string()]]
    );
    let d = ddl_of(src);
    for stmt in [
        r#"CREATE INDEX "tasks$board$ix" ON "tasks" ("$space", "board");"#,
        r#"CREATE INDEX "tasks$owner$ix" ON "tasks" ("$space", "owner");"#,
        r#"CREATE INDEX "tasks$reviewer$ix" ON "tasks" ("$space", "reviewer");"#,
        // `member` is second in the unique key, so it is not covered.
        r#"CREATE INDEX "seats$member$ix" ON "seats" ("$space", "member");"#,
        r#"CREATE INDEX "notes$board$at$ix" ON "notes" ("$space", "board", "at");"#,
    ] {
        assert_eq!(d.matches(stmt).count(), 1, "{stmt}\n{d}");
    }
    // Covered: the unique key and the declared index each lead with `board`.
    for absent in [r#""seats$board$ix""#, r#""notes$board$ix""#] {
        assert!(!d.contains(absent), "{absent}\n{d}");
    }
    // A table with no refs gets no index it did not declare.
    assert!(!d.contains(r#"ON "boards""#), "{d}");
}

/// The two refusals a declared `index` owes, mirroring the ones the
/// composite unique key already had.
#[test]
fn an_index_over_a_missing_or_sealed_field_is_refused() {
    let ddl_err = |src: &str| -> &'static str {
        ddl_for(&build(&parse_program(src).expect("parses")).expect("builds"))
            .expect_err("expected a ddl error")
            .code
    };
    assert_eq!(
        ddl_err("entity x { a: text\n b: text\n index (a, nope) }"),
        "unknown_index_field"
    );
    assert_eq!(
        ddl_err("entity x { a: text\n b: text sealed\n index (a, b) }"),
        "unsupported_index"
    );
    assert_eq!(
        ddl_err("entity x { a: text\n b: text sealed\n unique (a, b) }"),
        "unsupported_unique_key"
    );
}

#[test]
fn identifiers_are_quoted_so_case_survives() {
    let d = ddl_of("entity orderItems { lineTotal: money }");
    assert!(d.contains(r#"CREATE TABLE "orderItems""#), "{d}");
    assert!(d.contains(r#""lineTotal""#), "{d}");
}

#[test]
fn every_ref_gets_a_foreign_key() {
    let d = ddl_of("entity p { n: text }\nentity c { p -> p }");
    // COMPOSITE, and that is the point (9h): the referencing side carries
    // the child's own space, and the referenced side is the parent's
    // `(space, id)` primary key. A ref can therefore only ever name a row
    // in the ref holder's own space — the database refuses a cross-space
    // edge rather than nobody happening to write one.
    assert!(
        d.contains(
            r#"ALTER TABLE "c" ADD CONSTRAINT "c$p$fk" FOREIGN KEY ("$space", "p") REFERENCES "p" ("$space", "id")"#
        ),
        "{d}"
    );
}

/// The primary key is `(space, id)`, which is what a composite foreign key
/// can reference at all — and it is the reason a cross-space ref is a
/// constraint violation rather than a convention.
#[test]
fn the_primary_key_leads_with_the_space() {
    let d = ddl_of("entity p { n: text }");
    assert!(
        d.contains(r#"CONSTRAINT "p$pk" PRIMARY KEY ("$space", "id")"#),
        "{d}"
    );
    // And no deployment-wide id uniqueness: asserting it would turn a
    // mint collision in one tenant into a failed write in another.
    assert!(!d.contains(r#"UNIQUE ("id")"#), "{d}");
}

// ── duration units: the regression that started this file ────────────────

#[test]
fn duration_defaults_convert_to_the_declared_unit() {
    // `duration(ms) = 7d` is seven days, not seven milliseconds. Dropping the
    // literal's unit produced `DEFAULT 7` and a schema that looked correct.
    let d = ddl_of("entity a { cadence: duration(ms) = 7d\n shift: duration(h) = 8h }");
    assert!(
        d.contains(r#""cadence" bigint NOT NULL DEFAULT 604800000"#),
        "{d}"
    );
    assert!(d.contains(r#""shift" bigint NOT NULL DEFAULT 8"#), "{d}");
}

#[test]
fn inexact_duration_conversions_are_refused() {
    // 90 minutes has no exact value in hours, and rounding it silently would
    // be a data bug the DDL could not reveal.
    assert_eq!(
        err_code("entity a { d: duration(h) = 90m }"),
        "duration_unit_mismatch"
    );
    // A month is not a fixed number of milliseconds at all.
    assert_eq!(
        err_code("entity a { d: duration(ms) = 1mo }"),
        "duration_unit_mismatch"
    );
}

#[test]
fn duration_conversion_is_exact_or_absent() {
    assert_eq!(convert_duration(7, D, Ms), Some(604_800_000));
    assert_eq!(convert_duration(8, H, H), Some(8));
    assert_eq!(convert_duration(2, H, M), Some(120));
    assert_eq!(convert_duration(90, M, H), None, "not representable");
    assert_eq!(convert_duration(1, Mo, Ms), None, "months are not fixed");
    assert_eq!(unit_ms(Y), None);
}

#[test]
fn ref_edges_expose_the_multigraph() {
    // Parallel edges distinct; tree self-loop excluded; declaration order.
    let s = schema_of(
        "entity p { n: text }\nentity q { lead -> p\n second -> p?\n parent -> q? tree }",
    );
    let edges = s.ref_edges();
    let named: Vec<(&str, &str, &str)> = edges
        .iter()
        .map(|e| (e.from.as_str(), e.field.as_str(), e.to.as_str()))
        .collect();
    assert_eq!(
        named,
        [
            // The platform's own link entity: both refs point at `members`,
            // which is exactly why a reverse step into it must name the ref
            // it arrives by.
            ("groupMembers", "group", "members"),
            ("groupMembers", "member", "members"),
            ("q", "lead", "p"),
            ("q", "second", "p"),
        ]
    );
    // Found by name, not by index: this asserted `edges[1]` back when the
    // list held two entries, and a platform entity arriving at the front
    // silently repointed it at a different edge. An assertion that moves
    // when an unrelated entity is added was never about `second`.
    let second = edges
        .iter()
        .find(|e| e.field == "second")
        .expect("the optional edge");
    assert!(second.optional);
}

// ── typed-id keys ────────────────────────────────────────────────────────

#[test]
fn keys_derive_by_truncation_and_table() {
    use ordo_schema::derive_key;
    assert_eq!(derive_key("products"), "prod");
    assert_eq!(derive_key("vendors"), "vend");
    assert_eq!(derive_key("tags"), "tags", "short words survive whole");
    assert_eq!(
        derive_key("orderItems"),
        "orde-item",
        "camelCase hyphenates"
    );
    assert_eq!(
        derive_key("transactions"),
        "txns",
        "common words abbreviate"
    );
    assert_eq!(derive_key("accounts"), "acct");
}

#[test]
fn explicit_key_overrides_and_collisions_fail_naming_both() {
    let s = schema_of("entity projects (key: proj) { n: text }");
    assert_eq!(s.entity("projects").unwrap().key, "proj");

    // Two names that truncate identically must not compile — and the error
    // names both offenders and the fix.
    let e = match parse_program("entity orders { a: text }\nentity orderings { b: text }") {
        Ok(p) => ordo_schema::build(&p).expect_err("collision must fail"),
        Err(e) => panic!("parse: {}", e.message),
    };
    assert_eq!(e.code, "id_key_collision");
    assert!(
        e.message.contains("orders") && e.message.contains("orderings"),
        "{}",
        e.message
    );
    assert!(
        e.message.contains("key:"),
        "the fix is named: {}",
        e.message
    );

    // An explicit key colliding with a derived one fails identically.
    let e = match parse_program(
        "entity products { a: text }\nentity promotions (key: prod) { b: text }",
    ) {
        Ok(p) => ordo_schema::build(&p).expect_err("explicit collision must fail"),
        Err(e) => panic!("parse: {}", e.message),
    };
    assert_eq!(e.code, "id_key_collision");
}

#[test]
fn user_key_table_overrides_builtins() {
    use ordo_schema::{derive_key_with, KeyTable};
    let t =
        KeyTable::parse("# our house style\ntransactions = trx\nwidgets=wdg\n\ncustomers = c\n")
            .expect("parses");
    assert_eq!(
        derive_key_with("transactions", &t),
        "trx",
        "user beats builtin"
    );
    assert_eq!(derive_key_with("widgets", &t), "wdg");
    assert_eq!(derive_key_with("customers", &t), "c");
    assert_eq!(
        derive_key_with("invoices", &t),
        "inv",
        "builtins still apply"
    );

    // Malformed lines are loud, never dropped.
    assert!(KeyTable::parse("transactions trx").is_err());
    assert!(KeyTable::parse("a=b c").is_err());
}

#[test]
fn general_english_abbreviations_apply() {
    use ordo_schema::derive_key;
    assert_eq!(derive_key("appointments"), "appt");
    assert_eq!(derive_key("buildings"), "bldg");
    assert_eq!(derive_key("receipts"), "rcpt");
    // camelCase compounds pick up table words per segment.
    assert_eq!(derive_key("invoicePackages"), "invo-pkg");
    // The two deliberate absences: rev and std are reserved elsewhere.
    assert_eq!(derive_key("revisions"), "revi");
    assert_eq!(derive_key("standards"), "stan");
}

// ── Stage 3a: guards parse, resolve, validate ────────────────────────────

const GUARDED: &str = r#"
entity projects {
  name:  text
  owner -> members
  access read   owner == $caller or projectMembers(member == $caller)
  access write  owner == $caller
}

entity projectMembers {
  project -> projects  owned
  member  -> members   owned
  canWrite: bool = false
  access read   project.projectMembers(member == $caller)
  access write  project.owner == $caller
}

entity tasks {
  project  -> projects  owned
  assignee -> members?
  title:  text
  access read   project.projectMembers(member == $caller)
  access write  project.projectMembers(member == $caller and canWrite)
}

entity taskNotes {
  task -> tasks  owned
  body: text
}

entity boards {
  name: text
  acl (read, write, admin)
}

entity cards {
  board -> boards  owned
  title: text
}
"#;

#[test]
fn guards_parse_and_resolve_with_provenance() {
    use ordo_schema::{GuardBody, GuardSource};
    let s = schema_of(GUARDED);

    // Own clauses.
    let p = s.entity("projects").unwrap();
    let g = p.guards.as_ref().expect("projects is guarded");
    assert!(matches!(g.source, GuardSource::Own));
    assert!(matches!(g.read, GuardBody::Walk(_)));

    // Inheritance rebases through the ownership path (G6): taskNotes reads
    // under task.<tasks' walk>.
    let tn = s.entity("taskNotes").unwrap();
    let g = tn.guards.as_ref().expect("taskNotes inherits");
    assert!(matches!(&g.source, GuardSource::Inherited { from } if from == "tasks"));
    assert_eq!(g.rebase, ["task"]);

    // acl derivation (GC036's shape): the link entity exists with its refs
    // and level, and both root and link carry derived guards.
    let link = s.entity("boardsAccess").expect("acl mints boardsAccess");
    assert!(matches!(
        link.field("board").unwrap().ty,
        ordo_schema::FieldTy::Ref { owned: true, .. }
    ));
    assert!(link.field("member").is_some() && link.field("level").is_some());
    assert!(matches!(
        s.entity("boards").unwrap().guards.as_ref().unwrap().source,
        GuardSource::AclDerived
    ));

    // cards inherits the boards ACL down the owned subtree.
    let c = s.entity("cards").unwrap().guards.as_ref().unwrap();
    assert!(matches!(&c.source, GuardSource::Inherited { from } if from == "boards"));
    assert_eq!(c.rebase, ["board"]);

    // A link entity with its own clauses keeps them (G7 is about
    // inheritance, not declaration).
    assert!(matches!(
        s.entity("projectMembers")
            .unwrap()
            .guards
            .as_ref()
            .unwrap()
            .source,
        GuardSource::Own
    ));
}

#[test]
fn guard_rejects_carry_their_corpus_classes() {
    // G9: one verb without the other.
    assert_eq!(
        err_code("entity x { n: text\n access read n == $caller }"),
        "incomplete_access_clause"
    );
    // G2: no $caller anywhere.
    assert_eq!(
        err_code("entity x { done: bool\n access read done\n access write done }"),
        "invalid_guard_walk"
    );
    // G3: sealed in the walk.
    assert_eq!(
        err_code("entity x { secret: text = \"\" sealed\n owner -> members\n access read secret == $caller\n access write owner == $caller }"),
        "guard_on_sealed"
    );
    // Unreachable walk.
    assert_eq!(
        err_code("entity x { owner -> members\n access read nothing(member == $caller)\n access write owner == $caller }"),
        "invalid_guard_walk"
    );
    // members is platform-provided.
    assert_eq!(err_code("entity members { n: text }"), "parse_error");
    // reserved derived name.
    assert_eq!(
        err_code("entity boards { n: text\n acl (read, admin) }\nentity boardsAccess { n: text }"),
        "reserved_derived_name"
    );
}

#[test]
fn open_is_a_clause_and_true_is_not() {
    use ordo_schema::GuardBody;
    let s = schema_of(
        "entity ann { body: text\n owner -> members\n access read owner == $caller\n access write open }",
    );
    let g = s.entity("ann").unwrap().guards.as_ref().unwrap();
    assert!(matches!(g.write, GuardBody::Open));
    // GC100: the withdrawn boolean spelling fails at parse.
    assert!(parse_program("entity x { n: text\n access write true }").is_err());
    // GC079: the verb set is closed.
    assert!(parse_program("entity x { n: text\n access admin n == $caller }").is_err());
}

// ── acl: the level ordering, the constraints, the derived ops ────────────

/// R-G14's thresholds are **positions** in the declared ladder, so the
/// derivation has to behave on ladders the fixture does not use.
#[test]
fn acl_level_thresholds_are_positional_and_clamp() {
    // Three levels: read at the bottom (no level test — every declared
    // level qualifies, and the enum is what makes that a tautology rather
    // than a gap), write at the SECOND rung, admin at the top.
    let s = schema_of("entity boards { name: text\n acl (read, write, admin) }");
    let root = s.entity("boards").unwrap().guards.as_ref().unwrap();
    assert_eq!(root.render(true), "boardsAccess(member == $caller) or boardsAccess(member.groupMembers.group(member == $caller))");
    assert_eq!(
        root.render(false),
        "boardsAccess(member == $caller and level == \"write\" \
         or member == $caller and level == \"admin\") or boardsAccess(member.groupMembers.group(member == $caller) and level == \"write\" \
         or member.groupMembers.group(member == $caller) and level == \"admin\")"
    );
    let link = s.entity("boardsAccess").unwrap().guards.as_ref().unwrap();
    assert_eq!(link.render(true), "board.boardsAccess(member == $caller) or board.boardsAccess(member.groupMembers.group(member == $caller))");
    assert_eq!(
        link.render(false),
        "board.boardsAccess(member == $caller and level == \"admin\") or board.boardsAccess(member.groupMembers.group(member == $caller) and level == \"admin\")",
        "the link's own write guard is the meta-permission: top level only"
    );

    // Two levels: the second rung IS the top, so `write` both writes and
    // administers — which is what those two names mean.
    let s = schema_of("entity lists { name: text\n acl (read, write) }");
    assert_eq!(
        s.entity("lists")
            .unwrap()
            .guards
            .as_ref()
            .unwrap()
            .render(false),
        "listsAccess(member == $caller and level == \"write\") or listsAccess(member.groupMembers.group(member == $caller) and level == \"write\")"
    );
    assert_eq!(
        s.entity("listsAccess")
            .unwrap()
            .guards
            .as_ref()
            .unwrap()
            .render(false),
        "list.listsAccess(member == $caller and level == \"write\") or list.listsAccess(member.groupMembers.group(member == $caller) and level == \"write\")"
    );

    // Four levels: the rule is applied, never inferred from the names —
    // the second rung is the write threshold, so `comment` writes. An
    // author who wants a rung that reads and does not write declares it
    // first.
    let s = schema_of("entity wikis { name: text\n acl (read, comment, write, admin) }");
    assert_eq!(
        s.entity("wikis")
            .unwrap()
            .guards
            .as_ref()
            .unwrap()
            .render(false),
        "wikisAccess(member == $caller and level == \"comment\" \
         or member == $caller and level == \"write\" \
         or member == $caller and level == \"admin\") or wikisAccess(member.groupMembers.group(member == $caller) and level == \"comment\" \
         or member.groupMembers.group(member == $caller) and level == \"write\" \
         or member.groupMembers.group(member == $caller) and level == \"admin\")"
    );
}

#[test]
fn acl_derives_the_link_constraints_and_the_admin_ops() {
    let src = "entity careTeams { name: text\n acl (read, write, admin) }";
    let s = schema_of(src);
    let link = s.entity("careTeamsAccess").expect("derived");

    // `level: enum(<the ladder>)` and `unique (<root>, member)`, the
    // back-ref carrying its mechanical name (trailing `s` dropped).
    assert_eq!(
        link.field("level").unwrap().enum_values.as_deref(),
        Some(["read".into(), "write".into(), "admin".into()].as_slice())
    );
    assert_eq!(
        link.unique_keys,
        vec![vec!["careTeam".to_string(), "member".to_string()]]
    );

    // Both reach the DDL, and the unique index leads with the space.
    let ddl = ddl_of(src);
    assert!(
        ddl.contains(
            r#"CONSTRAINT "careTeamsAccess$level$enum" CHECK ("level" IN ('read', 'write', 'admin'))"#
        ),
        "{ddl}"
    );
    assert!(
        ddl.contains(
            r#"ALTER TABLE "careTeamsAccess" ADD CONSTRAINT "careTeamsAccess$careTeam$member$uq" UNIQUE NULLS NOT DISTINCT ("$space", "careTeam", "member");"#
        ),
        "{ddl}"
    );

    // The three admin ops, named by rule rather than by example.
    let ops: Vec<String> = ordo_schema::acl_admin_ops(&s)
        .ops
        .iter()
        .map(|o| o.name.name.clone())
        .collect();
    assert_eq!(
        ops,
        [
            "grantCareTeamsAccess",
            "revokeCareTeamsAccess",
            "setCareTeamsAccessLevel"
        ]
    );

    // And their names are reserved: a domain cannot declare one, or the
    // name a caller was granted is not the op they think it is.
    assert_eq!(
        err_code(
            "entity careTeams { name: text\n acl (read, write, admin) }\n\
             op grantCareTeamsAccess($x: id) = set careTeams { name: \"x\" }"
        ),
        "reserved_derived_name"
    );
}
