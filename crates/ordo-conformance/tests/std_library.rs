//! **The std library, executed** — `docs/std-library.md`, checklist 9k.
//!
//! The library is *derived*, so the thing worth testing is not that the ops
//! exist but that **being derived buys them no exemption**. A std op over a
//! guarded entity is a whole-entity read-and-write surface for every entity
//! at once; if the derivation could miss a guard, skip a registration gate,
//! or widen a manifest, that is not one bug — it is the same bug repeated
//! once per shape in the schema. So every assertion below is of the form
//! *the derived op answers what the hand-written op answers*, with the
//! hand-written op executed in the same test, against the same rows, as the
//! same principal. A test that only asserted the derived op's behaviour
//! would pass just as happily if the guard were inlined into both or into
//! neither.
//!
//! Callers and seed are the guards corpus's (`gc_support`): Ana holds
//! `canWrite` in `proj_1`, Bram reads `proj_1` but cannot write it, Cleo is
//! in `proj_2` only, and Dov holds no project membership at all. Those four
//! standings are exactly the three answers an addressed write has to be able
//! to give — applied, `access_guard`, `not_found` — plus the fourth that
//! makes `not_found` non-informative.

mod gc_support;

use gc_support::{db, fixture, ANA, BRAM, CLEO, DOV};
use ordo_conformance::{Fixture, Pg};
use serde_json::json;

/// The error class of a refusal — every harness entry point puts the class
/// first. (`gc_support::class_of`, restated so this file reads alone.)
fn class(e: &str) -> &str {
    e.split(':').next().unwrap_or(e).trim()
}

/// This suite's own seeded read database.
///
/// Deliberately **not** `gc_support::seeded_reads`, which is
/// `ordo_gc_reads`: that helper rebuilds its database once per *test
/// binary*, and this is a second binary. Two `Fixture::reset` calls against
/// one database race inside Postgres's own catalog — the collision
/// `gc_support` documents and that `Pg::named` exists to prevent. Sharing a
/// read-only database across suites looks free and is not.
fn std_reads(f: &Fixture) -> Pg {
    static ONCE: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    let pg = db("ordo_std_reads");
    ONCE.get_or_init(|| f.reset(&pg))
        .clone()
        .expect("fixture DDL applies and seeds");
    pg
}

/// `std.tasks.update` for `task_1`, spelled once. Full-row CAS (§2.2): every
/// open field, every time, and `$rev` is mandatory.
fn update_task_1(title: &str) -> String {
    format!(
        r#"std.tasks.update($id: "task_1", $rev: 1, $project: "proj_1", $assignee: "mem_1", $title: "{title}", $status: "open")"#
    )
}

// ═════════════════════════════════════════════════════════════════════════
// A. The derivation is a function of the schema, and of nothing else
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn the_derived_surface_is_exactly_what_the_shapes_imply() {
    let f = Fixture::load().expect("the market fixture compiles");
    let has = |n: &str| f.registry.get(n).is_some();

    // Derived for every entity: the addressed read, the aggregate, and the
    // four write verbs.
    for verb in ["get", "count", "create", "update", "delete"] {
        assert!(has(&format!("std.tasks.{verb}")), "std.tasks.{verb}");
    }

    // `deleteSubtree` is a TREE op and its own op — never a flag on delete
    // (§2), so it is separately grantable and separately consented.
    assert!(has("std.categories.deleteSubtree"), "categories is a tree");
    assert!(
        !has("std.tasks.deleteSubtree"),
        "tasks is not a tree — a cascade op over it would name a cascade \
         the shape cannot have"
    );

    // `put` derives only where exactly one unique key exists (§2): with two,
    // identity-precedence rule 8 makes any covering statement ambiguous and
    // std refuses to pick a key.
    assert!(has("std.customers.put"), "customers.email is unique");
    assert!(has("std.categories.put"), "categories.name is unique");
    assert!(
        has("std.boardsAccess.put"),
        "the acl link entity's composite `unique (board, member)` counts as one"
    );
    assert!(has("std.vendors.create"), "vendors declares no unique key");
    assert!(
        !has("std.vendors.put"),
        "…so there is no key to converge on"
    );

    // `coupons` declares `code` AND `slug` unique. The language refuses a
    // statement covering two identities (DL §6 rule 8 — `claimCoupon` is in
    // the fixture to be refused), so neither `put` NOR `create` derives.
    // The alternative — deriving one anyway — is an op that cannot register.
    assert!(!has("std.coupons.put"), "two unique keys: no key to pick");
    assert!(
        !has("std.coupons.create"),
        "two unique keys: the create statement would claim two identities"
    );
    // But the ops that need no key still derive: an entity is not excluded
    // wholesale for having an ambiguous identity.
    assert!(has("std.coupons.get") && has("std.coupons.delete"));

    // The platform's own entity derives NOTHING. `members` is unguarded, so
    // a derived read is a whole-space principal directory and a derived
    // write is an unattributed path to minting and re-roling principals —
    // which is the access model's job, done by attributed commands.
    for verb in ["get", "count", "create", "put", "update", "delete"] {
        assert!(
            !has(&format!("std.members.{verb}")),
            "std.members.{verb} must not exist: the platform's principal \
             entity is administered by commands, not by derived CRUD"
        );
    }
}

#[test]
fn the_std_namespace_is_reserved_by_the_grammar() {
    // §6 reserves `std.` wholesale. No check is needed to enforce it: an
    // authored op name is one identifier, and the lexer admits no dot in
    // one — so the collision `reserved_derived_name` exists to catch on the
    // acl side is unspellable here.
    let e = ordo_syntax::parse_program("op std.tasks.get($id: id) =\n  tasks[title]\n")
        .expect_err("a dotted op NAME does not parse");
    assert!(
        !e.message.is_empty(),
        "the parser refuses it with a message: {e:?}"
    );

    // The other half of §7, which DOES have to work: a qualified name in
    // SOURCE position parses, because that is how a std op is called and
    // composed. The dots are part of the name, never a traversal.
    let pl = ordo_syntax::parse_pipeline(r#"std.tasks.get($id: "task_1")"#)
        .expect("a qualified name parses in source position");
    assert_eq!(pl.sources[0].name.name, "std.tasks.get");
}

// ═════════════════════════════════════════════════════════════════════════
// B. No exemption: the gates, the guards, the manifest
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn no_derived_op_carries_a_flag_or_a_bypass() {
    let f = Fixture::load().expect("fixture");
    let mut checked = 0;
    for name in f.registry.names().filter(|n| n.starts_with("std.")) {
        let op = f.registry.get(name).expect("registered");
        // §5.1: std never uses `unguarded` — there is no derived bypass.
        // §5.7: no std op ever needs the `unbounded` acknowledgment, because
        // a derivation cannot acknowledge on a human's behalf.
        assert!(!op.flags.unguarded, "{name} carries `unguarded`");
        assert!(!op.flags.unbounded, "{name} carries `unbounded`");

        let art = ordo_plan::artifact::op_artifact(&f.schema, &f.registry, name)
            .unwrap_or_else(|e| panic!("{name} has an artifact: {e}"));
        assert_eq!(art["bypassed_guards"], json!([]), "{name} bypasses a guard");
        assert_eq!(art["error"], json!(null), "{name} compiles: {art}");
        checked += 1;
    }
    assert!(checked > 50, "the sweep actually ran: {checked} ops");

    // And the gate itself is the shared one. `Fixture::load` already ran
    // `check_registration` over the derived ops beside the authored ones —
    // this asserts that it is genuinely the same call, not a laxer twin.
    ordo_plan::cost::check_registration(&f.schema, &f.registry)
        .expect("the derived library passes the authored registration gate");
}

#[test]
fn every_derived_read_proves_a_row_bound_without_acknowledging_one() {
    let f = Fixture::load().expect("fixture");
    let mut listed = 0;
    for name in f.registry.names().filter(|n| n.starts_with("std.")) {
        let op = f.registry.get(name).expect("registered");
        if op.read_pipeline().is_none() {
            continue;
        }
        let q = ordo_sema::bind_op_generic(&f.schema, &f.registry, name, ordo_sema::Clock::CORPUS)
            .expect("binds");
        // §5.7: every derivation carries a bound the compiler can PROVE —
        // and since 2026-09-05 there are exactly TWO kinds of proof it may
        // carry, which is why they are checked apart rather than joined
        // into one boolean.
        if name.ends_with(".list") {
            listed += 1;
            // The paged half. `list` is the ONLY derived read that leans on
            // it, and it leans on it in the open: `row_bound` still answers
            // `Unbounded` for the plan, and the evidence is the total order
            // the door pages over. Asserting BOTH halves is the point —
            // if the ruling had been implemented by widening the prover,
            // the first assertion would fail here and `std.E.count`'s
            // pre-reduce refinement gate would have silently gone with it.
            assert_eq!(
                ordo_plan::cost::row_bound(&q, &f.schema),
                ordo_plan::cost::RowBound::Unbounded,
                "{name}: paging must not be smuggled into the row-bound prover — \
                 `refine_candidate_bound` reads that prover at a reduce's INPUT, \
                 where no page window exists"
            );
            assert!(
                ordo_plan::cost::keyset_paged(&q),
                "{name} is not keyset-pageable, so nothing bounds it"
            );
        } else {
            // `get` is addressed (unique pin); `count` is reduce-shaped.
            assert_eq!(
                ordo_plan::cost::row_bound(&q, &f.schema),
                ordo_plan::cost::RowBound::AtMost(1),
                "{name} has no proven row bound"
            );
        }
        // And the venue is SQL — no derived read opens a sealed payload,
        // which is what keeps `bounded-host` off the derived surface.
        assert_eq!(
            ordo_plan::cost::classify(&q),
            ordo_plan::cost::CostClass::Sql,
            "{name} does host work"
        );
    }
    assert!(
        listed > 5,
        "the paged branch actually ran: {listed} listings"
    );
}

#[test]
fn sealed_fields_are_absent_from_the_derived_read_surface() {
    let f = Fixture::load().expect("fixture");
    let outputs = |op: &str| -> Vec<String> {
        let art = ordo_plan::artifact::op_artifact(&f.schema, &f.registry, op).expect("artifact");
        art["outputs"]
            .as_array()
            .expect("outputs")
            .iter()
            .map(|o| o["name"].as_str().expect("name").to_string())
            .collect()
    };

    // §2.4: an entity containing sealed fields derives its std READ surface
    // over the open skeleton only. `orders.memo` and `customers.email` are
    // sealed; neither appears, so neither is in the manifest, so neither is
    // on the consent page, so granting `std.orders.get` does not grant the
    // memo. This is the default because the alternative — deriving a
    // full-disclosure read for every entity and relying on nobody granting
    // it — put a sealed field one careless profile template away from every
    // viewer in the space.
    let orders = outputs("std.orders.get");
    assert!(orders.contains(&"total".to_string()), "{orders:?}");
    assert!(
        !orders.contains(&"memo".to_string()),
        "sealed `memo` leaked into std.orders.get: {orders:?}"
    );
    let customers = outputs("std.customers.get");
    assert!(customers.contains(&"tier".to_string()), "{customers:?}");
    assert!(
        !customers.contains(&"email".to_string()),
        "sealed `email` leaked into std.customers.get: {customers:?}"
    );
    // An entity with no sealed field discloses its whole declared row —
    // §5 rule 4, stated rather than discovered.
    let vendors = outputs("std.vendors.get");
    for f in ["name", "region", "active"] {
        assert!(vendors.contains(&f.to_string()), "{vendors:?}");
    }

    // The write half is deliberately NOT symmetric (§2.4): writing a value
    // is not disclosing it, so `create` keeps sealed params — dropping them
    // would break insert-completeness on a required sealed field.
    let params = |op: &str| -> Vec<String> {
        f.registry
            .get(op)
            .expect("registered")
            .params
            .iter()
            .map(|p| p.name.name.clone())
            .collect()
    };
    assert!(
        params("std.orders.create").contains(&"memo".to_string()),
        "sealed fields stay writable"
    );
    // …but `update`'s converge braces are the open skeleton, so a sealed
    // field is left untouched rather than nulled — and the param goes with
    // the brace, because a param no brace consumes is one the manifest
    // advertises and the op ignores.
    assert!(
        !params("std.orders.update").contains(&"memo".to_string()),
        "std.orders.update must not claim a param it cannot write"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// C. The load-bearing pair: derived and authored refuse the same callers
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn a_derived_read_filters_exactly_as_the_authored_read_does() {
    let f = fixture();
    let pg = std_reads(&f);

    // `taskOne($id)` is the guards corpus's hand-written addressed read;
    // `std.tasks.get($id)` is the derived one. Same row, same four
    // principals, same answers — including the empty answer, which is what
    // makes `not_found` non-informative on the write side below.
    for (who, label, visible) in [
        (ANA, "Ana (member of proj_1)", true),
        (BRAM, "Bram (member of proj_1)", true),
        (CLEO, "Cleo (proj_2 only)", false),
        (DOV, "Dov (no membership)", false),
    ] {
        let authored = f
            .run_case_as(&pg, r#"taskOne($id: "task_1")"#, &[], who)
            .expect("authored read runs");
        let derived = f
            .run_case_as(&pg, r#"std.tasks.get($id: "task_1")"#, &[], who)
            .expect("derived read runs");

        let authored_rows = authored.as_array().expect("rows").len();
        let derived_rows = derived.as_array().expect("rows").len();
        assert_eq!(
            authored_rows, derived_rows,
            "{label}: authored saw {authored_rows} rows, derived saw {derived_rows}"
        );
        assert_eq!(
            derived_rows,
            usize::from(visible),
            "{label}: the guard did not decide the derived read"
        );
    }

    // The derived read is the whole open row, so when it IS visible it says
    // more than `taskOne` does — that is the disclosure §5 rule 4 requires
    // the manifest to state, not a guard failure.
    let ana = f
        .run_case_as(&pg, r#"std.tasks.get($id: "task_1")"#, &[], ANA)
        .expect("runs");
    assert_eq!(ana[0]["title"], json!("draft spec"));
    assert_eq!(ana[0]["project"], json!("proj_1"));
    assert_eq!(ana[0]["id"], json!("task_1"));
}

#[test]
fn a_derived_write_refuses_the_callers_the_authored_write_refuses() {
    let f = fixture();
    let pg = db("ordo_std_write_pair");

    // The three answers an addressed write can give, each demanded from the
    // authored op and the derived op in turn. `renameTask` is
    // `set tasks(id == $task) { title: $title }`; `std.tasks.update` is the
    // derived full-row converge over the same rows. They share nothing but
    // the entity — and therefore the guard.
    //
    // Direction 1: what the authored op refuses, the derived op refuses.
    // Direction 2: what the authored op ALLOWS, the derived op allows —
    // without which "refuses everything" would pass this test.
    for (who, label, expect) in [
        (ANA, "Ana: canWrite in proj_1", Ok(())),
        (
            BRAM,
            "Bram: reads proj_1, cannot write it",
            Err("access_guard"),
        ),
        (
            CLEO,
            "Cleo: proj_2 only — outside the read view",
            Err("not_found"),
        ),
        (DOV, "Dov: no membership at all", Err("not_found")),
    ] {
        let mut log = ordo_conformance::WriteLog::new();
        f.reset(&pg).expect("fresh seed");
        let authored = f.run_write_as(
            &pg,
            &mut log,
            r#"renameTask($task: "task_1", $title: "by hand")"#,
            who,
        );

        let mut log = ordo_conformance::WriteLog::new();
        f.reset(&pg).expect("fresh seed");
        let derived = f.run_write_as(&pg, &mut log, &update_task_1("derived"), who);

        match expect {
            Ok(()) => {
                authored.unwrap_or_else(|e| panic!("{label}: authored write refused: {e}"));
                derived.unwrap_or_else(|e| {
                    panic!(
                        "{label}: the AUTHORED write was allowed but the derived one was \
                         refused ({e}) — a derived op must not be stricter either, or the \
                         baseline surface is a surface nobody can use"
                    )
                });
            }
            Err(want) => {
                let a = authored.expect_err(&format!("{label}: authored write must refuse"));
                let d = derived.expect_err(&format!(
                    "{label}: the derived write was ALLOWED where the authored write was \
                     refused — this is the privilege escalation the derivation exists not \
                     to introduce"
                ));
                assert_eq!(class(&a), want, "{label}: authored class: {a}");
                assert_eq!(
                    class(&d),
                    want,
                    "{label}: derived class differs from authored `{want}`: {d}"
                );
            }
        }
    }
}

#[test]
fn not_found_and_access_guard_stay_indistinguishable_where_they_must() {
    let f = fixture();
    let pg = db("ordo_std_not_found");

    // §2.5: on a guarded entity `not_found` means EITHER "no such row" OR
    // "a row you may not read", and the two are deliberately the same
    // answer. `std.tasks.get` is the op most likely to be used as an
    // existence oracle, so the pair is asserted on the derived op directly.
    f.reset(&pg).expect("seed");

    let absent = {
        let mut log = ordo_conformance::WriteLog::new();
        f.run_write_as(&pg, &mut log, r#"std.tasks.delete($id: "task_nope")"#, CLEO)
            .expect_err("no such row")
    };
    let forbidden = {
        let mut log = ordo_conformance::WriteLog::new();
        f.run_write_as(&pg, &mut log, r#"std.tasks.delete($id: "task_1")"#, CLEO)
            .expect_err("a row Cleo may not read")
    };
    assert_eq!(class(&absent), "not_found", "{absent}");
    assert_eq!(
        class(&forbidden),
        "not_found",
        "a row outside the read view must not answer `access_guard` — that \
         answer would confirm the row exists: {forbidden}"
    );

    // And the informative case survives: a caller who MAY read the row gets
    // `access_guard`, because they already know it exists.
    let mut log = ordo_conformance::WriteLog::new();
    let e = f
        .run_write_as(&pg, &mut log, r#"std.tasks.delete($id: "task_1")"#, BRAM)
        .expect_err("Bram reads task_1 but cannot write it");
    assert_eq!(class(&e), "access_guard", "{e}");
}

#[test]
fn std_over_an_acl_link_entity_cannot_undercut_the_grant_op() {
    let f = fixture();
    let pg = db("ordo_std_acl_link");

    // §2: `acl`-derived link entities receive the same std derivation, and
    // there must be **no privilege gap** — `std.boardsAccess.*` cannot be a
    // way around `grantBoardsAccess`. Both are gated by the link entity's
    // own inherited write guard, pinned at the highest declared level.
    //
    // Bram holds `write` on board_1, not `admin`. The escalation this test
    // exists to refuse is Bram raising himself to `admin`, because the row
    // he would be writing is the one that decides who may write rows. It is
    // asserted on the **resulting acl state**, not on an error class: the
    // authored op addresses by condition and the derived ops address by id
    // and by key, so their refusal SHAPES legitimately differ (R-G9 — a
    // conditioned write is bounded by the caller's write view and converges
    // nothing; an addressed write refuses out loud). What must not differ,
    // and what an attacker cares about, is whether the level moved.
    let level_of = |member: &str| -> String {
        let rows = pg
            .query(&format!(
                r#"SELECT "level" FROM "boardsAccess" WHERE "board"='board_1' AND "member"='{member}'"#
            ))
            .expect("readback");
        rows[0][0].clone().expect("a level")
    };

    for (call, label) in [
        (
            r#"setBoardsAccessLevel($board: "board_1", $member: "mem_2", $level: "admin")"#,
            "the authored acl admin op",
        ),
        (
            r#"std.boardsAccess.put($board: "board_1", $member: "mem_2", $level: "admin")"#,
            "the derived converge-or-create",
        ),
        (
            r#"std.boardsAccess.update($id: "ba_2", $rev: 1, $board: "board_1", $member: "mem_2", $level: "admin")"#,
            "the derived full-row converge",
        ),
        (
            r#"std.boardsAccess.delete($id: "ba_1")"#,
            "the derived delete, aimed at the admin's own row",
        ),
    ] {
        f.reset(&pg).expect("fresh seed");
        let mut log = ordo_conformance::WriteLog::new();
        let _ = f.run_write_as(&pg, &mut log, call, BRAM);
        assert_eq!(
            level_of("mem_2"),
            "write",
            "{label}: Bram raised his own acl level — the derived surface is a \
             privilege-escalation path around `grantBoardsAccess`"
        );
        assert_eq!(
            level_of("mem_1"),
            "admin",
            "{label}: Bram removed the admin's grant"
        );
    }

    // The other direction, without which "refuses everything" would pass:
    // Ana DOES hold admin, and the derived op works for her exactly as the
    // authored one does.
    f.reset(&pg).expect("fresh seed");
    let mut log = ordo_conformance::WriteLog::new();
    f.run_write_as(
        &pg,
        &mut log,
        r#"std.boardsAccess.put($board: "board_1", $member: "mem_2", $level: "admin")"#,
        ANA,
    )
    .expect("Ana holds admin on board_1");
    assert_eq!(level_of("mem_2"), "admin", "the admin's grant took effect");
}

#[test]
fn the_derived_aggregate_counts_only_the_rows_the_caller_may_read() {
    let f = fixture();
    let pg = std_reads(&f);

    // §5.1: on a guarded entity the guard inlines into `count` exactly as it
    // does into a read. An aggregate that counted rows the caller cannot see
    // would be an oracle over precisely the rows the guard exists to hide —
    // and it would be a *cheap* one, because a number reveals no row.
    let n = |who| {
        f.run_case_as(&pg, "std.tasks.count", &[], who)
            .expect("counts")[0]["n"]
            .as_i64()
            .expect("an integer")
    };
    // Ana reads proj_1 (2 tasks) and proj_3 (1); Cleo reads proj_2 (2);
    // Dov reads nothing. These are the row counts of `taskBoard` above.
    assert_eq!(n(ANA), 3);
    assert_eq!(n(CLEO), 2);
    assert_eq!(n(DOV), 0, "an empty count, not the table's size");

    // The unguarded contrast, so the assertion above is known to be the
    // guard talking and not an artefact of the seed: `vendors` carries no
    // guard, so every caller sees the same number.
    let v = |who| {
        f.run_case_as(&pg, "std.vendors.count", &[], who)
            .expect("counts")[0]["n"]
            .as_i64()
            .expect("an integer")
    };
    assert_eq!(
        v(ANA),
        v(DOV),
        "an unguarded entity counts the same for all"
    );
    assert!(v(DOV) > 0, "and it is not zero for the wrong reason");
}

// ═════════════════════════════════════════════════════════════════════════
// D. `std.E.list` and the paging ruling (9k′, `corpus-rulings.md` 2026-09-05)
//
// `list` is the one derived read whose row bound is not a number the plan
// carries. Its bound is the door's page, and the evidence is a total order
// — so everything below either executes a page walk or poses the shape the
// ruling refuses. Databases are this section's own, prefixed `ordo_list_`.
// ═════════════════════════════════════════════════════════════════════════

/// This section's seeded read database. Separate from `std_reads` so a page
/// walk and the §C reads cannot race each other's `reset`.
fn list_reads(f: &Fixture) -> Pg {
    static ONCE: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    let pg = Pg::named("ordo_list_reads");
    ONCE.get_or_init(|| f.reset(&pg))
        .clone()
        .expect("fixture DDL applies and seeds");
    pg
}

/// Walk a read to exhaustion **the way the door walks it** — one keyset
/// page at a time, cursor minted from the last row's order keys — and
/// return the `id` of every row the walk visited, in visit order.
///
/// This is `ordo_exec::Executor::read_page_as`'s loop, reproduced here
/// because that method needs a live pool and this suite plans against
/// `psql`. The two must agree about one thing only — which columns carry
/// the cursor — and they do, because both ask `ordo_plan::order_key_outputs`
/// rather than agreeing by convention.
fn walk(f: &Fixture, pg: &Pg, source: &str, caller: Option<&str>, page_size: i64) -> Vec<String> {
    let pl = ordo_syntax::parse_pipeline(source).expect("the call parses");
    let q = ordo_sema::bind_as(
        &f.schema,
        &f.registry,
        &pl,
        &[],
        caller,
        ordo_sema::Clock::CORPUS,
    )
    .expect("binds");
    let names = ordo_plan::order_key_outputs(&q);
    let mut cursor: Option<Vec<Option<String>>> = None;
    let mut ids: Vec<String> = Vec::new();
    // Generous, and finite: a walk that never terminates is a repeating
    // cursor, which is one of the two failures this exists to catch.
    for _ in 0..40 {
        let paged = ordo_plan::plan_paged(&q, &f.plan_space(), page_size, cursor.as_deref())
            .expect("a listing pages");
        let rows = pg.query(&paged.sql).expect("the page runs");
        if rows.is_empty() {
            break;
        }
        let id_at = paged
            .outputs
            .iter()
            .position(|(o, _)| o == "id")
            .expect("a std listing projects `id`");
        for r in &rows {
            ids.push(r[id_at].clone().expect("an id"));
        }
        if (rows.len() as i64) < page_size {
            break;
        }
        let last = rows.last().expect("non-empty").clone();
        let mut hidden = last[paged.outputs.len()..].iter().cloned();
        cursor = Some(
            names
                .iter()
                .map(|n| match n {
                    Some(name) => paged
                        .outputs
                        .iter()
                        .position(|(o, _)| o == name)
                        .and_then(|p| last.get(p).cloned())
                        .flatten(),
                    None => hidden.next().flatten(),
                })
                .collect(),
        );
    }
    ids
}

#[test]
fn the_derived_listing_shows_a_caller_only_their_own_rows() {
    let f = fixture();
    let pg = list_reads(&f);

    // The pair, as everywhere in this file: `taskBoard` is the guards
    // corpus's hand-written whole-entity listing, `std.tasks.list` is the
    // derived one. They share nothing but the entity, and therefore the
    // guard.
    //
    // Ana reads proj_1 (task_1, task_2) and proj_3 (task_5); Bram reads
    // proj_1 and proj_2; Cleo reads proj_2 only; Dov reads nothing.
    let titles = |src: &str, who| -> Vec<String> {
        let mut t: Vec<String> = f
            .run_case_as(&pg, src, &[], who)
            .expect("the listing runs")
            .as_array()
            .expect("rows")
            .iter()
            .map(|r| r["title"].as_str().expect("a title").to_string())
            .collect();
        t.sort();
        t
    };

    for (who, label, expect) in [
        (
            ANA,
            "Ana (proj_1 + proj_3)",
            vec!["audit books", "draft spec", "review spec"],
        ),
        (
            BRAM,
            "Bram (proj_1 + proj_2)",
            vec!["build site", "draft spec", "review spec", "ship site"],
        ),
        (CLEO, "Cleo (proj_2 only)", vec!["build site", "ship site"]),
        (DOV, "Dov (no membership)", vec![]),
        (None, "anonymous (every guard false, G15)", vec![]),
    ] {
        let derived = titles("std.tasks.list", who);
        let authored = titles("taskBoard", who);
        assert_eq!(
            derived, authored,
            "{label}: the derived listing and the authored one disagree"
        );
        assert_eq!(
            derived, expect,
            "{label}: the guard did not decide the list"
        );
    }

    // The second direction, spelled as a set difference rather than as a
    // count, because equal counts would pass a test that returned the wrong
    // rows: no row Cleo may read appears in Ana's listing, and vice versa.
    let ana = titles("std.tasks.list", ANA);
    let cleo = titles("std.tasks.list", CLEO);
    assert!(
        ana.iter().all(|t| !cleo.contains(t)),
        "Ana's listing overlaps Cleo's: {ana:?} vs {cleo:?}"
    );
    assert!(!ana.is_empty() && !cleo.is_empty(), "and neither is empty");
}

#[test]
fn the_guard_travels_into_every_page_not_just_the_first() {
    let f = fixture();
    let pg = list_reads(&f);

    // The assertion the unpaged test above cannot make. A guard inlined
    // into the plan is re-evaluated inside every page's subquery, so a
    // caller's second page is filtered exactly as their first — but the
    // only way to know is to walk, at a page size small enough that the
    // walk has a second page at all.
    //
    // `-id` descending is §10's default sort, so Ana's order is task_5,
    // task_2, task_1.
    let ana = ["task_5", "task_2", "task_1"];
    let cleo = ["task_4", "task_3"];

    for size in [1, 2, 3, 5] {
        assert_eq!(
            walk(&f, &pg, "std.tasks.list", ANA, size),
            ana,
            "page size {size}: Ana's walk is not the newest-first listing of \
             exactly her rows — a keyset that skips or repeats at a page \
             boundary shows up here and nowhere else"
        );
        assert_eq!(
            walk(&f, &pg, "std.tasks.list", CLEO, size),
            cleo,
            "page size {size}: Cleo's walk"
        );
        assert!(
            walk(&f, &pg, "std.tasks.list", DOV, size).is_empty(),
            "page size {size}: Dov holds no membership"
        );
        assert!(
            walk(&f, &pg, "std.tasks.list", None, size).is_empty(),
            "page size {size}: anonymous"
        );
    }

    // And the walk is genuinely the whole set: what Ana pages through is
    // what the aggregate counts, which is §2.1's list/count agreement asked
    // of the page loop rather than of one request.
    let n = |who| {
        f.run_case_as(&pg, "std.tasks.count", &[], who)
            .expect("counts")[0]["n"]
            .as_i64()
            .expect("an integer")
    };
    assert_eq!(n(ANA), ana.len() as i64);
    assert_eq!(n(CLEO), cleo.len() as i64);
    assert_eq!(n(DOV), 0);
}

#[test]
fn a_take_cannot_stand_in_for_paging() {
    let f = fixture();
    let pg = list_reads(&f);

    // The ruling's central reason, executed rather than asserted in prose.
    // `plan_paged` wraps the bound query — take included — in the paged
    // subquery, so a `take n` caps the WHOLE listing at n rather than
    // windowing it, and row n+1 is unreachable by any cursor at any page
    // size. A list that stops silently and looks complete is worse than one
    // that refuses, which is why `std.E.list` carries no take.
    let capped = "tasks[id, rev, project, assignee, title, status]\nsort -id\ntake 2";
    for size in [1, 2, 3] {
        let got = walk(&f, &pg, capped, ANA, size);
        assert_eq!(
            got,
            ["task_5", "task_2"],
            "page size {size}: a take'd listing"
        );
        assert!(
            !got.contains(&"task_1".to_string()),
            "page size {size}: `task_1` is Ana's and the take made it \
             unreachable — this is the row a take-bounded `std.tasks.list` \
             would lose, and it would lose it silently"
        );
    }
    // The listing that carries no take reaches it.
    assert!(walk(&f, &pg, "std.tasks.list", ANA, 2).contains(&"task_1".to_string()));

    // The structural reason, read off the SQL: the take's LIMIT is INSIDE
    // the page wrapper's subquery, not around it.
    let q = f.bind_case(capped).expect("binds");
    let paged = ordo_plan::plan_paged(&q, &f.plan_space(), 2, None).expect("pages");
    let inner = paged.sql.find("LIMIT 2").expect("the take's limit");
    let wrapper = paged.sql.rfind("LIMIT 2").expect("the page's limit");
    assert!(
        inner < wrapper && paged.sql[..inner].contains("SELECT * FROM ("),
        "the take must sit inside the paged subquery — that IS the reason a \
         take cannot window a listing:\n{}",
        paged.sql
    );
}

#[test]
fn the_gate_still_refuses_a_listing_whose_order_is_not_total() {
    let f = Fixture::load().expect("fixture");

    // The ruling admits **keyset paging over the language's guaranteed
    // total order**, and that qualifier is the whole of it. A many-side
    // source join repeats the anchor's id once per joined row, so `id`
    // stops identifying a row and the seek predicate steps over the
    // repeats — the plan is orderable but not TOTALLY orderable, and paging
    // it drops rows without saying so.
    //
    // This is the shape the relaxation must not reach, posed as an authored
    // op because that is who it protects: `list` is single-source and could
    // never write it.
    let refused = "op fanned =\n  customers[name]  orders[total]\n  sort name";
    let prog = ordo_syntax::parse_program(refused).expect("it parses");
    let mut registry = f.registry.clone();
    registry.extend_from(&prog);
    let e = ordo_plan::cost::check_registration(&f.schema, &registry)
        .expect_err("a non-total order is not paging evidence");
    assert_eq!(e.code, "unacknowledged_unbounded", "{e}");

    // …and the acknowledgment still buys it, exactly as before: the gate
    // moved the line, it did not remove it.
    let ok = "op fanned unbounded =\n  customers[name]  orders[total]\n  sort name";
    let prog = ordo_syntax::parse_program(ok).expect("it parses");
    let mut registry = f.registry.clone();
    registry.extend_from(&prog);
    ordo_plan::cost::check_registration(&f.schema, &registry)
        .expect("the human's acknowledgment is still the way through");

    // The predicate directly, on the two shapes, so the reason is pinned to
    // the join and not to something else about the op.
    let fan = ordo_sema::bind_op_generic(&f.schema, &registry, "fanned", ordo_sema::Clock::CORPUS)
        .expect("binds");
    assert!(!ordo_plan::cost::keyset_paged(&fan));
    let listed = ordo_sema::bind_op_generic(
        &f.schema,
        &f.registry,
        "std.tasks.list",
        ordo_sema::Clock::CORPUS,
    )
    .expect("binds");
    assert!(ordo_plan::cost::keyset_paged(&listed));

    // A keyless `reduce` is the other non-paged shape, and it is the one
    // that needs no page: `plan_paged` calls it `unpageable` and the
    // executor answers it whole, which is sound because the prover already
    // proved one row.
    let counted = ordo_sema::bind_op_generic(
        &f.schema,
        &f.registry,
        "std.tasks.count",
        ordo_sema::Clock::CORPUS,
    )
    .expect("binds");
    assert!(!ordo_plan::cost::keyset_paged(&counted));
    assert_eq!(
        ordo_plan::cost::row_bound(&counted, &f.schema),
        ordo_plan::cost::RowBound::AtMost(1)
    );
}

#[test]
fn deriving_list_does_not_loosen_the_pre_reduce_gate_on_count() {
    let f = Fixture::load().expect("fixture");
    let bind = |n: &str| {
        ordo_sema::bind_op_generic(&f.schema, &f.registry, n, ordo_sema::Clock::CORPUS)
            .expect("binds")
    };

    // §2.1 rule 4 is the reason the paging ruling was implemented at the
    // GATE and not in the prover, and this is the assertion that pins it.
    //
    // `std.E.count` refines BEFORE the reduce, over every row the reduce
    // examines — a set with no page window at all. `refine_candidate_bound`
    // asks `row_bound` at that input, and it must still answer `Unbounded`,
    // because that answer is what classifies a sealed refinement on `count`
    // as `unbounded-host` and gets it rejected (`refinement_unbounded`)
    // instead of served. Had `row_bound` learned to accept paging, this
    // would now read `AtMost` — count's input is a plain ordered listing —
    // and the binary-search oracle rule 3 closes would be reachable again.
    for e in ["tasks", "orders", "customers"] {
        assert_eq!(
            ordo_plan::cost::refine_candidate_bound(&bind(&format!("std.{e}.count")), &f.schema),
            ordo_plan::cost::RowBound::Unbounded,
            "std.{e}.count's PRE-REDUCE set must stay unbounded"
        );
        // The same for `list`, which matters because §2.1 has `count`
        // borrow `list`'s manifest: if that borrowing is ever built, the
        // field classes it carries across must be no weaker than count's.
        assert_eq!(
            ordo_plan::cost::refine_candidate_bound(&bind(&format!("std.{e}.list")), &f.schema),
            ordo_plan::cost::RowBound::Unbounded,
            "std.{e}.list's refinement candidate set must stay unbounded"
        );
    }

    // And the pairing itself, at the grain this test owns: `list` and
    // `count` describe the same set for the same caller, asserted with rows.
    // The manifest half (§2.1's "count validates against list's manifest")
    // is built in the door (`ordo-host`'s `count_fields`, 2026-10-05) and
    // proved under a refinement by `refine_paging.rs`; here, `list`
    // arriving must not disturb the agreement.
    let pg = list_reads(&f);
    for who in [ANA, BRAM, CLEO, DOV, None] {
        let rows = f
            .run_case_as(&pg, "std.tasks.list", &[], who)
            .expect("lists")
            .as_array()
            .expect("rows")
            .len() as i64;
        let n = f
            .run_case_as(&pg, "std.tasks.count", &[], who)
            .expect("counts")[0]["n"]
            .as_i64()
            .expect("an integer");
        assert_eq!(rows, n, "list and count disagree for {who:?}");
    }
}
