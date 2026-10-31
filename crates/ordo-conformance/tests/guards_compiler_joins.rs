//! **A row the compiler brings into a query is guarded like a row the
//! author listed.**
//!
//! Two IR nodes are added by the compiler and not by the developer: the
//! target of a dot-hop (`Join`) and the bridge between two listed sources
//! (`SourceJoin`). Until 2026-09-28 neither carried a guard, so the guard of
//! the row a query started from was the only one evaluated:
//!
//! - **the dot-hop** is GC007, recorded as a divergence and never falsified,
//!   because no dot-path in `market.ordo` reaches an entity whose read guard
//!   is independent of its referrer's (`corpus.md` has the table);
//! - **the bridge** was recorded nowhere. The 2026-09-28 security review
//!   found it by asking whether anything else had GC007's shape, and
//!   reproduced it against the running binary (`security-review-6.md`
//!   S6-001).
//!
//! This file carries **its own program**, for the reason GC007 stood so
//! long: the shared fixture cannot hold the falsifying case, and adding an
//! entity to it moves the schema, the DDL and the seed under every other
//! suite. Both targets here have a guard that is independent of the row
//! that reaches them, which is the one property the fixture lacks.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_conformance::{Fixture, Pg};
use serde_json::{json, Value};

/// `dossiers` and `assignments` are readable by their **owner only**.
/// `cases`, `people` and `places` are open. So a caller who is not the owner
/// can read every row they start from and none of the rows the compiler
/// adds on their behalf.
const PROGRAM: &str = "\
entity people (key: pers) {
  name: text
  access read   open
  access write  open
}

entity places (key: plc) {
  label: text
  access read   open
  access write  open
}

entity assignments (key: asg) {
  person -> people
  place  -> places
  owner  -> members
  access read   owner == $caller
  access write  owner == $caller
}

entity dossiers (key: dos) {
  owner  -> members
  secret: text
  access read   owner == $caller
  access write  owner == $caller
}

entity cases (key: cas) {
  dossier -> dossiers
  title: text
  access read   open
  access write  open
}
";

const OWNER: &str = "mem_owner";
const OTHER: &str = "mem_other";

fn seeded(db: &str) -> (Fixture, Pg) {
    let f = Fixture::from_source(PROGRAM, "").expect("the program compiles");
    let pg = Pg::scoped(db);
    f.reset(&pg).expect("the schema applies");
    let space = ordo_conformance::FIXTURE_SPACE;
    let member = |id: &str| {
        format!(
            "INSERT INTO \"members\" (\"$space\", \"id\", \"rev\", \"principal\", \
             \"displayName\", \"kind\", \"memberCount\", \"role\", \"status\", \"joined\") \
             VALUES ('{space}', '{id}', 1, '{id}', '{id}', 'principal', 1, 'editor', \
             'active', CURRENT_DATE)"
        )
    };
    for sql in [
        member(OWNER),
        member(OTHER),
        format!("INSERT INTO \"people\" (\"$space\",\"id\",\"rev\",\"name\") VALUES ('{space}','pers_1',1,'Alice')"),
        format!("INSERT INTO \"places\" (\"$space\",\"id\",\"rev\",\"label\") VALUES ('{space}','plc_1',1,'Vault')"),
        format!("INSERT INTO \"assignments\" (\"$space\",\"id\",\"rev\",\"person\",\"place\",\"owner\") VALUES ('{space}','asg_1',1,'pers_1','plc_1','{OWNER}')"),
        format!("INSERT INTO \"dossiers\" (\"$space\",\"id\",\"rev\",\"owner\",\"secret\") VALUES ('{space}','dos_1',1,'{OWNER}','the secret')"),
        format!("INSERT INTO \"cases\" (\"$space\",\"id\",\"rev\",\"dossier\",\"title\") VALUES ('{space}','cas_1',1,'dos_1','Case one')"),
    ] {
        pg.exec(&sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    (f, pg)
}

fn rows(v: Value) -> Vec<Value> {
    v.as_array().cloned().expect("an array of rows")
}

/// **The dot-hop (GC007).** The caller may read the case and may not read
/// the dossier it points at. The row stays and the hop is null, exactly as
/// it would be if the ref were null: a target you may not read is
/// indistinguishable from a target that is not there.
#[test]
fn a_dot_hop_does_not_read_a_target_the_caller_may_not() {
    let (f, pg) = seeded("gcj_dothop");
    let src = "cases[title, dossier.secret]";

    let owner = rows(
        f.run_case_as(&pg, src, &[], Some(OWNER))
            .expect("owner reads"),
    );
    assert_eq!(
        owner,
        vec![json!({"title": "Case one", "secret": "the secret"})]
    );

    let other = rows(
        f.run_case_as(&pg, src, &[], Some(OTHER))
            .expect("other reads"),
    );
    assert_eq!(
        other,
        vec![json!({"title": "Case one", "secret": null})],
        "the row the caller may read stays, and the hop through a dossier they \
         may not read is null"
    );
}

/// A filter through the hop is the same read as a projection through it, so
/// it must not become an oracle: asking "is the secret X?" of a dossier the
/// caller cannot read answers as it would of a dossier that is not there.
#[test]
fn a_filter_through_a_dot_hop_is_not_an_oracle() {
    let (f, pg) = seeded("gcj_oracle");
    let src = "cases(dossier.secret == \"the secret\")[title]";

    let owner = rows(
        f.run_case_as(&pg, src, &[], Some(OWNER))
            .expect("owner reads"),
    );
    assert_eq!(owner.len(), 1, "the owner's guess is confirmed");

    let other = rows(
        f.run_case_as(&pg, src, &[], Some(OTHER))
            .expect("other reads"),
    );
    assert!(
        other.is_empty(),
        "a correct guess about an unreadable dossier must not return the row: {other:?}"
    );
}

/// **The bridge (S6-001).** `people` and `places` are both open and connect
/// only through `assignments`, which the compiler inserts and the author
/// never lists. Which person is assigned to which place is content of an
/// `assignments` row, so a caller who may not read that row must not learn
/// it from a query that never names the entity.
#[test]
fn an_inferred_bridge_is_guarded_like_a_listed_source() {
    let (f, pg) = seeded("gcj_bridge");
    let src = "people[name] places[label]";

    let owner = rows(
        f.run_case_as(&pg, src, &[], Some(OWNER))
            .expect("owner reads"),
    );
    assert_eq!(owner, vec![json!({"name": "Alice", "label": "Vault"})]);

    let other = rows(
        f.run_case_as(&pg, src, &[], Some(OTHER))
            .expect("other reads"),
    );
    assert_eq!(
        other,
        vec![json!({"name": "Alice", "label": null})],
        "the person is open and stays; the place is reachable only through an \
         assignment the caller may not read, so it is null"
    );
}

/// **The signpost (the third instance, found by auditing for the first
/// two).** `whole` returns a row plus a one-hop object for each ref it
/// holds. The anchor's guard was inlined; the signpost's target was read
/// with the space predicate and nothing else, so a whole-read of an open
/// `cases` row handed over the scalars of a `dossiers` row its caller may
/// not read.
///
/// Same rule, same answer: a target the caller may not read looks like a
/// target that is not there, so the signpost is null.
#[test]
fn a_whole_read_does_not_signpost_a_target_the_caller_may_not_read() {
    let (f, pg) = seeded("gcj_whole");
    let src = "whole cases(\"cas_1\")";

    let owner = f.run_whole_as(&pg, src, Some(OWNER)).expect("owner reads");
    let owner_text = owner.to_string();
    assert!(
        owner_text.contains("the secret"),
        "the owner's whole-read carries the dossier: {owner_text}"
    );

    let other = f.run_whole_as(&pg, src, Some(OTHER)).expect("other reads");
    let other_text = other.to_string();
    assert!(
        other_text.contains("Case one"),
        "the case itself is open and must still be returned: {other_text}"
    );
    assert!(
        !other_text.contains("the secret"),
        "a whole-read must not carry a dossier its caller may not read: {other_text}"
    );
}
