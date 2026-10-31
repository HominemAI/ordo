//! Checklist 10i, the half that writes — and **Y-013**, ruled.
//!
//! 10i landed `groupMembers` as a platform entity and taught the acl
//! derivation to emit the group disjunct. Nothing wrote the table. A group
//! could be granted access and could not be given members, so the second
//! disjunct was a semi-join against an empty relation: correct, disclosed in
//! the scope manifest, and inert. `docs/definition-language.md` §3 also names
//! a refusal class, `group_nesting`, that no code had ever produced.
//!
//! This suite executes both halves against real Postgres, and then rules the
//! question the glossary was standing in for with advice.
//!
//! # Y-013 — the more permissive level wins
//!
//! A principal can hold a level on a row **directly** and a *different* level
//! on the same row **through a group**. `docs/corpus-rulings.md` (2026-09-05)
//! takes the maximum, and the two orderings below are why: the alternative —
//! the lower level, or a refusal — makes *adding somebody to a group* able to
//! **reduce** their access, so a routine administrative act becomes a silent
//! revocation.
//!
//! Both orderings are asserted because a union is only symmetric if it
//! actually is one. `direct read + group admin` and `direct admin + group
//! read` must both write, and if only the first passes, the outcome was an
//! accident of which disjunct the planner reached first rather than the rule.
//! The `read + read` control is what makes those two mean anything: without
//! it, "Dov could write" is equally explained by the group disjunct admitting
//! anyone it names, at any level.

use ordo_commandlog::store::Store;
use ordo_commandlog::{Actor, SpaceId};
use ordo_conformance::{Fixture, GrantPath, GroupCommand, Pg, WriteLog};
use serde_json::{json, Value};

/// mem_o, the space owner — the actor every membership command below is
/// attributed to. Who *may* administer groups is a door question this
/// surface deliberately does not answer (see `membership.rs`); what it
/// enforces is that the actor is an attributed, non-guest member.
const ORA: Option<&str> = Some("mem_o");
/// mem_4 — Dov holds no `boardsAccess` row in the shipped seed, so every
/// level he reaches in this file is one this file gave him.
const DOV: Option<&str> = Some("mem_4");
/// mem_g — Gil is the fixture's guest: a members row with a **null** role.
const GIL: Option<&str> = Some("mem_g");

fn space() -> SpaceId {
    SpaceId(ordo_conformance::FIXTURE_SPACE.into())
}

/// The class of a harness failure — every entry point puts it first.
fn class_of(err: &str) -> &str {
    err.split(':').next().unwrap_or(err).trim()
}

/// A fresh database and a fresh seed for one case.
///
/// One per test function: `Fixture::reset` drops and rebuilds `public`, and
/// two of those at once race inside Postgres's own catalog.
fn seeded(f: &Fixture, name: &str) -> Pg {
    let pg = Pg::named(name);
    f.reset(&pg).expect("fixture DDL applies and seeds");
    pg
}

fn create_group(f: &Fixture, pg: &Pg, log: &mut WriteLog, name: &str) -> String {
    let out = f
        .group_command_as(pg, log, GroupCommand::Create { display_name: name }, ORA)
        .unwrap_or_else(|e| panic!("creating group `{name}`: {e}"));
    out["created"]
        .as_str()
        .expect("a create answers with the id it minted")
        .to_string()
}

fn add(
    f: &Fixture,
    pg: &Pg,
    log: &mut WriteLog,
    group: &str,
    member: &str,
) -> Result<Value, String> {
    f.group_command_as(pg, log, GroupCommand::Add { group, member }, ORA)
}

fn remove(
    f: &Fixture,
    pg: &Pg,
    log: &mut WriteLog,
    group: &str,
    member: &str,
) -> Result<Value, String> {
    f.group_command_as(pg, log, GroupCommand::Remove { group, member }, ORA)
}

/// Grant `member` `level` on `board_1`. An acl link row is an ordinary
/// derived entity row, so seeding one directly is the sanctioned setup
/// (DL §acl: *"seeding a link row is an ordinary write"*); the subject under
/// test is the membership surface, not the grant path.
fn grant(pg: &Pg, id: &str, member: &str, level: &str) {
    pg.exec(&format!(
        "INSERT INTO \"boardsAccess\" (\"$space\", \"id\", \"board\", \"member\", \"level\") \
         VALUES ('conformance', '{id}', 'board_1', '{member}', '{level}');"
    ))
    .expect("the acl row seeds");
}

/// Can `who` write a card on `board_1`? `cards` inherits `boards`' acl
/// guards, and the write rung of `acl (read, write, admin)` is `write` — so
/// this answers "does this caller hold `write` or `admin`, by any path".
fn can_write_card(f: &Fixture, pg: &Pg, who: Option<&str>) -> Result<Value, String> {
    let mut log = WriteLog::new();
    f.run_write_as(pg, &mut log, "finishCard($card: \"card_1\")", who)
}

fn card_titles(f: &Fixture, pg: &Pg, who: Option<&str>) -> Value {
    f.run_case_as(pg, "cardList", &[], who)
        .expect("cardList runs")
}

// ── the surface ──────────────────────────────────────────────────────────

/// The rows land, and they land as commands.
///
/// DL: *"Group membership changes are ordinary audited commands … Creating a
/// group, adding a member, removing one: three commands, three log entries."*
#[test]
fn the_surface_writes_membership_and_logs_it() {
    let f = Fixture::load().expect("fixture");
    let pg = seeded(&f, "ordo_grp_surface");
    let mut log = WriteLog::new();

    let g = create_group(&f, &pg, &mut log, "Contractors");
    add(&f, &pg, &mut log, &g, "mem_4").expect("Dov joins");
    add(&f, &pg, &mut log, &g, "mem_3").expect("Cleo joins");

    // The group row is a `members` row with `kind: group` — no new type,
    // nameable everywhere a principal is.
    let rows = pg
        .query(&format!(
            "SELECT \"kind\", \"displayName\", \"memberCount\", \"role\" FROM \"members\" \
             WHERE \"$space\" = 'conformance' AND \"id\" = '{g}';"
        ))
        .expect("the group row reads");
    assert_eq!(
        rows,
        vec![vec![
            Some("group".to_string()),
            Some("Contractors".to_string()),
            Some("2".to_string()),
            None,
        ]],
        "a group is a members row with `kind: group`, its given name, a \
         maintained count, and no role — a collection does not hold one"
    );

    // The link rows are real, walkable rows.
    let links = pg
        .query(&format!(
            "SELECT \"member\" FROM \"groupMembers\" WHERE \"$space\" = 'conformance' \
             AND \"group\" = '{g}' ORDER BY \"member\";"
        ))
        .expect("the link rows read");
    assert_eq!(
        links,
        vec![
            vec![Some("mem_3".to_string())],
            vec![Some("mem_4".to_string())]
        ],
    );

    // Three commands, three log entries — and every one attributed.
    assert_eq!(log.len(), 3, "one command group per membership command");
    for i in 0..3 {
        let entry = log.store.get(&space(), i).expect("a chained group");
        assert_eq!(
            entry.group.actor,
            Actor::User("mem_o".into()),
            "entry {i} names the administrator, not the platform"
        );
    }
    // The add carries the link row AND the count it changed: two rows moved,
    // so a revert of this group has both to put back.
    let add_entry = log.store.get(&space(), 1).expect("the first add");
    let touched: Vec<&str> = add_entry
        .group
        .commands
        .iter()
        .map(|c| c.entity.as_str())
        .collect();
    assert_eq!(touched, ["groupMembers", "members"]);
}

/// The finding 10i left standing, executed: a group grant conferred
/// **nothing** because the group could not be given members.
#[test]
fn a_group_grant_is_inert_until_the_group_has_members() {
    let f = Fixture::load().expect("fixture");
    let pg = seeded(&f, "ordo_grp_inert");
    let mut log = WriteLog::new();

    let g = create_group(&f, &pg, &mut log, "Reviewers");
    grant(&pg, "ba_grp", &g, "admin");

    // The acl row exists and names the group. Dov is not in it yet.
    assert_eq!(
        card_titles(&f, &pg, DOV),
        json!([]),
        "a grant to a group nobody is in shows nobody anything"
    );

    add(&f, &pg, &mut log, &g, "mem_4").expect("Dov joins Reviewers");
    assert_eq!(
        card_titles(&f, &pg, DOV),
        json!([
            {"title": "q3 goals", "done": false, "board": "board_1"},
            {"title": "q4 goals", "done": true,  "board": "board_1"},
        ]),
        "the second disjunct now has a row to walk"
    );
    can_write_card(&f, &pg, DOV).expect("and `admin` on the group confers write");

    // Removal is the revocation path DL names: *"Revoking group access for
    // one person is removing them from the group."*
    remove(&f, &pg, &mut log, &g, "mem_4").expect("Dov leaves");
    assert_eq!(
        card_titles(&f, &pg, DOV),
        json!([]),
        "and the walk stops finding him the moment the link row is gone"
    );
}

/// The tenant-defined-roles story end to end (`op-role-limit` ask 4): a
/// project admin grants a *group* through the derived grant op, and a
/// member of the group opens the row while a person outside it does not.
/// Bram is admin of `board_2`; Dov joins "Design"; Cleo has no grant on
/// `board_2` and is not in the group.
#[test]
fn a_group_member_opens_a_granted_resource_and_a_non_member_does_not() {
    let f = Fixture::load().expect("fixture");
    let pg = seeded(&f, "ordo_grp_resource");
    let mut log = WriteLog::new();
    const BRAM: Option<&str> = Some("mem_2");
    const CLEO: Option<&str> = Some("mem_3");

    let design = create_group(&f, &pg, &mut log, "Design");
    add(&f, &pg, &mut log, &design, "mem_4").expect("Dov joins Design");

    let someday = json!({"title": "someday", "done": false, "board": "board_2"});
    for who in [DOV, CLEO] {
        assert!(
            !card_titles(&f, &pg, who).to_string().contains("someday"),
            "before the grant, {who:?} does not see board_2"
        );
    }

    f.run_write_as(
        &pg,
        &mut log,
        &format!(r#"grantBoardsAccess($board: "board_2", $member: "{design}", $level: "read")"#),
        BRAM,
    )
    .expect("Bram is admin of board_2 and grants it to the group");

    assert!(
        card_titles(&f, &pg, DOV)
            .as_array()
            .expect("rows")
            .contains(&someday),
        "a member of Design opens the granted board"
    );
    assert!(
        !card_titles(&f, &pg, CLEO).to_string().contains("someday"),
        "a person outside Design does not"
    );
}

// ── Y-013 ────────────────────────────────────────────────────────────────

/// **The ruling, proven in rows, in both orderings.**
///
/// `acl (read, write, admin)`: the write rung is `write`, so `read` alone
/// refuses and `admin` by any path admits.
#[test]
fn the_more_permissive_level_wins_whichever_path_carries_it() {
    let f = Fixture::load().expect("fixture");

    // (a) direct `read`, group `admin`. The direct path is the weaker one.
    {
        let pg = seeded(&f, "ordo_grp_y013_a");
        let mut log = WriteLog::new();
        let g = create_group(&f, &pg, &mut log, "Admins");
        add(&f, &pg, &mut log, &g, "mem_4").expect("Dov joins");
        grant(&pg, "ba_direct", "mem_4", "read");
        grant(&pg, "ba_group", &g, "admin");
        can_write_card(&f, &pg, DOV).expect(
            "direct `read` + group `admin` writes: the maximum of the paths, not the \
             path that was named first",
        );
        assert_paths(&f, &pg, &g, "read", "admin", "admin");
    }

    // (b) the same two levels, the other way round. If only (a) passed, the
    // outcome would be an artifact of disjunct order, not the rule.
    {
        let pg = seeded(&f, "ordo_grp_y013_b");
        let mut log = WriteLog::new();
        let g = create_group(&f, &pg, &mut log, "Readers");
        add(&f, &pg, &mut log, &g, "mem_4").expect("Dov joins");
        grant(&pg, "ba_direct", "mem_4", "admin");
        grant(&pg, "ba_group", &g, "read");
        can_write_card(&f, &pg, DOV)
            .expect("direct `admin` + group `read` writes: a group never lowers a direct grant");
        assert_paths(&f, &pg, &g, "admin", "read", "admin");
    }

    // (c) the control. Both paths at `read` — everything the two cases above
    // had except a qualifying level anywhere. A guard that admitted this
    // would have admitted them for a reason that has nothing to do with the
    // ruling.
    {
        let pg = seeded(&f, "ordo_grp_y013_c");
        let mut log = WriteLog::new();
        let g = create_group(&f, &pg, &mut log, "Lurkers");
        add(&f, &pg, &mut log, &g, "mem_4").expect("Dov joins");
        grant(&pg, "ba_direct", "mem_4", "read");
        grant(&pg, "ba_group", &g, "read");
        let e = can_write_card(&f, &pg, DOV)
            .expect_err("`read` by both paths is still `read` — the max of two reads");
        assert_eq!(class_of(&e), "access_guard", "{e}");
        // …and he can still SEE the card, which is what makes the refusal
        // above an authorization answer rather than an invisible row.
        assert_eq!(card_titles(&f, &pg, DOV).as_array().map(Vec::len), Some(2));
        assert_paths(&f, &pg, &g, "read", "read", "read");
    }
}

/// The audit half of the ruling: **both** paths are reported, each named for
/// what it is, and the effective level is the maximum.
///
/// A report that showed only the winning path would be the silence the
/// ruling forbids — removing Dov's direct row is a no-op while the group
/// still holds `admin`, and an administrator who cannot see the group row
/// will believe they revoked something.
fn assert_paths(f: &Fixture, pg: &Pg, group: &str, direct: &str, via: &str, effective: &str) {
    let paths = f
        .grant_paths(pg, "boards", "board_1", "mem_4")
        .expect("the grant-path report runs");
    assert_eq!(
        paths,
        vec![
            GrantPath {
                row: "ba_direct".into(),
                level: direct.into(),
                via_group: None,
            },
            GrantPath {
                row: "ba_group".into(),
                level: via.into(),
                via_group: Some(group.to_string()),
            },
        ],
        "both paths, the group one attributed to the group that carries it"
    );
    assert_eq!(
        f.effective_level("boards", &paths).as_deref(),
        Some(effective),
        "the effective level is the maximum of the ladder positions"
    );
}

// ── the refusals ─────────────────────────────────────────────────────────

/// `group_nesting` — the class DL names and nothing produced.
#[test]
fn groups_may_not_contain_groups() {
    let f = Fixture::load().expect("fixture");
    let pg = seeded(&f, "ordo_grp_nesting");
    let mut log = WriteLog::new();

    let outer = create_group(&f, &pg, &mut log, "Everyone");
    let inner = create_group(&f, &pg, &mut log, "Finance");
    let before = log.len();

    let e = add(&f, &pg, &mut log, &outer, &inner).expect_err("a group may not join a group");
    assert_eq!(class_of(&e), "group_nesting", "{e}");

    assert_eq!(
        pg.query("SELECT \"id\" FROM \"groupMembers\" WHERE \"$space\" = 'conformance';")
            .expect("the link table reads"),
        Vec::<Vec<Option<String>>>::new(),
        "a refused nesting leaves no row behind"
    );
    assert_eq!(log.len(), before, "and no entry claiming it happened");

    // The refusal is about the MEMBER being a group, not about the pair: a
    // principal joins the same group in the same call shape.
    add(&f, &pg, &mut log, &outer, "mem_4").expect("a principal still joins");
}

/// Every other refusal, each with its class, each changing nothing.
#[test]
fn the_refusals_are_named_and_leave_no_trace() {
    let f = Fixture::load().expect("fixture");
    let pg = seeded(&f, "ordo_grp_refusals");
    let mut log = WriteLog::new();

    let g = create_group(&f, &pg, &mut log, "Ops");
    add(&f, &pg, &mut log, &g, "mem_4").expect("Dov joins");
    let settled = log.len();

    // Attribution. A committed group carries an `Actor` and there is no
    // anonymous one; a guest holds a members row with a null role.
    for actor in [None, GIL] {
        let e = f
            .group_command_as(
                &pg,
                &mut log,
                GroupCommand::Add {
                    group: &g,
                    member: "mem_3",
                },
                actor,
            )
            .expect_err("{actor:?} may not administer membership");
        assert_eq!(class_of(&e), "access_guard", "as {actor:?}: {e}");
    }

    // Addressing. All one message, byte-identical (R-G10): "that id is a
    // person, not a group" is an existence answer about a row.
    for (group, member, why) in [
        ("mem_nope", "mem_3", "a group id that names nothing"),
        ("mem_1", "mem_3", "a members row that is not a group"),
        (g.as_str(), "mem_nope", "a member who is not in this space"),
    ] {
        let e = add(&f, &pg, &mut log, group, member).expect_err(why);
        assert_eq!(class_of(&e), "not_found", "{why}: {e}");
        assert_eq!(e, ordo_conformance::write::NOT_FOUND, "{why}");
    }

    // One row per (group, member) — so that a removal removes all of it.
    let e = add(&f, &pg, &mut log, &g, "mem_4").expect_err("Dov is already in Ops");
    assert_eq!(class_of(&e), "duplicate", "{e}");

    // Removing a link that is not there.
    let e = remove(&f, &pg, &mut log, &g, "mem_3").expect_err("Cleo was never in Ops");
    assert_eq!(class_of(&e), "not_found", "{e}");

    assert_eq!(log.len(), settled, "no refusal appended an entry");
    assert_eq!(
        pg.query("SELECT count(*) FROM \"groupMembers\" WHERE \"$space\" = 'conformance';")
            .expect("count reads"),
        vec![vec![Some("1".to_string())]],
        "and none of them wrote a row"
    );
}

// ── counting ─────────────────────────────────────────────────────────────

/// `memberCount` is maintained by the surface, and is the **upper bound** DL
/// says it is — not a headcount.
#[test]
fn member_count_is_maintained_and_is_honestly_an_upper_bound() {
    let f = Fixture::load().expect("fixture");
    let pg = seeded(&f, "ordo_grp_count");
    let mut log = WriteLog::new();

    let count_of = |id: &str| -> i64 {
        pg.query(&format!(
            "SELECT \"memberCount\" FROM \"members\" WHERE \"$space\" = 'conformance' \
             AND \"id\" = '{id}';"
        ))
        .expect("count reads")[0][0]
            .as_deref()
            .expect("not null")
            .parse()
            .expect("an int")
    };

    let g = create_group(&f, &pg, &mut log, "Finance");
    assert_eq!(count_of(&g), 0, "a fresh group has no members");
    add(&f, &pg, &mut log, &g, "mem_4").expect("Dov joins");
    add(&f, &pg, &mut log, &g, "mem_3").expect("Cleo joins");
    assert_eq!(count_of(&g), 2);
    remove(&f, &pg, &mut log, &g, "mem_3").expect("Cleo leaves");
    assert_eq!(count_of(&g), 1, "and the count follows the removal");
    assert_eq!(count_of("mem_4"), 1, "a principal counts as one, always");

    // The stated limitation, executed. Dov holds `board_1` directly AND
    // through Finance, so `sum member.memberCount` over the board's acl rows
    // counts him twice: two acl rows, one person.
    grant(&pg, "ba_direct", "mem_4", "read");
    grant(&pg, "ba_group", &g, "write");
    let seats: i64 = pg
        .query(
            "SELECT sum(m.\"memberCount\") FROM \"boardsAccess\" a \
             JOIN \"members\" m ON m.\"$space\" = a.\"$space\" AND m.\"id\" = a.\"member\" \
             WHERE a.\"$space\" = 'conformance' AND a.\"board\" = 'board_1';",
        )
        .expect("the seat sum reads")[0][0]
        .as_deref()
        .expect("not null")
        .parse()
        .expect("an int");
    // board_1's shipped rows: mem_1, mem_2, mem_3, mem_g — four principals,
    // one each. Plus Dov's direct row (1) and Finance (1, holding only Dov).
    assert_eq!(seats, 6, "the sum");
    let people: i64 = pg
        .query(&format!(
            "SELECT count(DISTINCT p) FROM ( \
               SELECT a.\"member\" AS p FROM \"boardsAccess\" a \
                 JOIN \"members\" m ON m.\"$space\" = a.\"$space\" AND m.\"id\" = a.\"member\" \
                 WHERE a.\"$space\" = 'conformance' AND a.\"board\" = 'board_1' \
                   AND m.\"kind\" = 'principal' \
               UNION ALL \
               SELECT gm.\"member\" AS p FROM \"boardsAccess\" a \
                 JOIN \"groupMembers\" gm ON gm.\"$space\" = a.\"$space\" \
                   AND gm.\"group\" = a.\"member\" \
                 WHERE a.\"$space\" = 'conformance' AND a.\"board\" = 'board_1' \
             ) u WHERE p IS NOT NULL AND p <> '{g}';"
        ))
        .expect("the distinct headcount reads")[0][0]
        .as_deref()
        .expect("not null")
        .parse()
        .expect("an int");
    assert_eq!(
        people, 5,
        "five humans hold board_1: the sum is an upper bound, and the exact answer is \
         the two-source `distinct` pipeline DL prescribes — which is expressible \
         only because `groupMembers` is an ordinary queryable entity and nesting is \
         refused, so one expansion suffices"
    );
    assert!(
        seats > people,
        "the documented gap is real, not theoretical"
    );
}
