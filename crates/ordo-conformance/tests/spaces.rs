//! 9h — **the isolation suite**: two spaces, the same ids, one database.
//!
//! Every other suite runs in one space, which is exactly the shape that
//! cannot detect a missing space predicate: a query that reads every
//! tenant's rows and a query that reads one tenant's rows return the same
//! answer when there is only one tenant. So this suite seeds the fixture
//! **twice** — into `conformance` and into `shadow` — with the SAME row
//! ids in both, and then makes the two differ in ways a leak would show:
//!
//! | dimension        | how the twin differs                                  |
//! |------------------|-------------------------------------------------------|
//! | reads            | every product is renamed `twin-…`                      |
//! | guards           | the twin's `boardsAccess` rows are gone                |
//! | tree walks       | `cat_3` is a ROOT in the twin, a grandchild in the real |
//! | writes           | each write runs in one space and is checked in both    |
//! | uniqueness       | the seed itself proves it (below)                      |
//!
//! **The seed succeeding at all is the first result.** `categories.name`
//! is a plain `unique` field and the twin seeds the same five names; a
//! deployment-wide unique index refuses the second space outright. So does
//! the primary key, which would see `cat_1` twice.
//!
//! Ids are deliberately identical across the two spaces. Real typed ids do
//! not collide, and that is precisely why an id-only test proves nothing:
//! a query missing its space predicate still finds the right row by id and
//! looks correct forever, until two tenants share a row id for any reason
//! at all — a restore, an import, a fixture, a migration. Isolation must
//! not rest on ids being lucky.

use ordo_conformance::{Fixture, Pg, WriteLog};
use serde_json::json;

const ANA: Option<&str> = Some("mem_1");
const TWIN: &str = "shadow";

fn pg() -> Pg {
    Pg {
        db: "ordo_spaces".into(),
        ..Pg::default()
    }
}

/// The two fixtures and their shared database, seeded and skewed.
fn setup() -> (Fixture, Fixture, Pg) {
    let real = Fixture::load().expect("fixture");
    let twin = real.in_space(TWIN);
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_spaces");
    let pg = pg();

    // One schema, two tenants. `reset` builds the tables and seeds the
    // real space; `seed_into` adds the twin's rows to the same tables.
    real.reset(&pg).expect("the real space seeds");
    twin.seed_into(&pg).expect("the twin seeds beside it");

    // ── skew the twin, so a leak has something to leak ──────────────────
    // Reads: every product is renamed. A read that crossed would return
    // twelve products, or the wrong six.
    pg.exec(&format!(
        r#"UPDATE "products" SET "name" = 'twin-' || "name" WHERE "$space" = '{TWIN}'"#
    ))
    .expect("rename the twin's products");
    // Guards: the twin grants nobody anything on its boards. The acl guard
    // is an EXISTS over `boardsAccess`, and the real space's rows carry the
    // same member ids — so an unscoped sub-EXISTS authorises the twin's
    // boards with the real space's grants.
    pg.exec(&format!(
        r#"DELETE FROM "boardsAccess" WHERE "$space" = '{TWIN}'"#
    ))
    .expect("strip the twin's grants");
    // Tree: in the twin, `cat_3` is a root. In the real space it sits two
    // levels under `cat_1`. A `$path` walk that read both spaces' rows
    // would find the real space's `cat_1/cat_2/cat_3/` and place the twin's
    // `cat_3` back under `cat_1`.
    pg.exec(&format!(
        r#"UPDATE "categories" SET "parent" = NULL, "depth" = 0, "$path" = 'cat_3/'
           WHERE "$space" = '{TWIN}' AND "id" = 'cat_3'"#
    ))
    .expect("reroot the twin's cat_3");

    (real, twin, pg)
}

/// The suite is ONE test function, deliberately.
///
/// Two spaces share one database and the write checks build on each
/// other's state, so running them as separate `#[test]`s would have them
/// racing to rebuild the same schema — and a suite whose failures depend
/// on thread scheduling proves nothing about isolation.
#[test]
fn spaces_are_isolated() {
    let (real, twin, pg) = setup();
    reads_see_only_their_own_space(&real, &twin, &pg);
    a_dot_hop_scopes_the_table_it_joins(&real, &twin, &pg);
    a_guard_proves_itself_inside_one_space(&real, &twin, &pg);
    a_tree_walk_does_not_climb_into_another_space(&real, &twin, &pg);
    a_whole_document_is_assembled_from_one_space(&real, &twin, &pg);
    pages_are_drawn_from_one_space(&real, &pg);
    // Writes from here down; each mutates the database the next reads.
    an_addressed_write_lands_in_one_space(&twin, &pg);
    a_create_stamps_the_space_it_ran_in(&twin, &pg);
    a_bulk_write_ranges_over_one_space(&real, &twin, &pg);
    a_subtree_cascade_stops_at_the_space_boundary(&twin, &pg);
    a_unique_key_is_claimable_once_per_space(&real, &twin, &pg);
    a_ref_cannot_cross_a_space(&real, &pg);
}

/// Reads see one space. Both directions, because "the twin sees nothing"
/// and "each sees only its own" are different claims and only the second
/// one is isolation.
fn reads_see_only_their_own_space(real: &Fixture, twin: &Fixture, pg: &Pg) {
    let src = "products[name]\nsort name";

    let mine = real.run_case(pg, src).expect("the real space reads");
    let theirs = twin.run_case(pg, src).expect("the twin reads");
    assert_eq!(
        mine,
        json!([
            { "name": "consult" }, { "name": "drill" }, { "name": "hammer" },
            { "name": "saw" }, { "name": "tune-up" }, { "name": "wrench" },
        ])
    );
    assert_eq!(
        theirs,
        json!([
            { "name": "twin-consult" }, { "name": "twin-drill" }, { "name": "twin-hammer" },
            { "name": "twin-saw" }, { "name": "twin-tune-up" }, { "name": "twin-wrench" },
        ])
    );

    // And the same statement WITHOUT its predicate answers with both — so
    // the predicate is what is doing the work, not the seed's shape.
    let plan = real.compile(src).expect("plans");
    let leaky = plan
        .sql
        .replace("WHERE \"t0\".\"$space\" = 'conformance'\n  AND", "WHERE")
        .replace("WHERE \"t0\".\"$space\" = 'conformance'\n", "");
    assert_ne!(leaky, plan.sql, "the plan must carry the predicate");
    let rows = pg.query(&leaky).expect("the unscoped statement runs");
    assert_eq!(
        rows.len(),
        12,
        "an unscoped read returns BOTH tenants — which is the whole hazard:\n{leaky}"
    );
}

/// A joined read (`customer.name` through a ref) scopes the join target
/// too. The ON clause carries it, not the WHERE: this join is LEFT so a
/// nullable ref keeps its row, and a WHERE predicate on the right-hand
/// alias would discard exactly the rows the outer join preserved.
fn a_dot_hop_scopes_the_table_it_joins(real: &Fixture, twin: &Fixture, pg: &Pg) {
    pg.exec(&format!(
        r#"UPDATE "customers" SET "name" = 'twin-' || "name" WHERE "$space" = '{TWIN}'"#
    ))
    .expect("rename the twin's customers");
    let src = "orders[total, customer.name]\nsort total\ntake 1";
    assert_eq!(
        real.run_case(pg, src).expect("reads"),
        json!([{ "total": 1500, "name": "Ada" }])
    );
    assert_eq!(
        twin.run_case(pg, src).expect("reads"),
        json!([{ "total": 1500, "name": "twin-Ada" }])
    );
}

/// **Guards.** The sub-EXISTS a guard compiles to is a source like any
/// other and carries its own predicate. Ana holds `admin` on `board_1` in
/// the real space and nothing in the twin — and `mem_1` names a row in
/// both, so an unscoped guard would hand her the twin's boards on the
/// strength of a grant she holds somewhere else entirely.
fn a_guard_proves_itself_inside_one_space(real: &Fixture, twin: &Fixture, pg: &Pg) {
    assert_eq!(
        real.run_case_as(pg, "boardList", &[], ANA).expect("reads"),
        json!([{ "name": "roadmap" }]),
        "Ana's grant is real in her own space"
    );
    assert_eq!(
        twin.run_case_as(pg, "boardList", &[], ANA).expect("reads"),
        json!([]),
        "and buys her nothing in the twin, where she was granted nothing"
    );
    // The nested case: `cards` inherits the board's acl guard, so this is
    // an EXISTS inside an EXISTS and BOTH levels need scoping.
    assert_eq!(
        twin.run_case_as(pg, "cardList", &[], ANA).expect("reads"),
        json!([]),
        "a nested guard scopes at every level, not only the outermost"
    );
    assert_eq!(
        real.run_case_as(pg, "cardList", &[], ANA)
            .expect("reads")
            .as_array()
            .map(|a| a.len()),
        Some(2)
    );
}

/// **Tree walks.** `under` is a `$path` prefix test over a self-join, and
/// both ends of that join are base tables. The twin's `cat_3` is a root,
/// so nothing under `cat_1` reaches it — unless the walk reads the real
/// space's `cat_3`, whose path still says `cat_1/cat_2/cat_3/`.
fn a_tree_walk_does_not_climb_into_another_space(real: &Fixture, twin: &Fixture, pg: &Pg) {
    let src = r#"toolDeals($cap: 100000, $root: "cat_1")"#;
    let mine = real.run_case(pg, src).expect("reads");
    assert_eq!(
        mine,
        json!([
            { "name": "hammer", "price": 1500 },
            { "name": "saw", "price": 3000 },
            { "name": "drill", "price": 9500 },
        ]),
        "cat_2 and cat_3 are both under cat_1 here"
    );
    let theirs = twin.run_case(pg, src).expect("reads");
    assert_eq!(
        theirs,
        json!([{ "name": "twin-drill", "price": 9500 }]),
        "in the twin only cat_2 is under cat_1 — a walk that found the real \
         space's cat_3 would drag hammer, saw and wrench back in"
    );
}

/// **Writes: addressed.** The same id names a row in both spaces, so an
/// unscoped `UPDATE … WHERE id = …` writes both. The command log's space
/// follows the write, not a constant.
fn an_addressed_write_lands_in_one_space(twin: &Fixture, pg: &Pg) {
    let mut log = WriteLog::new();
    let r = twin
        .run_write_as(
            pg,
            &mut log,
            r#"renameTask($task: "task_1", $title: "twin rename")"#,
            ANA,
        )
        .expect("Ana renames in the twin");
    assert_eq!(r, json!([{ "entity": "tasks", "updated": 1 }]));

    let titles = pg
        .query(r#"SELECT "$space", "title", "rev" FROM "tasks" WHERE "id" = 'task_1' ORDER BY "$space""#)
        .expect("reads both");
    assert_eq!(
        titles,
        vec![
            vec![
                Some("conformance".into()),
                Some("draft spec".into()),
                Some("1".into())
            ],
            vec![
                Some(TWIN.into()),
                Some("twin rename".into()),
                Some("2".into())
            ],
        ],
        "one row changed, one row did not, and they share an id"
    );
    // The group this write logged joined the TWIN's chain. The chain is a
    // per-space ordering point, so a write attributed to a constant space
    // would put one tenant's history in another tenant's audit spine —
    // and `verify(space)` would then be verifying a sequence that never
    // happened there.
    assert_eq!(
        log.len_in(&ordo_commandlog::SpaceId(TWIN.into())),
        1,
        "the twin's chain grew"
    );
    assert_eq!(
        log.len_in(&ordo_commandlog::SpaceId("conformance".into())),
        0,
        "and the real space's did not"
    );
}

/// **Writes: create.** A create stamps the space it ran in — there is no
/// field an author could assign to choose a different one.
fn a_create_stamps_the_space_it_ran_in(twin: &Fixture, pg: &Pg) {
    let mut log = WriteLog::new();
    twin.run_write_as(
        pg,
        &mut log,
        r#"addTask($project: "proj_1", $title: "twin work")"#,
        ANA,
    )
    .expect("Ana creates in the twin");
    let rows = pg
        .query(r#"SELECT "$space" FROM "tasks" WHERE "title" = 'twin work'"#)
        .expect("reads");
    assert_eq!(
        rows,
        vec![vec![Some(TWIN.into())]],
        "exactly one row, in the twin"
    );
}

/// **Writes: bulk.** A ranged delete selects through the read binder and
/// the write guard, both of which are scoped — so the range stops at the
/// space boundary and the count says so.
fn a_bulk_write_ranges_over_one_space(real: &Fixture, twin: &Fixture, pg: &Pg) {
    let mut log = WriteLog::new();
    let r = twin
        .run_write_as(pg, &mut log, r#"purgeDone($project: "proj_1")"#, ANA)
        .expect("Ana purges in the twin");
    assert_eq!(
        r,
        json!([{ "entity": "tasks", "deleted": 1 }]),
        "one done task in proj_1 per space, not two"
    );
    assert_eq!(
        real.run_case_as(pg, "allTasks", &[], ANA).expect("reads"),
        json!([
            { "title": "audit books", "status": "open" },
            { "title": "build site", "status": "open" },
            { "title": "draft spec", "status": "open" },
            { "title": "review spec", "status": "done" },
            { "title": "ship site", "status": "open" },
        ]),
        "the real space keeps its done task"
    );
}

/// **Writes: the tree's `$path` prefix walk.** A subtree cascade selects
/// its victims by prefix and rewrites paths by prefix; both are the
/// statements most likely to reach across, because a path is not an id and
/// two spaces' paths are byte-identical here.
fn a_subtree_cascade_stops_at_the_space_boundary(twin: &Fixture, pg: &Pg) {
    let mut log = WriteLog::new();
    let r = twin
        .run_write_as(pg, &mut log, r#"dropFolderTree($f: "fold_5")"#, ANA)
        .expect("Ana cascades in the twin");
    assert_eq!(
        r,
        json!([{ "entity": "folders", "deleted": 3 }]),
        "three folders under fold_5 per space, not six"
    );
    let left = pg
        .query(r#"SELECT "id" FROM "folders" WHERE "$space" = 'conformance' ORDER BY "id""#)
        .expect("reads");
    assert_eq!(left.len(), 7, "the real space's tree is untouched");
    // And a reparent's prefix substitution does not rewrite the other
    // space's paths, which is the same hazard with an UPDATE behind it.
    twin.run_write_as(
        pg,
        &mut log,
        r#"moveCategory($cat: "cat_2", $parent: "cat_4")"#,
        ANA,
    )
    .expect("Ana moves a category in the twin");
    let paths = pg
        .query(
            r#"SELECT "$space", "$path" FROM "categories" WHERE "id" = 'cat_2' ORDER BY "$space""#,
        )
        .expect("reads");
    assert_eq!(
        paths,
        vec![
            vec![Some("conformance".into()), Some("cat_1/cat_2/".into())],
            vec![Some(TWIN.into()), Some("cat_4/cat_2/".into())],
        ]
    );
    // The real space's descendant kept its path too — the substitution is
    // a prefix rewrite over every row it matches, so an unscoped one
    // rewrites the neighbour's whole subtree.
    let child = pg
        .query(
            r#"SELECT "$path" FROM "categories" WHERE "$space" = 'conformance' AND "id" = 'cat_3'"#,
        )
        .expect("reads");
    assert_eq!(child, vec![vec![Some("cat_1/cat_2/cat_3/".into())]]);
}

/// **Uniqueness.** Every generated unique index leads with the space, so
/// one key is claimable once PER SPACE and addresses one row WITHIN a
/// space.
///
/// A unique field is an ADDRESS (§6 rule 8), so the observable is not a
/// `duplicate` error — it is which row the statement lands on. That makes
/// the test sharper than an error check: a repeat in the same space
/// converges onto the row already holding the key, and the same statement
/// in the twin CREATES, because the key was never taken there. A
/// deployment-wide index would make the twin converge onto the real
/// space's row instead, which is a cross-tenant write of the worst kind —
/// silent, and reported as success.
fn a_unique_key_is_claimable_once_per_space(real: &Fixture, twin: &Fixture, pg: &Pg) {
    let mut log = WriteLog::new();

    // Plain unique (`categories.name`).
    let a = real
        .run_write_as(pg, &mut log, r#"addRootCategory($name: "shared")"#, ANA)
        .expect("the real space claims `shared`");
    let created = a[0]["created"].as_str().expect("a create").to_string();
    let b = twin
        .run_write_as(pg, &mut log, r#"addRootCategory($name: "shared")"#, ANA)
        .expect("and the twin claims it too");
    let twin_created = b[0]["created"]
        .as_str()
        .expect("the twin CREATES rather than converging — the key was free there")
        .to_string();
    assert_ne!(created, twin_created, "two rows, one key, two spaces");
    let again = real
        .run_write_as(pg, &mut log, r#"addRootCategory($name: "shared")"#, ANA)
        .expect("a repeat converges");
    assert_eq!(
        again,
        json!([{ "entity": "categories", "converged": created }]),
        "within a space the key addresses the row that already holds it"
    );
    // Two rows named `shared`, one per space, and no more.
    let n = pg
        .query(r#"SELECT "$space" FROM "categories" WHERE "name" = 'shared' ORDER BY "$space""#)
        .expect("reads");
    assert_eq!(n.len(), 2, "{n:?}");

    // Sealed unique, through the blind index: `UNIQUE (space, hmac)`.
    // The digests already differ between spaces (the index key derives
    // under the space's own SMK), so the column is belt over braces — and
    // it is the half that survives a provider that stops salting.
    let a = real
        .run_write_as(
            pg,
            &mut log,
            r#"addCustomer($name: "Zed", $email: "zed@example.com")"#,
            ANA,
        )
        .expect("the real space registers zed");
    let zed = a[0]["created"].as_str().expect("a create").to_string();
    let b = twin
        .run_write_as(
            pg,
            &mut log,
            r#"addCustomer($name: "Zed", $email: "zed@example.com")"#,
            ANA,
        )
        .expect("and the twin registers the same address");
    assert_ne!(
        b[0]["created"].as_str().expect("the twin creates too"),
        zed,
        "the same address is a different row in a different space"
    );

    // A blind-index READ finds its own space's row and only that one. This
    // is the sealed path's equivalent of the plain read test: the digest
    // column is compared, and the space predicate rides beside it.
    assert_eq!(
        real.run_case_params(
            pg,
            "customerByEmail($e: $e)",
            &[("e", "\"zed@example.com\"")]
        )
        .expect("reads"),
        json!([{ "name": "Zed", "tier": "basic" }])
    );
    assert_eq!(
        twin.run_case_params(
            pg,
            "customerByEmail($e: $e)",
            &[("e", "\"zed@example.com\"")]
        )
        .expect("reads")
        .as_array()
        .map(|a| a.len()),
        Some(1),
        "one row each, never both"
    );
}

/// **The foreign key.** A cross-space ref is not "never written" — it is
/// refused by the database. This is the one property that survives a bug
/// anywhere above the storage layer, which is why the primary key is
/// `(space, id)` and every ref is composite.
fn a_ref_cannot_cross_a_space(real: &Fixture, pg: &Pg) {
    let mut log = WriteLog::new();
    // A project that exists in the real space and nowhere else.
    pg.exec(
        r#"INSERT INTO "projects" ("$space", "id", "name", "owner", "rev")
           VALUES ('conformance', 'proj_only', 'Solo', 'mem_1', 1)"#,
    )
    .expect("a real-space-only project");

    // Hand-written, because the platform above would refuse it first with
    // `not_found` — and that refusal is the one this test is NOT about.
    // What is being proved is the floor underneath it.
    let err = pg
        .exec(&format!(
            r#"INSERT INTO "tasks" ("$space", "id", "project", "title", "status", "rev")
               VALUES ('{TWIN}', 'task_x', 'proj_only', 'reach across', 'open', 1)"#
        ))
        .expect_err("the database refuses a cross-space reference");
    assert!(
        err.contains("foreign key") || err.contains("violates"),
        "{err}"
    );

    // The platform's own answer, for completeness: an invisible parent is
    // `not_found` (G12), never a leak of its existence.
    let e = real
        .in_space(TWIN)
        .run_write_as(
            pg,
            &mut log,
            r#"addTask($project: "proj_only", $title: "reach across")"#,
            ANA,
        )
        .expect_err("the write path refuses first");
    assert!(e.contains("not_found"), "{e}");
}

/// **`whole`.** The anchored document read builds its SELECT by hand — one
/// nested select per signpost and per owned child — so it is the statement
/// with the most sources and the most places to forget one.
fn a_whole_document_is_assembled_from_one_space(real: &Fixture, twin: &Fixture, pg: &Pg) {
    let real_doc = real
        .run_whole_as(pg, r#"whole orders("ord_1")"#, ANA)
        .expect("reads");
    let twin_doc = twin
        .run_whole_as(pg, r#"whole orders("ord_1")"#, ANA)
        .expect("reads");
    // `orderItems` are owned by `orders`; the twin's items point at the
    // twin's products, whose names are all `twin-…`. A signpost that read
    // across would resolve to the wrong space's row.
    let names = |d: &serde_json::Value| -> Vec<String> {
        d["orderItems"]
            .as_array()
            .expect("owned closure")
            .iter()
            .map(|i| i["product"]["name"].as_str().unwrap_or("?").to_string())
            .collect()
    };
    assert_eq!(names(&real_doc), vec!["hammer", "saw"]);
    assert_eq!(names(&twin_doc), vec!["twin-hammer", "twin-saw"]);
}

/// **Paging.** The cursor walks a settled, space-scoped result; a page that
/// reached across would show up as a page size the caller never asked for
/// or a row from the wrong tenant.
fn pages_are_drawn_from_one_space(real: &Fixture, pg: &Pg) {
    let q = real.bind_case("products[name]\nsort name").expect("binds");
    let mut cursor: Option<Vec<Option<String>>> = None;
    let mut walked: Vec<String> = Vec::new();
    for _ in 0..10 {
        let paged =
            ordo_plan::plan_paged(&q, &real.plan_space(), 4, cursor.as_deref()).expect("pages");
        let rows = pg.query(&paged.sql).expect("page runs");
        if rows.is_empty() {
            break;
        }
        let last = rows.last().expect("a row").clone();
        for r in &rows {
            walked.push(r[0].clone().unwrap_or_default());
        }
        if rows.len() < 4 {
            break;
        }
        cursor = Some(last[paged.outputs.len() - paged.key_count..].to_vec());
    }
    assert_eq!(
        walked,
        vec!["consult", "drill", "hammer", "saw", "tune-up", "wrench"],
        "six products, not twelve, and none of them the twin's"
    );
}
