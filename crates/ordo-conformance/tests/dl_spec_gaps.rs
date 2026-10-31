//! Registration gates on the op wrapper and its parameters, compile tier.
//!
//! Closes four lines of `examples/syntax/README.md`'s Findings (2026-10-04,
//! board `dl-spec-gaps`): settled syntax the compiler refused, and settled
//! rules it did not enforce.
//!
//! - **`reproducible` forbids the clock.** DL §13 (the op wrapper) and DL
//!   §4 say `since` is disallowed under `reproducible`; DL §2 Parameters
//!   says `filled` under `reproducible` is a compile error. One class,
//!   `reproducible_clock`, because the defect is one: an op that promises
//!   the same answer tomorrow and reads today's date.
//! - **`allocate` is refused in a write's braces.** DL §3 (Allocation):
//!   legal only inside an `each` emission over a list parameter, anywhere
//!   else `invalid_allocate`. The read binder and the planner already
//!   refused it; the write folder did not.
//! - **`$x: date filled` registers** and the artifact carries the mark,
//!   with the parameter still required on the wire (DL: "not a default,
//!   not optional").
//! - **`$payee: text(64)` registers** and the artifact carries the bound
//!   as `max`, the same key the door reads for `bound_violation`.
//!
//! Nothing here touches a database.

use ordo_conformance::{Fixture, FIXTURE, SEED};

fn compiles(extra: &str) -> Fixture {
    Fixture::from_source(&format!("{FIXTURE}\n{extra}"), SEED)
        .unwrap_or_else(|e| panic!("must compile:\n{extra}\n{e}"))
}

fn refusal(extra: &str) -> String {
    match Fixture::from_source(&format!("{FIXTURE}\n{extra}"), SEED) {
        Ok(_) => panic!("compiled, and must not have:\n{extra}"),
        Err(e) => e.to_string(),
    }
}

// ── reproducible ─────────────────────────────────────────────────────────

#[test]
fn a_reproducible_op_may_not_use_since() {
    let e =
        refusal("op recentOrders reproducible =\n  orders(placed since 7d)[id, total]\n  take 5\n");
    assert!(e.contains("reproducible_clock"), "{e}");
    assert!(e.contains("since"), "{e}");
}

#[test]
fn a_reproducible_op_may_not_hide_since_in_a_let_or_a_filter() {
    let e = refusal(
        "op viaLet reproducible =\n  let recent = orders(placed since 7d)[id]\n  \
         recent[id]\n  take 5\n",
    );
    assert!(e.contains("reproducible_clock"), "{e}");
    let e =
        refusal("op viaBar reproducible =\n  orders[id, created]\n  | placed since 7d\n  take 5\n");
    assert!(e.contains("reproducible_clock"), "{e}");
}

/// A write's condition reads the clock the same way: a bulk converge over a
/// `since` window converges different rows tomorrow.
#[test]
fn a_reproducible_write_may_not_range_over_since() {
    let e = refusal(
        "op closeStale reproducible =\n  set orders(placed since 30d) { status: \"closed\" }\n",
    );
    assert!(e.contains("reproducible_clock"), "{e}");
    let e = refusal("op purgeOld reproducible =\n  del orders(placed since 30d)\n");
    assert!(e.contains("reproducible_clock"), "{e}");
    // The addressed form has no window and registers.
    compiles(
        "op closeOne($id: id) reproducible =\n  set orders(id == $id) { status: \"closed\" }\n",
    );
}

#[test]
fn a_reproducible_op_may_not_take_a_filled_parameter() {
    let e = refusal(
        "op ordersBefore($day: date filled) reproducible =\n  orders(placed < $day)[id]\n  take 5\n",
    );
    assert!(e.contains("reproducible_clock"), "{e}");
    assert!(e.contains("$day") && e.contains("filled"), "{e}");
}

/// The same window, bounded by a parameter instead of the clock, is what
/// DL §4 tells the author to write, and it registers.
#[test]
fn a_reproducible_op_with_a_date_parameter_registers() {
    let f = compiles(
        "op ordersBefore($day: date) reproducible =\n  orders(placed < $day)[id]\n  take 5\n",
    );
    let a =
        ordo_plan::artifact::op_artifact(&f.schema, &f.registry, "ordersBefore").expect("artifact");
    assert_eq!(a["flags"]["reproducible"], true);
}

/// The flag without `reproducible` is unaffected: `since` is the one
/// blessed nondeterminism everywhere else.
#[test]
fn since_without_reproducible_still_registers() {
    compiles("op recentOrders =\n  orders(placed since 7d)[id, total]\n  take 5\n");
}

// ── allocate outside each ────────────────────────────────────────────────

#[test]
fn allocate_in_a_write_is_invalid_allocate() {
    let e = refusal(
        "op splitOne($id: id, $x: money) =\n  set products(id == $id) { price: allocate($x, 1) }\n",
    );
    assert!(e.contains("invalid_allocate"), "{e}");
    // Nested inside an expression too.
    let e = refusal(
        "op splitNested($id: id, $x: money) =\n  set products(id == $id) { price: 1 + allocate($x, 1) }\n",
    );
    assert!(e.contains("invalid_allocate"), "{e}");
}

// ── filled ───────────────────────────────────────────────────────────────

#[test]
fn a_filled_parameter_registers_and_stays_required() {
    let f =
        compiles("op thirsty($today: date filled) =\n  orders(placed < $today)[id]\n  take 5\n");
    let a = ordo_plan::artifact::op_artifact(&f.schema, &f.registry, "thirsty").expect("artifact");
    let p = &a["params"][0];
    assert_eq!(p["name"], "today");
    assert_eq!(p["filled"], true, "the mark rides the artifact");
    assert_eq!(p["required"], true, "`filled` is not a default");
    // And an unmarked parameter says so.
    let f = compiles("op plain($today: date) =\n  orders(placed < $today)[id]\n  take 5\n");
    let a = ordo_plan::artifact::op_artifact(&f.schema, &f.registry, "plain").expect("artifact");
    assert_eq!(a["params"][0]["filled"], false);
}

/// Ruled 2026-10-05 (`dl-spec-gaps` ask 5): `filled` marks only `date` and
/// `datetime`, the values a client produces alone from its clock. Every
/// other type is refused at registration with `filled_type`.
#[test]
fn filled_is_refused_on_any_type_but_date_and_datetime() {
    for (ty, cond) in [
        ("int", "orderItems(qty < $x)"),
        ("text", "orders(status == $x)"),
        ("bool", "vendors(active == $x)"),
        ("id", "orders(id == $x)"),
        ("money", "orders(total < $x)"),
    ] {
        let e = refusal(&format!(
            "op guessed($x: {ty} filled) =\n  {cond}[id]\n  take 5\n"
        ));
        assert!(e.contains("filled_type"), "{ty}: {e}");
        assert!(e.contains("$x") && e.contains("date"), "{ty}: {e}");
    }
    // Both clock types register.
    compiles("op byDay($d: date filled) =\n  orders(placed < $d)[id]\n  take 5\n");
    compiles(
        "entity visits {\n  at: datetime\n}\n\
         op byNow($t: datetime filled) =\n  visits(at < $t)[id]\n  take 5\n",
    );
}

// ── text(n) ──────────────────────────────────────────────────────────────

#[test]
fn a_text_bound_registers_and_is_the_parameters_max() {
    let f = compiles(
        "op nameIt($id: id, $name: text(64)) =\n  set products(id == $id) { name: $name }\n",
    );
    let a = ordo_plan::artifact::op_artifact(&f.schema, &f.registry, "nameIt").expect("artifact");
    let p = &a["params"][1];
    assert_eq!(p["name"], "name");
    assert_eq!(p["type"], "text");
    assert_eq!(p["max"], "64", "the length bound is the parameter's bound");
}

/// A derived std op carries the field's bound onto its parameter, so the
/// door refuses an over-long value before the write path has to.
#[test]
fn a_bounded_field_bounds_the_derived_std_params() {
    let f = compiles("entity tags (key: tag) { name: text(8)\n note: text }\n");
    for op in ["std.tags.create", "std.tags.update"] {
        let a = ordo_plan::artifact::op_artifact(&f.schema, &f.registry, op).expect("artifact");
        let params = a["params"].as_array().expect("params");
        let name = params.iter().find(|p| p["name"] == "name").expect("$name");
        assert_eq!(name["max"], "8", "{op}");
        let note = params.iter().find(|p| p["name"] == "note").expect("$note");
        assert!(note["max"].is_null(), "{op}: an unbounded text has no max");
    }
}

// ── std sealed ───────────────────────────────────────────────────────────

/// DL §3, "Sealed fields and derived std ops — `std sealed` [settled]":
/// the marker puts the entity's sealed fields into its derived std ops'
/// projections and braces. Without it the derived surface is the open
/// skeleton (`std_library.rs` pins that half).
#[test]
fn std_sealed_opts_the_derived_ops_into_the_sealed_fields() {
    let f = compiles(
        "entity patients (key: pat) {\n  name: text sealed\n  diagnosis: text sealed\n  \
         admitted: date\n  std sealed\n}\n",
    );
    let outputs = |op: &str| -> Vec<String> {
        let a = ordo_plan::artifact::op_artifact(&f.schema, &f.registry, op).expect("artifact");
        a["outputs"]
            .as_array()
            .expect("outputs")
            .iter()
            .map(|o| o["name"].as_str().expect("name").to_string())
            .collect()
    };
    for op in ["std.patients.get", "std.patients.list"] {
        let out = outputs(op);
        assert!(out.contains(&"name".to_string()), "{op}: {out:?}");
        assert!(out.contains(&"diagnosis".to_string()), "{op}: {out:?}");
        assert!(out.contains(&"admitted".to_string()), "{op}: {out:?}");
    }
    let params: Vec<String> = f
        .registry
        .get("std.patients.update")
        .expect("registered")
        .params
        .iter()
        .map(|p| p.name.name.clone())
        .collect();
    assert!(
        params.contains(&"diagnosis".to_string()),
        "update writes the sealed field: {params:?}"
    );

    // And the class is honest about it: `get` opens one row's payloads,
    // `list` one page's.
    for op in ["std.patients.get", "std.patients.list"] {
        let a = ordo_plan::artifact::op_artifact(&f.schema, &f.registry, op).expect("artifact");
        assert_eq!(a["cost"]["class"], "bounded-host", "{op}: {}", a["cost"]);
    }
}

/// The control: the same entity without the marker derives over the open
/// skeleton, as before.
#[test]
fn without_std_sealed_the_derived_surface_is_the_open_skeleton() {
    let f = compiles(
        "entity patients (key: pat) {\n  name: text sealed\n  diagnosis: text sealed\n  \
         admitted: date\n}\n",
    );
    let a = ordo_plan::artifact::op_artifact(&f.schema, &f.registry, "std.patients.get")
        .expect("artifact");
    let out: Vec<&str> = a["outputs"]
        .as_array()
        .expect("outputs")
        .iter()
        .filter_map(|o| o["name"].as_str())
        .collect();
    assert_eq!(out, ["id", "rev", "admitted"]);
}

// ── a page bounds a sealed projection ────────────────────────────────────

/// DL §5: `take` bounds the host-side work in a projection; the 2026-09-05
/// ruling makes a keyset page a row bound. Together: a paged op whose only
/// host work is a sealed projection is `bounded-host` and needs no
/// acknowledgment, which is what lets `std sealed`'s `list` derive.
#[test]
fn a_paged_sealed_projection_is_bounded_host_without_an_acknowledgment() {
    let f = compiles("op memos =\n  orders[id, memo]\n  sort -placed\n");
    let a = ordo_plan::artifact::op_artifact(&f.schema, &f.registry, "memos").expect("artifact");
    assert_eq!(a["cost"]["class"], "bounded-host", "{}", a["cost"]);
    // The control: the same projection with `take` was always bounded.
    let f = compiles("op fiveMemos =\n  orders[id, memo]\n  sort -placed\n  take 5\n");
    let a =
        ordo_plan::artifact::op_artifact(&f.schema, &f.registry, "fiveMemos").expect("artifact");
    assert_eq!(a["cost"]["class"], "bounded-host", "{}", a["cost"]);
}

/// A group or a reduce over a sealed field is plain SQL (ruled 2026-10-05,
/// `audit-2026-10-05` ask 1). Nothing sealed survives a fold: a sealed group
/// key and an aggregate over a sealed field are refused, and `count` counts
/// rows without opening them. So the fold opens nothing, releases no key and
/// needs no `unbounded`.
///
/// This test was tightened the other way earlier on 2026-10-05, when the
/// fold was counted as host work over every row before it. The owner ruled
/// the opposite, and the test now pins the ruling.
#[test]
fn a_group_or_reduce_over_a_sealed_field_is_plain_sql() {
    for body in [
        "orders[placed, memo]\n  group placed { n: count }\n  sort placed\n",
        "orders[placed, memo]\n  reduce { n: count }\n",
        "orders[placed, memo]\n  group placed { n: count }\n  sort placed\n  take 3\n",
        // An aggregate may take the sealed field's old name: it no longer
        // marks anything to open.
        "orders[placed, memo]\n  group placed { memo: count }\n  sort placed\n",
        // Through a join, too.
        "customers[name] orders[placed, memo]\n  group name { n: count }\n  sort name\n",
    ] {
        let src = format!("op folded =\n  {body}");
        let f = Fixture::from_source(&format!("{FIXTURE}\n{src}"), SEED).unwrap_or_else(|e| {
            panic!("{body}: a fold over a sealed field needs no `unbounded`: {e}")
        });
        let a =
            ordo_plan::artifact::op_artifact(&f.schema, &f.registry, "folded").expect("artifact");
        assert_eq!(a["cost"]["class"], "sql", "{body}: {}", a["cost"]);
        let keys = a["release"]["keys"].as_array().expect("keys");
        assert!(keys.is_empty(), "{body}: a fold releases no key: {keys:?}");
        assert!(
            a["release"]["purpose"].is_null(),
            "{body}: {}",
            a["release"]
        );
    }
    // Over a callee that opens the memo and is settled by its `take`, so it
    // is wrapped as a sub-select: the fold reads no memo, so the
    // sub-select selects none (reported fix 25v) and the op is plain SQL.
    let f = compiles(
        "op lastMemos =\n  orders[placed, memo]\n  sort -placed\n  take 5\n\
         op lastCounts =\n  lastMemos\n  group placed { n: count }\n  sort placed\n",
    );
    let a =
        ordo_plan::artifact::op_artifact(&f.schema, &f.registry, "lastCounts").expect("artifact");
    assert_eq!(a["cost"]["class"], "sql", "{}", a["cost"]);
    assert!(a["release"]["purpose"].is_null(), "{}", a["release"]);
    // The control: the same projection without the fold opens the memo.
    let f = compiles("op memos =\n  orders[placed, memo]\n  sort placed\n");
    let a = ordo_plan::artifact::op_artifact(&f.schema, &f.registry, "memos").expect("artifact");
    assert_eq!(a["cost"]["class"], "bounded-host", "{}", a["cost"]);
    assert_eq!(a["release"]["keys"][0]["field"], "memo", "{}", a["release"]);
}

// ── the group walk ───────────────────────────────────────────────────────

/// DL §3 Groups: the hand-written group disjunct,
/// `member.groupMembers.group(member == $caller)`, binds and registers.
/// `group` is the platform ref's name and is read as a segment after the
/// dot; the acl derivation has emitted this walk for a while, and now an
/// author can write the same one.
#[test]
fn a_hand_written_group_walk_binds() {
    let f = compiles(
        "entity teamNotes (key: tnote) {\n  team -> members\n  body: text\n  \
         access read   team == $caller or team.groupMembers.group(member == $caller)\n  \
         access write  team == $caller\n}\n\
         op groupHeads unbounded =\n  groupMembers[member, group.displayName]\n  \
         match groupMembers.group\n  sort member\n",
    );
    let e = f.schema.entity("teamNotes").expect("the shape");
    let guards = e.guards.as_ref().expect("guarded");
    let read = format!("{:?}", guards.read);
    assert!(
        read.contains("groupMembers"),
        "the group disjunct survives into the guard: {read}"
    );
    let a =
        ordo_plan::artifact::op_artifact(&f.schema, &f.registry, "groupHeads").expect("artifact");
    let out: Vec<&str> = a["outputs"]
        .as_array()
        .expect("outputs")
        .iter()
        .filter_map(|o| o["name"].as_str())
        .collect();
    assert_eq!(
        &out[..2],
        ["member", "displayName"],
        "the paged tiebreak may follow"
    );
}
