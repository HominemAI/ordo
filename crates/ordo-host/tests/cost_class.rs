//! The **effective cost class per request** — `api-engine-design.md` §7.1
//! and `std-library.md` §2.1 rule 4, both `[settled]` — proved through the
//! real compiler, the real door, and real Postgres.
//!
//! > A registered cost class describes the op **as written**. A refinement
//! > changes the work the op actually does — a `where` on a sealed field
//! > turns a pushed-down `sql` op into a per-row host decrypt. So the
//! > registered class is not the served class, and nothing recomputed it.
//!
//! `ordo-plan`'s `refine_cost.rs` proves the arithmetic and `ordo-api`'s
//! `api.rs` proves the door's answers over hand-written specs. What is left,
//! and what this suite is for, is the join between them: that the facts the
//! gate decides on are the **compiler's**, computed from each op's own plan
//! by the same functions the registration gates use, and that they survive
//! into a deployment that boots against a database.
//!
//! Its own databases, prefixed `ordo_cost_`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{
    router, ApiState, CostClass, MapRegistry, MemoryBus, MemoryReplayCache, OpSpec, PageKey,
    RefineTier,
};
use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use ordo_host::{build_specs, FixtureExecutor, MemberAccess, DEFAULT_SPACE};
use serde_json::{json, Value};
use tower::util::ServiceExt;

/// This suite's own ops, over an entity of its own.
///
/// `ledgers.memo` is the shape the whole gate turns on: **sealed, refinable,
/// and not indexed**. Sealed, so a condition on it is answered by opening
/// payloads rather than by the database. Refinable, because §2.1 rule 1
/// makes that an authored opt-in and without it the door refuses the
/// condition before cost is ever priced. Not indexed, because a
/// `sealed indexed` equality rides the digest and costs `blind-index` — the
/// cheap, servable case.
///
/// `ledgerCount` is §7.1's sharp shape: a keyless reduce. It returns one
/// row, so the row-bound prover proves `<= 1` and it registers with no
/// `unbounded` acknowledgment — and its refinement, being pre-reduce, is
/// applied to every ledger row in the space.
const OPS: &str = r#"
entity ledgers {
  who:    text
  memo:   text = ""   sealed refinable
  amount: int
}

op addLedger($who: text, $memo: text, $amount: int) =
  set ledgers { who: $who, memo: $memo, amount: $amount }

op ledgerCount =
  ledgers[amount]
  reduce { n: count }

op ledgerCountAcked unbounded =
  ledgers[amount]
  reduce { n: count }

op ledgerLines =
  ledgers[who, memo]
  sort who
  take 3
"#;

fn fixture() -> Arc<Fixture> {
    Arc::new(
        Fixture::from_source(&format!("{FIXTURE}\n{OPS}"), SEED)
            .expect("this suite's ops register"),
    )
}

// ── the producer: every fact the gate reads is the compiler's ────────────

/// The op-level facts, off the shipped fixture's own ops.
///
/// Before this, `build_specs` gave every op `OpCost::default()`-shaped
/// silence and every output field `FieldSpec::new` — open, refinable, `sql`.
/// The door therefore could not tell a sealed field from an open one on any
/// spec the platform produced, which made `std-library.md` §2.1 rules 1–4
/// enforceable only against hand-written specs. These are the same numbers
/// the artifact prints and the venue gate enforced.
#[test]
fn derived_specs_carry_the_compilers_cost_facts() {
    let f = fixture();
    let specs = build_specs(&f);
    let spec = |name: &str| -> OpSpec {
        specs
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("`{name}` is registered"))
            .clone()
    };

    // A plain SQL read: cheapest class, and its bound is `take 3`.
    let top = spec("top3Orders");
    assert_eq!(top.cost.class, CostClass::Sql);
    assert!(top.cost.refine_candidate_bounded);
    assert!(!top.cost.unbounded_ack);
    assert!(
        top.output_fields.iter().all(|fld| !fld.sealed),
        "no sealed field in this manifest"
    );

    // `memoDigest` projects `orders.memo`, so it opens sealed payloads —
    // host work, bounded by its own `take 3`. This is the venue gate's own
    // fixture case, read back through the door's spec.
    let digest = spec("memoDigest");
    assert_eq!(digest.cost.class, CostClass::BoundedHost);
    assert!(digest.cost.refine_candidate_bounded);
    let memo = digest
        .output_fields
        .iter()
        .find(|fld| fld.name == "memo")
        .expect("memo is in the manifest");
    assert!(memo.sealed, "sealed is read off the plan's host_open list");
    assert!(
        !memo.refinable,
        "market's `orders.memo` never opted in, so no condition on it is legal"
    );
    assert_eq!(
        memo.refine_class,
        CostClass::BoundedHost,
        "post-take, a condition here opens at most the op's own rows"
    );

    // The same projection with no `take`: served as a keyset page, it opens
    // one page's payloads per request, so it is `bounded-host` (ruling
    // 2026-09-05; std-library.md §5 rules 6 and 7). Its acknowledgment is
    // the author's and still rides; the candidate set a pre-reduce
    // condition would see is still unbounded.
    let memos = spec("orderMemos");
    assert_eq!(memos.cost.class, CostClass::BoundedHost);
    assert!(memos.cost.unbounded_ack);
    assert!(!memos.cost.refine_candidate_bounded);

    // §7.1's shape. One row out — and the set a pre-reduce condition would
    // be applied to is every ledger in the space, with no acknowledgment
    // anywhere in sight.
    let count = spec("ledgerCount");
    assert_eq!(count.cost.class, CostClass::Sql);
    assert!(!count.cost.unbounded_ack);
    assert!(
        !count.cost.refine_candidate_bounded,
        "the reduce returns one row and EXAMINES the space; the gate must see the second"
    );
    assert!(spec("ledgerCountAcked").cost.unbounded_ack);
}

/// The declaration-time opt-in reaches the door, per field and per op.
///
/// `ledgers.memo` said `refinable`; `orders.memo` did not. Both are sealed,
/// and the difference is the author's, which is the whole of rule 1: the
/// capability is a disclosure, so it is authored, and it lands in the
/// manifest and therefore on the consent page.
#[test]
fn sealed_refinability_reaches_the_manifest_per_field() {
    let f = fixture();
    let specs = build_specs(&f);
    let field = |op: &str, name: &str| {
        specs
            .iter()
            .find(|s| s.name == op)
            .and_then(|s| s.output_fields.iter().find(|fld| fld.name == name))
            .unwrap_or_else(|| panic!("`{op}.{name}`"))
            .clone()
    };

    let opted_in = field("ledgerLines", "memo");
    assert!(opted_in.sealed && opted_in.refinable);
    assert_eq!(
        opted_in.refine_class,
        CostClass::BoundedHost,
        "`take 3` bounds what a post-take condition opens"
    );

    let not_opted_in = field("memoDigest", "memo");
    assert!(not_opted_in.sealed && !not_opted_in.refinable);

    // An open field is open: refinable, `sql`, and nothing about this gate
    // may change that.
    let who = field("ledgerLines", "who");
    assert!(!who.sealed && who.refinable);
    assert_eq!(who.refine_class, CostClass::Sql);
}

/// One cost model, two spellings, and no way for them to drift.
///
/// `ordo-api` depends on no sibling crate, so the class names had to be
/// re-declared there to travel on an `OpSpec`. This walks both inventories
/// and both joins over the whole cross product: same names, same order, same
/// lattice. `ordo_host::cost_class` is the only crossing, and it is total.
#[test]
fn the_doors_cost_lattice_is_the_planners() {
    let plan_all = ordo_plan::cost::CostClass::ALL;
    let api_all = CostClass::ALL;
    assert_eq!(plan_all.len(), api_all.len());

    for (i, &p) in plan_all.iter().enumerate() {
        assert_eq!(
            ordo_host::cost_class(p).as_str(),
            p.as_str(),
            "the crossing renames a class"
        );
        assert_eq!(api_all[i], ordo_host::cost_class(p), "the orders differ");
        assert_eq!(CostClass::from_class(p.as_str()), Some(api_all[i]));

        for &q in plan_all.iter() {
            assert_eq!(
                ordo_host::cost_class(p.join(q)),
                ordo_host::cost_class(p).join(ordo_host::cost_class(q)),
                "`{}` ⊔ `{}` joins differently on the two sides",
                p.as_str(),
                q.as_str()
            );
        }
    }
}

// ── the exploit, through the door, against Postgres ──────────────────────

async fn deployment(db: &str) -> (ApiState, Arc<Fixture>, Vec<OpSpec>) {
    let f = fixture();
    let pg = Pg::named(db);
    f.reset(&pg).expect("DDL applies and the fixture seeds");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool boots");
    let deployment = ordo_host::deploy::Deployment::pinned(
        "cost-class-test",
        f.clone(),
        ordo_exec::Executor {
            pool,
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
    );
    let specs = build_specs(&f);
    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(FixtureExecutor {
            spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
            pg: pg.clone(),
            page_size: 20,
        }),
        access: Arc::new(MemberAccess {
            fixture: f.clone(),
            pg,
        }),
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"ordo-cost-class"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    (state, f, specs)
}

async fn call(
    state: &ApiState,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("x-ordo-space", DEFAULT_SPACE)
        .header("authorization", "Bearer member:mem_1");
    if body.is_some() {
        req = req.header("content-type", "application/json");
    }
    let req = match body {
        Some(v) => req.body(Body::from(v.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = router(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn refine_query(op: &str, refine: Value) -> String {
    let encoded: String = refine
        .to_string()
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect();
    format!("/op/{op}?$refine={encoded}")
}

/// Serve `replacement` in place of the derived spec of the same name,
/// leaving every other derived spec alone. The registry is the door's; the
/// executor still runs the compiler's op of that name, so what changes is
/// only what the DOOR was told about it.
fn install(state: &ApiState, specs: &[OpSpec], replacement: OpSpec) {
    let name = replacement.name.clone();
    state.registry.swap(Arc::new(
        MapRegistry::new(
            specs
                .iter()
                .filter(|s| s.name != name)
                .cloned()
                .chain([replacement]),
        )
        .expect("specs validate"),
    ));
}

/// **§2.1's list/count pairing, assembled the way the specification says it
/// is.**
///
/// > `std.E.count` gets one std-specific pairing: its own output is `{ n }`,
/// > which would make refinement useless under the output-fields-only rule —
/// > so **count's refinement conditions validate against `std.E.list`'s
/// > manifest** and apply before the reduce.
///
/// So the field *names* come from the list op, and the **cost** is the count
/// op's, because it is the count's plan the condition would run under. Both
/// are computed here by the same `ordo_plan::cost` functions the registration
/// gates call — a producer shipping this pairing writes exactly this.
///
/// `aggregate_only` is deliberately left `false`. Rule 3 (*if you may not
/// read the rows, you may not measure them*) would refuse a sealed condition
/// here first, and a test that only ever ran the two rules together would
/// prove nothing about either. §7.1: "implementing only one half leaves the
/// hole open in one direction."
fn paired_count_spec(f: &Fixture, specs: &[OpSpec], count_op: &str) -> OpSpec {
    let list = specs
        .iter()
        .find(|s| s.name == "ledgerLines")
        .expect("the list half");
    let mut count = specs
        .iter()
        .find(|s| s.name == count_op)
        .expect("the count half")
        .clone();

    // The count's own candidate set: what a pre-reduce condition examines.
    let bound =
        ordo_sema::bind_op_generic(&f.schema, &f.registry, count_op, ordo_sema::Clock::CORPUS)
            .expect("the count op binds");
    let candidate = ordo_plan::cost::refine_candidate_bound(&bound, &f.schema);

    count.output_fields = list
        .output_fields
        .iter()
        .cloned()
        .map(|mut fld| {
            let indexed = f
                .schema
                .entity("ledgers")
                .and_then(|e| e.field(&fld.name))
                .is_some_and(|d| d.indexed);
            fld.refine_class = ordo_host::cost_class(ordo_plan::cost::refine_field_class(
                fld.sealed, indexed, true, candidate,
            ));
            fld
        })
        .collect();
    // Every program read is refinable by derivation (owner, 2026-10-05), so
    // the tier is the compiler's like everything else here: the classes, the
    // candidate bound, the acknowledgment, the sealed flags.
    assert_eq!(count.refinable, RefineTier::Basic);
    // See the paragraph above: rule 3 is switched OFF here so rule 4 stands
    // alone. `build_specs` derives it as `true` for any op with a `reduce`
    // step, and the test below asserts that half separately — the point is
    // that neither rule is the other, not that one covers for the other.
    count.aggregate_only = false;
    count
}

/// **The exploit, refused.** A caller with nothing but the right to call a
/// count refines it on a sealed field. Serving that means opening every
/// sealed payload in the space to answer it — `unbounded-host` work under an
/// op registered as `sql` with a proven bound of one row, which is precisely
/// the gate `unacknowledged_unbounded` enforces at registration and which
/// a refinement would otherwise walk around at request time.
///
/// Then the other direction, which is half the proof: the same envelope on
/// an **open** field is served, and the rows come back from Postgres.
#[tokio::test]
async fn a_sealed_refinement_cannot_walk_around_the_unbounded_gate() {
    let (state, f, specs) = deployment("ordo_cost_gate").await;
    let paired = paired_count_spec(&f, &specs, "ledgerCount");

    // Real rows, written through the real write path — `memo` is sealed on
    // the way in, so what Postgres holds is an envelope.
    for (who, memo, amount) in [
        ("ana", "gift", 10),
        ("bram", "invoice", 20),
        ("cass", "gift", 30),
    ] {
        let (status, body) = call(
            &state,
            "POST",
            "/op/addLedger",
            Some(json!({"who": who, "memo": memo, "amount": amount})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    // First, the half this suite is NOT about, so that the half it is about
    // cannot be confused for it: with the aggregate flag the compiler
    // actually derives for a `reduce`, the DECLARATION-time rule answers —
    // *if you may not read the rows, you may not measure them* — and the
    // request never reaches the cost gate. Both halves are live; they are
    // different refusals for different reasons.
    let sealed = json!({"where": [{"field": "memo", "op": "eq", "value": "gift"}]});
    let mut derived = paired.clone();
    derived.aggregate_only = true;
    install(&state, &specs, derived);
    let (status, body) = call(
        &state,
        "GET",
        &refine_query("ledgerCount", sealed.clone()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_refinement");

    // The escalation, with rule 3 off duty and rule 4 alone on the door.
    install(&state, &specs, paired);
    let (status, body) = call(&state, "GET", &refine_query("ledgerCount", sealed), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "refinement_unbounded");
    assert_eq!(body["error"]["param"], "memo");
    assert_eq!(body["error"]["op"], "ledgerCount");

    // An OPEN condition is served, and the number is Postgres's over the
    // filtered set. `std.ledgers.count` is the pairing `build_specs` now
    // derives itself (`std-library.md` §2.1, ruled this round): its
    // conditions name `std.ledgers.list`'s fields and apply before the
    // count. A cost gate that refused this would have broken every refining
    // caller, and would have looked like a security feature doing its job.
    let open = json!({"where": [{"field": "who", "op": "eq", "value": "ana"}]});
    let (status, body) = call(
        &state,
        "GET",
        &refine_query("std.ledgers.count", open),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["rows"][0]["n"], 1,
        "one of the three rows this test wrote is ana's, and the executor \
         applies `$refine` since 2026-10-05"
    );
    let unrefined = json!({"where": []});
    let (status, body) = call(
        &state,
        "GET",
        &refine_query("std.ledgers.count", unrefined),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["n"], 3);

    // A `pick` adds no work: it removes columns from a result the op
    // already produces. On a count, the one output is `n`.
    let picked = json!({"pick": ["n"]});
    let (status, body) = call(
        &state,
        "GET",
        &refine_query("std.ledgers.count", picked),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["n"], 3);
}

/// The acknowledgment is the exemption, and it is a human's sentence: *this
/// op's work grows with the size of the space, and I mean it.* The identical
/// request that `ledgerCount` refuses, `ledgerCountAcked` serves — same
/// pipeline, same sealed condition, one word of difference in the program.
///
/// This is what keeps the refusal honest rather than absolute: §7.1 rejects
/// the op that "quietly became unbounded at request time", not the op whose
/// author said so at registration.
#[tokio::test]
async fn the_acknowledgment_is_the_only_difference_between_refusal_and_service() {
    let (state, f, specs) = deployment("ordo_cost_ack").await;
    let paired = paired_count_spec(&f, &specs, "ledgerCountAcked");
    assert!(paired.cost.unbounded_ack);
    install(&state, &specs, paired);

    let sealed = json!({"where": [{"field": "memo", "op": "eq", "value": "gift"}]});
    let (status, body) = call(
        &state,
        "GET",
        &refine_query("ledgerCountAcked", sealed),
        None,
    )
    .await;
    // The cost gate passes it: the acknowledgment is what changed. The venue
    // then refuses it at the door, naming the field, because host-side
    // filtering is not built (ruled 2026-10-05, `refinement-default` ask 3).
    assert_ne!(body["error"]["code"], "refinement_unbounded", "{body}");
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "sealed_unsupported");
    assert_eq!(body["error"]["param"], "memo");
}

/// §7.1's own carve-out, and the false-positive guard for the sealed case:
///
/// > Because the filter runs after the page window, the *candidate count*
/// > for a row-shaped op is the page, not the entity: refinement raises the
/// > **venue** (`sql` → host-side decrypt of a page) without raising
/// > **cardinality**.
///
/// The identical sealed condition that `ledgerCount` refuses is legal on
/// `ledgerLines`, whose `take 3` bounds what any post-take condition can
/// open. The cost gate lets it through; the door's venue check then refuses
/// it for a different reason: `memo` has no blind index, and host-side
/// filtering is not built, so the condition is `sealed_unsupported` (422),
/// naming the field. The assertion is that the cost gate is not the one
/// refusing. (Returning `memo` is served since 27ao; filtering by it is not.)
#[tokio::test]
async fn a_bounded_sealed_refinement_is_not_the_cost_gates_business() {
    let (state, _f, specs) = deployment("ordo_cost_bounded").await;
    let lines = specs
        .iter()
        .find(|s| s.name == "ledgerLines")
        .expect("registered")
        .clone();
    assert_eq!(lines.cost.class, CostClass::BoundedHost);
    // Every program read is refinable now; nothing is switched on here.
    assert_eq!(lines.refinable, RefineTier::Basic);
    install(&state, &specs, lines);

    let sealed = json!({"where": [{"field": "memo", "op": "eq", "value": "gift"}]});
    let (status, body) = call(&state, "GET", &refine_query("ledgerLines", sealed), None).await;
    assert_ne!(
        body["error"]["code"], "refinement_unbounded",
        "post-take, this condition opens at most the op's own three rows"
    );
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "the door's venue check answers instead: {body}"
    );
    assert_eq!(body["error"]["code"], "sealed_unsupported");
    assert_eq!(
        body["error"]["param"], "memo",
        "the door names the field; the executor's refusal named neither it nor the fix"
    );
}

/// `plan-replica-feed.md` §1 on the door's spec, over the fixture: the spec
/// carries the planner's answer, and each refusal names its rule.
#[test]
fn specs_say_which_ops_can_be_synced_and_which_rule_the_rest_fail() {
    use ordo_api::{NotSyncable, Syncability};
    let f = fixture();
    let specs = build_specs(&f);
    let sync = |name: &str| {
        specs
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("`{name}` is registered"))
            .sync
            .clone()
    };
    assert_eq!(
        sync("std.tasks.list"),
        Syncability::Syncable {
            entity: "tasks".into(),
            id_output: "id".into(),
        }
    );
    let refused = |name: &str, rule: NotSyncable| {
        assert_eq!(sync(name), Syncability::NotSyncable(rule), "{name}");
    };
    // `taskBoard` was the standing example of "cannot sync, the author did not
    // project the id". Since 2026-09-16 the anchor's id is attached instead,
    // so it syncs under `$id`. Asserted rather than dropped: the change is
    // that this STOPPED refusing, and a vanished line would not say so.
    assert_eq!(
        sync("taskBoard"),
        Syncability::Syncable {
            entity: "tasks".into(),
            id_output: ordo_plan::artifact::ROW_KEY.into(),
        }
    );
    refused("spendByCustomer", NotSyncable::Grouped);
    refused("ledgerCount", NotSyncable::Measure);
    refused("addLedger", NotSyncable::NotARead);
}

/// The planner's refusals and the door's are one inventory spelled twice,
/// joined here as `cost_class` is: every planner rule crosses to the door
/// rule with the same spelling.
#[test]
fn sync_refusals_cross_to_the_door_with_one_spelling() {
    use ordo_api::{NotSyncable, Syncability};
    use ordo_plan::artifact::SyncRule;
    let f = fixture();
    let bound = |src: &str| {
        ordo_sema::bind(
            &f.schema,
            &f.registry,
            &ordo_syntax::parse_pipeline(src).expect("parses"),
            ordo_sema::Clock::CORPUS,
        )
        .expect("binds")
    };
    for (src, rule) in [
        (
            "tasks[title, status]\n  group status { n: count }",
            SyncRule::Grouped,
        ),
        ("tasks[title]\n  reduce { n: count }", SyncRule::Measure),
        // Both of these project the id, so they were syncable and wrong
        // until 2026-09-16: the old `id_not_projected` had been refusing them
        // by accident, and the accident lapsed as soon as an author added
        // `id` to the projection.
        ("tasks[id, title]\ndistinct", SyncRule::Distinct),
        ("customers[id, name]  orders[total]", SyncRule::ManySideJoin),
    ] {
        let q = bound(src);
        assert_eq!(ordo_plan::artifact::syncability(&q), Err(rule), "{src}");
        let Syncability::NotSyncable(door) = ordo_host::syncability(&q) else {
            panic!("{src} crossed as syncable");
        };
        assert_eq!(door.as_str(), rule.as_str(), "{src}");
        assert!(!door.rule().is_empty());
    }
    assert_eq!(NotSyncable::NoAnchor.as_str(), SyncRule::NoAnchor.as_str());

    // And the case that STOPPED being a refusal on 2026-09-16: an op that
    // does not project its id is syncable now, keyed by the attached `$id`.
    // Asserted here rather than deleted, because "this no longer refuses" is
    // the change, and a loop that simply lost an entry would not say so.
    let open = bound("tasks[title]");
    assert_eq!(
        ordo_plan::artifact::syncability(&open).map(|s| s.id_output),
        Ok(ordo_plan::artifact::ROW_KEY.to_string())
    );
    assert!(matches!(
        ordo_host::syncability(&open),
        Syncability::Syncable { .. }
    ));
}
