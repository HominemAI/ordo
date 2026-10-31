//! SQL → Ordo: the reverse compiler, v0.
//!
//! An honest subset, not a general SQL front end: SELECTs over one FROM
//! table each, joined by `UNION ALL` / `UNION` where there is more than one;
//! joins that map onto Ordo's dot-paths or two-source lists; literal
//! columns; AND/OR comparisons with SQL's precedence and explicit
//! parentheses; GROUP BY with the closed aggregate set; ORDER BY, LIMIT,
//! DISTINCT. Anything outside the subset returns a named refusal — never a
//! guessed translation.
//!
//! # The rule this file is held to
//!
//! A decompilation may differ from the SQL it came from only in ways that
//! **cannot change which values come back**. Anything that changes the row
//! set, the column set, or a row order the SQL had actually defined is
//! either translated faithfully or **refused by name**. A note is not a
//! licence to return different rows: the page exists to make a translation
//! visible, and a reader trusts the page over the docs.
//!
//! The rule has **one standing exception**, and it is an exception rather
//! than a carve-out: the text-collation pin below CAN change which rows come
//! back under a `LIMIT`, and it is disclosed in a note anyway, because Ordo
//! has no spelling for another collation and refusing every text ordering
//! would refuse a third of the corpus. It is the one place this file knowingly
//! does the thing it tells you not to do. Everything else that can change an
//! answer is translated or refused.
//!
//! Two consequences worth stating, because both were once "documented
//! limitations" that returned different rows anyway:
//!
//! - An `INNER JOIN` on a ref hop drops the rows whose foreign key is null.
//!   A dot-path does not. The join is `fk = joined.id` against a declared
//!   ref, so the foreign key makes a non-null `fk` match exactly one row —
//!   which means the inner join's row set is *exactly* the dot-path's minus
//!   the null-`fk` rows, and `fk is not null` restores it. Emitted, not
//!   noted.
//! - Ordo's `group` always projects its keys, so `SELECT count(*) … GROUP BY
//!   team` used to come back with a column the SQL did not have. A `let`
//!   binding takes the projection: the group happens inside the `let` and
//!   the outer pipeline projects exactly the SELECT's own list, in the
//!   SELECT's own order.
//!
//! Places where the languages disagree and the bridge is spelled out:
//!
//! - **Null logic.** SQL's `=`, `<>` and `NOT col` are three-valued and a
//!   null operand drops the row; Ordo's `==`/`!=` compile to `IS [NOT]
//!   DISTINCT FROM` and are two-valued. Where the operand can be null the
//!   emitted Ordo carries an explicit `is not null` conjunct, and says so in
//!   a note. Without it `nickname <> 'B'` was one row in SQL and five in the
//!   decompilation, with nothing on the page to say which was meant.
//! - **Connective precedence.** SQL binds `AND` tighter than `OR`; Ordo has
//!   no precedence table at all and refuses to mix them unparenthesised. So
//!   the emitter parenthesises every mixed nesting, which preserves SQL's
//!   reading by construction.
//! - **Null placement in an ordering.** Postgres sorts nulls FIRST under
//!   `DESC` and LAST under `ASC`; Ordo emits `NULLS LAST` unconditionally,
//!   and `NULLS FIRST`/`NULLS LAST` were being parsed and thrown away. Under
//!   a `LIMIT` that is not a reordering, it is a different set of rows. A
//!   leading `cond(k is null -> 0, 1)` key reproduces SQL's placement
//!   exactly.
//! - **How a union lines its arms up.** SQL takes a union's column names
//!   from the FIRST SELECT and matches every later arm by POSITION; Ordo's
//!   `append` matches by NAME (DL §5). Emitting each later arm under the
//!   first arm's names, in the first arm's order, is not an approximation of
//!   SQL's rule — it *is* SQL's rule, written in the language that only has
//!   the other one. And the types must then agree exactly, where SQL would
//!   have coerced: that is checked here rather than left to arrive from the
//!   forward compiler as `append_misalignment`, at line 2 of source the
//!   reader never wrote.
//!
//! And the two that stay notes — one because no value changes, one in spite
//! of the fact that a value can:
//!
//! - **Text ordering collation.** Ordo pins `COLLATE "C"` on every text
//!   ordering so the answer does not move between deployments; Postgres's
//!   own default is the database's (`en_US.UTF-8` here, where `'a' < 'B'`
//!   is true and under `C` it is false). There is no Ordo spelling for
//!   another collation, and the pin is deliberate, so this is disclosed
//!   rather than refused — but it is disclosed on every text ordering key,
//!   and the note says a `LIMIT` turns it into a row difference.
//! - **`avg`'s scale.** Ordo pins scale 12; Postgres derives its scale from
//!   the operand. Same number, different number of decimal places.
//!
//! Every successful decompilation should be VERIFIED by the caller: compile
//! the emitted Ordo back through the forward compiler and (when a database
//! is present) compare rows. The playground does exactly that, and the
//! round-trip test runs it over the stage-two corpus whose row sets are
//! already blind-validated.

use std::collections::HashMap;

use ordo_schema::{FieldTy, Schema};
use ordo_syntax::ScalarTy;

#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub what: String,
}

fn no(what: impl Into<String>) -> Refusal {
    Refusal { what: what.into() }
}

/// A bind parameter, refused as one.
///
/// `$1` lexes as a word (the lexer takes `$` as a name character so it can
/// see `"$space"`), so it used to travel all the way to `check_segment` and
/// come back as "not a legal Ordo path segment — field names are
/// alphanumeric", which is true of `$1` and says nothing about what it is.
/// A pasted application query is full of them; the answer should be about
/// parameters.
fn placeholder(what: &str) -> Refusal {
    no(format!(
        "`{what}` is a bind parameter. This pane decompiles to a STANDALONE pipeline, and Ordo's parameters are declared on an `op` (`op feed($who: id) = …`), not on a bare query — so there is nowhere for `{what}` to be bound. Substitute the value you would have bound and the query decompiles; the `$who` version is what the op around it would look like."
    ))
}

// ── tokens ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),   // bare identifier or keyword (kept as written)
    Quoted(String), // "identifier"
    Str(String),    // 'literal'
    Num(String),
    Sym(String), // ( ) , . * = <> <= >= < >
}

fn lex(sql: &str) -> Result<Vec<Tok>, Refusal> {
    let mut out = Vec::new();
    let b: Vec<char> = sql.chars().collect();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '"' {
            let mut s = String::new();
            i += 1;
            while i < b.len() && b[i] != '"' {
                s.push(b[i]);
                i += 1;
            }
            if i == b.len() {
                return Err(no("unterminated quoted identifier"));
            }
            i += 1;
            out.push(Tok::Quoted(s));
        } else if c == '\'' {
            let mut s = String::new();
            i += 1;
            loop {
                if i >= b.len() {
                    return Err(no("unterminated string literal"));
                }
                if b[i] == '\'' {
                    if i + 1 < b.len() && b[i + 1] == '\'' {
                        s.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                s.push(b[i]);
                i += 1;
            }
            out.push(Tok::Str(s));
        } else if c.is_ascii_digit() {
            let mut s = String::new();
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == '.') {
                s.push(b[i]);
                i += 1;
            }
            out.push(Tok::Num(s));
        } else if c.is_alphabetic() || c == '_' || c == '$' {
            let mut s = String::new();
            while i < b.len() && (b[i].is_alphanumeric() || b[i] == '_' || b[i] == '$') {
                s.push(b[i]);
                i += 1;
            }
            out.push(Tok::Word(s));
        } else {
            let two: String = b[i..(i + 2).min(b.len())].iter().collect();
            if two == "<=" || two == ">=" || two == "<>" || two == "!=" {
                out.push(Tok::Sym(two));
                i += 2;
            } else if "(),.*=<>;".contains(c) {
                if c != ';' {
                    out.push(Tok::Sym(c.to_string()));
                }
                i += 1;
            } else if c == ':' && b.get(i + 1) == Some(&':') {
                return Err(no(
                    "a `::` cast is outside the v0 subset. Ordo has no cast operator: a column's type is the one the schema declares, and a projection cannot change it.",
                ));
            } else if c == '?' || c == ':' {
                // The other two placeholder spellings a driver emits. They
                // are not identifier characters, so without this they came
                // back as "character `?` is outside the v0 subset" — true,
                // and about the wrong thing.
                let mut s = String::from(c);
                let mut j = i + 1;
                while j < b.len() && (b[j].is_alphanumeric() || b[j] == '_') {
                    s.push(b[j]);
                    j += 1;
                }
                return Err(placeholder(&s));
            } else {
                return Err(no(format!("character `{c}` is outside the v0 subset")));
            }
        }
    }
    Ok(out)
}

// ── parsed shape ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct ColRef {
    alias: Option<String>,
    col: String,
}

#[derive(Debug, Clone)]
enum SelExpr {
    Col(ColRef),
    /// A constant column: `'order' AS kind`, `5 AS n`, `NULL AS n`.
    ///
    /// Ordo spells it as a literal projection entry (`kind: "order"`), which
    /// the forward compiler accepts for every literal kind SQL can put in a
    /// select list — string, integer, decimal, boolean, null — and lowers to
    /// the same constant column the SQL emits. Verified on Postgres rather
    /// than assumed; see `differential.rs`.
    Lit(Lit),
    Agg {
        func: String,
        arg: Option<ColRef>,
        star: bool,
    },
    /// coalesce(a, b) — the one scalar function in the v0 subset.
    Fn {
        name: String,
        args: Vec<ColRef>,
    },
}

#[derive(Debug, Clone)]
struct SelItem {
    expr: SelExpr,
    out: Option<String>,
}

#[derive(Debug, Clone)]
struct JoinCl {
    table: String,
    alias: String,
    left: ColRef,
    right: ColRef,
    inner: bool,
}

#[derive(Debug, Clone)]
enum CondAtom {
    /// `a OR b` — binds LOOSER than AND, as in SQL.
    Or(Vec<CondAtom>),
    /// `a AND b` — binds TIGHTER than OR, as in SQL.
    And(Vec<CondAtom>),
    /// strpos(col, 's') > 0  →  col contains "s"
    Contains {
        c: ColRef,
        s: String,
    },
    /// left(col, n) = 's' with n = len(s)  →  col starts "s"
    Starts {
        c: ColRef,
        s: String,
    },
    Cmp {
        l: ColRef,
        op: String,
        r: Lit,
    },
    CmpCol {
        l: ColRef,
        op: String,
        r: ColRef,
    },
    IsNull {
        c: ColRef,
        negated: bool,
    },
    In {
        c: ColRef,
        items: Vec<Lit>,
    },
    Bool {
        c: ColRef,
        negated: bool,
    },
}

#[derive(Debug, Clone)]
enum Lit {
    Num(String),
    Str(String),
    Bool(bool),
    Date(String),
    /// Only ever a SELECT-list column. `P::lit` does not produce it, so no
    /// condition can compare against it — SQL's `= NULL` is never true and
    /// Ordo's `== null` is, and the two have no bridge worth writing.
    Null,
}

/// The Ordo type a literal column takes.
///
/// Needed only by the `append` path, which must line the arms up by type as
/// well as by name: Ordo's `append` demands the types agree exactly, and a
/// mismatch it does not catch here arrives from the forward compiler as
/// `append_misalignment`, worded about source the user never wrote.
fn lit_ty(l: &Lit) -> ScalarTy {
    match l {
        // Postgres reads an unsuffixed `5` as `integer` and `5.5` as
        // `numeric`; Ordo's own literals type as `int` and as a `decimal`
        // with the places written (27ak).
        Lit::Num(n) if n.contains('.') => ScalarTy::Decimal(
            n.split_once('.')
                .map_or(0, |(_, f)| u8::try_from(f.len()).unwrap_or(u8::MAX)),
        ),
        Lit::Num(_) => ScalarTy::Int,
        // A bare `NULL` in a select list resolves to `text` in Postgres, and
        // Ordo's `null` literal types as `text` too. They agree standalone —
        // but SQL takes a union column's type from the OTHER arm and Ordo
        // does not, which is what the alignment check below is for.
        Lit::Str(_) | Lit::Null => ScalarTy::Text,
        Lit::Bool(_) => ScalarTy::Bool,
        Lit::Date(d) if d.len() > 10 => ScalarTy::Datetime,
        Lit::Date(_) => ScalarTy::Date,
    }
}

/// One `ORDER BY` key, with the null placement SQL actually asked for.
///
/// `nulls_first` was previously read and dropped on the floor
/// (`if p.kw("nulls") { p.kw("last"); p.kw("first"); }`), which meant
/// `ORDER BY x ASC NULLS FIRST` and `ORDER BY x ASC` decompiled to the same
/// Ordo. They are not the same query.
#[derive(Debug, Clone)]
struct OrderKey {
    c: ColRef,
    desc: bool,
    /// `None` = SQL's default for the direction: FIRST under DESC, LAST
    /// under ASC.
    nulls_first: Option<bool>,
}

impl OrderKey {
    fn nulls_first(&self) -> bool {
        self.nulls_first.unwrap_or(self.desc)
    }
}

/// One SELECT: everything up to (but not including) the ORDER BY and LIMIT,
/// which belong to the whole statement rather than to an arm of it.
#[derive(Debug, Clone)]
struct Sel {
    distinct: bool,
    /// `SELECT *` — or `SELECT t.*`, which is the same thing qualified, and
    /// carries the alias so the refusal can name the right table.
    star: bool,
    star_alias: Option<String>,
    items: Vec<SelItem>,
    table: String,
    table_alias: String,
    joins: Vec<JoinCl>,
    /// The whole WHERE as one expression tree — `None` when there is no
    /// WHERE. It was a `Vec` implicitly AND-ed, which is why `a AND b OR c`
    /// came out as `a AND (b OR c)`; see [`parse_or`].
    where_: Option<CondAtom>,
    group_by: Vec<ColRef>,
}

/// Which set operator joins a chain of SELECTs.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SetOp {
    /// `UNION ALL` — Ordo's `append`, name for name.
    All,
    /// `UNION` / `UNION DISTINCT` — `append` followed by `distinct`.
    Distinct,
}

/// A whole statement: one or more SELECTs, and the ordering and limit that
/// apply to all of them.
///
/// `ORDER BY` and `LIMIT` moved off `Sel` because after a set operation SQL
/// applies them to the WHOLE union, never to an arm — which is also where
/// Ordo puts them, after the last `append`. Leaving them on the arm would
/// have made "arm 2's ORDER BY" representable, and it is not a thing.
#[derive(Debug, Clone)]
struct Query {
    arms: Vec<Sel>,
    /// `None` for a single SELECT.
    set_op: Option<SetOp>,
    order_by: Vec<OrderKey>,
    limit: Option<i64>,
}

// ── parser ───────────────────────────────────────────────────────────────

struct P {
    t: Vec<Tok>,
    i: usize,
}

impl P {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.t.get(self.i).cloned();
        self.i += 1;
        t
    }
    fn kw(&mut self, w: &str) -> bool {
        if let Some(Tok::Word(s)) = self.peek() {
            if s.eq_ignore_ascii_case(w) {
                self.i += 1;
                return true;
            }
        }
        false
    }
    fn is_kw(&self, w: &str) -> bool {
        matches!(self.peek(), Some(Tok::Word(s)) if s.eq_ignore_ascii_case(w))
    }
    fn sym(&mut self, s: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Sym(x)) if x == s) {
            self.i += 1;
            return true;
        }
        false
    }
    /// An identifier, or a refusal that NAMES what stood there instead.
    ///
    /// This used to be `expected an identifier, found {other:?}` — a Rust
    /// `Debug` of a token, printed onto the page. It is the error the owner's
    /// many-branch feed hit (`found Str("order")`), and it is reachable from
    /// perfectly well-formed SQL in at least four more ways: a column
    /// ordinal (`ORDER BY 1`), a parameter placeholder (`$1`), a
    /// parenthesised set-operation arm, and the end of a truncated
    /// statement. Every one of them now says which construct it was and
    /// where the subset ends.
    fn ident(&mut self) -> Result<String, Refusal> {
        match self.next() {
            Some(Tok::Word(s)) if s.starts_with('$') => Err(placeholder(&s)),
            Some(Tok::Quoted(s)) | Some(Tok::Word(s)) => Ok(s),
            Some(Tok::Num(n)) => Err(no(format!(
                "expected a column name, found the number `{n}`. A column ORDINAL (`ORDER BY 1`, `GROUP BY 1`) is outside the v0 subset — Ordo's `sort` and `group` take names, not positions, so there is nothing to count from. Name the column."
            ))),
            Some(Tok::Str(s)) => Err(no(format!(
                "expected a column name, found the string literal `'{s}'`. A literal is a column only in a SELECT list, and there it needs an `AS` name — `'{s}' AS kind`."
            ))),
            Some(Tok::Sym(s)) => Err(no(format!(
                "expected a column name, found `{s}`. Expressions, subqueries and parenthesised set-operation arms are all outside the v0 subset."
            ))),
            None => Err(no(
                "the statement ends where a column name was expected — it is cut off",
            )),
        }
    }
    fn colref(&mut self) -> Result<ColRef, Refusal> {
        let a = self.ident()?;
        if self.sym(".") {
            if matches!(self.peek(), Some(Tok::Sym(s)) if s == "*") {
                return Err(no(format!(
                    "`{a}.*` is `SELECT *` qualified to one table, and is refused for the same reason: Ordo's whole-row form is the DECLARED OPEN fields, and `*` is every PHYSICAL column. Name the columns you want."
                )));
            }
            let c = self.ident()?;
            Ok(ColRef {
                alias: Some(a),
                col: c,
            })
        } else {
            Ok(ColRef {
                alias: None,
                col: a,
            })
        }
    }
    fn lit(&mut self) -> Result<Lit, Refusal> {
        // DATE '...' / TIMESTAMPTZ '...'
        if self.is_kw("date") || self.is_kw("timestamptz") {
            self.next();
            match self.next() {
                Some(Tok::Str(s)) => return Ok(Lit::Date(s)),
                _ => return Err(no("expected a string after DATE")),
            }
        }
        match self.next() {
            Some(Tok::Num(n)) => Ok(Lit::Num(n)),
            Some(Tok::Str(s)) => Ok(Lit::Str(s)),
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("true") => Ok(Lit::Bool(true)),
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("false") => Ok(Lit::Bool(false)),
            // `x = NULL` used to fall through to the column-to-column branch
            // and emit `x == null`, which is a DIFFERENT PREDICATE: SQL's
            // `= NULL` is unknown and matches nothing, and Ordo's `==` is
            // `IS NOT DISTINCT FROM`, so `x == null` matches every null row.
            // Silent, and a wrong answer in both directions — `= NULL`
            // returned rows it should not and `<> NULL` dropped rows it
            // should have kept.
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("null") => Err(no(
                "comparing against `NULL` with `=` or `<>` is outside the v0 subset. In SQL those are three-valued and match NOTHING — `x = NULL` is unknown for every row, null ones included. Ordo's `==`/`!=` are `IS [NOT] DISTINCT FROM`, so `x == null` matches exactly the null rows: the opposite answer. Write `IS NULL` / `IS NOT NULL` for the test, which decompiles.",
            )),
            Some(Tok::Word(w)) if w.starts_with('$') => Err(placeholder(&w)),
            other => Err(no(format!("literal outside the v0 subset: {other:?}"))),
        }
    }
}

/// A whole statement: a chain of SELECTs joined by a set operator, then the
/// ordering and limit that apply to all of them.
///
/// SQL's set operators are left-associative with equal precedence, so
/// `a UNION ALL b UNION c` means `(a UNION ALL b) UNION c` — a dedupe over
/// the whole thing. Ordo's `distinct` is a pipeline step and cannot dedupe
/// part of a chain, so a MIXED chain is refused by name rather than emitted
/// with the dedupe in the wrong place.
fn parse(sql: &str) -> Result<Query, Refusal> {
    let mut p = P { t: lex(sql)?, i: 0 };
    let mut arms = vec![parse_select(&mut p)?];
    let mut set_op: Option<SetOp> = None;
    loop {
        for (kw, what) in [("intersect", "INTERSECT"), ("except", "EXCEPT")] {
            if p.is_kw(kw) {
                return Err(no(format!(
                    "`{what}` has no Ordo spelling. Ordo's only set operation is `append`, which is UNION ALL; there is no intersection or difference step, and the semijoin/antijoin that would stand in for one is `match any`/`match none` over a declared ref — a shape a two-SELECT {what} does not carry enough information to reconstruct. Refused rather than guessed."
                )));
            }
        }
        if !p.is_kw("union") {
            break;
        }
        p.next();
        let op = if p.kw("all") {
            SetOp::All
        } else {
            p.kw("distinct");
            SetOp::Distinct
        };
        match set_op {
            None => set_op = Some(op),
            Some(prev) if prev != op => {
                return Err(no(
                    "a chain that mixes `UNION` and `UNION ALL` is outside the v0 subset. SQL's set operators are left-associative, so the dedupe applies to everything to its left — and Ordo's `distinct` is a pipeline step that can only dedupe the whole pipeline. There is no place to put it that means the same thing.",
                ))
            }
            _ => {}
        }
        arms.push(parse_select(&mut p)?);
    }
    let (order_by, limit) = parse_tail_clauses(&mut p)?;
    if p.peek().is_some() {
        return Err(no(format!(
            "trailing tokens outside the v0 subset: {:?}",
            p.peek()
        )));
    }
    Ok(Query {
        arms,
        set_op,
        order_by,
        limit,
    })
}

/// One SELECT of the chain, up to but not including ORDER BY / LIMIT.
fn parse_select(p: &mut P) -> Result<Sel, Refusal> {
    if matches!(p.peek(), Some(Tok::Sym(s)) if s == "(") {
        // `(SELECT … ORDER BY x LIMIT 2) UNION ALL (SELECT …)`. The parens
        // exist precisely so each arm can carry its own ORDER BY and LIMIT,
        // and Ordo's `append` takes ONE SOURCE FORM per arm (DL §1: "never a
        // nested pipeline") — an arm has no steps of its own. A branch that
        // needs them is a `let`, which is a different pipeline, not a
        // rewriting of this one.
        return Err(no(
            "a parenthesised set-operation arm is outside the v0 subset. The parentheses are there so the arm can carry its own ORDER BY or LIMIT, and Ordo's `append` takes exactly one SOURCE form per arm — no steps, so nowhere to put them. `sort` and `take` after the last `append` apply to the whole union, which is what an UNPARENTHESISED trailing ORDER BY/LIMIT means in SQL too; a per-arm one is a different query and would need a `let` per branch.",
        ));
    }
    if !p.kw("select") {
        return Err(no("only SELECT statements decompile"));
    }
    let distinct = p.kw("distinct");

    // `SELECT *`, and `SELECT t.*` which is the same thing qualified. Both
    // reach the argued refusal in `decompile`, which can name the fields.
    let star_alias = match (p.peek(), p.t.get(p.i + 1), p.t.get(p.i + 2)) {
        (Some(Tok::Word(a)), Some(Tok::Sym(d)), Some(Tok::Sym(s))) if d == "." && s == "*" => {
            let a = a.clone();
            p.i += 3;
            Some(a)
        }
        _ => None,
    };
    if star_alias.is_some() || p.sym("*") {
        if !p.kw("from") {
            return Err(no("expected FROM after *"));
        }
        let (table, table_alias) = parse_from(p)?;
        let mut sel = parse_body(p, table, table_alias)?;
        sel.distinct = distinct;
        sel.star = true;
        sel.star_alias = star_alias;
        return Ok(sel);
    }

    let mut items = Vec::new();
    loop {
        let expr = if let Some(tok) = p.peek().cloned() {
            match tok {
                // A constant column. Postgres reserves TRUE/FALSE/NULL, so an
                // unquoted one in a select list can only be the literal.
                Tok::Str(s) => {
                    p.next();
                    SelExpr::Lit(Lit::Str(s))
                }
                Tok::Num(n) => {
                    p.next();
                    SelExpr::Lit(Lit::Num(n))
                }
                Tok::Word(w) if w.eq_ignore_ascii_case("null") => {
                    p.next();
                    SelExpr::Lit(Lit::Null)
                }
                Tok::Word(w) if w.eq_ignore_ascii_case("true") => {
                    p.next();
                    SelExpr::Lit(Lit::Bool(true))
                }
                Tok::Word(w) if w.eq_ignore_ascii_case("false") => {
                    p.next();
                    SelExpr::Lit(Lit::Bool(false))
                }
                Tok::Word(w) if w.eq_ignore_ascii_case("case") => {
                    return Err(no(
                        "`CASE` in a SELECT list is outside the v0 subset. Ordo spells the shape `cond(test -> value, …)`, but the subset translates COLUMNS, not expressions — the emitter would be inventing a lowering nobody could check against the SQL.",
                    ))
                }
                Tok::Word(w) => parse_select_word(p, &w)?,
                _ => SelExpr::Col(p.colref()?),
            }
        } else {
            SelExpr::Col(p.colref()?)
        };
        let out = if p.kw("as") { Some(p.ident()?) } else { None };
        items.push(SelItem { expr, out });
        if !p.sym(",") {
            break;
        }
    }

    if !p.kw("from") {
        return Err(no("expected FROM"));
    }
    let (table, table_alias) = parse_from(p)?;
    let mut sel = parse_body(p, table, table_alias)?;
    sel.distinct = distinct;
    sel.items = items;
    Ok(sel)
}

/// `FROM <table> [AS] [alias]`.
fn parse_from(p: &mut P) -> Result<(String, String), Refusal> {
    let table = p.ident()?;
    let alias = if p.kw("as") || matches!(p.peek(), Some(Tok::Word(w)) if !is_clause_kw(w)) {
        p.ident()?
    } else {
        table.clone()
    };
    Ok((table, alias))
}

/// A select item that starts with a word: an aggregate, `coalesce`, another
/// function (refused BY ITS NAME), or a column path.
fn parse_select_word(p: &mut P, w: &str) -> Result<SelExpr, Refusal> {
    let f = w.to_ascii_lowercase();
    let called = matches!(p.t.get(p.i + 1), Some(Tok::Sym(s)) if s == "(");
    if !called {
        return Ok(SelExpr::Col(p.colref()?));
    }
    if f == "coalesce" {
        p.next();
        p.sym("(");
        let mut args = vec![p.colref()?];
        while p.sym(",") {
            args.push(p.colref()?);
        }
        if !p.sym(")") {
            return Err(no("expected `)` closing coalesce"));
        }
        return Ok(SelExpr::Fn { name: f, args });
    }
    if ["count", "sum", "avg", "min", "max"].contains(&f.as_str()) {
        p.next();
        p.sym("(");
        let (arg, star) = if p.sym("*") {
            (None, true)
        } else {
            (Some(p.colref()?), false)
        };
        if !p.sym(")") {
            return Err(no("expected `)` closing the aggregate"));
        }
        return Ok(SelExpr::Agg { func: f, arg, star });
    }
    // A call to something else. It used to fall through to `colref`, which
    // read the name as a column, found `(` where an `AS` or a comma should
    // be, and refused with "expected FROM" — naming the clause after the
    // one that was actually the problem.
    Err(no(format!(
        "`{w}(…)` is not one of the v0 subset's functions. The subset takes the aggregates `count`, `sum`, `avg`, `min`, `max`, and `coalesce`; every other function would have to be matched against an Ordo expression the emitter cannot check."
    )))
}

/// Everything after FROM <table> [alias] and before ORDER BY: joins, WHERE,
/// GROUP BY — shared by the star and item-list paths.
fn parse_body(p: &mut P, table: String, table_alias: String) -> Result<Sel, Refusal> {
    let mut joins = Vec::new();
    loop {
        let inner = if p.kw("left") {
            p.kw("outer");
            if !p.kw("join") {
                return Err(no("expected JOIN after LEFT"));
            }
            false
        } else if p.kw("inner") {
            if !p.kw("join") {
                return Err(no("expected JOIN after INNER"));
            }
            true
        } else if p.kw("join") {
            true
        } else {
            break;
        };
        let t = p.ident()?;
        let a = if p.kw("as")
            || matches!(p.peek(), Some(Tok::Word(w)) if !w.eq_ignore_ascii_case("on"))
        {
            p.ident()?
        } else {
            t.clone()
        };
        if !p.kw("on") {
            return Err(no("expected ON"));
        }
        let l = p.colref()?;
        if !p.sym("=") {
            return Err(no("only equality joins decompile"));
        }
        let r = p.colref()?;
        joins.push(JoinCl {
            table: t,
            alias: a,
            left: l,
            right: r,
            inner,
        });
    }

    let where_ = if p.kw("where") {
        Some(parse_or(p)?)
    } else {
        None
    };

    let mut group_by = Vec::new();
    if p.kw("group") {
        if !p.kw("by") {
            return Err(no("expected BY"));
        }
        loop {
            group_by.push(p.colref()?);
            if !p.sym(",") {
                break;
            }
        }
    }
    if p.kw("having") {
        return Err(no(
            "HAVING is outside the v0 subset — filter after group in Ordo (`| cond`)",
        ));
    }

    Ok(Sel {
        distinct: false,
        star: false,
        star_alias: None,
        items: Vec::new(),
        // set by the caller
        table,
        table_alias,
        joins,
        where_,
        group_by,
    })
}

/// `ORDER BY … LIMIT n` — the clauses that belong to the whole statement.
///
/// After a set operation SQL applies both to the union rather than to an
/// arm, which is also where Ordo puts them: after the last `append`.
fn parse_tail_clauses(p: &mut P) -> Result<(Vec<OrderKey>, Option<i64>), Refusal> {
    let mut order_by = Vec::new();
    if p.kw("order") {
        if !p.kw("by") {
            return Err(no("expected BY"));
        }
        loop {
            let c = p.colref()?;
            let desc = if p.kw("desc") {
                true
            } else {
                p.kw("asc");
                false
            };
            let nulls_first = if p.kw("nulls") {
                if p.kw("first") {
                    Some(true)
                } else if p.kw("last") {
                    Some(false)
                } else {
                    return Err(no("expected FIRST or LAST after NULLS"));
                }
            } else {
                None
            };
            order_by.push(OrderKey {
                c,
                desc,
                nulls_first,
            });
            if !p.sym(",") {
                break;
            }
        }
    }

    let mut limit = None;
    if p.kw("limit") {
        match p.next() {
            Some(Tok::Num(n)) => limit = Some(n.parse().map_err(|_| no("bad LIMIT"))?),
            _ => return Err(no("expected a number after LIMIT")),
        }
    }

    // `OFFSET` is refused BY NAME rather than as "trailing tokens". Ordo's
    // paging is a cursor, not a skip count: `page` resumes from a key, which
    // is a different (and stable-under-writes) question from "discard the
    // first n of this answer". There is no honest emitter spelling, and a
    // reader who pasted `LIMIT 10 OFFSET 20` deserves to be told which word
    // was the problem.
    if p.is_kw("offset") {
        return Err(no(
            "`OFFSET` has no Ordo spelling in the v0 subset. Ordo pages with a CURSOR — `page` resumes from a key rather than discarding the first n rows — so an offset is not a translation away from it; the two answer different questions once rows are written between pages.",
        ));
    }

    Ok((order_by, limit))
}

/// Words that cannot be a table alias, because they start the next clause.
///
/// A word missing from here is silently eaten as the alias — which is how
/// `SELECT name FROM people OFFSET 2` was refused as *trailing tokens:
/// Num("2")*, with `OFFSET` recorded as the table's alias. The refusal was
/// still a refusal (the catch-all at the end of `parse_tail` sees to that),
/// but it named the wrong token. The join-type and set-operation words are
/// here for the same reason: none of them is in the subset, and each should
/// be refused as itself.
fn is_clause_kw(w: &str) -> bool {
    [
        "left",
        "inner",
        "join",
        "where",
        "group",
        "order",
        "limit",
        "offset",
        "having",
        "on",
        "as",
        "union",
        "intersect",
        "except",
        "cross",
        "right",
        "full",
        "natural",
        "fetch",
    ]
    .iter()
    .any(|k| w.eq_ignore_ascii_case(k))
}

/// `or_expr := and_expr ('OR' and_expr)*` — OR binds looser, as in SQL.
///
/// The previous shape read a flat AND list and folded an OR into whichever
/// conjunct happened to precede it, so `a AND b OR c` decompiled to
/// `a and (b or c)`: a different predicate, emitted silently, with no note
/// and no refusal. Two nested functions are the whole fix.
fn parse_or(p: &mut P) -> Result<CondAtom, Refusal> {
    let mut arms = vec![parse_and(p)?];
    while p.kw("or") {
        arms.push(parse_and(p)?);
    }
    Ok(if arms.len() == 1 {
        arms.pop().expect("one arm")
    } else {
        CondAtom::Or(arms)
    })
}

fn parse_and(p: &mut P) -> Result<CondAtom, Refusal> {
    let mut arms = vec![parse_factor(p)?];
    while p.kw("and") {
        arms.push(parse_factor(p)?);
    }
    Ok(if arms.len() == 1 {
        arms.pop().expect("one arm")
    } else {
        CondAtom::And(arms)
    })
}

/// A parenthesised sub-expression or one atom. Parentheses had no rule at
/// all before, so `(a OR b) AND c` — the only way to spell that grouping in
/// SQL — was refused as "expected an identifier, found Sym(\"(\")".
fn parse_factor(p: &mut P) -> Result<CondAtom, Refusal> {
    if p.sym("(") {
        let inner = parse_or(p)?;
        if !p.sym(")") {
            return Err(no("expected `)` closing a parenthesised condition"));
        }
        return Ok(inner);
    }
    parse_cond(p)
}

fn parse_cond(p: &mut P) -> Result<CondAtom, Refusal> {
    if p.kw("not") {
        let c = p.colref()?;
        return Ok(CondAtom::Bool { c, negated: true });
    }
    // strpos(col, 's') > 0 — the planner's own `contains` lowering.
    if p.is_kw("strpos") {
        p.next();
        if !p.sym("(") {
            return Err(no("expected `(` after strpos"));
        }
        let c = p.colref()?;
        if !p.sym(",") {
            return Err(no("expected `,` in strpos"));
        }
        let s = match p.next() {
            Some(Tok::Str(s)) => s,
            _ => return Err(no("strpos needs a string needle")),
        };
        if !p.sym(")") || !p.sym(">") {
            return Err(no("only strpos(...) > 0 decompiles (contains)"));
        }
        match p.next() {
            Some(Tok::Num(n)) if n == "0" => {}
            _ => return Err(no("only strpos(...) > 0 decompiles (contains)")),
        }
        return Ok(CondAtom::Contains { c, s });
    }
    // left(col, n) = 's' with n = len(s) — the `starts` lowering.
    if p.is_kw("left") {
        p.next();
        if !p.sym("(") {
            return Err(no("expected `(` after left"));
        }
        let c = p.colref()?;
        if !p.sym(",") {
            return Err(no("expected `,` in left"));
        }
        // Either a number or length('s').
        let n: Option<usize> = match p.next() {
            Some(Tok::Num(n)) => n.parse().ok(),
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("length") => {
                if !p.sym("(") {
                    return Err(no("expected `(` after length"));
                }
                let inner = match p.next() {
                    Some(Tok::Str(s)) => Some(s.chars().count()),
                    _ => None,
                };
                if !p.sym(")") {
                    return Err(no("expected `)` closing length"));
                }
                inner
            }
            _ => None,
        };
        if !p.sym(")") || !p.sym("=") {
            return Err(no("only left(col, n) = 'prefix' decompiles (starts)"));
        }
        let s = match p.next() {
            Some(Tok::Str(s)) => s,
            _ => return Err(no("left(...) compares against a string prefix")),
        };
        if n != Some(s.chars().count()) {
            return Err(no("left length does not match the prefix — not a `starts`"));
        }
        return Ok(CondAtom::Starts { c, s });
    }
    let l = p.colref()?;
    if p.kw("is") {
        let negated = p.kw("not");
        if !p.kw("null") {
            return Err(no("expected NULL after IS"));
        }
        return Ok(CondAtom::IsNull { c: l, negated });
    }
    if p.kw("in") {
        if !p.sym("(") {
            return Err(no("expected `(` after IN"));
        }
        let mut items = Vec::new();
        loop {
            items.push(p.lit()?);
            if !p.sym(",") {
                break;
            }
        }
        if !p.sym(")") {
            return Err(no("expected `)` closing IN"));
        }
        return Ok(CondAtom::In { c: l, items });
    }
    for op in ["<=", ">=", "<>", "!=", "=", "<", ">"] {
        if p.sym(op) {
            // Column-to-column or column-to-literal.
            if matches!(p.peek(), Some(Tok::Quoted(_)))
                || matches!(p.peek(), Some(Tok::Word(w))
                    if !w.eq_ignore_ascii_case("true") && !w.eq_ignore_ascii_case("false")
                       && !w.eq_ignore_ascii_case("null")
                       && !w.eq_ignore_ascii_case("date") && !w.eq_ignore_ascii_case("timestamptz"))
            {
                let r = p.colref()?;
                return Ok(CondAtom::CmpCol {
                    l,
                    op: op.to_string(),
                    r,
                });
            }
            let r = p.lit()?;
            return Ok(CondAtom::Cmp {
                l,
                op: op.to_string(),
                r,
            });
        }
    }
    // Bare boolean column.
    Ok(CondAtom::Bool {
        c: l,
        negated: false,
    })
}

// ── emitter ──────────────────────────────────────────────────────────────

pub struct Decompiled {
    pub ordo: String,
    pub notes: Vec<String>,
}

/// The alias environment one SELECT's paths resolve in.
///
/// Extracted from the body of [`decompile`] when `append` arrived: every arm
/// of a UNION needs the same join-to-dot-path resolution, the same
/// nullability question and the same scalar type, and a second copy of that
/// reasoning is a second place for the null bridge to be subtly different.
struct Resolver<'a> {
    schema: &'a Schema,
    table_alias: String,
    /// FROM alias → `""` (own fields); join alias → the dot prefix when the
    /// join is fk-on-FROM-table = joined.id (a ref hop).
    prefix: HashMap<String, String>,
    /// The same aliases, resolved to (entity, reached-through-an-optional-ref).
    /// This is what makes the null translation decidable rather than guessed:
    /// SQL's `=` and `<>` are three-valued and Ordo's `==`/`!=` are not, so
    /// whether a guard is needed depends on whether the operand can be null.
    alias_ent: HashMap<String, (String, bool)>,
}

impl Resolver<'_> {
    fn col(&self, c: &ColRef) -> Result<String, Refusal> {
        check_segment(&c.col)?;
        match &c.alias {
            None => Ok(c.col.clone()),
            Some(a) => {
                let pre = self
                    .prefix
                    .get(a)
                    .ok_or_else(|| no(format!("unknown alias `{a}`")))?;
                Ok(format!("{pre}{}", c.col))
            }
        }
    }

    /// Can this path hold a null? Everything about the three-valued/
    /// two-valued translation turns on it.
    fn nullable(&self, c: &ColRef) -> bool {
        let a = c.alias.clone().unwrap_or_else(|| self.table_alias.clone());
        match self.alias_ent.get(&a) {
            // An alias we could not resolve is treated as nullable: the
            // guarded form is always *correct*, only sometimes redundant,
            // and a guess in the other direction changes rows.
            None => true,
            Some((ent, via_optional_ref)) => {
                *via_optional_ref
                    || self
                        .schema
                        .entity(ent)
                        .and_then(|e| e.field(&c.col))
                        .map(|f| f.optional)
                        .unwrap_or(true)
            }
        }
    }

    /// The scalar type a path resolves to. Needed for two things: deciding
    /// whether an ordering is a TEXT ordering (where Ordo's pinned `COLLATE
    /// "C"` and the database's own collation can disagree), and lining up
    /// the arms of an `append`, whose types must agree exactly.
    fn scalar(&self, c: &ColRef) -> Option<ScalarTy> {
        let a = c.alias.clone().unwrap_or_else(|| self.table_alias.clone());
        let (ent, _) = self.alias_ent.get(&a)?;
        self.schema
            .entity(ent)
            .and_then(|e| e.field(&c.col))
            .map(|f| f.ty.value_ty())
    }
}

/// Decompile a statement against a schema. The schema is what turns joins
/// into dot-paths: `LEFT JOIN people ld ON pr.lead = ld.id` becomes the
/// path prefix `lead.` when `lead` is a declared ref from the FROM table.
pub fn decompile(schema: &Schema, sql: &str) -> Result<Decompiled, Refusal> {
    let q = parse(sql)?;
    if q.arms.len() > 1 {
        decompile_union(schema, q)
    } else {
        decompile_one(schema, q)
    }
}

/// Resolve one SELECT's FROM and JOINs, folding an INNER JOIN's conjunct
/// into its WHERE. Returns the environment its paths resolve in, and the
/// notes the joins earned.
fn resolve_source<'a>(
    schema: &'a Schema,
    sel: &mut Sel,
) -> Result<(Resolver<'a>, Vec<String>), Refusal> {
    let mut notes = Vec::new();
    let entity = schema.entity(&sel.table).ok_or_else(|| {
        let mut have: Vec<&str> = schema.entities.iter().map(|e| e.name.as_str()).collect();
        have.sort_unstable();
        no(format!(
            "`{}` is not an entity of this fixture — it has: {}",
            sel.table,
            have.join(", ")
        ))
    })?;
    // `SELECT *` has no faithful Ordo spelling, so it is refused rather than
    // approximated.
    //
    // It used to become the bare source with a note saying what had been
    // dropped. But `SELECT * FROM people` is nineteen physical columns —
    // `$space`, `$rev`, `id`, the `email$bidx` blind-index digest and the
    // raw ciphertext of both sealed fields among them — and the bare source
    // is the twelve DECLARED OPEN fields. That is not a translation with a
    // caveat, it is a different query; the reverse pane printed `ROWS
    // DIVERGE` under it every time. The refusal carries the explicit
    // projection so it is one paste away from a query that does translate.
    if sel.star {
        if let Some(a) = &sel.star_alias {
            if *a != sel.table_alias {
                return Err(no(format!(
                    "`{a}.*` names a JOINED table. The v0 subset reaches a joined table through the dot-path a declared ref gives it, one column at a time — there is no whole-row form for the far side of a hop. Name the columns."
                )));
            }
        }
        let named = |f: &dyn Fn(&ordo_schema::Field) -> bool| -> Vec<String> {
            entity
                .fields
                .iter()
                .filter(|x| f(x))
                .map(|x| format!("`{}`", x.name))
                .collect()
        };
        let mut extra = vec!["the tenancy and revision columns `$space`/`$rev`".to_string()];
        let implicit = named(&|f| f.implicit);
        if !implicit.is_empty() {
            extra.push(format!("the platform fields {}", implicit.join(", ")));
        }
        let sealed = named(&|f| f.sealed);
        if !sealed.is_empty() {
            extra.push(format!(
                "the raw ciphertext (and blind-index digest) of {}",
                sealed.join(", ")
            ));
        }
        let open: Vec<&str> = entity
            .fields
            .iter()
            .filter(|f| !f.implicit && !f.sealed)
            .map(|f| f.name.as_str())
            .collect();
        return Err(no(format!(
            "`SELECT *` returns every PHYSICAL column of `{}`, including {}. Ordo's whole-row form is the DECLARED OPEN fields, a different set of columns, so there is no faithful translation — only one that quietly returns a narrower row. Name the columns: SELECT {}",
            sel.table,
            extra.join(", "),
            open.join(", ")
        )));
    }

    // Alias map: FROM alias → "" (own fields); join alias → dot prefix when
    // the join is fk-on-FROM-table = joined.id (a ref hop).
    let mut prefix: HashMap<String, String> = HashMap::new();
    prefix.insert(sel.table_alias.clone(), String::new());

    // The same aliases, resolved to (entity, reached-through-an-optional-ref).
    // This is what makes the null translation decidable rather than guessed:
    // SQL's `=` and `<>` are three-valued and Ordo's `==`/`!=` are not, so
    // whether a guard is needed depends on whether the operand can be null.
    let mut alias_ent: HashMap<String, (String, bool)> = HashMap::new();
    alias_ent.insert(sel.table_alias.clone(), (sel.table.clone(), false));

    // The conjuncts an INNER JOIN contributes — see below.
    let mut inner_guards: Vec<ColRef> = Vec::new();

    for j in &sel.joins {
        // Which side belongs to the FROM table?
        let (from_side, join_side) = if j.left.alias.as_deref() == Some(sel.table_alias.as_str()) {
            (&j.left, &j.right)
        } else if j.right.alias.as_deref() == Some(sel.table_alias.as_str()) {
            (&j.right, &j.left)
        } else {
            return Err(no(
                "a join not anchored on the FROM table is outside the v0 subset",
            ));
        };
        if join_side.col != "id" {
            return Err(no(
                "only fk = joined.id joins decompile (the ref-hop shape)",
            ));
        }
        let fk = &from_side.col;
        let fk_optional = entity.field(fk).map(|f| f.optional).unwrap_or(true);
        match entity.field(fk).map(|f| &f.ty) {
            Some(FieldTy::Ref { target, .. }) if *target == j.table => {
                prefix.insert(j.alias.clone(), format!("{fk}."));
                alias_ent.insert(j.alias.clone(), (j.table.clone(), fk_optional));
                // An INNER JOIN drops the rows with no match; a dot-path
                // keeps them, and this used to be a note. It does not have
                // to be. The join is `fk = joined.id` against a DECLARED
                // ref, so the foreign key guarantees a non-null `fk` matches
                // exactly one row of the target: the inner join's row set is
                // the dot-path's minus the null-`fk` rows, no duplication,
                // no reordering. `fk is not null` is therefore not an
                // approximation of the join — it is the join.
                //
                // A required ref cannot be null, so an INNER JOIN on one
                // drops nothing and gets no conjunct.
                if j.inner && fk_optional {
                    inner_guards.push(from_side.clone());
                    notes.push(format!(
                        "INNER JOIN `{}` became the dot-path `{fk}.` plus the conjunct `{fk} is not null` — Ordo's dots are outer-preserving, and that conjunct is exactly the rows the inner join drops",
                        j.table
                    ));
                }
            }
            _ => {
                return Err(no(format!(
                    "join on `{fk}` does not correspond to a declared ref to `{}` — the parent-side (collection) direction is not in v0",
                    j.table
                )))
            }
        }
    }

    // Fold the inner joins' conjuncts into the WHERE, so they go through the
    // same precedence-correct renderer as everything else — `(a OR b) AND
    // sponsor is not null`, parenthesised because Ordo has no precedence
    // table, rather than a string appended to a rendered condition.
    if !inner_guards.is_empty() {
        let extra = inner_guards
            .into_iter()
            .map(|c| CondAtom::IsNull { c, negated: true });
        sel.where_ = Some(match sel.where_.take() {
            None => {
                let mut v: Vec<CondAtom> = extra.collect();
                if v.len() == 1 {
                    v.pop().expect("one guard")
                } else {
                    CondAtom::And(v)
                }
            }
            // Flatten rather than nest: `a and b and g`, not `(a and b) and g`.
            Some(CondAtom::And(mut v)) => {
                v.extend(extra);
                CondAtom::And(v)
            }
            Some(w) => {
                let mut v = vec![w];
                v.extend(extra);
                CondAtom::And(v)
            }
        });
    }

    Ok((
        Resolver {
            schema,
            table_alias: sel.table_alias.clone(),
            prefix,
            alias_ent,
        },
        notes,
    ))
}

/// The single-SELECT path: everything the v0 subset had before `append`.
fn decompile_one(schema: &Schema, q: Query) -> Result<Decompiled, Refusal> {
    let Query {
        mut arms,
        order_by,
        limit,
        ..
    } = q;
    let mut sel = arms.pop().expect("one arm");
    let (resolver, mut notes) = resolve_source(schema, &mut sel)?;
    let render_col = |c: &ColRef| -> Result<String, Refusal> { resolver.col(c) };
    let nullable = |c: &ColRef| -> bool { resolver.nullable(c) };
    let scalar_of = |c: &ColRef| -> Option<ScalarTy> { resolver.scalar(c) };

    // ── outputs ─────────────────────────────────────────────────────────
    //
    // Every column the decompilation will produce, in order, under the name
    // it will carry.
    //
    // SQL lets two output columns share a name: `SELECT p.name, sp.name` is
    // two columns, both called `name`. An Ordo projection cannot say that,
    // and the forward compiler does not refuse it — it emits two `AS "name"`
    // columns and the reader gets ONE, the second silently standing where
    // the first was. So the collision is refused here, where the message can
    // name the SQL that caused it.
    let group_keys: Vec<String> = sel
        .group_by
        .iter()
        .map(&render_col)
        .collect::<Result<_, _>>()?;
    // A group names its keys by their LAST segment: `group assignee.team`
    // produces a column called `team`. Two keys with the same leaf therefore
    // collide even though their paths do not.
    let group_key_names: Vec<String> = sel.group_by.iter().map(|c| c.col.clone()).collect();
    let grouping = !group_keys.is_empty();
    if grouping {
        collides(&group_key_names, "GROUP BY key")?;
    }

    /// One aggregate binding, with the two facts the ordering needs about it.
    struct Agg {
        name: String,
        body: String,
        /// `count` cannot be null; `sum`/`min`/`max`/`avg` over an
        /// all-null bucket are.
        nullable: bool,
        /// `min`/`max` over text ARE a text ordering, and take the pin.
        text: bool,
    }

    let mut aggs: Vec<Agg> = Vec::new();
    let mut proj: Vec<String> = Vec::new();
    let mut out_names: Vec<String> = Vec::new();
    let mut has_avg = false;

    for it in &sel.items {
        match &it.expr {
            SelExpr::Col(c) => {
                let path = render_col(c)?;
                if grouping {
                    if !group_keys.contains(&path) {
                        return Err(no(format!(
                            "`{path}` is selected but is not a GROUP BY key — outside the v0 subset"
                        )));
                    }
                    if let Some(o) = &it.out {
                        if *o != c.col {
                            return Err(no(format!(
                                "a group key cannot be renamed: `{}` AS `{o}` — Ordo's `group` names its keys by their path, so the alias would be silently lost",
                                c.col
                            )));
                        }
                    }
                    out_names.push(c.col.clone());
                } else {
                    let default_name = c.col.clone();
                    match &it.out {
                        Some(o) if *o != default_name => {
                            check_name(o)?;
                            out_names.push(o.clone());
                            proj.push(format!("{o}: {path}"));
                        }
                        _ => {
                            out_names.push(default_name);
                            proj.push(path);
                        }
                    }
                }
            }
            SelExpr::Lit(l) => {
                if grouping {
                    return Err(no(
                        "a literal column alongside GROUP BY is outside the v0 subset — Ordo's `group` projects its keys and its aggregate bindings, and a constant is neither",
                    ));
                }
                let name = literal_name(it)?;
                proj.push(format!("{name}: {}", render_lit(l)?));
                out_names.push(name);
            }
            SelExpr::Agg { func, arg, star } => {
                let name = it
                    .out
                    .clone()
                    .ok_or_else(|| no("an aggregate needs an AS name to decompile"))?;
                check_name(&name)?;
                let a = if *star {
                    Agg {
                        name: name.clone(),
                        body: "count".into(),
                        nullable: false,
                        text: false,
                    }
                } else if func == "count" {
                    // COUNT(col) counts NON-NULL values — bare `count`
                    // counts rows, and the round-trip oracle caught the
                    // difference (W015: 3 aged people, 4 rows). The honest
                    // spelling is the documented sum-of-cond workaround.
                    //
                    // It is only honest where the input cannot be EMPTY.
                    // `sum` of no rows is null; SQL's `count` of no rows is
                    // 0. Inside a `group` every bucket holds at least one
                    // row and `cond` yields 0 or 1 and never null, so the
                    // sum is always a number and the spelling is exact.
                    // Ungrouped, `SELECT count(nickname) FROM people WHERE
                    // age > 1000` is `0` in SQL and `null` in the emitted
                    // Ordo — a different value, checked on Postgres. Ordo's
                    // aggregate braces take aggregates only, so there is no
                    // `coalesce(sum …, 0)` to wrap it in: refused.
                    if !grouping {
                        return Err(no(format!(
                            "`count({})` with no GROUP BY has no faithful Ordo spelling. It counts non-null values, which is `sum cond(… is null -> 0, 1)` — but `sum` over NO rows is null and SQL's `count` over no rows is 0, so an empty result would come back as `null` instead of `0`. Ordo's `reduce` braces admit aggregates only, so the `coalesce` that would bridge it cannot be written there. Use `count(*)` if you meant the row count, or add a GROUP BY (every bucket has a row, so the spelling is exact there).",
                            render_col(arg.as_ref().expect("non-star count has an arg"))?
                        )));
                    }
                    let a = render_col(arg.as_ref().expect("non-star count has an arg"))?;
                    notes.push(format!(
                        "count({a}) counts non-null values — emitted as `sum cond({a} is null -> 0, 1)`, Ordo's spelling for that"
                    ));
                    Agg {
                        name: name.clone(),
                        body: format!("sum cond({a} is null -> 0, 1)"),
                        nullable: false,
                        text: false,
                    }
                } else {
                    let argref = arg.as_ref().expect("non-star agg has an arg");
                    let a = render_col(argref)?;
                    has_avg |= func == "avg";
                    Agg {
                        name: name.clone(),
                        body: format!("{func} {a}"),
                        nullable: true,
                        text: matches!(func.as_str(), "min" | "max")
                            && scalar_of(argref) == Some(ScalarTy::Text),
                    }
                };
                aggs.push(a);
                out_names.push(name);
            }
            SelExpr::Fn { name: f, args } => {
                if grouping {
                    return Err(no(
                        "a computed column alongside GROUP BY is outside the v0 subset",
                    ));
                }
                let name = it
                    .out
                    .clone()
                    .ok_or_else(|| no("a computed column needs an AS name"))?;
                check_name(&name)?;
                let rendered: Vec<String> =
                    args.iter().map(&render_col).collect::<Result<_, _>>()?;
                proj.push(format!("{name}: {f}({})", rendered.join(", ")));
                out_names.push(name);
            }
        }
    }
    collides(&out_names, "output column")?;

    // What the group would produce if it projected itself: its keys, in
    // GROUP BY order, then the aggregates in SELECT order. When the SELECT
    // asks for anything else — a key it does not want, or the same columns
    // in a different order — the group goes inside a `let` and the outer
    // pipeline projects the SELECT's list verbatim.
    let natural: Vec<String> = group_key_names
        .iter()
        .cloned()
        .chain(aggs.iter().map(|a| a.name.clone()))
        .collect();
    if grouping {
        collides(&natural, "grouped output column")?;
    }
    let needs_let = grouping && out_names != natural;

    if has_avg {
        notes.push(
            "`avg` is pinned to scale 12 in Ordo so the answer does not move between deployments; Postgres derives its scale from the operand. The same number, printed with a different number of decimal places."
                .into(),
        );
    }

    // Conditions.
    //
    // The one place where the two languages genuinely disagree. SQL's `=`,
    // `<>` and `NOT col` are THREE-valued: a null operand makes the whole
    // predicate unknown and WHERE drops the row. Ordo's `==` and `!=` are
    // two-valued — they compile to `IS NOT DISTINCT FROM` / `IS DISTINCT
    // FROM`, so `null == null` is true and `null != 'x'` is true. Rendering
    // `<>` as a bare `!=` therefore returns rows SQL would not, silently:
    // on the `wide` seed, `nickname <> 'B'` is one row in SQL and five in
    // the old decompilation. The bridge is an explicit `is not null`
    // conjunct, emitted only where the operand can actually be null.
    let cx = Ctx {
        resolver: &resolver,
        notes: std::cell::RefCell::new(Vec::new()),
    };
    let cond = match &sel.where_ {
        Some(w) => Some(render_cond(w, &cx)?.0),
        None => None,
    };
    notes.extend(cx.notes.into_inner());

    // ── the ordering ────────────────────────────────────────────────────
    //
    // Resolve each ORDER BY key to the name it has in the pipeline at the
    // point `sort` runs. After a `group` that is the key's LAST segment or
    // an aggregate's name — `sort assignee.team` is a bind error, because
    // the row no longer has an `assignee`.
    let order_name = |k: &OrderKey| -> Result<(String, bool, bool), Refusal> {
        if !grouping {
            return Ok((render_col(&k.c)?, nullable(&k.c), is_text(scalar_of(&k.c))));
        }
        let path = render_col(&k.c)?;
        if let Some(i) = group_keys.iter().position(|p| *p == path) {
            return Ok((
                group_key_names[i].clone(),
                nullable(&sel.group_by[i]),
                is_text(scalar_of(&sel.group_by[i])),
            ));
        }
        if k.c.alias.is_none() {
            if let Some(a) = aggs.iter().find(|a| a.name == k.c.col) {
                return Ok((a.name.clone(), a.nullable, a.text));
            }
        }
        Err(no(format!(
            "`{path}` is an ORDER BY key that the grouped rows do not have — after a `group` the only names are the keys ({}) and the aggregates ({})",
            group_key_names.join(", "),
            if aggs.is_empty() {
                "none".to_string()
            } else {
                aggs.iter()
                    .map(|a| a.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        )))
    };

    let mut sort_keys: Vec<String> = Vec::new();
    let mut collated: Vec<String> = Vec::new();
    for (i, k) in order_by.iter().enumerate() {
        // The trailing implicit id tiebreak is the compiler's own; a
        // decompilation drops it rather than restating it. (`id` is never
        // null, so its null placement cannot be what is being asked for.)
        //
        // It has to be the FROM TABLE's own `id`. The test was `k.c.col ==
        // "id"` alone, which also swallowed `ORDER BY a.name, a.id` on a
        // joined alias — dropping the ASSIGNEE's id and leaving a note
        // saying "Ordo appends it itself", when what Ordo appends is the
        // TASK's id. A different tiebreak, and under a LIMIT a different
        // set of rows. A joined-side id is a real sort key and is kept.
        let is_own_id = k.c.col == "id"
            && match &k.c.alias {
                None => true,
                Some(a) => *a == sel.table_alias,
            };
        if !grouping && order_by.len() > 1 && i + 1 == order_by.len() && is_own_id && !k.desc {
            notes.push("dropped the trailing `id` tiebreak — Ordo appends it itself".into());
            continue;
        }
        let (name, may_be_null, text) = order_name(k)?;
        // Postgres sorts nulls FIRST under DESC and LAST under ASC, and
        // honours an explicit NULLS FIRST/LAST. Ordo's `sort` is always
        // NULLS LAST — there is no spelling for the other one. A leading
        // `cond(k is null -> 0, 1)` key IS that spelling: nulls take 0 and
        // sort ahead of everything, in one bucket, and the real key then
        // orders the rest. Without it `ORDER BY x DESC LIMIT 3` is a
        // DIFFERENT SET OF ROWS, not a different order.
        if k.nulls_first() && may_be_null {
            sort_keys.push(format!("cond({name} is null -> 0, 1)"));
            notes.push(format!(
                "`{name}` can be null and your SQL sorts its nulls FIRST ({}); Ordo's `sort` is always NULLS LAST, so the ordering carries a leading `cond({name} is null -> 0, 1)` key. Under a LIMIT that difference is a different set of rows, not a different order.",
                match k.nulls_first {
                    Some(_) => "you said so",
                    None => "Postgres's default under DESC",
                }
            ));
        }
        sort_keys.push(if k.desc {
            format!("-{name}")
        } else {
            name.clone()
        });
        if text {
            collated.push(name);
        }
    }
    if !collated.is_empty() {
        notes.push(collation_note(&collated));
    }

    // ── assemble ────────────────────────────────────────────────────────
    let mut source = sel.table.clone();
    if let Some(c) = &cond {
        source.push_str(&format!("({c})"));
    }

    let mut lines: Vec<String> = Vec::new();
    if grouping {
        let body = aggs
            .iter()
            .map(|a| format!("{}: {}", a.name, a.body))
            .collect::<Vec<_>>()
            .join(", ");
        let group_line = if aggs.is_empty() {
            format!("group {} {{ }}", group_keys.join(", "))
        } else {
            format!("group {} {{ {body} }}", group_keys.join(", "))
        };
        if needs_let {
            // Ordo's `group` always projects its keys, and projects them
            // first. That was a note ("the decompilation has one more column
            // than your SQL") standing over a real column difference on the
            // page built to make translations visible. It does not have to
            // be: a `let` binds the grouped pipeline and the OUTER source
            // projects whatever subset the SELECT asked for, in the SELECT's
            // own order.
            let bind = let_binding_name(schema);
            lines.push(format!("let {bind} = {source}"));
            lines.push(group_line);
            lines.push(String::new());
            lines.push(format!("{bind}[{}]", out_names.join(", ")));
            let unprojected: Vec<&String> = group_key_names
                .iter()
                .filter(|k| !out_names.contains(k))
                .collect();
            notes.push(if unprojected.is_empty() {
                format!("Ordo's `group` projects its keys first; your SELECT lists the same columns in another order, so the group is bound as `let {bind} = …` and the outer line projects your order.")
            } else {
                format!(
                    "Ordo's `group` always projects its keys, and your SELECT does not project {}. The group is bound as `let {bind} = …` so the outer line can project exactly your SELECT list — the grouping is unchanged, the extra column is not returned.",
                    unprojected
                        .iter()
                        .map(|k| format!("`{k}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            });
        } else {
            lines.push(source);
            lines.push(group_line);
        }
    } else {
        let mut line1 = source;
        if !proj.is_empty() {
            line1.push_str(&format!("[{}]", proj.join(", ")));
        }
        lines.push(line1);
        if !aggs.is_empty() {
            lines.push(format!(
                "reduce {{ {} }}",
                aggs.iter()
                    .map(|a| format!("{}: {}", a.name, a.body))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    if sel.distinct {
        lines.push("distinct".into());
    }
    if !sort_keys.is_empty() {
        lines.push(format!("sort {}", sort_keys.join(", ")));
    }
    if let Some(n) = limit {
        lines.push(format!("take {n}"));
        if order_by.is_empty() {
            notes.push(
                "LIMIT with no ORDER BY picks an arbitrary n in SQL; Ordo's `take` follows the implicit id order, so the decompilation is DEFINED where your SQL was not"
                    .into(),
            );
        } else {
            notes.push(
                "Ordo appends an implicit `id` tiebreak to every ordering. Where your ORDER BY has ties, SQL picks arbitrarily which of them falls inside the LIMIT and Ordo picks the lowest id — so a tied boundary can return different rows."
                    .into(),
            );
        }
    }

    Ok(Decompiled {
        ordo: lines.join("\n"),
        notes,
    })
}

/// The name a literal column will carry.
///
/// SQL will happily leave it unnamed — `SELECT 'order' FROM orders` comes
/// back as `?column?` — and an Ordo projection has no unnamed entry at all.
/// There is no name to invent that would be the SQL's; refused by name.
fn literal_name(it: &SelItem) -> Result<String, Refusal> {
    let name = it.out.clone().ok_or_else(|| {
        no("a literal column needs an `AS` name. SQL leaves an unnamed one called `?column?`; an Ordo projection entry is `name: value` and has no unnamed form, so there is no name to emit that would be the one your SQL produced. Write `'order' AS kind`.")
    })?;
    check_name(&name)?;
    Ok(name)
}

/// [`check_name`], as a value-returning form for `Option::map`.
fn check_named(name: &str) -> Result<String, Refusal> {
    check_name(name)?;
    Ok(name.to_string())
}

// ── conditions ───────────────────────────────────────────────────────────
//
// The one place where the two languages genuinely disagree. SQL's `=`,
// `<>` and `NOT col` are THREE-valued: a null operand makes the whole
// predicate unknown and WHERE drops the row. Ordo's `==` and `!=` are
// two-valued — they compile to `IS NOT DISTINCT FROM` / `IS DISTINCT
// FROM`, so `null == null` is true and `null != 'x'` is true. Rendering
// `<>` as a bare `!=` therefore returns rows SQL would not, silently:
// on the `wide` seed, `nickname <> 'B'` is one row in SQL and five in
// the old decompilation. The bridge is an explicit `is not null`
// conjunct, emitted only where the operand can actually be null.
//
// Lifted to module scope when `append` arrived: every arm of a union needs
// the same renderer, and a second copy of the null bridge is a second place
// for it to be subtly different.

struct Ctx<'a, 'b> {
    resolver: &'a Resolver<'b>,
    notes: std::cell::RefCell<Vec<String>>,
}
/// How tightly the rendered string binds, so a parent adds parentheses
/// only where they change the reading.
#[derive(PartialEq, PartialOrd, Clone, Copy)]
enum Prec {
    Atom,
    And,
    Or,
}
fn render_cond(w: &CondAtom, cx: &Ctx) -> Result<(String, Prec), Refusal> {
    let sub = |a: &CondAtom, floor: Prec| -> Result<String, Refusal> {
        let (s, p) = render_cond(a, cx)?;
        Ok(if p >= floor { format!("({s})") } else { s })
    };
    // `x <op> y and x is not null` — the three-valued reading, spelled
    // out. It is a conjunction standing where an atom stood, so it
    // reports itself as one.
    let guarded = |body: String, guards: Vec<String>| -> (String, Prec) {
        let mut guards = guards;
        guards.dedup();
        if guards.is_empty() {
            (body, Prec::Atom)
        } else {
            cx.notes.borrow_mut().push(format!(
                "SQL drops rows where an operand is null; Ordo's `==`/`!=` do not (`IS [NOT] DISTINCT FROM`), so `{}` carries an explicit `{}`",
                body,
                guards.join(" and ")
            ));
            (format!("{body} and {}", guards.join(" and ")), Prec::And)
        }
    };
    Ok(match w {
        // Ordo has NO precedence table (spec §1): mixing `and` and `or`
        // without parentheses is a parse error, not a default reading.
        // So an `and` arm inside an `or` is parenthesised even though
        // SQL would not have needed it — and SQL's own precedence is
        // exactly what those parentheses preserve.
        CondAtom::Or(arms) => (
            arms.iter()
                .map(|a| sub(a, Prec::And))
                .collect::<Result<Vec<_>, _>>()?
                .join(" or "),
            Prec::Or,
        ),
        CondAtom::And(arms) => (
            arms.iter()
                .map(|a| sub(a, Prec::And))
                .collect::<Result<Vec<_>, _>>()?
                .join(" and "),
            Prec::And,
        ),
        CondAtom::Contains { c, s } => (
            format!("{} contains {}", cx.resolver.col(c)?, render_str(s)?),
            Prec::Atom,
        ),
        CondAtom::Starts { c, s } => (
            format!("{} starts {}", cx.resolver.col(c)?, render_str(s)?),
            Prec::Atom,
        ),
        CondAtom::Cmp { l, op, r } => {
            let lhs = cx.resolver.col(l)?;
            let o = match op.as_str() {
                "=" => "==",
                "<>" | "!=" => "!=",
                other => other,
            };
            let body = format!("{lhs} {o} {}", render_lit(r)?);
            // `<` `<=` `>` `>=` already lower to `COALESCE(…, false)`,
            // which is SQL's own null behaviour; only the equalities
            // need the bridge, and only against a non-null literal
            // (there is no null literal in the v0 subset).
            let guards = if matches!(o, "==" | "!=") && cx.resolver.nullable(l) {
                vec![format!("{lhs} is not null")]
            } else {
                Vec::new()
            };
            guarded(body, guards)
        }
        CondAtom::CmpCol { l, op, r } => {
            let o = match op.as_str() {
                "=" => "==",
                "<>" | "!=" => "!=",
                other => other,
            };
            let (lhs, rhs) = (cx.resolver.col(l)?, cx.resolver.col(r)?);
            let body = format!("{lhs} {o} {rhs}");
            let mut guards = Vec::new();
            if matches!(o, "==" | "!=") {
                if cx.resolver.nullable(l) {
                    guards.push(format!("{lhs} is not null"));
                }
                if cx.resolver.nullable(r) {
                    guards.push(format!("{rhs} is not null"));
                }
            }
            guarded(body, guards)
        }
        CondAtom::IsNull { c, negated } => (
            format!(
                "{} is {}null",
                cx.resolver.col(c)?,
                if *negated { "not " } else { "" }
            ),
            Prec::Atom,
        ),
        CondAtom::In { c, items } => (
            format!(
                "{} in ({})",
                cx.resolver.col(c)?,
                items
                    .iter()
                    .map(render_lit)
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ")
            ),
            Prec::Atom,
        ),
        CondAtom::Bool { c, negated } => {
            let path = cx.resolver.col(c)?;
            if *negated {
                // `col` lowers to `col IS TRUE` (null → false, which is
                // SQL's own reading), but `not col` lowers to
                // `NOT (col IS TRUE)` — true for a null column, where
                // SQL's `NOT col` is unknown and drops the row.
                guarded(
                    format!("not {path}"),
                    if cx.resolver.nullable(c) {
                        vec![format!("{path} is not null")]
                    } else {
                        Vec::new()
                    },
                )
            } else {
                (path, Prec::Atom)
            }
        }
    })
}

// ── append: UNION ALL, name for name ─────────────────────────────────────

/// One output column of one arm of a union.
struct ArmCol {
    /// The name this arm gives it, if it gives it one. Only the FIRST arm's
    /// names survive into the union — SQL takes the union's column names
    /// from the first SELECT and matches the rest by position — so a later
    /// arm's `'refund'` or `r.reason` needs no `AS` at all, and usually has
    /// none. `None` is that: a column with a value and no name of its own.
    name: Option<String>,
    /// The Ordo path or literal this arm projects.
    body: String,
    ty: Option<ScalarTy>,
    nullable: bool,
}

/// One arm, reduced to a source line and its outputs.
struct Arm {
    entity: String,
    /// `orders(status == "done")` — the source form, conditions included.
    source: String,
    cols: Vec<ArmCol>,
    notes: Vec<String>,
}

/// `SELECT … UNION ALL SELECT …` → `source[…]` + `append source[…]`.
///
/// Ordo's `append` is UNION ALL and matches columns **by name** (DL §5);
/// SQL matches them **by position** and takes the union's names from the
/// FIRST arm. Those two rules agree exactly when every later arm renames its
/// columns to arm 1's names in arm 1's order — which is what this emits.
/// It is not an approximation of SQL's rule, it *is* SQL's rule written in
/// the language that only has the other one: position k of arm 2 is
/// projected as `<arm 1's name at k>: <arm 2's path>`, so the value SQL
/// would have put in that column is the value Ordo puts under that name.
///
/// The constraints Ordo adds on top, each refused by name rather than
/// approximated:
///
/// - **Types must agree exactly.** `append` does no coercion and no
///   null-filling, where SQL resolves a union column's type across the arms
///   (`int` against `numeric` becomes `numeric`; a bare `NULL` takes the
///   other arm's type). Checked here so the message can name the column and
///   the two types, instead of arriving from the forward compiler as
///   `append_misalignment` at line 2 of source the reader never wrote.
/// - **An entity may appear once as a source per pipeline** (DL §5). A
///   UNION of a table with itself has no Ordo spelling.
/// - **An arm is one source form**, never a pipeline (DL §1), so a per-arm
///   `GROUP BY`, aggregate or `DISTINCT` has nowhere to go.
fn decompile_union(schema: &Schema, q: Query) -> Result<Decompiled, Refusal> {
    let Query {
        arms: sels,
        set_op,
        order_by,
        limit,
    } = q;
    let set_op = set_op.expect("a multi-arm query has a set operator");
    let mut notes: Vec<String> = Vec::new();

    let mut arms: Vec<Arm> = Vec::new();
    for (i, mut sel) in sels.into_iter().enumerate() {
        let nth = ordinal(i);
        if sel.distinct {
            return Err(no(format!(
                "`SELECT DISTINCT` in the {nth} arm of a union is outside the v0 subset. Ordo's `distinct` is a pipeline step and dedupes the whole pipeline; there is no per-arm form, and putting it after the last `append` would dedupe rows the SQL keeps."
            )));
        }
        if !sel.group_by.is_empty() {
            return Err(no(format!(
                "a `GROUP BY` in the {nth} arm of a union is outside the v0 subset. Ordo's `append` takes exactly one SOURCE form per arm (DL §1: never a nested pipeline), and a `group` is a step — a grouped branch has to be bound with `let` first, which is a different pipeline rather than a rewriting of this one."
            )));
        }
        if sel
            .items
            .iter()
            .any(|it| matches!(it.expr, SelExpr::Agg { .. }))
        {
            return Err(no(format!(
                "an aggregate in the {nth} arm of a union is outside the v0 subset — an `append` arm is one source form, with no `reduce` or `group` step to hold one."
            )));
        }
        let (resolver, arm_notes) = resolve_source(schema, &mut sel)?;

        // Conditions, through the same precedence-correct, null-bridging
        // renderer the single-SELECT path uses.
        let cx = Ctx {
            resolver: &resolver,
            notes: std::cell::RefCell::new(Vec::new()),
        };
        let cond = match &sel.where_ {
            Some(w) => Some(render_cond(w, &cx)?.0),
            None => None,
        };
        let mut arm_notes = arm_notes;
        arm_notes.extend(cx.notes.into_inner());

        let mut source = sel.table.clone();
        if let Some(c) = &cond {
            source.push_str(&format!("({c})"));
        }

        let mut cols: Vec<ArmCol> = Vec::new();
        for it in &sel.items {
            match &it.expr {
                SelExpr::Col(c) => {
                    let path = resolver.col(c)?;
                    let name = match &it.out {
                        Some(o) => {
                            check_name(o)?;
                            o.clone()
                        }
                        // A bare column names itself, in every arm — which
                        // is also the name SQL would give the union if this
                        // were the first one.
                        None => c.col.clone(),
                    };
                    cols.push(ArmCol {
                        name: Some(name),
                        body: path,
                        ty: resolver.scalar(c),
                        nullable: resolver.nullable(c),
                    });
                }
                SelExpr::Lit(l) => {
                    // Only the FIRST arm has to name a literal: it is where
                    // the union's names come from. `SELECT 'refund', …` in a
                    // later arm is ordinary SQL and needs no `AS`.
                    let name = if i == 0 {
                        Some(literal_name(it)?)
                    } else {
                        it.out.as_deref().map(check_named).transpose()?
                    };
                    cols.push(ArmCol {
                        name,
                        body: render_lit(l)?,
                        ty: Some(lit_ty(l)),
                        nullable: matches!(l, Lit::Null),
                    });
                }
                SelExpr::Fn { name: f, args } => {
                    let name = match (&it.out, i) {
                        (Some(o), _) => Some(check_named(o)?),
                        (None, 0) => return Err(no("a computed column needs an AS name")),
                        (None, _) => None,
                    };
                    let rendered: Vec<String> = args
                        .iter()
                        .map(|a| resolver.col(a))
                        .collect::<Result<_, _>>()?;
                    cols.push(ArmCol {
                        name,
                        body: format!("{f}({})", rendered.join(", ")),
                        // `coalesce` takes the type of its arguments and is
                        // null only if its LAST argument is.
                        ty: args.first().and_then(|a| resolver.scalar(a)),
                        nullable: args.last().map(|a| resolver.nullable(a)).unwrap_or(true),
                    });
                }
                SelExpr::Agg { .. } => unreachable!("aggregates are refused above"),
            }
        }
        arms.push(Arm {
            entity: sel.table.clone(),
            source,
            cols,
            notes: arm_notes,
        });
    }

    // ── the arms have to line up ─────────────────────────────────────────
    let width = arms[0].cols.len();
    for (i, a) in arms.iter().enumerate().skip(1) {
        if a.cols.len() != width {
            return Err(no(format!(
                "the arms of the union select different numbers of columns ({} in the first, {} in the {}). SQL refuses that too — it is not a translation question, but the message should come from here rather than from Postgres.",
                width,
                a.cols.len(),
                ordinal(i)
            )));
        }
    }

    // DL §5: an entity may appear ONCE as a source per pipeline.
    for (i, a) in arms.iter().enumerate() {
        if let Some(j) = arms[..i].iter().position(|b| b.entity == a.entity) {
            return Err(no(format!(
                "`{}` is the source of both the {} and the {} arm. The definition language allows an entity to appear ONCE as a source per pipeline (DL §5) — two-role access goes through distinct refs and dot-paths, and a true self-union has no spelling. Note this refusal is NOT a divergence: the rows would agree today, because the forward compiler does not yet enforce the rule. It is refused because the emitted text would not be Ordo.",
                a.entity,
                ordinal(j),
                ordinal(i)
            )));
        }
    }

    // SQL takes the union's column NAMES from the first arm and matches
    // every later arm POSITIONALLY. Ordo matches by name. So arm 1's names
    // are the union's names, and each later arm renames onto them.
    let out_names: Vec<String> = arms[0]
        .cols
        .iter()
        .map(|c| c.name.clone().expect("the first arm names every column"))
        .collect();
    collides(&out_names, "output column")?;

    // A later arm that named a column something ELSE is worth a sentence:
    // SQL silently discards that name and Ordo would silently match on it.
    let mut renamed: Vec<String> = Vec::new();
    for (i, a) in arms.iter().enumerate().skip(1) {
        for (k, c) in a.cols.iter().enumerate() {
            if c.name.as_deref().is_some_and(|n| n != out_names[k]) {
                renamed.push(format!(
                    "`{}` in the {} arm becomes `{}`",
                    c.name.as_deref().unwrap_or_default(),
                    ordinal(i),
                    out_names[k]
                ));
            }
        }
    }
    if !renamed.is_empty() {
        notes.push(format!(
            "SQL takes a union's column names from the FIRST SELECT and lines the later arms up by POSITION; Ordo's `append` lines them up by NAME. So every later arm projects its column under the first arm's name — {}. That is SQL's own rule, written in the language that only has the other one: the value SQL would put in each column is the value Ordo puts under that name.",
            renamed.join(", ")
        ));
    }

    // Types must agree exactly: `append` does no coercion and no
    // null-filling, and SQL does both.
    for (k, out) in out_names.iter().enumerate() {
        let want = arms[0].cols[k].ty;
        for (i, a) in arms.iter().enumerate().skip(1) {
            let got = a.cols[k].ty;
            if got != want {
                let name = |t: Option<ScalarTy>| match t {
                    Some(t) => t.as_str().to_string(),
                    None => "unknown".to_string(),
                };
                return Err(no(format!(
                    "column `{}` is `{}` in the first arm and `{}` in the {} arm. SQL resolves a union column's type across the arms — `int` against `numeric` becomes `numeric`, and a bare `NULL` takes the other arm's type — and Ordo's `append` does neither: the types must agree exactly, with no coercion and no null-filling (DL §5). There is no projection that makes them agree without changing a value.",
                    out,
                    name(want),
                    name(got),
                    ordinal(i)
                )));
            }
        }
    }

    // A union column is nullable if ANY arm can put a null in it — which is
    // what the ordering's null placement turns on.
    let out_nullable: Vec<bool> = (0..width)
        .map(|k| arms.iter().any(|a| a.cols[k].nullable))
        .collect();
    let out_text: Vec<bool> = (0..width).map(|k| is_text(arms[0].cols[k].ty)).collect();

    for a in &arms {
        notes.extend(a.notes.iter().cloned());
    }

    // ── the ordering ─────────────────────────────────────────────────────
    //
    // After a set operation SQL's ORDER BY may only name an OUTPUT column of
    // the union — not a table's column, and not a qualified one. Ordo is the
    // same: after the last `append` the row has the union's columns and
    // nothing else.
    let mut sort_keys: Vec<String> = Vec::new();
    let mut collated: Vec<String> = Vec::new();
    for k in &order_by {
        if let Some(a) = &k.c.alias {
            return Err(no(format!(
                "`{a}.{}` is a QUALIFIED ORDER BY key after a union. SQL does not allow one there either — after a set operation the only names are the union's own output columns ({}) — and neither does Ordo: past the last `append` the row no longer has an `{a}`.",
                k.c.col,
                out_names.join(", ")
            )));
        }
        let i = out_names
            .iter()
            .position(|n| *n == k.c.col)
            .ok_or_else(|| {
                no(format!(
                    "`{}` is an ORDER BY key the union does not have. After a set operation the only names are the output columns of the FIRST SELECT: {}.",
                    k.c.col,
                    out_names.join(", ")
                ))
            })?;
        let name = out_names[i].clone();
        if k.nulls_first() && out_nullable[i] {
            sort_keys.push(format!("cond({name} is null -> 0, 1)"));
            notes.push(format!(
                "`{name}` can be null in at least one arm and your SQL sorts its nulls FIRST ({}); Ordo's `sort` is always NULLS LAST, so the ordering carries a leading `cond({name} is null -> 0, 1)` key. Under a LIMIT that difference is a different set of rows, not a different order.",
                match k.nulls_first {
                    Some(_) => "you said so",
                    None => "Postgres's default under DESC",
                }
            ));
        }
        sort_keys.push(if k.desc {
            format!("-{name}")
        } else {
            name.clone()
        });
        if out_text[i] {
            collated.push(name);
        }
    }
    if !collated.is_empty() {
        notes.push(collation_note(&collated));
    }

    // ── assemble ─────────────────────────────────────────────────────────
    let mut lines: Vec<String> = Vec::new();
    for (i, a) in arms.iter().enumerate() {
        let proj: Vec<String> = a
            .cols
            .iter()
            .enumerate()
            .map(|(k, c)| {
                // A bare path whose leaf already IS the union's name for that
                // column needs no rename.
                if c.body == out_names[k] {
                    c.body.clone()
                } else {
                    format!("{}: {}", out_names[k], c.body)
                }
            })
            .collect();
        let line = format!("{}[{}]", a.source, proj.join(", "));
        lines.push(if i == 0 {
            line
        } else {
            format!("append {line}")
        });
    }

    if set_op == SetOp::Distinct {
        lines.push("distinct".into());
        notes.push(
            "`UNION` (without ALL) dedupes; Ordo spells that as `append … distinct`, which dedupes over the projected outputs — the same columns SQL compares. Two details, both checked on this database rather than assumed: SQL and Ordo agree that two nulls are the SAME value for deduplication, and Ordo's `distinct` pins `COLLATE \"C\"` where SQL uses the database's collation. Under a DETERMINISTIC collation (Postgres's default, and this one) equality is byte equality either way and the two dedupe identically; under a nondeterministic collation they would not, and there is no Ordo spelling for one."
                .into(),
        );
    }
    if !sort_keys.is_empty() {
        lines.push(format!("sort {}", sort_keys.join(", ")));
    }
    if let Some(n) = limit {
        lines.push(format!("take {n}"));
        notes.push(if order_by.is_empty() {
            "LIMIT with no ORDER BY picks an arbitrary n in SQL; Ordo's `take` follows the append's own total order (arm, then the projected columns, then a per-arm row number), so the decompilation is DEFINED where your SQL was not."
                .into()
        } else {
            "Ordo makes every ordering total. After an `append` the tiebreak is the arm, then the projected columns, then a per-arm row number — not the `id` a single source would use, because the arms have different ids. Where your ORDER BY has ties, SQL picks arbitrarily which of them falls inside the LIMIT and Ordo picks by that rule, so a tied boundary can return different rows."
                .into()
        });
    }

    Ok(Decompiled {
        ordo: lines.join("\n"),
        notes,
    })
}

/// "first", "second", … for a refusal that has to say WHICH arm.
fn ordinal(i: usize) -> String {
    match i {
        0 => "first".into(),
        1 => "second".into(),
        2 => "third".into(),
        3 => "fourth".into(),
        n => format!("{}th", n + 1),
    }
}

/// Two output columns of the same name.
///
/// SQL permits it — `SELECT p.name, sp.name` is two columns, both `name` —
/// and Ordo does not. The forward compiler does not refuse it either: it
/// emits two `AS "name"` columns and the reader gets ONE. A column lost in
/// silence is the shape of defect this file exists to not have, so the
/// collision is named here.
fn collides(names: &[String], what: &str) -> Result<(), Refusal> {
    for (i, n) in names.iter().enumerate() {
        if names[..i].contains(n) {
            return Err(no(format!(
                "two columns would both be called `{n}`. SQL allows that and Ordo does not — the second silently stands where the first was, so one {what} disappears. Give one of them a distinct `AS` name."
            )));
        }
    }
    Ok(())
}

/// The name of the `let` binding the grouped form uses, chosen so it cannot
/// shadow an entity of the schema being decompiled against.
fn let_binding_name(schema: &Schema) -> String {
    let mut n = String::from("grouped");
    let mut i = 2;
    while schema.entity(&n).is_some() {
        n = format!("grouped{i}");
        i += 1;
    }
    n
}

/// The file's one standing exception, disclosed on every text ordering key.
///
/// Ordo has no spelling for another collation, so this cannot be bridged and
/// is not refused either — see the module doc. Shared by the single-SELECT
/// and `append` paths so the two cannot drift apart in what they promise.
fn collation_note(keys: &[String]) -> String {
    format!(
        "{} orders TEXT. Ordo pins `COLLATE \"C\"` on every text ordering so the answer does not move between deployments; your SQL takes the database's own collation, where (for example) `'a' < 'B'` can be true and under `C` it is false. There is no Ordo spelling for another collation. Values that differ only in case or punctuation can come back in a different order — and under a LIMIT, a different set of rows.",
        keys.iter()
            .map(|k| format!("`{k}`"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Does ordering by this take the text-collation pin?
fn is_text(t: Option<ScalarTy>) -> bool {
    matches!(t, Some(ScalarTy::Text) | Some(ScalarTy::Id))
}

/// Ordo names are lowerCamel with no underscores. A SQL `AS` alias is
/// conventionally snake_case, and emitting one produced Ordo that does not
/// parse — a refusal delivered by the *forward* compiler two steps later,
/// worded about a source the user never wrote.
fn check_name(name: &str) -> Result<(), Refusal> {
    let ok = name.starts_with(|c: char| c.is_ascii_lowercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric());
    if ok {
        Ok(())
    } else {
        Err(no(format!(
            "`{name}` is not a legal Ordo output name — names are lowerCamel with no underscores"
        )))
    }
}

/// A SQL string literal as an Ordo string literal.
///
/// It was `format!("\"{s}\"")` — no escaping at all. Ordo's string lexer has
/// a closed escape set (`\" \\ \n \t`), so a SQL literal was not being
/// quoted, it was being *re-lexed*: `'a\nb'` (a backslash and an `n`, under
/// `standard_conforming_strings`) became a NEWLINE in the emitted Ordo — a
/// different value, compared against silently — and `'x"y'` produced
/// `"x"y"`, which the forward compiler rejects as an unterminated string,
/// two steps later, worded about source the user never wrote.
fn render_str(s: &str) -> Result<String, Refusal> {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                return Err(no(format!(
                    "the literal contains U+{:04X}; Ordo's escapes are \\\" \\\\ \\n \\t and there is no spelling for that character",
                    c as u32
                )))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    Ok(out)
}

/// `DATE '…'` / `TIMESTAMPTZ '…'` were emitted verbatim, so Postgres's own
/// tolerance (`'2026-05-01 09:00:00+00'`, `'20260501'`, `'May 1 2026'`) fell
/// through into text no Ordo lexer accepts. Ordo's two shapes are exactly
/// `YYYY-MM-DD` and `YYYY-MM-DDThh:mm:ssZ`; anything else is refused here,
/// where the message can name the SQL that caused it.
fn check_temporal(s: &str) -> Result<(), Refusal> {
    let ok = |pat: &str| -> bool {
        pat.len() == s.len()
            && pat
                .chars()
                .zip(s.chars())
                .all(|(p, c)| if p == '9' { c.is_ascii_digit() } else { p == c })
    };
    if ok("9999-99-99") || ok("9999-99-99T99:99:99Z") {
        Ok(())
    } else {
        Err(no(format!(
            "`{s}` is not an Ordo temporal literal — the two shapes are `YYYY-MM-DD` and `YYYY-MM-DDThh:mm:ssZ` (UTC, the Z is mandatory)"
        )))
    }
}

/// A column identifier, as it will be spelled in an Ordo path.
///
/// `render_col` emitted whatever the SQL said, so `SELECT "$space"` became
/// the path segment `$space` and `SELECT "my col"` became two words — text
/// that only the forward compiler would reject, about a source the user
/// never wrote. Same reasoning as [`check_name`], one step earlier.
fn check_segment(name: &str) -> Result<(), Refusal> {
    let ok = name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric());
    if ok {
        Ok(())
    } else {
        Err(no(format!(
            "`{name}` is not a legal Ordo path segment — field names are alphanumeric and start with a letter, and the platform's `$`-prefixed columns are not addressable"
        )))
    }
}

fn render_lit(l: &Lit) -> Result<String, Refusal> {
    Ok(match l {
        Lit::Num(n) => n.clone(),
        Lit::Str(s) => render_str(s)?,
        Lit::Bool(b) => b.to_string(),
        Lit::Date(d) => {
            check_temporal(d)?;
            d.clone()
        }
        Lit::Null => "null".into(),
    })
}
