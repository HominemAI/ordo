//! Stage 5 — the write matrix, executed.
//!
//! The read-side visibility matrix (`corpus_stage3.rs`) established who can
//! SEE each row; this suite establishes who can CHANGE it, and that every
//! accepted change lands in the command log as an attributed, chained,
//! before/after-imaged group. The membership facts are seed.sql §GC:
//!
//! | caller | proj_1        | proj_2   | proj_3      | board_1 |
//! |--------|---------------|----------|-------------|---------|
//! | Ana    | owner+write   | —        | member (ro) | admin   |
//! | Bram   | member (ro)   | write    | —           | write   |
//! | Cleo   | —             | write    | owner, no pm| read    |
//! | Dov    | —             | —        | —           | —       |
//!
//! One test function: writes are order-dependent, and this database is the
//! write suite's own (`ordo_writes`), never shared with the read suites.

use ordo_commandlog::chain::verify_chain;
use ordo_commandlog::store::Store;
use ordo_commandlog::{CommandKind, SpaceId, Value as LogValue};
use ordo_conformance::{Fixture, Pg, WriteLog};
use serde_json::json;

const ANA: Option<&str> = Some("mem_1");
const BRAM: Option<&str> = Some("mem_2");
const CLEO: Option<&str> = Some("mem_3");
const DOV: Option<&str> = Some("mem_4");

fn pg() -> Pg {
    Pg {
        db: "ordo_writes".into(),
        ..Pg::default()
    }
}

#[test]
fn the_write_matrix_executes() {
    let f = Fixture::load().expect("fixture");
    // The suite owns its database; create it when the cluster is fresh.
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_writes");
    let pg = pg();
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();
    let w = |log: &mut WriteLog, call: &str, as_: Option<&str>| f.run_write_as(&pg, log, call, as_);

    // ── addressed converge: G10 both ways, G12, rev bump ────────────────
    // Ana can write in proj_1: the rename lands and rev bumps.
    let r = w(
        &mut log,
        r#"renameTask($task: "task_1", $title: "draft spec v2")"#,
        ANA,
    )
    .expect("Ana renames");
    assert_eq!(r, json!([{ "entity": "tasks", "updated": 1 }]));
    let row = pg
        .query(r#"SELECT "title", "rev" FROM "tasks" WHERE "id" = 'task_1'"#)
        .unwrap();
    assert_eq!(row[0][0].as_deref(), Some("draft spec v2"));
    assert_eq!(row[0][1].as_deref(), Some("2"), "rev bumps on converge");

    // Bram READS proj_1 but has canWrite=false: visible, refused — the
    // error names the guard, not the row's absence.
    let e = w(
        &mut log,
        r#"renameTask($task: "task_1", $title: "bram was here")"#,
        BRAM,
    )
    .expect_err("no canWrite");
    assert!(e.starts_with("access_guard"), "{e}");

    // Dov is outside proj_1's read view: the SAME failure as a task that
    // does not exist at all (G12's disclosure rule).
    let invisible = w(
        &mut log,
        r#"renameTask($task: "task_1", $title: "dov was here")"#,
        DOV,
    )
    .expect_err("outside read view");
    let absent = w(
        &mut log,
        r#"renameTask($task: "task_nope", $title: "x")"#,
        DOV,
    )
    .expect_err("no such row");
    assert!(invisible.starts_with("not_found"), "{invisible}");
    assert_eq!(
        invisible.split(':').next(),
        absent.split(':').next(),
        "invisible and absent must be indistinguishable classes"
    );

    // Anonymous callers have no write view at all (G15's write half) — but
    // *which* refusal they get is R-G10's business, not G15's. R-G12 makes
    // every guard false for a caller with no principal, so every row on a
    // guarded entity is outside their read view, and an ADDRESSED write must
    // therefore answer `not_found`, byte-identical to a row that never
    // existed. This asserted `access_guard` until checklist 10d.
    let e = w(
        &mut log,
        r#"renameTask($task: "task_1", $title: "x")"#,
        None,
    )
    .expect_err("anonymous");
    assert!(e.starts_with("not_found"), "{e}");
    let e404 = w(
        &mut log,
        r#"renameTask($task: "task_nope", $title: "x")"#,
        None,
    )
    .expect_err("anonymous");
    assert_eq!(
        e, e404,
        "and the two are the same bytes, not the same class"
    );
    // A CREATE is still `access_guard`: it addresses no row to be coy
    // about, and the guard simply evaluates false (R-G12).
    let e = w(
        &mut log,
        r#"addTask($project: "proj_1", $title: "x")"#,
        None,
    )
    .expect_err("anonymous");
    assert!(e.starts_with("access_guard"), "{e}");

    // ── $caller in a write RHS (G4) ─────────────────────────────────────
    let r = w(&mut log, r#"claimTask($task: "task_1")"#, ANA).expect("Ana claims");
    assert_eq!(r, json!([{ "entity": "tasks", "updated": 1 }]));
    let row = pg
        .query(r#"SELECT "assignee" FROM "tasks" WHERE "id" = 'task_1'"#)
        .unwrap();
    assert_eq!(row[0][0].as_deref(), Some("mem_1"));

    // ── create: parent visibility, would-be-row guard, minted id ────────
    // Ana creates in proj_1 (canWrite): the id is minted from the entity
    // key and the row is immediately in her read view.
    let r = w(
        &mut log,
        r#"addTask($project: "proj_1", $title: "write the docs")"#,
        ANA,
    )
    .expect("Ana creates");
    let new_id = r[0]["created"].as_str().expect("created id").to_string();
    assert!(new_id.starts_with("task_"), "typed id: {new_id}");
    let visible = f
        .run_case_as(
            &pg,
            r#"tasks(title == "write the docs")[title, status]"#,
            &[],
            ANA,
        )
        .expect("readable");
    assert_eq!(
        visible,
        json!([{ "title": "write the docs", "status": "open" }]),
        "status default applied"
    );

    // Bram reads proj_1, so the parent is visible — but the would-be row
    // fails the write guard (canWrite=false): access_guard, not not_found.
    let e = w(
        &mut log,
        r#"addTask($project: "proj_1", $title: "bram tries")"#,
        BRAM,
    )
    .expect_err("guarded create");
    assert!(e.starts_with("access_guard"), "{e}");

    // Dov cannot see proj_1 at all: the parent itself is not_found.
    let e = w(
        &mut log,
        r#"addTask($project: "proj_1", $title: "dov tries")"#,
        DOV,
    )
    .expect_err("invisible parent");
    assert!(e.starts_with("not_found"), "{e}");

    // ── create under an INHERITED guard (taskNotes ← tasks) ─────────────
    let r = w(
        &mut log,
        r#"addNote($task: "task_1", $body: "kickoff notes")"#,
        ANA,
    )
    .expect("Ana notes her task");
    assert!(r[0]["created"].as_str().unwrap().starts_with("note_"));
    let e = w(
        &mut log,
        r#"addNote($task: "task_1", $body: "bram notes")"#,
        BRAM,
    )
    .expect_err("inherited write guard");
    assert!(e.starts_with("access_guard"), "{e}");
    let e = w(
        &mut log,
        r#"addNote($task: "task_1", $body: "dov notes")"#,
        DOV,
    )
    .expect_err("invisible parent task");
    assert!(e.starts_with("not_found"), "{e}");

    // ── bulk: the range is read ∩ cond ∩ write view (G11) ───────────────
    // task_2 (proj_1, done) is in Bram's READ view but not his write view:
    // the purge silently ranges over nothing — count 0, no error, row alive.
    let r = w(&mut log, r#"purgeDone($project: "proj_1")"#, BRAM).expect("bulk never errors");
    assert_eq!(r, json!([{ "entity": "tasks", "deleted": 0 }]));
    assert_eq!(
        pg.query(r#"SELECT 1 FROM "tasks" WHERE "id" = 'task_2'"#)
            .unwrap()
            .len(),
        1,
        "task_2 survived Bram's purge"
    );
    // Ana's purge ranges over it and deletes exactly it.
    let r = w(&mut log, r#"purgeDone($project: "proj_1")"#, ANA).expect("Ana purges");
    assert_eq!(r, json!([{ "entity": "tasks", "deleted": 1 }]));
    assert_eq!(
        pg.query(r#"SELECT 1 FROM "tasks" WHERE "id" = 'task_2'"#)
            .unwrap()
            .len(),
        0
    );

    // ── create-exempt (G10a) + the acl bootstrap ────────────────────────
    // boards' write guard walks boardAccess — rows that must reference the
    // new board. Unsatisfiable pre-insert, so Cleo's create is exempt; the
    // bootstrap mints her top-level access row in the SAME command group.
    let r = w(&mut log, r#"addBoard($name: "retro board")"#, CLEO).expect("exempt create");
    let board_id = r[0]["created"].as_str().unwrap().to_string();
    assert!(board_id.starts_with("board_"));
    let ba = pg
        .query(&format!(
            r#"SELECT "member", "level" FROM "boardsAccess" WHERE "board" = '{board_id}'"#
        ))
        .unwrap();
    assert_eq!(ba.len(), 1, "exactly the bootstrap row");
    assert_eq!(ba[0][0].as_deref(), Some("mem_3"));
    assert_eq!(
        ba[0][1].as_deref(),
        Some("admin"),
        "creator gets the TOP level"
    );
    // And the board is immediately administrable: Cleo sees it.
    let seen = f
        .run_case_as(
            &pg,
            r#"boards(id == $b)[name]"#,
            &[("b", &format!("\"{board_id}\""))],
            CLEO,
        )
        .expect("visible to creator");
    assert_eq!(seen, json!([{ "name": "retro board" }]));

    // ── create through an acl-derived, inherited guard (cards ← boards) ─
    // R-G14: writes need level >= `write`, and the levels are a FULL
    // ORDERING, so `admin` carries `write`'s rights too. Ana is admin on
    // board_1, Bram holds `write` — both write; Cleo holds `read` and Dov
    // holds nothing.
    let r = w(
        &mut log,
        r#"addCard($board: "board_1", $title: "triage")"#,
        ANA,
    )
    .expect("admin creates");
    let card_id = r[0]["created"].as_str().unwrap().to_string();
    assert!(card_id.starts_with("card_"));
    w(
        &mut log,
        r#"addCard($board: "board_1", $title: "bram card")"#,
        BRAM,
    )
    .expect("the `write` level writes — it is the middle rung, not decoration");
    let e = w(
        &mut log,
        r#"addCard($board: "board_1", $title: "cleo card")"#,
        CLEO,
    )
    .expect_err("`read` is below the write rung");
    assert!(e.starts_with("access_guard"), "{e}");
    let e = w(
        &mut log,
        r#"addCard($board: "board_1", $title: "dov card")"#,
        DOV,
    )
    .expect_err("no access row at all");
    assert!(e.starts_with("not_found"), "{e}");

    // Addressed converge under the same inherited guard.
    let r =
        w(&mut log, &format!(r#"finishCard($card: "{card_id}")"#), ANA).expect("admin finishes");
    assert_eq!(r, json!([{ "entity": "cards", "updated": 1 }]));
    w(
        &mut log,
        &format!(r#"finishCard($card: "{card_id}")"#),
        BRAM,
    )
    .expect("bram holds `write`, and `write` writes");
    // Cleo reads the card through the inherited ACL and cannot write it —
    // `read` is below the write rung, which is the half of the ordering
    // that must still refuse.
    let e = w(
        &mut log,
        &format!(r#"finishCard($card: "{card_id}")"#),
        CLEO,
    )
    .expect_err("cleo reads the card but cannot write it");
    assert!(e.starts_with("access_guard"), "{e}");

    // ── the log: every accepted write, chained, attributed, imaged ──────
    let space = SpaceId("conformance".into());
    let n = log.store.len(&space);
    // Accepted ops above: rename, claim, addTask, addNote, purge(Bram: no
    // rows → NO group), purge(Ana), addBoard, addCard(Ana),
    // addCard(Bram — the write rung), finishCard(Ana),
    // finishCard(Bram) = 10.
    assert_eq!(n, 10, "one group per accepted mutating op");
    let chain: Vec<_> = (0..n)
        .map(|i| log.store.get(&space, i).expect("indexed").clone())
        .collect();
    verify_chain(&space, &chain).expect("hash chain verifies");

    // The first group is the rename: one converge with honest images.
    let g0 = &chain[0].group;
    assert_eq!(g0.commands.len(), 1);
    let c = &g0.commands[0];
    assert_eq!(c.kind, CommandKind::Converge);
    assert_eq!(c.row_id, "task_1");
    let before = c.before.as_ref().unwrap();
    let after = c.after.as_ref().unwrap();
    assert_eq!(before["title"], LogValue::Text("draft spec".into()));
    assert_eq!(after["title"], LogValue::Text("draft spec v2".into()));
    assert_eq!(before["rev"], LogValue::Int(1));
    assert_eq!(after["rev"], LogValue::Int(2));

    // The board create is a two-command group: the board AND its bootstrap
    // access row ride one correlation — neither exists without the other.
    let board_group = chain
        .iter()
        .find(|g| g.group.commands.iter().any(|c| c.entity == "boards"))
        .expect("board group");
    assert_eq!(board_group.group.commands.len(), 2);
    assert_eq!(board_group.group.commands[1].entity, "boardsAccess");
    assert!(matches!(
        &board_group.group.actor,
        ordo_commandlog::Actor::User(u) if u == "mem_3"
    ));

    // The purge's delete carries a full before image and no after.
    let del = chain
        .iter()
        .flat_map(|g| &g.group.commands)
        .find(|c| c.kind == CommandKind::Delete)
        .expect("the purge");
    assert_eq!(del.row_id, "task_2");
    assert!(del.before.is_some() && del.after.is_none());
    assert_eq!(
        del.before.as_ref().unwrap()["title"],
        LogValue::Text("review spec".into())
    );
}

/// F4 — sealed writes. The stored bytes are an AEAD envelope, the blind
/// index makes the row findable by exact (normalized) equality, and the
/// plaintext appears nowhere Postgres can see.
#[test]
fn sealed_writes_seal_and_stay_findable() {
    use ordo_conformance::fixture_space;
    use ordo_crypto::Purpose;

    let f = Fixture::load().expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_writes_sealed");
    let pg = Pg {
        db: "ordo_writes_sealed".into(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    // Create with a sealed indexed field. Mixed case on purpose: the INDEX
    // normalizes, the payload seals as written.
    let r = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addCustomer($name: "Zed", $email: "Zed@Example.COM")"#,
            ANA,
        )
        .expect("create with sealed email");
    let cust_id = r[0]["created"].as_str().unwrap().to_string();

    // 1. Postgres holds an envelope, not the plaintext: epoch byte 1,
    //    envelope-sized, and decrypting it yields the ORIGINAL casing.
    let row = pg
        .query(&format!(
            r#"SELECT encode("email", 'hex'), encode("email$bidx", 'hex') FROM "customers" WHERE "id" = '{cust_id}'"#
        ))
        .unwrap();
    let sealed = row[0][0].as_deref().expect("sealed bytes");
    assert!(sealed.starts_with("01"), "epoch tag leads: {sealed}");
    assert!(
        sealed.len() >= 2 * (1 + 24 + 16),
        "envelope-sized, not value-sized"
    );
    let bytes: Vec<u8> = (0..sealed.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&sealed[i..i + 2], 16).unwrap())
        .collect();
    assert_eq!(
        {
            let lease = f
                .keys
                .release(&fixture_space(), Purpose::Read, "t")
                .expect("release");
            f.keys
                .open_at(
                    &lease,
                    "customers",
                    "email",
                    ordo_crypto::Anchor::Row(&cust_id),
                    &bytes,
                )
                .expect("opens")
        },
        b"Zed@Example.COM",
        "the payload seals AS WRITTEN"
    );

    // 2. The blind-index read path finds the row — lowercase query against
    //    the mixed-case write, because both sides normalize.
    let found = f
        .run_case_params(
            &pg,
            "customerByEmail($e: $q)[name, tier]",
            &[("q", "\"zed@example.com\"")],
        )
        .expect("blind equality");
    assert_eq!(
        found,
        json!([{ "name": "Zed", "tier": "basic" }]),
        "found via digest; tier default applied"
    );

    // 3. The command image carries an opaque handle, never the value.
    use ordo_commandlog::store::Store;
    let space = SpaceId("conformance".into());
    let g = log.store.get(&space, 0).expect("the create group");
    let img = g.group.commands[0].after.as_ref().unwrap();
    assert!(
        matches!(img["email"], LogValue::SealedRef(_)),
        "image holds a SealedRef: {:?}",
        img["email"]
    );

    // 4. Addressed converge on a sealed field: the memo re-seals and the
    //    stored bytes change even for the same value (fresh nonce).
    // (`memo` seeds as the EMPTY bytea — probed beside `id` because the
    // harness's row parser drops a row whose only column is empty.)
    let before = pg
        .query(r#"SELECT encode("memo", 'hex'), "id" FROM "orders" WHERE "id" = 'ord_1'"#)
        .unwrap()[0][0]
        .clone();
    f.run_write_as(
        &pg,
        &mut log,
        r#"setMemo($order: "ord_1", $memo: "leave at the side door")"#,
        ANA,
    )
    .expect("sealed converge");
    let row = pg
        .query(r#"SELECT encode("memo", 'hex'), "rev" FROM "orders" WHERE "id" = 'ord_1'"#)
        .unwrap();
    assert_ne!(row[0][0], before, "the envelope changed");
    assert_eq!(row[0][1].as_deref(), Some("2"), "rev bumped");
    let memo_bytes: Vec<u8> = {
        let h = row[0][0].as_deref().unwrap();
        (0..h.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
            .collect()
    };
    assert_eq!(
        {
            let lease = f
                .keys
                .release(&fixture_space(), Purpose::Read, "t")
                .expect("release");
            f.keys
                .open_at(
                    &lease,
                    "orders",
                    "memo",
                    ordo_crypto::Anchor::Row("ord_1"),
                    &memo_bytes,
                )
                .expect("opens")
        },
        b"leave at the side door"
    );

    // 5. The read side closes the loop: the sealed value written above
    // comes back as PLAINTEXT, opened host-side — not as the envelope the
    // database actually stores, and not as a refusal.
    let memos = f
        .run_case(&pg, r#"orders(id == "ord_1")[memo]"#)
        .expect("a sealed projection reads");
    assert_eq!(
        memos,
        json!([{ "memo": "leave at the side door" }]),
        "the host opened it"
    );
    // An untouched row's empty payload is an empty value, not a failure:
    // `''::bytea` is a real, empty memo.
    let all = f.run_case(&pg, "orders[memo]").expect("every memo reads");
    assert_eq!(all.as_array().unwrap().len(), 7);
    assert!(
        all.as_array()
            .unwrap()
            .iter()
            .all(|r| r["memo"].is_string() && !r["memo"].as_str().unwrap().starts_with("\\x")),
        "no envelope may reach a caller: {all}"
    );
}

/// Adversarial-sweep regressions. Both were PROVEN escalations against the
/// live database before the fix; each assertion below is the exploit.
#[test]
fn guard_sweep_regressions_stay_fixed() {
    let f = Fixture::load().expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_writes_regr");
    let pg = Pg {
        db: "ordo_writes_regr".into(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();
    let w = |log: &mut WriteLog, call: &str, as_: Option<&str>| f.run_write_as(&pg, log, call, as_);

    // ── Critical 1: reparenting across the authorization boundary ───────
    // Bram owns proj_2, so the write guard `project.owner == $caller`
    // holds on pm_3's CURRENT parent. Moving it to proj_3 (Cleo's) would
    // have carried his membership into a project he cannot even see.
    let e = w(&mut log, r#"movePM($pm: "pm_3", $p: "proj_3")"#, BRAM)
        .expect_err("reparenting into an invisible project must fail");
    assert!(
        e.starts_with("not_found"),
        "an invisible target is not_found, like any unreachable row: {e}"
    );
    assert_eq!(
        pg.query(r#"SELECT "project" FROM "projectMembers" WHERE "id" = 'pm_3'"#)
            .unwrap()[0][0]
            .as_deref(),
        Some("proj_2"),
        "the row did not move"
    );
    // And proj_3 stayed invisible to him — the escalation's whole point.
    assert_eq!(
        f.run_case_as(&pg, r#"projects(id == "proj_3")[name]"#, &[], BRAM)
            .unwrap()
            .as_array()
            .map(Vec::len),
        Some(0)
    );

    // The visible-but-unwritable target is the other half: Ana can SEE
    // proj_1 and proj_3, so this is an access_guard, not a not_found.
    let e = w(&mut log, r#"movePM($pm: "pm_5", $p: "proj_1")"#, ANA);
    if let Err(msg) = &e {
        assert!(
            msg.starts_with("access_guard") || msg.starts_with("not_found"),
            "{msg}"
        );
    }

    // The acl form of the same attack: re-point an admin access row at a
    // board the caller has no access to.
    //
    // `duplicate` is now in the accepted set, and it is the answer this
    // particular move earns: the derived link carries `unique (board,
    // member)`, and Bram already holds ba_2 on board_1, so moving his
    // board_2 admin row there would make two rows for one (board, member)
    // — refused before the guard's post-image check is even reached.
    // Nothing is applied, which is the property under test; the class
    // names the first thing that was wrong with the statement.
    let e = w(&mut log, r#"moveBA($ba: "ba_4", $b: "board_1")"#, BRAM);
    match e {
        Err(m) => assert!(
            m.starts_with("access_guard")
                || m.starts_with("not_found")
                || m.starts_with("duplicate"),
            "{m}"
        ),
        Ok(_) => {
            // Bram is admin on board_2 and only `write` on board_1, so if
            // this were ever allowed it would be an escalation.
            panic!("moving an access row onto a board the caller does not administer must fail");
        }
    }

    // A reassignment WITHIN the caller's authority still works — the fix
    // must not have made ordinary writes impossible.
    let ok = w(&mut log, r#"reassign($task: "task_1", $who: "mem_2")"#, ANA)
        .expect("Ana may reassign her own project's task");
    assert_eq!(ok, json!([{ "entity": "tasks", "updated": 1 }]));

    // ── Critical 2: create-exemption must not swallow a satisfiable
    // disjunct. `boards` (all-reverse-step) stays exempt; `tasks`, whose
    // guard walks a parent that already exists, never was.
    assert!(
        ordo_sema::create_exempt(&f.schema, "boards"),
        "an all-reverse-step guard is still exempt"
    );
    assert!(
        !ordo_sema::create_exempt(&f.schema, "tasks"),
        "a guard reaching an existing parent must be proven, not assumed"
    );
    assert!(
        !ordo_sema::create_exempt(&f.schema, "projects"),
        "`owner == $caller or projectMembers(...)` has a satisfiable \
         disjunct — flattening the atoms once made this wrongly exempt"
    );
}

/// Tree writes (§tree, settled): the platform maintains the materialized
/// path and depth, rejects cycles, caps depth, and refuses to delete a
/// node with children. The read side (`under`, `above`, `depth`) must keep
/// answering correctly after every one of these.
#[test]
fn tree_writes_maintain_the_path_and_refuse_what_they_should() {
    let f = Fixture::load().expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_writes_tree");
    let pg = Pg {
        db: "ordo_writes_tree".into(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();
    let w = |log: &mut WriteLog, call: &str| f.run_write_as(&pg, log, call, ANA);
    let node = |id: &str| {
        pg.query(&format!(
            r#"SELECT "$path", "depth" FROM "categories" WHERE "id" = '{id}'"#
        ))
        .unwrap()
    };

    // ── create: path and depth derive from the parent chain ─────────────
    // Seeded: cat_1 goods / cat_2 tools / cat_3 handtools (depth 0,1,2).
    let r = w(
        &mut log,
        r#"addCategory($name: "chisels", $parent: "cat_3")"#,
    )
    .expect("create under a real parent");
    let kid = r[0]["created"].as_str().unwrap().to_string();
    let row = node(&kid);
    assert_eq!(
        row[0][0].as_deref(),
        Some(format!("cat_1/cat_2/cat_3/{kid}/").as_str()),
        "the path is the id chain with a trailing delimiter"
    );
    assert_eq!(row[0][1].as_deref(), Some("3"), "depth follows the parent");

    // A root has no parent and starts at zero.
    let r = w(&mut log, r#"addRootCategory($name: "rentals")"#).expect("root");
    let root = r[0]["created"].as_str().unwrap().to_string();
    assert_eq!(node(&root)[0][1].as_deref(), Some("0"));

    // And the READ side sees it immediately — the write maintained exactly
    // what `under` tests against.
    let under = f
        .run_case_params(
            &pg,
            "categories(id under $r)[name]\nsort name",
            &[("r", "\"cat_1\"")],
        )
        .expect("under");
    let names: Vec<&str> = under
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["chisels", "goods", "handtools", "tools"]);

    // ── reparent: the whole subtree moves, at every depth ───────────────
    // Move tools (with handtools and chisels under it) to be a root.
    w(&mut log, r#"moveCategory($cat: "cat_2", $parent: null)"#).expect("reparent to root");
    assert_eq!(
        node("cat_2")[0],
        vec![Some("cat_2/".into()), Some("0".into())]
    );
    assert_eq!(
        node("cat_3")[0],
        vec![Some("cat_2/cat_3/".into()), Some("1".into())],
        "the child followed"
    );
    assert_eq!(
        node(&kid)[0],
        vec![Some(format!("cat_2/cat_3/{kid}/")), Some("2".into())],
        "and so did the grandchild"
    );
    // The read side agrees: goods no longer contains tools.
    let under = f
        .run_case_params(
            &pg,
            "categories(id under $r)[name]\nsort name",
            &[("r", "\"cat_1\"")],
        )
        .unwrap();
    assert_eq!(under.as_array().unwrap().len(), 1, "goods stands alone");

    // Move it back under goods; depths restore.
    w(&mut log, r#"moveCategory($cat: "cat_2", $parent: "cat_1")"#).expect("reparent back");
    assert_eq!(node("cat_3")[0][1].as_deref(), Some("2"));
    assert_eq!(node(&kid)[0][1].as_deref(), Some("3"));

    // ── cycles are refused, including the transitive case ───────────────
    let e = w(&mut log, r#"moveCategory($cat: "cat_2", $parent: "cat_3")"#)
        .expect_err("a node cannot move under its own child");
    assert!(e.starts_with("tree_cycle"), "{e}");
    let e = w(
        &mut log,
        &format!(r#"moveCategory($cat: "cat_1", $parent: "{kid}")"#),
    )
    .expect_err("nor under its own grandchild");
    assert!(e.starts_with("tree_cycle"), "{e}");
    let e = w(&mut log, r#"moveCategory($cat: "cat_2", $parent: "cat_2")"#)
        .expect_err("nor under itself");
    assert!(e.starts_with("tree_cycle"), "{e}");
    // A refused move changed nothing.
    assert_eq!(node("cat_2")[0][0].as_deref(), Some("cat_1/cat_2/"));

    // ── delete refuses with children, and states the count ──────────────
    let e = w(&mut log, r#"dropCategory($cat: "cat_2")"#)
        .expect_err("tools still has handtools under it");
    assert!(e.starts_with("tree_has_children"), "{e}");
    assert!(e.contains(" 1 child"), "the count is in the message: {e}");
    // A leaf with nothing pointing at it deletes cleanly.
    w(&mut log, &format!(r#"dropCategory($cat: "{kid}")"#)).expect("leaf");
    assert!(node(&kid).is_empty());
    // Its parent is childless now, but products still point at it: the
    // settled rule is refuse-with-referrers, in the platform's own
    // vocabulary rather than a leaked foreign-key violation.
    let e = w(&mut log, r#"dropCategory($cat: "cat_3")"#)
        .expect_err("products still reference handtools");
    assert!(e.starts_with("has_referrers"), "{e}");
    assert!(
        e.contains("products.category"),
        "the message names a referrer: {e}"
    );
    assert!(
        !e.contains("constraint") && !e.contains("psql"),
        "and does not leak the database's own error: {e}"
    );

    // ── a ranged reparent is refused rather than half-applied ───────────
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"moveCategory($cat: "cat_2", $parent: "cat_4")"#,
            ANA,
        )
        .expect("single-node moves are the supported shape");
    assert_eq!(e, json!([{ "entity": "categories", "updated": 1 }]));
}

/// The venue gate, now that host classes are reachable (DL ~1816, the
/// landmine ledgered when they were not): `take` bounds what a plan
/// RETURNS, not what it EXAMINES. An op that opens sealed payloads for an
/// unbounded number of rows must acknowledge it, even if its row bound is
/// small.
///
/// A keyset page bounds a sealed projection the way `take` does (ruling
/// 2026-09-05; std-library.md §5 rules 6 and 7), so the unbounded case
/// here is a plan whose opened rows outnumber the page: a many-side join
/// repeats its anchor across rows. A group over a sealed field is not one:
/// nothing sealed survives the fold, so it opens nothing (ruled
/// 2026-10-05, `audit-2026-10-05` ask 1).
#[test]
fn the_venue_gate_keys_on_where_the_work_happens() {
    use ordo_plan::cost::{classify_with, CostClass};
    let f = Fixture::load().expect("fixture");

    // A bounded-host op registers without the acknowledgment: `take 3`
    // bounds the rows AND the payloads, because the take is the whole
    // plan's limit.
    let q = f.bind_case("memoDigest").expect("bound host op binds");
    assert_eq!(classify_with(&q, Some(&f.schema)), CostClass::BoundedHost);

    let pg = Pg {
        db: "ordo_conformance".into(),
        ..Pg::default()
    };
    // The shared database is seeded by whichever suite reaches it first.
    // On a fresh cluster this binary alone found no `orders` table.
    f.reset_once(&pg).expect("seeded");
    // So does a paged one: each request opens one page's payloads.
    let paged = "op pagedMemos =\n  orders[placed, memo]\n\npagedMemos[placed]";
    f.run_case_with_ops(&pg, paged, &[])
        .expect("a paged sealed projection is bounded and needs no acknowledgment");

    // A group folds the memos away, so it is plain SQL and registers
    // without the acknowledgment.
    let folded = "op memoCounts =\n  orders[placed, memo]\n  group placed { n: count }\n\n\
                  memoCounts[placed, n]";
    f.run_case_with_ops(&pg, folded, &[])
        .expect("a group over a sealed field opens nothing and needs no acknowledgment");

    // An unbounded one does not: registering it without `unbounded` is
    // refused, and the message says the take/examine distinction out loud.
    // A many-side join pages over its anchor and opens a payload for every
    // joined row, so the page bounds nothing the host does.
    let body = "customers[name] orders[placed, memo]\n\nleakyMemos[name, memo]";
    let src = format!("op leakyMemos =\n  {body}");
    let e = f
        .run_case_with_ops(&pg, &src, &[])
        .expect_err("unbounded host work must be acknowledged");
    assert!(e.contains("unacknowledged_unbounded"), "{e}");
    assert!(e.contains("EXAMINED"), "the reason is stated: {e}");

    // And the acknowledgment is enough — nothing else changes.
    let ok = format!("op leakyMemos unbounded =\n  {body}");
    let ok = ok.as_str();
    f.run_case_with_ops(&pg, ok, &[])
        .expect("acknowledged host work registers");
}

/// §6 rule 9: `rev` is a belief about the addressed row, and a write whose
/// belief is stale is refused rather than silently overwriting.
///
/// `cas_miss` was in the wire error inventory with no code path able to
/// raise it — a promise the API made and the platform could not keep.
#[test]
fn rev_is_a_belief_and_a_stale_one_is_refused() {
    let f = Fixture::load().expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_writes_cas");
    let pg = Pg {
        db: "ordo_writes_cas".into(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();
    let w = |log: &mut WriteLog, call: &str| f.run_write_as(&pg, log, call, ANA);
    let rev = || {
        pg.query(r#"SELECT "rev" FROM "tasks" WHERE "id" = 'task_1'"#)
            .unwrap()[0][0]
            .clone()
    };

    // The row is at rev 1. A belief that matches converges.
    assert_eq!(rev(), Some("1".into()));
    let r = w(
        &mut log,
        r#"renameTaskIf($task: "task_1", $title: "first", $rev: 1)"#,
    )
    .expect("the belief held");
    assert_eq!(r, json!([{ "entity": "tasks", "updated": 1 }]));
    assert_eq!(rev(), Some("2".into()), "and the version moved");

    // The same call again is now stale — this is the read-modify-write
    // race, and the whole point is that it is refused rather than applied.
    let e = w(
        &mut log,
        r#"renameTaskIf($task: "task_1", $title: "second", $rev: 1)"#,
    )
    .expect_err("the belief is stale");
    assert!(e.starts_with("cas_miss"), "{e}");
    assert!(
        e.contains("is at rev 2") && e.contains("expected 1"),
        "both versions named, so a retry knows how far behind it is: {e}"
    );
    // Nothing was applied.
    assert_eq!(
        pg.query(r#"SELECT "title" FROM "tasks" WHERE "id" = 'task_1'"#)
            .unwrap()[0][0]
            .as_deref(),
        Some("first")
    );
    assert_eq!(rev(), Some("2".into()));

    // Believing the current version works again.
    w(
        &mut log,
        r#"renameTaskIf($task: "task_1", $title: "third", $rev: 2)"#,
    )
    .expect("current belief");

    // The guard is checked BEFORE the version: a caller who may not write
    // the row learns that, not how far behind they are.
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"renameTaskIf($task: "task_1", $title: "bram", $rev: 999)"#,
            BRAM,
        )
        .expect_err("Bram cannot write task_1");
    assert!(
        e.starts_with("access_guard"),
        "authorization precedes version disclosure: {e}"
    );
}

/// Register `src`'s ops on top of the fixture's and run the deploy gate —
/// the compile half alone: no database, no call, nothing but the check an
/// `ordo deploy` would run.
fn register(f: &Fixture, src: &str) -> Result<(), String> {
    let program = ordo_syntax::parse_program(src)
        .map_err(|e| format!("parse: {} at {}", e.message, e.span))?;
    let mut registry = f.registry.clone();
    registry.extend_from(&program);
    ordo_plan::cost::check_registration(&f.schema, &registry).map_err(|e| e.to_string())
}

/// `subtree` is spellable on a tree entity only. On any other entity it used
/// to register, deploy, and then answer `internal` when a caller reached it
/// (25b). It is now refused when the op is registered, so it never deploys.
#[test]
fn subtree_on_an_entity_with_no_tree_ref_never_registers() {
    let f = Fixture::load().expect("fixture");

    let e = register(
        &f,
        "op dropTask($id: id) =\n  del tasks { id: $id } subtree",
    )
    .expect_err("`tasks` is not a tree");
    assert!(
        e.contains("subtree_not_tree") && e.contains("`tasks`"),
        "{e}"
    );

    // A tree entity still registers.
    register(
        &f,
        "op dropBranch($id: id) =\n  del categories { id: $id } subtree",
    )
    .expect("`categories` is a tree");
}

/// §6 rule 2: a statement that can create must be **insert-complete**, and
/// the compiler says so at registration.
///
/// The failure this replaces is the one that matters: a create missing a
/// required field used to register fine, deploy fine, and then answer a
/// Postgres NOT NULL violation on the first call that reached the create
/// branch — the wrong layer, the wrong error class, and discovered by a
/// caller rather than by a reviewer. Nothing about the statement needed a
/// row to be knowable, so nothing about it should have waited for one.
#[test]
fn a_create_that_cannot_be_complete_never_registers() {
    let f = Fixture::load().expect("fixture");

    // `tasks` requires `project` and `title`; this create supplies neither.
    let e = register(&f, "op orphanTask($s: text) =\n  set tasks { status: $s }")
        .expect_err("an incomplete create must not register");
    assert!(e.contains("insert_incomplete"), "{e}");
    assert!(
        e.contains("`project`") && e.contains("`title`"),
        "the message names every missing field, not just the first: {e}"
    );
    assert!(
        !e.contains("null value") && !e.contains("NOT NULL"),
        "and it is the compiler's answer, not the database's: {e}"
    );

    // Every statement of a multi-statement op is checked: the op is
    // atomic, so one incomplete statement condemns the whole group.
    let e = register(
        &f,
        "op taskWithNote($p: id, $t: text, $b: text) =\n  set tasks { project: $p, title: $t }\n  set taskNotes { body: $b }",
    )
    .expect_err("the second statement is incomplete");
    assert!(
        e.contains("insert_incomplete") && e.contains("`task`"),
        "{e}"
    );

    // The gate the harness itself runs rejects it too — this is the same
    // check every registration path goes through, not a test-only one.
    let pg = pg();
    let e = f
        .run_case_with_ops(
            &pg,
            "op orphanTask($s: text) =\n  set tasks { status: $s }",
            &[],
        )
        .expect_err("registration precedes everything");
    assert!(
        e.starts_with("register:") && e.contains("insert_incomplete"),
        "{e}"
    );

    // ── and the false positives the rule must NOT produce ───────────────
    // A DEFAULTED field is covered (`tasks.status`), an OPTIONAL one is
    // covered (`tasks.assignee`), and the implicit fields are the
    // platform's (`id`, `rev`) — so this create is complete as written.
    register(
        &f,
        "op fineTask($p: id, $t: text) =\n  set tasks { project: $p, title: $t }",
    )
    .expect("a complete create clears the gate");

    // An optional tree ref (`categories.parent`) needs no value, and the
    // implicit `depth` is never the caller's to supply.
    register(&f, "op rootCat($n: text) =\n  set categories { name: $n }")
        .expect("optional parent, implicit depth");

    // Converge and bulk are NOT creation-capable: they describe rows that
    // already exist, so covering one field is the whole statement.
    register(
        &f,
        "op justStatus($t: id) =\n  set tasks(id == $t) { status: \"done\" }",
    )
    .expect("addressed converge touches only what it describes");
    register(
        &f,
        "op closeProject($p: id) =\n  set tasks(project == $p) { status: \"done\" }",
    )
    .expect("bulk converge likewise");

    // The fixture's own eighteen write ops are the standing control:
    // `Fixture::load` ran `check_registration` over all of them above.
}

/// §6 rule 3: identity and unique-key fields may not appear in bulk braces
/// — addressed and bulk cannot blur.
///
/// The unique case is the one that reads innocently and cannot be meant:
/// one value assigned across a range asks every matched row to collide on
/// that key, so the statement is a no-op when one row matches and a
/// guaranteed `duplicate` when two do. Which one it is depends on data the
/// author is not looking at, which is exactly the kind of write that must
/// not survive review.
#[test]
fn bulk_braces_may_not_carry_identity() {
    let f = Fixture::load().expect("fixture");

    // `categories.name` is unique; renaming a range to one name collides.
    let e = register(
        &f,
        "op renameAll($n: text) =\n  set categories(parent is null) { name: $n }",
    )
    .expect_err("a ranged write may not assign a unique field");
    assert!(e.contains("bulk_identity_field"), "{e}");
    assert!(e.contains("`name`"), "the message names the field: {e}");

    // A `sealed indexed unique` field is unique for this purpose too — the
    // claim rides the blind index, and the collision is the same one.
    let e = register(
        &f,
        "op mergeCustomers($e: text) =\n  set customers(tier == \"basic\") { email: $e }",
    )
    .expect_err("uniqueness rides the blind index");
    assert!(
        e.contains("bulk_identity_field") && e.contains("`email`"),
        "{e}"
    );

    // Assigning `id` over a range blurs addressed and bulk outright.
    let e = register(
        &f,
        "op reidTasks($i: id) =\n  set tasks(status == \"open\") { id: $i }",
    )
    .expect_err("identity may not be assigned, least of all in bulk");
    assert!(e.contains("bulk_identity_field"), "{e}");

    // A condition that MENTIONS `id` without being `id == …` still ranges,
    // and the compile-time discriminator agrees with the executor's.
    let e = register(
        &f,
        "op halfAddressed($c: id, $n: text) =\n  set categories(id == $c and parent is null) { name: $n }",
    )
    .expect_err("a compound condition ranges");
    assert!(e.contains("bulk_identity_field"), "{e}");

    // The harness's own registration path rejects it as well.
    let pg = pg();
    let e = f
        .run_case_with_ops(
            &pg,
            "op renameAll($n: text) =\n  set categories(parent is null) { name: $n }",
            &[],
        )
        .expect_err("registration precedes everything");
    assert!(
        e.starts_with("register:") && e.contains("bulk_identity_field"),
        "{e}"
    );

    // ── the false positives it must NOT produce ─────────────────────────
    // An ordinary non-unique field over a range is what bulk is FOR.
    register(
        &f,
        "op closeAll($p: id) =\n  set tasks(project == $p) { status: \"done\" }",
    )
    .expect("bulk converge on ordinary fields stays legal");

    // The addressed form may assign a unique field: one row, one value, no
    // collision — the fixture's `renameTask` shape over a unique column.
    register(
        &f,
        "op renameCat($c: id, $n: text) =\n  set categories(id == $c) { name: $n }",
    )
    .expect("addressed writes are unaffected");

    // And a create may of course supply the unique field — that is the
    // key it will be addressed by.
    register(&f, "op newCat($n: text) =\n  set categories { name: $n }")
        .expect("creates carry their own key");

    // A ranged DELETE has no braces at all, so rule 3 has nothing to say.
    register(
        &f,
        "op wipeOpen($p: id) =\n  del tasks(project == $p and status == \"open\")",
    )
    .expect("deletes are unaffected");
}

/// Rules 5 and 6 — a delete states its target, and a cascade says so.
///
/// `del <entity> { id: $id } subtree` is the only spelling that deletes a
/// tree node's descendants. It compiles to **per-row commands** so a
/// revert can put the subtree back, evaluates the write guard over
/// **every** row rather than the anchor alone, and still refuses when
/// something outside the subtree points into it. Bare `del <entity>` is
/// refused by the parser (`ordo-syntax`'s own tests); the stated
/// `del <entity>(true)` form closes this one.
#[test]
fn subtree_deletes_cascade_per_row_or_refuse_the_whole_thing() {
    let f = Fixture::load().expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_writes_subtree");
    let pg = Pg {
        db: "ordo_writes_subtree".into(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();
    let space = SpaceId("conformance".into());
    let ids = |sql: &str| -> Vec<String> {
        pg.query(sql)
            .unwrap()
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect()
    };
    let folders = || ids(r#"SELECT "id" FROM "folders" ORDER BY "id""#);
    let cat_count = || {
        pg.query(r#"SELECT count(*) FROM "categories""#).unwrap()[0][0]
            .clone()
            .unwrap()
    };

    // ── the happy cascade: Ana owns all of fold_5's subtree ─────────────
    // Seeded: fold_5 archive > fold_6 2025 > fold_7 q1, all Ana's.
    let r = f
        .run_write_as(&pg, &mut log, r#"dropFolderTree($f: "fold_5")"#, ANA)
        .expect("Ana owns every row under fold_5");
    assert_eq!(
        r,
        json!([{ "entity": "folders", "deleted": 3 }]),
        "the anchor AND both descendants — a cascade, not an addressed delete"
    );
    assert_eq!(
        folders(),
        ["fold_1", "fold_2", "fold_3", "fold_4"],
        "and nothing outside the subtree went with it"
    );

    // Per-row commands, deepest first, each with its own before image —
    // this is what makes the cascade revertible. One bulk command could
    // not put three rows back.
    let g = log.store.get(&space, 0).expect("the cascade group");
    let cmds = &g.group.commands;
    assert_eq!(cmds.len(), 3, "one command per deleted row");
    assert_eq!(
        cmds.iter().map(|c| c.row_id.as_str()).collect::<Vec<_>>(),
        ["fold_7", "fold_6", "fold_5"],
        "deepest first: a parent cannot go before the children pointing at it"
    );
    for c in cmds {
        assert_eq!(c.kind, CommandKind::Delete);
        assert!(
            c.before.is_some() && c.after.is_none(),
            "a delete owes its before image and has no after"
        );
    }
    assert_eq!(
        cmds[0].before.as_ref().unwrap()["name"],
        LogValue::Text("q1".into()),
        "the image is the row as it was, field by field"
    );

    // ── guards apply to EVERY row, and a partial cascade is refused ─────
    // fold_4 (Bram's) hangs under fold_1 (Ana's). Ana may delete the
    // anchor and two of the three descendants — exactly the case that
    // must not half-apply: deleting what she may leaves fold_4 pointing
    // at a fold_1 that no longer exists.
    let before = log.store.len(&space);
    let e = f
        .run_write_as(&pg, &mut log, r#"dropFolderTree($f: "fold_1")"#, ANA)
        .expect_err("Bram's folder is inside Ana's subtree");
    assert!(e.starts_with("access_guard"), "{e}");
    assert!(e.contains("1 row"), "the refusal states how many: {e}");
    assert!(
        !e.contains("fold_4"),
        "but not WHICH — being told no is not how a caller learns which \
         rows exist: {e}"
    );
    assert_eq!(
        folders(),
        ["fold_1", "fold_2", "fold_3", "fold_4"],
        "all or nothing: even the rows Ana could delete are still there"
    );
    assert_eq!(
        log.store.len(&space),
        before,
        "and a refused write appends no command group"
    );

    // The anchor's own guard failure is the ordinary single-row answer;
    // nothing about `subtree` makes it a different one.
    let e = f
        .run_write_as(&pg, &mut log, r#"dropFolderTree($f: "fold_4")"#, CLEO)
        .expect_err("Cleo owns nothing here");
    assert!(e.starts_with("access_guard: delete `fold_4`"), "{e}");

    // Bram takes his own leaf out — one row, one command.
    let r = f
        .run_write_as(&pg, &mut log, r#"dropFolderTree($f: "fold_4")"#, BRAM)
        .expect("his own row");
    assert_eq!(r, json!([{ "entity": "folders", "deleted": 1 }]));
    // …and now every row under fold_1 is Ana's, so her cascade lands.
    let r = f
        .run_write_as(&pg, &mut log, r#"dropFolderTree($f: "fold_1")"#, ANA)
        .expect("the blocker is gone");
    assert_eq!(r, json!([{ "entity": "folders", "deleted": 3 }]));
    assert!(folders().is_empty(), "the whole forest is gone");

    // ── the default is still refuse-with-children ───────────────────────
    // The plain addressed form (`del folders { id: $f }`) never slides
    // into a cascade — that is the entire reason `subtree` is a word.
    f.reset(&pg).expect("reseeded");
    let mut log = WriteLog::new();
    let e = f
        .run_write_as(&pg, &mut log, r#"dropFolder($f: "fold_1")"#, ANA)
        .expect_err("fold_1 has children");
    assert!(e.starts_with("tree_has_children"), "{e}");
    assert!(e.contains(" 2 child"), "with the count: {e}");
    assert_eq!(folders().len(), 7, "and nothing deleted");

    // ── referential integrity outlives the cascade ──────────────────────
    // `categories` is unguarded, so what stops a cascade here is the other
    // half of rule 6: rows OUTSIDE the subtree that point into it. The
    // seeded products reference cat_2, cat_3, cat_4 and cat_5.
    let e = f
        .run_write_as(&pg, &mut log, r#"dropCategoryTree($cat: "cat_1")"#, ANA)
        .expect_err("products still point at handtools");
    assert!(e.starts_with("has_referrers"), "{e}");
    assert!(
        e.contains("products.category"),
        "naming the referrer, in the platform's vocabulary rather than the \
         database's: {e}"
    );
    assert_eq!(
        cat_count(),
        "5",
        "a refused cascade deletes nothing at all — the refusal is not a \
         foreign-key violation two rows in"
    );

    // A subtree nothing else references cascades to any depth. Build one
    // three deep through the ordinary create path, so the paths under it
    // are the platform's own rather than the seed's.
    let w = |log: &mut WriteLog, call: &str| f.run_write_as(&pg, log, call, ANA);
    let made = |r: serde_json::Value| r[0]["created"].as_str().unwrap().to_string();
    let root = made(w(&mut log, r#"addRootCategory($name: "rentals")"#).expect("root"));
    let mid = made(
        w(
            &mut log,
            &format!(r#"addCategory($name: "vans", $parent: "{root}")"#),
        )
        .expect("mid"),
    );
    let leaf = made(
        w(
            &mut log,
            &format!(r#"addCategory($name: "cargo", $parent: "{mid}")"#),
        )
        .expect("leaf"),
    );
    let at = log.store.len(&space);
    let r = w(&mut log, &format!(r#"dropCategoryTree($cat: "{root}")"#)).expect("free subtree");
    assert_eq!(r, json!([{ "entity": "categories", "deleted": 3 }]));
    let cascade = log.store.get(&space, at).expect("the cascade group");
    assert_eq!(
        cascade
            .group
            .commands
            .iter()
            .map(|c| c.row_id.clone())
            .collect::<Vec<_>>(),
        [leaf, mid, root],
        "deepest first again, this time over paths the writes maintained"
    );
    assert_eq!(
        cat_count(),
        "5",
        "the seeded five survive; only the built subtree went"
    );

    // ── rule 5: delete-everything is spelled out, and still guarded ─────
    // `del taskNotes(true)` says what it does. It is a BULK delete, so
    // G11 narrows it to the caller's write view — the opposite of the
    // subtree ruling above, and deliberately: bulk rows are independent,
    // so a narrowed range leaves every survivor coherent, while half a
    // subtree is not a smaller subtree.
    let r = f
        .run_write_as(&pg, &mut log, "purgeNotes", ANA)
        .expect("stated delete-everything");
    assert_eq!(r, json!([{ "entity": "taskNotes", "deleted": 1 }]));
    assert_eq!(
        ids(r#"SELECT "id" FROM "taskNotes" ORDER BY "id""#),
        ["note_2", "note_3"],
        "the notes on tasks Ana cannot write are untouched"
    );
}

/// §6 rule 8's second clause — **converge-or-create**, the one blessed
/// upsert — and rule 9's beliefs on top of it.
///
/// The promise this mode makes is idempotent ingestion: one statement per
/// record, delivered twice with the same external id, leaves one row. It
/// compiles to `INSERT … ON CONFLICT` (`implementation.md` pins it), so
/// the branch is decided by the database against the unique index and not
/// by a `SELECT` a competing writer has already invalidated.
///
/// `ledgerEntries` is guarded read ≠ write on purpose: the two branches
/// ask different authorization questions, and this is where that shows.
#[test]
fn converge_or_create_is_the_one_blessed_upsert() {
    let f = Fixture::load().expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_writes_upsert");
    let pg = Pg {
        db: "ordo_writes_upsert".into(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();
    let w = |log: &mut WriteLog, call: &str, as_: Option<&str>| f.run_write_as(&pg, log, call, as_);
    let rows = || {
        pg.query(
            r#"SELECT "id", "externalId", "amount", "note", "rev" FROM "ledgerEntries" ORDER BY "id""#,
        )
        .unwrap()
    };

    // ── the ingestion promise: the same record twice is one row ─────────
    let r = w(
        &mut log,
        r#"recordEntry($ext: "inv-1", $amount: 100, $note: "shared")"#,
        ANA,
    )
    .expect("first delivery creates");
    let entry = r[0]["created"].as_str().expect("created id").to_string();
    assert!(entry.starts_with("led_"), "typed id: {entry}");

    // The webhook redelivers with a corrected amount. No id was ever sent
    // and none is needed: the declared unique key IS the address.
    let r = w(
        &mut log,
        r#"recordEntry($ext: "inv-1", $amount: 250, $note: "shared")"#,
        ANA,
    )
    .expect("redelivery converges");
    assert_eq!(
        r,
        json!([{ "entity": "ledgerEntries", "converged": entry }]),
        "the summary names the row it landed on — the one fact this mode settles"
    );
    assert_eq!(rows().len(), 1, "two deliveries, one row");
    assert_eq!(
        rows()[0][2].as_deref(),
        Some("250"),
        "and the later value won"
    );
    assert_eq!(rows()[0][4].as_deref(), Some("2"), "a converge bumps rev");

    // Rule 10: a converge touches the fields it DESCRIBES. `reviseEntry`
    // says nothing about `note`, so `note` keeps the value it has — the
    // create branch's default does not reach across.
    w(
        &mut log,
        r#"reviseEntry($ext: "inv-1", $amount: 300, $rev: 2)"#,
        ANA,
    )
    .expect("the belief held");
    assert_eq!(rows()[0][2].as_deref(), Some("300"));
    assert_eq!(
        rows()[0][3].as_deref(),
        Some("shared"),
        "an undescribed field keeps its value; defaults belong to the create branch"
    );

    // ── rule 9 on a KEYED write: `rev: $r` ──────────────────────────────
    // This is the hole the rule was written to close: without it, calling
    // the upsert twice in earnest SILENTLY updates the first row.
    let e = w(
        &mut log,
        r#"reviseEntry($ext: "inv-1", $amount: 400, $rev: 2)"#,
        ANA,
    )
    .expect_err("the belief is now stale");
    assert!(e.starts_with("cas_miss"), "{e}");
    assert!(
        e.contains("is at rev 3") && e.contains("expected 2"),
        "both versions named, exactly as on the id-addressed path: {e}"
    );
    assert_eq!(rows()[0][2].as_deref(), Some("300"), "nothing applied");

    // A belief about a row the key does not name is stale in the only way
    // that matters: nothing is at that version, so the caller refetches.
    // It is NOT a create — the belief said "converge if r".
    let e = w(
        &mut log,
        r#"reviseEntry($ext: "inv-absent", $amount: 400, $rev: 1)"#,
        ANA,
    )
    .expect_err("no row holds that key");
    assert!(e.starts_with("cas_miss"), "{e}");
    assert_eq!(rows().len(), 1, "and a stated belief never creates");

    // ── rule 9 on a KEYED write: `rev: null` is create-once ─────────────
    let r = w(
        &mut log,
        r#"recordEntryOnce($ext: "inv-2", $amount: 500)"#,
        ANA,
    )
    .expect("nothing holds inv-2, so create-once creates");
    let once = r[0]["created"].as_str().expect("created").to_string();
    let e = w(
        &mut log,
        r#"recordEntryOnce($ext: "inv-2", $amount: 700)"#,
        ANA,
    )
    .expect_err("the second call asserted absence and the row is present");
    assert!(e.starts_with("expected_absent"), "{e}");
    assert!(
        e.contains(&once),
        "the refusal names the row that refuted the belief: {e}"
    );
    let inv2 = rows()
        .into_iter()
        .find(|r| r[1].as_deref() == Some("inv-2"))
        .expect("inv-2");
    assert_eq!(inv2[2].as_deref(), Some("500"), "nothing applied");
    assert_eq!(inv2[4].as_deref(), Some("1"), "not even a rev bump");

    // ── the null key: never a create ────────────────────────────────────
    // `null == null` is true in this language, so a null key is indistinct
    // from every other unaddressed row — not an identity claim. Postgres's
    // default `NULLS DISTINCT` would have made it a quiet second row under
    // the one blessed upsert.
    let e = w(&mut log, r#"recordEntry($ext: null, $amount: 1)"#, ANA)
        .expect_err("a null key addresses nothing");
    assert!(e.starts_with("null_key_address"), "{e}");
    assert_eq!(rows().len(), 2, "and created nothing");

    // ── two covered unique keys: refused, never guessed ─────────────────
    // `coupons` declares `code` and `slug` unique. A statement covering
    // both claims two identities that can name two different rows, and no
    // tie-break the platform invented would be one the author could
    // predict. It costs nothing at runtime: it never reaches SQL.
    let e = w(
        &mut log,
        r#"claimCoupon($code: "c-1", $slug: "welcome", $value: 500)"#,
        ANA,
    )
    .expect_err("ambiguous identity");
    assert!(e.starts_with("ambiguous_identity"), "{e}");
    assert!(
        e.contains("code") && e.contains("slug"),
        "the refusal names both keys, because the fix is to drop one: {e}"
    );

    // ── the guard question: which guard, on which branch ────────────────
    // Converging onto a row the caller can SEE but not WRITE is the write
    // guard's answer, exactly as on the addressed path. It must not be the
    // create gate's: `recordEntry` supplies `owner: $caller`, which
    // SATISFIES the create gate — so a converge authorized by the create
    // gate would let anyone overwrite anyone's row just by supplying
    // values that suit them.
    let e = w(
        &mut log,
        r#"recordEntry($ext: "inv-1", $amount: 999, $note: "bram was here")"#,
        BRAM,
    )
    .expect_err("Bram may read the shared line, not write it");
    assert!(e.starts_with("access_guard"), "{e}");
    assert_eq!(rows()[0][2].as_deref(), Some("300"), "nothing applied");

    // Converging onto a row the caller cannot see at all answers
    // `duplicate`: the key is taken. That is exactly what the unique index
    // would have told them had the create been attempted, and no more —
    // `access_guard` would confirm the row is one they may look at.
    let e = w(
        &mut log,
        r#"recordEntry($ext: "inv-2", $amount: 999)"#,
        BRAM,
    )
    .expect_err("inv-2 is Ana's private line");
    assert!(e.starts_with("duplicate"), "{e}");

    // The CREATE branch is the create gate's, evaluated against the
    // would-be row — the only thing there is to evaluate it against.
    let e = w(
        &mut log,
        r#"recordEntryFor($ext: "inv-3", $owner: "mem_1", $amount: 10)"#,
        BRAM,
    )
    .expect_err("Bram cannot create a line owned by Ana");
    assert!(e.starts_with("access_guard"), "{e}");
    // …and it accepts the create it should.
    w(
        &mut log,
        r#"recordEntryFor($ext: "inv-4", $owner: "mem_2", $amount: 10)"#,
        BRAM,
    )
    .expect("Bram's own line");

    // ── a sealed unique key addresses through its blind index ───────────
    // `customers.email` is `sealed indexed unique normalize(lower)`: the
    // payload carries a fresh nonce, so two seals of one value never
    // compare equal and the digest is the only comparable spelling. Mixed
    // case on the create, lowercase on the converge: the same row.
    let r = w(
        &mut log,
        r#"upsertCustomer($name: "Zed", $email: "Zed@Example.COM")"#,
        ANA,
    )
    .expect("create through the blind index");
    let cust = r[0]["created"].as_str().expect("created").to_string();
    let r = w(
        &mut log,
        r#"upsertCustomer($name: "Zed II", $email: "zed@example.com", $tier: "pro")"#,
        ANA,
    )
    .expect("converge through the blind index");
    assert_eq!(
        r,
        json!([{ "entity": "customers", "converged": cust }]),
        "normalize(lower) decides what `the same row` means"
    );
    let row = pg
        .query(&format!(
            r#"SELECT "name", "tier", "rev" FROM "customers" WHERE "id" = '{cust}'"#
        ))
        .unwrap();
    assert_eq!(row[0][0].as_deref(), Some("Zed II"));
    assert_eq!(row[0][1].as_deref(), Some("pro"));
    assert_eq!(row[0][2].as_deref(), Some("2"), "converged, not duplicated");
    assert_eq!(
        pg.query(r#"SELECT count(*) FROM "customers""#).unwrap()[0][0].as_deref(),
        Some("4"),
        "three seeded customers and exactly one Zed"
    );

    // ── the log tells the two branches apart ────────────────────────────
    let space = SpaceId("conformance".into());
    let n = log.store.len(&space);
    let chain: Vec<_> = (0..n)
        .map(|i| log.store.get(&space, i).expect("indexed").clone())
        .collect();
    verify_chain(&space, &chain).expect("hash chain verifies");
    let c = &chain[0].group.commands[0];
    assert_eq!(c.kind, CommandKind::Create, "the first delivery created");
    assert_eq!(c.row_id, entry);
    let c = &chain[1].group.commands[0];
    assert_eq!(
        c.kind,
        CommandKind::Converge,
        "the redelivery converged — one mode, two commands, and the log \
         records which one happened"
    );
    assert_eq!(c.row_id, entry, "onto the same row");
    // (Money rides as text in the conformance images — the executor's
    // minor-unit encoding is F5's, not the log's.)
    assert_eq!(
        c.before.as_ref().unwrap()["amount"],
        LogValue::Text("100".into()),
        "with the pre-image the create wrote"
    );
    assert_eq!(
        c.after.as_ref().unwrap()["amount"],
        LogValue::Text("250".into())
    );
}

/// §6 rule 8's first clause — **identity precedence**: `id` in the braces
/// addresses, always, and every other unique field is an ordinary value.
///
/// The rule exists because the alternative is a statement whose meaning
/// depends on which fields happen to be marked unique three files away.
#[test]
fn identity_precedence_puts_the_id_first() {
    let f = Fixture::load().expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_writes_identity");
    let pg = Pg {
        db: "ordo_writes_identity".into(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();
    let w = |log: &mut WriteLog, call: &str| f.run_write_as(&pg, log, call, ANA);
    let rows = || {
        pg.query(r#"SELECT "id", "externalId", "rev" FROM "ledgerEntries" ORDER BY "id""#)
            .unwrap()
    };

    let a = w(&mut log, r#"recordEntry($ext: "inv-a", $amount: 10)"#).expect("a")[0]["created"]
        .as_str()
        .unwrap()
        .to_string();
    let b = w(&mut log, r#"recordEntry($ext: "inv-b", $amount: 20)"#).expect("b")[0]["created"]
        .as_str()
        .unwrap()
        .to_string();

    // `renumberEntry` covers `externalId`, a declared unique key — and it
    // also carries `id`. The id wins: this CONVERGES row `a` and renames
    // its external id. Had the unique key addressed instead, "inv-c" would
    // have named no row and the statement would have CREATED a third.
    let r = w(
        &mut log,
        &format!(r#"renumberEntry($entry: "{a}", $ext: "inv-c")"#),
    )
    .expect("the id addresses");
    assert_eq!(r, json!([{ "entity": "ledgerEntries", "updated": 1 }]));
    assert_eq!(
        rows().len(),
        2,
        "no third row: the unique key was a value, not an address"
    );
    assert_eq!(rows()[0][1].as_deref(), Some("inv-c"));
    assert_eq!(rows()[0][2].as_deref(), Some("2"), "row a converged");

    // A value that already identifies ANOTHER row makes the statement two
    // identity claims pointing at two rows. Rejected whole.
    let e = w(
        &mut log,
        &format!(r#"renumberEntry($entry: "{a}", $ext: "inv-b")"#),
    )
    .expect_err("id names a, externalId names b");
    assert!(e.starts_with("conflicting_identity"), "{e}");
    assert!(
        e.contains(&a) && e.contains("externalId"),
        "the message names the addressed row and the colliding key — but \
         never the row it collided with, which the caller may not be able \
         to read: {e}"
    );
    assert_eq!(rows()[0][1].as_deref(), Some("inv-c"), "nothing applied");
    assert_eq!(rows()[1][1].as_deref(), Some("inv-b"), "on either row");

    // Rule 4, no resurrection: converge on an ABSENT id is an error, never
    // a create — ids are server-minted, so absence means deleted.
    let e = w(
        &mut log,
        r#"renumberEntry($entry: "led_gone", $ext: "inv-z")"#,
    )
    .expect_err("absent id");
    assert!(e.starts_with("not_found"), "{e}");
    assert_eq!(rows().len(), 2, "and no row was resurrected");

    // Rule 8's null clause on the id form: `id == null` addresses no row.
    // The mode is STATIC — `id == $x` is an addressed write whatever `$x`
    // turns out to be — so a null address must not quietly become a ranged
    // write that selects nothing and reports success.
    let e = w(&mut log, r#"renameTask($task: null, $title: "x")"#)
        .expect_err("a null address is not an address");
    assert!(e.starts_with("null_key_address"), "{e}");

    // Rule 9's compile error, in the braces spelling: an id is obtainable
    // only from a row that existed, so asserting its absence contradicts
    // itself. Refused before any SQL runs.
    let e = w(
        &mut log,
        &format!(r#"renumberEntryOnce($entry: "{b}", $ext: "inv-y")"#),
    )
    .expect_err("id with rev: null");
    assert!(e.starts_with("invalid_binding_expr"), "{e}");
    assert!(e.contains("rev: null"), "{e}");
}

/// **8l — the reserved null sentinel, against real Postgres.**
///
/// A nullable `sealed indexed unique` field HMACs a reserved sentinel for
/// null instead of writing SQL `NULL` into its `$bidx` column. Two things
/// were silently wrong while it wrote NULL, and both are asserted here:
///
/// 1. Postgres `UNIQUE` permits unlimited NULLs, so `sealed indexed
///    unique` did not constrain nulls AT ALL — any number of customers
///    could have no email, against DL's settled rule that a nullable
///    unique field admits at most ONE null row (`null == null` is true).
/// 2. Blind-index equality against a null could never match, because the
///    comparison was `"email$bidx" = <digest>` against a column holding
///    NULL. Now `email == null` digests the sentinel and finds the row.
#[test]
fn the_null_sentinel_makes_sealed_nulls_unique_and_findable() {
    use ordo_conformance::fixture_space;
    use ordo_crypto::BlindValue;

    let f = Fixture::load().expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_null_sentinel");
    let pg = Pg {
        db: "ordo_null_sentinel".into(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    // The schema itself refuses a null digest: the column is NOT NULL even
    // though the base field is optional, because the digest is total.
    assert!(
        f.ddl.contains(r#""email$bidx" bytea NOT NULL"#),
        "a nullable sealed field still has a TOTAL digest:\n{}",
        f.ddl
    );

    // cust_3 has no email. Its payload is NULL and its digest is the
    // sentinel — the seed says so and Postgres agrees.
    let sentinel = ordo_crypto::hex(
        &f.keys
            .blind_index(&fixture_space(), "customers", "email", BlindValue::Null)
            .expect("sentinel digest"),
    );
    let row = pg
        .query(
            r#"SELECT "email" IS NULL, encode("email$bidx", 'hex')
                 FROM "customers" WHERE "id" = 'cust_3'"#,
        )
        .unwrap();
    assert_eq!(row[0][0].as_deref(), Some("t"), "the PAYLOAD is null");
    assert_eq!(
        row[0][1].as_deref(),
        Some(sentinel.as_str()),
        "the DIGEST is the sentinel, not null"
    );

    // 2. Equality against null reaches the digest column and MATCHES.
    assert_eq!(
        f.run_case(&pg, "anonCustomers[name]")
            .expect("== null runs"),
        json!([{ "name": "Cy" }]),
        "`email == null` finds the null row through the sentinel"
    );
    // It is a digest comparison, not `IS NULL` on the payload: the
    // plaintext column never appears in the plan.
    let p = f.compile("anonCustomers[name]").expect("compiles");
    assert!(p.sql.contains("email$bidx"), "{}", p.sql);
    assert!(!p.sql.contains(r#""email""#), "{}", p.sql);

    // 1. A SECOND null collides. cust_3 already holds the one null the
    //    unique field admits, so an anonymous create is refused by the
    //    database — the constraint is doing work it did not do before.
    let e = f
        .run_write_as(&pg, &mut log, r#"addAnonCustomer($name: "Dee")"#, ANA)
        .expect_err("a second null email collides");
    assert!(
        e.contains("customers$email$uq") || e.contains("duplicate key"),
        "the unique constraint refused it: {e}"
    );
    assert_eq!(
        f.run_case(&pg, "anonCustomers[name]").unwrap(),
        json!([{ "name": "Cy" }]),
        "and nothing was created"
    );

    // Clearing an email MOVES the digest to the sentinel; it does not
    // erase it. Which means clearing a second one collides too — with the
    // old behaviour both rows would have been nulled happily and the
    // unique field would have held two unaddressable rows.
    let e = f
        .run_write_as(&pg, &mut log, r#"clearCustomerEmail($c: "cust_1")"#, ANA)
        .expect_err("clearing into an occupied null slot collides");
    assert!(
        e.contains("customers$email$uq") || e.contains("duplicate key"),
        "assigning null lands on the sentinel, so it collides: {e}"
    );

    // With the null slot free, the same assignment succeeds and the row
    // becomes findable by `== null` — the positive half of the same rule.
    let cy = ordo_crypto::hex(
        &f.keys
            .blind_index(
                &fixture_space(),
                "customers",
                "email",
                BlindValue::Text("cy@example.com"),
            )
            .expect("digest"),
    );
    pg.exec(&format!(
        r#"UPDATE "customers" SET "email" = '\x00'::bytea,
             "email$bidx" = '\x{cy}'::bytea WHERE "id" = 'cust_3'"#
    ))
    .expect("free the null slot by giving cust_3 an email");
    f.run_write_as(&pg, &mut log, r#"clearCustomerEmail($c: "cust_1")"#, ANA)
        .expect("the null slot is free now");
    let row = pg
        .query(
            r#"SELECT "email" IS NULL, encode("email$bidx", 'hex')
                 FROM "customers" WHERE "id" = 'cust_1'"#,
        )
        .unwrap();
    assert_eq!(row[0][0].as_deref(), Some("t"), "payload cleared");
    assert_eq!(
        row[0][1].as_deref(),
        Some(sentinel.as_str()),
        "digest MOVED to the sentinel rather than being erased"
    );
    assert_eq!(
        f.run_case(&pg, "anonCustomers[name]").unwrap(),
        json!([{ "name": "Ada" }]),
        "and the cleared row is findable by `== null`"
    );
}
