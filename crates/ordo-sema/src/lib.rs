//! P3 (slice) — the binder. Pipelines resolved against the schema graph,
//! composed ops inlined, bound IR out.
//!
//! # What this implements
//!
//! - Source resolution: entity or registered op, with cycle rejection.
//! - Parameter binding and flow-through, including defaults.
//! - Composition **with post-take semantics made structural**: a callee that
//!   carries a `take`, `group`, or `distinct` becomes a subquery, so the
//!   caller's conditions cannot reach inside it and change which rows the
//!   `take` took. A callee without one merges into the caller's query.
//! - Projection: subset, rename, computed, `[*]`, `[*, -field]`.
//! - Dot-paths through refs, as LEFT JOINs (a nullable ref must not drop the
//!   row it is dotted from).
//! - The implicit ordering tiebreak (R-C4/R-C7): declared sorts, then anchor
//!   id; grouped pipelines tiebreak on their group keys instead.
//!
//! # What this deliberately refuses, by name
//!
//! Every one of these returns a typed error rather than a wrong plan:
//!
//! - **Guards and `acl` (X-001).** Nothing here inlines a guard, because
//!   nothing parses one. Every query this binder produces is unguarded in the
//!   literal sense — it is a plain query over the whole table. This slice must
//!   not be read as row-level authorization of any kind.
//! - **Sealed fields.** A sealed field may be PROJECTED — it leaves as an
//!   envelope and the host opens it, which makes the plan host-class and
//!   is what the venue gate keys on. Every other position refuses
//!   (`sealed_unsupported`): the database cannot compare, sort, group, or
//!   compute from a value it cannot read, and doing that work host-side
//!   would mean examining rows the caller never asked for. Equality on a
//!   `sealed indexed` field is the exception, answered in SQL by the
//!   blind index.
//! - Multi-source lists and their join inference, `let`, `append`, `reduce`,
//!   `whole`/`subtree`, `under`/`above`, and `since` — all `unsupported`.

use std::collections::HashMap;

use ordo_syntax::decl::{
    AggDecl, MatchKind, OpBody, OpDecl, OpFlags, Pipeline, Program, ProjItem, ScalarTy, Source,
    SourceArg, Step, WriteStmt, DIVISION_SCALE,
};
use ordo_syntax::span::Span;
use ordo_syntax::{BinOp, Condition, Expr, TextOp};

use ordo_schema::{FieldTy, Schema};

pub mod ir;
mod prune;

pub use ir::{BCond, BExpr, Cte, From, Group, Join, OutField, Query, Sort, SourceJoin};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemaError {
    pub code: &'static str,
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for SemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {} at {}", self.code, self.message, self.span)
    }
}

impl std::error::Error for SemaError {}

/// **A whole `json` value is opaque to the language** (checklist 28b): it
/// is written whole and read whole. No condition, sort, group key,
/// aggregate, function or operator takes one, because an equality or order
/// over whole documents would be Postgres's `jsonb` rules arriving
/// unannounced. `is null` asks whether the field holds a document at all.
/// A condition may walk INTO a document with a path, typed by its other
/// side (ruled 2026-10-08): see [`doc_path`] and `Binder::operand`.
///
/// One class, `json_opaque`, for every site, with the site named.
fn refuse_json(e: &BExpr, span: Span, site: &str) -> Result<(), SemaError> {
    if e.ty() == ScalarTy::Json {
        return Err(err(
            "json_opaque",
            span,
            format!(
                "rule: a `json` field is read and written whole, so {site} cannot take one \
                 (DL §2, json).\n\
                 fix: pick the field and read the document in the app, or keep the value \
                 you need to filter or sort on in its own typed field"
            ),
        ));
    }
    Ok(())
}

/// A path that walked into a `json` field: `cur` is the field, `rest` the
/// steps inside its document. Legal only as one side of a condition (`doc`),
/// and never through a sealed field, whose document is ciphertext.
fn doc_path(cur: &SField, rest: &[ordo_syntax::Ident], doc: bool) -> Result<BExpr, SemaError> {
    let span = rest[0].span.to(rest[rest.len() - 1].span);
    if cur.sealed {
        return Err(err(
            "sealed_unsupported",
            span,
            format!(
                "rule: `{}` is a sealed `json` field, stored encrypted, so nothing inside \
                 its document can be filtered (DL §2, json).\n\
                 fix: drop `sealed` from the field, or keep the value you filter on in its \
                 own field",
                cur.name
            ),
        ));
    }
    if !doc {
        return Err(err(
            "json_opaque",
            span,
            format!(
                "rule: a path into the `json` field `{}` is legal only as one side of a \
                 condition, compared with a typed value (DL §2, json).\n\
                 fix: pick the whole field and read the document in the app, or keep the \
                 value in its own typed field",
                cur.name
            ),
        ));
    }
    Ok(BExpr::DocPath {
        doc: Box::new(cur.expr.clone()),
        keys: rest.iter().map(|s| s.name.clone()).collect(),
        ty: ScalarTy::Json,
    })
}

fn is_doc(e: &BExpr) -> bool {
    matches!(e, BExpr::DocPath { .. })
}

fn is_null_lit(e: &BExpr) -> bool {
    matches!(
        e,
        BExpr::Lit {
            value: Expr::NullLit { .. },
            ..
        }
    )
}

/// A document path read as `ty`.
fn typed_doc(e: BExpr, ty: ScalarTy) -> BExpr {
    match e {
        BExpr::DocPath { doc, keys, .. } => BExpr::DocPath { doc, keys, ty },
        other => other,
    }
}

/// **Typing by the other side.** A value inside a document has no schema
/// type, so a condition reads it as the type of what it is compared with:
/// text, a number (`int` or `decimal`, read as an exact `numeric`), or a
/// bool. Anything else has no JSON counterpart and is refused.
fn doc_side_ty(other: &BExpr, span: Span) -> Result<ScalarTy, SemaError> {
    if is_doc(other) {
        return Err(err(
            "json_path_type",
            span,
            "rule: both sides are paths into `json` documents, and neither has a type, so \
             the condition cannot say how to compare them (DL §2, json).\n\
             fix: compare each path with a typed value, such as a literal or a parameter",
        ));
    }
    if is_null_lit(other) {
        return Err(err(
            "json_path_type",
            span,
            "rule: `null` gives a path into a document no type to compare as (DL §2, json).\n\
             fix: ask `prefs.theme is null`, which is true when the key is missing or holds \
             JSON null",
        ));
    }
    match other.ty() {
        ScalarTy::Text => Ok(ScalarTy::Text),
        ScalarTy::Int => Ok(ScalarTy::Int),
        ScalarTy::Decimal(p) => Ok(ScalarTy::Decimal(p)),
        ScalarTy::Bool => Ok(ScalarTy::Bool),
        ScalarTy::Json => Err(err(
            "json_opaque",
            span,
            "rule: a whole `json` document is read and written whole, so it is never \
             compared, with a path into a document or with anything else (DL §2, json).\n\
             fix: compare the path with a text, a number or a bool",
        )),
        t => Err(err(
            "json_path_type",
            span,
            format!(
                "rule: a path into a `json` document compares only with text, a number \
                 (`int` or `decimal`) or a bool, the types JSON has; this side is `{}` \
                 (DL §2, json).\n\
                 fix: compare with a value of one of those types",
                t.as_str()
            ),
        )),
    }
}

/// A comparison with a document path on one side, typed by the other.
fn doc_pair(l: BExpr, r: BExpr, span: Span) -> Result<(BExpr, BExpr), SemaError> {
    if is_doc(&l) {
        let ty = doc_side_ty(&r, span)?;
        Ok((typed_doc(l, ty), r))
    } else {
        let ty = doc_side_ty(&l, span)?;
        Ok((l, typed_doc(r, ty)))
    }
}

/// `prefs.theme in ("dark", "dim")`: the list types the path. Every member
/// that is not `null` must agree on one JSON type.
fn doc_list_ty(items: &[BExpr], span: Span) -> Result<ScalarTy, SemaError> {
    let mut ty: Option<ScalarTy> = None;
    for i in items.iter().filter(|i| !is_null_lit(i)) {
        let t = doc_side_ty(i, span)?;
        ty = Some(match (ty, t) {
            (None, t) => t,
            (Some(a), b) if a == b => a,
            (Some(ScalarTy::Int | ScalarTy::Decimal(_)), ScalarTy::Int | ScalarTy::Decimal(_)) => {
                ScalarTy::Decimal(0)
            }
            (Some(a), b) => {
                return Err(err(
                    "json_path_type",
                    span,
                    format!(
                        "rule: the list types the path into the document, and it mixes `{}` \
                         and `{}` (DL §2, json).\n\
                         fix: list values of one type",
                        a.as_str(),
                        b.as_str()
                    ),
                ))
            }
        });
    }
    ty.ok_or_else(|| {
        err(
            "json_path_type",
            span,
            "rule: a list of only `null` gives a path into a document no type (DL §2, json).\n\
             fix: ask `prefs.theme is null`",
        )
    })
}

/// The left side of a text condition over a document. `starts` and
/// `matches` read the value as text. `contains` keeps the raw value: an
/// array holds an element, a string holds a substring, and which one is
/// asked is settled by the value's JSON type when the row is read (DL §2,
/// json; board `json-field` ask 6). A whole `json` field takes `contains`
/// as a path with no steps.
fn doc_text_lhs(op: TextOp, l: BExpr, r: &BExpr, span: Span) -> Result<BExpr, SemaError> {
    if is_doc(r) {
        return Err(err(
            "json_path_type",
            span,
            format!(
                "rule: `{}` asks about the value on its left, and the path into a document \
                 is on its right (DL §2, json).\n\
                 fix: put the path on the left: `prefs.tags contains \"red\"`",
                op.as_str()
            ),
        ));
    }
    let l = match l {
        BExpr::DocPath { .. } => l,
        whole => BExpr::DocPath {
            doc: Box::new(whole),
            keys: Vec::new(),
            ty: ScalarTy::Json,
        },
    };
    if let BExpr::DocPath { doc, .. } = &l {
        if let BExpr::Col { .. } = doc.as_ref() {
        } else {
            return Err(err(
                "json_opaque",
                span,
                "rule: only a stored `json` field can be searched (DL §2, json).\n\
                 fix: name the field",
            ));
        }
    }
    let ty = doc_side_ty(r, span)?;
    match op {
        TextOp::Contains => Ok(l),
        TextOp::Starts | TextOp::Matches if ty == ScalarTy::Text => Ok(typed_doc(l, ty)),
        _ => Err(err(
            "json_path_type",
            span,
            format!(
                "rule: `{}` reads the value in the document as text, and the other side is \
                 `{}` (DL §2, json).\n\
                 fix: compare with text",
                op.as_str(),
                ty.as_str()
            ),
        )),
    }
}

/// The `sealed_unsupported` message for a sealed value used where the
/// database must compute on it: a filter, a sort key, an aggregate's argument
/// or an expression. Postgres stores the field as an AEAD envelope, so no SQL
/// can sum, compare or order it; the plaintext exists only on the host, after
/// a key release.
///
/// A `group` or `reduce` over rows that carry a sealed field is plain SQL
/// (ruled 2026-10-05) because `count` reads no value. An aggregate OVER the
/// sealed value (`sum total`) is this refusal: summing it would need every
/// row opened on the host first, and that is not built.
fn sealed_in_sql(field: &str) -> String {
    format!(
        "`{field}` is sealed: the database holds only its ciphertext, so it cannot \
         filter, sort, aggregate or compute on it. Project `{field}` as it is to read \
         it opened on the server, `count` the rows that carry it, or declare it \
         `sealed indexed` to look it up with `==`"
    )
}

fn err(code: &'static str, span: Span, message: impl Into<String>) -> SemaError {
    SemaError {
        code,
        message: message.into(),
        span,
    }
}

/// Re-class a field miss raised inside a PROJECTION.
///
/// The class of a missing field is decided by the POSITION that named it,
/// not by the lookup that failed: the closed inventory has
/// `projection_unknown_field` for "projection names a field outside the
/// callee's output shape" (`composition-conformance.md` §0,
/// `api-engine-design.md` §5) and `not_output_field` for a condition,
/// group/take key or sort key. `lookup` cannot know which it is in, so the
/// projection path relabels on the way out.
///
/// `unknown_field` — the internal class for "this entity has no such
/// field" — appears in NO closed inventory, so it must never be the class
/// a reject case sees (checklist 10f, GC088: `[member.name]` used to
/// answer `unknown_field` where the document says
/// `projection_unknown_field`). Dotted projections go through `path`, which
/// raises `unknown_field`, so both spellings are relabelled here.
fn as_projection_miss(e: SemaError) -> SemaError {
    if e.code == "not_output_field" || e.code == "unknown_field" {
        SemaError {
            code: "projection_unknown_field",
            ..e
        }
    } else {
        e
    }
}

/// The deployed op set. Ops resolve by name from here, exactly as they will
/// at runtime from the artifact registry.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    ops: HashMap<String, OpDecl>,
}

impl Registry {
    pub fn from_program(p: &Program) -> Registry {
        let mut ops = HashMap::new();
        for o in &p.ops {
            ops.insert(o.name.name.clone(), o.clone());
        }
        Registry { ops }
    }

    pub fn get(&self, name: &str) -> Option<&OpDecl> {
        self.ops.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.ops.keys()
    }

    /// Register additional ops — corpus cases may declare local ones.
    pub fn extend_from(&mut self, p: &Program) {
        for o in &p.ops {
            self.ops.insert(o.name.name.clone(), o.clone());
        }
    }

    /// Names the platform binds and an op may not declare.
    ///
    /// `space` is the whole list. It is bound at the door, before the
    /// access check, and carried into the executor as its own argument —
    /// so a declared `$space` would put a **second, caller-controlled**
    /// space in the parameter map (SH2-1). The envelope already strips
    /// `$`-prefixed keys and `ordo_api::OpSpec::validate` already refuses a
    /// `$`-prefixed parameter; this is the same rule one layer earlier, at
    /// the source, where the author gets told rather than the request.
    ///
    /// `caller` is deliberately NOT here: `$caller` is a guard-language
    /// keyword the parser recognises, not a parameter an op can declare,
    /// so there is nothing to refuse.
    pub fn check_reserved_params(&self) -> Result<(), SemaError> {
        let mut names: Vec<&String> = self.ops.keys().collect();
        names.sort();
        for name in names {
            let op = &self.ops[name];
            for p in &op.params {
                if p.name.name == "space" {
                    return Err(err(
                        "reserved_param",
                        p.name.span,
                        format!(
                            "`{name}` declares `$space`, which the platform binds. The space a \
                             request runs in comes from the door, before the access check, and \
                             nothing a caller sends can name it — a parameter by that name \
                             would be a second space the caller controls. Rename it."
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// A `money(p)` or `decimal(p)` parameter's bound and default are
    /// written in major units and must fit the places (27ak): `$cap:
    /// money(2) <= 100.001` is a bound the door could only keep by
    /// rounding, so it is `implicit_rounding` at registration.
    pub fn check_param_places(&self) -> Result<(), SemaError> {
        let mut names: Vec<&String> = self.ops.keys().collect();
        names.sort();
        for name in names {
            for p in &self.ops[name].params {
                if !matches!(p.ty, ScalarTy::Money(_) | ScalarTy::Decimal(_)) {
                    continue;
                }
                for e in [&p.max, &p.default].into_iter().flatten() {
                    let v = fold_neg_lit(e);
                    let Some(ty) = lit_ty(&v) else { continue };
                    if !matches!(ty, ScalarTy::Int | ScalarTy::Decimal(_)) {
                        continue;
                    }
                    let span = v.span();
                    fit_slot(BExpr::Lit { value: v, ty }, p.ty, span).map_err(|e| SemaError {
                        message: format!("`{name}` declares `${}`: {}", p.name.name, e.message),
                        ..e
                    })?;
                }
            }
        }
        Ok(())
    }

    /// **`filled` marks only `date` and `datetime`** (ruled 2026-10-05,
    /// `dl-spec-gaps` ask 5). Those are the values a client can produce
    /// alone, from its own clock. On any other type the generated client
    /// has nothing to fill it with, so the mark is refused at
    /// registration, class `filled_type`.
    pub fn check_filled_types(&self) -> Result<(), SemaError> {
        let mut names: Vec<&String> = self.ops.keys().collect();
        names.sort();
        for name in names {
            for p in &self.ops[name].params {
                if p.filled && !matches!(p.ty, ScalarTy::Date | ScalarTy::Datetime) {
                    return Err(err(
                        "filled_type",
                        p.name.span,
                        format!(
                            "`{name}` marks `${}` as `filled`, and `filled` is for `date` and \
                             `datetime` only: the generated client fills those from the \
                             caller's clock and can produce no other value on its own; drop \
                             `filled` and let the caller pass it",
                            p.name.name
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// **A `blob` is never a parameter** (DL §2, Content types).
    ///
    /// Bytes are not a JSON wire value, and `api-engine-design.md` §5's
    /// `ParamTy` table mirrors the language only for values that fit an
    /// envelope key. Bytes arrive through the blob door, which is the
    /// reason that door exists as a second route rather than as an op.
    ///
    /// # Why the refusal is here and not at the door
    ///
    /// Because the door has no way to say no that an author ever reads. A
    /// registration gate refuses the *program*, with a span, at compile
    /// time; the door would refuse a *request*, at 3 a.m., to a caller who
    /// did not write the op. `ordo_host::param_ty` therefore answers
    /// `None` for `blob` and its caller panics on `None` rather than
    /// falling back to text: two mechanisms saying the same thing, with
    /// this one being the one that talks to a human.
    ///
    /// The obvious workaround — `$f: text` holding a hex handle — is worse
    /// than it looks and is what the `fix:` line steers away from: a
    /// caller-supplied handle names an object the caller did not upload,
    /// and the only thing standing between that and a cross-row read is
    /// the `("$space", "ref")` predicate. Uploading through the door means
    /// the platform mints the handle, so there is no such request to make.
    pub fn check_content_params(&self) -> Result<(), SemaError> {
        let mut names: Vec<&String> = self.ops.keys().collect();
        names.sort();
        for name in names {
            let op = &self.ops[name];
            for p in &op.params {
                if p.ty.is_content() {
                    return Err(err(
                        "blob_parameter",
                        p.name.span,
                        format!(
                            "rule: bytes are not a JSON wire value, so `blob` is not a \
                             parameter type — `{name}` declares `${}: blob`.\n\
                             fix: upload through the blob door \
                             (`PUT /blob/<entity>/<field>/<id>`), which writes the field \
                             and mints its handle; an op parameter cannot name an object \
                             the caller did not put there",
                            p.name.name
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// DL "Other steps" [settled]: *"`take $n` — legal: the count may be a
    /// parameter, and the parameter must carry a bound (`$n: int <= 200`)
    /// so the op's cost class is still statically provable. An unbounded
    /// parameter here is `unbounded_take`."*
    ///
    /// # Why this is a gate and not a lint
    ///
    /// `take` is the row-bound prover's main evidence, and a `take $n`
    /// whose value arrives with the call is evidence of nothing on its
    /// own: `take $n` with no bound proves at most "the caller will get as
    /// many rows as the caller asked for", which is the definition of
    /// unbounded. The declared bound is what turns it back into a proof,
    /// and it is only a proof because the DOOR refuses a value above it
    /// (`bound_violation`, DL §2: "Bounds are enforced at the door, before
    /// planning"). Registration and the door are two halves of one
    /// argument; this half without the other is a number nobody checks,
    /// and the other without this one is a check on a number nobody used.
    ///
    /// The rule covers `take[keys] $n` too, though its own count is
    /// deliberately not row-bound evidence. DL states the rule about `take`
    /// with a parameter count and draws no distinction, and one rule for
    /// one construct beats two rules that differ by a bracket.
    pub fn check_take_bounds(&self) -> Result<(), SemaError> {
        fn takes(pl: &Pipeline, out: &mut Vec<ordo_syntax::Ident>) {
            for l in &pl.lets {
                takes(&l.pipeline, out);
            }
            for st in &pl.steps {
                if let Step::Take {
                    n: ordo_syntax::TakeCount::Param { name },
                    ..
                } = st
                {
                    out.push(name.clone());
                }
            }
        }
        let mut names: Vec<&String> = self.ops.keys().collect();
        names.sort();
        for opname in names {
            let op = &self.ops[opname];
            let Some(pl) = op.read_pipeline() else {
                continue;
            };
            let mut found = Vec::new();
            takes(pl, &mut found);
            for pname in found {
                let Some(decl) = op.params.iter().find(|p| p.name.name == pname.name) else {
                    return Err(err(
                        "unknown_param",
                        pname.span,
                        format!(
                            "`{opname}` takes `${}`, which it does not declare",
                            pname.name
                        ),
                    ));
                };
                if decl.ty != ordo_syntax::ScalarTy::Int {
                    return Err(err(
                        "param_type_mismatch",
                        pname.span,
                        format!(
                            "`take ${}` counts rows, so `${}` must be `int`, not `{}`",
                            pname.name,
                            pname.name,
                            decl.ty.as_str()
                        ),
                    ));
                }
                if decl.max.is_none() {
                    return Err(err(
                        "unbounded_take",
                        pname.span,
                        format!(
                            "`{opname}` takes `${p}` rows and `${p}` carries no bound, so \
                             nothing bounds what this op returns — declare \
                             `${p}: int <= <n>`",
                            p = pname.name
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// **`reproducible` forbids the clock** (DL §13, the op wrapper:
    /// "`reproducible` (forbids `since`)"; DL §4 "`since` and
    /// reproducibility"; DL §2 Parameters: "Marked `filled` and also
    /// `reproducible` on the same op is a compile error: filling from a
    /// clock is the nondeterminism `reproducible` forbids").
    ///
    /// Two producers of execution-time values exist in the language and
    /// this refuses both under one class, `reproducible_clock`: a `since`
    /// window anywhere in the op's own pipeline (its `let`s included), and
    /// a `filled` parameter. A composed callee's `since` is **not** this
    /// op's to refuse here — the callee is registered on its own terms,
    /// and an op that wants reproducibility over a `since`-bearing callee
    /// has written a contradiction the callee's own flag would have to
    /// resolve; that viral half is owed and recorded in the message.
    pub fn check_reproducible(&self) -> Result<(), SemaError> {
        fn cond_has_since(c: &Condition) -> Option<Span> {
            match c {
                Condition::Since { span, .. } => Some(*span),
                Condition::And { items, .. } | Condition::Or { items, .. } => {
                    items.iter().find_map(cond_has_since)
                }
                Condition::Not { inner, .. } => cond_has_since(inner),
                _ => None,
            }
        }
        fn pipeline_has_since(pl: &Pipeline) -> Option<Span> {
            for l in &pl.lets {
                if let Some(s) = pipeline_has_since(&l.pipeline) {
                    return Some(s);
                }
            }
            for src in &pl.sources {
                for a in &src.args {
                    if let SourceArg::Cond(c) = a {
                        if let Some(s) = cond_has_since(c) {
                            return Some(s);
                        }
                    }
                }
            }
            for st in &pl.steps {
                let found = match st {
                    Step::Filter { cond, .. } => cond_has_since(cond),
                    Step::Match {
                        kind: MatchKind::Any(src) | MatchKind::None(src),
                        ..
                    }
                    | Step::Append { source: src, .. } => src.args.iter().find_map(|a| match a {
                        SourceArg::Cond(c) => cond_has_since(c),
                        SourceArg::Named { .. } => None,
                    }),
                    _ => None,
                };
                if found.is_some() {
                    return found;
                }
            }
            None
        }
        let mut names: Vec<&String> = self.ops.keys().collect();
        names.sort();
        for name in names {
            let op = &self.ops[name];
            if !op.flags.reproducible {
                continue;
            }
            if let Some(p) = op.params.iter().find(|p| p.filled) {
                return Err(err(
                    "reproducible_clock",
                    p.name.span,
                    format!(
                        "`{name}` is `reproducible` and `${}` is `filled` — a filled parameter \
                         is produced from the caller's clock, which is the nondeterminism \
                         `reproducible` forbids; drop one of the two",
                        p.name.name
                    ),
                ));
            }
            // A write's condition is a pipeline-shaped filter too: `set
            // orders(placed since 7d) { … }` ranges over a window the clock
            // decides, so the same call converges different rows tomorrow.
            let since = match &op.body {
                OpBody::Read(pl) => pipeline_has_since(pl),
                OpBody::Write(stmts) => stmts.iter().find_map(|st| match st {
                    WriteStmt::Set { cond: Some(c), .. } | WriteStmt::Del { cond: c, .. } => {
                        cond_has_since(c)
                    }
                    WriteStmt::Set { cond: None, .. } => None,
                }),
            };
            if let Some(span) = since {
                return Err(err(
                    "reproducible_clock",
                    span,
                    format!(
                        "`{name}` is `reproducible` and uses `since`, which anchors to execution \
                         time — the same call returns different rows tomorrow; bound the window \
                         with a date parameter instead (DL §4, \"`since` and reproducibility\")"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// R-C9: an op that composes an `unbounded` callee must carry the
    /// acknowledgment itself — greppability is the point. Each level must
    /// re-acknowledge, so a direct check suffices. "Composes" means a
    /// source **or** an `append`: both put the callee's rows in this op's
    /// result.
    pub fn check_acknowledgments(&self) -> Result<(), SemaError> {
        for op in self.ops.values() {
            if op.flags.unbounded {
                continue;
            }
            // A caller that provably bounds its OWN FINAL stream — a
            // keyless `take n` or a `reduce` with no `append` after it —
            // composes a row-unbounded callee without re-acknowledging
            // (C048's top2Cheap). A take BEFORE an append bounds nothing
            // (the audit's `take 5 append feed`); and per DL ~1816 a take
            // bounds RETURNED rows, not EXAMINED ones, so when host-side
            // staging exists (F5) this skip must additionally require the
            // callee's venue class to be non-host — recorded in the
            // rulings ledger, unreachable today because sealed filters
            // refuse to bind.
            let Some(pipeline) = op.read_pipeline() else {
                // A write group addresses rows; the row-work story for
                // writes is P5's own (bulk ranges over the write view).
                continue;
            };
            let last_append = pipeline
                .steps
                .iter()
                .rposition(|st| matches!(st, Step::Append { .. }));
            let self_bounded = pipeline.steps.iter().enumerate().any(|(i, st)| {
                (matches!(st, Step::Take { keys, .. } if keys.is_empty())
                    || matches!(st, Step::Reduce { .. }))
                    && last_append.map(|a| i > a).unwrap_or(true)
            });
            if self_bounded {
                continue;
            }
            // Sources AND `append`ed callees. An `append` concatenates the
            // callee's rows into this op's own stream — they *are* the
            // caller's results — so it composes the callee in the only
            // sense R-C9 is about, and the marker has to travel with them.
            //
            // Walking only `pipeline.sources` left that hole, and the
            // row-cardinality gate was covering it by accident: an op
            // appending an unbounded callee had no provable row bound
            // either, so it failed `check_row_bounds` first and the missing
            // viral check never showed. The 2026-09-05 paging ruling gives
            // that op a bound (an `append` orders totally, by construction
            // — see `cost::keyset_paged`), which took the cover away and
            // left the audit's `take 5 … append feed` case registering.
            // Same fix `check_unguarded` already applies: its `sources_of`
            // counts `Append` for exactly this reason.
            //
            // `match any/none` is deliberately NOT counted: a semi-join
            // decides which of THIS op's rows survive and contributes none
            // of its own, so it is not composition in R-C9's sense.
            let appended = pipeline.steps.iter().filter_map(|st| match st {
                Step::Append { source, .. } => Some(source),
                _ => None,
            });
            for src in pipeline.sources.iter().chain(appended) {
                if let Some(callee) = self.ops.get(&src.name.name) {
                    if callee.flags.unbounded {
                        return Err(err(
                            "unacknowledged_unbounded",
                            op.name.span,
                            format!(
                                "`{}` composes unbounded `{}` and must carry `unbounded` itself",
                                op.name.name, src.name.name
                            ),
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

/// A sealed indexed field's blind index as a scope sees it: (entity,
/// field, normalize, digest column, alias).
type Blind = (String, String, Option<String>, String, String);

/// One name visible to expressions at the current pipeline position.
#[derive(Debug, Clone)]
struct SField {
    name: String,
    expr: BExpr,
    /// Set when the field is a ref, so `field.sub` can join through it.
    ref_target: Option<String>,
    sealed: bool,
    /// For a sealed field, the `(entity, field)` its envelope is bound to.
    /// Opening a payload needs that pair as associated data, so a value
    /// cannot be moved to another column and still open.
    seal_of: Option<(String, String)>,
    /// A sealed INDEXED field: (entity, field, normalize, digest column,
    /// alias) — EQUALITY lowers to the blind index (F1); `in` is planned
    /// but refuses today, and everything else always refuses.
    blind: Option<Blind>,
    /// An entity's implicit field (`id`, `rev`, `depth`) reached through a
    /// sub-select that stands for the entity. A step may name it, as it may
    /// on the entity itself, and `*` leaves it out, as it does on the
    /// entity itself. A callee's output is never implicit: the callee chose
    /// it.
    implicit: bool,
}

#[derive(Debug, Clone)]
enum Scope {
    /// A base entity: every declared and implicit field is addressable.
    Entity { entity: String, alias: String },
    /// A callee's output fields.
    Fields(Vec<SField>),
    /// Per-source scopes of a multi-source list, for qualified lookup
    /// (`teams.name`) and the unique-across-sources rule (DL ~1660).
    Multi(Vec<(String, Scope)>),
    /// Step scope after a projection [ruling W047]: outputs first, then the
    /// pre-projection source scope. A sort or filter key does not have to be
    /// projected to be used — `products[name] sort price` is legal, and the
    /// key is inferred from the source row. The fallback is the SOURCE scope,
    /// not the base table: a composed callee's caller can reach what the
    /// callee output and nothing deeper, so encapsulation holds. The layer
    /// dies with row identity (group / distinct / settled callees).
    Layered(Vec<Scope>),
}

/// Values bound to `$params` at the current call depth.
type ParamEnv = HashMap<String, BExpr>;

/// A bound `let`, visible to the pipeline that declared it.
#[derive(Debug, Clone)]
struct LetInfo {
    name: String,
    /// Output name, value type, ref target if the output is a ref.
    fields: Vec<(String, ScalarTy, Option<String>)>,
    /// The let's sealed outputs, as its query's `host_open` names them:
    /// `(output, entity, field)`. A source that reads the let inherits the
    /// obligation to open them (reported fix 25r).
    seals: Vec<(String, String, String)>,
    /// Scalar-referenceable (`cap.m`) only when the body ends in `reduce`.
    ends_in_reduce: bool,
    /// Outputs that hold at most one row each: a group key that is an
    /// entity's `id` (with the other keys, if any, read from that same
    /// row), or a lone ref key. Joining the let back on one of these is
    /// one-to-one, so it is not a many-branch (DL "Let outputs as
    /// sources", reported fix 25at).
    unique_keys: Vec<String>,
}

/// Who is asking. Guards are a function of the caller (GC §1): a principal
/// binds `$caller` (as a value when known, as a prepared slot otherwise);
/// `Anonymous` makes every guard false BY RULE (G15), not by null folklore.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Caller {
    #[default]
    Slot,
    Anonymous,
}

/// The rows a listener-woken run may see, resolved to concrete ids before
/// the bind, one set per entity.
///
/// `$trigger.ids` names rows of the entity the event fired on. "Reachable
/// from" those rows is a fact about *data*, not about the grammar, so it is
/// resolved by the runtime (`ordo_exec::triggers::reachable`) against the
/// database and handed here already answered. That keeps this seam a filter
/// and nothing more — no path search, no join planning, and nothing the
/// binder has to be trusted to get right.
///
/// **An entity absent from the map has no visible rows.** Not "unbounded",
/// not "left alone": absent means the semi-join is `false`, which is exactly
/// what §8 promises — "every other row is invisible exactly as a guard would
/// make it". Defaulting the other way is the confused deputy.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TriggerRows(pub std::collections::BTreeMap<String, Vec<String>>);

impl TriggerRows {
    /// One entity's visible ids. `None` and `Some(&[])` mean the same thing
    /// and both lower to `false`; they are distinguished only so a caller
    /// can tell "the runtime resolved this and found nothing" from "the
    /// runtime never looked", which is a debugging distinction and never a
    /// visibility one.
    pub fn ids(&self, entity: &str) -> Option<&[String]> {
        self.0.get(entity).map(|v| v.as_slice())
    }

    pub fn with(mut self, entity: &str, ids: Vec<String>) -> TriggerRows {
        self.0.insert(entity.to_string(), ids);
        self
    }
}

/// **R-G20 — a routine's bound follows its trigger's provenance, not its
/// principal** (`guards-conformance.md`; `definition-language.md` §8,
/// "Trigger provenance sets the bound [settled — supersedes the blanket
/// rule above]").
///
/// The rationale that a routine "holds no members row, so `$caller` is null
/// and every guard evaluates false" is **wrong and must not appear here**:
/// that describes R-G12's anonymous caller, and it would make every routine
/// useless rather than bounded. The reason the bound exists is provenance.
/// Who chose the moment the run happens?
///
/// | Woken by | Bound |
/// |---|---|
/// | **clock** (`every …`) | guards pass — nobody outside chose the moment |
/// | **listener** (`when …`) | only rows reachable from `$trigger.ids` |
///
/// A listener fires on `ingested` (a `source:` principal) and `fed` (a copy
/// from another space). Both are outsiders, and `$trigger.ids` hands the run
/// exactly the rows an outsider just wrote — so a globally guard-exempt
/// listener is a **confused deputy**: it does work an outsider steered, on
/// rows the outsider could never touch. That is a privilege escalation with
/// a scheduler attached, which is why this lives in the binder next to the
/// guard it replaces rather than in a check somewhere above it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Provenance {
    /// Not a routine. Guards apply in the ordinary way (G5/G13/G14/G15).
    #[default]
    Caller,
    /// `every …` — nobody outside chose the moment, so guards pass. This is
    /// what keeps the blanket rule's one real protection: a clock-driven
    /// rule can never silently do nothing.
    Clock,
    /// `when …` — bounded to the resolved reachable set.
    Listener(TriggerRows),
}

pub struct Binder<'a> {
    schema: &'a Schema,
    registry: &'a Registry,
    alias_n: usize,
    stack: Vec<String>,
    lets: Vec<LetInfo>,
    /// G14: `unguarded` bypasses guards for entity sources written directly
    /// in the CURRENT op body — never a composed callee's (G13). Saved and
    /// restored around callee binds.
    unguarded: bool,
    caller: Caller,
    /// R-G20. See [`Provenance`]. Saved and restored around callee binds
    /// exactly as `unguarded` is, and for the same reason (G13): the bound
    /// belongs to the body that was woken, never to a registered op it
    /// happens to compose.
    provenance: Provenance,
    /// The execution clock `since` windows anchor to — see [`Clock`].
    clock: (i32, u8, u8),
    /// Compile no guard at all, in this body or any callee. Set only by
    /// [`bind_op_body_only`], whose query is never run: it exists so the
    /// field-scope manifest can tell what the op's own pipeline reads from
    /// what an inlined guard reads (25bf).
    body_only: bool,
}

/// The execution instant a `since` window anchors to.
///
/// DL §5: *"`since` anchors to execution time — the same op returns
/// different rows tomorrow."* The engine folds it at BIND time into an exact
/// date literal, so no `NOW()` ever reaches SQL and a plan stays
/// reproducible; that part was always right. What was missing is that
/// **nothing supplied one**. All eight `Binder` construction sites hard-coded
/// the conformance corpus's pinned date, including the one the executor calls
/// per request, so `since 30d` was a window anchored to 2026-05-01 and
/// widening by a day every day. Silent, and invisible to the corpus because
/// the corpus pins the same date on purpose.
///
/// So it is a required argument rather than a defaulted field. A default here
/// is precisely the shape of the bug: the wrong answer, available without
/// anyone choosing it. A test that wants the pinned date now says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clock(pub (i32, u8, u8));

impl Clock {
    /// The date the conformance corpus pins (§2.4 stage 3). Deliberately
    /// named rather than written as a literal at each site, so a reader can
    /// tell "this test wants the fixed date" from "somebody forgot".
    pub const CORPUS: Clock = Clock((2026, 5, 1));

    pub const fn at(y: i32, m: u8, d: u8) -> Clock {
        Clock((y, m, d))
    }

    /// The real execution date, UTC.
    ///
    /// Civil arithmetic rather than a time crate: `civil_from_days` is
    /// already here for `since`'s own shifting, and the platform's one
    /// nondeterminism should not also be its one date dependency. Before the
    /// epoch is not representable and answers the epoch — a clock set to 1969
    /// is a broken deployment, and a window anchored to 1970 is a more
    /// legible symptom than a panic in the binder.
    pub fn now() -> Clock {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Clock(civil_from_days(secs.div_euclid(86_400)))
    }
}

/// Civil-date arithmetic (Howard Hinnant's algorithms) — exact, no time
/// library. Days since 1970-01-01.
fn days_from_civil(y: i32, m: u8, d: u8) -> i64 {
    let y = y as i64 - if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i32, u8, u8) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    (if m <= 2 { y + 1 } else { y } as i32, m, d)
}

fn last_day_of_month(y: i32, m: u8) -> u8 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
    }
}

/// date ± duration, exactly. Day-based units move whole days; `mo`/`y`
/// move months with the spec's end-of-month clamp (DL ~123).
fn shift_date(
    (y, m, d): (i32, u8, u8),
    n: i64,
    unit: ordo_syntax::token::DurationUnit,
    forward: bool,
) -> Option<(i32, u8, u8)> {
    use ordo_syntax::token::DurationUnit::*;
    let sign = if forward { 1 } else { -1 };
    match unit {
        D | W => {
            let days = n * if unit == W { 7 } else { 1 };
            Some(civil_from_days(days_from_civil(y, m, d) + sign * days))
        }
        Mo | Y => {
            let months = n * if unit == Y { 12 } else { 1 };
            let total = (y as i64 * 12 + (m as i64 - 1)) + sign * months;
            let ny = total.div_euclid(12) as i32;
            let nm = (total.rem_euclid(12) + 1) as u8;
            Some((ny, nm, d.min(last_day_of_month(ny, nm))))
        }
        // Sub-day units cannot shift a DATE exactly.
        _ => None,
    }
}

/// Bind a case pipeline against a schema and op registry.
pub fn bind(
    schema: &Schema,
    registry: &Registry,
    pl: &Pipeline,
    clock: Clock,
) -> Result<Query, SemaError> {
    let mut b = Binder {
        schema,
        registry,
        alias_n: 0,
        stack: Vec::new(),
        lets: Vec::new(),
        unguarded: false,
        body_only: false,
        provenance: Provenance::Caller,
        caller: Caller::default(),
        clock: clock.0,
    };
    b.pipeline(pl, &ParamEnv::new()).map(|q| pruned(q, schema))
}

/// The anchor's READ guard as a bound condition over `alias`, for `whole`
/// — the one read that is not a pipeline and so cannot get its guard the
/// ordinary way. The planner renders it; what crosses this boundary is
/// the IR, never SQL text.
///
/// `None` means the entity is genuinely unguarded (no guards, or `access
/// read open`), never "we could not work it out": a guard that fails to
/// compile is an error here, because the alternative is emitting a
/// document with no guard at all.
pub fn whole_guard_cond(
    schema: &Schema,
    entity: &str,
    alias: &str,
    caller: Option<&str>,
) -> Result<Option<BCond>, SemaError> {
    whole_guard_cond_at(schema, entity, alias, caller, 100)
}

/// [`whole_guard_cond`] with the binder's alias counter started at
/// `alias_base`.
///
/// A whole-read used to compile **one** guard, the anchor's, so one fixed
/// base was enough. It now compiles one per row it brings in (every
/// signpost and every owned child, S6-001's third instance), and two guards
/// compiled from the same base would mint the same aliases inside one
/// statement. The planner hands each a base of its own.
pub fn whole_guard_cond_at(
    schema: &Schema,
    entity: &str,
    alias: &str,
    caller: Option<&str>,
    alias_base: usize,
) -> Result<Option<BCond>, SemaError> {
    let span = Span::new(0, 0, 1, 1);
    let Some(e) = schema.entity(entity) else {
        return Err(err("unknown_source", span, format!("no entity `{entity}`")));
    };
    let Some(g) = e.guards.clone() else {
        return Ok(None);
    };
    if matches!(g.read, ordo_schema::GuardBody::Open) {
        return Ok(None);
    }
    let registry = Registry::default();
    let mut b = Binder {
        schema,
        registry: &registry,
        alias_n: alias_base, // clear of the planner's own `w<n>` aliases
        stack: Vec::new(),
        lets: Vec::new(),
        unguarded: false,
        body_only: false,
        provenance: Provenance::Caller,
        caller: match caller {
            Some(_) => Caller::Slot,
            None => Caller::Anonymous,
        },
        // `since` is a STEP, and neither a guard atom nor a write
        // right-hand side can contain one — so no window is folded
        // on this path and the value cannot reach a plan.
        clock: Clock::CORPUS.0,
    };
    let mut env = ParamEnv::new();
    if let Some(id) = caller {
        env.insert(
            "caller".into(),
            BExpr::Lit {
                value: Expr::StrLit {
                    value: id.to_string(),
                    span,
                },
                ty: ScalarTy::Id,
            },
        );
    }
    b.guard_cond_of(&g, entity, alias, &env)
}

/// One bound write right-hand side, plus the one fact the write protocol
/// has to branch on.
#[derive(Debug, Clone, PartialEq)]
pub struct WriteValue {
    /// Ready for `ordo_plan::render_write_value`.
    pub expr: BExpr,
    /// Does it read a column of the row being written?
    ///
    /// This is the whole of what distinguishes the two computed forms. A
    /// **row-free** value (`"thirsty:" + $plant`, `round($t / $n, 0)`) has
    /// an answer before the statement runs, so the write protocol can hold
    /// it like any literal — seal it, digest it, check it against a closed
    /// value set, prove a guard over it. A **row-reading** value
    /// (`used + 1`) has no answer until the row is in hand, so it can only
    /// be computed by the statement itself, and every host-side use of the
    /// value is off the table.
    pub reads_row: bool,
}

/// **Bind a write's right-hand side** (DL §6, "RHS field references";
/// §3, "Legal anywhere expressions are: projections, write RHS (per-row
/// values in bulk writes and rules), aggregate arguments, sort keys").
///
/// The scope is one entity — the row being written — under `alias`, which
/// the write path sets to the table name so the rendered SQL reads
/// `"teams"."used"` and needs no `FROM`. A one-segment path is therefore a
/// column of that row and nothing else.
///
/// Everything about *what an expression means* is the read path's, reached
/// through `Binder::expr`: the function domains (`check_arg_types`), the
/// result typing (`binary_ty`/`fn_ty`, and so the division rule that makes
/// `/` a `decimal`), `allocate`'s one legal home, the refusal to read a
/// sealed column. Only the *frame* is new. That is the point: DL says an
/// expression is one language wherever it is legal, and a second binder
/// for writes would be a second language.
///
/// Two shapes are refused here rather than allowed to lower into something
/// plausible:
///
/// - **A dot-path.** `owner.name` in a read walks a ref with a LEFT JOIN;
///   a write has no join to walk one with, and an UPDATE that quietly
///   correlated a subquery per row would be a different cost and a
///   different set of rows from the one the author wrote. DL's own rule for
///   the neighbouring construct — `cond` arms — is that "conditions see the
///   row, not other rows", and §11 declines inter-row computation outright.
///   So the refusal is by name, and it is the same class the write path
///   already gives an unresolvable `x.y`.
/// - **An aggregate.** There is no group here to aggregate over.
///
/// `declared` is the target column's type, checked with the very same
/// [`fit_slot`] a parameter binding and a guard literal go through —
/// so a `decimal` reaching a `money` slot is `implicit_rounding` with the
/// message that names the three conversions, and a text reaching an int
/// slot is `param_type_mismatch`, in both cases without this path knowing
/// anything about either rule.
pub fn write_value_expr(
    schema: &Schema,
    entity: &str,
    alias: &str,
    value: &Expr,
    params: &[(String, Expr)],
    caller: Option<&str>,
    declared: ScalarTy,
) -> Result<WriteValue, SemaError> {
    let span = value.span();
    if schema.entity(entity).is_none() {
        return Err(err("unknown_source", span, format!("no entity `{entity}`")));
    }
    let registry = Registry::default();
    let mut b = Binder {
        schema,
        registry: &registry,
        alias_n: 0,
        stack: Vec::new(),
        lets: Vec::new(),
        unguarded: false,
        body_only: false,
        provenance: Provenance::Caller,
        caller: match caller {
            Some(_) => Caller::Slot,
            None => Caller::Anonymous,
        },
        // `since` is a STEP; a write right-hand side cannot contain one, so
        // no window is folded here and no date can reach the statement.
        clock: Clock::CORPUS.0,
    };
    let mut env = ParamEnv::new();
    for (name, v) in params {
        // `ordo_conformance::write` folds a negative literal before it ever
        // reaches here (it must — its own `is_held_value` pattern-matches
        // literal shapes directly, and a `Neg` wrapper would misclassify a
        // negative literal as a computed value). Folding again here is a
        // no-op for an already-folded value and makes this entry point
        // correct on its own for any future caller that does not pre-fold.
        let v = fold_neg_lit(v);
        let ty = lit_ty(&v).ok_or_else(|| {
            err(
                "invalid_binding_expr",
                v.span(),
                format!("`${name}` is bound to something that is not a value"),
            )
        })?;
        env.insert(name.clone(), BExpr::Lit { value: v, ty });
    }
    if let Some(id) = caller {
        env.insert(
            "caller".into(),
            BExpr::Lit {
                value: Expr::StrLit {
                    value: id.to_string(),
                    span,
                },
                ty: ScalarTy::Id,
            },
        );
    }
    let scope = Scope::Entity {
        entity: entity.to_string(),
        alias: alias.to_string(),
    };
    let mut q = Query {
        not_null: None,
        from: From::Table {
            entity: entity.to_string(),
            alias: alias.to_string(),
        },
        joins: Vec::new(),
        filters: Vec::new(),
        having: Vec::new(),
        ctes: Vec::new(),
        source_joins: Vec::new(),
        group: None,
        outputs: Vec::new(),
        sort: Vec::new(),
        sort_inherited: false,
        host_open: Vec::new(),
        limit: None,
        distinct: false,
        tiebreak: Vec::new(),
    };
    let bound = b.expr(value, &scope, &mut q, &env)?;
    if !q.joins.is_empty() || !q.ctes.is_empty() || !q.source_joins.is_empty() {
        return Err(err(
            "invalid_binding_expr",
            span,
            format!(
                "a write's right-hand side sees the row it writes and no other row, so it \
                 cannot dot through a ref — read the value first and pass it as a \
                 parameter, or make the other row's value a field of `{entity}`"
            ),
        ));
    }
    if bound.has_agg() {
        return Err(err(
            "invalid_binding_expr",
            span,
            "a write's right-hand side is a per-row value, and an aggregate needs rows to \
             aggregate over — DL §11 declines inter-row computation",
        ));
    }
    let bound = fit_slot(bound, declared, span)?;
    Ok(WriteValue {
        reads_row: reads_row(&bound),
        expr: bound,
    })
}

/// Does this bound expression read a column of the row being written?
fn reads_row(e: &BExpr) -> bool {
    match e {
        BExpr::Col { .. } => true,
        BExpr::Neg { expr, .. } => reads_row(expr),
        BExpr::Binary { lhs, rhs, .. } => reads_row(lhs) || reads_row(rhs),
        BExpr::Fn { args, .. } => args.iter().any(reads_row),
        BExpr::Cond { arms, else_, .. } => {
            arms.iter().any(|(c, v)| cond_reads_row(c) || reads_row(v)) || reads_row(else_)
        }
        _ => false,
    }
}

fn cond_reads_row(c: &BCond) -> bool {
    match c {
        BCond::Cmp { lhs, rhs, .. } | BCond::Text { lhs, rhs, .. } => {
            reads_row(lhs) || reads_row(rhs)
        }
        BCond::In { expr, list } => reads_row(expr) || list.iter().any(reads_row),
        BCond::IsNull { expr, .. } | BCond::Bare(expr) => reads_row(expr),
        BCond::And(items) | BCond::Or(items) => items.iter().any(cond_reads_row),
        BCond::Not(inner) => cond_reads_row(inner),
        BCond::TreeWalk { expr, anchor, .. } => reads_row(expr) || reads_row(anchor),
        // A semi-join is a read of other rows; the bind above has already
        // refused the shapes that could produce one here.
        BCond::Exists { .. } => true,
    }
}

/// A one-entity query answering "which rows may this caller read/write":
/// `SELECT id FROM entity WHERE <guard>`. `None` when the verb is open or
/// the entity is unguarded. The write protocol's preconditions build on it:
/// pinning `id` into its filters asks about one row; running it bare lists
/// the view.
pub fn guard_filter_query(
    schema: &Schema,
    entity: &str,
    write_verb: bool,
    caller: Option<&str>,
) -> Result<Option<Query>, SemaError> {
    let registry = Registry::default();
    let mut b = Binder {
        schema,
        registry: &registry,
        alias_n: 0,
        stack: Vec::new(),
        lets: Vec::new(),
        unguarded: false,
        body_only: false,
        provenance: Provenance::Caller,
        caller: match caller {
            Some(_) => Caller::Slot,
            None => Caller::Anonymous,
        },
        // `since` is a STEP, and neither a guard atom nor a write
        // right-hand side can contain one — so no window is folded
        // on this path and the value cannot reach a plan.
        clock: Clock::CORPUS.0,
    };
    let mut env = ParamEnv::new();
    if let Some(id) = caller {
        env.insert(
            "caller".into(),
            BExpr::Lit {
                value: Expr::StrLit {
                    value: id.to_string(),
                    span: Span::new(0, 0, 1, 1),
                },
                ty: ScalarTy::Id,
            },
        );
    }
    let Some(e) = schema.entity(entity) else {
        return Err(err(
            "unknown_source",
            Span::new(0, 0, 1, 1),
            format!("no entity `{entity}`"),
        ));
    };
    let Some(g) = e.guards.clone() else {
        return Ok(None);
    };
    let body = if write_verb { &g.write } else { &g.read };
    if matches!(body, ordo_schema::GuardBody::Open) {
        return Ok(None);
    }
    let use_g = if write_verb {
        // Reuse the read-guard compiler with the write walk in its seat.
        ordo_schema::Guards {
            read: g.write.clone(),
            write: g.write.clone(),
            source: g.source.clone(),
            rebase: g.rebase.clone(),
        }
    } else {
        g
    };
    let alias = b.alias();
    let Some(cond) = b.guard_cond_of(&use_g, entity, &alias, &env)? else {
        return Ok(None);
    };
    Ok(Some(Query {
        not_null: Some(not_null_columns(schema)),
        from: From::Table {
            entity: entity.to_string(),
            alias: alias.clone(),
        },
        joins: Vec::new(),
        filters: vec![cond],
        having: Vec::new(),
        ctes: Vec::new(),
        source_joins: Vec::new(),
        group: None,
        outputs: vec![OutField {
            name: "id".into(),
            expr: BExpr::Col {
                alias,
                column: "id".into(),
                ty: ScalarTy::Id,
            },
        }],
        sort: Vec::new(),
        sort_inherited: false,
        host_open: Vec::new(),
        limit: None,
        distinct: false,
        tiebreak: Vec::new(),
    }))
}

/// One condition of a `release member` rule, or one reassignment lookup,
/// as a query (27aq): `SELECT <output> FROM entity WHERE <inner>`, with
/// `$member` bound to `member`.
///
/// The inner walk is a guard walk and is compiled by the guard compiler, so
/// a release condition reaches exactly the rows a guard with the same text
/// would. The subject is the seat being released, supplied here as a
/// literal; nothing a caller sends reaches it.
///
/// `output` is `id` for a condition (the caller asks whether any row
/// matched) and the lookup's member field for a reassignment.
pub fn release_query(
    schema: &Schema,
    entity: &str,
    inner: &ordo_syntax::GuardExpr,
    member: &str,
    output: &str,
) -> Result<Query, SemaError> {
    let registry = Registry::default();
    let mut b = Binder {
        schema,
        registry: &registry,
        alias_n: 0,
        stack: Vec::new(),
        lets: Vec::new(),
        unguarded: false,
        body_only: false,
        provenance: Provenance::Caller,
        caller: Caller::Slot,
        // No step in a guard walk can fold a window.
        clock: Clock::CORPUS.0,
    };
    let mut env = ParamEnv::new();
    env.insert(
        "caller".into(),
        BExpr::Lit {
            value: Expr::StrLit {
                value: member.to_string(),
                span: inner.span,
            },
            ty: ScalarTy::Id,
        },
    );
    let Some(e) = schema.entity(entity) else {
        return Err(err(
            "release_unknown_entity",
            inner.span,
            format!("no entity `{entity}`"),
        ));
    };
    let Some(f) = e.field(output) else {
        return Err(err(
            "invalid_release_effect",
            inner.span,
            format!("`{entity}` has no field `{output}`"),
        ));
    };
    let ty = f.ty.value_ty();
    let column = f.column();
    let alias = b.alias();
    let cond = b.guard_expr_cond(entity, &alias, inner, &env)?;
    Ok(Query {
        not_null: Some(not_null_columns(schema)),
        from: From::Table {
            entity: entity.to_string(),
            alias: alias.clone(),
        },
        joins: Vec::new(),
        filters: vec![cond],
        having: Vec::new(),
        ctes: Vec::new(),
        source_joins: Vec::new(),
        group: None,
        outputs: vec![OutField {
            name: output.into(),
            expr: BExpr::Col { alias, column, ty },
        }],
        sort: Vec::new(),
        sort_inherited: false,
        host_open: Vec::new(),
        limit: None,
        distinct: false,
        tiebreak: Vec::new(),
    })
}

/// Bind every condition and lookup of the schema's release rule once, so a
/// walk the guard compiler refuses (a literal of the wrong type, say) is a
/// compile error rather than a refusal on the day somebody tries to leave.
pub fn check_release(schema: &Schema) -> Result<(), SemaError> {
    let Some(r) = &schema.release else {
        return Ok(());
    };
    const PROBE: &str = "mem_probe";
    if let ordo_syntax::ReleaseMode::When(cond) = &r.mode {
        for atom in cond.disjuncts.iter().flatten() {
            release_query(schema, &atom.entity.name, &atom.inner, PROBE, "id")?;
        }
    }
    for effect in &r.effects {
        if let ordo_syntax::ReleaseEffect::Reassign { to, .. } = effect {
            for t in to {
                if let ordo_syntax::ReassignTarget::Lookup {
                    entity,
                    inner,
                    field,
                    ..
                } = t
                {
                    release_query(schema, &entity.name, inner, PROBE, &field.name)?;
                }
            }
        }
    }
    Ok(())
}

/// G10a's static create-exemption test: a write guard is create-exempt iff
/// every `$caller`-referencing atom is a REVERSE collection step rooted at
/// the entity being created — each path to the caller runs through rows
/// that must reference the new row, which cannot exist yet. Such a guard
/// is vacuously unsatisfiable at create time, so create skips it; every
/// other shape (forward hops to a parent, inherited/rebased guards, direct
/// `$caller` compares) must hold against the would-be row.
pub fn create_exempt(schema: &Schema, entity: &str) -> bool {
    let Some(e) = schema.entity(entity) else {
        return false;
    };
    let Some(g) = &e.guards else { return true };
    let ordo_schema::GuardBody::Walk(w) = &g.write else {
        return true;
    };
    if matches!(g.source, ordo_schema::GuardSource::Inherited { .. }) {
        // Rooted at an ancestor that already exists.
        return false;
    }
    // Exempt only when EVERY disjunct is provably false before the row
    // exists — that is, every disjunct contains at least one reverse
    // collection step rooted at the entity being created, since no row can
    // reference a row that is not there yet.
    //
    // Flattening the disjuncts here would be a hole, and was one: a guard
    // like `widgetShares(member == $caller) or open` has an atom-by-atom
    // shape that looks exempt, but its second disjunct is a bare bool that
    // is perfectly satisfiable — or perfectly UNsatisfiable — at create
    // time, and skipping the guard writes the row either way. A disjunct
    // with no such step is an independent authorization path and must be
    // proven, not assumed away.
    !w.disjuncts.is_empty()
        && w.disjuncts.iter().all(|conj| {
            conj.iter().any(|a| match a {
                ordo_syntax::GuardAtom::Step { path, .. } => {
                    // Reverse-rooted: the first segment names an entity, not
                    // a ref field on this one (a forward hop reaches a parent
                    // that already exists).
                    e.field(&path[0].name).is_none()
                }
                _ => false,
            })
        })
}

/// A field value supplied to a create, as the write path knows it before
/// the row exists. Guard atoms only ever compare ids, texts, and bools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateVal {
    Id(String),
    Text(String),
    Bool(bool),
    Int(i64),
    /// A `decimal(p)` value, as its digits (27ak).
    Decimal(String),
    Null,
}

/// What a create must prove before inserting (G10 on the would-be row).
pub enum CreateGate {
    /// G10a: every path to `$caller` runs through rows that must reference
    /// the new row — vacuously unsatisfiable pre-insert, so create skips
    /// the guard. The acl bootstrap (creator's top-level access row, same
    /// command group) is the writer's side of this bargain.
    Exempt,
    /// Run this query; a row back means the guard holds. It is anchored at
    /// the caller's own `members` row, so a vanished caller also fails.
    Check(Box<Query>),
}

/// Compile the write-guard precondition for creating `entity` with the
/// given field values. Atoms over supplied scalar fields fold host-side;
/// atoms that walk through a supplied ref pin the parent by literal id and
/// continue in SQL; reverse steps rooted at the not-yet-existing row are
/// FALSE in their disjunct (and when ALL caller paths are such, the guard
/// is create-exempt before any of this runs). Inherited guards re-anchor
/// at the owning parent named by the rebase chain.
pub fn create_write_query(
    schema: &Schema,
    entity: &str,
    values: &std::collections::BTreeMap<String, CreateVal>,
    caller: &str,
) -> Result<CreateGate, SemaError> {
    let span = Span::new(0, 0, 1, 1);
    let Some(e) = schema.entity(entity) else {
        return Err(err("unknown_source", span, format!("no entity `{entity}`")));
    };
    let Some(g) = e.guards.clone() else {
        return Ok(CreateGate::Exempt);
    };
    if matches!(g.write, ordo_schema::GuardBody::Open) {
        return Ok(CreateGate::Exempt);
    }
    if create_exempt(schema, entity) {
        return Ok(CreateGate::Exempt);
    }
    let registry = Registry::default();
    let mut b = Binder {
        schema,
        registry: &registry,
        alias_n: 0,
        stack: Vec::new(),
        lets: Vec::new(),
        unguarded: false,
        body_only: false,
        provenance: Provenance::Caller,
        caller: Caller::Slot,
        // `since` is a STEP, and neither a guard atom nor a write
        // right-hand side can contain one — so no window is folded
        // on this path and the value cannot reach a plan.
        clock: Clock::CORPUS.0,
    };
    let mut env = ParamEnv::new();
    env.insert(
        "caller".into(),
        BExpr::Lit {
            value: Expr::StrLit {
                value: caller.to_string(),
                span,
            },
            ty: ScalarTy::Id,
        },
    );
    let ordo_schema::GuardBody::Walk(walk) = &g.write else {
        unreachable!("open handled above")
    };

    let cond = if let ordo_schema::GuardSource::Inherited { from } = &g.source {
        // ── the rebase chain, anchored at what the caller supplied ──────
        //
        // The guard belongs to an ancestor and `rebase` is the ownership
        // path down to this entity: `rebase[0]` is a ref on the row being
        // created, `rebase[i]` a ref on the row `rebase[i-1]` points at,
        // and the last one lands on `from`, where the walk's text starts.
        //
        // A read rebases by chasing those refs off the row it already has
        // (`guard_cond_of`). A create has no row: `rebase[0]` cannot be
        // READ, it can only be the value the caller SUPPLIED, so hop zero
        // is a literal pin and every hop above it is an ordinary ref
        // chase off the ancestor the previous hop pinned. That is the
        // whole difference, and it is why one hop was ever special.
        //
        //   EXISTS (SELECT 1 FROM <e1> a1 WHERE a1.id = '<supplied>'
        //     AND EXISTS (SELECT 1 FROM <e2> a2 WHERE a2.id = a1.<hop1>
        //       AND … AND <walk at the origin>))
        //
        // Nothing here is length-limited any more; a two-hop grandchild
        // under an acl root is the shape that used to be uncreatable.
        let Some(first) = g.rebase.first() else {
            return Err(err(
                "invalid_guard_walk",
                span,
                format!("`{entity}` inherits from `{from}` through no ownership path"),
            ));
        };
        // A ref's value is an id because the SCHEMA says so, whatever its
        // spelling: the write path's literal classifier calls a minted id
        // longer than 40 characters `Text`, and matching `Id` alone made
        // every such create refuse its own owner (shopfront example,
        // 2026-10-10). G12 in the write path reads the schema the same way.
        let (Some(CreateVal::Id(parent)) | Some(CreateVal::Text(parent))) = values.get(first)
        else {
            // No parent supplied → the guard cannot hold. (Only the FIRST
            // hop can be missing this way: the rest are read off rows.)
            return Ok(CreateGate::Check(Box::new(members_query(
                caller,
                const_cond(false),
                &mut b,
            ))));
        };
        // Resolve the chain of ancestor entities and give each an alias,
        // outermost first. Allocating the aliases before the walk binds
        // keeps single-hop numbering exactly as it was.
        let mut chain: Vec<(String, String)> = Vec::new();
        let mut cur = entity.to_string();
        for hop in &g.rebase {
            let ce = schema
                .entity(&cur)
                .ok_or_else(|| err("unknown_source", span, format!("no entity `{cur}`")))?;
            let Some(f) = ce.field(hop) else {
                return Err(err(
                    "invalid_guard_walk",
                    span,
                    format!("rebase hop `{hop}` missing on `{cur}`"),
                ));
            };
            let FieldTy::Ref { target, .. } = &f.ty else {
                return Err(err("invalid_guard_walk", span, "rebase hop is not a ref"));
            };
            cur = target.clone();
            chain.push((cur.clone(), b.alias()));
        }
        debug_assert_eq!(cur, *from, "the rebase chain ends at the walk's origin");

        // Innermost: the ancestor's own walk, evaluated on the last row.
        let (_, origin_alias) = chain.last().expect("rebase is non-empty");
        let mut cond = b.guard_expr_cond(from, origin_alias, walk, &env)?;
        // Wrap outward. Hop i links a_{i+1}.id = a_i.rebase[i].
        for i in (1..chain.len()).rev() {
            let (ent, alias) = chain[i].clone();
            let link = BCond::Cmp {
                op: ordo_syntax::CmpOp::Eq,
                lhs: BExpr::Col {
                    alias: alias.clone(),
                    column: "id".into(),
                    ty: ScalarTy::Id,
                },
                rhs: BExpr::Col {
                    alias: chain[i - 1].1.clone(),
                    column: g.rebase[i].clone(),
                    ty: ScalarTy::Id,
                },
            };
            cond = BCond::Exists {
                query: Box::new(bare_query(&ent, &alias, vec![link, cond])),
                negated: false,
            };
        }
        // Outermost: hop zero, pinned by the id the caller supplied. A
        // parent that does not exist, or lives in another space, makes
        // this EXISTS false — the guard fails rather than opening.
        let (ent0, alias0) = chain[0].clone();
        let pin = BCond::Cmp {
            op: ordo_syntax::CmpOp::Eq,
            lhs: BExpr::Col {
                alias: alias0.clone(),
                column: "id".into(),
                ty: ScalarTy::Id,
            },
            rhs: lit_id(parent, span),
        };
        BCond::Exists {
            query: Box::new(bare_query(&ent0, &alias0, vec![pin, cond])),
            negated: false,
        }
    } else {
        // Own walk over the would-be row.
        let mut ors = Vec::new();
        for conj in &walk.disjuncts {
            let mut ands = Vec::new();
            for atom in conj {
                ands.push(create_atom_cond(&mut b, e, values, atom, &env, caller)?);
            }
            ors.push(if ands.len() == 1 {
                ands.pop().expect("one")
            } else {
                BCond::And(ands)
            });
        }
        if ors.len() == 1 {
            ors.pop().expect("one")
        } else {
            BCond::Or(ors)
        }
    };
    Ok(CreateGate::Check(Box::new(members_query(
        caller, cond, &mut b,
    ))))
}

fn lit_id(v: &str, span: Span) -> BExpr {
    BExpr::Lit {
        value: Expr::StrLit {
            value: v.to_string(),
            span,
        },
        ty: ScalarTy::Id,
    }
}

fn const_cond(v: bool) -> BCond {
    BCond::Bare(BExpr::Lit {
        value: Expr::BoolLit {
            value: v,
            span: Span::new(0, 0, 1, 1),
        },
        ty: ScalarTy::Bool,
    })
}

fn bare_query(entity: &str, alias: &str, filters: Vec<BCond>) -> Query {
    Query {
        not_null: None,
        from: From::Table {
            entity: entity.to_string(),
            alias: alias.to_string(),
        },
        joins: Vec::new(),
        filters,
        having: Vec::new(),
        ctes: Vec::new(),
        source_joins: Vec::new(),
        group: None,
        outputs: Vec::new(),
        sort: Vec::new(),
        sort_inherited: false,
        host_open: Vec::new(),
        limit: None,
        distinct: false,
        tiebreak: Vec::new(),
    }
}

/// The create gate's outer query: anchored at the caller's members row.
fn members_query(caller: &str, cond: BCond, b: &mut Binder) -> Query {
    let span = Span::new(0, 0, 1, 1);
    let m = b.alias();
    let pin = BCond::Cmp {
        op: ordo_syntax::CmpOp::Eq,
        lhs: BExpr::Col {
            alias: m.clone(),
            column: "id".into(),
            ty: ScalarTy::Id,
        },
        rhs: lit_id(caller, span),
    };
    let mut q = bare_query("members", &m, vec![pin, cond]);
    q.not_null = Some(not_null_columns(b.schema));
    q.outputs = vec![OutField {
        name: "id".into(),
        expr: BExpr::Col {
            alias: m,
            column: "id".into(),
            ty: ScalarTy::Id,
        },
    }];
    q
}

/// One guard atom against the would-be row: host folds where the value is
/// in hand, literal-pinned EXISTS where the atom walks through a ref.
fn create_atom_cond(
    b: &mut Binder,
    e: &ordo_schema::Entity,
    values: &std::collections::BTreeMap<String, CreateVal>,
    atom: &ordo_syntax::GuardAtom,
    env: &ParamEnv,
    caller: &str,
) -> Result<BCond, SemaError> {
    use ordo_syntax::GuardAtom as GA;
    // Peel: the atom's first segment names a ref field whose supplied value
    // pins the parent row; the remainder evaluates there in SQL.
    let peel =
        |b: &mut Binder, path: &[ordo_syntax::Ident], rest: GA| -> Result<BCond, SemaError> {
            let seg = &path[0].name;
            let Some(FieldTy::Ref { target, .. }) = e.field(seg).map(|f| f.ty.clone()) else {
                return Ok(const_cond(false));
            };
            // `seg` is a ref (checked above), so the value is an id
            // however the write path classified its spelling.
            let (Some(CreateVal::Id(parent)) | Some(CreateVal::Text(parent))) = values.get(seg)
            else {
                return Ok(const_cond(false));
            };
            let pa = b.alias();
            let inner = b.guard_atom_cond(&target, &pa, &rest, env)?;
            let pin = BCond::Cmp {
                op: ordo_syntax::CmpOp::Eq,
                lhs: BExpr::Col {
                    alias: pa.clone(),
                    column: "id".into(),
                    ty: ScalarTy::Id,
                },
                rhs: lit_id(parent, atom_span(&rest)),
            };
            Ok(BCond::Exists {
                query: Box::new(bare_query(&target, &pa, vec![pin, inner])),
                negated: false,
            })
        };
    match atom {
        GA::Cmp {
            path,
            caller: is_caller,
            lit,
            span,
        } => {
            if path.len() > 1 && e.field(&path[0].name).is_some() {
                return peel(
                    b,
                    path,
                    GA::Cmp {
                        path: path[1..].to_vec(),
                        caller: *is_caller,
                        lit: lit.clone(),
                        span: *span,
                    },
                );
            }
            let got = values.get(&path[0].name);
            let holds = if *is_caller {
                matches!(got, Some(CreateVal::Id(v) | CreateVal::Text(v)) if v == caller)
            } else {
                match (got, lit) {
                    (Some(CreateVal::Text(v)), Some(Expr::StrLit { value, .. })) => v == value,
                    (Some(CreateVal::Id(v)), Some(Expr::StrLit { value, .. })) => v == value,
                    (Some(CreateVal::Bool(v)), Some(Expr::BoolLit { value, .. })) => v == value,
                    // A money value supplied to a create is already whole
                    // minor units; the guard's literal is in major units
                    // and converts at the field's places (27ak).
                    (
                        Some(CreateVal::Int(v)),
                        Some(l @ (Expr::IntLit { .. } | Expr::DecimalLit { .. })),
                    ) => {
                        let ty = e
                            .field(&path[0].name)
                            .map(|f| f.ty.value_ty())
                            .unwrap_or(ScalarTy::Int);
                        match ty {
                            ScalarTy::Money(p) => {
                                ordo_syntax::numeric::literal_minor_units(l, p) == Ok(*v)
                            }
                            _ => {
                                matches!(l, Expr::IntLit { value, .. } if i128::from(*v) == *value)
                            }
                        }
                    }
                    (
                        Some(CreateVal::Decimal(v)),
                        Some(l @ (Expr::IntLit { .. } | Expr::DecimalLit { .. })),
                    ) => {
                        let lt = match l {
                            Expr::IntLit { value, .. } => value.to_string(),
                            Expr::DecimalLit { digits, .. } => digits.clone(),
                            _ => unreachable!(),
                        };
                        ordo_syntax::numeric::at_places(v, ordo_syntax::DIVISION_SCALE)
                            == ordo_syntax::numeric::at_places(&lt, ordo_syntax::DIVISION_SCALE)
                    }
                    _ => false,
                }
            };
            Ok(const_cond(holds))
        }
        GA::Bare { path, span } => {
            if path.len() > 1 && e.field(&path[0].name).is_some() {
                return peel(
                    b,
                    path,
                    GA::Bare {
                        path: path[1..].to_vec(),
                        span: *span,
                    },
                );
            }
            Ok(const_cond(matches!(
                values.get(&path[0].name),
                Some(CreateVal::Bool(true))
            )))
        }
        GA::Step { path, inner, span } => {
            if e.field(&path[0].name).is_some() {
                return peel(
                    b,
                    path,
                    GA::Step {
                        path: path[1..].to_vec(),
                        inner: inner.clone(),
                        span: *span,
                    },
                );
            }
            // Reverse step rooted at the row being created: no referencing
            // row can exist yet — FALSE in this disjunct.
            Ok(const_cond(false))
        }
    }
}

fn atom_span(a: &ordo_syntax::GuardAtom) -> Span {
    match a {
        ordo_syntax::GuardAtom::Cmp { span, .. }
        | ordo_syntax::GuardAtom::Bare { span, .. }
        | ordo_syntax::GuardAtom::Step { span, .. } => *span,
    }
}

/// R-G20's listener bound, lowered where a guard would have gone.
///
/// `id in (…)` over the resolved reachable set, or literal `false` when the
/// set is empty or the entity was never resolved. Two things are worth being
/// explicit about:
///
/// - **`false` is the safe default and the specified one.** DL §8: "every
///   other row is invisible exactly as a guard would make it". An entity a
///   listener-woken body names but the runtime could not reach from
///   `$trigger.ids` yields nothing — in the same way, and at the same place,
///   an unsatisfiable guard does.
/// - **This is a filter, not a join.** The reachability walk happened
///   already, in the runtime, against the database. What crosses into the
///   binder is a list of ids, so nothing here can widen the set, and a bug in
///   the walk shows up as too few rows rather than as a silent bypass.
fn trigger_bound(entity: &str, alias: &str, rows: &TriggerRows) -> BCond {
    let span = Span::new(0, 0, 1, 1);
    let ids = rows.ids(entity).unwrap_or(&[]);
    if ids.is_empty() {
        return BCond::Bare(BExpr::Lit {
            value: Expr::BoolLit { value: false, span },
            ty: ScalarTy::Bool,
        });
    }
    BCond::In {
        expr: BExpr::Col {
            alias: alias.to_string(),
            column: "id".into(),
            ty: ScalarTy::Id,
        },
        list: ids
            .iter()
            .map(|id| BExpr::Lit {
                value: Expr::StrLit {
                    value: id.clone(),
                    span,
                },
                ty: ScalarTy::Id,
            })
            .collect(),
    }
}

/// Bind a routine body under its trigger's provenance (R-G20).
///
/// Deliberately a separate entry point rather than an extra argument on
/// [`bind_as`]. A routine **is not a caller** (`definition-language.md` §8,
/// "Routines are not callers"), and the two paths answer different questions:
/// `bind_as` asks *which rows may this outsider see through this op*, this
/// one asks *which rows may this wake-up touch*. One signature for both would
/// have needed a default, and the only defaults available are "guards pass"
/// — the confused deputy — and "every guard false", which is R-G12's
/// anonymous caller wearing a routine's name and makes every routine useless.
///
/// `flags` is the `run` header's, and **only `unguarded` is read**: it lifts
/// the listener bound and nothing else does. `unbounded` is about work and
/// `reproducible` about determinism; neither has any bearing on which rows
/// are reachable, which is why GC096's probe can carry `unbounded` alone and
/// stay bounded to `$trigger.ids`.
pub fn bind_routine(
    schema: &Schema,
    registry: &Registry,
    pl: &Pipeline,
    params: &[(String, Expr)],
    provenance: Provenance,
    flags: OpFlags,
    clock: Clock,
) -> Result<Query, SemaError> {
    let mut env = ParamEnv::new();
    for (name, value) in params {
        // See [`fold_neg_lit`]: `-2500` parses as `Neg { IntLit(2500) }`,
        // and a literal parameter value must fold that before `lit_ty`
        // can answer for it.
        let value = fold_neg_lit(value);
        let ty = lit_ty(&value).ok_or_else(|| {
            err(
                "invalid_binding_expr",
                value.span(),
                "a routine parameter is a literal",
            )
        })?;
        env.insert(name.clone(), BExpr::Lit { value, ty });
    }
    let mut b = Binder {
        schema,
        registry,
        alias_n: 0,
        stack: Vec::new(),
        lets: Vec::new(),
        unguarded: flags.unguarded,
        body_only: false,
        provenance,
        // A routine binds no `$caller` — it has no members row. That is a
        // FACT about it and emphatically not the reason it is bounded; see
        // [`Provenance`] for why citing it as the reason is an error the
        // conformance corpus refuses to accept in a case justification.
        // `Slot` rather than `Anonymous` because `Anonymous` means "every
        // guard false", and no arm of `Provenance` wants that.
        caller: Caller::Slot,
        clock: clock.0,
    };
    b.pipeline(pl, &env).map(|q| pruned(q, schema))
}

/// Bind with an explicit caller: `Some(id)` binds `$caller` to that
/// member id (the harness's `as:`), `None` is anonymous — every guard
/// false (G15). The plain entry points leave `$caller` a prepared slot.
pub fn bind_as(
    schema: &Schema,
    registry: &Registry,
    pl: &Pipeline,
    params: &[(String, Expr)],
    caller: Option<&str>,
    clock: Clock,
) -> Result<Query, SemaError> {
    let mut env = ParamEnv::new();
    for (name, value) in params {
        // See [`fold_neg_lit`].
        let value = fold_neg_lit(value);
        let ty = lit_ty(&value).ok_or_else(|| {
            err(
                "invalid_binding_expr",
                value.span(),
                "a harness-supplied parameter is a literal",
            )
        })?;
        env.insert(name.clone(), BExpr::Lit { value, ty });
    }
    let mut b = Binder {
        schema,
        registry,
        alias_n: 0,
        stack: Vec::new(),
        lets: Vec::new(),
        unguarded: false,
        body_only: false,
        provenance: Provenance::Caller,
        caller: match caller {
            Some(_) => Caller::Slot,
            None => Caller::Anonymous,
        },
        clock: clock.0,
    };
    if let Some(id) = caller {
        env.insert(
            "caller".into(),
            BExpr::Lit {
                value: Expr::StrLit {
                    value: id.to_string(),
                    span: Span::new(0, 0, 1, 1),
                },
                ty: ScalarTy::Id,
            },
        );
    }
    b.pipeline(pl, &env).map(|q| pruned(q, schema))
}

/// Bind a case pipeline with harness-supplied top-level parameters —
/// the corpus's `params: $p = prod_3` lines. Values are literal expressions;
/// anything else has no meaning at the top level of a case.
pub fn bind_with_params(
    schema: &Schema,
    registry: &Registry,
    pl: &Pipeline,
    params: &[(String, Expr)],
    clock: Clock,
) -> Result<Query, SemaError> {
    let mut env = ParamEnv::new();
    for (name, value) in params {
        // See [`fold_neg_lit`].
        let value = fold_neg_lit(value);
        let ty = lit_ty(&value).ok_or_else(|| {
            err(
                "invalid_binding_expr",
                value.span(),
                "a harness-supplied parameter is a literal",
            )
        })?;
        env.insert(name.clone(), BExpr::Lit { value, ty });
    }
    let mut b = Binder {
        schema,
        registry,
        alias_n: 0,
        stack: Vec::new(),
        lets: Vec::new(),
        unguarded: false,
        body_only: false,
        provenance: Provenance::Caller,
        caller: Caller::default(),
        clock: clock.0,
    };
    b.pipeline(pl, &env).map(|q| pruned(q, schema))
}

/// Bind a registered op by name for INSPECTION: every parameter becomes a
/// placeholder (`$1`…), defaults ignored — `ordo check` and `ordo plan`
/// need the shape and cost of an op nobody has called yet.
///
/// # `$caller` is a slot here, exactly like `$space`
///
/// A guard-injected `$caller` already reached the artifact as a reserved
/// binding slot ([`Binder::caller_expr`] falls back to `BExpr::Param`), but a
/// `$caller` written in the op's **own body** — `tasks(assignee == $caller)`,
/// the shipped fixture's `myAssignedTasks` — was looked up in the parameter
/// environment, which this function fills from `op.params` only. So it was
/// not there, and every caller-scoped read in the language recorded
/// `unbound_required_param` in its artifact: no outputs, no cost, no SQL,
/// and — since 9l — an **empty field-scope manifest**. A consent page
/// therefore described the one class of op that exists to narrow rows to the
/// caller as touching nothing at all.
///
/// `ordo-cli`'s `apply` named the defect and the fix (*"`$caller` should be a
/// bind slot in the artifact the way the space already is"*) and deferred it,
/// because it moves every op's version hash and so kills every outstanding
/// capability grant. Grant pinning is the change that moves those hashes
/// anyway, and it is the change that cannot proceed without this: a pinned
/// **row-scope shape** must record whether the body restricts rows to
/// `$caller`, and a body that does not bind has no such record.
///
/// The slot is inserted **before** `op.params`, so a parameter literally
/// named `caller` still shadows it rather than being silently overwritten by
/// the session binding.
pub fn bind_op_generic(
    schema: &Schema,
    registry: &Registry,
    name: &str,
    clock: Clock,
) -> Result<Query, SemaError> {
    let op = registry
        .get(name)
        .ok_or_else(|| {
            err(
                "unknown_source",
                Span::new(0, 0, 1, 1),
                format!("no op `{name}`"),
            )
        })?
        .clone();
    let mut b = Binder {
        schema,
        registry,
        alias_n: 0,
        stack: vec![name.to_string()],
        lets: Vec::new(),
        unguarded: false,
        body_only: false,
        provenance: Provenance::Caller,
        caller: Caller::default(),
        clock: clock.0,
    };
    let mut env = ParamEnv::new();
    // The session's binding, as a slot. `Caller::Slot` is this binder's
    // default, so a guard already compiles to exactly this expression; an op
    // body naming `$caller` now resolves to the same one, and the two agree
    // instead of one of them refusing.
    env.insert(
        "caller".into(),
        BExpr::Param {
            name: "caller".into(),
            ty: ScalarTy::Id,
        },
    );
    for p in &op.params {
        env.insert(
            p.name.name.clone(),
            BExpr::Param {
                name: p.name.name.clone(),
                ty: p.ty,
            },
        );
    }
    b.unguarded = op.flags.unguarded;
    let pl = op.read_pipeline().ok_or_else(|| {
        err(
            "unsupported",
            op.span,
            format!(
                "`{}` is a write op — it is called, never composed",
                op.name.name
            ),
        )
    })?;
    b.pipeline(pl, &env).map(|q| pruned(q, schema))
}

/// [`bind_op_generic`] with every guard left out — the op's own pipeline
/// and nothing a guard adds, through every callee.
///
/// **Never execute this query.** It reads rows no guard bounds. It exists
/// for one consumer, the field-scope manifest, which must say which entities
/// the op's own pipeline reaches (a projection, condition, sort or group
/// through a ref hop included) and which only a guard examines. The guarded
/// bind cannot answer that: by then the guard is inlined and its reads are
/// indistinguishable from the body's (25bf).
pub fn bind_op_body_only(
    schema: &Schema,
    registry: &Registry,
    name: &str,
    clock: Clock,
) -> Result<Query, SemaError> {
    let op = registry
        .get(name)
        .ok_or_else(|| {
            err(
                "unknown_source",
                Span::new(0, 0, 1, 1),
                format!("no op `{name}`"),
            )
        })?
        .clone();
    let mut b = Binder {
        schema,
        registry,
        alias_n: 0,
        stack: vec![name.to_string()],
        lets: Vec::new(),
        unguarded: true,
        body_only: true,
        provenance: Provenance::Caller,
        caller: Caller::default(),
        clock: clock.0,
    };
    let mut env = ParamEnv::new();
    env.insert(
        "caller".into(),
        BExpr::Param {
            name: "caller".into(),
            ty: ScalarTy::Id,
        },
    );
    for p in &op.params {
        env.insert(
            p.name.name.clone(),
            BExpr::Param {
                name: p.name.name.clone(),
                ty: p.ty,
            },
        );
    }
    let pl = op.read_pipeline().ok_or_else(|| {
        err(
            "unsupported",
            op.span,
            format!(
                "`{}` is a write op — it is called, never composed",
                op.name.name
            ),
        )
    })?;
    b.pipeline(pl, &env).map(|q| pruned(q, schema))
}

/// Bind a registered op by name, with no caller-supplied arguments.
pub fn bind_op(
    schema: &Schema,
    registry: &Registry,
    name: &str,
    clock: Clock,
) -> Result<Query, SemaError> {
    let op = registry.get(name).ok_or_else(|| {
        err(
            "unknown_source",
            Span::new(0, 0, 1, 1),
            format!("no op `{name}`"),
        )
    })?;
    let mut b = Binder {
        schema,
        registry,
        alias_n: 0,
        stack: vec![name.to_string()],
        lets: Vec::new(),
        unguarded: false,
        body_only: false,
        provenance: Provenance::Caller,
        caller: Caller::default(),
        clock: clock.0,
    };
    let env = b.param_env(op, &[], op.span)?;
    b.unguarded = op.flags.unguarded;
    let pl = op.read_pipeline().ok_or_else(|| {
        err(
            "unsupported",
            op.span,
            format!(
                "`{}` is a write op — it is called, never composed",
                op.name.name
            ),
        )
    })?;
    b.pipeline(pl, &env).map(|q| pruned(q, schema))
}

/// The finished query of a top-level bind, with every sub-select trimmed
/// to the columns read from outside it (reported fix 25v, see
/// [`prune`]). Only a top-level bind prunes: a callee or a `let` bound on
/// the way is still open to the steps that follow it.
fn pruned(mut q: Query, schema: &Schema) -> Query {
    prune::prune_subs(&mut q);
    with_not_null(q, schema)
}

/// Every table's `NOT NULL` columns, from [`ordo_schema::ddl::physical_columns`]:
/// the same list `CREATE TABLE` is printed from, so the planner's evidence
/// and the database's constraint cannot be two answers. The boot check
/// (`ordo_host::shape`) asks the live database for the same set and refuses
/// to serve when one of them allows nulls.
///
/// When external entities land (`docs/plan-external-entities.md`), their
/// columns come from the database's own declaration, never from the Ordo
/// field's `?`: a column another system owns is never null only if the
/// catalog says `NOT NULL`.
pub fn not_null_columns(schema: &Schema) -> ir::NotNullColumns {
    let mut out = std::collections::BTreeMap::new();
    for e in &schema.entities {
        let cols: std::collections::BTreeSet<String> = ordo_schema::ddl::physical_columns(e)
            .into_iter()
            .filter(|c| c.not_null)
            .map(|c| c.name)
            .collect();
        out.insert(e.name.clone(), cols);
    }
    std::sync::Arc::new(out)
}

/// Attach the schema's `NOT NULL` evidence to a query the binder hands out.
fn with_not_null(mut q: Query, schema: &Schema) -> Query {
    q.not_null = Some(not_null_columns(schema));
    q
}

impl<'a> Binder<'a> {
    fn alias(&mut self) -> String {
        let a = format!("t{}", self.alias_n);
        self.alias_n += 1;
        a
    }

    // ── pipelines ────────────────────────────────────────────────────────

    fn pipeline(&mut self, pl: &Pipeline, env: &ParamEnv) -> Result<Query, SemaError> {
        // Bind lets in order; each becomes a CTE and a name later sources
        // and scalar references resolve. Scoped to this pipeline.
        let let_mark = self.lets.len();
        let mut ctes: Vec<Cte> = Vec::new();
        for l in &pl.lets {
            if self.lets.iter().any(|x| x.name == l.name.name) {
                return Err(err(
                    "unsupported",
                    l.name.span,
                    format!("`{}` is already a let in this pipeline", l.name.name),
                ));
            }
            let lq = self.pipeline(&l.pipeline, env)?;
            let sf = self.scope_fields(&lq.outputs, &lq)?;
            // Group-key columns remember where they came from (DL "Let
            // outputs as sources"): an entity's `id` key points at that
            // entity as a ref would, so match inference can join the let
            // back to it (reported fix 25at).
            let prov = self.group_key_provenance(&lq);
            let fields = sf
                .iter()
                .map(|f| {
                    let rt = f.ref_target.clone().or_else(|| {
                        prov.iter()
                            .find(|(n, _, _)| n == &f.name)
                            .map(|(_, e, _)| e.clone())
                    });
                    (f.name.clone(), f.expr.ty(), rt)
                })
                .collect();
            let unique_keys = prov
                .iter()
                .filter(|(_, _, unique)| *unique)
                .map(|(n, _, _)| n.clone())
                .collect();
            let ends_in_reduce = lq
                .group
                .as_ref()
                .map(|g| g.keys.is_empty())
                .unwrap_or(false);
            self.lets.push(LetInfo {
                name: l.name.name.clone(),
                fields,
                seals: lq.host_open.clone(),
                ends_in_reduce,
                unique_keys,
            });
            ctes.push(Cte {
                name: l.name.name.clone(),
                query: lq,
            });
        }
        let (mut q, src_scope) = self.source(&pl.sources[0], env)?;

        let mut scope = if pl.sources.len() == 1 {
            // The projection settles the OUTPUT field set; steps resolve
            // against outputs first and fall back to the source scope
            // [ruling W047] — a key does not need to be projected to sort
            // or filter by it.
            let out_scope = self.project(&mut q, &pl.sources[0], src_scope.clone(), env)?;
            Scope::Layered(vec![out_scope, src_scope])
        } else {
            // `match <ref>` / `match on <col>` steps pre-select join edges
            // for parallel-edge ambiguity (DL ~1594); collected before
            // inference runs.
            let mut ref_sel: Vec<String> = Vec::new();
            let mut on_sel: Vec<String> = Vec::new();
            for st in &pl.steps {
                match st {
                    Step::Match {
                        kind: MatchKind::Ref { path },
                        ..
                    } => ref_sel.push(
                        path.iter()
                            .map(|i| i.name.clone())
                            .collect::<Vec<_>>()
                            .join("."),
                    ),
                    Step::Match {
                        kind: MatchKind::On { col },
                        ..
                    } => on_sel.push(col.name.clone()),
                    _ => None::<()>.map(|_| ()).unwrap_or(()),
                }
            }
            self.bind_multi_source(&mut q, pl, src_scope, &ref_sel, &on_sel, env)?
        };

        for step in &pl.steps {
            // A step after `take` operates on the taken rows, not the
            // pre-take stream — intra-pipeline post-take semantics are as
            // structural as the cross-composition ones. (Antagonist
            // finding 2: `take 3 sort -total` re-ranked before limiting.)
            let settles = matches!(step, Step::Group { .. } | Step::Reduce { .. })
                && (q.distinct || q.group.is_some());
            if q.limit.is_some() || settles {
                let (wq, ws) = self.wrap(q);
                q = wq;
                scope = ws;
            }
            // take[keys] settles rows per bucket and needs the query by
            // value for its window wrap — handled here, not in step().
            if let Step::Take { keys, n, span } = step {
                if !keys.is_empty() {
                    q = self.take_keys(q, &scope, keys, n, *span, env)?;
                    scope = Scope::Fields(self.scope_fields(&q.outputs, &q)?);
                    continue;
                }
            }
            if let Step::Append { source, span } = step {
                q = self.append_arm(q, source, *span, env)?;
                scope = Scope::Fields(self.scope_fields(&q.outputs, &q)?);
                continue;
            }
            self.step(&mut q, &scope, step, env)?;
            // Group and distinct destroy row identity: the source row no
            // longer exists behind an output row, so the fallback dies and
            // later steps see outputs only.
            if matches!(step, Step::Group { .. } | Step::Distinct { .. }) {
                scope = Scope::Fields(self.scope_fields(&q.outputs, &q)?);
            }
        }
        // This pipeline's CTEs ride its outermost query; its lets go out of
        // scope with it.
        if !ctes.is_empty() {
            ctes.extend(std::mem::take(&mut q.ctes));
            q.ctes = ctes;
        }
        self.lets.truncate(let_mark);
        Ok(q)
    }

    fn source(&mut self, s: &Source, env: &ParamEnv) -> Result<(Query, Scope), SemaError> {
        if let Some(info) = self.lets.iter().find(|l| l.name == s.name.name).cloned() {
            return self.let_source(s, &info, env);
        }
        if self.schema.entity(&s.name.name).is_some() {
            self.entity_source(s, env)
        } else if self.registry.get(&s.name.name).is_some() {
            self.composed_source(s, env)
        } else {
            Err(err(
                "unknown_source",
                s.name.span,
                format!(
                    "`{}` is neither an entity nor a registered op in the deployed definition set",
                    s.name.name
                ),
            ))
        }
    }

    fn entity_source(&mut self, s: &Source, env: &ParamEnv) -> Result<(Query, Scope), SemaError> {
        let entity = self.schema.entity(&s.name.name).expect("checked").clone();
        let alias = self.alias();
        let scope = Scope::Entity {
            entity: entity.name.clone(),
            alias: alias.clone(),
        };

        let mut q = Query {
            not_null: None,
            from: From::Table {
                entity: entity.name.clone(),
                alias: alias.clone(),
            },
            joins: Vec::new(),
            filters: Vec::new(),
            having: Vec::new(),
            ctes: Vec::new(),
            source_joins: Vec::new(),
            group: None,
            outputs: Vec::new(),
            sort: Vec::new(),
            sort_inherited: false,
            host_open: Vec::new(),
            limit: None,
            distinct: false,
            tiebreak: vec![BExpr::Col {
                alias: alias.clone(),
                column: "id".into(),
                ty: ScalarTy::Id,
            }],
        };

        for a in &s.args {
            match a {
                SourceArg::Named { name, .. } => {
                    return Err(err(
                        "unknown_param",
                        name.span,
                        format!(
                            "`{}` takes no parameters — `${}` is a call-form argument to an entity source",
                            s.name.name, name.name
                        ),
                    ))
                }
                SourceArg::Cond(c) => {
                    let bc = self.cond(c, &scope, &mut q, env)?;
                    q.filters.push(bc);
                }
            }
        }
        // What bounds the rows this source may see. Three cases, and which
        // one applies is decided by WHO CHOSE THE MOMENT this body runs —
        // never by which principal it runs as (R-G20).
        //
        // Note which marker does which job. `unguarded` is about AUTHORITY
        // and appears in two arms below; `unbounded` is about WORK and does
        // not appear at all, because widening how much a run reads was never
        // a licence to widen what it may reach (GC096's probe).
        match &self.provenance {
            // G5: the entity's read guard inlines as a conjunctive semi-join
            // — before the op's own sort/take by construction (G13),
            // bypassed only by this body's `unguarded` (G14).
            Provenance::Caller => {
                if !self.unguarded {
                    if let Some(gc) = self.guard_cond(&entity.name, &alias, env)? {
                        q.filters.push(gc);
                    }
                }
            }
            // R-G20, clock: nobody outside chose the moment, so there is no
            // confused deputy to be and guards pass. This is deliberately
            // NOT spelled as "set `unguarded`": the flag is an author's
            // acknowledgment and this is the platform's own wake-up, and
            // conflating them would make `bypassed_guards` in the artifact
            // report a decision nobody made.
            Provenance::Clock => {}
            // R-G20, listener: the run may read only rows reachable from
            // `$trigger.ids`; every other row is invisible exactly as a
            // guard would make it. `unguarded` on the run header lifts it —
            // one greppable word, a decision someone made on purpose.
            Provenance::Listener(rows) => {
                if !self.unguarded {
                    q.filters.push(trigger_bound(&entity.name, &alias, rows));
                }
            }
        }
        Ok((q, scope))
    }

    /// What bounds a row **the compiler brought into the query**: the target
    /// of a dot-hop, or the bridge between two listed sources.
    ///
    /// The same three-way decision `entity_source` makes for a listed
    /// source, in one place so the two compiler-added nodes cannot drift
    /// from it or from each other. That drift is the whole of S6-001: the
    /// decision existed for the source a developer wrote and for neither of
    /// the rows the compiler wrote on their behalf.
    ///
    /// `unguarded` lifts it, because the word is the author's acknowledgment
    /// for the whole body and a body that may read every `tasks` row was
    /// never promised a narrower view of `projects`. Clock and listener
    /// provenance add nothing: a clock run passes guards by R-G20, and a
    /// listener run is bounded at its anchor to rows reachable from
    /// `$trigger.ids`, which a hop from such a row is.
    fn joined_bound(
        &mut self,
        entity: &str,
        alias: &str,
        env: &ParamEnv,
    ) -> Result<Option<BCond>, SemaError> {
        if self.unguarded {
            return Ok(None);
        }
        match &self.provenance {
            Provenance::Caller => self.guard_cond(entity, alias, env),
            Provenance::Clock | Provenance::Listener(_) => Ok(None),
        }
    }

    /// The read guard of `entity`, compiled against the row at `alias`:
    /// EXISTS semi-joins for every hop, `$caller` from the environment when
    /// bound (the harness's `as:`), the prepared slot otherwise, FALSE for
    /// anonymous (G15 — a defined rule, not null folklore).
    fn guard_cond(
        &mut self,
        entity: &str,
        alias: &str,
        env: &ParamEnv,
    ) -> Result<Option<BCond>, SemaError> {
        if self.body_only {
            return Ok(None);
        }
        let Some(g) = self.schema.entity(entity).and_then(|e| e.guards.clone()) else {
            return Ok(None);
        };
        self.guard_cond_of(&g, entity, alias, env)
    }

    /// The compiled form of a specific guard body — `guard_cond` passes an
    /// entity's own guards; the write path passes a copy whose `read` slot
    /// holds the WRITE walk, reusing the whole rebase/EXISTS machinery.
    fn guard_cond_of(
        &mut self,
        g: &ordo_schema::Guards,
        entity: &str,
        alias: &str,
        env: &ParamEnv,
    ) -> Result<Option<BCond>, SemaError> {
        let (walk, rebase, origin) = match (&g.read, &g.source) {
            (ordo_schema::GuardBody::Open, _) => return Ok(None),
            (ordo_schema::GuardBody::Walk(w), ordo_schema::GuardSource::Inherited { from }) => {
                (w.clone(), g.rebase.clone(), from.clone())
            }
            (ordo_schema::GuardBody::Walk(w), _) => (w.clone(), Vec::new(), entity.to_string()),
        };
        if self.caller == Caller::Anonymous {
            // Reads return empty; the guard IS false.
            return Ok(Some(BCond::Bare(BExpr::Lit {
                value: Expr::BoolLit {
                    value: false,
                    span: walk.span,
                },
                ty: ScalarTy::Bool,
            })));
        }
        // The rebase walks from the child up to the walk's origin entity:
        // chase the owned refs so the walk starts on the right row.
        let mut cur_entity = entity.to_string();
        // Build nested EXISTS from the outside in: for a rebase [task],
        // guard applies to the row task points at, so we wrap the walk in
        // EXISTS over the parent: EXISTS(SELECT 1 FROM tasks p WHERE p.id =
        // child.task AND <walk from p>). Simpler: fold rebase as forward
        // hops prefixed onto every atom via ref columns.
        let mut prefix_alias = alias.to_string();
        let mut inner_conds: Vec<BCond> = Vec::new();
        for hop in &rebase {
            // child.hop -> parent: EXISTS over parent rows.
            let e = self.schema.entity(&cur_entity).expect("resolved");
            let Some(f) = e.field(hop) else {
                return Err(err(
                    "invalid_guard_walk",
                    walk.span,
                    format!("rebase hop `{hop}` missing on `{cur_entity}`"),
                ));
            };
            let FieldTy::Ref { target, .. } = &f.ty else {
                return Err(err(
                    "invalid_guard_walk",
                    walk.span,
                    "rebase hop is not a ref",
                ));
            };
            let pa = self.alias();
            let parent = target.clone();
            let link = BCond::Cmp {
                op: ordo_syntax::CmpOp::Eq,
                lhs: BExpr::Col {
                    alias: pa.clone(),
                    column: "id".into(),
                    ty: ScalarTy::Id,
                },
                rhs: BExpr::Col {
                    alias: prefix_alias.clone(),
                    column: hop.clone(),
                    ty: ScalarTy::Id,
                },
            };
            inner_conds.push(link);
            prefix_alias = pa;
            cur_entity = parent;
        }
        debug_assert_eq!(cur_entity, origin);

        let walk_cond = self.guard_expr_cond(&origin, &prefix_alias, &walk, env)?;

        if rebase.is_empty() {
            return Ok(Some(walk_cond));
        }
        // Wrap the rebase chain in one EXISTS over the joined ancestors.
        // Build FROM the origin row: EXISTS(SELECT 1 FROM <ancestors...>)
        // — v1 supports a single-hop chain per level by nesting.
        let mut cond = walk_cond;
        // innermost link comes last in inner_conds; nest from origin out.
        for (hop_i, link) in inner_conds.into_iter().enumerate().rev() {
            let ent = self.rebase_entity_at(entity, &rebase, hop_i)?;
            let sub = Query {
                not_null: None,
                from: From::Table {
                    entity: ent.clone(),
                    alias: self.rebase_alias_of(&link),
                },
                joins: Vec::new(),
                filters: vec![link, cond],
                having: Vec::new(),
                ctes: Vec::new(),
                source_joins: Vec::new(),
                group: None,
                outputs: Vec::new(),
                sort: Vec::new(),
                sort_inherited: false,
                host_open: Vec::new(),
                limit: None,
                distinct: false,
                tiebreak: Vec::new(),
            };
            cond = BCond::Exists {
                query: Box::new(sub),
                negated: false,
            };
        }
        Ok(Some(cond))
    }

    fn rebase_entity_at(
        &self,
        child: &str,
        rebase: &[String],
        i: usize,
    ) -> Result<String, SemaError> {
        let mut cur = child.to_string();
        for hop in &rebase[..=i] {
            let e = self.schema.entity(&cur).expect("resolved");
            let Some(FieldTy::Ref { target, .. }) = e.field(hop).map(|f| &f.ty) else {
                return Err(err(
                    "invalid_guard_walk",
                    Span::new(0, 0, 1, 1),
                    "rebase hop is not a ref",
                ));
            };
            cur = target.clone();
        }
        Ok(cur)
    }

    fn rebase_alias_of(&self, link: &BCond) -> String {
        if let BCond::Cmp {
            lhs: BExpr::Col { alias, .. },
            ..
        } = link
        {
            alias.clone()
        } else {
            "t?".into()
        }
    }

    /// One guard expression compiled at `alias` on `entity`.
    fn guard_expr_cond(
        &mut self,
        entity: &str,
        alias: &str,
        w: &ordo_syntax::GuardExpr,
        env: &ParamEnv,
    ) -> Result<BCond, SemaError> {
        let mut ors = Vec::new();
        for conj in &w.disjuncts {
            let mut ands = Vec::new();
            for atom in conj {
                ands.push(self.guard_atom_cond(entity, alias, atom, env)?);
            }
            ors.push(if ands.len() == 1 {
                ands.pop().expect("one")
            } else {
                BCond::And(ands)
            });
        }
        Ok(if ors.len() == 1 {
            ors.pop().expect("one")
        } else {
            BCond::Or(ors)
        })
    }

    fn caller_expr(&self, env: &ParamEnv) -> BExpr {
        env.get("caller").cloned().unwrap_or(BExpr::Param {
            name: "caller".into(),
            ty: ScalarTy::Id,
        })
    }

    /// One forward ref hop compiled as an EXISTS over the parent row, with
    /// the rest of the atom evaluated there. Shared by Cmp/Bare/Step peels.
    fn guard_hop_exists(
        &mut self,
        entity: &str,
        alias: &str,
        seg: &str,
        target: &str,
        rest: &ordo_syntax::GuardAtom,
        env: &ParamEnv,
    ) -> Result<BCond, SemaError> {
        let _ = entity;
        let pa = self.alias();
        let link = BCond::Cmp {
            op: ordo_syntax::CmpOp::Eq,
            lhs: BExpr::Col {
                alias: pa.clone(),
                column: "id".into(),
                ty: ScalarTy::Id,
            },
            rhs: BExpr::Col {
                alias: alias.to_string(),
                column: seg.to_string(),
                ty: ScalarTy::Id,
            },
        };
        let inner = self.guard_atom_cond(target, &pa, rest, env)?;
        Ok(BCond::Exists {
            query: Box::new(Query {
                not_null: None,
                from: From::Table {
                    entity: target.to_string(),
                    alias: pa,
                },
                joins: Vec::new(),
                filters: vec![link, inner],
                having: Vec::new(),
                ctes: Vec::new(),
                source_joins: Vec::new(),
                group: None,
                outputs: Vec::new(),
                sort: Vec::new(),
                sort_inherited: false,
                host_open: Vec::new(),
                limit: None,
                distinct: false,
                tiebreak: Vec::new(),
            }),
            negated: false,
        })
    }

    fn guard_atom_cond(
        &mut self,
        entity: &str,
        alias: &str,
        atom: &ordo_syntax::GuardAtom,
        env: &ParamEnv,
    ) -> Result<BCond, SemaError> {
        use ordo_syntax::GuardAtom as GA;
        match atom {
            GA::Cmp {
                path,
                caller,
                lit,
                span,
            } => {
                // `project.owner == $caller`: peel the leading ref hop into
                // an EXISTS over the parent and recurse with the remainder.
                if path.len() > 1 {
                    if let Some(FieldTy::Ref { target, .. }) = self
                        .schema
                        .entity(entity)
                        .and_then(|e| e.field(&path[0].name))
                        .map(|f| f.ty.clone())
                    {
                        let rest = GA::Cmp {
                            path: path[1..].to_vec(),
                            caller: *caller,
                            lit: lit.clone(),
                            span: *span,
                        };
                        return self.guard_hop_exists(
                            entity,
                            alias,
                            &path[0].name,
                            &target,
                            &rest,
                            env,
                        );
                    }
                }
                let (lhs, home_alias) =
                    self.guard_forward(entity, alias, &path[..path.len() - 1], *span)?;
                let last = &path[path.len() - 1].name;
                let e = self.schema.entity(&lhs).expect("resolved");
                let Some(f) = e.field(last) else {
                    return Err(err(
                        "invalid_guard_walk",
                        *span,
                        format!("`{lhs}` has no field `{last}`"),
                    ));
                };
                let col = BExpr::Col {
                    alias: home_alias,
                    column: f.column(),
                    ty: f.ty.value_ty(),
                };
                let rhs = if *caller {
                    self.caller_expr(env)
                } else {
                    // 9f‴. This literal used to be wrapped raw, with
                    // `lit_ty(...).unwrap_or(Text)` and no reference to the
                    // column beside it — the one path where a comparison is
                    // built without ever consulting what it compares against.
                    //
                    // The consequence was not cosmetic: `qty == "seven"` on
                    // an `int` field REGISTERED, PLANNED, and emitted
                    // `"t0"."qty" = 'seven'`, so a real server answered
                    // `invalid input syntax for type bigint` on every read.
                    // A registered op that looks healthy and fails at request
                    // time — on the AUTHORIZATION path, where the failure
                    // mode of a guard that cannot be evaluated is the worst
                    // one available.
                    //
                    // `check_guard_lit_ty` is `fit_slot` restricted to
                    // literals. It is deliberately the same rules rather than
                    // a second opinion: a guard is not a place to be more
                    // permissive than a parameter slot, and two type tables
                    // would eventually disagree about `money`.
                    let l = lit.clone().expect("non-caller cmp has a literal");
                    let ty = lit_ty(&l).unwrap_or(ScalarTy::Text);
                    let lit_expr = BExpr::Lit { value: l, ty };
                    check_guard_lit_ty(lit_expr, col.ty(), &f.name, *span)?
                };
                Ok(BCond::Cmp {
                    op: ordo_syntax::CmpOp::Eq,
                    lhs: col,
                    rhs,
                })
            }
            GA::Bare { path, span } => {
                if path.len() > 1 {
                    if let Some(FieldTy::Ref { target, .. }) = self
                        .schema
                        .entity(entity)
                        .and_then(|e| e.field(&path[0].name))
                        .map(|f| f.ty.clone())
                    {
                        let rest = GA::Bare {
                            path: path[1..].to_vec(),
                            span: *span,
                        };
                        return self.guard_hop_exists(
                            entity,
                            alias,
                            &path[0].name,
                            &target,
                            &rest,
                            env,
                        );
                    }
                }
                let (lhs, home_alias) =
                    self.guard_forward(entity, alias, &path[..path.len() - 1], *span)?;
                let last = &path[path.len() - 1].name;
                let e = self.schema.entity(&lhs).expect("resolved");
                let Some(f) = e.field(last) else {
                    return Err(err(
                        "invalid_guard_walk",
                        *span,
                        format!("`{lhs}` has no field `{last}`"),
                    ));
                };
                Ok(BCond::Bare(BExpr::Col {
                    alias: home_alias,
                    column: f.column(),
                    ty: f.ty.value_ty(),
                }))
            }
            GA::Step { path, inner, span } => {
                // forward hops, then the reverse-step entity (+optional ref).
                let i = 0;
                let cur = entity.to_string();
                let cur_alias = alias.to_string();
                // A leading forward hop peels one ref and recurses (the
                // recursion handles arbitrarily deep prefixes); otherwise
                // the first segment is the reverse-step entity.
                if !path.is_empty() {
                    let seg = &path[0].name;
                    let e = self.schema.entity(&cur).expect("resolved");
                    if let Some(FieldTy::Ref { target, .. }) = e.field(seg).map(|f| f.ty.clone()) {
                        // hop via EXISTS to the parent row
                        let pa = self.alias();
                        let link = BCond::Cmp {
                            op: ordo_syntax::CmpOp::Eq,
                            lhs: BExpr::Col {
                                alias: pa.clone(),
                                column: "id".into(),
                                ty: ScalarTy::Id,
                            },
                            rhs: BExpr::Col {
                                alias: cur_alias.clone(),
                                column: seg.clone(),
                                ty: ScalarTy::Id,
                            },
                        };
                        // remaining path from the parent:
                        let rest = ordo_syntax::GuardAtom::Step {
                            path: path[i + 1..].to_vec(),
                            inner: inner.clone(),
                            span: *span,
                        };
                        let inner_cond = self.guard_atom_cond(&target, &pa, &rest, env)?;
                        let sub = Query {
                            not_null: None,
                            from: From::Table {
                                entity: target.clone(),
                                alias: pa,
                            },
                            joins: Vec::new(),
                            filters: vec![link, inner_cond],
                            having: Vec::new(),
                            ctes: Vec::new(),
                            source_joins: Vec::new(),
                            group: None,
                            outputs: Vec::new(),
                            sort: Vec::new(),
                            sort_inherited: false,
                            host_open: Vec::new(),
                            limit: None,
                            distinct: false,
                            tiebreak: Vec::new(),
                        };
                        return Ok(BCond::Exists {
                            query: Box::new(sub),
                            negated: false,
                        });
                    }
                }
                let step_entity = &path[i].name;
                let se = self.schema.entity(step_entity).ok_or_else(|| {
                    err(
                        "invalid_guard_walk",
                        *span,
                        format!("`{step_entity}` is not an entity"),
                    )
                })?;
                let arriving: Vec<&ordo_schema::Field> = se
                    .fields
                    .iter()
                    .filter(|f| matches!(&f.ty, FieldTy::Ref { target, .. } if target == &cur))
                    .collect();
                let named = path.get(i + 1).map(|id| id.name.as_str());
                let via = match (arriving.len(), named) {
                    (1, None) => arriving[0].name.clone(),
                    (_, Some(r)) if arriving.iter().any(|f| f.name == r) => r.to_string(),
                    _ => {
                        return Err(err(
                            "invalid_guard_walk",
                            *span,
                            format!("`{step_entity}` does not arrive at `{cur}` unambiguously"),
                        ))
                    }
                };
                let sa = self.alias();
                let link = BCond::Cmp {
                    op: ordo_syntax::CmpOp::Eq,
                    lhs: BExpr::Col {
                        alias: sa.clone(),
                        column: via,
                        ty: ScalarTy::Id,
                    },
                    rhs: BExpr::Col {
                        alias: cur_alias.clone(),
                        column: "id".into(),
                        ty: ScalarTy::Id,
                    },
                };
                let inner_cond = self.guard_expr_cond(step_entity, &sa, inner, env)?;
                let sub = Query {
                    not_null: None,
                    from: From::Table {
                        entity: step_entity.clone(),
                        alias: sa,
                    },
                    joins: Vec::new(),
                    filters: vec![link, inner_cond],
                    having: Vec::new(),
                    ctes: Vec::new(),
                    source_joins: Vec::new(),
                    group: None,
                    outputs: Vec::new(),
                    sort: Vec::new(),
                    sort_inherited: false,
                    host_open: Vec::new(),
                    limit: None,
                    distinct: false,
                    tiebreak: Vec::new(),
                };
                Ok(BCond::Exists {
                    query: Box::new(sub),
                    negated: false,
                })
            }
        }
    }

    /// Forward hops for guard Cmp/Bare paths — v1 supports row-local paths
    /// (len 1) and one forward hop compiled as an EXISTS is handled by the
    /// Cmp caller; deeper chains arrive via Step recursion.
    fn guard_forward(
        &mut self,
        entity: &str,
        alias: &str,
        hops: &[ordo_syntax::Ident],
        span: Span,
    ) -> Result<(String, String), SemaError> {
        if hops.is_empty() {
            return Ok((entity.to_string(), alias.to_string()));
        }
        Err(err(
            "invalid_guard_walk",
            span,
            "forward-hop comparisons beyond the row are compiled via steps in this slice",
        ))
    }

    fn composed_source(&mut self, s: &Source, env: &ParamEnv) -> Result<(Query, Scope), SemaError> {
        let name = s.name.name.clone();
        if self.stack.contains(&name) {
            let mut chain = self.stack.clone();
            chain.push(name.clone());
            return Err(err(
                "cycle",
                s.name.span,
                format!("composition cycle: {}", chain.join(" -> ")),
            ));
        }
        let op = self.registry.get(&name).expect("checked").clone();

        // Callee parameters bind from the caller's named arguments, evaluated
        // in the *caller's* environment — that is what makes `$from: $from`
        // flow-through rather than self-reference.
        let callee_env = self.param_env_with(&op, &s.args, env, s.name.span)?;

        self.stack.push(name.clone());
        let saved = self.unguarded;
        self.unguarded = op.flags.unguarded;
        // R-G13 for the provenance bound: a registered op's own entity
        // sources are bound the way that op declares, not the way the body
        // that composed it was woken. A routine reaching data through a
        // callee gets the callee's guards, and `definition-language.md` §8
        // says the marker is still required — "Composing a callee whose
        // bypass is live requires the marker in either case, because that
        // marker is about the callee's semantics, not the caller's
        // identity."
        let saved_prov = std::mem::take(&mut self.provenance);
        let Some(callee_pl) = op.read_pipeline() else {
            return Err(err(
                "unsupported",
                s.name.span,
                format!("`{}` is a write op — it cannot be a source", s.name.name),
            ));
        };
        let callee = self.pipeline(callee_pl, &callee_env);
        self.unguarded = saved;
        self.provenance = saved_prov;
        self.stack.pop();
        let callee = callee?;

        // A callee whose row set is already settled cannot be merged into:
        // a caller condition would otherwise filter *before* the take, and
        // C004 exists precisely to catch that.
        let settled = callee.limit.is_some() || callee.group.is_some() || callee.distinct;

        let (mut q, scope) = if settled {
            self.wrap(callee)
        } else {
            let fields = self.arm_blind(self.scope_fields(&callee.outputs, &callee)?);
            let mut q = callee;
            // The callee's own sort is inherited order for the caller
            // exactly as a wrapped callee's is — a caller's declared sort
            // must replace it, not append after it. (Antagonist finding 1:
            // `recentOrders sort -total` kept placed-order as primary.)
            q.sort_inherited = !q.sort.is_empty();
            (q, Scope::Fields(fields))
        };

        for a in &s.args {
            match a {
                // Named arguments were consumed as parameter bindings above.
                SourceArg::Named { .. } => {}
                SourceArg::Cond(c) => {
                    let bc = self.cond(c, &scope, &mut q, env)?;
                    q.filters.push(bc);
                }
            }
        }
        Ok((q, scope))
    }

    /// A let referenced as a source: FROM the CTE, its outputs the fields
    /// (DL ~1785 — a let output is a first-class source, restrictable and
    /// projectable, joined back on its provenance-tracked keys).
    fn let_source(
        &mut self,
        s: &Source,
        info: &LetInfo,
        env: &ParamEnv,
    ) -> Result<(Query, Scope), SemaError> {
        let alias = self.alias();
        // A sealed output of the let stays sealed when the let is read: the
        // CTE hands over the envelope, and the obligation to open it
        // travels with the name, exactly as it does across composition.
        // Dropping it here served the envelope as text (reported fix 25r).
        let fields: Vec<SField> = info
            .fields
            .iter()
            .map(|(n, ty, rt)| {
                let seal = info
                    .seals
                    .iter()
                    .find(|(o, _, _)| o == n)
                    .map(|(_, e, f)| (e.clone(), f.clone()));
                SField {
                    name: n.clone(),
                    expr: BExpr::Col {
                        alias: alias.clone(),
                        column: n.clone(),
                        ty: *ty,
                    },
                    ref_target: rt.clone(),
                    sealed: seal.is_some(),
                    seal_of: seal,
                    blind: None,
                    implicit: false,
                }
            })
            .collect();
        let host_open = self.host_open_of(&fields);
        let scope = Scope::Fields(fields.clone());
        let mut q = Query {
            not_null: None,
            from: From::CteRef {
                name: info.name.clone(),
                alias,
            },
            joins: Vec::new(),
            filters: Vec::new(),
            having: Vec::new(),
            ctes: Vec::new(),
            source_joins: Vec::new(),
            group: None,
            outputs: fields
                .iter()
                .map(|f| OutField {
                    name: f.name.clone(),
                    expr: f.expr.clone(),
                })
                .collect(),
            sort: Vec::new(),
            sort_inherited: false,
            host_open,
            limit: None,
            distinct: false,
            tiebreak: fields.iter().map(|f| f.expr.clone()).collect(),
        };
        for a in &s.args {
            match a {
                SourceArg::Named { name, .. } => {
                    return Err(err(
                        "unknown_param",
                        name.span,
                        format!(
                            "`{}` is a let, not an op — it takes no parameters",
                            s.name.name
                        ),
                    ))
                }
                SourceArg::Cond(c) => {
                    let bc = self.cond(c, &scope, &mut q, env)?;
                    q.filters.push(bc);
                }
            }
        }
        Ok((q, scope))
    }

    /// `append <source>` — UNION ALL (G8): columns matched BY NAME, types
    /// agreeing exactly — no coercion, no null-filling; misalignment is
    /// `append_misalignment`. The unioned stream re-orders by declared
    /// sorts after the append, then arm index, then every output column —
    /// a total order without demanding the arms share a tiebreak type.
    fn append_arm(
        &mut self,
        q: Query,
        src: &Source,
        span: Span,
        env: &ParamEnv,
    ) -> Result<Query, SemaError> {
        // Bind and project the arm as its own query.
        let (mut a_q, a_scope) = self.source(src, env)?;
        let a_fields = self.project_fields(&mut a_q, src, &a_scope, env)?;
        a_q.outputs = a_fields
            .iter()
            .map(|f| OutField {
                name: f.name.clone(),
                expr: f.expr.clone(),
            })
            .collect();

        // Alignment by name, exact type, arm reordered to the anchor's
        // column order.
        let mut arm_outputs = Vec::new();
        for o in &q.outputs {
            let found = a_q
                .outputs
                .iter()
                .find(|a| a.name == o.name)
                .ok_or_else(|| {
                    err(
                        "append_misalignment",
                        span,
                        format!("`{}` does not produce column `{}`", src.name.name, o.name),
                    )
                })?;
            if found.expr.ty() != o.expr.ty() {
                return Err(err(
                    "append_misalignment",
                    span,
                    format!(
                        "column `{}` is `{}` here and `{}` in the appended arm — no coercion",
                        o.name,
                        o.expr.ty().as_str(),
                        found.expr.ty().as_str()
                    ),
                ));
            }
            // A column is sealed in both arms under the same field, or in
            // neither. The union hands the host one column, opened under
            // one `(entity, field)`: an envelope beside plaintext would
            // serve the envelope as text, and two different sealed fields
            // would open one of them under the wrong key binding.
            let seal_here = q
                .host_open
                .iter()
                .find(|(n, _, _)| n == &o.name)
                .map(|(_, e, f)| (e.clone(), f.clone()));
            let seal_there = a_fields
                .iter()
                .find(|f| f.name == o.name)
                .and_then(|f| f.seal_of.clone());
            if seal_here != seal_there {
                let say = |s: &Option<(String, String)>| match s {
                    Some((e, f)) => format!("sealed as `{e}.{f}`"),
                    None => "not sealed".to_string(),
                };
                return Err(err(
                    "append_misalignment",
                    span,
                    format!(
                        "column `{}` is {} here and {} in the appended arm",
                        o.name,
                        say(&seal_here),
                        say(&seal_there)
                    ),
                ));
            }
            arm_outputs.push(found.clone());
        }
        // The arm opens what it projects, as any query does, and the union
        // carries the same obligations as its first arm: the alignment
        // above made them agree (reported fix 25r).
        a_q.host_open = self.host_open_of(&a_fields);
        let union_host_open = q.host_open.clone();
        if a_q.outputs.len() != q.outputs.len() {
            let extra: Vec<&str> = a_q
                .outputs
                .iter()
                .filter(|a| !q.outputs.iter().any(|o| o.name == a.name))
                .map(|a| a.name.as_str())
                .collect();
            return Err(err(
                "append_misalignment",
                span,
                format!("the appended arm produces extra columns: {extra:?}"),
            ));
        }
        a_q.outputs = arm_outputs;

        // Fold into a union. A chained append pushes another arm.
        let names: Vec<(String, ScalarTy)> = q
            .outputs
            .iter()
            .map(|o| (o.name.clone(), o.expr.ty()))
            .collect();
        let arm_tag = |i: i64| OutField {
            name: "$arm".into(),
            expr: BExpr::Lit {
                value: Expr::IntLit {
                    value: i as i128,
                    span,
                },
                ty: ScalarTy::Int,
            },
        };
        // `$arm` plus the outputs orders the union, but only up to row
        // IDENTITY: two identical rows in the SAME arm are indistinguishable,
        // and a keyset cursor is STRICTLY after its key, so paging would
        // exclude every duplicate past the first and silently drop rows.
        //
        // So each arm also carries `$uid`, a row number over that arm's own
        // ordering. Which physical row of a run of identical rows gets which
        // number is not stable across queries and does not need to be: the
        // rows are byte-identical, so any assignment yields the same values
        // in the same quantity. What it buys is a per-row-unique final key,
        // which is what keyset paging actually requires.
        let arm_uid = |outs: &[OutField]| OutField {
            name: "$uid".into(),
            expr: BExpr::RowNumber {
                partition: Vec::new(),
                order: outs
                    .iter()
                    .map(|o| Sort {
                        expr: o.expr.clone(),
                        desc: false,
                    })
                    .collect(),
                ty: ScalarTy::Int,
            },
        };
        let (arms, alias, ctes) = match q.from {
            From::Union { arms, alias } if q.filters.is_empty() && q.group.is_none() => {
                let mut arms = arms;
                a_q.sort.clear();
                a_q.sort_inherited = false;
                a_q.outputs.push(arm_uid(&a_q.outputs.clone()));
                a_q.outputs.push(arm_tag(arms.len() as i64));
                arms.push(a_q);
                (arms, alias, q.ctes)
            }
            _ => {
                let mut first = q;
                let ctes = std::mem::take(&mut first.ctes);
                first.sort.clear();
                first.sort_inherited = false;
                first.outputs.push(arm_uid(&first.outputs.clone()));
                first.outputs.push(arm_tag(0));
                a_q.sort.clear();
                a_q.sort_inherited = false;
                a_q.outputs.push(arm_uid(&a_q.outputs.clone()));
                a_q.outputs.push(arm_tag(1));
                (vec![first, a_q], self.alias(), ctes)
            }
        };

        let outputs: Vec<OutField> = names
            .iter()
            .map(|(n, ty)| OutField {
                name: n.clone(),
                expr: BExpr::Col {
                    alias: alias.clone(),
                    column: n.clone(),
                    ty: *ty,
                },
            })
            .collect();
        let mut tiebreak = vec![BExpr::Col {
            alias: alias.clone(),
            column: "$arm".into(),
            ty: ScalarTy::Int,
        }];
        tiebreak.extend(outputs.iter().map(|o| o.expr.clone()));
        // Last, and only ever a tiebreak of last resort: the per-arm row
        // number that makes the whole order total rather than total-up-to-
        // identity.
        tiebreak.push(BExpr::Col {
            alias: alias.clone(),
            column: "$uid".into(),
            ty: ScalarTy::Int,
        });

        Ok(Query {
            not_null: None,
            from: From::Union { arms, alias },
            joins: Vec::new(),
            filters: Vec::new(),
            having: Vec::new(),
            ctes,
            source_joins: Vec::new(),
            group: None,
            outputs,
            sort: Vec::new(),
            sort_inherited: false,
            host_open: union_host_open,
            limit: None,
            distinct: false,
            tiebreak,
        })
    }

    /// `match any <collection>` / `match none <collection>` — the existence
    /// operators (DL ~1662): a semi/anti-join, NEVER a preservation marker
    /// (ruling 3, amendment 1). The collection binds as its own query; the
    /// correlation edge is inferred exactly as a join edge would be — the
    /// collection's ref pointing at the anchor — and lands inside the
    /// subquery as an ordinary filter referencing the outer alias.
    fn bind_exists(
        &mut self,
        q: &mut Query,
        src: &Source,
        negated: bool,
        span: Span,
        env: &ParamEnv,
    ) -> Result<(), SemaError> {
        let anchor_entity = match &q.from {
            From::Table { entity, .. } => entity.clone(),
            _ => {
                return Err(err(
                    "unsupported",
                    span,
                    "`match any`/`none` anchors on an entity source in this slice",
                ))
            }
        };
        let anchor_alias = q.from.alias().to_string();

        let (mut b_q, b_scope) = self.source(src, env)?;
        let b_fields: Vec<SField> = match &b_scope {
            Scope::Entity { entity, alias } => {
                let e = self.schema.entity(entity).expect("resolved");
                e.fields.iter().map(|f| sfield_of(f, alias)).collect()
            }
            Scope::Fields(f) => f.clone(),
            _ => Vec::new(),
        };
        let mut cands: Vec<&SField> = b_fields
            .iter()
            .filter(|f| f.ref_target.as_deref() == Some(anchor_entity.as_str()))
            .collect();
        let key = match cands.len() {
            1 => cands.pop().expect("one"),
            0 => {
                return Err(err(
                    "no_match_path",
                    span,
                    format!("`{}` has no ref to `{anchor_entity}`", src.name.name),
                ))
            }
            _ => {
                let names: Vec<String> = cands
                    .iter()
                    .map(|f| format!("{}.{}", src.name.name, f.name))
                    .collect();
                return Err(err(
                    "ambiguous_match",
                    span,
                    format!(
                        "more than one ref connects — say which: {}",
                        names.join(" · ")
                    ),
                ));
            }
        };

        b_q.filters.push(BCond::Cmp {
            op: ordo_syntax::CmpOp::Eq,
            lhs: key.expr.clone(),
            rhs: BExpr::Col {
                alias: anchor_alias,
                column: "id".into(),
                ty: ScalarTy::Id,
            },
        });
        q.filters.push(BCond::Exists {
            query: Box::new(b_q),
            negated,
        });
        Ok(())
    }

    /// Bind sources[1..] of a multi-source list — the general form.
    ///
    /// Inference (DL ~1578–1638): terminals are the listed sources; edges
    /// are every declared ref in the schema's undirected multigraph, plus
    /// provenance edges from composed/let sources (a ref-typed output
    /// pointing at an entity). The join shape is the MINIMAL CONNECTING
    /// SUBGRAPH over the terminals — fewest edges, bridge entities inserted
    /// by the compiler when the unique minimum routes through an unlisted
    /// entity. Exactly one minimum joins; several is `ambiguous_match`
    /// (enumerating them); none is `no_match_path`. A unique direct edge
    /// wins by construction: it is the smaller subgraph. `match <ref>` and
    /// `match on <col>` narrow the edge set before the search.
    ///
    /// Every edge becomes an outer-preserving LEFT SourceJoin walking away
    /// from the anchor (per-edge preservation, ruling 3 amendment 3). The
    /// fan-out guard rejects a node with two many-side branches (DL ~1706:
    /// branches, not hops) as `fan_out`.
    fn bind_multi_source(
        &mut self,
        q: &mut Query,
        pl: &Pipeline,
        anchor_scope: Scope,
        ref_sel: &[String],
        on_sel: &[String],
        env: &ParamEnv,
    ) -> Result<Scope, SemaError> {
        // Entity-once over LISTED ENTITY sources (R-C8: composed sources
        // are let-like and may share base entities).
        {
            let mut seen: Vec<&str> = Vec::new();
            for src in &pl.sources {
                if self.schema.entity(&src.name.name).is_some() {
                    if seen.contains(&src.name.name.as_str()) {
                        return Err(err(
                            "unsupported",
                            src.name.span,
                            format!(
                                "`{}` is listed twice — an entity appears once per pipeline",
                                src.name.name
                            ),
                        ));
                    }
                    seen.push(&src.name.name);
                }
            }
        }

        // Bind every non-anchor source.
        let mut bound: Vec<(Query, Scope)> = Vec::new();
        for src in &pl.sources[1..] {
            bound.push(self.source(src, env)?);
        }

        // ── node & edge model ────────────────────────────────────────────
        // Node: Terminal(i) = pl.sources[i]; Bridge(entity).
        #[derive(Clone, PartialEq, Debug)]
        enum Node {
            Term(usize),
            Bridge(String),
        }
        // Snapshot of every terminal's scope for graph building — the
        // rebase of subquery terminals later mutates `bound`, so the graph
        // works from clones.
        let term_scopes: Vec<Scope> = std::iter::once(anchor_scope.clone())
            .chain(bound.iter().map(|(_, sc)| sc.clone()))
            .collect();
        let term_entity = |k: usize| -> Option<String> {
            match &term_scopes[k] {
                Scope::Entity { entity, .. } => Some(entity.clone()),
                _ => None,
            }
        };
        let node_of_entity = |e: &str| -> Node {
            for k in 0..pl.sources.len() {
                if term_entity(k).as_deref() == Some(e) {
                    return Node::Term(k);
                }
            }
            Node::Bridge(e.to_string())
        };

        // Edge: (a, b, desc, spec). spec drives the join build.
        #[derive(Clone, Debug)]
        enum ESpec {
            /// A declared ref `on_entity.field -> to_entity`.
            Ref { on_entity: String, field: String },
            /// A composed terminal's ref-typed output `key` -> entity.
            Prov { term: usize, key: String },
        }
        // A let's output that holds one row per value joins back one-to-one
        // (reported fix 25at): `let n = vendors[id] invoices[] group
        // vendors.id { … }` has at most one row per vendor.
        let unique_of_term: Vec<Vec<String>> = pl
            .sources
            .iter()
            .map(|src| {
                self.lets
                    .iter()
                    .find(|l| l.name == src.name.name)
                    .map(|l| l.unique_keys.clone())
                    .unwrap_or_default()
            })
            .collect();
        let mut edges: Vec<(Node, Node, String, ESpec)> = Vec::new();
        for re in self.schema.ref_edges() {
            let a = node_of_entity(&re.from);
            let b = node_of_entity(&re.to);
            edges.push((
                a,
                b,
                format!("{}.{} -> {}", re.from, re.field, re.to),
                ESpec::Ref {
                    on_entity: re.from.clone(),
                    field: re.field.clone(),
                },
            ));
        }
        for (k, ts) in term_scopes.iter().enumerate() {
            if let Scope::Fields(fs) = ts {
                for f in fs {
                    if let Some(t) = &f.ref_target {
                        let b = node_of_entity(t);
                        edges.push((
                            Node::Term(k),
                            b,
                            format!("{}.{} -> {t}", pl.sources[k].name.name, f.name),
                            ESpec::Prov {
                                term: k,
                                key: f.name.clone(),
                            },
                        ));
                    }
                }
            }
        }

        // Selections narrow parallel edges between the same node pair.
        if !ref_sel.is_empty() || !on_sel.is_empty() {
            let selected: Vec<usize> = edges
                .iter()
                .enumerate()
                .filter(|(_, (_, _, d, sp))| {
                    ref_sel
                        .iter()
                        .any(|sel| d.starts_with(&format!("{sel} ->")))
                        || matches!(sp, ESpec::Prov { key, .. }
                            if on_sel.iter().any(|c| c == key))
                })
                .map(|(i, _)| i)
                .collect();
            // Remove UNselected edges only between pairs a selection covers.
            let pairs: Vec<(Node, Node)> = selected
                .iter()
                .map(|&i| (edges[i].0.clone(), edges[i].1.clone()))
                .collect();
            let mut keep = Vec::new();
            for (i, e) in edges.iter().enumerate() {
                let covered = pairs
                    .iter()
                    .any(|(a, b)| (e.0 == *a && e.1 == *b) || (e.0 == *b && e.1 == *a));
                if !covered || selected.contains(&i) {
                    keep.push(e.clone());
                }
            }
            edges = keep;
        }

        // No minimal connecting subgraph holds a self-ref (it joins nothing
        // new) or an edge to an unlisted entity that touches only one other
        // node (dropping it leaves the rest connected). Pruning them first
        // keeps the search below to the part of the schema that can matter
        // (reported fix 25au).
        edges.retain(|e| e.0 != e.1);
        loop {
            let leaf = edges.iter().find_map(|e| {
                [&e.0, &e.1].into_iter().find_map(|n| {
                    if !matches!(n, Node::Bridge(_)) {
                        return None;
                    }
                    let mut nbrs: Vec<&Node> = Vec::new();
                    for x in &edges {
                        let other = if x.0 == *n {
                            &x.1
                        } else if x.1 == *n {
                            &x.0
                        } else {
                            continue;
                        };
                        if !nbrs.contains(&other) {
                            nbrs.push(other);
                        }
                    }
                    (nbrs.len() <= 1).then(|| n.clone())
                })
            });
            match leaf {
                Some(n) => edges.retain(|e| e.0 != n && e.1 != n),
                None => break,
            }
        }

        // ── minimal connecting subgraph ──────────────────────────────────
        let n_terms = pl.sources.len();
        let connects = |chosen: &[usize]| -> bool {
            // Union-find over the nodes the chosen edges touch.
            let mut nodes: Vec<Node> = Vec::new();
            let id = |n: &Node, nodes: &mut Vec<Node>| -> usize {
                if let Some(i) = nodes.iter().position(|x| x == n) {
                    i
                } else {
                    nodes.push(n.clone());
                    nodes.len() - 1
                }
            };
            let mut parent: Vec<usize> = Vec::new();
            fn find(p: &mut Vec<usize>, x: usize) -> usize {
                if p[x] != x {
                    let r = find(p, p[x]);
                    p[x] = r;
                }
                p[x]
            }
            for &ei in chosen {
                let a = id(&edges[ei].0, &mut nodes);
                let b = id(&edges[ei].1, &mut nodes);
                while parent.len() < nodes.len() {
                    parent.push(parent.len());
                }
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                parent[ra] = rb;
            }
            // Every terminal present and in one component.
            let mut root = None;
            for k in 0..n_terms {
                match nodes.iter().position(|x| *x == Node::Term(k)) {
                    None => return false,
                    Some(i) => {
                        let r = find(&mut parent, i);
                        match root {
                            None => root = Some(r),
                            Some(r0) if r0 != r => return false,
                            _ => {}
                        }
                    }
                }
            }
            true
        };

        // Every edge at once is the most any subset can connect. When even
        // that leaves a source apart, no subset joins them, and the search
        // below would walk all 2^edges subsets to learn it: about 75 s on
        // the estate schema in a debug build (reported fix 25au). Name the
        // sources that cannot be reached instead.
        let all_edges: Vec<usize> = (0..edges.len()).collect();
        if !connects(&all_edges) {
            let mut adj: Vec<Node> = vec![Node::Term(0)];
            loop {
                let before = adj.len();
                for e in &edges {
                    if adj.contains(&e.0) && !adj.contains(&e.1) {
                        adj.push(e.1.clone());
                    } else if adj.contains(&e.1) && !adj.contains(&e.0) {
                        adj.push(e.0.clone());
                    }
                }
                if adj.len() == before {
                    break;
                }
            }
            let apart: Vec<String> = (1..n_terms)
                .filter(|k| !adj.contains(&Node::Term(*k)))
                .map(|k| format!("`{}`", pl.sources[k].name.name))
                .collect();
            return Err(err(
                "no_match_path",
                pl.span,
                format!(
                    "no combination of declared refs connects `{}` to {}",
                    pl.sources[0].name.name,
                    apart.join(", ")
                ),
            ));
        }

        // The search tries every subset of each size until one connects, so
        // its cost is the sum of binomials up to the minimum size. A schema
        // whose minimum join is long enough to make that slow is refused
        // rather than left to run for a minute (reported fix 25au).
        const SEARCH_BUDGET: u64 = 2_000_000;
        let mut tried: u64 = 0;
        let mut minima: Vec<Vec<usize>> = Vec::new();
        'sizes: for size in (n_terms - 1)..=edges.len() {
            // All combinations of `size` edges.
            let mut combo: Vec<usize> = (0..size).collect();
            loop {
                tried += 1;
                if tried > SEARCH_BUDGET {
                    return Err(err(
                        "unsupported",
                        pl.span,
                        "the shortest join between these sources is too long for match \
                         inference to search; name the hops with `match <ref>`",
                    ));
                }
                if connects(&combo) {
                    minima.push(combo.clone());
                }
                // next combination
                let mut i = size;
                loop {
                    if i == 0 {
                        break;
                    }
                    i -= 1;
                    if combo[i] != i + edges.len() - size {
                        combo[i] += 1;
                        for j2 in (i + 1)..size {
                            combo[j2] = combo[j2 - 1] + 1;
                        }
                        break;
                    }
                    if i == 0 {
                        // exhausted
                        if !minima.is_empty() {
                            break 'sizes;
                        }
                        continue 'sizes;
                    }
                }
                if combo.last().copied().unwrap_or(0) >= edges.len() {
                    break;
                }
            }
        }

        let chosen: Vec<usize> = match minima.len() {
            1 => minima.pop().expect("one"),
            0 => {
                return Err(err(
                    "no_match_path",
                    pl.span,
                    "no combination of declared refs connects these sources",
                ))
            }
            _ => {
                let alts: Vec<String> = minima
                    .iter()
                    .take(4)
                    .map(|m| {
                        m.iter()
                            .map(|&i| edges[i].2.clone())
                            .collect::<Vec<_>>()
                            .join(" + ")
                    })
                    .collect();
                return Err(err(
                    "ambiguous_match",
                    pl.span,
                    format!(
                        "more than one minimal join connects these sources — say which with `match <ref>`: {}",
                        alts.join("  ·  ")
                    ),
                ));
            }
        };

        // ── build the join tree, walking away from the anchor ────────────
        // Alias per node. The anchor keeps its alias; terminals get their
        // bound queries; bridges get fresh table aliases.
        let anchor_alias = q.from.alias().to_string();
        let mut alias_of: Vec<(Node, String)> = vec![(Node::Term(0), anchor_alias)];
        let mut visited: Vec<Node> = vec![Node::Term(0)];
        let mut remaining: Vec<usize> = chosen.clone();
        let mut many_children: Vec<(Node, usize)> = Vec::new(); // fan-out count

        // For terminals: materialize as join target once reached.
        while !remaining.is_empty() {
            let mut progressed = false;
            let mut rem2 = Vec::new();
            for &ei in &remaining {
                let (a, b, _, spec) = edges[ei].clone();
                let (near, far) = if visited.contains(&a) && !visited.contains(&b) {
                    (a, b)
                } else if visited.contains(&b) && !visited.contains(&a) {
                    (b, a)
                } else if visited.contains(&a) && visited.contains(&b) {
                    // Cycle in the chosen set — cannot happen at minimum size.
                    continue;
                } else {
                    rem2.push(ei);
                    continue;
                };
                progressed = true;

                let near_alias = alias_of
                    .iter()
                    .find(|(n, _)| *n == near)
                    .expect("visited has alias")
                    .1
                    .clone();

                // Resolve the ON columns and which side is many.
                // near_col/far_col are column names on each side.
                let mut one_to_one = false;
                let (near_col, far_col, far_is_many) = match &spec {
                    ESpec::Ref { on_entity, field } => {
                        let near_entity = match &near {
                            Node::Bridge(e) => Some(e.clone()),
                            Node::Term(k) => term_entity(*k),
                        };
                        if near_entity.as_deref() == Some(on_entity.as_str()) {
                            // ref lives on the near side → far is the ONE side
                            (field.clone(), "id".to_string(), false)
                        } else {
                            // ref lives on the far side → far is the MANY side
                            ("id".to_string(), field.clone(), true)
                        }
                    }
                    ESpec::Prov { term, key } => {
                        one_to_one = unique_of_term[*term].contains(key);
                        if matches!(&far, Node::Term(k) if k == term) {
                            ("id".to_string(), key.clone(), !one_to_one)
                        } else {
                            (key.clone(), "id".to_string(), false)
                        }
                    }
                };

                // Materialize the far node as a join target.
                let far_alias;
                // Only a bridge needs one. A listed term was bound by
                // `entity_source`, so its guard is already in its own
                // filters, and a term with filters is lowered below as a
                // sub-select that carries them.
                let mut bridge_guard: Option<BCond> = None;
                let far_from: From = match &far {
                    Node::Bridge(e) => {
                        far_alias = self.alias();
                        bridge_guard = self.joined_bound(e, &far_alias, env)?;
                        From::Table {
                            entity: e.clone(),
                            alias: far_alias.clone(),
                        }
                    }
                    Node::Term(k) => {
                        let (b_q, b_scope) = bound[k - 1].clone();
                        let plain = matches!(&b_scope, Scope::Entity { .. })
                            && b_q.filters.is_empty()
                            && b_q.joins.is_empty();
                        if plain {
                            let Scope::Entity { entity, alias } = b_scope else {
                                unreachable!()
                            };
                            far_alias = alias.clone();
                            From::Table { entity, alias }
                        } else {
                            // Subquery target: ensure the join key is output.
                            let mut bq = b_q;
                            // A filtered or guarded entity term is a
                            // sub-select, and the list's projection reads
                            // through it, so it carries the term's fields
                            // and not only its join key (reported fix 25s:
                            // `customers[name] orders(status == "done")
                            // [placed]` found only `customer`). A sealed
                            // field goes in only when this term's
                            // projection names it, and with its obligation
                            // in the sub-select's `host_open`, exactly as
                            // fix 25r carries it for the plain join: the
                            // sub-select packs the envelope with its row id
                            // and the outer level passes it on.
                            //
                            // The entity's implicit fields (`id`, `rev`) ride
                            // too, so a step may name `orders.rev` as it may
                            // on the plain join. The rebased scope remembers
                            // which they are, and `*` leaves them out there
                            // as it does on the entity: without that,
                            // `orders(status == "done")[*]` answered `id`
                            // and `rev` where `orders[*]` did not.
                            let mut implicit_names: Vec<String> = Vec::new();
                            if let Scope::Entity { entity, alias } = &b_scope {
                                if bq.outputs.is_empty() {
                                    let wanted = projection_heads(&pl.sources[*k]);
                                    let e = self.schema.entity(entity).expect("resolved");
                                    let fields: Vec<SField> = e
                                        .fields
                                        .iter()
                                        .filter(|f| !f.sealed || wanted.contains(&f.name))
                                        .map(|f| sfield_of_entity(f, entity, alias))
                                        .collect();
                                    implicit_names = fields
                                        .iter()
                                        .filter(|f| f.implicit)
                                        .map(|f| f.name.clone())
                                        .collect();
                                    bq.outputs = fields
                                        .iter()
                                        .map(|f| OutField {
                                            name: f.name.clone(),
                                            expr: f.expr.clone(),
                                        })
                                        .collect();
                                    bq.host_open = self.host_open_of(&fields);
                                }
                            }
                            if !bq.outputs.iter().any(|o| o.name == far_col) {
                                let fields: Vec<SField> = match &b_scope {
                                    Scope::Entity { entity, alias } => {
                                        let e = self.schema.entity(entity).expect("resolved");
                                        e.fields.iter().map(|f| sfield_of(f, alias)).collect()
                                    }
                                    Scope::Fields(f) => f.clone(),
                                    _ => Vec::new(),
                                };
                                let kf = fields
                                    .iter()
                                    .find(|f| f.name == far_col)
                                    .cloned()
                                    .ok_or_else(|| {
                                        err(
                                            "no_match_path",
                                            pl.sources[*k].name.span,
                                            format!(
                                                "`{}` does not expose join key `{far_col}`",
                                                pl.sources[*k].name.name
                                            ),
                                        )
                                    })?;
                                bq.outputs.push(OutField {
                                    name: far_col.clone(),
                                    expr: kf.expr,
                                });
                            }
                            far_alias = self.alias();
                            // Rebase the terminal's scope onto the sub alias.
                            let fields = self.scope_fields(&bq.outputs, &bq)?;
                            let rebased: Vec<SField> = fields
                                .into_iter()
                                .map(|f| SField {
                                    expr: BExpr::Col {
                                        alias: far_alias.clone(),
                                        column: f.name.clone(),
                                        ty: f.expr.ty(),
                                    },
                                    implicit: implicit_names.contains(&f.name),
                                    ..f
                                })
                                .collect();
                            bound[k - 1].1 = Scope::Fields(rebased);
                            From::Sub {
                                query: Box::new(bq),
                                alias: far_alias.clone(),
                            }
                        }
                    }
                };

                let on = vec![(
                    BExpr::Col {
                        alias: far_alias.clone(),
                        column: far_col,
                        ty: ScalarTy::Id,
                    },
                    BExpr::Col {
                        alias: near_alias,
                        column: near_col,
                        ty: ScalarTy::Id,
                    },
                )];
                q.source_joins.push(SourceJoin {
                    from: far_from,
                    on,
                    many_side: far_is_many,
                    guard: bridge_guard,
                });

                // Fan-out accounting: every many-edge counts at its ONE-
                // side node, regardless of walk direction — the anchor
                // being the many side of a bridge is a branch too (C083's
                // M:N: orders and doneOrders both fan out from customers).
                // A one-to-one edge (a let joined back on a key it holds
                // once) is no branch at all.
                if !one_to_one {
                    let one_side = if far_is_many {
                        near.clone()
                    } else {
                        far.clone()
                    };
                    let slot = many_children.iter_mut().find(|(n, _)| *n == one_side);
                    match slot {
                        Some((_, c)) => *c += 1,
                        None => many_children.push((one_side, 1)),
                    }
                }

                alias_of.push((far.clone(), far_alias));
                visited.push(far);
            }
            remaining = rem2;
            if !progressed && !remaining.is_empty() {
                return Err(err(
                    "no_match_path",
                    pl.span,
                    "the chosen join subgraph does not reach the anchor",
                ));
            }
        }

        // Fan-out guard: two many-branches from one node (DL ~1706 —
        // branches, not hops).
        if let Some((_, c)) = many_children.iter().find(|(_, c)| *c > 1) {
            let _ = c;
            return Err(err(
                "fan_out",
                pl.span,
                "two independent many-branches in one pipeline — restructure with `let` (group each side first)",
            ));
        }

        // ── projections & scope ──────────────────────────────────────────
        let mut outputs: Vec<SField> = Vec::new();
        for (k, src) in pl.sources.iter().enumerate() {
            if src.projection.is_none() {
                continue;
            }
            let sc = if k == 0 {
                anchor_scope.clone()
            } else {
                bound[k - 1].1.clone()
            };
            let fields = self.project_fields(q, src, &sc, env)?;
            // One output name, one column. Two sources projecting the same
            // name (`a[note] b[note]`) would leave every later reader to
            // pick one by name, and they did not agree: the open, the
            // conformance decoder and the artifact took the first, the
            // canonical row the last (ruled 2026-10-05). A rename
            // (`b[n2: note]`) keeps both.
            for f in &fields {
                if outputs.iter().any(|o| o.name == f.name) {
                    return Err(err(
                        "projection_unknown_field",
                        src.span,
                        format!(
                            "two sources project `{}`; rename one (`{}[other: {}]`)",
                            f.name, src.name.name, f.name
                        ),
                    ));
                }
            }
            outputs.extend(fields);
        }
        q.outputs = outputs
            .iter()
            .map(|f| OutField {
                name: f.name.clone(),
                expr: f.expr.clone(),
            })
            .collect();
        // The obligation to open a sealed value is the same whichever
        // source of the list projected it. `project` sets this for a single
        // source; a list that skipped it served `orders.memo` reached
        // through `customers[name] orders[placed, memo]` as its stored
        // envelope, classed sql, with no key release in the manifest
        // (reported fix 25r). Everything downstream reads this list: the
        // planner's anchoring, the executor's open, the cost class, the
        // artifact's sealed outputs and its release set.
        q.host_open = self.host_open_of(&outputs);

        let multi = Scope::Multi(
            pl.sources
                .iter()
                .enumerate()
                .map(|(k, src)| {
                    let sc = if k == 0 {
                        anchor_scope.clone()
                    } else {
                        bound[k - 1].1.clone()
                    };
                    (src.name.name.clone(), sc)
                })
                .collect(),
        );
        Ok(Scope::Layered(vec![Scope::Fields(outputs), multi]))
    }

    /// `take[keys] n` — per-bucket windows (DL ~1890). The ranking is the
    /// pipeline's current order: declared sorts, then the tiebreak, so an
    /// unsorted pipeline ranks by id (R-C4, C063). Lowered as a hidden
    /// ROW_NUMBER column inside the existing wrap machinery; the outer
    /// query keeps rows with $rn <= n and reproduces the pipeline's order.
    /// A `take`'s count as the plan carries it (`ir::Limit`).
    ///
    /// A parameter that the caller has already supplied a value for folds
    /// to the literal it is — the harness and the executor both arrive
    /// that way — so `take $n` and `take 20` produce identical plans for a
    /// call with `$n = 20`. Left as a slot, the parameter's declared bound
    /// rides along: that number, and only that number, is what makes the
    /// op's row bound provable at registration.
    fn take_limit(
        &self,
        n: &ordo_syntax::TakeCount,
        env: &ParamEnv,
    ) -> Result<ir::Limit, SemaError> {
        let pname = match n {
            ordo_syntax::TakeCount::Lit { n, .. } => return Ok(ir::Limit::Lit(*n)),
            ordo_syntax::TakeCount::Param { name } => name,
        };
        match env.get(&pname.name) {
            Some(BExpr::Lit {
                value: Expr::IntLit { value, .. },
                ..
            }) => i64::try_from(*value)
                .map(ir::Limit::Lit)
                .map_err(|_| err("bound_violation", pname.span, "take count out of range")),
            Some(BExpr::Lit { value, .. }) => Err(err(
                "param_type_mismatch",
                pname.span,
                format!(
                    "`take ${}` needs an `int`; this call bound {value:?}",
                    pname.name
                ),
            )),
            // An unsupplied parameter stays a slot. `BExpr::Param`'s name
            // is the OUTERMOST op's parameter (substitution is transitive
            // through the call envs), which is where the bound lives.
            Some(BExpr::Param { name, .. }) => Ok(ir::Limit::Param {
                max: self.param_max(name),
                name: name.clone(),
            }),
            Some(other) => Err(err(
                "invalid_binding_expr",
                pname.span,
                format!("`take ${}` was bound to {other:?}", pname.name),
            )),
            // Same answer every other parameter position gives: a name
            // nothing bound is not a slot, it is a mistake.
            None => Err(err(
                "unbound_required_param",
                pname.span,
                format!("`${}` is not bound at this call site", pname.name),
            )),
        }
    }

    /// The declared `<=` bound of a parameter of the op this binding
    /// started from. `None` when there is no op (a bare pipeline) or the
    /// parameter carries no bound — and `None` costs the plan its row
    /// bound, which is the registration gate's business, not a silent
    /// promotion here.
    fn param_max(&self, pname: &str) -> Option<i64> {
        let op = self.registry.get(self.stack.first()?)?;
        match op
            .params
            .iter()
            .find(|p| p.name.name == pname)?
            .max
            .as_ref()
        {
            Some(Expr::IntLit { value, .. }) => i64::try_from(*value).ok(),
            _ => None,
        }
    }

    fn take_keys(
        &mut self,
        mut q: Query,
        scope: &Scope,
        keys: &[Expr],
        n: &ordo_syntax::TakeCount,
        span: Span,
        env: &ParamEnv,
    ) -> Result<Query, SemaError> {
        let mut partition = Vec::new();
        for k in keys {
            partition.push(self.expr(k, scope, &mut q, env)?);
        }
        let mut order: Vec<Sort> = q.sort.clone();
        for tb in &q.tiebreak {
            order.push(Sort {
                expr: tb.clone(),
                desc: false,
            });
        }
        if order.is_empty() {
            return Err(err(
                "unsupported",
                span,
                "take[keys] needs an order to rank by and this query has none",
            ));
        }
        q.outputs.push(OutField {
            name: "$rn".into(),
            expr: BExpr::RowNumber {
                partition,
                order,
                ty: ScalarTy::Int,
            },
        });
        let (mut outer, _) = self.wrap(q);
        let alias = outer.from.alias().to_string();
        outer.filters.push(BCond::Cmp {
            op: ordo_syntax::CmpOp::Le,
            lhs: BExpr::Col {
                alias,
                column: "$rn".into(),
                ty: ScalarTy::Int,
            },
            // `take[keys] $n` binds the same slot the plain form does: the
            // bucket limit is a `row_number() <= …` filter rather than a
            // LIMIT, but the count is the count.
            rhs: match self.take_limit(n, env)? {
                ir::Limit::Lit(v) => BExpr::Lit {
                    value: Expr::IntLit {
                        value: v as i128,
                        span,
                    },
                    ty: ScalarTy::Int,
                },
                ir::Limit::Param { name, .. } => BExpr::Param {
                    name,
                    ty: ScalarTy::Int,
                },
            },
        });
        outer.outputs.retain(|o| o.name != "$rn");
        Ok(outer)
    }

    /// Wrap a settled callee in an outer query. Sort keys and the anchor are
    /// re-exposed as hidden columns so the outer query can reproduce the
    /// callee's order — R-C4 keeps the callee's order when the caller
    /// declares none (C004, C009).
    fn wrap(&mut self, mut callee: Query) -> (Query, Scope) {
        let alias = self.alias();

        let mut hidden: Vec<(String, BExpr)> = Vec::new();
        let mut outer_sort = Vec::new();
        for (i, s) in callee.sort.clone().iter().enumerate() {
            let hname = format!("$s{i}");
            hidden.push((hname.clone(), s.expr.clone()));
            outer_sort.push(Sort {
                expr: BExpr::Col {
                    alias: alias.clone(),
                    column: hname,
                    ty: s.expr.ty(),
                },
                desc: s.desc,
            });
        }
        // The callee's tiebreak must survive the wrap. Without this the outer
        // query orders by the caller's keys alone and two rows agreeing on
        // them come back in whatever order the server felt like.
        let mut outer_tiebreak = Vec::new();
        for (i, tb) in callee.tiebreak.clone().iter().enumerate() {
            let hname = format!("$tb{i}");
            hidden.push((hname.clone(), tb.clone()));
            outer_tiebreak.push(BExpr::Col {
                alias: alias.clone(),
                column: hname,
                ty: tb.ty(),
            });
        }

        let visible: Vec<OutField> = callee
            .outputs
            .iter()
            .map(|o| OutField {
                name: o.name.clone(),
                expr: BExpr::Col {
                    alias: alias.clone(),
                    column: o.name.clone(),
                    ty: o.expr.ty(),
                },
            })
            .collect();

        let ref_targets: Vec<Option<String>> = callee
            .outputs
            .iter()
            .map(|o| self.ref_target_of(&o.expr, &callee))
            .collect();

        // Sealed outputs stay sealed through the wrap, by name.
        let inherited: Vec<(String, String, String)> = callee
            .host_open
            .iter()
            .filter(|(n, _, _)| visible.iter().any(|o| &o.name == n))
            .cloned()
            .collect();

        // A sealed INDEXED output that is the stored column itself keeps its
        // blind index through the wrap: the sub-select carries the digest
        // column as a hidden output, and the outer query compares against it
        // under the wrap's alias. Without it, the door's `$refine` equality
        // on an op settled by a `take` was refused at bind and answered 500
        // (reported fix 25as). Not under a `group`: a hidden column there
        // would have to be a group key.
        let mut blinds: Vec<Option<Blind>> = vec![None; visible.len()];
        if callee.group.is_none() {
            for (i, o) in callee.outputs.iter().enumerate().take(visible.len()) {
                let Some((_, entity, field)) = inherited.iter().find(|(n, _, _)| n == &o.name)
                else {
                    continue;
                };
                let Some((digest, normalize)) = self.digest_of(entity, field, &o.expr, &callee)
                else {
                    continue;
                };
                let hname = format!("$bidx{i}");
                hidden.push((hname.clone(), digest));
                blinds[i] = Some((
                    entity.clone(),
                    field.clone(),
                    normalize,
                    hname,
                    alias.clone(),
                ));
            }
        }

        for (n, e) in hidden {
            if !callee.outputs.iter().any(|o| o.name == n) {
                callee.outputs.push(OutField { name: n, expr: e });
            }
        }

        let fields: Vec<SField> = visible
            .iter()
            .zip(ref_targets)
            .zip(blinds)
            .map(|((o, rt), blind)| {
                let seal = inherited
                    .iter()
                    .find(|(n, _, _)| n == &o.name)
                    .map(|(_, e, f)| (e.clone(), f.clone()));
                SField {
                    name: o.name.clone(),
                    expr: o.expr.clone(),
                    ref_target: rt,
                    sealed: seal.is_some(),
                    seal_of: seal,
                    blind,
                    implicit: false,
                }
            })
            .collect();

        let q = Query {
            not_null: None,
            from: From::Sub {
                query: Box::new(callee),
                alias,
            },
            joins: Vec::new(),
            filters: Vec::new(),
            having: Vec::new(),
            ctes: Vec::new(),
            source_joins: Vec::new(),
            group: None,
            outputs: visible,
            sort: outer_sort,
            sort_inherited: true,
            host_open: inherited,
            limit: None,
            distinct: false,
            tiebreak: outer_tiebreak,
        };
        (q, Scope::Fields(fields))
    }

    /// The digest column behind a sealed indexed output, when the output is
    /// that field's stored column read straight from its table (the query's
    /// own table or a join of it), with the field's normalization.
    fn digest_of(
        &self,
        entity: &str,
        field: &str,
        e: &BExpr,
        q: &Query,
    ) -> Option<(BExpr, Option<String>)> {
        let BExpr::Col { alias, column, .. } = e else {
            return None;
        };
        if column != field {
            return None;
        }
        let table = match &q.from {
            From::Table { entity, alias: a } if a == alias => entity.as_str(),
            _ => q
                .joins
                .iter()
                .find(|j| &j.alias == alias)
                .map(|j| j.table.as_str())?,
        };
        if table != entity {
            return None;
        }
        let decl = self.schema.entity(entity)?.field(field)?;
        if !(decl.sealed && decl.indexed) {
            return None;
        }
        Some((
            BExpr::Col {
                alias: alias.clone(),
                column: decl.index_column(),
                ty: ScalarTy::Text,
            },
            decl.normalize.clone(),
        ))
    }

    /// The entity whose `id` a bound expression reads, if it reads one.
    /// Follows a sub-select's output back to its table, so a filtered term
    /// lowered as a sub-select still names its entity.
    fn id_entity_of(&self, e: &BExpr, q: &Query) -> Option<String> {
        let BExpr::Col { alias, column, .. } = e else {
            return None;
        };
        let from_of = |f: &From| -> Option<String> {
            match f {
                From::Table { entity, alias: a } if a == alias && column == "id" => {
                    Some(entity.clone())
                }
                From::Sub { query, alias: a } if a == alias => {
                    let inner = query.outputs.iter().find(|o| &o.name == column)?;
                    self.id_entity_of(&inner.expr, query)
                }
                _ => None,
            }
        };
        if let Some(e) = from_of(&q.from) {
            return Some(e);
        }
        for sj in &q.source_joins {
            if let Some(e) = from_of(&sj.from) {
                return Some(e);
            }
        }
        q.joins
            .iter()
            .find(|j| &j.alias == alias && column == "id")
            .map(|j| j.table.clone())
    }

    /// The group-key outputs of a let's query that point at an entity:
    /// `(output, entity, unique)`. An entity's `id` key points at that
    /// entity; a ref key at the ref's target. `unique` says the let holds at
    /// most one row per value of that output: an `id` key whose fellow keys
    /// all read the same row (`group vendors.id, vendors.name`), or a lone
    /// ref key. A settled wrap of a grouped query (a `take`, a bucket
    /// filter) keeps both facts, since it only drops rows.
    fn group_key_provenance(&self, q: &Query) -> Vec<(String, String, bool)> {
        if let Some(g) = &q.group {
            if g.keys.is_empty() || q.distinct {
                return Vec::new();
            }
            let alias_of = |e: &BExpr| match e {
                BExpr::Col { alias, .. } => Some(alias.clone()),
                _ => None,
            };
            let mut out = Vec::new();
            for o in &q.outputs {
                if !g.keys.contains(&o.expr) {
                    continue;
                }
                if let Some(ent) = self.id_entity_of(&o.expr, q) {
                    let a = alias_of(&o.expr);
                    let unique = g.keys.iter().all(|k| alias_of(k) == a);
                    out.push((o.name.clone(), ent, unique));
                } else if let Some(t) = self.ref_target_of(&o.expr, q) {
                    out.push((o.name.clone(), t, g.keys.len() == 1));
                }
            }
            return out;
        }
        let From::Sub { query, alias } = &q.from else {
            return Vec::new();
        };
        if q.distinct || !q.joins.is_empty() || !q.source_joins.is_empty() {
            return Vec::new();
        }
        let inner = self.group_key_provenance(query);
        q.outputs
            .iter()
            .filter_map(|o| match &o.expr {
                BExpr::Col {
                    alias: a, column, ..
                } if a == alias => inner
                    .iter()
                    .find(|(n, _, _)| n == column)
                    .map(|(_, e, u)| (o.name.clone(), e.clone(), *u)),
                _ => None,
            })
            .collect()
    }

    /// Which entity, if any, a bound output expression refs — needed so a
    /// caller can dot through a callee's ref output (C005).
    fn ref_target_of(&self, e: &BExpr, q: &Query) -> Option<String> {
        let BExpr::Col { alias, column, .. } = e else {
            return None;
        };
        match &q.from {
            From::Table { entity, alias: a } if a == alias => {
                let ent = self.schema.entity(entity)?;
                match &ent.field(column)?.ty {
                    FieldTy::Ref { target, .. } => Some(target.clone()),
                    _ => None,
                }
            }
            From::Sub { query, alias: a } if a == alias => {
                let inner = query.outputs.iter().find(|o| &o.name == column)?;
                self.ref_target_of(&inner.expr, query)
            }
            _ => {
                // A join alias: find which join, then the target entity.
                let j = q.joins.iter().find(|j| &j.alias == alias)?;
                let ent = self.schema.entity(&j.table)?;
                match &ent.field(column)?.ty {
                    FieldTy::Ref { target, .. } => Some(target.clone()),
                    _ => None,
                }
            }
        }
    }

    /// A merged callee's sealed outputs, with the blind index armed where
    /// the output is the indexed column itself.
    ///
    /// Composition merges an unsettled callee into the caller's query, so
    /// the callee's aliases are the caller's and its digest column is
    /// reachable under the same alias. A caller's `==` on such an output
    /// then lowers to the blind index exactly as it does on the entity
    /// (`try_blind_eq`). Without this a composed sealed-indexed field was
    /// refused as unreadable, and so was the door's `$refine` equality on
    /// it, which is a composition (ruled 2026-10-05, `refinement-default`
    /// ask 1). A wrapped (settled) callee is armed by `wrap` instead, which
    /// carries the digest column out of the sub-select (25as).
    fn arm_blind(&self, fields: Vec<SField>) -> Vec<SField> {
        fields
            .into_iter()
            .map(|mut f| {
                let (Some((entity, field)), BExpr::Col { alias, column, .. }) =
                    (&f.seal_of, &f.expr)
                else {
                    return f;
                };
                if column != field {
                    return f;
                }
                let Some(decl) = self.schema.entity(entity).and_then(|e| e.field(field)) else {
                    return f;
                };
                if decl.sealed && decl.indexed {
                    f.blind = Some((
                        entity.clone(),
                        field.clone(),
                        decl.normalize.clone(),
                        decl.index_column(),
                        alias.clone(),
                    ));
                }
                f
            })
            .collect()
    }

    fn scope_fields(&self, outs: &[OutField], q: &Query) -> Result<Vec<SField>, SemaError> {
        Ok(outs
            .iter()
            .map(|o| {
                // A sealed value stays sealed across composition: the
                // caller sees a name, not a decrypted column, and the
                // obligation to open it travels with the name. Losing it
                // here would serve ciphertext as if it were text.
                let seal = q
                    .host_open
                    .iter()
                    .find(|(n, _, _)| n == &o.name)
                    .map(|(_, e, f)| (e.clone(), f.clone()));
                SField {
                    name: o.name.clone(),
                    expr: o.expr.clone(),
                    ref_target: self.ref_target_of(&o.expr, q),
                    sealed: seal.is_some(),
                    seal_of: seal,
                    blind: None,
                    implicit: false,
                }
            })
            .collect())
    }

    // ── parameters ───────────────────────────────────────────────────────

    fn param_env(
        &mut self,
        op: &OpDecl,
        args: &[SourceArg],
        span: Span,
    ) -> Result<ParamEnv, SemaError> {
        self.param_env_with(op, args, &ParamEnv::new(), span)
    }

    fn param_env_with(
        &mut self,
        op: &OpDecl,
        args: &[SourceArg],
        caller_env: &ParamEnv,
        span: Span,
    ) -> Result<ParamEnv, SemaError> {
        let mut supplied: HashMap<String, &Expr> = HashMap::new();
        for a in args {
            if let SourceArg::Named { name, value, .. } = a {
                if !op.params.iter().any(|p| p.name.name == name.name) {
                    return Err(err(
                        "unknown_param",
                        name.span,
                        format!("`{}` has no parameter `${}`", op.name.name, name.name),
                    ));
                }
                if supplied.insert(name.name.clone(), value).is_some() {
                    return Err(err(
                        "duplicate_binding",
                        name.span,
                        format!("`${}` is bound twice in one call", name.name),
                    ));
                }
            }
        }

        let mut env = ParamEnv::new();
        for p in &op.params {
            // A caller's argument is evaluated in the caller's scope-free
            // environment: parameter values are constants, never row values.
            let bound = if let Some(v) = supplied.get(&p.name.name) {
                self.const_expr(v, caller_env)?
            } else if let Some(d) = &p.default {
                self.const_expr(d, &ParamEnv::new())?
            } else {
                return Err(err(
                    "unbound_required_param",
                    span,
                    format!(
                        "`{}` requires `${}` and no value was supplied",
                        op.name.name, p.name.name
                    ),
                ));
            };
            let bound = fit_slot(bound, p.ty, span)?;
            env.insert(p.name.name.clone(), bound);
        }
        // $caller is the SESSION's binding, not an op parameter — it flows
        // through every composition level untouched (G18: binds post-door,
        // uniformly). Without this, a callee's guard reverts to the
        // prepared slot and the harness's `as:` never reaches it.
        if let Some(c) = caller_env.get("caller") {
            env.insert("caller".into(), c.clone());
        }
        Ok(env)
    }

    /// Bind an expression that may not touch rows — parameter values and
    /// defaults. A path here would be a row reference in a constant position.
    fn const_expr(&mut self, e: &Expr, env: &ParamEnv) -> Result<BExpr, SemaError> {
        match e {
            Expr::Param { name, span } => env.get(name).cloned().ok_or_else(|| {
                err(
                    "unbound_required_param",
                    *span,
                    format!("`${name}` is not bound at this call site"),
                )
            }),
            Expr::Path { segments, span } => {
                // R-C3: a reduce-let scalar is the one row-free path a
                // binding admits (C020); a non-reduce let is C069's error;
                // anything else stays a row reference in a constant.
                if let Some(info) = self.lets.iter().find(|l| l.name == segments[0].name) {
                    if !info.ends_in_reduce {
                        return Err(err(
                            "let_scalar_requires_reduce",
                            *span,
                            format!(
                                "`{}` does not end in `reduce`, so it has rows, not a scalar",
                                segments[0].name
                            ),
                        ));
                    }
                    let col = &segments[1];
                    let (_, ty, _) = info
                        .fields
                        .iter()
                        .find(|(n, _, _)| n == &col.name)
                        .ok_or_else(|| {
                            err(
                                "unknown_field",
                                col.span,
                                format!("`{}` has no output `{}`", segments[0].name, col.name),
                            )
                        })?;
                    return Ok(BExpr::ScalarSub {
                        cte: info.name.clone(),
                        column: col.name.clone(),
                        ty: *ty,
                    });
                }
                Err(err(
                    "invalid_binding_expr",
                    *span,
                    "a parameter value is a constant; it cannot reference a row field",
                ))
            }
            Expr::Binary { op, lhs, rhs, span } => {
                // Binding expressions are row-free; arithmetic over
                // literals and params FOLDS at bind time — `$d + 7d` is an
                // exact date, not SQL interval math (C019).
                let l = self.const_expr(lhs, env)?;
                let r = self.const_expr(rhs, env)?;
                // A value already typed `money(p)` is in minor units, a
                // source literal in major units: bring them to one footing
                // before the digits are added (27ak), and keep the money
                // type on the result so nothing converts it twice.
                let (l, r) = match op {
                    BinOp::Add | BinOp::Sub => unify_money(l, r, *span)?,
                    _ => (l, r),
                };
                let money_ty = [l.ty(), r.ty()]
                    .into_iter()
                    .find(|t| matches!(t, ScalarTy::Money(_)));
                if matches!(op, BinOp::Mul)
                    && matches!((l.ty(), r.ty()), (ScalarTy::Money(_), ScalarTy::Money(_)))
                {
                    return Err(err(
                        "invalid_binding_expr",
                        *span,
                        "a money times a money is not an amount of money",
                    ));
                }
                let lv = match &l {
                    BExpr::Lit { value, .. } => value.clone(),
                    _ => return Err(err("invalid_binding_expr", *span, "not a constant")),
                };
                let rv = match &r {
                    BExpr::Lit { value, .. } => value.clone(),
                    _ => return Err(err("invalid_binding_expr", *span, "not a constant")),
                };
                let folded = match (&lv, op, &rv) {
                    (
                        Expr::DateLit { y, m, d, .. },
                        BinOp::Add | BinOp::Sub,
                        Expr::DurationLit { n, unit, .. },
                    ) => {
                        let fwd = matches!(op, BinOp::Add);
                        let (ny, nm, nd) = shift_date((*y, *m, *d), *n as i64, *unit, fwd)
                            .ok_or_else(|| {
                                err(
                                    "invalid_binding_expr",
                                    *span,
                                    "sub-day duration cannot shift a date exactly",
                                )
                            })?;
                        Expr::DateLit {
                            y: ny,
                            m: nm,
                            d: nd,
                            span: *span,
                        }
                    }
                    (Expr::IntLit { value: a, .. }, _, Expr::IntLit { value: b, .. }) => {
                        let v =
                            match op {
                                BinOp::Add => a + b,
                                BinOp::Sub => a - b,
                                BinOp::Mul => a * b,
                                // The host-side twin of the `bigint / bigint`
                                // bug. Rust's `a / b` on integers truncates, so
                                // folding `$cap: 3000 / 7` here answered 428
                                // while the same expression in SQL answers
                                // 428.571428571429 — an expression whose value
                                // depends on which venue evaluated it, which is
                                // the one thing the division ruling exists to
                                // prevent (DL §2, "venue-independence wins").
                                //
                                // There is no folding this correctly either: a
                                // parameter is a typed slot, `/` produces a
                                // scale-12 decimal, and a decimal does not land
                                // in a typed slot without an explicit
                                // conversion. So the honest answer is the class
                                // the spec already names.
                                BinOp::Div => return Err(err(
                                    "implicit_rounding",
                                    *span,
                                    "`/` produces `decimal` at a fixed fractional scale of 12, \
                                     and a parameter is a typed slot — a decimal does not land \
                                     in one without an explicit `round(x, 0)`, `floor(x)` or \
                                     `ceil(x)`. Bind the value you mean instead of dividing at \
                                     the call site.",
                                )),
                            };
                        Expr::IntLit {
                            value: v,
                            span: *span,
                        }
                    }
                    _ => {
                        return Err(err(
                            "invalid_binding_expr",
                            *span,
                            "this arithmetic does not fold to a constant",
                        ))
                    }
                };
                let ty = match (&folded, money_ty) {
                    (Expr::IntLit { .. }, Some(m)) => m,
                    _ => lit_ty(&folded).expect("folded literals have types"),
                };
                Ok(BExpr::Lit { value: folded, ty })
            }
            other => {
                // `-2500` parses as `Neg { IntLit(2500) }` (every unary
                // minus does, `ordo-syntax/src/parser.rs`), so it must fold
                // to the literal `-2500` names before `lit_ty` is asked what
                // it is — see [`fold_neg_lit`]. Without this a negative
                // argument at a call site (`total: -2500`) was
                // `invalid_binding_expr`, unconditionally, because `Neg`
                // never bound at all.
                let folded = fold_neg_lit(other);
                let ty = lit_ty(&folded);
                match ty {
                    Some(ty) => Ok(BExpr::Lit { value: folded, ty }),
                    None => Err(err(
                        "unsupported",
                        other.span(),
                        "only literals and `$params` are bound as parameter values in this slice",
                    )),
                }
            }
        }
    }

    // ── projection ───────────────────────────────────────────────────────

    fn project(
        &mut self,
        q: &mut Query,
        s: &Source,
        scope: Scope,
        env: &ParamEnv,
    ) -> Result<Scope, SemaError> {
        let out = self.project_fields(q, s, &scope, env)?;
        q.outputs = out
            .iter()
            .map(|f| OutField {
                name: f.name.clone(),
                expr: f.expr.clone(),
            })
            .collect();
        // Sealed values in the projection become the query's host-open
        // obligations — the cost gate reads this, and so does the executor.
        q.host_open = self.host_open_of(&out);
        Ok(Scope::Fields(out))
    }

    /// One source's projected fields, without assigning the query outputs —
    /// a multi-source list concatenates several of these.
    fn project_fields(
        &mut self,
        q: &mut Query,
        s: &Source,
        scope: &Scope,
        env: &ParamEnv,
    ) -> Result<Vec<SField>, SemaError> {
        let base: Vec<SField> = match &scope {
            Scope::Entity { entity, alias } => {
                let e = self.schema.entity(entity).expect("resolved");
                // `sfield_of_entity`, not `sfield_of`: a sealed field needs
                // to know which entity and field it was sealed under, or
                // the projection cannot say how to open it.
                e.declared()
                    .map(|f| sfield_of_entity(f, entity, alias))
                    .collect()
            }
            Scope::Fields(f) => f.clone(),
            // Projections always run against a plain source scope; layering
            // and per-source multiplexing exist only for steps.
            Scope::Layered(_) | Scope::Multi(_) => {
                unreachable!("projection never sees a layered scope")
            }
        };

        let Some(items) = &s.projection else {
            // Omitted projection = every output field (R-C2).
            //
            // Over an ENTITY that means the OPEN SKELETON: an entity
            // carrying a sealed field is still readable, and the derived
            // surface omits the sealed column rather than refusing the
            // entity (`std-library.md` §2.4). Naming the field is how a
            // caller opts into the host-class cost.
            //
            // Over a COMPOSED CALLEE it does not. The callee already chose
            // its outputs, and if it named a sealed field then that field
            // IS its result shape — its manifest says so, and its
            // `host_open` carries the obligation to open it. Filtering
            // here dropped the column silently: `orderMemos` answered 200
            // with `memo` simply absent from every row, no refusal and no
            // release, while the catalog advertised it. A caller cannot
            // tell that from "the value was null".
            if matches!(scope, Scope::Entity { .. }) {
                return Ok(base.into_iter().filter(|f| !f.sealed).collect());
            }
            return Ok(base);
        };

        let mut out: Vec<SField> = Vec::new();
        let mut excluded: Vec<String> = Vec::new();

        for item in items {
            match item {
                ProjItem::Star { .. } => {
                    // Same open-skeleton rule as an omitted projection: `*`
                    // means everything the caller is allowed to see, and a
                    // sealed field is never that. Naming it explicitly still
                    // refuses — that is the difference between "not offered"
                    // and "asked for and denied".
                    //
                    // Nor an implicit field (`id`, `rev`) reached through a
                    // sub-select that stands for an entity: `*` on
                    // `orders(status == "done")` offers what `*` on
                    // `orders` offers.
                    for f in base.iter().filter(|f| !f.sealed && !f.implicit) {
                        if !out.iter().any(|o| o.name == f.name) {
                            out.push(f.clone());
                        }
                    }
                }
                ProjItem::Exclude { name, .. } => excluded.push(name.name.clone()),
                ProjItem::Field { path, span } => {
                    let last = path.last().expect("a path has at least one segment");
                    if path.len() == 1 {
                        // The corpus distinguishes a projection miss from a
                        // step miss: C080's class is projection_unknown_field.
                        let f = self
                            .lookup(scope, &last.name, last.span, q)
                            .map_err(as_projection_miss)?;
                        out.push(f);
                    } else {
                        // `customer.name` — the output takes the last segment's
                        // name, and the hop becomes a join.
                        let expr = self
                            .path(path, scope, q, *span, env, false)
                            .map_err(as_projection_miss)?;
                        let ref_target = self.ref_target_of(&expr, q);
                        out.push(SField {
                            name: last.name.clone(),
                            expr,
                            ref_target,
                            sealed: false,
                            seal_of: None,
                            blind: None,
                            implicit: false,
                        });
                    }
                }
                ProjItem::Named { out: o, value, .. } => {
                    // A bare rename of a sealed field is still a
                    // projection — `[copy: memo]` discloses exactly what
                    // `[memo]` does. Anything COMPUTED from a sealed value
                    // is not: the database would have to read it to
                    // compute, so `lower(memo)` refuses through the
                    // ordinary expression path below.
                    if let Expr::Path { segments, .. } = value {
                        if segments.len() == 1 {
                            if let Some(f) = base.iter().find(|f| f.name == segments[0].name) {
                                if f.sealed {
                                    let mut renamed = f.clone();
                                    renamed.name = o.name.clone();
                                    out.push(renamed);
                                    continue;
                                }
                            }
                        }
                    }
                    let expr = self.expr(value, scope, q, env)?;
                    let ref_target = self.ref_target_of(&expr, q);
                    out.push(SField {
                        name: o.name.clone(),
                        expr,
                        ref_target,
                        sealed: false,
                        seal_of: None,
                        blind: None,
                        implicit: false,
                    });
                }
            }
        }

        out.retain(|f| !excluded.contains(&f.name));
        for x in &excluded {
            if !base.iter().any(|f| &f.name == x) {
                return Err(err(
                    "projection_unknown_field",
                    s.name.span,
                    format!("`-{x}` excludes a field the source does not produce"),
                ));
            }
        }

        // Projection is the one place a sealed field is allowed; the
        // obligation to open it travels with the query.
        Ok(out)
    }

    /// Sealed fields in a PROJECTION are host-opened, not refused: the
    /// payload leaves the database as an envelope and becomes plaintext in
    /// this process. What the caller gets is the value; what changes is
    /// the plan's cost class, because rows now cross a boundary the
    /// database cannot do work behind.
    ///
    /// Only projection. A sealed field in a filter, a sort key, or a group
    /// key still refuses: the database cannot compare what it cannot read,
    /// and doing the comparison host-side would mean EXAMINING rows the
    /// caller never asked for — the venue gate's whole concern. Equality
    /// on a `sealed indexed` field is the one exception, and it is not
    /// this path: the blind index answers it in SQL.
    fn host_open_of(&self, fields: &[SField]) -> Vec<(String, String, String)> {
        fields
            .iter()
            .filter(|f| f.sealed)
            .filter_map(|f| {
                f.seal_of
                    .as_ref()
                    .map(|(e, fld)| (f.name.clone(), e.clone(), fld.clone()))
            })
            .collect()
    }

    // ── steps ────────────────────────────────────────────────────────────

    fn step(
        &mut self,
        q: &mut Query,
        scope: &Scope,
        s: &Step,
        env: &ParamEnv,
    ) -> Result<(), SemaError> {
        match s {
            Step::Filter { cond, .. } => {
                let c = self.cond(cond, scope, q, env)?;
                // After a group, a bar filters buckets — its conditions
                // reference aggregate outputs, so it must land in HAVING.
                if q.group.is_some() {
                    q.having.push(c);
                } else {
                    q.filters.push(c);
                }
                Ok(())
            }
            Step::Sort { keys, .. } => {
                // A declared sort replaces an inherited (callee) order —
                // found by corpus C045/C046, where appending made the
                // caller's sort a mere tiebreak on the callee's (R-C4).
                if q.sort_inherited {
                    q.sort.clear();
                    q.sort_inherited = false;
                }
                for k in keys {
                    let e = self.expr(&k.expr, scope, q, env)?;
                    refuse_json(&e, k.expr.span(), "a sort key")?;
                    q.sort.push(Sort {
                        expr: e,
                        desc: k.desc,
                    });
                }
                Ok(())
            }
            Step::Take { keys, n, .. } => {
                debug_assert!(keys.is_empty(), "keyed take is handled in pipeline()");
                q.limit = Some(self.take_limit(n, env)?);
                Ok(())
            }
            Step::Distinct { span } => {
                check_prior_sort(q, s)?;
                // A sealed output is an envelope under a fresh nonce per
                // row, so two equal plaintexts are never equal bytes:
                // `distinct` over one would remove nothing and order by
                // ciphertext. The same rule as a sort or a group key over a
                // sealed field, which this module's doc already states.
                if let Some((name, _, _)) = q.host_open.first() {
                    return Err(err(
                        "sealed_unsupported",
                        *span,
                        format!(
                            "`{name}` is sealed; `distinct` compares values the database \
                             cannot read, so it cannot run over a sealed output"
                        ),
                    ));
                }
                q.distinct = true;
                // Distinct destroys row identity exactly as grouping does:
                // the anchor row no longer exists in the output, so ordering
                // by its id is not just wrong, it is illegal — Postgres
                // rejects a DISTINCT whose ORDER BY is not in the select
                // list. The output fields become the tiebreak.
                q.tiebreak = q.outputs.iter().map(|o| o.expr.clone()).collect();
                Ok(())
            }
            Step::Group { keys, aggs, span } => {
                check_prior_sort(q, s)?;
                self.group(q, scope, keys, aggs, *span, env)
            }
            Step::Reduce { aggs, span } => {
                check_prior_sort(q, s)?;
                self.group(q, scope, &[], aggs, *span, env)
            }
            Step::Match { kind, span } => match kind {
                MatchKind::Any(src) | MatchKind::None(src) => {
                    let negated = matches!(kind, MatchKind::None(_));
                    self.bind_exists(q, src, negated, *span, env)
                }
                // Consumed by bind_second_source before inference; the
                // word appears only when it carries information, and one
                // that resolved nothing is accepted as noise (the antagonist
                // review's recommendation) rather than invented as an error.
                MatchKind::Ref { .. } => Ok(()),
                MatchKind::On { .. } => Err(err(
                    "unsupported",
                    *span,
                    "`match on <col>` joins a let back (Stage 1 P7)",
                )),
            },
            Step::Append { .. } => unreachable!("append is handled in pipeline()"),
        }
    }

    fn group(
        &mut self,
        q: &mut Query,
        scope: &Scope,
        keys: &[Expr],
        aggs: &[AggDecl],
        span: Span,
        env: &ParamEnv,
    ) -> Result<(), SemaError> {
        let mut gkeys = Vec::new();
        let mut outs = Vec::new();
        for k in keys {
            let e = self.expr(k, scope, q, env)?;
            refuse_json(&e, k.span(), "a group key")?;
            let name = match k {
                Expr::Path { segments, .. } => segments.last().expect("nonempty").name.clone(),
                Expr::FnCall { name, .. } => name.name.clone(),
                other => {
                    return Err(err(
                        "unsupported",
                        other.span(),
                        "a group key needs a name: use a path or a function call",
                    ))
                }
            };
            gkeys.push(e.clone());
            outs.push(OutField { name, expr: e });
        }

        // Ruling 3, amendment 2: over an outer-preserved join, the null-
        // extension row must be masked from EVERY aggregate. The probe is
        // the many side's join key: non-null exactly when a real row
        // matched. Bare `count` counts the deepest many-entity (probe as
        // its argument — COUNT(col) self-masks); `count(cond)` combines the
        // probe with its condition, or an empty parent counts a phantom;
        // `sum` gets the probe as a mask so the planner's coalesce makes an
        // empty bucket 0, not null; avg/min/max skip nulls natively.
        let probe = q
            .source_joins
            .iter()
            .rev()
            .find(|j| j.many_side)
            .map(|j| j.on[0].0.clone());

        // `joined`'s order is the pipeline's order AT THE GROUP — declared
        // sorts, then the tiebreak (DL: "order = the pipeline's current sort,
        // id tiebreak"). It has to be captured before the group replaces both.
        let mut current_order: Vec<Sort> = q.sort.clone();
        for tb in &q.tiebreak {
            current_order.push(Sort {
                expr: tb.clone(),
                desc: false,
            });
        }

        for a in aggs {
            let arg = match &a.arg {
                Some(x) => {
                    let b = self.expr(x, scope, q, env)?;
                    refuse_json(&b, a.span, "an aggregate")?;
                    Some(Box::new(b))
                }
                None => None,
            };
            let arg2 = match &a.arg2 {
                Some(x) => Some(Box::new(self.expr(x, scope, q, env)?)),
                None => None,
            };
            let cond_mask = match &a.cond {
                Some(c) => Some(self.cond(c, scope, q, env)?),
                None => None,
            };
            let not_phantom = probe.as_ref().map(|p| BCond::IsNull {
                expr: p.clone(),
                negated: true,
            });
            let (arg, mask) = match (a.func.name.as_str(), arg, cond_mask, &probe) {
                ("count", None, None, Some(p)) => (Some(Box::new(p.clone())), None),
                ("count", None, Some(c), Some(_)) => (
                    None,
                    Some(Box::new(BCond::And(vec![
                        not_phantom.clone().expect("probe present"),
                        c,
                    ]))),
                ),
                ("sum", arg, cond, Some(_)) => {
                    let mut parts = vec![not_phantom.clone().expect("probe present")];
                    if let Some(c) = cond {
                        parts.push(c);
                    }
                    (arg, Some(Box::new(BCond::And(parts))))
                }
                (_, arg, Some(c), _) => (arg, Some(Box::new(c))),
                (_, arg, None, _) => (arg, None),
            };
            // An average of a `money(2)` is a decimal in dollars, not in
            // cents (27ak): the argument widens before it is averaged.
            let arg = if a.func.name == "avg" {
                arg.map(|x| Box::new(widen(*x)))
            } else {
                arg
            };
            let ty = match a.func.name.as_str() {
                // Both count rows: bare `count` and `distinct field`
                // (count-distinct).
                "count" | "distinct" => ScalarTy::Int,
                // Ordered string aggregation renders text whatever it reads.
                "joined" => ScalarTy::Text,
                // `avg` is the second half of the division ruling: it IS a
                // division — by the row count — and it produces `decimal`
                // at the same fixed scale of 12, whatever it averaged.
                "avg" => ScalarTy::Decimal(DIVISION_SCALE),
                _ => arg.as_ref().map(|x| x.ty()).unwrap_or(ScalarTy::Int),
            };
            // DL: `joined` aggregates a TEXT field. Concatenating anything
            // else would need a rendering rule the language has not stated.
            if a.func.name == "joined"
                && !matches!(arg.as_ref().map(|x| x.ty()), Some(ScalarTy::Text))
            {
                // 9x: this was `type_error`, a name so general it could not
                // predict where to look — and it names exactly what
                // `argument_type_mismatch` was minted for: a function applied
                // to an argument outside its domain. One mistake, one class.
                return Err(err(
                    "argument_type_mismatch",
                    a.span,
                    "`joined` aggregates a text field",
                ));
            }
            let order = if a.func.name == "joined" {
                current_order.clone()
            } else {
                Vec::new()
            };
            outs.push(OutField {
                name: a.out.name.clone(),
                expr: BExpr::Agg {
                    func: a.func.name.clone(),
                    arg,
                    arg2,
                    order,
                    mask,
                    ty,
                },
            });
        }

        let _ = span;
        // Grouping replaces the row identity: the group keys become the
        // tiebreak (R-C4). A keyless group — `reduce` — provably yields one
        // row, the one legitimate empty tiebreak in the system.
        //
        // The declared sort goes with it. A pre-group `sort` orders ROWS, and
        // after the group there are no rows to order — carrying it forward
        // emitted `ORDER BY t0.name` over a grouped select, which Postgres
        // rejects outright ("must appear in the GROUP BY clause"). Its one
        // surviving meaning is the one DL gives it: it is the order `joined`
        // concatenates in, and that has been captured above.
        q.sort.clear();
        q.tiebreak = gkeys.clone();
        q.group = Some(Group { keys: gkeys });
        // Nothing sealed survives a fold (ruled 2026-10-05, audit ask 1). A
        // sealed group key and an aggregate over a sealed field both refuse
        // above, and `count` counts rows without opening them, so no grouped
        // output is an envelope. The pre-group projection's obligations go
        // with its rows: a group or reduce over a sealed field is plain SQL,
        // releases no key, and needs no `unbounded`. An aggregate may take a
        // sealed field's old name (`group placed { memo: count }`), because
        // that name no longer marks anything to open.
        q.host_open.clear();
        q.outputs = outs;
        Ok(())
    }

    // ── expressions and conditions ───────────────────────────────────────

    /// The blind-index path for a sealed-indexed field under `==`. Returns
    /// None when the condition is not that shape (and the ordinary path —
    /// including the sealed refusal — proceeds).
    #[allow(clippy::too_many_arguments)]
    fn try_blind_eq(
        &mut self,
        lhs: &Expr,
        op: ordo_syntax::CmpOp,
        rhs: &Expr,
        scope: &Scope,
        q: &mut Query,
        env: &ParamEnv,
        span: Span,
    ) -> Result<Option<BCond>, SemaError> {
        let Expr::Path { segments, .. } = lhs else {
            return Ok(None);
        };
        if segments.len() != 1 {
            return Ok(None);
        }
        let Ok(f) = self.lookup(scope, &segments[0].name, segments[0].span, q) else {
            return Ok(None);
        };
        let Some((entity, field, normalize, bidx_col, alias)) = f.blind else {
            return Ok(None);
        };
        if op != ordo_syntax::CmpOp::Eq {
            return Err(err(
                "sealed_unsupported",
                span,
                format!(
                    "`{field}` is sealed — the blind index answers equality only, not `{}`",
                    op.as_str()
                ),
            ));
        }
        let r = self.const_expr(rhs, env)?;
        // An UNBOUND parameter (inspection binding, prepared statements)
        // compares against its slot; the door digests the value at call
        // time before the statement runs. A bound literal digests here.
        if let BExpr::Param { name, .. } = &r {
            // A parameter's digest cannot be computed until the value
            // arrives. It used to compare the RAW parameter against the
            // digest column, which matched nothing and said nothing — a
            // silent empty result for a lookup that should have worked.
            return Ok(Some(BCond::Cmp {
                op: ordo_syntax::CmpOp::Eq,
                lhs: BExpr::Col {
                    alias,
                    column: bidx_col,
                    ty: ScalarTy::Text,
                },
                rhs: BExpr::BlindDigest {
                    source: ir::BlindSource::Param(name.clone()),
                    entity,
                    field,
                    normalize,
                    ty: ScalarTy::Text,
                },
            }));
        }
        // `email == null`. The reserved null sentinel is what makes this
        // answerable at all: nulls carry a digest like every other value,
        // so the language's one equality (`null == null` is true, DL §4)
        // needs no special case here either. Emitting SQL `NULL` instead
        // would compare a digest column against NULL and match nothing,
        // which is the silent-zero-rows shape this whole path exists to
        // delete.
        //
        // This is an AUTHOR writing a condition in their own op, and it is
        // a different actor from a caller narrowing an std-library op at
        // request time. `std-library.md` §sealed refinement refuses
        // `is_null` on that caller-supplied surface — presence is itself a
        // disclosure, and the sentinel makes nullness the cheapest thing
        // to learn about a sealed field. That refusal belongs to the
        // refinement layer (9k, unbuilt) and must not be lost when it
        // lands; blessing `== null` here does not bless it there.
        if let BExpr::Lit {
            value: Expr::NullLit { .. },
            ..
        } = r
        {
            return Ok(Some(BCond::Cmp {
                op: ordo_syntax::CmpOp::Eq,
                lhs: BExpr::Col {
                    alias,
                    column: bidx_col,
                    ty: ScalarTy::Text,
                },
                rhs: BExpr::BlindDigest {
                    source: ir::BlindSource::Null,
                    entity,
                    field,
                    normalize,
                    ty: ScalarTy::Text,
                },
            }));
        }
        let BExpr::Lit {
            value: Expr::StrLit { value, .. },
            ..
        } = r
        else {
            return Err(err(
                "sealed_unsupported",
                span,
                "blind-index equality compares against a text value or `null`",
            ));
        };
        // The SITE, not the digest. Normalization is a schema fact and
        // travels with it; the executor applies it and then HMACs, behind
        // its own key release.
        Ok(Some(BCond::Cmp {
            op: ordo_syntax::CmpOp::Eq,
            lhs: BExpr::Col {
                alias,
                column: bidx_col,
                ty: ScalarTy::Text,
            },
            rhs: BExpr::BlindDigest {
                source: ir::BlindSource::Literal(value),
                entity,
                field,
                normalize,
                ty: ScalarTy::Text,
            },
        }))
    }

    fn lookup(
        &mut self,
        scope: &Scope,
        name: &str,
        span: Span,
        q: &mut Query,
    ) -> Result<SField, SemaError> {
        match scope {
            Scope::Entity { entity, alias } => {
                let e = self.schema.entity(entity).expect("resolved");
                let f = e.field(name).ok_or_else(|| {
                    err(
                        "unknown_field",
                        span,
                        format!("`{}` has no field `{name}`", e.name),
                    )
                })?;
                Ok(sfield_of_entity(f, entity, alias))
            }
            Scope::Fields(fs) => fs.iter().find(|f| f.name == name).cloned().ok_or_else(|| {
                let have: Vec<&str> = fs.iter().map(|f| f.name.as_str()).collect();
                err(
                    "not_output_field",
                    span,
                    format!("no field `{name}` here — available: [{}]", have.join(", ")),
                )
            }),
            Scope::Multi(sources) => {
                let mut hits = Vec::new();
                for (src_name, sc) in sources {
                    if let Ok(f) = self.lookup(sc, name, span, q) {
                        hits.push((src_name.clone(), f));
                    }
                }
                match hits.len() {
                    1 => Ok(hits.pop().expect("one").1),
                    0 => Err(err(
                        "unknown_field",
                        span,
                        format!("no source produces `{name}`"),
                    )),
                    _ => {
                        let names: Vec<String> =
                            hits.iter().map(|(s, _)| format!("{s}.{name}")).collect();
                        Err(err(
                            "ambiguous_field",
                            span,
                            format!(
                                "`{name}` exists on more than one source — qualify it: {}",
                                names.join(" or ")
                            ),
                        ))
                    }
                }
            }
            Scope::Layered(layers) => {
                for l in layers {
                    if let Ok(f) = self.lookup(l, name, span, q) {
                        return Ok(f);
                    }
                }
                // Report from the deepest layer: it has the widest view, so
                // its error names the source rather than a narrowed subset.
                let last = layers.last().expect("a layered scope has layers");
                self.lookup(last, name, span, q)
            }
        }
        .inspect(|_| {
            let _ = &q;
        })
    }

    /// Resolve a dot-path, adding one LEFT JOIN per hop. Joins are keyed by
    /// (alias, column) so two paths through the same ref share a join.
    fn path(
        &mut self,
        segments: &[ordo_syntax::Ident],
        scope: &Scope,
        q: &mut Query,
        span: Span,
        env: &ParamEnv,
        doc: bool,
    ) -> Result<BExpr, SemaError> {
        let head = &segments[0];
        // Source qualification (DL ~1660): in a multi-source list,
        // `teams.name` names the source first. Source names shadow fields
        // in this position only when a source with that name is listed.
        if segments.len() > 1 {
            if let Some(info) = self.lets.iter().find(|l| l.name == head.name) {
                // `cap.m` — a reduce-let is a scalar (R-C3); anything else
                // referenced as one is the C069 error.
                if !info.ends_in_reduce {
                    return Err(err(
                        "let_scalar_requires_reduce",
                        span,
                        format!(
                            "`{}` does not end in `reduce`, so it has rows, not a scalar",
                            head.name
                        ),
                    ));
                }
                let col = &segments[1];
                let (_, ty, _) = info
                    .fields
                    .iter()
                    .find(|(n, _, _)| n == &col.name)
                    .ok_or_else(|| {
                        err(
                            "unknown_field",
                            col.span,
                            format!("`{}` has no output `{}`", head.name, col.name),
                        )
                    })?;
                if segments.len() > 2 {
                    return Err(err(
                        "unsupported",
                        span,
                        "a let scalar does not dot further",
                    ));
                }
                return Ok(BExpr::ScalarSub {
                    cte: info.name.clone(),
                    column: col.name.clone(),
                    ty: *ty,
                });
            }
            if let Some(src_scope) = find_source_scope(scope, &head.name) {
                let src_scope = src_scope.clone();
                let second = &segments[1];
                let mut cur = self.lookup(&src_scope, &second.name, second.span, q)?;
                if cur.sealed {
                    return Err(err(
                        "sealed_unsupported",
                        second.span,
                        sealed_in_sql(&cur.name),
                    ));
                }
                if segments.len() == 2 {
                    return Ok(cur.expr);
                }
                return self.walk_refs(&segments[2..], &mut cur, q, env, doc);
            }
        }
        let mut cur = self.lookup(scope, &head.name, head.span, q)?;
        if cur.sealed && segments.len() == 1 {
            return Err(err("sealed_unsupported", span, sealed_in_sql(&cur.name)));
        }
        if segments.len() == 1 {
            return Ok(cur.expr);
        }
        self.walk_refs(&segments[1..], &mut cur, q, env, doc)
    }

    /// Walk ref hops from a resolved head, adding one LEFT dot-join per hop.
    fn walk_refs(
        &mut self,
        segments: &[ordo_syntax::Ident],
        cur: &mut SField,
        q: &mut Query,
        env: &ParamEnv,
        doc: bool,
    ) -> Result<BExpr, SemaError> {
        for (i, seg) in segments.iter().enumerate() {
            // A `json` field ends the walk through refs: every segment after
            // it is a step inside the document (DL §2, JSON documents).
            if cur.expr.ty() == ScalarTy::Json {
                return doc_path(cur, &segments[i..], doc);
            }
            let target = cur.ref_target.clone().ok_or_else(|| {
                err(
                    "not_a_ref",
                    seg.span,
                    format!(
                        "`{}` is not a ref, so `.{}` has nothing to walk to",
                        cur.name, seg.name
                    ),
                )
            })?;
            let BExpr::Col {
                alias: from_alias,
                column: from_column,
                ..
            } = cur.expr.clone()
            else {
                return Err(err(
                    "unsupported",
                    seg.span,
                    "dotting through a computed field is not lowered in this slice",
                ));
            };

            // A dot-walk that needs a NEW join after grouping would put an
            // ungrouped column into HAVING (antagonist finding 4). The walk
            // belongs in the group key: `group customer.tier`.
            let existing = q
                .joins
                .iter()
                .find(|j| j.from_alias == from_alias && j.from_column == from_column)
                .map(|j| j.alias.clone());
            if existing.is_none() && q.group.is_some() {
                return Err(err(
                    "unsupported",
                    seg.span,
                    format!(
                        "cannot dot through `{}` after group — put the walk in the group key",
                        cur.name
                    ),
                ));
            }
            let jalias = match existing {
                Some(a) => a,
                None => {
                    let a = self.alias();
                    // Compiled once, when the hop is first made. A second
                    // path through the same hop reuses the alias above and
                    // the guard with it.
                    let guard = self.joined_bound(&target, &a, env)?;
                    q.joins.push(Join {
                        table: target.clone(),
                        alias: a.clone(),
                        from_alias,
                        from_column,
                        guard,
                    });
                    a
                }
            };

            let ent = self.schema.entity(&target).expect("resolved target");
            let f = ent.field(&seg.name).ok_or_else(|| {
                err(
                    "unknown_field",
                    seg.span,
                    format!("`{target}` has no field `{}`", seg.name),
                )
            })?;
            *cur = sfield_of(f, &jalias);
            if cur.sealed && cur.expr.ty() == ScalarTy::Json && i + 1 < segments.len() {
                return doc_path(cur, &segments[i + 1..], doc);
            }
            if cur.sealed {
                return Err(err(
                    "sealed_unsupported",
                    seg.span,
                    sealed_in_sql(&format!("{target}.{}", seg.name)),
                ));
            }
        }
        Ok(cur.expr.clone())
    }

    fn expr(
        &mut self,
        e: &Expr,
        scope: &Scope,
        q: &mut Query,
        env: &ParamEnv,
    ) -> Result<BExpr, SemaError> {
        Ok(match e {
            Expr::Param { name, span } => env.get(name).cloned().ok_or_else(|| {
                err(
                    "unbound_required_param",
                    *span,
                    format!("`${name}` is not bound at this call site"),
                )
            })?,
            Expr::Path { segments, span } => self.path(segments, scope, q, *span, env, false)?,
            Expr::Neg { expr, .. } => {
                let inner = self.expr(expr, scope, q, env)?;
                let ty = inner.ty();
                BExpr::Neg {
                    expr: Box::new(inner),
                    ty,
                }
            }
            Expr::Binary { op, lhs, rhs, span } => {
                let l = self.expr(lhs, scope, q, env)?;
                let r = self.expr(rhs, scope, q, env)?;
                refuse_json(&l, *span, "an arithmetic operator")?;
                refuse_json(&r, *span, "an arithmetic operator")?;
                let (l, r) = arith_footing(*op, l, r, *span)?;
                let ty = binary_ty(*op, l.ty(), r.ty());
                BExpr::Binary {
                    op: *op,
                    lhs: Box::new(l),
                    rhs: Box::new(r),
                    ty,
                }
            }
            Expr::FnCall { name, args, span } => {
                // `allocate` is the blessed money-split primitive, and its
                // legal home is narrow: inside an `each` emission over a
                // LIST PARAMETER, with the weight written as the element
                // field. Everywhere else it is `invalid_allocate` (DL §3,
                // Allocation; `glossary.md`, "`allocate`"). Expression
                // position is everywhere else.
                //
                // `each` over a list parameter is itself [lean] and does
                // not parse yet, so today every site the language can
                // express is an illegal one — which is exactly why this
                // arm is the whole of the primitive that is buildable
                // without inventing the construct it lives in.
                if name.name == "allocate" {
                    return Err(err(
                        "invalid_allocate",
                        *span,
                        "`allocate(total, weights)` is legal only inside an `each` emission \
                         over a list parameter, with the weight as the element field \
                         (`allocate($total, item.weight)`) — it splits a total among \
                         parties, and there is no vector to split among here. The \
                         optional third argument is the mode (who absorbs the odd \
                         minor unit); it does not make a site legal",
                    ));
                }
                let mut bs = Vec::new();
                for a in args {
                    bs.push(self.expr(a, scope, q, env)?);
                }
                let bs = fn_footing(&name.name, bs, *span)?;
                check_arg_types(&name.name, &bs, *span)?;
                let ty = fn_ty(&name.name, &bs);
                BExpr::Fn {
                    name: name.name.clone(),
                    args: bs,
                    ty,
                }
            }
            Expr::Cond { arms, else_, .. } => {
                let mut bas = Vec::new();
                for a in arms {
                    let c = self.cond(&a.condition, scope, q, env)?;
                    let v = self.expr(&a.value, scope, q, env)?;
                    bas.push((c, v));
                }
                let e2 = self.expr(else_, scope, q, env)?;
                // Every arm lands in one column, so a money arm brings the
                // others to its places (27ak).
                let (conds, vals): (Vec<BCond>, Vec<BExpr>) = bas.into_iter().unzip();
                let mut vals = money_footing_all(
                    vals.into_iter().chain(std::iter::once(e2)).collect(),
                    e.span(),
                )?;
                let e2 = vals.pop().expect("the else arm");
                let bas: Vec<(BCond, BExpr)> = conds.into_iter().zip(vals).collect();
                let ty = bas.first().map(|(_, v)| v.ty()).unwrap_or_else(|| e2.ty());
                BExpr::Cond {
                    arms: bas,
                    else_: Box::new(e2),
                    ty,
                }
            }
            other => {
                let ty = lit_ty(other).ok_or_else(|| {
                    err(
                        "unsupported",
                        other.span(),
                        "expression form not lowered in this slice",
                    )
                })?;
                BExpr::Lit {
                    value: other.clone(),
                    ty,
                }
            }
        })
    }

    /// One side of a condition. A bare path may walk into a `json` document
    /// here, and nowhere else: the condition types it from its other side.
    fn operand(
        &mut self,
        e: &Expr,
        scope: &Scope,
        q: &mut Query,
        env: &ParamEnv,
    ) -> Result<BExpr, SemaError> {
        match e {
            Expr::Path { segments, span } => self.path(segments, scope, q, *span, env, true),
            _ => self.expr(e, scope, q, env),
        }
    }

    fn cond(
        &mut self,
        c: &Condition,
        scope: &Scope,
        q: &mut Query,
        env: &ParamEnv,
    ) -> Result<BCond, SemaError> {
        Ok(match c {
            Condition::Compare { op, lhs, rhs, span } => {
                // Blind-index equality (F1): `email == $e` on a sealed
                // INDEXED field compares digests, never payloads. Only
                // equality — every other operator would need the value.
                if let Some(bc) = self.try_blind_eq(lhs, *op, rhs, scope, q, env, *span)? {
                    return Ok(bc);
                }
                let l = self.operand(lhs, scope, q, env)?;
                let r = self.operand(rhs, scope, q, env)?;
                if is_doc(&l) || is_doc(&r) {
                    let (l, r) = doc_pair(l, r, *span)?;
                    return Ok(BCond::Cmp {
                        op: *op,
                        lhs: l,
                        rhs: r,
                    });
                }
                refuse_json(&l, *span, "a comparison")?;
                refuse_json(&r, *span, "a comparison")?;
                check_duration_units(&l, &r, *span)?;
                // `price > 29.99` against a `money(2)` field compares with
                // 2999 minor units (27ak; antagonist finding 6).
                let (l, r) = unify_money(l, r, *span)?;
                BCond::Cmp {
                    op: *op,
                    lhs: l,
                    rhs: r,
                }
            }
            Condition::In { expr, list, span } => {
                let e = self.operand(expr, scope, q, env)?;
                if is_doc(&e) {
                    let mut items = Vec::new();
                    for i in list {
                        items.push(self.expr(i, scope, q, env)?);
                    }
                    let ty = doc_list_ty(&items, *span)?;
                    return Ok(BCond::In {
                        expr: typed_doc(e, ty),
                        list: items,
                    });
                }
                refuse_json(&e, *span, "an `in` condition")?;
                let mut items = vec![e];
                for i in list {
                    items.push(self.expr(i, scope, q, env)?);
                }
                let mut items = money_footing_all(items, *span)?;
                let e = items.remove(0);
                BCond::In {
                    expr: e,
                    list: items,
                }
            }
            Condition::Text { op, lhs, rhs, span } => {
                let l = self.operand(lhs, scope, q, env)?;
                let r = self.operand(rhs, scope, q, env)?;
                if is_doc(&l) || is_doc(&r) || (*op == TextOp::Contains && l.ty() == ScalarTy::Json)
                {
                    return Ok(BCond::Text {
                        op: *op,
                        lhs: doc_text_lhs(*op, l, &r, *span)?,
                        rhs: r,
                    });
                }
                refuse_json(&l, *span, "a text condition")?;
                refuse_json(&r, *span, "a text condition")?;
                BCond::Text {
                    op: *op,
                    lhs: l,
                    rhs: r,
                }
            }
            Condition::IsNull { expr, negated, .. } => BCond::IsNull {
                expr: self.operand(expr, scope, q, env)?,
                negated: *negated,
            },
            Condition::And { items, .. } => {
                let mut v = Vec::new();
                for i in items {
                    v.push(self.cond(i, scope, q, env)?);
                }
                BCond::And(v)
            }
            Condition::Or { items, .. } => {
                let mut v = Vec::new();
                for i in items {
                    v.push(self.cond(i, scope, q, env)?);
                }
                BCond::Or(v)
            }
            Condition::Not { inner, .. } => BCond::Not(Box::new(self.cond(inner, scope, q, env)?)),
            Condition::BareBool(e) => {
                // G7 hands this to sema deliberately: the parser admits any
                // path here, and only the binder knows its type. Without this
                // check `activeProducts(name, price)` planned cleanly to
                // `"name" IS TRUE` over a text column (C105).
                let b = self.operand(e, scope, q, env)?;
                if is_doc(&b) {
                    return Err(err(
                        "json_path_type",
                        e.span(),
                        "rule: a path into a `json` document has no type of its own, so it is \
                         not a condition alone (DL §2, json).\n\
                         fix: compare it with a bool: `prefs.on == true`",
                    ));
                }
                if b.ty() != ScalarTy::Bool {
                    return Err(err(
                        "non_boolean_condition",
                        e.span(),
                        format!(
                            "a bare field is a condition only when it is `bool`; this one is `{}`",
                            b.ty().as_str()
                        ),
                    ));
                }
                BCond::Bare(b)
            }
            Condition::Since { expr, dur, span } => {
                let lhs = self.expr(expr, scope, q, env)?;
                let Expr::DurationLit { n, unit, .. } = dur else {
                    return Err(err(
                        "unsupported",
                        *span,
                        "`since` takes a duration literal",
                    ));
                };
                let anchor = shift_date(self.clock, *n as i64, *unit, false).ok_or_else(|| {
                    err(
                        "unsupported",
                        *span,
                        "`since` over sub-day units needs a datetime clock (not in this slice)",
                    )
                })?;
                let (ay, am, ad) = anchor;
                BCond::Cmp {
                    op: ordo_syntax::CmpOp::Ge,
                    lhs,
                    rhs: BExpr::Lit {
                        value: Expr::DateLit {
                            y: ay,
                            m: am,
                            d: ad,
                            span: *span,
                        },
                        ty: ScalarTy::Date,
                    },
                }
            }
            Condition::Under { expr, id, span } | Condition::Above { expr, id, span } => {
                let above = matches!(c, Condition::Above { .. });
                let lhs = self.expr(expr, scope, q, env)?;
                // A ref to a tree entity — or the tree entity's OWN id,
                // which is the primary spelling for its own subtree:
                // `categories(id under $root)`.
                let tree = self.ref_target_of(&lhs, q).or_else(|| {
                    if let (BExpr::Col { alias, column, .. }, From::Table { entity, alias: fa }) =
                        (&lhs, &q.from)
                    {
                        if column == "id" && alias == fa {
                            return Some(entity.clone());
                        }
                    }
                    None
                });
                let tree = tree.ok_or_else(|| {
                    err(
                        "unsupported",
                        *span,
                        "`under`/`above` need a ref to a tree entity, or the tree's own `id`",
                    )
                })?;
                if !self
                    .schema
                    .entity(&tree)
                    .map(|e| e.is_tree)
                    .unwrap_or(false)
                {
                    return Err(err(
                        "unsupported",
                        *span,
                        format!("`{tree}` is not a tree entity"),
                    ));
                }
                let anchor = self.expr(id, scope, q, env)?;
                BCond::TreeWalk {
                    expr: lhs,
                    tree,
                    anchor,
                    above,
                }
            }
        })
    }
}

/// A duration column stores its declared unit, so comparing it against a
/// literal in another unit compares raw numbers with different meanings.
/// Converting is the right answer and belongs to full P3's typing pass; until
/// then this is loud rather than quietly off by a factor of 3,600,000.
fn check_duration_units(l: &BExpr, r: &BExpr, span: Span) -> Result<(), SemaError> {
    if let (ScalarTy::Duration(a), ScalarTy::Duration(b)) = (l.ty(), r.ty()) {
        if a != b {
            return Err(err(
                "duration_unit_mismatch",
                span,
                format!(
                    "comparing `duration({})` with `duration({})` — this slice does not convert between units",
                    a.as_str(),
                    b.as_str()
                ),
            ));
        }
    }
    Ok(())
}

/// Distinct and group settle the rows; a prior sort key that is not one of
/// the output expressions can no longer order anything and previously
/// compiled into `SELECT DISTINCT … ORDER BY <unprojected>` — a Postgres
/// error at runtime (antagonist finding 3). Output keys keep their role.
fn check_prior_sort(q: &mut Query, step: &Step) -> Result<(), SemaError> {
    // An INHERITED sort (a callee's own order) dies silently at a settle —
    // aggregating over an ordered stream never needed the order. Only a
    // sort the author declared here gets the loud error.
    if q.sort_inherited {
        q.sort.clear();
        q.sort_inherited = false;
        return Ok(());
    }
    for srt in &q.sort {
        if !q.outputs.iter().any(|o| o.expr == srt.expr) {
            return Err(err(
                "not_output_field",
                step_span_of(step),
                "a sort before distinct/group must use an output field — or sort after it",
            ));
        }
    }
    Ok(())
}

fn step_span_of(s: &Step) -> Span {
    match s {
        Step::Filter { span, .. }
        | Step::Sort { span, .. }
        | Step::Take { span, .. }
        | Step::Group { span, .. }
        | Step::Reduce { span, .. }
        | Step::Distinct { span }
        | Step::Match { span, .. }
        | Step::Append { span, .. } => *span,
    }
}

/// The scope of a listed source with this name, if the step scope carries a
/// multi-source layer.
fn find_source_scope<'a>(scope: &'a Scope, name: &str) -> Option<&'a Scope> {
    match scope {
        Scope::Multi(sources) => sources.iter().find(|(n, _)| n == name).map(|(_, s)| s),
        Scope::Layered(layers) => layers.iter().find_map(|l| find_source_scope(l, name)),
        _ => None,
    }
}

fn sfield_of(f: &ordo_schema::Field, alias: &str) -> SField {
    SField {
        name: f.name.clone(),
        expr: BExpr::Col {
            alias: alias.to_string(),
            column: f.column(),
            ty: f.ty.value_ty(),
        },
        ref_target: match &f.ty {
            FieldTy::Ref { target, .. } => Some(target.clone()),
            _ => None,
        },
        sealed: f.sealed,
        seal_of: None,
        blind: None,
        implicit: f.implicit,
    }
}

/// The field names a source's projection reads first: `memo` in
/// `[memo]`, `[note: memo]` and `[memo.x]`. What a sub-select standing for
/// that source must carry when the field is sealed and so not carried by
/// default.
fn projection_heads(src: &Source) -> Vec<String> {
    let Some(items) = &src.projection else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            ProjItem::Field { path, .. } => path.first().map(|s| s.name.clone()),
            ProjItem::Named {
                value: Expr::Path { segments, .. },
                ..
            } => segments.first().map(|s| s.name.clone()),
            _ => None,
        })
        .collect()
}

/// As `sfield_of`, but knowing the owning entity — which is what arms the
/// blind index on sealed-indexed fields.
fn sfield_of_entity(f: &ordo_schema::Field, entity: &str, alias: &str) -> SField {
    let mut s = sfield_of(f, alias);
    if f.sealed {
        s.seal_of = Some((entity.to_string(), f.name.clone()));
    }
    if f.sealed && f.indexed {
        s.blind = Some((
            entity.to_string(),
            f.name.clone(),
            f.normalize.clone(),
            f.index_column(),
            alias.to_string(),
        ));
    }
    s
}

/// A binding's value must fit the parameter's declared type. Before this
/// check, `recentOrders($since: 5)` stamped `date` onto the integer and
/// silently mis-ran (C025/C026) — the exact wrong-answer class a closed
/// language exists to refuse.
/// A guard atom's literal, against the type of the column it is compared to.
///
/// # Why this reuses the parameter rules rather than inventing guard ones
///
/// [`fit_slot`] already encodes what the language admits between a
/// declared slot and a value: `int` into `money` and `duration`, `text` and
/// `id` either way, `decimal` into none of them without an explicit
/// conversion, and never `null`. A guard has no reason to be more permissive
/// than a parameter slot — it is the authorization path — and two type tables
/// would eventually disagree about `money`, which is the one every reader
/// gets wrong.
///
/// The class is `param_type_mismatch`, and that is a reuse rather than a
/// stretch: the compile inventory defines it as *"binding expression's type ≠
/// declared parameter type"*, and a guard literal is a value being fitted to
/// a declared column type by exactly the same rules. Minting a guard-specific
/// class would name where the mistake was NOTICED rather than what is wrong,
/// which the 9r boundary ruling forbids. `implicit_rounding` reaches the
/// caller unchanged from the shared check, for the same reason.
fn check_guard_lit_ty(
    value: BExpr,
    column: ScalarTy,
    field: &str,
    span: Span,
) -> Result<BExpr, SemaError> {
    fit_slot(value, column, span).map_err(|e| SemaError {
        code: e.code,
        // The shared message says "expected X, got Y", which is right and
        // says nothing about where. On a guard the field is the whole
        // orientation: an author reads `access read tasks(qty == "seven")`
        // and needs to be told it is `qty` that is an int.
        message: format!("in a guard comparison on `{field}`: {}", e.message),
        span: e.span,
    })
}

/// Fit a value to a **typed slot** — a parameter, a write's right-hand
/// side, a guard literal — and hand back the value as the slot stores it.
///
/// The checks are the ones a parameter always had, and since 27ak (ruled 2026-10-04)
/// the fit also converts: a numeric literal written in major units lands in
/// a `money(p)` slot as whole minor units (`29.99` into `money(2)` is
/// `2999`), an `int` expression is scaled by `10^p`, and an explicit
/// `round(x, n)` with `n <= p` is scaled the same way. A literal finer than
/// the places is `implicit_rounding`: storing it would round, and nothing
/// rounds implicitly (DL §2). This is antagonist finding 6 fixed by
/// building the conversion the first spec promised, where 2026-08-12 fixed
/// it by refusing the literal.
fn fit_slot(value: BExpr, declared: ScalarTy, span: Span) -> Result<BExpr, SemaError> {
    use ScalarTy::*;
    // **No value fits a `blob` slot**, and this is the check that makes
    // that true wherever a value is fitted to a declared type — which,
    // since `check_guard_lit_ty` delegates here, includes a guard
    // comparison against a blob column.
    //
    // There is no literal form for a blob (`lit_ty` produces none, which
    // is correct) and no expression that produces one, so the only thing
    // that can reach this slot is a `text` the author hoped would be
    // interpreted as a handle. That is the request the platform must not
    // serve: a caller-supplied handle names an object the caller did not
    // upload.
    if declared.is_content() || value.ty().is_content() {
        return Err(err(
            "param_type_mismatch",
            span,
            "rule: a `blob` is opaque content produced only by an upload, so no value \
             fits a blob slot and a blob fits no other slot.\n\
             fix: compare or bind `f.mime` / `f.size`, which are ordinary values, and \
             move the bytes through the blob door",
        ));
    }
    if let BExpr::Lit { value: v, .. } = &value {
        // `null` lexes with Text's type and would plan `= NULL` — never
        // true, silently empty (antagonist finding 5). Params are values;
        // absence is a condition (`is null`), not a binding.
        if matches!(v, Expr::NullLit { .. }) {
            return Err(err(
                "param_type_mismatch",
                span,
                "a parameter cannot be bound to null",
            ));
        }
    }
    let got = value.ty();
    if declared == Json || got == Json {
        return fit_json(value, declared, span);
    }
    match (declared, got) {
        (Money(p), Money(q)) if p != q => return Err(places_mismatch(declared, got, span)),
        (Money(_), Money(_)) => return Ok(value),
        // A source literal is in major units; one already typed `money`
        // (a parameter value passed on) is in minor units and is the arm
        // above.
        (Money(p), Int | Decimal(_)) if is_numeric_lit(&value) => return money_lit(value, p, span),
        (Money(p), Int) => return Ok(scale_to_money(value, p)),
        // An explicit `round(x, n)` with `n` no finer than the places is
        // the conversion the author wrote; scaling it by `10^p` is exact.
        (Money(p), Decimal(n)) if n <= p && is_round_call(&value) => {
            return Ok(scale_to_money(value, p))
        }
        (Decimal(p), Int | Decimal(_)) if is_numeric_lit(&value) => {
            return decimal_lit(value, p, span)
        }
        (Decimal(p), Decimal(s) | Money(s)) if s <= p => return Ok(widen(value)),
        (Decimal(_), Int) => return Ok(value),
        (Decimal(p), Decimal(s) | Money(s)) => {
            return Err(err(
                "implicit_rounding",
                span,
                format!(
                    "a value with up to {s} places cannot land in a `decimal({p})` slot on \
                     its own — say how it converts with `round(x, {p})`, `floor(x)` or \
                     `ceil(x)`"
                ),
            ))
        }
        _ => {}
    }
    // A parameter is a typed slot, so it is one of the two places a
    // `decimal` can try to land in this slice (the other is a write RHS,
    // refused in `ordo_plan::writes`). DL §2, "No implicit rounding,
    // anywhere": the conversion is the author's to write, because the
    // alternative is a compiler that silently drops digits exactly where
    // the cents are decided.
    if matches!(got, Decimal(_)) && matches!(declared, Money(_) | Int | Duration(_)) {
        let places = match declared {
            Money(p) if p > 0 => format!("`round(x, {p})`, "),
            _ => String::new(),
        };
        return Err(err(
            "implicit_rounding",
            span,
            format!(
                "a `decimal` cannot land in a `{}` slot on its own — division and \
                 `avg` produce decimal at scale 12; write {places}`round(x, 0)`, \
                 `floor(x)` or `ceil(x)` to say how it converts",
                declared.spelled()
            ),
        ));
    }
    let ok = got == declared
        || matches!(
            (declared, got),
            // `uri` accepts a text literal and text accepts a uri, for the
            // reason `id` does: a string literal has no way to announce
            // which of the string-shaped types it meant, and an address IS
            // a string. What it does not get is the *validation* for free
            // — `ordo_api::bind` checks the shape of a caller's value at
            // the door, which is where a value the compiler never saw
            // arrives.
            (Duration(_), Int) | (Id, Text) | (Text, Id) | (Uri, Text) | (Text, Uri)
        );
    if ok {
        Ok(coerce(value, declared))
    } else {
        Err(err(
            "param_type_mismatch",
            span,
            format!("expected `{}`, got `{}`", declared.spelled(), got.spelled()),
        ))
    }
}

/// Fit a value to a slot when either side is `json` (28b).
///
/// A `json` slot takes a `json` value (a parameter or a field), or a string
/// literal holding a JSON document: that is how a caller's document rides
/// the call (`ordo_host::json_source_literal`), and it is checked here so a
/// value that is not JSON never reaches a `jsonb` column as a 500. A `json`
/// value lands in no other slot: a document is not a text, and nothing
/// converts one implicitly.
fn fit_json(value: BExpr, declared: ScalarTy, span: Span) -> Result<BExpr, SemaError> {
    let got = value.ty();
    if declared == ScalarTy::Json && got == ScalarTy::Json {
        return Ok(value);
    }
    if declared == ScalarTy::Json {
        if let BExpr::Lit {
            value: Expr::StrLit { value: text, .. },
            ..
        } = &value
        {
            if serde_json::from_str::<serde_json::Value>(text).is_err() {
                return Err(err(
                    "param_type_mismatch",
                    span,
                    "expected `json`, and this string is not a JSON document",
                ));
            }
            return Ok(coerce(value, ScalarTy::Json));
        }
    }
    Err(err(
        "param_type_mismatch",
        span,
        format!(
            "expected `{}`, got `{}`: a `json` document and other values never convert \
             into each other",
            declared.spelled(),
            got.spelled()
        ),
    ))
}

fn coerce(e: BExpr, ty: ScalarTy) -> BExpr {
    match e {
        BExpr::Lit { value, .. } => BExpr::Lit { value, ty },
        other => other,
    }
}

// ── money and decimal: places, and the conversions between them ──────────
//
// Ruled 2026-10-04 (27ak). A `money(p)` value is whole minor units in every
// venue — the column, the SQL, the command log — and is written in major
// units in source and on the wire. So the compiler converts exactly where a
// value crosses between the two, and the conversions are ordinary bound
// arithmetic the planner already knows how to print:
//
// - a numeric literal meeting `money(p)` becomes whole minor units
//   (`29.99` → `2999`), refused as `implicit_rounding` in a typed slot when
//   it is finer than `p`;
// - an `int` meeting `money(p)` additively, in a comparison or in a slot is
//   `int * 10^p` (`price > qty` compares `price` with `qty` dollars);
// - a `money(p)` meeting a `decimal` is widened to major units,
//   `price * 0.01`, so `price * 0.08` is 8% of the price in dollars and
//   `round(x, 2)` rounds to the cent.
//
// At places 0 every one of these is the identity, so a program written
// before places existed plans the SQL it always planned.

/// `money_places_mismatch`: two `money` values with different places met.
///
/// Its own class, the way `duration_unit_mismatch` is: different places
/// almost always means different currencies, and the compiler cannot know
/// the exchange rate. Converting is the author's to write.
fn places_mismatch(a: ScalarTy, b: ScalarTy, span: Span) -> SemaError {
    err(
        "money_places_mismatch",
        span,
        format!(
            "`{}` meets `{}` — money with different places is usually different \
             currencies, so nothing converts between them on its own. Widen one to \
             a decimal and round it back (`round(x * 1.0, 2)`) if the conversion is \
             meant",
            a.spelled(),
            b.spelled()
        ),
    )
}

fn is_numeric_lit(e: &BExpr) -> bool {
    matches!(
        e,
        BExpr::Lit {
            value: Expr::IntLit { .. } | Expr::DecimalLit { .. },
            ty: ScalarTy::Int | ScalarTy::Decimal(_),
        }
    )
}

fn is_round_call(e: &BExpr) -> bool {
    matches!(e, BExpr::Fn { name, .. } if name == "round")
}

fn lit_span(e: &BExpr) -> Option<Span> {
    match e {
        BExpr::Lit { value, .. } => Some(value.span()),
        _ => None,
    }
}

/// A source literal (major units) as whole minor units of `money(p)`.
fn money_lit(e: BExpr, p: u8, span: Span) -> Result<BExpr, SemaError> {
    let BExpr::Lit { value, .. } = &e else {
        return Ok(e);
    };
    let at = lit_span(&e).unwrap_or(span);
    match ordo_syntax::numeric::literal_minor_units(value, p) {
        Ok(minor) => Ok(BExpr::Lit {
            value: Expr::IntLit {
                value: i128::from(minor),
                span: at,
            },
            ty: ScalarTy::Money(p),
        }),
        Err(ordo_syntax::numeric::Fit::TooFine { written }) => Err(err(
            "implicit_rounding",
            span,
            format!(
                "`{}` has {written} places and `money({p})` holds {p} — storing it would \
                 round, and nothing rounds implicitly. Write it to the minor unit, or \
                 declare the field with the places it needs",
                lit_text(value)
            ),
        )),
        Err(ordo_syntax::numeric::Fit::Overflow) => Err(err(
            "param_type_mismatch",
            span,
            format!(
                "`{}` is too large for `money({p})`, which holds whole minor units in a \
                 64-bit integer",
                lit_text(value)
            ),
        )),
        Err(ordo_syntax::numeric::Fit::NotNumeric) => Ok(e),
    }
}

/// A source literal fitted to a `decimal(p)` slot: kept as written when it
/// carries no more than `p` significant places, refused otherwise.
fn decimal_lit(e: BExpr, p: u8, span: Span) -> Result<BExpr, SemaError> {
    let BExpr::Lit { value, .. } = &e else {
        return Ok(e);
    };
    let text = lit_text(value);
    match ordo_syntax::numeric::significant_places(&text) {
        Some(s) if s <= u32::from(p) => Ok(BExpr::Lit {
            value: value.clone(),
            ty: ScalarTy::Decimal(p),
        }),
        Some(s) => Err(err(
            "implicit_rounding",
            span,
            format!(
                "`{text}` has {s} places and `decimal({p})` holds {p} — storing it would \
                 round, and nothing rounds implicitly"
            ),
        )),
        None => Ok(e),
    }
}

fn lit_text(e: &Expr) -> String {
    match e {
        Expr::IntLit { value, .. } => value.to_string(),
        Expr::DecimalLit { digits, .. } => digits.clone(),
        _ => String::new(),
    }
}

fn pow10(p: u8) -> i128 {
    10i128.pow(u32::from(p))
}

/// An `int` value in a `money(p)` position: `x * 10^p`. The identity at
/// places 0, so nothing written before places existed changes shape.
fn scale_to_money(e: BExpr, p: u8) -> BExpr {
    if p == 0 {
        return match e {
            // `round(x)` at places 0 is already whole units.
            BExpr::Fn { name, args, .. } if name == "round" && args.len() == 1 => BExpr::Fn {
                name,
                args,
                ty: ScalarTy::Money(0),
            },
            other => coerce(other, ScalarTy::Money(0)),
        };
    }
    let factor = BExpr::Lit {
        value: Expr::IntLit {
            value: pow10(p),
            span: Span::new(0, 0, 0, 0),
        },
        ty: ScalarTy::Int,
    };
    let scaled_ty = match e.ty() {
        ScalarTy::Decimal(n) => ScalarTy::Decimal(n),
        _ => ScalarTy::Money(p),
    };
    let product = BExpr::Binary {
        op: BinOp::Mul,
        lhs: Box::new(e),
        rhs: Box::new(factor),
        ty: scaled_ty,
    };
    match scaled_ty {
        // `round(x, 2) * 100` is whole, but `numeric`: one more `round`
        // makes it the `bigint` a money column holds, and rounds nothing.
        ScalarTy::Decimal(_) => BExpr::Fn {
            name: "round".into(),
            args: vec![product],
            ty: ScalarTy::Money(p),
        },
        _ => product,
    }
}

/// A `money(p)` value read in major units, as a decimal: `x * 0.01` at
/// places 2. Exact — a multiplication by a power of ten moves the point
/// and rounds nothing. Every other value is returned as it is.
fn widen(e: BExpr) -> BExpr {
    let ScalarTy::Money(p) = e.ty() else {
        return e;
    };
    if p == 0 {
        return e;
    }
    if let BExpr::Lit {
        value: Expr::IntLit { value, span },
        ..
    } = &e
    {
        return BExpr::Lit {
            value: Expr::DecimalLit {
                digits: ordo_syntax::numeric::major_spelling(*value, p),
                span: *span,
            },
            ty: ScalarTy::Decimal(p),
        };
    }
    let unit = BExpr::Lit {
        value: Expr::DecimalLit {
            digits: ordo_syntax::numeric::major_spelling(1, p),
            span: Span::new(0, 0, 0, 0),
        },
        ty: ScalarTy::Decimal(p),
    };
    BExpr::Binary {
        op: BinOp::Mul,
        lhs: Box::new(e),
        rhs: Box::new(unit),
        ty: ScalarTy::Decimal(p),
    }
}

/// [`widen`], and a `money(0)` retyped as the `decimal(0)` it reads as.
/// `widen` leaves places 0 alone because the identity needs no scaling;
/// where the result must be a decimal, the multiplication by `1` is what
/// makes it one, in the IR and in SQL (`bigint * numeric`).
fn as_decimal(e: BExpr) -> BExpr {
    if e.ty() != ScalarTy::Money(0) {
        return widen(e);
    }
    BExpr::Binary {
        op: BinOp::Mul,
        lhs: Box::new(e),
        rhs: Box::new(BExpr::Lit {
            value: Expr::DecimalLit {
                digits: "1".into(),
                span: Span::new(0, 0, 0, 0),
            },
            ty: ScalarTy::Decimal(0),
        }),
        ty: ScalarTy::Decimal(0),
    }
}

/// Bring a `money(p)` and a numeric operand to one footing, for the places
/// a value is compared, added, subtracted or chosen between. See
/// [`money_footing_all`].
fn unify_money(l: BExpr, r: BExpr, span: Span) -> Result<(BExpr, BExpr), SemaError> {
    let mut v = money_footing_all(vec![l, r], span)?;
    let r = v.pop().expect("two in, two out");
    let l = v.pop().expect("two in, two out");
    Ok((l, r))
}

/// Bring every value of a group — two operands, the arms of a `cond`, the
/// arguments of `least`, a value and its `in` list — to one footing when
/// any of them is `money(p)`.
///
/// - Two moneys must agree on places (`money_places_mismatch`).
/// - If every other value is an `int` or a literal that fits `p`, they all
///   become `money(p)`: a literal exactly (`29.99` → `2999`), an `int`
///   expression scaled by `10^p`. The money side keeps its column, so a
///   filter like `price > 29.99` still reads the index.
/// - Otherwise a decimal is involved (an expression, or a literal finer
///   than `p`), and every money widens to major units. A comparison is not
///   a typed slot, so `price > 29.999` compares exactly instead of being
///   refused (DL §2, "Comparisons are not typed slots").
fn money_footing_all(vals: Vec<BExpr>, span: Span) -> Result<Vec<BExpr>, SemaError> {
    use ScalarTy::*;
    let Some(p) = vals.iter().find_map(|v| match v.ty() {
        Money(p) => Some(p),
        _ => None,
    }) else {
        return Ok(vals);
    };
    for v in &vals {
        if let Money(q) = v.ty() {
            if q != p {
                return Err(places_mismatch(Money(p), Money(q), span));
            }
        }
    }
    let stays_money = vals.iter().all(|v| match v.ty() {
        Decimal(_) => match v {
            BExpr::Lit { value, .. } if is_numeric_lit(v) => {
                ordo_syntax::numeric::literal_minor_units(value, p).is_ok()
            }
            _ => false,
        },
        _ => true,
    });
    vals.into_iter()
        .map(|v| {
            if !stays_money {
                return Ok(widen(v));
            }
            match v.ty() {
                Int | Decimal(_) if is_numeric_lit(&v) => money_lit(v, p, span),
                Int => Ok(scale_to_money(v, p)),
                _ => Ok(v),
            }
        })
        .collect()
}

/// The footing an arithmetic operator needs before [`binary_ty`] types it.
///
/// `+` and `-` add like to like ([`unify_money`]). `*` scales: a money
/// times an `int` is a money and the `int` is a count, so it is not scaled;
/// a money times a decimal, or two moneys, is a decimal in major units.
/// `/` is always a decimal (DL §2, Division), and the ratio of two moneys
/// at one place count is the same in minor and major units, so only a
/// money divided by something else widens.
fn arith_footing(op: BinOp, l: BExpr, r: BExpr, span: Span) -> Result<(BExpr, BExpr), SemaError> {
    use ScalarTy::*;
    match (op, l.ty(), r.ty()) {
        (BinOp::Add, Text, _) | (BinOp::Add, _, Text) => Ok((l, r)),
        (BinOp::Add | BinOp::Sub, _, _) => unify_money(l, r, span),
        // Two moneys multiply to a decimal at every places value, `money(0)`
        // included (ruled 2026-10-05): one rule, so a square of yen lands
        // in a money slot no more than a square of cents does.
        (BinOp::Mul, Money(_), Money(_)) => Ok((as_decimal(l), as_decimal(r))),
        (BinOp::Mul, Money(_), Decimal(_)) | (BinOp::Mul, Decimal(_), Money(_)) => {
            Ok((widen(l), widen(r)))
        }
        (BinOp::Div, Money(a), Money(b)) if a == b => Ok((l, r)),
        (BinOp::Div, _, _) => Ok((widen(l), widen(r))),
        _ => Ok((l, r)),
    }
}

/// The footing an expression function's arguments need (27ak).
///
/// - `least`, `greatest`, `coalesce` choose one of their arguments, so the
///   arguments share a footing ([`money_footing_all`]).
/// - `mod(price, 5)` is the remainder in dollars: the divisor is scaled to
///   the dividend's places, and two moneys must agree on them.
/// - `round`, `floor` and `ceil` of a `money(p)` work in major units:
///   `floor(29.99)` is 29 dollars, never 2999 cents. The argument widens to
///   a decimal, and the result lands back in a money slot by the slot's
///   own rule.
fn fn_footing(name: &str, mut args: Vec<BExpr>, span: Span) -> Result<Vec<BExpr>, SemaError> {
    match name {
        "least" | "greatest" | "coalesce" => money_footing_all(args, span),
        "mod" if args.len() == 2 => {
            let r = args.pop().expect("two");
            let l = args.pop().expect("two");
            let (l, r) = match (l.ty(), r.ty()) {
                (ScalarTy::Money(_), ScalarTy::Int) | (ScalarTy::Money(_), ScalarTy::Money(_)) => {
                    unify_money(l, r, span)?
                }
                _ => (l, r),
            };
            Ok(vec![l, r])
        }
        "round" | "floor" | "ceil" if !args.is_empty() => {
            let first = args.remove(0);
            args.insert(0, widen(first));
            Ok(args)
        }
        _ => Ok(args),
    }
}

/// Fold a leading `-` into an `int` or `decimal` literal, so `-2500` binds
/// as the value `-2500` names rather than as the negation of `2500`.
///
/// The grammar reads a leading `-` as `unary := '-' unary`
/// (`ordo-syntax/src/parser.rs`) and builds `Expr::Neg` uniformly for every
/// operand, which is right for an expression and wrong for a literal
/// parameter value: [`lit_ty`] answers only for the literal productions, so
/// `$total: -2500` was `invalid_binding_expr` — "bound to something that is
/// not a value" — and every negative `int`, `money` or `decimal` a caller
/// could send was unbindable, including through
/// `ordo_host::FixtureExecutor::call_source`, which renders a negative
/// amount exactly this way.
///
/// This is the **one place** a call's named arguments become the parameter
/// environment, on both sides that need it: [`const_expr`]'s fallback arm
/// (the read path — an op call's arguments, `param_env_with`) and
/// [`write_value_expr`]'s environment loop (the write path). Before this it
/// was folded a second time, independently, in
/// `ordo_conformance::write` — which had no way to know the read path had
/// the identical gap, because the read path did not fold at all. Both call
/// through here now, so a negative parameter means the same thing wherever
/// it is bound, by one rule read once rather than by two copies that could
/// drift.
///
/// It is a **strict widening**: nothing that bound before binds
/// differently, because `Neg` never bound at all. Only the two numeric
/// literal productions fold — `-$x` and `-(a + b)` stay `Expr::Neg` and stay
/// expressions, so a write's right-hand side rules for those
/// ([`WriteValue`]) and the general expression binder's own `Neg` handling
/// (`Binder::expr`) are never bypassed by a parameter binding admitting more
/// than a signed numeric literal.
pub fn fold_neg_lit(e: &Expr) -> Expr {
    match e {
        Expr::Neg { expr, span } => match expr.as_ref() {
            Expr::IntLit { value, .. } => Expr::IntLit {
                value: -value,
                span: *span,
            },
            Expr::DecimalLit { digits, .. } => Expr::DecimalLit {
                digits: format!("-{digits}"),
                span: *span,
            },
            _ => e.clone(),
        },
        _ => e.clone(),
    }
}

fn lit_ty(e: &Expr) -> Option<ScalarTy> {
    Some(match e {
        Expr::IntLit { .. } => ScalarTy::Int,
        // A decimal literal is a number with the places it was written
        // with. It becomes money only where it meets a `money(p)` — see
        // `fit_slot` and `unify_money` — and is converted there.
        Expr::DecimalLit { digits, .. } => ScalarTy::Decimal(
            digits
                .split_once('.')
                .map_or(0, |(_, f)| u8::try_from(f.len()).unwrap_or(u8::MAX)),
        ),
        Expr::StrLit { .. } => ScalarTy::Text,
        Expr::DateLit { .. } => ScalarTy::Date,
        Expr::DatetimeLit { .. } => ScalarTy::Datetime,
        Expr::DurationLit { unit, .. } => ScalarTy::Duration(*unit),
        Expr::BoolLit { .. } => ScalarTy::Bool,
        Expr::NullLit { .. } => ScalarTy::Text,
        _ => return None,
    })
}

/// Result typing for arithmetic. Still thin — full type checking is P3's —
/// but division is no longer among the thin parts.
///
/// `/` **changes type** (DL §2, Division; `glossary.md`, "`decimal` vs
/// `money`"). `money` and `int` are both minor-unit `bigint`, so typing `/`
/// by its operands makes the planner emit `bigint / bigint`, which is
/// truncating integer division: `total / qty` then answers with a *wrong
/// number*, not an error. The result type is `decimal` at a fixed
/// fractional scale of 12, and the scale is a property of the RESULT, not
/// of the operands — which is what makes the same expression mean the same
/// thing in every plan and every deployment, host-side or in SQL.
///
/// Decimal is then contagious through `+ - *`: those stay exact, but an
/// exact operation on a scale-12 value is a scale-12 value, and pretending
/// otherwise would let a division launder itself back into a money slot.
fn binary_ty(op: BinOp, l: ScalarTy, r: ScalarTy) -> ScalarTy {
    use ScalarTy::*;
    match (op, l, r) {
        // A blob has no arithmetic, and this arm is here so that the
        // trailing `_ => l` cannot answer for it. Before this arm existed
        // the wildcard typed `blob + blob` as `blob`, which is the shape a
        // wrong answer takes when a wildcard is asked a question nobody
        // decided: not an error, a plausible type.
        //
        // The refusal itself is `check_arg_types`' — an expression that
        // reaches here has already been type-checked — so what this arm
        // owes is a value that cannot be mistaken for a working one.
        // `Text` would let a blob launder itself into a text slot;
        // `Decimal` cannot be stored anywhere without an explicit
        // conversion the author has to write, which makes any path that
        // reaches it stop.
        (_, Blob, _) | (_, _, Blob) => Decimal(DIVISION_SCALE),
        (BinOp::Add, Text, _) | (BinOp::Add, _, Text) => Text,
        (BinOp::Div, _, _) => Decimal(DIVISION_SCALE),
        // The scale is the most places the result can carry: a product
        // adds them, a sum keeps the larger. A money operand here is
        // `money(0)` beside an `int` (a count) or a decimal; any other has
        // been widened by `arith_footing`, and two moneys always are.
        (BinOp::Mul, Decimal(a), Decimal(b)) => Decimal(a.saturating_add(b)),
        (_, Decimal(a), Decimal(b)) => Decimal(a.max(b)),
        (_, Decimal(a), _) | (_, _, Decimal(a)) => Decimal(a),
        (_, Money(p), _) | (_, _, Money(p)) => Money(p),
        (_, Date, _) => Date,
        (_, Datetime, _) => Datetime,
        _ => l,
    }
}

/// Do `round`/`floor`/`ceil` convert this decimal to an integral value?
///
/// These three are the *only* decimal → storage conversions (DL §2, "No
/// implicit rounding, anywhere"). `floor(x)`, `ceil(x)` and bare `round(x)`
/// always land on a whole number. `round(x, n)` lands on one only when `n`
/// is zero. `round(x, 2)` is a decimal with at most two places: it lands in
/// a `money(2)` or a `decimal(2)` slot, where `fit_slot` scales it exactly,
/// and in no integral slot, where calling it converted would put the
/// truncation back one layer down (27ak).
///
/// A non-literal `n` counts as unconverted: the answer has to be decidable
/// at compile time, and a parameter is not.
fn rounds_to_integral(name: &str, args: &[BExpr]) -> bool {
    match name {
        "floor" | "ceil" => true,
        "round" => match args.get(1) {
            None => true,
            Some(BExpr::Lit {
                value: ordo_syntax::Expr::IntLit { value, .. },
                ..
            }) => *value == 0,
            Some(_) => false,
        },
        _ => false,
    }
}

/// The declared domain of every expression function, checklist 9v.
///
/// `None` means the function accepts any scalar and constrains only the
/// relationship between its arguments (see [`check_arg_types`]).
///
/// **`Blob` appears in no domain here and does not need to**:
/// [`check_arg_types`] refuses it by type before it consults this table,
/// precisely because `None` means "any scalar" and a per-function
/// enumeration would have to be extended by whoever adds the next
/// function. `Uri` appears in `TEXTUAL` and `HASHABLE` below, because an
/// address *is* text — `lower(link)` and `hash(link)` are meaningful, and
/// `hash` promises stability over bytes the language pins, which it does
/// for a URI exactly as it does for a `text`.
fn fn_domain(name: &str, position: usize) -> Option<&'static [ScalarTy]> {
    use ScalarTy::*;
    const NUMERIC: &[ScalarTy] = &[Int, Money(0), Decimal(DIVISION_SCALE)];
    const TEMPORAL: &[ScalarTy] = &[Date, Datetime];
    const TEXTUAL: &[ScalarTy] = &[Text, Uri];
    const INTEGER: &[ScalarTy] = &[Int];
    /// Whole-number types: `money` is minor units, so it counts.
    const INTEGRAL: &[ScalarTy] = &[Int, Money(0)];
    // G12 pinned this one explicitly: bit-stability is a promise about
    // BYTES, and `text`/`id` are the only types whose bytes the language
    // pins. Hashing a money would promise stability over a representation
    // the spec has not fixed.
    //
    // `uri` joins them and nothing else does: an address is a string whose
    // bytes the language pins exactly as it pins a `text`'s (NFC, stored
    // form), so `hash(link)` is as stable as `hash(name)`. That is not
    // true of a `blob`, whose handle is fresh randomness per upload —
    // hashing one would promise stability over a value that is different
    // every time the same file is uploaded.
    const HASHABLE: &[ScalarTy] = &[Text, Id, Uri];

    match (name, position) {
        ("year" | "month" | "week" | "day" | "hour" | "dow", 0) => Some(TEMPORAL),
        ("abs" | "floor" | "ceil", 0) => Some(NUMERIC),
        ("round", 0) => Some(NUMERIC),
        // `round(x, n)`'s second argument is a digit count, not a value.
        ("round", 1) => Some(INTEGER),
        // DL §3: "`mod(a, b)` (integer; truncated division, sign follows the
        // dividend)". "Integer" there describes the DIVISION'S SEMANTICS —
        // truncated, sign follows the dividend — and does not exclude
        // `money`, which IS an integer count of minor units in this
        // platform. `mod(price, 100)` is the cents part and is meaningful,
        // so refusing it would be a false positive on working code (it was:
        // `decimal.rs` asks exactly this and the first draft of this table
        // broke it). `decimal` stays out, because truncated integer
        // division over a fractional value is not what that sentence
        // describes.
        ("mod", _) => Some(INTEGRAL),
        ("lower" | "trim" | "length", 0) => Some(TEXTUAL),
        ("hash", 0) => Some(HASHABLE),
        _ => None,
    }
}

/// Refuse an expression function applied to an argument outside its domain.
///
/// # Why this is `argument_type_mismatch` and not an existing class
///
/// The compile inventory's `param_type_mismatch` is *"binding expression's
/// type ≠ declared parameter type"* — it is about **composition**, a caller
/// binding a value into a callee's declared parameter. `invalid_binding_expr`
/// is about a **write's right-hand side**. Neither is about applying a
/// function to a value, and stretching either would make the class name
/// stop predicting where to look. So the closed inventory gains one member,
/// deliberately and with this reasoning recorded — which is what adding to a
/// closed set is supposed to cost.
///
/// # Why it is worth catching here at all
///
/// It was already caught: `lower(price)` plans, emits SQL Postgres refuses,
/// and F6's shape verification fails the op at LOAD. That is a real gate and
/// it is why `hash()` was restorable without inventing anything. But it is
/// decent by accident — the operator is told the database would not describe
/// a statement, not that an author applied `lower` to a money, and the op
/// that fails to load may be one nobody edited. Binding is where the author's
/// line is still in hand.
///
/// # What it deliberately does not do
///
/// It does not refuse what the language's own conversions allow: `round`,
/// `floor` and `ceil` take the whole numeric tower because `Decimal` is
/// expression-only and flows into `Int`/`Money` by the rules DL §2 pins, and
/// widening between them is not a mistake. And it constrains `least`,
/// `greatest` and `coalesce` only by AGREEMENT rather than by a domain,
/// because a common type is the whole of what they require — a rule about
/// the relationship, which a per-position domain table cannot express.
fn check_arg_types(name: &str, args: &[BExpr], span: Span) -> Result<(), SemaError> {
    for (i, a) in args.iter().enumerate() {
        // **No expression function accepts a `blob`, whatever its domain
        // says.** `fn_domain` answers `None` for the functions that
        // constrain only the relationship between their arguments
        // (`least`, `greatest`, `coalesce`) and for anything not in the
        // table, and `None` means "any scalar" — so before this check
        // existed, `coalesce(f, g)` over two blobs was admitted by
        // default, as was every function added to the language after the
        // domain table was written.
        //
        // A blob is opaque content behind a keyed-random handle. There is
        // no function of it that means anything: `length` would measure
        // the handle, `hash` would hash randomness, `coalesce` would pick
        // between two objects by nullness — which is the one case that
        // would even *work*, and is exactly why the refusal has to be by
        // type rather than by enumerating functions. Its `mime` and
        // `size` are ordinary values and every function applies to them.
        if a.ty().is_content() {
            return Err(err(
                "argument_type_mismatch",
                span,
                format!(
                    "rule: a `blob` is opaque content behind a keyed-random handle, so no \
                     expression function takes one — `{name}` was applied to one in \
                     argument {}.\n\
                     fix: apply it to the blob's metadata instead — `f.mime` and `f.size` \
                     are ordinary values",
                    i + 1
                ),
            ));
        }
        refuse_json(a, span, &format!("`{name}`"))?;
        let Some(domain) = fn_domain(name, i) else {
            continue;
        };
        let ty = a.ty();
        // By kind, not by value: a domain names `money` and means every
        // `money(p)`, and `decimal` at every scale.
        if !domain.iter().any(|d| d.as_str() == ty.as_str()) {
            let want: Vec<String> = domain
                .iter()
                .map(|t| format!("{t:?}").to_lowercase())
                .collect();
            return Err(err(
                "argument_type_mismatch",
                span,
                format!(
                    "`{name}` does not accept {} in argument {} — it takes {}",
                    format!("{ty:?}").to_lowercase(),
                    i + 1,
                    want.join(" or ")
                ),
            ));
        }
    }

    // `least`, `greatest` and `coalesce` say nothing about WHICH type; they
    // require one type. Two arguments that disagree have no answer, and
    // Postgres would either coerce silently or refuse at load.
    if matches!(name, "least" | "greatest" | "coalesce") {
        if let (Some(a), Some(b)) = (args.first(), args.get(1)) {
            let (x, y) = (a.ty(), b.ty());
            // The numeric tower agrees with itself: `least(price, 0)` is an
            // int beside a money and is exactly the comparison the author
            // meant. Refusing it would be a false positive, which is the
            // failure mode that breaks working programs.
            let numeric = |t: ScalarTy| {
                matches!(t, ScalarTy::Int | ScalarTy::Money(_) | ScalarTy::Decimal(_))
            };
            if x != y && !(numeric(x) && numeric(y)) {
                return Err(err(
                    "argument_type_mismatch",
                    span,
                    format!(
                        "`{name}` needs one type in both arguments, and got {} beside {}",
                        format!("{x:?}").to_lowercase(),
                        format!("{y:?}").to_lowercase()
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn fn_ty(name: &str, args: &[BExpr]) -> ScalarTy {
    // The conversions come first: over a decimal they are what makes the
    // value storable, and over anything else they stay the identity they
    // always were (`floor` of a money is a money).
    if matches!(name, "round" | "floor" | "ceil")
        && matches!(args.first().map(|a| a.ty()), Some(ScalarTy::Decimal(_)))
    {
        return if rounds_to_integral(name, args) {
            ScalarTy::Int
        } else {
            // `round(x, 2)` carries at most two places, which is what lets
            // it land in a `money(2)` or a `decimal(2)` slot (27ak).
            match args.get(1) {
                Some(BExpr::Lit {
                    value: Expr::IntLit { value, .. },
                    ..
                }) => ScalarTy::Decimal(u8::try_from(*value).unwrap_or(DIVISION_SCALE)),
                _ => ScalarTy::Decimal(DIVISION_SCALE),
            }
        };
    }
    match name {
        // DL ~617: date parts are BUCKETING functions returning sortable
        // scalars — month(d) is '2026-07', not the number 7. The EXTRACT
        // lowering was a real bug C051 would have caught.
        // `hash` is `int` by DL §3 ("stable 64-bit non-cryptographic hash →
        // int"), and by G12's ruling it is a NON-NEGATIVE int: the top bit
        // is cleared so `mod(hash(id), n)` lands in `[0, n)` rather than
        // straddling zero.
        "year" | "dow" | "length" | "hash" => ScalarTy::Int,
        "month" | "week" | "hour" => ScalarTy::Text,
        "day" => ScalarTy::Date,
        "lower" | "trim" => ScalarTy::Text,
        _ => args.first().map(|a| a.ty()).unwrap_or(ScalarTy::Int),
    }
}
