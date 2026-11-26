//! `grant` blocks at apply (`plan-federation-grants.md` §3, step G1),
//! against a real database.
//!
//! What it proves:
//!
//! - Each block is expanded against the program's own ops, the derived
//!   `std.<entity>.get` and `.list` included. The last line that matches an
//!   op decides, and no match is not granted.
//! - The expanded list is stored with the version, and the next apply shows
//!   what changed ("note_viewer gains addReaction, matches /add.*/").
//! - Each refusal stops the apply before anything is stored, naming the
//!   class and the fix: a pattern matching nothing, a line changing nothing,
//!   a pattern that is not a regex, and a `grant provider` that is not a
//!   sign-in connection to Ordo's identity service.
//! - The plan (`std.deployment.plan`) shows each list, refuses what the apply
//!   refuses, and names `ORDO_CONSUMER_<NAME>_CLIENT`: its value when set, a
//!   warning when not.
//! - notes v2's own file, with a `grant note_viewer` block added, compiles
//!   and expands.
//!
//! A regression looks like an op granted that no line opens, a negated op
//! granted, a broken block applied, or a plan that does not say what a grant
//! gained. Nothing here is enforced on the door; that is step G3.
//!
//! Needs the conformance cluster. Its own databases, prefixed `ordo_cgrant_`.

use ordo_conformance::Pg;
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op::{self, ApplyRefused, Report, Would};
use ordo_host::consumer_grants::{
    self as cg, Expanded, CHANGES_NOTHING, MATCHES_NOTHING, PATTERN_INVALID,
    PROVIDER_NOT_IDENTITY_SERVICE,
};
use ordo_host::schema_apply::Program;

const OPS: &str = r#"
entity notes {
  title: text
}

op addNote($title: text) =
  set notes { title: $title }

op addAdmin($title: text) =
  set notes { title: $title }

op archiveNote($id: id) =
  del notes { id: $id }
"#;

fn v1(grants: &str) -> String {
    format!("domain notes @ 1.0.0\n\n{grants}\n{OPS}")
}

const VIEWER: &str = r#"
grant note_viewer {
  std.notes.get
  /add.*/
  !addAdmin
}

grant billing as itself {
  archiveNote
}
"#;

async fn fresh(stem: &str) -> Pool {
    let pg = Pg::scoped(stem);
    Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool")
}

async fn try_apply(pool: &Pool, source: &str) -> Result<Report, ApplyRefused> {
    let program = Program::compile(source).expect("compiles");
    ordo_host::rollouts::Rollouts::on(pool)
        .set_setting("rollout.strategy", Some("all"), "test", "at once", 0)
        .await
        .expect("rollout.strategy all");
    apply_op::run_program(
        pool,
        &SinglePool::new(pool.clone()),
        &program,
        vec![],
        "t",
        "t",
        true,
    )
    .await
}

fn has(notes: &[String], want: &str) -> bool {
    notes.iter().any(|n| n.contains(want))
}

#[tokio::test]
async fn a_grant_is_expanded_last_match_wins_and_stored_with_the_version() {
    let pool = fresh("ordo_cgrant_expand").await;
    let r = try_apply(&pool, &v1(VIEWER)).await.expect("applies");
    assert_eq!(r.version, "1.0.0");
    assert!(
        has(
            &r.notes,
            "note_viewer: 2 op(s), on behalf of a person: addNote, std.notes.get"
        ),
        "{:#?}",
        r.notes
    );
    assert!(
        has(
            &r.notes,
            "billing: 1 op(s), on behalf of a person or as itself: archiveNote"
        ),
        "{:#?}",
        r.notes
    );
    assert!(has(&r.notes, "note_viewer: new"), "{:#?}", r.notes);

    let stored = cg::stored(&pool, "1.0.0").await.expect("reads");
    assert_eq!(
        stored,
        [
            Expanded {
                name: "billing".into(),
                provider: false,
                as_itself: true,
                ops: vec!["archiveNote".into()],
            },
            Expanded {
                name: "note_viewer".into(),
                provider: false,
                as_itself: false,
                ops: vec!["addNote".into(), "std.notes.get".into()],
            },
        ]
    );

    // A later op that matches is granted by the pattern, and the plan says
    // which line granted it. `archiveNote` leaves billing's list.
    let v2 = format!(
        "{}\nop addReaction($title: text) =\n  set notes {{ title: $title }}\n",
        v1(&VIEWER.replace("  archiveNote", "  std.notes.list"))
    )
    .replace("@ 1.0.0", "@ 1.1.0");
    let r = try_apply(&pool, &v2).await.expect("applies");
    assert!(
        has(&r.notes, "note_viewer gains addReaction, matches /add.*/"),
        "{:#?}",
        r.notes
    );
    assert!(
        has(&r.notes, "billing gains std.notes.list, named"),
        "{:#?}",
        r.notes
    );
    assert!(has(&r.notes, "billing loses archiveNote"), "{:#?}", r.notes);
    let stored = cg::stored(&pool, "1.1.0").await.expect("reads");
    assert_eq!(
        stored[1].ops,
        ["addNote", "addReaction", "std.notes.get"],
        "{stored:#?}"
    );
    // The first version keeps its own list.
    assert_eq!(
        cg::stored(&pool, "1.0.0").await.expect("reads")[1].ops,
        ["addNote", "std.notes.get"]
    );

    // Taking a block away is shown too.
    let v3 = v1(&VIEWER.replace("grant billing as itself {\n  archiveNote\n}\n", ""))
        .replace("@ 1.0.0", "@ 1.2.0");
    let r = try_apply(&pool, &v3).await.expect("applies");
    assert!(
        has(&r.notes, "grant billing as itself: removed"),
        "{:#?}",
        r.notes
    );
    assert_eq!(cg::stored(&pool, "1.2.0").await.expect("reads").len(), 1);
}

/// Each refusal tries the forbidden block, and nothing is stored.
#[tokio::test]
async fn each_broken_block_refuses_the_apply_by_name() {
    let pool = fresh("ordo_cgrant_refuse").await;
    let cases: &[(&str, &str, &str)] = &[
        (
            "grant v {\n  /edit.*/\n}\n",
            MATCHES_NOTHING,
            "line 4 `/edit.*/`: no op of this program matches it",
        ),
        (
            "grant v {\n  std.notez.get\n}\n",
            MATCHES_NOTHING,
            "Fix: name an op this program declares",
        ),
        (
            "grant v {\n  addNote\n  /add.*/\n}\n",
            CHANGES_NOTHING,
            "every op it matches is decided by a later line (line 5 `/add.*/`)",
        ),
        ("grant v {\n  /(add/\n}\n", PATTERN_INVALID, "not a pattern"),
        (
            "grant provider hominem {\n  std.notes.get\n}\n",
            PROVIDER_NOT_IDENTITY_SERVICE,
            "ORDO_AUTH_HOMINEM_KIND is unset",
        ),
    ];
    for (block, class, text) in cases {
        let e = try_apply(&pool, &v1(block)).await.expect_err(block);
        assert_eq!(e.class(), "grant_refused", "{e}");
        let ApplyRefused::Grant(lines) = &e else {
            panic!("{e}")
        };
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with(class) && l.contains(text)),
            "{block}: {lines:#?}"
        );
        assert!(e.to_string().contains("so nothing was applied"), "{e}");
    }
    // Nothing was stored or activated.
    let active = ordo_host::versions::Versions::on(&pool)
        .active_version()
        .await
        .ok()
        .flatten();
    assert_eq!(active, None);
    assert!(cg::stored(&pool, "1.0.0").await.expect("reads").is_empty());
}

#[tokio::test]
async fn the_plan_shows_each_list_and_the_consumer_setting() {
    let pool = fresh("ordo_cgrant_plan").await;

    // The plan refuses what the apply would, and stores nothing.
    let p = apply_op::preview(&pool, &v1("grant v {\n  /edit.*/\n}\n"))
        .await
        .expect("previews");
    match &p.would {
        Would::Refuse(r) => assert_eq!(r.class(), "grant_refused", "{r}"),
        other => panic!("{other:?}"),
    }

    // A good program: the list, and the setting the door will read. The
    // test process does not set it, so the plan warns, naming it.
    let p = apply_op::preview(&pool, &v1(VIEWER))
        .await
        .expect("previews");
    assert!(matches!(p.would, Would::Create { .. }), "{:?}", p.would);
    assert!(
        has(&p.notes, "note_viewer: 2 op(s), on behalf of a person"),
        "{:#?}",
        p.notes
    );
    assert!(
        has(
            &p.notes,
            "warning     grant note_viewer: ORDO_CONSUMER_NOTE_VIEWER_CLIENT is not set"
        ),
        "{:#?}",
        p.notes
    );
    assert!(cg::stored(&pool, "1.0.0").await.expect("reads").is_empty());

    // Set, the plan shows its value. A provider grant whose connection is
    // the identity service passes; one to an outside OIDC provider does not.
    let src = v1(&format!(
        "{VIEWER}\ngrant provider hominem {{\n  std.notes.get\n}}\n"
    ));
    let program = Program::compile(&src).expect("compiles");
    let env = |pairs: Vec<(&'static str, &'static str)>| {
        move |k: &str| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    };
    let ids = env(vec![
        ("ORDO_CONSUMER_NOTE_VIEWER_CLIENT", "nv-123"),
        ("ORDO_AUTH_HOMINEM_KIND", "oidc"),
        ("ORDO_AUTH_HOMINEM_ISSUER", "https://auth.example.com"),
        ("ORDO_IDENTITY_ORIGIN", "https://auth.example.com"),
    ]);
    let r = cg::plan_with(&pool, &program.schema, &program.doc, &ids)
        .await
        .expect("plans");
    assert!(r.refusals.is_empty(), "{:#?}", r.refusals);
    assert!(
        has(
            &r.notes,
            "note_viewer: ORDO_CONSUMER_NOTE_VIEWER_CLIENT=nv-123"
        ),
        "{:#?}",
        r.notes
    );
    assert!(
        has(
            &r.notes,
            "hominem: 1 op(s), every deployment registered at this identity service"
        ),
        "{:#?}",
        r.notes
    );
    let outside = env(vec![
        ("ORDO_AUTH_HOMINEM_KIND", "oidc"),
        ("ORDO_AUTH_HOMINEM_ISSUER", "https://accounts.google.com"),
        ("ORDO_IDENTITY_ORIGIN", "https://auth.example.com"),
    ]);
    let r = cg::plan_with(&pool, &program.schema, &program.doc, &outside)
        .await
        .expect("plans");
    assert_eq!(r.refusals.len(), 1, "{:#?}", r.refusals);
    assert!(
        r.refusals[0].text.contains("is an outside OIDC provider"),
        "{}",
        r.refusals[0].text
    );
}

/// notes v2's own file with a grant to note_viewer added: it compiles, and
/// the block expands against the ops notes v2 really has.
#[test]
fn notes_v2_with_a_grant_to_note_viewer_compiles_and_expands() {
    let src = format!(
        "{}\ngrant note_viewer {{\n  std.notes.get\n  std.notes.list\n  /std\\.notesAccess\\.(get|list)/\n  /.*Note/\n  !deleteNote\n}}\n",
        include_str!("../../../examples/notes-v2/notes.ordo")
    );
    let program = Program::compile(&src).expect("notes v2 with a grant compiles");
    assert_eq!(program.schema.grants.len(), 1);
    let ops = cg::program_ops(&program.doc);
    let x = cg::expand(&program.schema.grants[0], &ops).expect("expands");
    let granted = &x.grant.ops;
    for want in [
        "std.notes.get",
        "std.notes.list",
        "std.notesAccess.get",
        "std.notesAccess.list",
        "createNote",
        "editNote",
        "archiveNote",
    ] {
        assert!(granted.iter().any(|o| o == want), "{want}: {granted:?}");
    }
    assert!(!granted.iter().any(|o| o == "deleteNote"), "{granted:?}");
    assert!(
        !granted.iter().any(|o| o == "std.notes.delete"),
        "{granted:?}"
    );
    // The artifact carries the block as written, so a changed grant is a new
    // version.
    assert_eq!(program.doc["grants"][0]["name"], "note_viewer");
}
