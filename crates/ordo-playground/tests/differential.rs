//! The row oracle for the queries the corpus does not contain.
//!
//! `roundtrip.rs` proves the reverse compiler against the stage-two SQL
//! corpus, whose row sets are blind-validated — but that corpus was written
//! to exercise the FORWARD compiler, and it happens to contain no `<>`
//! against a nullable column and no `a AND b OR c`. Both of those
//! decompiled to a different query than the one submitted, and both passed
//! every test in the crate, because nothing ever ran the two sides against
//! each other.
//!
//! So: a small set of statements chosen for exactly the places the two
//! languages could disagree, each executed twice on the seeded `wide`
//! fixture — once as submitted, once as the decompilation recompiled —
//! and required to return the same rows.
//!
//! Four oracles live here, because a row multiset cannot see everything:
//!
//! - [`decompiled_sql_returns_the_rows_the_sql_returned`] — the row SET.
//! - [`an_ordering_decompiles_to_the_same_sequence`] — the row ORDER, on
//!   single-column statements where positions align. `row_multiset` sorts
//!   both sides, so an ordering defect is invisible to the first oracle;
//!   the null-placement bug hid behind exactly that for as long as it
//!   existed.
//! - [`what_is_refused_is_refused_because_it_diverges`] — the refusals. A
//!   refusal is only honest if the thing refused really would have returned
//!   different rows, so the would-be translation is built by hand and run.
//! - [`the_self_union_refusal_is_a_language_rule_not_a_divergence`] — the
//!   ONE refusal that is not in the oracle above, and why. DL §5 forbids an
//!   entity appearing twice as a source; `ordo-sema` does not enforce it and
//!   the rows agree. Pinned as such, so nobody later reads the refusal as a
//!   claim about rows.
//! - [`the_market_fixtures_feed_shapes_agree_row_for_row`] — `append`, on
//!   the fixture the playground opens on, including the row ORDER.
//! - [`the_seed_still_makes_the_null_cases_reachable`] and
//!   [`the_collation_the_note_warns_about_is_real`] — the suite's own
//!   premises, asserted rather than assumed.
//!
//! What this deliberately does NOT assert: that every SQL statement
//! decompiles. Most do not, on purpose. The subject here is the ones that
//! do, and whether they mean what they said.

use ordo_conformance::{Fixture, Pg};
use ordo_playground::decompile::decompile;
use ordo_playground::server::row_multiset;
use serde_json::Value;

/// Statements whose answer turns on a place the two languages disagree.
/// The comment on each is the trap it is standing on.
const DIFFERENTIAL: &[&str] = &[
    // Three-valued `<>` against a nullable column. Before the fix: 1 row in
    // SQL, 5 in the decompilation.
    "SELECT name FROM people WHERE nickname <> 'B'",
    "SELECT name FROM people WHERE nickname = 'B'",
    "SELECT name FROM people WHERE nickname <> 'B' OR age > 40",
    "SELECT name FROM people WHERE nickname IS NULL OR nickname <> 'B'",
    // Column against column, one side optional (`projects.sponsor`).
    "SELECT name FROM projects WHERE lead <> sponsor",
    "SELECT name FROM projects WHERE lead = sponsor",
    // The same nullable path twice — one guard, not two, and still right.
    "SELECT name FROM people WHERE nickname = nickname",
    // A nullable column reached through an optional ref.
    "SELECT t.title FROM tasks t LEFT JOIN people a ON t.assignee = a.id WHERE a.nickname <> 'B'",
    // SQL precedence. Before the fix: `a and (b or c)`, which is a
    // different set of rows on this seed.
    "SELECT name FROM people WHERE active = true AND age > 40 OR salary > 100000",
    "SELECT name FROM people WHERE age > 40 AND active = true OR nickname IS NOT NULL",
    "SELECT name FROM people WHERE (active = true OR age > 40) AND salary > 100000",
    "SELECT name FROM people WHERE age > 1 AND age < 40 OR age > 44 AND age < 99",
    // Ordering comparisons and IN, which need no bridge — asserted so a
    // future "fix" that guards everything is caught changing rows.
    "SELECT name FROM people WHERE age > 40",
    "SELECT name FROM people WHERE age IN (34, 45)",
    // Quoted because Postgres folds an unquoted identifier to lower case
    // and the fixture's DDL is camelCase.
    "SELECT name FROM people WHERE \"lastSeen\" IS NOT NULL",
    // Text operators lower to expressions that are already null-correct.
    "SELECT name FROM people WHERE strpos(nickname, 'B') > 0",
    "SELECT name FROM people WHERE left(name, length('A')) = 'A'",
    // Grouping and ordering, with a null group key present in the seed.
    "SELECT p.team, count(*) AS n FROM people p GROUP BY p.team",
    "SELECT p.team, sum(p.salary) AS total FROM people p GROUP BY p.team",
    "SELECT p.team FROM people p GROUP BY p.team",
    "SELECT DISTINCT region FROM orgs",
    // A dot-path projection and sort through an optional ref.
    "SELECT p.name, sp.name AS sponsorName FROM projects p LEFT JOIN people sp ON p.sponsor = sp.id ORDER BY sp.name DESC, p.id",
    // ── INNER JOIN: a dot-path keeps the rows the join drops ─────────────
    //
    // Before the fix these were a NOTE and three rows against two.
    "SELECT p.name FROM projects p JOIN people sp ON p.sponsor = sp.id",
    "SELECT p.name, sp.name AS sponsorName FROM projects p INNER JOIN people sp ON p.sponsor = sp.id",
    "SELECT t.title FROM tasks t JOIN people a ON t.assignee = a.id WHERE t.done = false",
    // The conjunct has to survive an OR without being absorbed into it.
    "SELECT p.name FROM projects p JOIN people sp ON p.sponsor = sp.id WHERE p.shipped = true OR p.budget > 200000",
    // A required ref drops nothing, so the conjunct must NOT appear: this
    // case fails if a future change guards every inner join unconditionally
    // — the guard is correct there, but claiming a divergence that is not
    // there is its own kind of lie.
    "SELECT p.name FROM projects p JOIN people ld ON p.lead = ld.id",
    // ── GROUP BY keys the SELECT does not project ────────────────────────
    //
    // Before the fix: one more column than the SQL, disclosed in a note.
    "SELECT count(*) AS n FROM people p GROUP BY p.team",
    "SELECT sum(p.salary) AS total FROM people p GROUP BY p.team",
    "SELECT count(*) AS n, p.team FROM people p GROUP BY p.team",
    "SELECT count(*) AS n FROM people p GROUP BY p.team ORDER BY p.team",
    "SELECT count(*) AS n FROM tasks t LEFT JOIN people a ON t.assignee = a.id GROUP BY a.team",
    // The plain form still has to work, keys projected and in order.
    "SELECT a.team, count(*) AS n FROM tasks t LEFT JOIN people a ON t.assignee = a.id GROUP BY a.team ORDER BY a.team",
    // `sort` and `take` are applied by the OUTER pipeline in the `let` form.
    // A `sort` inside a `let` is not discarded — it still decides which rows
    // an inner `take` keeps — but its ORDER does not survive to the outer
    // line, silently. So this is the case that catches putting them on the
    // wrong side.
    "SELECT count(*) AS n FROM people p GROUP BY p.team ORDER BY n DESC, p.team LIMIT 2",
    "SELECT sum(p.salary) AS total FROM people p GROUP BY p.team ORDER BY total DESC, p.team LIMIT 2",
    "SELECT p.team, count(*) AS n FROM people p GROUP BY p.team ORDER BY n DESC, p.team LIMIT 2",
    // ── null placement in an ordering, where LIMIT makes it a ROW set ────
    //
    // Postgres puts nulls FIRST under DESC; Ordo's `sort` is always NULLS
    // LAST. Before the fix `ORDER BY nickname DESC LIMIT 3` was Di/Bo/Ada in
    // the decompilation and Ada/Cy/Eli in Postgres — three rows each, none
    // of them the same three.
    //
    // Every one carries an explicit `id` tiebreak. Four people share a null
    // nickname, and WHICH three of them a LIMIT keeps is undefined in SQL
    // without one — an undefined answer cannot be the thing an oracle
    // compares against.
    "SELECT name FROM people ORDER BY nickname DESC, id LIMIT 3",
    "SELECT name FROM people ORDER BY age DESC, id LIMIT 3",
    "SELECT name FROM people ORDER BY nickname ASC NULLS FIRST, id LIMIT 2",
    "SELECT name FROM people ORDER BY nickname DESC NULLS LAST, id LIMIT 2",
    "SELECT name FROM projects ORDER BY ends DESC, id LIMIT 2",
    "SELECT p.name FROM projects p LEFT JOIN people sp ON p.sponsor = sp.id ORDER BY sp.name DESC, p.id LIMIT 2",
    // A required column has no nulls to place, so no bridge is emitted.
    "SELECT name FROM people ORDER BY salary DESC, id LIMIT 3",
    // ── literals the emitter has to QUOTE, not re-lex ────────────────────
    //
    // `'x\"y'` produced `\"x\"y\"` and did not compile at all; the refusal
    // came from the forward compiler, about source the user never wrote.
    "SELECT name FROM people WHERE nickname <> 'x\"y'",
    "SELECT name FROM people WHERE name <> 'a\\nb'",
    "SELECT name FROM people WHERE strpos(name, 'a\"b') > 0",
    // ── literal columns, which are VALUES RETURNED ───────────────────────
    //
    // Every other literal in this file decides which rows come back. These
    // decide what is IN one, so a quoting bug here is a wrong answer rather
    // than a wrong filter — and each literal KIND is checked, because the
    // decision to support a kind at all was "the forward compiler accepts it
    // in a projection", which is a claim about Postgres.
    "SELECT 'project' AS kind, p.name FROM projects p",
    "SELECT 5 AS n, true AS b, NULL AS z, p.name FROM projects p",
    "SELECT 5.5 AS n, p.name FROM projects p",
    "SELECT 'a''b' AS kind, p.name FROM projects p",
    "SELECT 'x\"y' AS kind, p.name FROM projects p",
    "SELECT 'a\\nb' AS kind, p.name FROM projects p",
    // ── UNION ALL is `append` ────────────────────────────────────────────
    //
    // The owner's many-branch feed, in the entities this fixture has: a
    // literal discriminator, per-arm renames, per-arm WHERE, and one
    // ORDER BY / LIMIT over the whole union. `tasks.due` is optional and
    // `projects.startsOn` is not, so the union's `at` is nullable and the
    // DESC ordering needs the null-placement bridge — decided across the
    // arms rather than from the first one.
    "SELECT 'project' AS kind, p.name AS title, p.\"startsOn\" AS at FROM projects p WHERE p.shipped = false \
     UNION ALL SELECT 'task', t.title, t.due FROM tasks t WHERE t.done = false \
     ORDER BY at DESC LIMIT 100",
    // Name matching: SQL takes the union's names from the first arm and
    // lines the rest up by POSITION. If the emitter matched Ordo's names
    // naively instead, `heading` would not line up with `label` and the
    // forward compiler would refuse — or worse, line up with the wrong one.
    "SELECT p.name AS label FROM projects p UNION ALL SELECT t.title AS heading FROM tasks t",
    "SELECT p.name AS label, p.budget AS amount FROM projects p \
     UNION ALL SELECT t.title, t.points FROM tasks t",
    // Three arms, and a chain whose arms carry their own conditions.
    "SELECT p.name FROM projects p UNION ALL SELECT o.name FROM orgs o \
     UNION ALL SELECT tm.name FROM teams tm",
    "SELECT p.name AS label FROM projects p WHERE p.shipped = false \
     UNION ALL SELECT t.title FROM tasks t WHERE t.priority < 3",
    // An arm's own null bridge has to survive the append.
    "SELECT p.name AS label FROM people p WHERE p.nickname <> 'B' \
     UNION ALL SELECT t.title FROM tasks t",
    // …and an INNER JOIN's conjunct with it.
    "SELECT pr.name AS label FROM projects pr JOIN people sp ON pr.sponsor = sp.id \
     UNION ALL SELECT t.title FROM tasks t",
    // A dot-path through a ref inside an arm.
    "SELECT ld.name AS label FROM projects pr LEFT JOIN people ld ON pr.lead = ld.id \
     UNION ALL SELECT t.title FROM tasks t",
    // ── plain UNION dedupes ──────────────────────────────────────────────
    //
    // Supported only because these agree. `tasks.title` and `tags.label`
    // share no value, so the third case is the one that matters: it dedupes
    // ACROSS the arms and WITHIN one, on a key that can be null.
    "SELECT t.title AS label FROM tasks t UNION SELECT g.label FROM tags g",
    "SELECT p.team AS who FROM people p UNION SELECT t.parent FROM teams t",
    "SELECT t.assignee AS who FROM tasks t UNION SELECT pr.sponsor FROM projects pr",
    "SELECT t.due AS at FROM tasks t UNION SELECT pr.ends FROM projects pr ORDER BY at",
];

/// Statements whose ORDER BY the decompilation must reproduce as a
/// SEQUENCE, not merely as a set.
///
/// One column each, so the SQL side's positional row and the Ordo side's
/// single-valued object compare without guessing an alignment. Every case
/// here also appears, or could appear, in [`DIFFERENTIAL`] — and would pass
/// there while returning the rows in the wrong order.
/// Each carries an explicit trailing `id`, for the same reason the LIMIT
/// cases do: ties on a nullable key are ordered arbitrarily by SQL, and a
/// sequence oracle against an arbitrary sequence is a coin toss. The
/// emitter drops that trailing `id` (Ordo appends its own), so both sides
/// end up totally ordered by the same rule.
const ORDERED: &[&str] = &[
    // Nulls first under DESC (Postgres) against Ordo's unconditional NULLS
    // LAST. No LIMIT: the SET is identical either way and only the sequence
    // tells them apart.
    "SELECT name FROM people ORDER BY nickname DESC, id",
    "SELECT name FROM people ORDER BY age DESC, id",
    "SELECT name FROM people ORDER BY nickname ASC NULLS FIRST, id",
    "SELECT name FROM people ORDER BY nickname ASC, id",
    "SELECT title FROM tasks ORDER BY due DESC, id",
    "SELECT name FROM projects ORDER BY ends DESC, id",
    // Ordo's `sort` is NULLS LAST, so an explicit NULLS LAST needs nothing —
    // and must not acquire a bridge that reverses it.
    "SELECT name FROM people ORDER BY nickname DESC NULLS LAST, id",
    // Non-nullable keys, ascending and descending: no bridge, same sequence.
    "SELECT name FROM people ORDER BY salary DESC, id",
    "SELECT title FROM tasks ORDER BY priority ASC, title ASC, id",
    // A JOINED side's id is an ordinary sort key, not the implicit tiebreak
    // Ordo appends (which is the SOURCE entity's id). The inner join makes
    // `a.team, a.id` a total order on this seed — every assigned task has a
    // distinct assignee — so the sequence is SQL's own, not an arbitrary
    // one. See the shape assertion in `decompile.rs` for the discriminating
    // case: on this seed task ids and assignee ids happen to run in the same
    // direction, so a row oracle cannot tell the two tiebreaks apart.
    "SELECT t.title FROM tasks t JOIN people a ON t.assignee = a.id ORDER BY a.team, a.id",
    // An `append`'s ordering. Ordo's own tiebreak after an append is the
    // arm, then the projected columns, then a per-arm row number — NOT the
    // `id` a single source uses — so an emitter that put the ordering in the
    // wrong place would come back arm-by-arm rather than interleaved. Every
    // project name and task title on this seed is distinct, so the SQL side
    // is totally ordered and there is a sequence to compare against.
    "SELECT p.name AS label FROM projects p UNION ALL SELECT t.title FROM tasks t ORDER BY label",
    "SELECT p.name AS label FROM projects p UNION ALL SELECT t.title FROM tasks t \
     ORDER BY label DESC",
];

#[test]
fn decompiled_sql_returns_the_rows_the_sql_returned() {
    let f = Fixture::load_wide().expect("wide fixture");
    let pg = Pg::named("ordo_pg_differential");
    f.reset(&pg).expect("seeded");

    let mut failures = Vec::new();
    let mut checked = 0;
    for sql in DIFFERENTIAL {
        let d = match decompile(&f.schema, sql) {
            Ok(d) => d,
            Err(r) => {
                failures.push(format!("{sql}\n  REFUSED: {}", r.what));
                continue;
            }
        };
        let sql_rows = match f.run_sql(&pg, sql) {
            Ok(r) => r,
            Err(e) => {
                // The statement itself must be valid Postgres, or the case
                // is testing nothing.
                failures.push(format!("{sql}\n  the SQL does not run: {e}"));
                continue;
            }
        };
        match f.run_case(&pg, &d.ordo) {
            Err(e) => failures.push(format!(
                "{sql}\n  decompiled to `{}`\n  which does not run: {e}",
                d.ordo.replace('\n', " ⏎ ")
            )),
            Ok(ordo_rows) => {
                let (s, o) = (row_multiset(&sql_rows), row_multiset(&ordo_rows));
                checked += 1;
                if s != o {
                    failures.push(format!(
                        "{sql}\n  decompiled to `{}`\n  sql  ({}): {}\n  ordo ({}): {}",
                        d.ordo.replace('\n', " ⏎ "),
                        s.len(),
                        s.join(" · "),
                        o.len(),
                        o.join(" · ")
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "\n\n{}", failures.join("\n\n"));
    assert_eq!(
        checked,
        DIFFERENTIAL.len(),
        "every differential case must actually have been compared"
    );
}

/// The row ORDER, which the multiset oracle above cannot see.
///
/// `row_multiset` sorts both sides before comparing, so a decompilation that
/// returns the right rows in the wrong sequence passes it. That is exactly
/// where the null-placement defect lived: `ORDER BY nickname DESC` came back
/// with the same six people, four of them in the wrong place, and every
/// existing oracle was green.
#[test]
fn an_ordering_decompiles_to_the_same_sequence() {
    let f = Fixture::load_wide().expect("wide fixture");
    let pg = Pg::named("ordo_pg_differential_order");
    f.reset(&pg).expect("seeded");

    // One column per statement, so position is not something to guess at.
    let column = |rows: &Value| -> Vec<String> {
        rows.as_array()
            .expect("rows")
            .iter()
            .map(|r| match r {
                Value::Array(a) => a.first().cloned().unwrap_or(Value::Null).to_string(),
                Value::Object(o) => o
                    .values()
                    .next()
                    .cloned()
                    .unwrap_or(Value::Null)
                    .to_string(),
                other => other.to_string(),
            })
            .collect()
    };

    let mut failures = Vec::new();
    for sql in ORDERED {
        let d = match decompile(&f.schema, sql) {
            Ok(d) => d,
            Err(r) => {
                failures.push(format!("{sql}\n  REFUSED: {}", r.what));
                continue;
            }
        };
        let (s, o) = (f.run_sql(&pg, sql), f.run_case(&pg, &d.ordo));
        match (s, o) {
            (Ok(s), Ok(o)) => {
                let (sv, ov) = (column(&s), column(&o));
                if sv != ov {
                    failures.push(format!(
                        "{sql}\n  decompiled to `{}`\n  sql  order: {}\n  ordo order: {}",
                        d.ordo.replace('\n', " ⏎ "),
                        sv.join(" · "),
                        ov.join(" · ")
                    ));
                }
            }
            (Err(e), _) => failures.push(format!("{sql}\n  the SQL does not run: {e}")),
            (_, Err(e)) => failures.push(format!(
                "{sql}\n  decompiled to `{}`\n  which does not run: {e}",
                d.ordo.replace('\n', " ⏎ ")
            )),
        }
    }
    assert!(failures.is_empty(), "\n\n{}", failures.join("\n\n"));
}

/// A refusal has to be refusing something.
///
/// `SELECT *` and a duplicated output name are refused rather than
/// approximated — but a refusal is only honest if the approximation really
/// would have returned different rows. So the translation the emitter used
/// to produce is written out by hand here and run against its SQL: if it
/// ever agrees, the refusal has become over-caution and should go.
#[test]
fn what_is_refused_is_refused_because_it_diverges() {
    let f = Fixture::load_wide().expect("wide fixture");
    let pg = Pg::named("ordo_pg_differential_refused");
    f.reset(&pg).expect("seeded");

    for (sql, would_have_been, why) in [
        (
            "SELECT * FROM people",
            "people",
            "the bare source is the declared open fields; `SELECT *` is every physical column",
        ),
        (
            "SELECT * FROM orgs",
            "orgs",
            "even with no sealed field, `$space`/`$rev`/`id` are columns Ordo's whole-row form does not carry",
        ),
        (
            "SELECT p.name, sp.name FROM projects p LEFT JOIN people sp ON p.sponsor = sp.id",
            "projects[name, sponsor.name]",
            "two columns named `name` come back as one",
        ),
        (
            // `sum cond(x is null -> 0, 1)` is `count(x)` only where the
            // input has rows. Empty, SQL says 0 and the sum says null.
            "SELECT count(nickname) AS n FROM people WHERE age > 1000",
            "people(age > 1000)\nreduce { n: sum cond(nickname is null -> 0, 1) }",
            "`sum` over no rows is null; SQL's `count` over no rows is 0",
        ),
        (
            // `x = NULL` fell through to the column-to-column branch and
            // emitted `x == null`. SQL's `=` against NULL is unknown for
            // EVERY row and matches nothing; Ordo's `==` is `IS NOT DISTINCT
            // FROM`, so `x == null` matches exactly the null rows. Not a
            // near miss — the opposite answer.
            "SELECT name FROM people WHERE nickname = NULL",
            "people(nickname == null)[name]",
            "`= NULL` matches no rows in SQL; `== null` matches every null row in Ordo",
        ),
        (
            // A per-arm LIMIT. The parenthesised arm is refused because an
            // `append` arm is one source form with no steps — and the
            // translation that ignored the parentheses returns the rows the
            // LIMIT was there to exclude.
            "(SELECT p.name AS label FROM projects p ORDER BY label LIMIT 1) \
             UNION ALL SELECT t.title FROM tasks t",
            "projects[label: name]\nappend tasks[label: title]",
            "dropping a per-arm LIMIT returns the rows it was there to exclude",
        ),
        (
            // A per-arm `SELECT DISTINCT`, for the same reason in the other
            // direction: moving the dedupe to the whole pipeline also
            // collapses rows the SQL keeps.
            "SELECT DISTINCT t.priority AS n FROM tasks t \
             UNION ALL SELECT o.seats FROM orgs o",
            "tasks[n: priority]\nappend orgs[n: seats]",
            "a per-arm DISTINCT is not the same as no dedupe at all",
        ),
    ] {
        let r = decompile(&f.schema, sql);
        assert!(
            r.is_err(),
            "`{sql}` is no longer refused — it now emits {:?}",
            r.map(|d| d.ordo)
        );
        let sql_rows = f.run_sql(&pg, sql).expect("the SQL runs");
        let ordo_rows = f
            .run_case(&pg, would_have_been)
            .unwrap_or_else(|e| panic!("`{would_have_been}` does not run: {e}"));
        assert_ne!(
            row_multiset(&sql_rows),
            row_multiset(&ordo_rows),
            "`{sql}` and `{would_have_been}` agree on this seed, so the refusal is over-caution: {why}"
        );
    }
}

/// The owner's feed, on the fixture the playground opens on.
///
/// The `wide` cases above are the ones with the awkward nulls; this one is
/// the shape a reader actually pastes — a many-branch feed over the market
/// schema, with a literal discriminator, per-arm renames and per-arm
/// filters, ordered and limited across the whole union. It runs on its own
/// database so it cannot race the `wide` suites.
///
/// Row ORDER is compared too, not just the row set: `row_multiset` sorts
/// both sides, and an `append` whose ordering ended up inside an arm rather
/// than after the last one returns the right rows arm-by-arm.
#[test]
fn the_market_fixtures_feed_shapes_agree_row_for_row() {
    let f = Fixture::load().expect("market fixture");
    let pg = Pg::named("ordo_dec_union_market");
    f.reset(&pg).expect("seeded");

    // Every entity here is unguarded, so the SQL side and the Ordo side see
    // the same rows without a caller. (`tasks`, `projects` and the rest of
    // the guards cluster are NOT: a guard is a predicate the raw SQL does
    // not carry, and comparing against one would be comparing two different
    // questions.)
    let cases: &[(&str, bool)] = &[
        // The owner's exact shape. `orders.status`/`coupons.code` are both
        // text and `orders.total`/`coupons.value` are both money, which is
        // what `append` requires — SQL would have coerced, and Ordo does
        // not, which is why the type check exists.
        (
            "SELECT 'order' AS kind, o.status AS title, o.total AS amount FROM orders o WHERE o.status = 'done' \
             UNION ALL SELECT 'product', p.name, p.price FROM products p WHERE p.active = true \
             ORDER BY amount DESC LIMIT 100",
            false,
        ),
        // The same, with a LIMIT that actually cuts and an ordering that is
        // total on this seed — so the sequence is SQL's own rather than an
        // arbitrary one, and can be compared.
        (
            "SELECT 'order' AS kind, o.status AS title, o.total AS amount FROM orders o \
             UNION ALL SELECT 'product', p.name, p.price FROM products p \
             ORDER BY amount DESC, title, kind LIMIT 3",
            true,
        ),
        // A quote in a literal, round-tripped: SQL doubles it, Ordo's
        // strings are double-quoted and take it as-is.
        (
            "SELECT 'it''s' AS kind, o.status AS title FROM orders o \
             UNION ALL SELECT 'say \"hi\"', p.name FROM products p \
             ORDER BY title, kind",
            true,
        ),
        // Name matching: the second arm's own `AS` name is discarded by SQL
        // and must be by the decompilation too.
        (
            "SELECT o.status AS label FROM orders o \
             UNION ALL SELECT p.name AS heading FROM products p ORDER BY label",
            true,
        ),
        // Plain UNION over a nullable ref column, deduping WITHIN an arm
        // (three products share `cat_3`, two categories have a null parent)
        // and ACROSS them (`cat_2` and `cat_4` appear on both sides).
        (
            "SELECT p.category AS who FROM products p UNION SELECT c.parent FROM categories c",
            false,
        ),
    ];

    // One row as a comparable string, with the ROWS still in order. Values
    // are sorted WITHIN a row for the same reason `row_multiset` sorts them:
    // `run_sql` yields positional arrays and `run_case` yields objects whose
    // keys iterate alphabetically, so the two sides lay one row out
    // differently. The question here is the sequence of ROWS.
    let sequence = |rows: &Value| -> Vec<String> {
        rows.as_array()
            .expect("rows")
            .iter()
            .map(|r| {
                let mut vals: Vec<String> = match r {
                    Value::Array(a) => a.iter().map(|x| x.to_string()).collect(),
                    Value::Object(o) => o.values().map(|x| x.to_string()).collect(),
                    other => vec![other.to_string()],
                };
                vals.sort();
                vals.join("|")
            })
            .collect()
    };

    let mut failures = Vec::new();
    for (sql, ordered) in cases {
        let d = match decompile(&f.schema, sql) {
            Ok(d) => d,
            Err(r) => {
                failures.push(format!("{sql}\n  REFUSED: {}", r.what));
                continue;
            }
        };
        let sql_rows = match f.run_sql(&pg, sql) {
            Ok(r) => r,
            Err(e) => {
                failures.push(format!("{sql}\n  the SQL does not run: {e}"));
                continue;
            }
        };
        match f.run_case(&pg, &d.ordo) {
            Err(e) => failures.push(format!(
                "{sql}\n  decompiled to `{}`\n  which does not run: {e}",
                d.ordo.replace('\n', " ⏎ ")
            )),
            Ok(ordo_rows) => {
                let (s, o) = (row_multiset(&sql_rows), row_multiset(&ordo_rows));
                if s != o {
                    failures.push(format!(
                        "{sql}\n  decompiled to `{}`\n  sql  ({}): {}\n  ordo ({}): {}",
                        d.ordo.replace('\n', " ⏎ "),
                        s.len(),
                        s.join(" · "),
                        o.len(),
                        o.join(" · ")
                    ));
                } else if *ordered {
                    // The SQL side's columns come back positionally and the
                    // Ordo side's as an object in the plan's output order,
                    // which for these cases is the SELECT's own order.
                    let (sv, ov) = (sequence(&sql_rows), sequence(&ordo_rows));
                    if sv != ov {
                        failures.push(format!(
                            "{sql}\n  decompiled to `{}`\n  sql  order: {}\n  ordo order: {}",
                            d.ordo.replace('\n', " ⏎ "),
                            sv.join(" · "),
                            ov.join(" · ")
                        ));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "\n\n{}", failures.join("\n\n"));
}

/// The self-union refusal is a LANGUAGE-RULE refusal, and says so.
///
/// Every other refusal in this file is refused because the translation would
/// have returned different rows — that is what
/// [`what_is_refused_is_refused_because_it_diverges`] exists to prove. This
/// one is not, and pretending otherwise would be its own kind of lie: DL §5
/// says an entity may appear once as a source per pipeline, `ordo-sema` does
/// not enforce it, and the rows agree. It is refused because the emitted
/// text would not be Ordo, and the refusal message states exactly that.
///
/// So the assertion is the honest one: EITHER the forward compiler has since
/// started enforcing the rule (in which case emitting it would have produced
/// a compile error two steps out, worded about source the reader never
/// wrote), OR it still accepts it and the rows agree. Both outcomes keep the
/// refusal correct; a third — the rows differing — would mean the message is
/// wrong and should be rewritten as a divergence.
#[test]
fn the_self_union_refusal_is_a_language_rule_not_a_divergence() {
    let f = Fixture::load_wide().expect("wide fixture");
    let pg = Pg::named("ordo_dec_selfunion");
    f.reset(&pg).expect("seeded");

    let sql = "SELECT 'a' AS kind, p.name AS label FROM projects p \
               UNION ALL SELECT 'b', q.code FROM projects q";
    let would_have_been =
        "projects[kind: \"a\", label: name]\nappend projects[kind: \"b\", label: code]";

    let r = decompile(&f.schema, sql);
    assert!(
        r.is_err(),
        "a self-union is refused on DL §5 grounds — it now emits {:?}",
        r.map(|d| d.ordo)
    );
    let sql_rows = f.run_sql(&pg, sql).expect("the SQL runs");
    match f.run_case(&pg, would_have_been) {
        // The DL rule landed in the forward compiler. Nothing to compare —
        // and the refusal is now doing the compiler's job one step earlier,
        // which is the whole point of this file.
        Err(_) => {}
        Ok(ordo_rows) => assert_eq!(
            row_multiset(&sql_rows),
            row_multiset(&ordo_rows),
            "the self-union's rows now DIFFER, so `{would_have_been}` is not merely \
             illegal Ordo — the refusal message says the rows would agree and it is \
             wrong. Rewrite it as a divergence refusal."
        ),
    }
}

/// The collation the emitter warns about, demonstrated rather than asserted.
///
/// Ordo pins `COLLATE "C"` on every text ordering; the SQL side takes the
/// database's. This is the one divergence in the file that is disclosed in a
/// note instead of being refused or bridged — there is no Ordo spelling for
/// another collation — so the note had better be about something real, and
/// the seed had better not be the reason the suites are green.
#[test]
fn the_collation_the_note_warns_about_is_real() {
    let f = Fixture::load_wide().expect("wide fixture");
    let pg = Pg::named("ordo_pg_differential_collation");
    f.reset(&pg).expect("seeded");

    let one = |sql: &str| -> bool {
        f.run_sql(&pg, sql).expect("runs")[0][0]
            .as_bool()
            .expect("a boolean")
    };
    let db = one("SELECT 'a' < 'B'");
    let c = one("SELECT 'a' COLLATE \"C\" < 'B' COLLATE \"C\"");
    assert_ne!(
        db, c,
        "this database's collation agrees with `C` on case ordering, so the collation note \
         is describing a hazard this suite can no longer demonstrate — say so rather than \
         leaving the note unbacked"
    );

    // And the note fires exactly where a text ordering is.
    let notes = |sql: &str| decompile(&f.schema, sql).expect("decompiles").notes;
    assert!(
        notes("SELECT name FROM people ORDER BY name")
            .iter()
            .any(|n| n.contains("COLLATE")),
        "a text ordering must disclose the pin"
    );
    assert!(
        !notes("SELECT name FROM people ORDER BY salary")
            .iter()
            .any(|n| n.contains("COLLATE")),
        "a numeric ordering takes no pin and must not claim one"
    );
}

/// The seed has to *contain* the nulls these cases turn on, or the suite
/// above is green for the wrong reason. Asserted rather than assumed: a
/// seed edit that filled in `nickname` would disarm eleven cases silently.
#[test]
fn the_seed_still_makes_the_null_cases_reachable() {
    let f = Fixture::load_wide().expect("wide fixture");
    let pg = Pg::named("ordo_pg_differential_seed");
    f.reset(&pg).expect("seeded");

    for (what, sql) in [
        (
            "a null nickname",
            "SELECT count(*) FROM people WHERE nickname IS NULL",
        ),
        (
            "a non-null nickname",
            "SELECT count(*) FROM people WHERE nickname IS NOT NULL",
        ),
        (
            "a null sponsor",
            "SELECT count(*) FROM projects WHERE sponsor IS NULL",
        ),
        (
            "a null team (the null group key)",
            "SELECT count(*) FROM people WHERE team IS NULL",
        ),
        // The inner-join cases are only cases while some project HAS a
        // sponsor and some does not.
        (
            "a non-null sponsor",
            "SELECT count(*) FROM projects WHERE sponsor IS NOT NULL",
        ),
        (
            "an unassigned task",
            "SELECT count(*) FROM tasks WHERE assignee IS NULL",
        ),
        (
            "an assigned task that is still open",
            "SELECT count(*) FROM tasks WHERE assignee IS NOT NULL AND done = false",
        ),
        // The null-placement cases need a null on each key they sort by.
        (
            "a null age",
            "SELECT count(*) FROM people WHERE age IS NULL",
        ),
        (
            "a null end date",
            "SELECT count(*) FROM projects WHERE ends IS NULL",
        ),
        (
            "a task with no due date",
            "SELECT count(*) FROM tasks WHERE due IS NULL",
        ),
        // ...and a non-null one, or "nulls first" and "nulls last" are the
        // same answer and the whole class is untested.
        (
            "a non-null age",
            "SELECT count(*) FROM people WHERE age IS NOT NULL",
        ),
        (
            "a non-null end date",
            "SELECT count(*) FROM projects WHERE ends IS NOT NULL",
        ),
        // ── the union cases' own premises ────────────────────────────────
        //
        // A plain UNION case proves nothing unless there is something to
        // dedupe. These are the three kinds of duplicate the `distinct`
        // decision turns on: within one arm, across two arms, and a NULL
        // appearing twice (SQL and Ordo both treat two nulls as the same
        // value for deduplication, and that is the half most likely to be
        // wrong).
        (
            "a value that repeats WITHIN one union arm",
            "SELECT count(*) FROM (SELECT team FROM people GROUP BY team HAVING count(*) > 1) q",
        ),
        (
            "a value shared ACROSS the two union arms",
            "SELECT count(*) FROM (SELECT assignee AS who FROM tasks INTERSECT \
             SELECT sponsor FROM projects) q",
        ),
        (
            "a null on both sides of a union's dedupe key",
            "SELECT least(a, b) FROM (SELECT count(*) FILTER (WHERE assignee IS NULL) AS a, \
             (SELECT count(*) FROM projects WHERE sponsor IS NULL) AS b FROM tasks) q",
        ),
        // The many-branch feed's own premise: both arms have to contribute
        // rows, or an `append` bug that dropped one would be invisible.
        (
            "an unshipped project (the feed's first arm)",
            "SELECT count(*) FROM projects WHERE shipped = false",
        ),
        (
            "an open task (the feed's second arm)",
            "SELECT count(*) FROM tasks WHERE done = false",
        ),
    ] {
        let rows = f.run_sql(&pg, sql).expect("counts");
        let n = rows[0][0].as_i64().unwrap_or_else(|| {
            panic!("`{sql}` did not return a count, got {}", rows[0][0]);
        });
        assert!(n > 0, "the wide seed no longer has {what} — `{sql}` is 0");
    }
}
