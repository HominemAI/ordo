//! What the reverse compiler EMITS, asserted on the text.
//!
//! The round-trip suite next door is the row oracle: it runs both sides and
//! demands the same rows. This one is the shape oracle, and it needs no
//! database, so it runs everywhere and it fails on the exact line that
//! changed. The two catch different things — a divergence the corpus does
//! not reach (the corpus has no `a AND b OR c`, and no `<>` against a
//! nullable column) is invisible to a row oracle until somebody writes the
//! query.
//!
//! Every case here is either a decided translation or a named refusal.
//! There is deliberately no "and this is what it happens to print today"
//! case: pinning arbitrary output is how a suite starts defending bugs.

use ordo_conformance::Fixture;
use ordo_playground::decompile::decompile;

fn wide() -> Fixture {
    Fixture::load_wide().expect("wide fixture compiles")
}

fn ordo(sql: &str) -> String {
    let f = wide();
    decompile(&f.schema, sql)
        .unwrap_or_else(|r| panic!("refused `{sql}`: {}", r.what))
        .ordo
}

fn notes(sql: &str) -> Vec<String> {
    let f = wide();
    decompile(&f.schema, sql).expect("decompiles").notes
}

fn refusal(sql: &str) -> String {
    let f = wide();
    match decompile(&f.schema, sql) {
        Err(r) => r.what,
        Ok(d) => panic!("`{sql}` should have been refused, got:\n{}", d.ordo),
    }
}

/// Whatever it emits must at least be Ordo. The old emitter produced
/// `lead_name: lead.name` for a snake_case alias — text that only the
/// forward compiler, two steps later, would reject.
fn compiles(sql: &str) -> String {
    let f = wide();
    let d = decompile(&f.schema, sql).unwrap_or_else(|r| panic!("refused `{sql}`: {}", r.what));
    if let Err(e) = f.compile(&d.ordo) {
        panic!(
            "`{sql}`\n  decompiled to: {}\n  which does not compile: {e}",
            d.ordo
        );
    }
    d.ordo
}

// ── connectives: SQL has a precedence table, Ordo has none ───────────────

#[test]
fn and_binds_tighter_than_or() {
    // SQL reads this as `(a AND b) OR c`. It decompiled to
    // `a and (b or c)` — a different predicate, emitted silently.
    assert_eq!(
        ordo("SELECT name FROM people WHERE active = true AND age > 40 OR salary > 100"),
        "people((active == true and age > 40) or salary > 100)[name]"
    );
}

#[test]
fn or_of_ands_parenthesises_every_arm() {
    assert_eq!(
        ordo("SELECT name FROM people WHERE age > 1 AND age < 2 OR age > 3 AND age < 4"),
        "people((age > 1 and age < 2) or (age > 3 and age < 4))[name]"
    );
}

#[test]
fn parenthesised_sql_keeps_its_grouping() {
    // And is expressible at all: `(a OR b) AND c` was refused outright,
    // because the condition parser had no rule for `(`.
    assert_eq!(
        ordo("SELECT name FROM people WHERE (active = true OR age > 40) AND salary > 100"),
        "people((active == true or age > 40) and salary > 100)[name]"
    );
}

#[test]
fn same_connective_chains_stay_flat() {
    // Ordo accepts `a and b and c`; parenthesising it would be noise.
    assert_eq!(
        ordo("SELECT name FROM people WHERE age > 1 AND age < 9 AND active = true"),
        "people(age > 1 and age < 9 and active == true)[name]"
    );
}

#[test]
fn mixed_connectives_always_compile() {
    // Ordo has no precedence table and refuses unparenthesised mixing, so
    // the emitter's parentheses are load-bearing, not cosmetic.
    compiles("SELECT name FROM people WHERE active = true AND age > 40 OR salary > 100");
    compiles("SELECT name FROM people WHERE (active = true OR age > 40) AND salary > 100");
}

// ── null logic: three-valued SQL against two-valued Ordo ─────────────────

#[test]
fn inequality_on_a_nullable_column_carries_its_null_guard() {
    // `nickname` is `text?`. SQL's `<>` drops the null rows; Ordo's `!=`
    // compiles to `IS DISTINCT FROM` and keeps them. One row against five,
    // on the shipped seed.
    assert_eq!(
        ordo("SELECT name FROM people WHERE nickname <> 'B'"),
        "people(nickname != \"B\" and nickname is not null)[name]"
    );
    assert_eq!(
        ordo("SELECT name FROM people WHERE nickname = 'B'"),
        "people(nickname == \"B\" and nickname is not null)[name]"
    );
}

#[test]
fn a_non_nullable_column_gets_no_guard() {
    // The bridge is emitted where it changes rows and nowhere else — a
    // guard on every comparison would be unreadable and would say the
    // schema does not know its own nullability.
    assert_eq!(
        ordo("SELECT name FROM people WHERE name <> 'Ada'"),
        "people(name != \"Ada\")[name]"
    );
    assert!(notes("SELECT name FROM people WHERE name <> 'Ada'").is_empty());
}

#[test]
fn column_to_column_guards_each_nullable_side_once() {
    // `projects.lead` is required, `projects.sponsor` optional.
    assert_eq!(
        ordo("SELECT name FROM projects WHERE lead <> sponsor"),
        "projects(lead != sponsor and sponsor is not null)[name]"
    );
    // The same path on both sides is one guard, not two.
    assert_eq!(
        ordo("SELECT name FROM people WHERE nickname = nickname"),
        "people(nickname == nickname and nickname is not null)[name]"
    );
}

#[test]
fn ordering_comparisons_need_no_guard() {
    // `<` `<=` `>` `>=` lower to `COALESCE(…, false)`, which is already
    // SQL's own null reading.
    assert_eq!(
        ordo("SELECT name FROM people WHERE age > 40"),
        "people(age > 40)[name]"
    );
}

#[test]
fn a_null_guard_is_disclosed_in_a_note() {
    let n = notes("SELECT name FROM people WHERE nickname <> 'B'");
    assert_eq!(n.len(), 1, "expected exactly one note, got {n:?}");
    assert!(n[0].contains("DISTINCT FROM"), "{}", n[0]);
}

#[test]
fn a_guarded_atom_inside_an_or_is_parenthesised() {
    assert_eq!(
        ordo("SELECT name FROM people WHERE nickname <> 'B' OR age > 40"),
        "people((nickname != \"B\" and nickname is not null) or age > 40)[name]"
    );
    compiles("SELECT name FROM people WHERE nickname <> 'B' OR age > 40");
}

#[test]
fn is_null_is_already_two_valued_and_untouched() {
    assert_eq!(
        ordo("SELECT name FROM people WHERE nickname IS NOT NULL"),
        "people(nickname is not null)[name]"
    );
}

#[test]
fn in_needs_no_guard() {
    // `x in (…)` lowers to `COALESCE(x IN (…), false)` — SQL's reading.
    assert_eq!(
        ordo("SELECT name FROM people WHERE age IN (34, 45)"),
        "people(age in (34, 45))[name]"
    );
}

// ── names ───────────────────────────────────────────────────────────────

#[test]
fn a_snake_case_alias_is_refused_here_not_downstream() {
    let r = refusal(
        "SELECT ld.name AS lead_name FROM projects p LEFT JOIN people ld ON p.lead = ld.id",
    );
    assert!(r.contains("lowerCamel"), "{r}");
    // SQL aliases are conventionally snake_case, so this is the common
    // case, not an exotic one.
    assert!(refusal("SELECT count(*) AS row_count FROM people").contains("lowerCamel"));
}

#[test]
fn a_lower_camel_alias_is_accepted() {
    assert_eq!(
        ordo("SELECT ld.name AS leadName FROM projects p LEFT JOIN people ld ON p.lead = ld.id"),
        "projects[leadName: lead.name]"
    );
}

// ── grouping ────────────────────────────────────────────────────────────

#[test]
fn a_renamed_group_key_is_refused_rather_than_silently_dropped() {
    // `AS t` was discarded and the column came back named `team`.
    let r = refusal("SELECT p.team AS t, count(*) AS n FROM people p GROUP BY p.team");
    assert!(r.contains("group key cannot be renamed"), "{r}");
}

#[test]
fn a_selected_non_key_is_refused() {
    let r = refusal("SELECT p.name, count(*) AS n FROM people p GROUP BY p.team");
    assert!(r.contains("not a GROUP BY key"), "{r}");
}

#[test]
fn an_unprojected_group_key_moves_into_a_let() {
    // Ordo's `group` always projects its keys, so this used to come back
    // with a column the SQL does not have — disclosed in a note, which is
    // not the same thing as translated. A `let` binds the grouped pipeline
    // and the outer line projects exactly the SELECT's list.
    let sql = "SELECT count(*) AS n FROM people p GROUP BY p.team";
    assert_eq!(
        ordo(sql),
        "let grouped = people\ngroup team { n: count }\n\ngrouped[n]"
    );
    assert!(
        notes(sql).iter().any(|n| n.contains("does not project")),
        "{:?}",
        notes(sql)
    );
    compiles(sql);
}

#[test]
fn a_group_whose_select_order_differs_moves_into_a_let_too() {
    // `group` projects its keys FIRST. `SELECT count(*) AS n, team` asks for
    // the other order, and column order is not something a row-multiset
    // oracle can see — so it is asserted here.
    assert_eq!(
        compiles("SELECT count(*) AS n, p.team FROM people p GROUP BY p.team"),
        "let grouped = people\ngroup team { n: count }\n\ngrouped[n, team]"
    );
}

#[test]
fn a_group_that_projects_its_keys_in_order_needs_no_let() {
    // The `let` is machinery for a mismatch. Where there is none, the plain
    // form stays — it is the readable one, and it is what the corpus's
    // W013/W014/W015 round-trip through.
    assert_eq!(
        ordo("SELECT p.team, count(*) AS n FROM people p GROUP BY p.team"),
        "people\ngroup team { n: count }"
    );
}

#[test]
fn an_order_by_after_a_group_names_the_grouped_columns() {
    // `sort assignee.team` after `group assignee.team` is a bind error: the
    // grouped row has a `team`, not an `assignee`. The emitter used to
    // render the path and let the forward compiler complain about source
    // the user never wrote.
    assert_eq!(
        compiles(
            "SELECT a.team, count(*) AS n FROM tasks t LEFT JOIN people a ON t.assignee = a.id \
             GROUP BY a.team ORDER BY a.team"
        ),
        "tasks\ngroup assignee.team { n: count }\nsort team"
    );
    let r = refusal("SELECT p.team, count(*) AS n FROM people p GROUP BY p.team ORDER BY p.name");
    assert!(r.contains("the grouped rows do not have"), "{r}");
}

#[test]
fn group_with_no_aggregate_is_the_distinct_shape() {
    assert_eq!(
        compiles("SELECT p.team FROM people p GROUP BY p.team"),
        "people\ngroup team { }"
    );
}

#[test]
fn count_of_a_column_becomes_the_non_null_spelling() {
    // Bare `count` counts ROWS; `count(col)` counts non-null values.
    let sql = "SELECT p.team, count(p.age) AS n FROM people p GROUP BY p.team";
    assert_eq!(
        ordo(sql),
        "people\ngroup team { n: sum cond(age is null -> 0, 1) }"
    );
    assert!(notes(sql).iter().any(|n| n.contains("non-null")));
}

#[test]
fn an_ungrouped_count_of_a_column_is_refused() {
    // `sum cond(x is null -> 0, 1)` is an exact spelling of `count(x)` only
    // where the input cannot be empty. Inside a `group` every bucket holds a
    // row, so it is exact. Ungrouped it is not: `sum` of NO rows is null and
    // SQL's `count` of no rows is 0, so `SELECT count(nickname) FROM people
    // WHERE age > 1000` came back `null` where SQL says `0` — a different
    // VALUE, run on Postgres, not a rendering difference. Ordo's `reduce`
    // braces admit aggregates only, so there is no `coalesce(sum …, 0)` to
    // write; the honest answer is a refusal.
    let r = refusal("SELECT count(p.age) AS n FROM people p");
    assert!(r.contains("`sum` over NO rows is null"), "{r}");
    assert!(r.contains("count(*)"), "{r}");
    // Grouped, the same aggregate is exact and still emitted.
    assert_eq!(
        ordo("SELECT p.team, count(p.age) AS n FROM people p GROUP BY p.team"),
        "people\ngroup team { n: sum cond(age is null -> 0, 1) }"
    );
    // And `count(*)` is the row count, which Ordo's bare `count` is,
    // empty input included.
    assert_eq!(
        ordo("SELECT count(*) AS n FROM people p"),
        "people\nreduce { n: count }"
    );
}

// ── joins ───────────────────────────────────────────────────────────────

#[test]
fn a_ref_hop_join_becomes_a_dot_path() {
    assert_eq!(
        ordo(
            "SELECT p.name, ld.name AS leadName FROM projects p \
             LEFT JOIN people ld ON p.lead = ld.id"
        ),
        "projects[name, leadName: lead.name]"
    );
}

#[test]
fn two_paths_that_would_share_an_output_name_are_refused() {
    // `SELECT p.name, sp.name` is two columns in SQL, both called `name`.
    // Ordo emits `[name, sponsor.name]`, the plan emits two `AS "name"`
    // columns, and the reader gets ONE — the sponsor's, standing where the
    // project's was. The forward compiler does not catch it.
    let r =
        refusal("SELECT p.name, sp.name FROM projects p LEFT JOIN people sp ON p.sponsor = sp.id");
    assert!(r.contains("both be called `name`"), "{r}");
    // The same collision through two GROUP BY keys with the same leaf.
    let r = refusal(
        "SELECT p.name, sp.name, count(*) AS n FROM projects p \
         LEFT JOIN people sp ON p.sponsor = sp.id GROUP BY p.name, sp.name",
    );
    assert!(r.contains("both be called `name`"), "{r}");
}

#[test]
fn an_inner_join_becomes_the_filter_that_it_is() {
    // A dot-path keeps the rows an INNER JOIN drops, and this was a note.
    // It does not have to be: the join is `fk = joined.id` against a
    // declared ref, so the foreign key makes `fk is not null` exactly the
    // rows the join drops — no duplication, no reordering.
    assert_eq!(
        ordo("SELECT p.name FROM projects p JOIN people sp ON p.sponsor = sp.id"),
        "projects(sponsor is not null)[name]"
    );
    // It composes with an existing WHERE, through the same
    // precedence-correct renderer.
    assert_eq!(
        ordo(
            "SELECT p.name FROM projects p JOIN people sp ON p.sponsor = sp.id \
             WHERE p.shipped = true OR p.budget > 1"
        ),
        "projects((shipped == true or budget > 1) and sponsor is not null)[name]"
    );
    assert_eq!(
        ordo(
            "SELECT p.name FROM projects p JOIN people sp ON p.sponsor = sp.id \
             WHERE p.shipped = true AND p.budget > 1"
        ),
        "projects(shipped == true and budget > 1 and sponsor is not null)[name]"
    );
}

#[test]
fn an_inner_join_on_a_required_ref_adds_nothing() {
    // `projects.lead` is not optional, so the inner join drops no rows and
    // a conjunct would be noise claiming otherwise.
    let sql = "SELECT p.name FROM projects p JOIN people ld ON p.lead = ld.id";
    assert_eq!(ordo(sql), "projects[name]");
    assert!(notes(sql).is_empty(), "{:?}", notes(sql));
}

#[test]
fn the_collection_direction_is_refused_by_name() {
    let r = refusal("SELECT o.name FROM orgs o LEFT JOIN teams t ON o.id = t.org");
    assert!(r.contains("only fk = joined.id joins decompile"), "{r}");
}

// ── the edges of the subset, each named ─────────────────────────────────

#[test]
fn refusals_say_what_they_refused() {
    for (sql, want) in [
        ("SELECT name FROM people HAVING count(*) > 1", "HAVING"),
        ("UPDATE people SET name = 'x'", "only SELECT"),
        ("SELECT name FROM widgets", "is not an entity"),
        (
            "SELECT name FROM people WHERE age > -1",
            "outside the v0 subset",
        ),
        (
            "SELECT name FROM people WHERE (age > 1",
            "closing a parenthesised",
        ),
        ("SELECT count(*) FROM people", "aggregate needs an AS name"),
    ] {
        let r = refusal(sql);
        assert!(r.contains(want), "`{sql}`\n  wanted {want:?}\n  got: {r}");
    }
}

#[test]
fn limit_without_order_says_the_two_are_not_the_same_question() {
    let n = notes("SELECT name FROM people LIMIT 2");
    assert!(n.iter().any(|x| x.contains("arbitrary")), "{n:?}");
}

#[test]
fn select_star_is_refused_rather_than_narrowed() {
    // It used to become the bare source with a note listing what had been
    // dropped. `SELECT * FROM people` is nineteen physical columns and the
    // bare source is twelve declared open fields: not a translation with a
    // caveat, a different query. The refusal hands back the explicit
    // projection that does translate.
    let r = refusal("SELECT * FROM orgs");
    assert!(
        r.contains("SELECT name, region, active, founded, seats"),
        "{r}"
    );
    let r = refusal("SELECT * FROM people");
    assert!(r.contains("ciphertext"), "{r}");
    assert!(r.contains("`email`"), "{r}");
}

// ── ordering: null placement, and the collation that stays a note ───────

#[test]
fn nulls_first_is_spelled_out_rather_than_silently_dropped() {
    // Postgres sorts nulls FIRST under DESC. Ordo's `sort` is always NULLS
    // LAST, so `sort -nickname` is a different ordering — and under a LIMIT,
    // a different set of rows. `cond(k is null -> 0, 1)` is the spelling.
    assert_eq!(
        ordo("SELECT name FROM people ORDER BY nickname DESC"),
        "people[name]\nsort cond(nickname is null -> 0, 1), -nickname"
    );
    // Explicit placement was parsed and thrown away, so `ASC NULLS FIRST`
    // and plain `ASC` decompiled identically.
    assert_eq!(
        ordo("SELECT name FROM people ORDER BY nickname ASC NULLS FIRST"),
        "people[name]\nsort cond(nickname is null -> 0, 1), nickname"
    );
    // NULLS LAST is what Ordo already does, either way round.
    assert_eq!(
        ordo("SELECT name FROM people ORDER BY nickname DESC NULLS LAST"),
        "people[name]\nsort -nickname"
    );
    // A required column has no nulls to place.
    assert_eq!(
        ordo("SELECT name FROM people ORDER BY name DESC"),
        "people[name]\nsort -name"
    );
}

#[test]
fn only_the_from_tables_own_trailing_id_is_the_implicit_tiebreak() {
    // Ordo appends an implicit `id` tiebreak of its own — the SOURCE
    // entity's id — so a trailing `ORDER BY …, t.id` is dropped rather than
    // restated.
    assert_eq!(
        ordo("SELECT name FROM people p ORDER BY p.name, p.id"),
        "people[name]\nsort name"
    );
    assert_eq!(
        ordo("SELECT name FROM people ORDER BY name, id"),
        "people[name]\nsort name"
    );
    // But the test for that was `col == "id"` with no look at the ALIAS,
    // which also swallowed a JOINED side's id: `ORDER BY a.name, a.id`
    // dropped the assignee's id and left a note saying Ordo appends it
    // itself, when what Ordo appends is the TASK's id. A different
    // tiebreak, and under a LIMIT a different set of rows. A joined-side id
    // is an ordinary sort key and is kept.
    let sql = "SELECT t.title FROM tasks t LEFT JOIN people a ON t.assignee = a.id \
               ORDER BY a.name, a.id";
    assert_eq!(ordo(sql), "tasks[title]\nsort assignee.name, assignee.id");
    assert!(
        !notes(sql).iter().any(|n| n.contains("tiebreak — Ordo")),
        "the joined id was not dropped, so nothing should claim it was: {:?}",
        notes(sql)
    );
    // A DESCENDING trailing own-id is a real key too: Ordo's implicit
    // tiebreak is ascending.
    assert_eq!(
        ordo("SELECT name FROM people p ORDER BY p.name, p.id DESC"),
        "people[name]\nsort name, -id"
    );
}

#[test]
fn offset_is_refused_by_name_and_says_what_ordo_does_instead() {
    // It was refused as `trailing tokens outside the v0 subset:
    // Some(Word("OFFSET"))` — true, and useless. Ordo pages with a cursor,
    // so there is no emitter spelling for a skip count and the reader should
    // be told which word was the problem and why.
    for sql in [
        "SELECT name FROM people ORDER BY id LIMIT 2 OFFSET 2",
        "SELECT name FROM people LIMIT 2 OFFSET 2",
        "SELECT name FROM people OFFSET 2",
    ] {
        let r = refusal(sql);
        assert!(r.contains("`OFFSET` has no Ordo spelling"), "{sql}: {r}");
        assert!(r.contains("CURSOR"), "{sql}: {r}");
    }
}

#[test]
fn a_subquery_is_refused_rather_than_flattened() {
    // The v0 subset is one SELECT over one FROM table. A correlated EXISTS
    // is exactly the shape a guard compiles TO, so an emitter that flattened
    // one would be inventing a join — refused instead.
    for sql in [
        "SELECT name FROM people p WHERE EXISTS (SELECT 1 FROM tasks t WHERE t.assignee = p.id)",
        "SELECT name FROM people WHERE id IN (SELECT assignee FROM tasks)",
        "SELECT name FROM (SELECT * FROM people) q",
    ] {
        refusal(sql);
    }
}

#[test]
fn a_text_ordering_discloses_the_collation_pin() {
    // Ordo pins `COLLATE "C"`; the database's own collation decides the SQL
    // side. No Ordo spelling for another collation exists, so this is
    // disclosed rather than refused — but never omitted.
    let n = notes("SELECT name FROM people ORDER BY name");
    assert!(n.iter().any(|x| x.contains("COLLATE")), "{n:?}");
    // A numeric ordering takes no pin and gets no note.
    let n = notes("SELECT name FROM people ORDER BY salary");
    assert!(!n.iter().any(|x| x.contains("COLLATE")), "{n:?}");
}

// ── literals: the emitter is quoting, not re-lexing ─────────────────────

#[test]
fn a_string_literal_is_escaped_for_ordos_lexer() {
    // Ordo's closed escape set is `\" \\ \n \t`. Emitting the SQL literal
    // verbatim meant `'a\nb'` — a backslash and an `n` — became a NEWLINE,
    // a different value compared in silence.
    assert_eq!(
        ordo("SELECT name FROM people WHERE name = 'a\\nb'"),
        "people(name == \"a\\\\nb\")[name]"
    );
    // And `'x\"y'` produced `\"x\"y\"`, which is not a string at all — the
    // refusal arrived from the forward compiler, worded about source the
    // user never wrote.
    assert_eq!(
        ordo("SELECT name FROM people WHERE nickname <> 'x\"y'"),
        "people(nickname != \"x\\\"y\" and nickname is not null)[name]"
    );
    // The same path for the text operators.
    assert_eq!(
        ordo("SELECT name FROM people WHERE strpos(name, 'a\\b') > 0"),
        "people(name contains \"a\\\\b\")[name]"
    );
}

#[test]
fn a_temporal_literal_outside_ordos_two_shapes_is_refused_here() {
    // Postgres takes `'2026-05-01 09:00:00+00'`; Ordo's datetime is
    // `YYYY-MM-DDThh:mm:ssZ` and the Z is mandatory.
    let r = refusal(
        "SELECT name FROM people WHERE \"lastSeen\" >= TIMESTAMPTZ '2026-05-01 09:00:00+00'",
    );
    assert!(r.contains("not an Ordo temporal literal"), "{r}");
    // The two that are legal stay legal.
    ordo("SELECT name FROM people WHERE hired >= DATE '2021-01-01'");
    ordo("SELECT name FROM people WHERE \"lastSeen\" >= TIMESTAMPTZ '2026-01-01T00:00:00Z'");
}

#[test]
fn a_column_that_is_not_an_ordo_path_segment_is_refused_here() {
    // `render_col` emitted whatever the SQL said, so a platform column came
    // out as the path segment `$space`.
    let r = refusal("SELECT \"$space\" FROM people");
    assert!(r.contains("not a legal Ordo path segment"), "{r}");
}

// ── literal columns ─────────────────────────────────────────────────────

#[test]
fn a_literal_select_column_is_a_literal_projection_entry() {
    // `'order' AS kind` is `kind: "order"` — the forward compiler takes a
    // literal projection entry and plans it as `'order' AS "kind"`, the same
    // constant column the SQL emits.
    assert_eq!(
        compiles("SELECT 'project' AS kind, p.name FROM projects p"),
        "projects[kind: \"project\", name]"
    );
    // Every literal kind SQL can put in a select list, each accepted by the
    // forward compiler in a projection and each checked on Postgres in
    // `differential.rs` rather than assumed here.
    assert_eq!(
        compiles("SELECT 5 AS n, true AS b, NULL AS z, p.name FROM projects p"),
        "projects[n: 5, b: true, z: null, name]"
    );
}

#[test]
fn a_literal_column_is_quoted_for_ordos_lexer_not_re_lexed() {
    // The same closed escape set the condition literals go through — this is
    // the one path where a value the user typed becomes a VALUE RETURNED, so
    // a re-lexing bug here is a wrong answer rather than a wrong filter.
    // SQL doubles a quote to escape it; Ordo's strings are double-quoted and
    // a single quote needs nothing.
    assert_eq!(
        compiles("SELECT 'a''b' AS kind FROM projects p"),
        "projects[kind: \"a'b\"]"
    );
    assert_eq!(
        compiles("SELECT 'x\"y' AS kind FROM projects p"),
        "projects[kind: \"x\\\"y\"]"
    );
    assert_eq!(
        compiles("SELECT 'a\\nb' AS kind FROM projects p"),
        "projects[kind: \"a\\\\nb\"]"
    );
}

#[test]
fn a_literal_without_an_as_name_is_refused_rather_than_named_for_you() {
    // SQL calls it `?column?`. An Ordo projection entry is `name: value` and
    // has no unnamed form, so any name the emitter picked would be a name
    // the SQL did not produce — a different column set, silently.
    let r = refusal("SELECT 'project' FROM projects p");
    assert!(r.contains("literal column needs an `AS` name"), "{r}");
    // …except in a later union arm, where SQL takes the name from the first
    // SELECT and one is not expected. See the union tests below.
    assert!(refusal("SELECT 'project' AS project_kind FROM projects p").contains("lowerCamel"));
}

// ── UNION ALL is `append` ───────────────────────────────────────────────

#[test]
fn the_many_branch_feed_decompiles_to_an_append() {
    // The shape the owner brought: a literal discriminator, renamed columns
    // per arm, a per-arm WHERE, and one ORDER BY / LIMIT over the whole
    // union. It used to come back as "expected an identifier, found
    // Str(\"project\")".
    assert_eq!(
        compiles(
            "SELECT 'project' AS kind, p.name AS title, p.\"startsOn\" AS at FROM projects p WHERE p.shipped = false \
             UNION ALL \
             SELECT 'task', t.title, t.due FROM tasks t WHERE t.done = false \
             ORDER BY at DESC LIMIT 100"
        ),
        "projects(shipped == false)[kind: \"project\", title: name, at: startsOn]\n\
         append tasks(done == false)[kind: \"task\", title, at: due]\n\
         sort cond(at is null -> 0, 1), -at\n\
         take 100"
    );
}

#[test]
fn later_arms_are_renamed_onto_the_first_arms_names() {
    // SQL names a union's columns from the FIRST SELECT and matches the rest
    // by POSITION; Ordo's `append` matches by NAME. Emitting each later arm
    // under the first arm's names is not an approximation of SQL's rule, it
    // is SQL's rule — position k of arm 2 carries arm 1's name for k.
    let sql = "SELECT p.name AS label FROM projects p UNION ALL SELECT t.title FROM tasks t";
    assert_eq!(
        compiles(sql),
        "projects[label: name]\nappend tasks[label: title]"
    );
    // A later arm that gave its own, DIFFERENT name is worth a sentence:
    // SQL discards that name and Ordo would otherwise match on it.
    let sql = "SELECT p.name AS label FROM projects p \
               UNION ALL SELECT t.title AS heading FROM tasks t";
    assert_eq!(
        compiles(sql),
        "projects[label: name]\nappend tasks[label: title]"
    );
    assert!(
        notes(sql)
            .iter()
            .any(|n| n.contains("`heading` in the second arm becomes `label`")),
        "{:?}",
        notes(sql)
    );
    // Three arms chain, and a column whose leaf already IS the union's name
    // takes no rename.
    assert_eq!(
        compiles(
            "SELECT p.name FROM projects p UNION ALL SELECT o.name FROM orgs o \
             UNION ALL SELECT tm.name FROM teams tm"
        ),
        "projects[name]\nappend orgs[name]\nappend teams[name]"
    );
}

#[test]
fn plain_union_is_append_plus_distinct() {
    // Supported because it was PROVEN row-equivalent, not because it looked
    // close: SQL's UNION dedupes over all output columns and Ordo's
    // `distinct` dedupes over the projected outputs, which are the same
    // columns; both treat two nulls as the same value; and under a
    // deterministic collation (Postgres's default) `COLLATE "C"` and the
    // database's collation partition text identically. See
    // `differential.rs::a_plain_union_dedupes_the_way_sql_does`.
    let sql = "SELECT t.title AS label FROM tasks t UNION SELECT g.label FROM tags g";
    assert_eq!(
        compiles(sql),
        "tasks[label: title]\nappend tags[label]\ndistinct"
    );
    assert!(
        notes(sql).iter().any(|n| n.contains("DETERMINISTIC")),
        "the one place the dedupe could differ is disclosed: {:?}",
        notes(sql)
    );
    // `UNION DISTINCT` is the same operator spelled out.
    assert_eq!(
        compiles("SELECT t.title AS label FROM tasks t UNION DISTINCT SELECT g.label FROM tags g"),
        "tasks[label: title]\nappend tags[label]\ndistinct"
    );
}

#[test]
fn the_unions_ordering_and_limit_go_after_the_last_append() {
    // SQL applies a trailing ORDER BY / LIMIT to the whole union, and so
    // does Ordo — `sort` and `take` are steps of the pipeline the last
    // `append` finished building.
    assert_eq!(
        compiles(
            "SELECT p.name AS label FROM projects p UNION ALL SELECT t.title FROM tasks t \
             ORDER BY label LIMIT 3"
        ),
        "projects[label: name]\nappend tasks[label: title]\nsort label\ntake 3"
    );
    // And `distinct` precedes them, as `UNION … ORDER BY` does.
    assert_eq!(
        compiles(
            "SELECT p.name AS label FROM projects p UNION SELECT t.title FROM tasks t \
             ORDER BY label LIMIT 3"
        ),
        "projects[label: name]\nappend tasks[label: title]\ndistinct\nsort label\ntake 3"
    );
}

#[test]
fn a_unions_take_says_which_tiebreak_it_got() {
    // The single-source note says Ordo appends an implicit `id`. After an
    // `append` it does not — the arms have different ids — and the note must
    // not claim otherwise.
    let n = notes(
        "SELECT p.name AS label FROM projects p UNION ALL SELECT t.title FROM tasks t \
         ORDER BY label LIMIT 3",
    );
    assert!(
        n.iter()
            .any(|x| x.contains("the arm, then the projected columns")),
        "{n:?}"
    );
    assert!(
        !n.iter().any(|x| x.contains("implicit `id` tiebreak")),
        "the union tiebreak is not an id, and nothing may say it is: {n:?}"
    );
}

#[test]
fn a_null_in_any_arm_makes_the_union_column_nullable() {
    // `projects.startsOn` is required and `tasks.due` is not, so the union's
    // `at` can be null and a DESC ordering needs the null-placement bridge —
    // decided across the arms, not from the first one.
    assert_eq!(
        compiles(
            "SELECT p.\"startsOn\" AS at FROM projects p UNION ALL SELECT t.due FROM tasks t \
             ORDER BY at DESC"
        ),
        "projects[at: startsOn]\nappend tasks[at: due]\nsort cond(at is null -> 0, 1), -at"
    );
    // Neither arm nullable: no bridge, and nothing claiming one.
    assert_eq!(
        compiles(
            "SELECT p.\"startsOn\" AS at FROM projects p UNION ALL SELECT o.founded FROM orgs o \
             ORDER BY at DESC"
        ),
        "projects[at: startsOn]\nappend orgs[at: founded]\nsort -at"
    );
}

#[test]
fn a_unions_arms_keep_their_own_conditions_and_null_bridges() {
    // Each arm is decompiled by the same machinery a single SELECT is, so
    // the three-valued bridge and the INNER JOIN conjunct still apply per
    // arm rather than being lost at the `append`.
    assert_eq!(
        compiles(
            "SELECT p.name AS label FROM people p WHERE p.nickname <> 'B' \
             UNION ALL SELECT t.title FROM tasks t"
        ),
        "people(nickname != \"B\" and nickname is not null)[label: name]\nappend tasks[label: title]"
    );
    assert_eq!(
        compiles(
            "SELECT pr.name AS label FROM projects pr JOIN people sp ON pr.sponsor = sp.id \
             UNION ALL SELECT t.title FROM tasks t"
        ),
        "projects(sponsor is not null)[label: name]\nappend tasks[label: title]"
    );
}

// ── where the union subset ends, each refused by name ───────────────────

#[test]
fn a_union_of_an_entity_with_itself_is_refused_by_name() {
    // DL §5: an entity may appear ONCE as a source per pipeline. This is the
    // one refusal in the file that is NOT a divergence refusal — the rows
    // would agree today, because `ordo-sema` does not enforce the rule — so
    // the message says so rather than implying a wrong answer.
    // `differential.rs::the_self_union_refusal_is_a_language_rule` pins that
    // reading.
    let r = refusal(
        "SELECT 'a' AS kind, p.name AS label FROM projects p \
         UNION ALL SELECT 'b', q.code FROM projects q",
    );
    assert!(r.contains("ONCE as a source per pipeline"), "{r}");
    assert!(r.contains("NOT a divergence"), "{r}");
}

#[test]
fn a_per_arm_ordering_or_limit_is_refused_by_name() {
    // The parentheses exist so an arm can carry its own ORDER BY / LIMIT,
    // and an `append` arm is ONE SOURCE FORM (DL §1) with no steps at all.
    for sql in [
        "(SELECT p.name AS label FROM projects p ORDER BY label LIMIT 2) \
         UNION ALL SELECT t.title FROM tasks t",
        "SELECT p.name AS label FROM projects p \
         UNION ALL (SELECT t.title FROM tasks t LIMIT 2)",
    ] {
        let r = refusal(sql);
        assert!(r.contains("parenthesised set-operation arm"), "{sql}: {r}");
        assert!(r.contains("one SOURCE form per arm"), "{sql}: {r}");
    }
}

#[test]
fn intersect_and_except_stay_refused_by_name() {
    for (sql, want) in [
        (
            "SELECT p.name FROM projects p INTERSECT SELECT t.title FROM tasks t",
            "INTERSECT",
        ),
        (
            "SELECT p.name FROM projects p EXCEPT SELECT t.title FROM tasks t",
            "EXCEPT",
        ),
    ] {
        let r = refusal(sql);
        assert!(r.contains(want), "{sql}: {r}");
        assert!(r.contains("no Ordo spelling"), "{sql}: {r}");
    }
}

#[test]
fn the_arms_have_to_line_up_and_the_message_says_how_they_did_not() {
    // Ordo's `append` does no coercion and no null-filling; SQL resolves a
    // union column's type across the arms. A mismatch this file does not
    // catch arrives from the FORWARD compiler as `append_misalignment`, at
    // line 2 of source the reader never wrote.
    let r = refusal(
        "SELECT p.\"startsOn\" AS at FROM projects p UNION ALL SELECT t.title FROM tasks t",
    );
    assert!(r.contains("`date` in the first arm"), "{r}");
    assert!(r.contains("`text` in the second arm"), "{r}");
    // A bare NULL is the same problem: SQL gives it the other arm's type.
    let r = refusal("SELECT NULL AS at FROM projects p UNION ALL SELECT t.due FROM tasks t");
    assert!(r.contains("null-filling"), "{r}");
    // Different widths, named here rather than by Postgres.
    let r = refusal(
        "SELECT p.name AS label, p.code FROM projects p UNION ALL SELECT t.title FROM tasks t",
    );
    assert!(r.contains("different numbers of columns"), "{r}");
}

#[test]
fn a_union_arm_that_is_more_than_one_source_form_is_refused() {
    for (sql, want) in [
        (
            "SELECT p.org, count(*) AS n FROM people p GROUP BY p.org \
             UNION ALL SELECT t.title, 1 FROM tasks t",
            "`GROUP BY` in the first arm",
        ),
        (
            "SELECT p.name AS label FROM projects p \
             UNION ALL SELECT count(*) AS n FROM tasks t",
            "aggregate in the second arm",
        ),
        (
            "SELECT DISTINCT p.name AS label FROM projects p \
             UNION ALL SELECT t.title FROM tasks t",
            "`SELECT DISTINCT` in the first arm",
        ),
    ] {
        let r = refusal(sql);
        assert!(r.contains(want), "{sql}\n  wanted {want:?}\n  got: {r}");
    }
}

#[test]
fn a_mixed_union_chain_is_refused_because_the_dedupe_has_nowhere_to_go() {
    let r = refusal(
        "SELECT p.name AS label FROM projects p UNION ALL SELECT t.title FROM tasks t \
         UNION SELECT g.label FROM tags g",
    );
    assert!(r.contains("mixes `UNION` and `UNION ALL`"), "{r}");
}

#[test]
fn a_unions_order_by_may_only_name_the_unions_own_columns() {
    // SQL says the same thing: after a set operation, ORDER BY sees the
    // output columns and nothing else.
    let r = refusal(
        "SELECT p.name AS label FROM projects p UNION ALL SELECT t.title FROM tasks t \
         ORDER BY p.code",
    );
    assert!(r.contains("QUALIFIED ORDER BY key after a union"), "{r}");
    let r = refusal(
        "SELECT p.name AS label FROM projects p UNION ALL SELECT t.title FROM tasks t \
         ORDER BY code",
    );
    assert!(r.contains("the union does not have"), "{r}");
    assert!(
        r.contains("label"),
        "the message lists what it does have: {r}"
    );
}

// ── the tokenizer-soup class, audited ───────────────────────────────────

#[test]
fn what_a_real_query_hits_is_named_rather_than_debug_printed() {
    // Every one of these is well-formed SQL that reached `expected an
    // identifier, found <Debug of a token>`. The owner's feed was the first
    // (`found Str("project")`); the rest are what the audit of that site
    // turned up.
    for (sql, want) in [
        // A bind parameter — every pasted application query has one.
        (
            "SELECT p.name FROM projects p WHERE p.lead = $1",
            "bind parameter",
        ),
        ("SELECT p.name FROM projects p WHERE p.lead = ?", "`?`"),
        // A column ordinal, which is the PORTABLE way to order a union.
        ("SELECT p.name FROM projects p ORDER BY 1", "ORDINAL"),
        ("SELECT p.name FROM projects p GROUP BY 1", "ORDINAL"),
        // `t.*` — `SELECT *` qualified, and refused with the same argument.
        ("SELECT p.* FROM projects p", "every PHYSICAL column"),
        // A function outside the closed set, named as itself rather than
        // read as a column and reported as "expected FROM".
        ("SELECT upper(p.name) AS u FROM projects p", "`upper(…)`"),
        // A cast.
        ("SELECT p.name::text AS t FROM projects p", "`::` cast"),
        // A CASE expression.
        (
            "SELECT CASE WHEN p.shipped THEN 1 ELSE 0 END AS n FROM projects p",
            "`CASE`",
        ),
        // And the statement that simply stops.
        (
            "SELECT p.name FROM projects p WHERE p.name =",
            "outside the v0 subset",
        ),
    ] {
        let r = refusal(sql);
        assert!(r.contains(want), "`{sql}`\n  wanted {want:?}\n  got: {r}");
        assert!(
            !r.contains("expected an identifier, found"),
            "`{sql}` still answers with a token dump: {r}"
        );
    }
}

#[test]
fn comparing_against_null_is_refused_rather_than_inverted() {
    // `x = NULL` fell through to the column-to-column branch and emitted
    // `x == null`. SQL's `= NULL` is unknown for EVERY row and matches
    // nothing; Ordo's `==` is `IS NOT DISTINCT FROM`, so `x == null` matches
    // exactly the null rows. Not a near miss — the opposite answer, emitted
    // in silence. `differential.rs` runs the would-be translation.
    for sql in [
        "SELECT p.name FROM people p WHERE p.nickname = NULL",
        "SELECT p.name FROM people p WHERE p.nickname <> NULL",
    ] {
        let r = refusal(sql);
        assert!(r.contains("match NOTHING"), "{sql}: {r}");
        assert!(r.contains("IS NULL"), "{sql}: {r}");
    }
    // The test that DOES decompile is untouched.
    assert_eq!(
        ordo("SELECT name FROM people WHERE nickname IS NULL"),
        "people(nickname is null)[name]"
    );
}

#[test]
fn every_emission_in_this_suite_compiles_forward() {
    // The blanket check: nothing above may emit text the forward compiler
    // will not take. A decompiler whose output does not parse is a
    // refusal with extra steps and worse wording.
    for sql in [
        "SELECT name FROM people WHERE nickname <> 'B'",
        "SELECT name FROM people WHERE nickname <> 'x\"y'",
        "SELECT name FROM people ORDER BY nickname DESC",
        "SELECT count(*) AS n FROM people GROUP BY team",
        "SELECT count(*) AS n, team FROM people GROUP BY team",
        "SELECT p.name FROM projects p JOIN people sp ON p.sponsor = sp.id",
        "SELECT name FROM people WHERE active = true AND age > 40 OR salary > 100",
        "SELECT name FROM people WHERE (active = true OR age > 40) AND salary > 100",
        "SELECT name FROM projects WHERE lead <> sponsor",
        "SELECT p.name, ld.name AS leadName FROM projects p LEFT JOIN people ld ON p.lead = ld.id",
        "SELECT p.team, count(*) AS n FROM people p GROUP BY p.team",
        "SELECT DISTINCT region FROM orgs",
        "SELECT name FROM people WHERE strpos(name, 'a') > 0",
        "SELECT name FROM people ORDER BY salary DESC, id LIMIT 3",
    ] {
        compiles(sql);
    }
}
