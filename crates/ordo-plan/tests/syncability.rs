//! `plan-replica-feed.md` §1: which reads a device can keep in sync, and the
//! rule each other read fails. Bound from source, so no database.

use ordo_plan::artifact::{
    anchor_id_key, project_row_key, syncability, IdKey, SyncRule, Synced, ROW_KEY,
};
use ordo_schema::build;
use ordo_sema::ir::From;
use ordo_sema::{bind, Registry};
use ordo_syntax::{parse_pipeline, parse_program};

const SRC: &str = r#"
entity customers {
  name:  text
  spend: money
}

op topFive =
  customers[id, name]
  sort name
  take 5

op topFiveNames =
  customers[name]
  sort name
  take 5

op spendByName =
  customers[name, spend]
  group name { total: sum spend }
"#;

/// A second entity carrying the ref, so a juxtaposed read fans out: the same
/// shape `ordo-sema`'s own bind test uses to prove `many_side`.
const JOINED: &str = r#"
entity customers {
  name: text
}

entity orders {
  customer -> customers
  total: money
}
"#;

fn q(src: &str) -> ordo_sema::ir::Query {
    let p = parse_program(SRC).expect("fixture parses");
    let schema = build(&p).expect("schema builds");
    let registry = Registry::from_program(&p);
    bind(
        &schema,
        &registry,
        &parse_pipeline(src).expect("parses"),
        ordo_sema::Clock::CORPUS,
    )
    .expect("binds")
}

fn customers(id_output: &str) -> Result<Synced, SyncRule> {
    Ok(Synced {
        entity: "customers".into(),
        id_output: id_output.into(),
    })
}

#[test]
fn a_projected_id_keys_the_rows() {
    assert_eq!(syncability(&q("customers[id, name]")), customers("id"));
    assert_eq!(
        syncability(&q("customers(spend > 10)[name, id]\n  sort name")),
        customers("id")
    );
}

#[test]
fn an_unprojected_id_no_longer_refuses_it_attaches() {
    // Until 2026-09-16 this was `Err(IdNotProjected)`: an author who did not
    // type `id` could not sync the op at all. `ui-gen.md` §1 always said the
    // anchor's id rides every list row even when unprojected, and now it does.
    assert_eq!(syncability(&q("customers[name]")), customers(ROW_KEY));
}

#[test]
fn grouped_rows_and_measures_have_no_identity() {
    assert_eq!(
        syncability(&q(
            "customers[name, spend]\n  group name { total: sum spend }"
        )),
        Err(SyncRule::Grouped)
    );
    assert_eq!(
        syncability(&q("customers[spend]\n  reduce { n: count }")),
        Err(SyncRule::Measure)
    );
}

/// A callee with a `take` is wrapped as a subquery, and the id reaches the
/// caller only if the callee projected it.
#[test]
fn the_id_is_followed_through_a_wrapped_callee() {
    let through = q("topFive[id, name]");
    assert!(
        matches!(through.from, From::Sub { .. }),
        "the premise: a callee with a take is wrapped"
    );
    assert_eq!(syncability(&through), customers("id"));

    // A callee that did not project its own id carries its tiebreak out as
    // `$tb0`, so the id rides at depth too and this syncs under the attached
    // name rather than refusing.
    let lost = q("topFiveNames[name]");
    assert!(matches!(lost.from, From::Sub { .. }));
    assert_eq!(syncability(&lost), customers(ROW_KEY));
}

/// The manifest's `anchor` recurses through a subquery without asking whether
/// it grouped; syncability refuses a group at any depth.
#[test]
fn a_group_inside_a_callee_still_refuses() {
    let caller = q("spendByName[name, total]");
    assert!(
        matches!(caller.from, From::Sub { .. }),
        "the premise: a grouped callee is wrapped"
    );
    assert!(caller.group.is_none(), "the caller itself does not group");
    assert_eq!(syncability(&caller), Err(SyncRule::Grouped));
}

/// Bind against a schema with a many-side edge.
fn joined(src: &str) -> ordo_sema::ir::Query {
    let p = parse_program(JOINED).expect("fixture parses");
    let schema = build(&p).expect("schema builds");
    let registry = Registry::from_program(&p);
    bind(
        &schema,
        &registry,
        &parse_pipeline(src).expect("parses"),
        ordo_sema::Clock::CORPUS,
    )
    .expect("binds")
}

/// Both of these PROJECT the id, so until 2026-09-16 they were syncable and
/// wrong. The old `id_not_projected` was refusing the shapes by accident, which meant
/// the accident stopped applying the moment an author added `id`.
#[test]
fn distinct_rows_are_not_stored_rows() {
    let q = q("customers[id, name]\ndistinct");
    assert!(q.distinct, "the fixture must actually set distinct");
    assert_eq!(syncability(&q), Err(SyncRule::Distinct));
    assert_eq!(SyncRule::Distinct.as_str(), "distinct");
}

#[test]
fn a_many_side_join_fans_one_anchor_row_into_several() {
    // `orders` carries the ref, so it is the many side: one customer row
    // becomes one result row per order, and the customer's id keys none of
    // them uniquely.
    let q = joined("customers[id, name]  orders[total]");
    assert!(
        q.source_joins.iter().any(|j| j.many_side),
        "the fixture must actually fan out"
    );
    assert_eq!(syncability(&q), Err(SyncRule::ManySideJoin));
    assert_eq!(SyncRule::ManySideJoin.as_str(), "many_side_join");
}

#[test]
fn a_plain_read_of_the_same_schema_still_syncs() {
    // The control: the refusals above are about the shape, not the schema.
    let q = joined("customers[id, name]");
    assert_eq!(
        syncability(&q),
        Ok(Synced {
            entity: "customers".into(),
            id_output: "id".into()
        })
    );
}

// ── C4e: finding the anchor id, whether or not the author projected it ──

#[test]
fn a_projected_id_is_found_under_its_own_name() {
    assert_eq!(
        anchor_id_key(&q("customers[id, name]")),
        Some(IdKey::Projected("id".into()))
    );
    // The author's name for it, not the column's.
    assert_eq!(
        anchor_id_key(&q("customers[cid: id, name]")),
        Some(IdKey::Projected("cid".into()))
    );
}

#[test]
fn an_unprojected_id_rides_the_first_hidden_cell() {
    // Terms: `name` (projected, so no hidden cell) then the `id` tiebreak.
    // The id is the FIRST hidden key even though it is the second term.
    assert_eq!(
        anchor_id_key(&q("customers[name]\nsort name")),
        Some(IdKey::Hidden(0))
    );
}

#[test]
fn the_count_is_of_hidden_keys_and_not_of_terms() {
    // Terms: `spend` (not projected, so hidden cell 0) then `id` (cell 1).
    // Counting TERMS would say 1 either way; counting hidden keys is what
    // makes this different from the case above, and getting it wrong reads
    // a sort value as the row's id.
    assert_eq!(
        anchor_id_key(&q("customers[name]\nsort spend")),
        Some(IdKey::Hidden(1))
    );
}

#[test]
fn an_output_merely_named_id_is_not_the_anchor_id() {
    // `customers[id: name]` projects the NAME under the label `id`. Keying
    // rows by it would file two customers who share a name into one row.
    assert_eq!(
        anchor_id_key(&q("customers[id: name]")),
        Some(IdKey::Hidden(0))
    );
}

#[test]
fn a_grouped_result_has_no_anchor_id_to_find() {
    // Group destroys row identity: the tiebreak becomes the group keys, so
    // the anchor's id is not an order term and there is nothing to attach.
    let grouped = q("customers[name, spend]\ngroup name { total: sum spend }");
    assert_eq!(anchor_id_key(&grouped), None);
}

// ── C4e: putting the anchor id in the projection, gated by syncability ──

#[test]
fn an_author_who_projected_the_id_gets_nothing_attached() {
    // The 32 ops that already project it keep their manifests byte for byte,
    // which is why they do not re-version.
    let mut it = q("customers[id, name]");
    let before = it.outputs.len();
    let key = project_row_key(&mut it).expect("syncable");
    assert_eq!(key.name, "id");
    assert!(!key.attached);
    assert_eq!(it.outputs.len(), before, "nothing added");
}

#[test]
fn an_unprojected_id_is_attached_as_a_real_output() {
    let mut it = q("customers[name]\nsort name");
    let key = project_row_key(&mut it).expect("syncable once attached");
    assert_eq!(key.name, ROW_KEY);
    assert!(key.attached);

    // A genuine output, so the SQL, the manifest and both read paths see it.
    let added = it.outputs.last().expect("an output was pushed");
    assert_eq!(added.name, ROW_KEY);

    // And it is the id, cloned from the order term rather than rebuilt: the
    // projection now carries the very expression the planner ordered by, so
    // the key cannot ride both projected and hidden.
    assert_eq!(anchor_id_key(&it), Some(IdKey::Projected(ROW_KEY.into())));
}

#[test]
fn a_wrapped_callee_that_lost_its_id_still_gets_one() {
    // `topFiveNames` has a `take`, so it is wrapped, and it projects only
    // `name`. The binder carries the callee's tiebreak out as `$tb0`, so the
    // id does ride and the rule's "any anchored, ungrouped read" is real.
    let mut it = q("topFiveNames[name]");
    assert!(matches!(it.from, From::Sub { .. }), "the premise: wrapped");
    let key = project_row_key(&mut it).expect("the id rides as `$tb0`");
    assert_eq!(key.name, ROW_KEY);
    assert_eq!(key.entity, "customers");
    assert!(key.attached);
}

#[test]
fn nothing_is_attached_to_rows_that_have_no_identity() {
    // The gate. `anchor_id_key` alone would answer for some of these, because
    // the id is still an order term; attaching on it would put `$id` on
    // fan-out rows and on synthetic rows. The answer here must be the answer
    // `syncability` gives the door.
    let mut grouped = q("customers[name, spend]\ngroup name { total: sum spend }");
    assert_eq!(project_row_key(&mut grouped), None);

    let mut measure = q("customers[name]\nreduce { n: count }");
    assert_eq!(project_row_key(&mut measure), None);

    let mut distinct = q("customers[id, name]\ndistinct");
    assert_eq!(project_row_key(&mut distinct), None);

    let mut fanned = joined("customers[id, name]  orders[total]");
    assert_eq!(project_row_key(&mut fanned), None);
}
