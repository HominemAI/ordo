//! **The AST → builder-call emitter**, and with it the isomorphism proof.
//!
//! # Why this is honest
//!
//! Track C's bar is that "isomorphism is a conformance-tested property, not an
//! aspiration": builder output ≡ hand-written `.ordo`, **both directions**. A
//! hand-written set of builder programs would test the direction that is easy
//! and prove nothing about the other — it would only ever exercise the nodes
//! whoever wrote it thought of.
//!
//! This module machine-translates a *parsed* program into the builder calls
//! that reconstruct it, matching **exhaustively** on every AST enum. So:
//!
//! - a node kind with no builder spelling **fails `cargo build`** (no arm), or
//!   fails loudly by name (an arm that returns `Err`), and the corpus report
//!   lists it as a miss rather than skipping it;
//! - a node kind that *is* reachable in the corpus is emitted, run through
//!   `synth`, reparsed, and compared against the original tree — so the
//!   builder's spelling of it is checked against the parser's, not against
//!   somebody's memory of the grammar.
//!
//! The other direction — everything the builder can express parses — is the
//! same test read backwards: every method the generator emits ends up in the
//! synthesized text of some corpus program, and that text is parsed.
//!
//! # What it is not
//!
//! Not a decompiler. It starts from an AST this repository's own parser
//! produced, so it never has to guess; `ordo-decompile` is the crate that
//! guesses, and it guesses from SQL.

use std::collections::BTreeMap;

use ordo_syntax::decl::{
    AggDecl, CheckDecl, EntityDecl, FieldDecl, FieldKind, GuardAtom, GuardClause, GuardExpr,
    LetDecl, MatchKind, OpBody, OpDecl, ParamDecl, Pipeline, ProjItem, ReassignTarget, ReleaseDecl,
    ReleaseEffect, ReleaseMode, RoutineDecl, RoutineTrigger, ScalarTy, Source, SourceArg, Step,
    TakeCount, WriteStmt,
};
use ordo_syntax::{AggShape, CondArm, Condition, Expr, Program};
use ordo_syntax::{AGGREGATE_SHAPES, ALLOCATE_MODES};

use crate::spell;
use crate::ts;

/// Everything the emitter may need to import, in the order the barrel exports
/// it. Only the names that appear in the emitted body are imported, because
/// the package compiles under `noUnusedLocals`.
const IMPORTABLE: &[&str] = &[
    "agg",
    "and",
    "as_",
    "bare",
    "cond",
    "daily",
    "dateLit",
    "datetimeLit",
    "decLit",
    "desc",
    "asc",
    "domain",
    "durationLit",
    "each",
    "entity",
    "every",
    "f",
    "fn",
    "g",
    "lit",
    "minus",
    "named",
    "not",
    "nul",
    "op",
    "or",
    "param",
    "path",
    "rel",
    "release",
    "routine",
    "src",
    "star",
    "synth",
    "t",
    "when",
];

/// Emit a TypeScript module that rebuilds `program` through the builder.
///
/// `import_from` is the module specifier the fixture imports the package
/// under — a relative path inside the package's own tests, the package name
/// outside it.
pub fn emit_program(program: &Program, import_from: &str) -> Result<String, String> {
    // The CDK has no `secret` builder yet (`plan-backend-functions.md` §12,
    // increment 9). Refused, so a round trip never drops a binding.
    if let Some(s) = program.secrets.first() {
        return Err(format!(
            "`secret {}` has no CDK form yet; keep secret declarations in the .ordo text",
            s.name.name
        ));
    }
    let mut e = Emitter::default();
    let mut body = String::new();
    let mut synth_args: Vec<String> = Vec::new();

    if let Some(d) = &program.domain {
        synth_args.push(match &d.version {
            Some(v) => format!("domain({}, {})", ts::str_lit(&d.name.name), ts::str_lit(v)),
            None => format!("domain({})", ts::str_lit(&d.name.name)),
        });
    }
    for ent in &program.entities {
        let bind = e.bind(&ent.name.name);
        body.push_str(&format!("const {bind} = {};\n\n", e.entity(ent)?));
        synth_args.push(bind);
    }
    for op in &program.ops {
        let bind = e.bind(&op.name.name);
        body.push_str(&format!("const {bind} = {};\n\n", e.op(op)?));
        synth_args.push(bind);
    }
    for r in &program.routines {
        let bind = e.bind(&r.name.name);
        body.push_str(&format!("const {bind} = {};\n\n", e.routine(r)?));
        synth_args.push(bind);
    }
    if let Some(r) = &program.release {
        synth_args.push(e.release(r)?);
    }

    let used: Vec<&str> = IMPORTABLE
        .iter()
        .copied()
        .filter(|n| mentions(&body, n) || synth_args.iter().any(|a| mentions(a, n)))
        .chain(std::iter::once("synth"))
        .collect();
    let mut imports: Vec<&str> = used;
    imports.sort_unstable();
    imports.dedup();

    Ok(format!(
        "{}import {{ {} }} from {};\n\n{body}export const ordo: string = synth(\n{}\n);\n",
        ts::header(
            "One corpus program, rebuilt through the builder.",
            "the program's own parsed AST, translated by `ordo_gen::emit`",
        ),
        imports.join(", "),
        ts::str_lit(import_from),
        synth_args
            .iter()
            .map(|a| format!("  {a},"))
            .collect::<Vec<_>>()
            .join("\n"),
    ))
}

/// Whole-word mention, so `not` does not match inside `notes`.
fn mentions(hay: &str, needle: &str) -> bool {
    let bytes = hay.as_bytes();
    let n = needle.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$';
    let mut i = 0;
    while let Some(pos) = hay[i..].find(needle) {
        let start = i + pos;
        let end = start + n.len();
        // A name after `.` is a method (`release.member.when(`), never an import.
        let before_ok = start == 0 || (!word(bytes[start - 1]) && bytes[start - 1] != b'.');
        let after_ok = end == bytes.len() || !word(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        i = start + 1;
        if i >= hay.len() {
            break;
        }
    }
    false
}

#[derive(Default)]
struct Emitter {
    used: BTreeMap<String, usize>,
    /// The parameters the op currently being emitted declares.
    ///
    /// `$caller` is the reason this field exists. It is an `Expr::Param` like
    /// any other, and no op declares it: it is the ambient principal, which
    /// `guards-conformance.md` treats as a parameter the door binds. So `p.`
    /// — the typed namespace built from the op's own list — cannot reach it,
    /// and the emitter spells any undeclared parameter `param('name')`
    /// instead. Getting this wrong produced `p.caller`, which is a TypeScript
    /// error rather than a silent one; it is written down here because the
    /// NEXT ambient parameter will look exactly as surprising.
    declared: std::collections::BTreeSet<String>,
}

impl Emitter {
    fn bind(&mut self, name: &str) -> String {
        let base = ts::binding(name);
        let n = self.used.entry(base.clone()).or_insert(0);
        *n += 1;
        if *n == 1 {
            base
        } else {
            format!("{base}_{n}")
        }
    }

    // ── shapes ───────────────────────────────────────────────────────────

    fn entity(&self, e: &EntityDecl) -> Result<String, String> {
        let fields: Vec<String> = e
            .fields
            .iter()
            .map(|f| self.field(f))
            .collect::<Result<_, _>>()?;
        let opts = match &e.key {
            Some(k) => format!(", {{ key: {} }}", ts::str_lit(&k.name)),
            None => String::new(),
        };
        let mut s = format!(
            "entity({}, {{\n{}\n}}{opts})",
            ts::str_lit(&e.name.name),
            fields
                .iter()
                .map(|f| format!("  {f},"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        for u in &e.unique_keys {
            s.push_str(&format!(
                "\n  .unique({})",
                u.iter()
                    .map(|i| ts::str_lit(&i.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for i in &e.indexes {
            s.push_str(&format!(
                "\n  .index({})",
                i.iter()
                    .map(|x| ts::str_lit(&x.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for c in &e.checks {
            let CheckDecl { cond, .. } = c;
            s.push_str(&format!("\n  .check({})", self.cond(cond)?));
        }
        if let Some(levels) = &e.acl {
            s.push_str(&format!(
                "\n  .acl({})",
                levels
                    .iter()
                    .map(|l| ts::str_lit(&l.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(mark) = &e.public {
            s.push_str(if mark.cascade {
                "\n  .publicAll()"
            } else {
                "\n  .public()"
            });
        }
        if let Some(gr) = &e.access_read {
            s.push_str(&format!("\n  .accessRead({})", self.guard_clause(gr)?));
        }
        if let Some(gw) = &e.access_write {
            s.push_str(&format!("\n  .accessWrite({})", self.guard_clause(gw)?));
        }
        Ok(s)
    }

    fn field(&self, f: &FieldDecl) -> Result<String, String> {
        let mut s = match &f.kind {
            // The target is always the string spelling, never the binding of
            // an already-declared entity: a tree self-edge references the
            // entity being declared, so a binding would be a use-before-init
            // in half the schemas that have one.
            FieldKind::Ref { target, .. } => format!("t.ref({})", ts::str_lit(&target.name)),
            FieldKind::Scalar(ty) => self.scalar_ctor(*ty)?,
        };
        if f.optional {
            s.push_str(".optional()");
        }
        if let FieldKind::Ref { owned, tree, .. } = &f.kind {
            if *owned {
                s.push_str(".owned()");
            }
            if *tree {
                s.push_str(".tree()");
            }
        }
        if f.unique {
            s.push_str(".unique()");
        }
        if f.sealed {
            s.push_str(".sealed()");
        }
        if f.indexed {
            s.push_str(".indexed()");
        }
        if f.public {
            s.push_str(".public()");
        }
        if f.refinable {
            s.push_str(".refinable()");
        }
        if f.inline {
            s.push_str(".inline()");
        }
        if f.private {
            s.push_str(".private()");
        }
        if let Some(n) = &f.normalize {
            s.push_str(&format!(".normalize({})", ts::str_lit(&n.name)));
        }
        if let Some(d) = &f.default {
            s.push_str(&format!(".default({})", self.expr(d)?));
        }
        Ok(format!("{}: {s}", ts::key(&f.name.name)))
    }

    /// `t.text()`, `t.duration('ms')` — and a loud refusal for the one type
    /// that has no declaration form.
    fn scalar_ctor(&self, ty: ScalarTy) -> Result<String, String> {
        let sc = spell::scalar(ty);
        let Some(ctor) = sc.ctor else {
            return Err(format!(
                "`{}` has no declaration form (DL §2 marks it expression-only), so \
                 the builder has no constructor for it",
                ty.as_str()
            ));
        };
        Ok(match ty {
            ScalarTy::Duration(u) => {
                format!("t.{ctor}({})", ts::str_lit(spell::duration_unit(u)))
            }
            // `money(2)`: the places are part of the type (27ak). Bare
            // `money` is `money(0)` and is spelled bare, as the source
            // spells it.
            ScalarTy::Money(0) => format!("t.{ctor}()"),
            ScalarTy::Money(p) | ScalarTy::Decimal(p) => format!("t.{ctor}({p})"),
            ScalarTy::Text
            | ScalarTy::Int
            | ScalarTy::Bool
            | ScalarTy::Date
            | ScalarTy::Datetime
            | ScalarTy::Id
            // `blob` takes no constructor argument. `inline` is a
            // *modifier* — `t.blob().inline()` — because DL §2 says the
            // custody tiers and `inline` compose orthogonally, and a
            // constructor argument would make one of them a parameter of
            // the other.
            | ScalarTy::Blob
            | ScalarTy::Uri
            | ScalarTy::Json => format!("t.{ctor}()"),
        })
    }

    // ── guards ───────────────────────────────────────────────────────────

    fn guard_clause(&self, c: &GuardClause) -> Result<String, String> {
        Ok(match c {
            GuardClause::Open { .. } => "g.OPEN".to_string(),
            GuardClause::Walk(w) => format!("g.walk({})", self.guard_expr(w)?),
        })
    }

    fn guard_expr(&self, w: &GuardExpr) -> Result<String, String> {
        let disjuncts: Vec<String> = w
            .disjuncts
            .iter()
            .map(|d| {
                let atoms: Vec<String> = d
                    .iter()
                    .map(|a| self.guard_atom(a))
                    .collect::<Result<_, _>>()?;
                Ok::<String, String>(format!("g.all({})", atoms.join(", ")))
            })
            .collect::<Result<_, _>>()?;
        Ok(format!("g.any({})", disjuncts.join(", ")))
    }

    fn guard_atom(&self, a: &GuardAtom) -> Result<String, String> {
        Ok(match a {
            GuardAtom::Cmp {
                path, caller, lit, ..
            } => {
                let p = ts::str_lit(&dotted(path));
                match (caller, lit) {
                    (true, _) => format!("g.eq({p})"),
                    (false, Some(e)) => format!("g.eq({p}, {})", self.expr(e)?),
                    (false, None) => {
                        return Err("a guard comparison with neither `$caller` nor a literal".into())
                    }
                }
            }
            GuardAtom::Bare { path, .. } => format!("g.bare({})", ts::str_lit(&dotted(path))),
            GuardAtom::Step { path, inner, .. } => format!(
                "g.step({}, {})",
                ts::str_lit(&dotted(path)),
                self.guard_expr(inner)?
            ),
        })
    }

    // ── release ──────────────────────────────────────────────────────────

    fn release(&self, r: &ReleaseDecl) -> Result<String, String> {
        let effects: Vec<String> = r
            .effects
            .iter()
            .map(|e| self.release_effect(e))
            .collect::<Result<_, _>>()?;
        let tail = if effects.is_empty() {
            String::new()
        } else {
            format!("[{}]", effects.join(", "))
        };
        Ok(match &r.mode {
            ReleaseMode::Open { .. } => format!("release.member.open({tail})"),
            ReleaseMode::Locked { .. } => format!("release.member.locked({tail})"),
            ReleaseMode::When(c) => {
                let disjuncts: Vec<String> = c
                    .disjuncts
                    .iter()
                    .map(|d| {
                        let atoms: Vec<String> = d
                            .iter()
                            .map(|a| {
                                Ok::<String, String>(format!(
                                    "rel.{}({}, {})",
                                    if a.negated { "none" } else { "has" },
                                    ts::str_lit(&a.entity.name),
                                    self.guard_expr(&a.inner)?
                                ))
                            })
                            .collect::<Result<_, _>>()?;
                        Ok::<String, String>(format!("rel.all({})", atoms.join(", ")))
                    })
                    .collect::<Result<_, _>>()?;
                let cond = format!("rel.any({})", disjuncts.join(", "));
                if tail.is_empty() {
                    format!("release.member.when({cond})")
                } else {
                    format!("release.member.when({cond}, {tail})")
                }
            }
        })
    }

    fn release_effect(&self, e: &ReleaseEffect) -> Result<String, String> {
        Ok(match e {
            ReleaseEffect::Remove { entity, field, .. } => format!(
                "rel.remove({})",
                ts::str_lit(&format!("{}.{}", entity.name, field.name))
            ),
            ReleaseEffect::Reassign {
                entity, field, to, ..
            } => {
                let targets: Vec<String> = to
                    .iter()
                    .map(|t| {
                        Ok::<String, String>(match t {
                            ReassignTarget::Owner { .. } => "rel.owner".to_string(),
                            ReassignTarget::Lookup {
                                entity,
                                inner,
                                field,
                                ..
                            } => format!(
                                "rel.lookup({}, {}, {})",
                                ts::str_lit(&entity.name),
                                self.guard_expr(inner)?,
                                ts::str_lit(&field.name)
                            ),
                        })
                    })
                    .collect::<Result<_, _>>()?;
                format!(
                    "rel.reassign({}, {})",
                    ts::str_lit(&format!("{}.{}", entity.name, field.name)),
                    targets.join(", ")
                )
            }
        })
    }

    // ── ops ──────────────────────────────────────────────────────────────

    fn op(&mut self, o: &OpDecl) -> Result<String, String> {
        self.declared = o.params.iter().map(|p| p.name.name.clone()).collect();
        let params: Vec<String> = o
            .params
            .iter()
            .map(|p| self.param(p))
            .collect::<Result<_, _>>()?;
        let params_obj = if params.is_empty() {
            "{}".to_string()
        } else {
            format!("{{ {} }}", params.join(", "))
        };
        let body = match &o.body {
            OpBody::Read(pipe) => self.pipeline(pipe, "b")?,
            OpBody::Write(stmts) => {
                let out: Vec<String> = stmts
                    .iter()
                    .map(|w| self.write(w))
                    .collect::<Result<_, _>>()?;
                format!("[\n    {},\n  ]", out.join(",\n    "))
            }
        };
        // `p` is named even when unused; TypeScript's `noUnusedParameters`
        // ignores an unused parameter that is not followed by a used one, and
        // a body with no parameters never mentions it.
        let head = if o.params.is_empty() { "(b)" } else { "(b, p)" };
        let mut s = format!(
            "op({}, {params_obj}, {head} =>\n  {body})",
            ts::str_lit(&o.name.name)
        );
        if o.flags.unbounded {
            s.push_str("\n  .unbounded()");
        }
        if o.flags.unguarded {
            s.push_str("\n  .unguarded()");
        }
        if o.flags.reproducible {
            s.push_str("\n  .reproducible()");
        }
        if o.flags.nopause {
            s.push_str("\n  .nopause()");
        }
        if o.flags.public {
            s.push_str("\n  .public()");
        }
        if o.flags.download {
            s.push_str("\n  .download()");
        }
        Ok(s)
    }

    // ── routines ─────────────────────────────────────────────────────────

    fn routine(&mut self, r: &RoutineDecl) -> Result<String, String> {
        if r.notify.is_some() {
            return Err(format!(
                "routine `{}` says `notify`, which the CDK cannot spell yet; keep it in `.ordo`",
                r.name.name
            ));
        }
        self.declared = r.params.iter().map(|p| p.name.name.clone()).collect();
        let trigger = match &r.trigger {
            RoutineTrigger::Clock(schedule) => {
                let text = schedule.render();
                match text.strip_prefix("every day ") {
                    Some(time) => format!("daily({})", ts::str_lit(time)),
                    None => format!(
                        "every({})",
                        ts::str_lit(text.strip_prefix("every ").unwrap_or(&text))
                    ),
                }
            }
            RoutineTrigger::Change { pattern, modifiers } => {
                let mut opts: Vec<String> = Vec::new();
                let rendered = modifiers.render();
                let mut words = rendered.split_whitespace();
                while let Some(w) = words.next() {
                    match w {
                        "debounce" => {
                            let d = words.next().unwrap_or_default();
                            opts.push(format!("debounce: {}", ts::str_lit(d)));
                        }
                        "coalesce" => opts.push("coalesce: true".to_string()),
                        other => return Err(format!("unknown trigger modifier `{other}`")),
                    }
                }
                if opts.is_empty() {
                    format!("when({})", ts::str_lit(&pattern.render()))
                } else {
                    format!(
                        "when({}, {{ {} }})",
                        ts::str_lit(&pattern.render()),
                        opts.join(", ")
                    )
                }
            }
        };
        let mut opts = Vec::new();
        if let Some(d) = &r.domain {
            opts.push(format!("in: {}", ts::str_lit(&d.name)));
        }
        opts.push(format!("trigger: {trigger}"));
        if !r.params.is_empty() {
            let params: Vec<String> = r
                .params
                .iter()
                .map(|p| self.param(p))
                .collect::<Result<_, _>>()?;
            opts.push(format!("params: {{ {} }}", params.join(", ")));
        }
        let stmts: Vec<String> = r
            .body
            .iter()
            .map(|w| self.write(w))
            .collect::<Result<_, _>>()?;
        let head = if r.params.is_empty() { "(b)" } else { "(b, p)" };
        let mut s = match &r.each {
            None => format!(
                "routine({}, {{ {} }}, {head} => [\n    {},\n  ])",
                ts::str_lit(&r.name.name),
                opts.join(", "),
                stmts.join(",\n    ")
            ),
            Some(pipe) => format!(
                "routine({}, {{ {} }}, {head} =>\n  each({}, [\n    {},\n  ]))",
                ts::str_lit(&r.name.name),
                opts.join(", "),
                self.pipeline(pipe, "b")?,
                stmts.join(",\n    ")
            ),
        };
        if r.flags.unbounded {
            s.push_str("\n  .unbounded()");
        }
        if r.flags.unguarded {
            s.push_str("\n  .unguarded()");
        }
        if r.flags.reproducible {
            s.push_str("\n  .reproducible()");
        }
        if r.flags.nopause {
            s.push_str("\n  .nopause()");
        }
        Ok(s)
    }

    fn param(&self, p: &ParamDecl) -> Result<String, String> {
        let mut s = self.scalar_ctor(p.ty)?;
        if let Some(m) = &p.max {
            s.push_str(&format!(".max({})", self.expr(m)?));
        }
        if let Some(d) = &p.default {
            s.push_str(&format!(".default({})", self.expr(d)?));
        }
        Ok(format!("{}: {s}", ts::key(&p.name.name)))
    }

    fn write(&self, w: &WriteStmt) -> Result<String, String> {
        Ok(match w {
            WriteStmt::Set {
                binding,
                entity,
                cond,
                fields,
                ..
            } => {
                let f: Vec<String> = fields
                    .iter()
                    .map(|(k, v)| Ok(format!("{}: {}", ts::key(&k.name), self.expr(v)?)))
                    .collect::<Result<_, String>>()?;
                let mut s = format!(
                    "b.set({}, {{ {} }})",
                    ts::str_lit(&entity.name),
                    f.join(", ")
                );
                if let Some(c) = cond {
                    s.push_str(&format!(".where({})", self.cond(c)?));
                }
                if let Some(b) = binding {
                    s.push_str(&format!(".let_({})", ts::str_lit(&b.name)));
                }
                s
            }
            WriteStmt::Del {
                entity,
                cond,
                subtree,
                ..
            } => {
                let mut s = format!("b.del({}, {})", ts::str_lit(&entity.name), self.cond(cond)?);
                if *subtree {
                    s.push_str(".subtree()");
                }
                s
            }
        })
    }

    // ── pipelines ────────────────────────────────────────────────────────

    fn pipeline(&self, p: &Pipeline, base: &str) -> Result<String, String> {
        let mut head = base.to_string();
        for (i, l) in p.lets.iter().enumerate() {
            let LetDecl { name, pipeline, .. } = l;
            let inner_base = format!("b{}", i + 1);
            head = format!(
                "{head}\n    .let_({}, ({inner_base}) => {})",
                ts::str_lit(&name.name),
                self.pipeline(pipeline, &inner_base)?
            );
        }
        let sources: Vec<String> = p
            .sources
            .iter()
            .map(|s| self.source(s))
            .collect::<Result<_, _>>()?;
        let mut out = format!("{head}\n    .source({})", sources.join(", "));
        for s in &p.steps {
            out.push_str(&format!("\n    {}", self.step(s)?));
        }
        Ok(out)
    }

    fn source(&self, s: &Source) -> Result<String, String> {
        let mut out = format!("src({})", ts::str_lit(&s.name.name));
        if !s.args.is_empty() {
            let args: Vec<String> = s
                .args
                .iter()
                .map(|a| match a {
                    SourceArg::Named { name, value, .. } => Ok(format!(
                        "named({}, {})",
                        ts::str_lit(&name.name),
                        self.expr(value)?
                    )),
                    SourceArg::Cond(c) => self.cond(c),
                })
                .collect::<Result<_, String>>()?;
            out.push_str(&format!(".args({})", args.join(", ")));
        }
        if let Some(proj) = &s.projection {
            let items: Vec<String> = proj
                .iter()
                .map(|i| self.proj_item(i))
                .collect::<Result<_, _>>()?;
            out.push_str(&format!(".pick({})", items.join(", ")));
        }
        Ok(out)
    }

    fn proj_item(&self, i: &ProjItem) -> Result<String, String> {
        Ok(match i {
            ProjItem::Star { .. } => "star()".to_string(),
            ProjItem::Exclude { name, .. } => format!("minus({})", ts::str_lit(&name.name)),
            ProjItem::Field { path, .. } => ts::str_lit(&dotted(path)),
            ProjItem::Named { out, value, .. } => {
                format!("as_({}, {})", ts::str_lit(&out.name), self.expr(value)?)
            }
        })
    }

    fn step(&self, s: &Step) -> Result<String, String> {
        Ok(match s {
            Step::Filter { cond, .. } => format!(".where({})", self.cond(cond)?),
            Step::Sort { keys, .. } => {
                let ks: Vec<String> = keys
                    .iter()
                    .map(|k| {
                        Ok(format!(
                            "{}({})",
                            if k.desc { "desc" } else { "asc" },
                            self.expr(&k.expr)?
                        ))
                    })
                    .collect::<Result<_, String>>()?;
                format!(".sort({})", ks.join(", "))
            }
            Step::Take { keys, n, .. } => {
                let count = match n {
                    TakeCount::Lit { n, .. } => int_arg(i128::from(*n)),
                    TakeCount::Param { name } => {
                        if self.declared.contains(&name.name) {
                            format!("p{}", ts::access(&name.name))
                        } else {
                            format!("param({})", ts::str_lit(&name.name))
                        }
                    }
                };
                if keys.is_empty() {
                    format!(".take({count})")
                } else {
                    let ks: Vec<String> = keys
                        .iter()
                        .map(|k| self.expr(k))
                        .collect::<Result<_, String>>()?;
                    format!(".take([{}], {count})", ks.join(", "))
                }
            }
            Step::Group { keys, aggs, .. } => {
                let ks: Vec<String> = keys
                    .iter()
                    .map(|k| self.expr(k))
                    .collect::<Result<_, String>>()?;
                format!(".group([{}], {})", ks.join(", "), self.aggs(aggs)?)
            }
            Step::Reduce { aggs, .. } => format!(".reduce({})", self.aggs(aggs)?),
            Step::Distinct { .. } => ".distinct()".to_string(),
            Step::Match { kind, .. } => match kind {
                MatchKind::Any(s) => format!(".matchAny({})", self.source(s)?),
                MatchKind::None(s) => format!(".matchNone({})", self.source(s)?),
                MatchKind::Ref { path } => {
                    format!(".matchRef({})", ts::str_lit(&dotted(path)))
                }
                MatchKind::On { col } => format!(".matchOn({})", ts::str_lit(&col.name)),
            },
            Step::Append { source, .. } => format!(".append({})", self.source(source)?),
        })
    }

    fn aggs(&self, aggs: &[AggDecl]) -> Result<String, String> {
        let mut seen = std::collections::BTreeSet::new();
        let mut items = Vec::new();
        for a in aggs {
            if !seen.insert(a.out.name.clone()) {
                // The builder keys aggregates by their output name, exactly
                // as `group customer { … }` does, so two aggregates with one
                // name are not representable. The language does not refuse
                // them at parse time; sema is where that belongs. Reported by
                // name rather than silently dropping the second.
                return Err(format!(
                    "two aggregates share the output name `{}`; the builder keys them \
                     by that name, so the pair is not representable",
                    a.out.name
                ));
            }
            items.push(format!("{}: {}", ts::key(&a.out.name), self.agg(a)?));
        }
        Ok(format!("{{ {} }}", items.join(", ")))
    }

    fn agg(&self, a: &AggDecl) -> Result<String, String> {
        let name = a.func.name.as_str();
        let Some(&(_, shape)) = AGGREGATE_SHAPES.iter().find(|(n, _)| *n == name) else {
            return Err(format!("`{name}` is not an aggregate"));
        };
        Ok(match shape {
            AggShape::NoneOrCondition => match &a.cond {
                Some(c) => format!("agg{}({})", ts::access(name), self.cond(c)?),
                None => format!("agg{}()", ts::access(name)),
            },
            AggShape::OneExpr => {
                let arg = a
                    .arg
                    .as_ref()
                    .ok_or("an aggregate is missing its argument")?;
                format!("agg{}({})", ts::access(name), self.expr(arg)?)
            }
            AggShape::ExprAndFraction => {
                let arg = a.arg.as_ref().ok_or("percentile is missing its field")?;
                let Some(Expr::DecimalLit { digits, .. }) = a.arg2.as_ref() else {
                    return Err("percentile's fraction is a decimal literal".into());
                };
                format!(
                    "agg{}({}, {})",
                    ts::access(name),
                    self.expr(arg)?,
                    ts::str_lit(digits)
                )
            }
            AggShape::ExprAndSeparator => {
                let arg = a.arg.as_ref().ok_or("joined is missing its field")?;
                let Some(Expr::StrLit { value, .. }) = a.arg2.as_ref() else {
                    return Err("joined's separator is a text literal".into());
                };
                format!(
                    "agg{}({}, {})",
                    ts::access(name),
                    self.expr(arg)?,
                    ts::str_lit(value)
                )
            }
        })
    }

    // ── conditions ───────────────────────────────────────────────────────

    fn cond(&self, c: &Condition) -> Result<String, String> {
        Ok(match c {
            Condition::Compare { op, lhs, rhs, .. } => format!(
                "{}.{}({})",
                self.expr(lhs)?,
                spell::cmp(*op),
                self.expr(rhs)?
            ),
            Condition::In { expr, list, .. } => {
                let items: Vec<String> = list
                    .iter()
                    .map(|e| self.expr(e))
                    .collect::<Result<_, String>>()?;
                format!("{}.in({})", self.expr(expr)?, items.join(", "))
            }
            Condition::Text { op, lhs, rhs, .. } => format!(
                "{}.{}({})",
                self.expr(lhs)?,
                spell::text_op(*op),
                self.expr(rhs)?
            ),
            Condition::IsNull { expr, negated, .. } => format!(
                "{}.{}()",
                self.expr(expr)?,
                if *negated { "isNotNull" } else { "isNull" }
            ),
            Condition::Since { expr, dur, .. } => {
                format!("{}.since({})", self.expr(expr)?, self.expr(dur)?)
            }
            Condition::Under { expr, id, .. } => {
                format!("{}.under({})", self.expr(expr)?, self.expr(id)?)
            }
            Condition::Above { expr, id, .. } => {
                format!("{}.above({})", self.expr(expr)?, self.expr(id)?)
            }
            Condition::BareBool(e) => format!("bare({})", self.expr(e)?),
            Condition::And { items, .. } => {
                let parts: Vec<String> = items
                    .iter()
                    .map(|i| self.cond(i))
                    .collect::<Result<_, String>>()?;
                format!("and({})", parts.join(", "))
            }
            Condition::Or { items, .. } => {
                let parts: Vec<String> = items
                    .iter()
                    .map(|i| self.cond(i))
                    .collect::<Result<_, String>>()?;
                format!("or({})", parts.join(", "))
            }
            Condition::Not { inner, .. } => format!("not({})", self.cond(inner)?),
        })
    }

    // ── expressions ──────────────────────────────────────────────────────

    fn expr(&self, e: &Expr) -> Result<String, String> {
        Ok(match e {
            Expr::IntLit { value, .. } => format!("lit({})", int_arg(*value)),
            Expr::DecimalLit { digits, .. } => format!("decLit({})", ts::str_lit(digits)),
            Expr::StrLit { value, .. } => format!("lit({})", ts::str_lit(value)),
            Expr::DateLit { y, m, d, .. } => {
                format!("dateLit({})", ts::str_lit(&format!("{y:04}-{m:02}-{d:02}")))
            }
            Expr::DatetimeLit {
                y,
                m,
                d,
                hh,
                mm,
                ss,
                ..
            } => format!(
                "datetimeLit({})",
                ts::str_lit(&format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z"))
            ),
            Expr::DurationLit { n, unit, .. } => format!(
                "durationLit({n}, {})",
                ts::str_lit(spell::duration_unit(*unit))
            ),
            Expr::BoolLit { value, .. } => format!("lit({value})"),
            Expr::NullLit { .. } => "nul()".to_string(),
            Expr::Param { name, .. } => {
                if self.declared.contains(name) {
                    format!("p{}", ts::access(name))
                } else {
                    format!("param({})", ts::str_lit(name))
                }
            }
            Expr::Path { segments, .. } => self.path(segments),
            Expr::Neg { expr, .. } => format!("{}.neg()", self.expr(expr)?),
            Expr::Binary { op, lhs, rhs, .. } => format!(
                "{}.{}({})",
                self.expr(lhs)?,
                spell::bin_op(*op),
                self.expr(rhs)?
            ),
            Expr::FnCall { name, args, .. } => self.fn_call(&name.name, args)?,
            Expr::Cond { arms, else_, .. } => {
                let a: Vec<String> = arms
                    .iter()
                    .map(|CondArm { condition, value }| {
                        Ok(format!(
                            "[{}, {}]",
                            self.cond(condition)?,
                            self.expr(value)?
                        ))
                    })
                    .collect::<Result<_, String>>()?;
                format!("cond([{}], {})", a.join(", "), self.expr(else_)?)
            }
        })
    }

    /// `f.customer.name`, or `path('…')` when a segment is a method name.
    fn path(&self, segments: &[ordo_syntax::Ident]) -> String {
        let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
        let safe = names.iter().all(|n| {
            !n.is_empty()
                && n.chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !is_value_method(n)
        });
        if safe {
            format!("f.{}", names.join("."))
        } else {
            format!(
                "path({})",
                names
                    .iter()
                    .map(|n| ts::str_lit(n))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }

    fn fn_call(&self, name: &str, args: &[Expr]) -> Result<String, String> {
        // `allocate`'s third argument is a bare mode word, not an expression;
        // the builder types it as the closed union, so the emitter passes the
        // word rather than a value.
        if name == "allocate" && args.len() == 3 {
            let Expr::Path { segments, .. } = &args[2] else {
                return Err("`allocate`'s third argument is a mode name".into());
            };
            let mode = dotted(segments);
            if !ALLOCATE_MODES.contains(&mode.as_str()) {
                return Err(format!("unknown allocation mode `{mode}`"));
            }
            return Ok(format!(
                "fn.allocate({}, {}, {})",
                self.expr(&args[0])?,
                self.expr(&args[1])?,
                ts::str_lit(&mode)
            ));
        }
        let a: Vec<String> = args
            .iter()
            .map(|x| self.expr(x))
            .collect::<Result<_, String>>()?;
        Ok(format!("fn{}({})", ts::access(name), a.join(", ")))
    }
}

/// The method names `f.` must not shadow — the same list the generator writes
/// into `lang.ts`, derived from the same tables.
fn is_value_method(name: &str) -> bool {
    spell::ALL_CMP.iter().any(|o| spell::cmp(*o) == name)
        || spell::ALL_TEXT_OPS
            .iter()
            .any(|o| spell::text_op(*o) == name)
        || spell::ALL_BIN_OPS.iter().any(|o| spell::bin_op(*o) == name)
        || matches!(
            name,
            "neg" | "in" | "isNull" | "isNotNull" | "since" | "under" | "above" | "node"
        )
}

fn dotted(path: &[ordo_syntax::Ident]) -> String {
    path.iter()
        .map(|i| i.name.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

/// An integer literal argument, exact.
///
/// A `number` above 2^53 would arrive at `lit()` already rounded, so anything
/// outside the safe range is emitted as a bigint. The builder refuses an
/// unsafe `number` at run time as well; both halves are needed, because only
/// one of them is present when a human writes the call.
fn int_arg(v: i128) -> String {
    const SAFE: i128 = 9_007_199_254_740_991;
    if v.abs() <= SAFE {
        v.to_string()
    } else {
        format!("{v}n")
    }
}

/// The unused-import guard, in Rust: a name in [`IMPORTABLE`] that the
/// emitter can never produce would ride into every fixture and fail `tsc`.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_boundaries_are_respected() {
        assert!(mentions("not(f.x)", "not"));
        assert!(!mentions("f.notes", "not"));
        assert!(mentions("a, f, b", "f"));
        assert!(!mentions("fn.year(x)", "f"));
    }

    #[test]
    fn large_integers_do_not_round() {
        assert_eq!(int_arg(3), "3");
        assert_eq!(int_arg(9_007_199_254_740_993), "9007199254740993n");
    }
}
