//! 14f — `@hominemai/ordo-cdk`, the typed TypeScript builder.
//!
//! The **second consumer of the same generator**, and the reason the
//! architecture is generator-first: the SDK is a function of the descriptor,
//! the CDK's language layer is a function of the closed tables in
//! `ordo-syntax`, and both are regenerated rather than rewritten when either
//! moves.
//!
//! # Two layers, two sources
//!
//! - **The language layer** (`vocabulary.ts`, `lang.ts`) comes from the
//!   parser's own tables: `Keyword`, `ScalarTy`, `CmpOp`, `TextOp`, `BinOp`,
//!   `DurationUnit`, `functions()`, `AGGREGATE_SHAPES`, `ALLOCATE_MODES`. It
//!   is the same for every deployment, because the language is.
//! - **The schema layer** (`schema.ts`) comes from one deployment's
//!   `interface.json`: the entity and op names that may be sources, and the
//!   `f.` field accessors, typed and following refs.
//!
//! # The manifest is the ratchet crossing the language boundary
//!
//! `vocabulary.ts` lists **every reserved word** with either the name it has
//! in TypeScript or the reason it has none, straight out of
//! [`crate::spell::keyword`]. `clients/ordo-cdk/test/vocabulary.test.ts` then
//! walks that list and asserts every `offered` name really resolves on the
//! package's surface.
//!
//! That is what carries the exhaustive-match ratchet the whole way. Adding a
//! keyword to `token.rs` fails `cargo build` (no arm in `spell.rs`); giving it
//! an `Offered` spelling and no implementation fails `npm run check` (no such
//! method). Only a word that is either genuinely absent, with a reason, or
//! genuinely implemented, can get through both.

use ordo_syntax::token::{DurationUnit, Keyword};
use ordo_syntax::{functions, AggShape, AGGREGATE_SHAPES, ALLOCATE_MODES};
use serde_json::Value;

use crate::spell;
use crate::ts::{self, GenFile};

const LANG_SOURCE: &str = "the parser's own closed tables in `crates/ordo-syntax` \
                           (`Keyword`, `ScalarTy`, `CmpOp`, `TextOp`, `BinOp`, \
                           `DurationUnit`, `functions()`, `AGGREGATE_SHAPES`, \
                           `ALLOCATE_MODES`)";
const SCHEMA_SOURCE: &str = "the deployment's `interface.json` (`ordo.interface/2`)";

/// The language layer: the same bytes for every deployment.
pub fn language_files(out_dir: &str) -> Vec<GenFile> {
    vec![
        GenFile {
            path: format!("{out_dir}/vocabulary.ts"),
            contents: vocabulary_file(),
        },
        GenFile {
            path: format!("{out_dir}/lang.ts"),
            contents: lang_file(),
        },
    ]
}

/// The schema layer, plus the barrel that re-exports both.
pub fn schema_files(doc: &Value, out_dir: &str) -> Result<Vec<GenFile>, String> {
    let entities = doc["entities"]
        .as_array()
        .ok_or("interface.json has no entities")?;
    let ops = doc["ops"].as_array().ok_or("interface.json has no ops")?;
    Ok(vec![
        GenFile {
            path: format!("{out_dir}/schema.ts"),
            contents: schema_file(doc, entities, ops),
        },
        GenFile {
            path: format!("{out_dir}/index.ts"),
            contents: index_file(),
        },
    ])
}

// ── vocabulary.ts ────────────────────────────────────────────────────────

fn vocabulary_file() -> String {
    let mut s = ts::header(
        "The language's closed vocabularies, and what each word is called here.",
        LANG_SOURCE,
    );
    s.push_str(&ts::doc(&[
        "One reserved word, and its TypeScript spelling — or the reason it has none.".to_string(),
        String::new(),
        "`tag` is `offered` (an author types `name`), `fused` (the word is \
         reachable only inside the spelling `name` gives), `does-not-parse` \
         (the LANGUAGE refuses the construct, so the builder must too — an \
         absent method is the language's refusal arriving one layer earlier), \
         or `out-of-scope` (this increment does not cover it, and the README \
         and the checklist both say so)."
            .to_string(),
    ]));
    s.push_str(
        "export interface Word {\n  \
         readonly word: string;\n  \
         readonly tag: 'offered' | 'fused' | 'does-not-parse' | 'out-of-scope';\n  \
         readonly note: string;\n}\n\n",
    );

    s.push_str(&ts::doc(&[
        "Every reserved word in the language, in `token.rs` order.".to_string(),
        String::new(),
        "The list is complete by construction: the generator matches `Keyword` \
         exhaustively with no wildcard arm, so a word added to the lexer fails \
         `cargo build` until someone decides what it is called here. \
         `test/vocabulary.test.ts` then checks the other half — that every \
         `offered` name really resolves on this package's surface."
            .to_string(),
    ]));
    s.push_str("export const KEYWORDS: readonly Word[] = [\n");
    for &k in Keyword::ALL {
        let sp = spell::keyword(k);
        s.push_str(&format!(
            "  {{ word: {}, tag: {}, note: {} }},\n",
            ts::str_lit(k.as_str()),
            ts::str_lit(sp.tag()),
            ts::str_lit(sp.note()),
        ));
    }
    s.push_str("] as const;\n\n");

    // The five borrowed closed sets, as data. A consumer that wants to render
    // a palette or validate a string reads these rather than retyping them,
    // which is the same argument `editor_grammar.rs` makes for the grammar.
    s.push_str(&ts::doc(&[
        "The duration units — `ms s m h d w mo y` and nothing else.".to_string(),
    ]));
    s.push_str(&format!(
        "export const DURATION_UNITS = [{}] as const;\n\n",
        DurationUnit::ALL
            .iter()
            .map(|u| ts::str_lit(spell::duration_unit(*u)))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    s.push_str(&ts::doc(&[
        "The allocation modes.".to_string(),
        String::new(),
        "`rotation` is deliberately absent: spreading the odd minor unit over \
         successive invoices has to remember what it did last time, which is \
         application state, not a function of `(total, weights)`."
            .to_string(),
    ]));
    s.push_str(&format!(
        "export const ALLOCATE_MODES = [{}] as const;\n\n",
        ALLOCATE_MODES
            .iter()
            .map(|m| ts::str_lit(m))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    s.push_str(&ts::doc(&[
        "The expression functions, with the arities the parser enforces.".to_string(),
    ]));
    s.push_str("export const FUNCTIONS = [\n");
    for (name, min, max) in functions() {
        s.push_str(&format!(
            "  {{ name: {}, min: {min}, max: {max} }},\n",
            ts::str_lit(name)
        ));
    }
    s.push_str("] as const;\n\n");
    s.push_str(&ts::doc(&[
        "The aggregates, with the argument shape `agg_decl` parses for each.".to_string(),
    ]));
    s.push_str("export const AGGREGATES = [\n");
    for (name, shape) in AGGREGATE_SHAPES {
        s.push_str(&format!(
            "  {{ name: {}, shape: {} }},\n",
            ts::str_lit(name),
            ts::str_lit(shape_tag(*shape))
        ));
    }
    s.push_str("] as const;\n\n");
    s.push_str(&ts::doc(&[
        "The comparison, text and arithmetic operators, with their spellings.".to_string(),
    ]));
    s.push_str("export const OPERATORS = {\n  compare: [\n");
    for op in spell::ALL_CMP {
        s.push_str(&format!(
            "    {{ method: {}, ordo: {} }},\n",
            ts::str_lit(spell::cmp(*op)),
            ts::str_lit(op.as_str())
        ));
    }
    s.push_str("  ],\n  text: [\n");
    for op in spell::ALL_TEXT_OPS {
        s.push_str(&format!(
            "    {{ method: {}, ordo: {} }},\n",
            ts::str_lit(spell::text_op(*op)),
            ts::str_lit(op.as_str())
        ));
    }
    s.push_str("  ],\n  arithmetic: [\n");
    for op in spell::ALL_BIN_OPS {
        s.push_str(&format!(
            "    {{ method: {}, ordo: {} }},\n",
            ts::str_lit(spell::bin_op(*op)),
            ts::str_lit(op.as_str())
        ));
    }
    s.push_str("  ],\n} as const;\n\n");
    s.push_str(&ts::doc(&[
        "The scalar types that have a DECLARATION form, with their constructors.".to_string(),
        String::new(),
        "`money(p)` and `decimal(p)` carry their places (27ak): `t.money(2)` is \
         an amount in cents, `t.decimal(4)` a rate or a unit price stored \
         exactly. A bare `money` is `money(0)`. This list is generated from \
         the same match every other type comes out of, so a type the parser \
         gains or loses cannot be missed here by accident."
            .to_string(),
    ]));
    s.push_str("export const SCALARS = [\n");
    for (t, sc) in spell::declarable_scalars() {
        let ctor = sc.ctor.unwrap_or_default();
        s.push_str(&format!(
            "  {{ ordo: {}, ctor: {}, ts: {}, wire: {} }},\n",
            ts::str_lit(t.as_str()),
            ts::str_lit(ctor),
            ts::str_lit(sc.ts),
            ts::str_lit(sc.wire),
        ));
    }
    s.push_str("] as const;\n");
    s
}

fn shape_tag(s: AggShape) -> &'static str {
    match s {
        AggShape::NoneOrCondition => "none-or-condition",
        AggShape::OneExpr => "one-expr",
        AggShape::ExprAndFraction => "expr-and-fraction",
        AggShape::ExprAndSeparator => "expr-and-separator",
    }
}

// ── lang.ts ──────────────────────────────────────────────────────────────

/// The builder's language surface: `Val`'s methods, `t`, `fn`, `agg`, the
/// literal constructors and the condition combinators.
///
/// Every name in it is a projection of a table in `ordo-syntax`. The *shapes*
/// it emits — that a comparison is a method on a value taking one operand,
/// that `t.duration` takes its unit — are the skeleton's, stated here once and
/// filled with whatever the tables say.
fn lang_file() -> String {
    let mut s = ts::header(
        "The builder's language vocabulary: values, types, functions, aggregates.",
        LANG_SOURCE,
    );
    s.push_str(
        "import type { AggNode, CondNode, ExprNode } from '../core/ir.js';\n\
         import { TypeSpec, ref } from '../core/build.js';\n\
         import {\n  \
         Cond,\n  \
         installValue,\n  \
         intDigits,\n  \
         makeFields,\n  \
         makePath,\n  \
         reserveAccessors,\n  \
         ValueBase,\n  \
         type Value,\n\
         } from '../core/value.js';\n\n",
    );

    // The closed string unions. A wrong unit or mode is a compile error in
    // the author's own editor rather than a compile error in the engine after
    // a deploy.
    s.push_str(&format!(
        "/** `ms s m h d w mo y` — the closed unit set. */\nexport type DurationUnit = {};\n\n",
        DurationUnit::ALL
            .iter()
            .map(|u| ts::str_lit(spell::duration_unit(*u)))
            .collect::<Vec<_>>()
            .join(" | ")
    ));
    s.push_str(&format!(
        "/** `allocate`'s optional third argument. */\nexport type AllocateMode = {};\n\n",
        ALLOCATE_MODES
            .iter()
            .map(|m| ts::str_lit(m))
            .collect::<Vec<_>>()
            .join(" | ")
    ));

    // ── Val ──
    s.push_str(&ts::doc(&[
        "A value: an expression node with the language's operators on it.".to_string(),
        String::new(),
        "Every method below is generated from a closed enum in the parser, so \
         the builder has exactly the operators the language has. There is no \
         escape hatch into an arbitrary predicate, because there could not be \
         one that compiled."
            .to_string(),
    ]));
    s.push_str(
        "/**\n \
         * The vocabulary, merged into the skeleton's `Value`.\n \
         *\n \
         * `core/value.ts` declares `Value` with no members so that the whole \n \
         * skeleton — parameters, field paths, sort keys, write values — can be \n \
         * typed against it without naming one operator. This block is where the \n \
         * operators arrive, and it is generated from the parser's enums, so the \n \
         * two halves cannot disagree about what a value can do.\n \
         */\n\
         declare module '../core/value.js' {\n  \
         // eslint-disable-next-line @typescript-eslint/no-shadow\n  \
         interface Value {\n",
    );
    for op in spell::ALL_CMP {
        s.push_str(&format!(
            "    /** `a {} b` */\n    {}(other: Value): Cond;\n",
            op.as_str(),
            spell::cmp(*op),
        ));
    }
    for op in spell::ALL_TEXT_OPS {
        s.push_str(&format!(
            "    /** `a {} b` */\n    {}(other: Value): Cond;\n",
            op.as_str(),
            spell::text_op(*op),
        ));
    }
    for op in spell::ALL_BIN_OPS {
        s.push_str(&format!(
            "    /** `a {} b` */\n    {}(other: Value): Value;\n",
            op.as_str(),
            spell::bin_op(*op),
        ));
    }
    s.push_str(
        "    /** `-x` */\n    neg(): Value;\n    \
         /** `expr in ($a, $b)` */\n    in(...items: Value[]): Cond;\n    \
         /** `expr is null` */\n    isNull(): Cond;\n    \
         /** `expr is not null` */\n    isNotNull(): Cond;\n    \
         /** `expr since 90d` */\n    since(dur: Value): Cond;\n    \
         /** `expr under $id` */\n    under(id: Value): Cond;\n    \
         /** `expr above $id` */\n    above(id: Value): Cond;\n  \
         }\n}\n\n",
    );
    s.push_str("export class Val extends ValueBase implements Value {\n");
    for op in spell::ALL_CMP {
        s.push_str(&format!(
            "  /** `a {} b` */\n  {}(other: Value): Cond {{\n    \
             return new Cond({{ k: 'cmp', op: {}, l: this.node, r: other.node }});\n  }}\n\n",
            op.as_str(),
            spell::cmp(*op),
            ts::str_lit(op.as_str()),
        ));
    }
    for op in spell::ALL_TEXT_OPS {
        s.push_str(&format!(
            "  /** `a {} b` — matching stays in operators, never functions. */\n  \
             {}(other: Value): Cond {{\n    \
             return new Cond({{ k: 'text', op: {}, l: this.node, r: other.node }});\n  }}\n\n",
            op.as_str(),
            spell::text_op(*op),
            ts::str_lit(op.as_str()),
        ));
    }
    for op in spell::ALL_BIN_OPS {
        s.push_str(&format!(
            "  /** `a {} b` */\n  {}(other: Value): Value {{\n    \
             return new Val({{ k: 'bin', op: {}, l: this.node, r: other.node }});\n  }}\n\n",
            op.as_str(),
            spell::bin_op(*op),
            ts::str_lit(op.as_str()),
        ));
    }
    s.push_str(
        "  /** `-x`. */\n  \
         neg(): Value {\n    return new Val({ k: 'neg', e: this.node });\n  }\n\n\
         /** `expr in ($a, $b)` — membership over a parenthesized comma list. */\n  \
         in(...items: Value[]): Cond {\n    \
         return new Cond({ k: 'in', e: this.node, list: items.map((i) => i.node) });\n  }\n\n\
         /** `expr is null` — one of the only two null tests the language has. */\n  \
         isNull(): Cond {\n    \
         return new Cond({ k: 'isnull', e: this.node, negated: false });\n  }\n\n\
         /** `expr is not null` — the other. */\n  \
         isNotNull(): Cond {\n    \
         return new Cond({ k: 'isnull', e: this.node, negated: true });\n  }\n\n\
         /** `expr since 90d` — a trailing window of execution time. */\n  \
         since(dur: Value): Cond {\n    \
         return new Cond({ k: 'since', e: this.node, dur: dur.node });\n  }\n\n\
         /** `expr under $id` — tree entities only. */\n  \
         under(id: Value): Cond {\n    \
         return new Cond({ k: 'under', e: this.node, id: id.node });\n  }\n\n\
         /** `expr above $id` — tree entities only. */\n  \
         above(id: Value): Cond {\n    \
         return new Cond({ k: 'above', e: this.node, id: id.node });\n  }\n}\n\n\
         // The run-time half of the seam: every path and parameter the skeleton\n\
         // mints from here on is built with this constructor, so a value typed\n\
         // as having `.eq()` really has it.\n\
         installValue(Val);\n\n",
    );

    // ── the accessor reservation ──
    s.push_str(&ts::doc(&[
        "The method names `f.…` must not shadow.".to_string(),
        String::new(),
        "A field genuinely called `eq` or `in` is unreachable through the \
         proxy and is spelled `path('eq')` instead. The list is generated \
         from the same tables the methods are, so it can never be short."
            .to_string(),
    ]));
    s.push_str("export const VALUE_METHODS = [\n");
    for name in value_method_names() {
        s.push_str(&format!("  {},\n", ts::str_lit(&name)));
    }
    s.push_str("] as const;\nreserveAccessors(VALUE_METHODS);\n\n");

    // ── f, path, params ──
    s.push_str(
        "/**\n \
         * `f` — the field namespace. `f.total`, `f.customer.name`.\n \
         *\n \
         * Untyped against any particular schema. `src/generated/schema.ts` \n \
         * re-types this same object against one `interface.json`, so an author \n \
         * with a descriptor gets completion and ref-following.\n \
         */\nexport const f = makeFields();\n\n\
         /** The escape when a field name collides with a method name. */\n\
         export function path(...segments: string[]): Value {\n  \
         return makePath(segments.flatMap((s) => s.split('.')));\n}\n\n",
    );

    // ── literals ──
    s.push_str(&literals_block());

    // ── combinators ──
    for (kw, node) in [(Keyword::And, "and"), (Keyword::Or, "or")] {
        let name = spell::keyword(kw)
            .offered()
            .expect("a combinator is offered");
        s.push_str(&format!(
            "/**\n * `a {node} b` — a same-connective chain.\n *\n \
             * Mixing `and` and `or` without parentheses does not parse, and the \n \
             * printer parenthesizes a nested chain for exactly that reason.\n \
             */\nexport function {name}(...items: Cond[]): Cond {{\n  \
             if (items.length === 0) throw new Error('`{node}` needs at least one condition');\n  \
             return new Cond({{ k: {}, items: items.map((i) => i.node) }});\n}}\n\n",
            ts::str_lit(node),
        ));
    }
    let not_name = spell::keyword(Keyword::Not)
        .offered()
        .expect("`not` is offered");
    s.push_str(&format!(
        "/** `not <atom>` — binds to a comparison or a parenthesized group. */\n\
         export function {not_name}(inner: Cond): Cond {{\n  \
         return new Cond({{ k: 'not', inner: inner.node }});\n}}\n\n\
         /** A bare boolean field as a complete condition (G7). */\n\
         export function bare(v: Value): Cond {{\n  \
         return new Cond({{ k: 'bare', e: v.node }});\n}}\n\n"
    ));

    // ── cond(…) ──
    let cond_name = spell::keyword(Keyword::Cond)
        .offered()
        .expect("`cond` is offered");
    s.push_str(&format!(
        "/**\n * `cond(c1 -> v1, c2 -> v2, …, velse)` — the only conditional.\n *\n \
         * The final bare else is mandatory (totality) and is therefore a \n \
         * required argument rather than an option.\n \
         */\nexport function {cond_name}(\n  \
         arms: readonly (readonly [Cond, Val])[],\n  \
         otherwise: Value,\n\
         ): Value {{\n  \
         if (arms.length === 0) throw new Error('`cond` needs at least one arm');\n  \
         return new Val({{\n    \
         k: 'cond',\n    \
         arms: arms.map(([c, v]) => ({{ c: c.node, v: v.node }})),\n    \
         else: otherwise.node,\n  \
         }});\n}}\n\n"
    ));

    // ── t ──
    s.push_str(&types_block());

    // ── fn ──
    s.push_str(&functions_block());

    // ── agg ──
    s.push_str(&aggregates_block());

    // A file-local reference so the imported node types are used even when a
    // future table stops mentioning one of them; `noUnusedLocals` is on.
    s.push_str("type _Nodes = [AggNode, CondNode, ExprNode];\nexport type { _Nodes };\n");
    s
}

/// Every method name on `Val`, for the accessor reservation.
fn value_method_names() -> Vec<String> {
    let mut names: Vec<String> = spell::ALL_CMP
        .iter()
        .map(|o| spell::cmp(*o).to_string())
        .chain(
            spell::ALL_TEXT_OPS
                .iter()
                .map(|o| spell::text_op(*o).to_string()),
        )
        .chain(
            spell::ALL_BIN_OPS
                .iter()
                .map(|o| spell::bin_op(*o).to_string()),
        )
        .collect();
    for extra in [
        "neg",
        "in",
        "isNull",
        "isNotNull",
        "since",
        "under",
        "above",
        "node",
    ] {
        names.push(extra.to_string());
    }
    names
}

fn literals_block() -> String {
    let mut s = String::new();
    s.push_str(
        "/**\n \
         * `lit(3)`, `lit(\"open\")`, `lit(true)` — the literals whose Ordo type \n \
         * a JavaScript value already determines.\n \
         *\n \
         * Integers pass through {@link intDigits}, which refuses a `number` \n \
         * that IEEE would have rounded: Ordo's `int` is exact, so a literal \n \
         * above 2^53 must arrive as a bigint or a digit string.\n \
         */\n\
         export function lit(v: number | bigint | string | boolean): Value {\n  \
         if (typeof v === 'boolean') return new Val({ k: 'bool', v });\n  \
         if (typeof v === 'string') return new Val({ k: 'str', v });\n  \
         return new Val({ k: 'int', v: intDigits(v) });\n}\n\n\
         /** `10.50` — exact source digits; arithmetic is exact decimal, no float exists. */\n\
         export function decLit(digits: string): Value {\n  \
         if (!/^-?\\d+\\.\\d+$/.test(digits)) {\n    \
         throw new Error(`not a decimal literal: ${digits}`);\n  }\n  \
         return new Val({ k: 'dec', v: digits });\n}\n\n\
         /** `2026-07-01` — a bare ISO date. */\n\
         export function dateLit(iso: string): Value {\n  \
         if (!/^\\d{4}-\\d{2}-\\d{2}$/.test(iso)) throw new Error(`not an ISO date: ${iso}`);\n  \
         return new Val({ k: 'date', v: iso });\n}\n\n\
         /** `2026-07-01T14:30:00Z` — UTC only; the `Z` is mandatory. */\n\
         export function datetimeLit(iso: string): Value {\n  \
         if (!/^\\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}:\\d{2}Z$/.test(iso)) {\n    \
         throw new Error(`not an ISO datetime with a mandatory Z: ${iso}`);\n  }\n  \
         return new Val({ k: 'datetime', v: iso });\n}\n\n\
         /** `90d`, `5m`, `1mo`, `500ms`. */\n\
         export function durationLit(n: number | bigint | string, unit: DurationUnit): Value {\n  \
         return new Val({ k: 'dur', n: intDigits(n), unit });\n}\n\n\
         /** `null`. */\n\
         export function nul(): Value {\n  return new Val({ k: 'null' });\n}\n\n",
    );
    s
}

/// `t` — the declaration types.
fn types_block() -> String {
    let mut s = String::from(
        "/**\n \
         * `t` — the declared types.\n \
         *\n \
         * Generated from `ScalarTy` by an exhaustive match: every declarable \n \
         * type has a constructor, `t.decimal(places)` included since 27ak.\n \
         */\nexport const t = {\n",
    );
    for (ty, sc) in spell::declarable_scalars() {
        let ctor = sc.ctor.expect("declarable");
        let ordo = ty.as_str();
        if ctor == "money" {
            s.push_str(&format!(
                "  /** `{ordo}(<places>)` — {} The places are the currency's: `t.money(2)` stores cents and is written `29.99`. Omitted, they are 0. */\n  \
                 {ctor}: (places?: number): TypeSpec =>\n    \
                 new TypeSpec({{ k: 'scalar', ty: places === undefined ? {{ name: {} }} : {{ name: {}, places }} }}),\n",
                sc.wire,
                ts::str_lit(ordo),
                ts::str_lit(ordo),
            ));
        } else if ctor == "decimal" {
            s.push_str(&format!(
                "  /** `{ordo}(<places>)` — {} Rates and unit prices; it becomes money only through `round`. */\n  \
                 {ctor}: (places: number): TypeSpec =>\n    \
                 new TypeSpec({{ k: 'scalar', ty: {{ name: {}, places }} }}),\n",
                sc.wire,
                ts::str_lit(ordo),
            ));
        } else if ctor == "duration" {
            s.push_str(&format!(
                "  /** `{ordo}(<unit>)` — the unit is part of the type; nothing converts between two. */\n  \
                 {ctor}: (unit: DurationUnit): TypeSpec =>\n    \
                 new TypeSpec({{ k: 'scalar', ty: {{ name: {}, unit }} }}),\n",
                ts::str_lit(ordo),
            ));
        } else {
            s.push_str(&format!(
                "  /** `{ordo}` — {} */\n  \
                 {ctor}: (): TypeSpec => new TypeSpec({{ k: 'scalar', ty: {{ name: {} }} }}),\n",
                sc.wire,
                ts::str_lit(ordo),
            ));
        }
    }
    s.push_str(
        "  /** `field -> target` — a many-to-one edge; `.owned()` and `.tree()` mark it. */\n  \
         ref,\n} as const;\n\n",
    );
    s
}

/// `fn` — the expression functions, at the arities the parser enforces.
fn functions_block() -> String {
    let mut s = String::from(
        "/**\n \
         * `fn` — the closed expression-function set, at the arities the parser \n \
         * checks. An unknown function is not a run-time error here; it is a \n \
         * name that does not exist.\n \
         */\nexport const fn = {\n",
    );
    for (name, min, max) in functions() {
        // `allocate`'s third argument is not an expression: the parser checks
        // the written form against `ALLOCATE_MODES`. So it is spelled as the
        // closed union rather than as a `Val`, and passing a value there is a
        // compile error rather than a parse error after `synth`.
        if name == "allocate" {
            s.push_str(
                "  /**\n   * `allocate(total, weights[, mode])` — the blessed money split.\n   *\n   \
                 * The third argument is a MODE NAME, not an expression: the parser\n   \
                 * checks the written form. Omit it for `largestRemainder`.\n   */\n  \
                 allocate: (total: Value, weights: Value, mode?: AllocateMode): Value =>\n    \
                 new Val({\n      k: 'call',\n      name: 'allocate',\n      args:\n        \
                 mode === undefined\n          ? [total.node, weights.node]\n          \
                 : [total.node, weights.node, { k: 'path', segments: [mode] }],\n    }),\n",
            );
            continue;
        }
        let params: Vec<String> = (0..max)
            .map(|i| {
                let arg = format!("a{i}");
                if i < min {
                    format!("{arg}: Value")
                } else {
                    format!("{arg}?: Value")
                }
            })
            .collect();
        let args: Vec<String> = (0..max).map(|i| format!("a{i}")).collect();
        let arg_expr = if min == max {
            format!(
                "[{}]",
                args.iter()
                    .map(|a| format!("{a}.node"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        } else {
            format!(
                "[{}].filter((x): x is Value => x !== undefined).map((x) => x.node)",
                args.join(", ")
            )
        };
        s.push_str(&format!(
            "  /** `{name}(…)` — {min}..{max} argument(s). */\n  \
             {name}: ({}): Value => new Val({{ k: 'call', name: {}, args: {arg_expr} }}),\n",
            params.join(", "),
            ts::str_lit(name),
        ));
    }
    s.push_str("} as const;\n\n");
    s
}

/// `agg` — the aggregates, at the argument shapes `agg_decl` parses.
fn aggregates_block() -> String {
    let mut s = String::from(
        "/**\n \
         * `agg` — the aggregates, at the argument shapes the parser accepts.\n \
         *\n \
         * The output name is the KEY in `group`'s object, not an argument \n \
         * here: `group(f.customer, { spent: agg.sum(f.total) })` is the shape \n \
         * `group customer { spent: sum total }` has, and one output name in two \n \
         * places would be two places for it to differ.\n \
         */\nexport const agg = {\n",
    );
    for (name, shape) in AGGREGATE_SHAPES {
        let body = match shape {
            AggShape::NoneOrCondition => format!(
                "  /** `{name}` and `{name}(<condition>)` — the conditional aggregate. */\n  \
                 {name}: (when?: Cond): AggNode => ({{\n    \
                 out: '',\n    func: {},\n    arg: null,\n    arg2: null,\n    \
                 cond: when ? when.node : null,\n  }}),\n",
                ts::str_lit(name)
            ),
            AggShape::OneExpr => format!(
                "  /** `{name} <expr>` */\n  \
                 {name}: (of: Value): AggNode => ({{\n    \
                 out: '',\n    func: {},\n    arg: of.node,\n    arg2: null,\n    cond: null,\n  }}),\n",
                ts::str_lit(name)
            ),
            AggShape::ExprAndFraction => format!(
                "  /**\n   * `{name}(<expr>, 0.95)` — the fraction is a literal strictly\n   \
                 * between 0 and 1, checked at parse time. 0 and 1 are excluded on\n   \
                 * purpose: those are `min` and `max`, which already have spellings.\n   */\n  \
                 {name}: (of: Value, fraction: string): AggNode => ({{\n    \
                 out: '',\n    func: {},\n    arg: of.node,\n    \
                 arg2: decLit(fraction).node,\n    cond: null,\n  }}),\n",
                ts::str_lit(name)
            ),
            AggShape::ExprAndSeparator => format!(
                "  /** `{name}(<expr>, \", \")` — the separator is a text literal. */\n  \
                 {name}: (of: Value, separator: string): AggNode => ({{\n    \
                 out: '',\n    func: {},\n    arg: of.node,\n    \
                 arg2: {{ k: 'str', v: separator }},\n    cond: null,\n  }}),\n",
                ts::str_lit(name)
            ),
        };
        s.push_str(&body);
    }
    s.push_str("} as const;\n\n");
    s
}

// ── schema.ts ────────────────────────────────────────────────────────────

/// The schema layer: typed sources and typed field accessors.
///
/// Two things, both derived from the descriptor and neither invented:
///
/// - **A source per entity and per op.** `q.source(products.pick('name'))` and
///   `q.source(activeProducts.bind('cap', p.cap))` need the name to exist; a
///   typo becomes a TypeScript error rather than an `unknown_source` after a
///   deploy.
/// - **`f`, re-typed.** The core's `f` is a proxy over any name; here it
///   carries the deployment's field names, and a field that is a **ref**
///   carries the target entity's fields under it, so `f.customer.name`
///   completes. The depth is capped at two hops, because `categories.parent ->
///   categories` is a self-edge and a naive walk of the ref graph does not
///   terminate.
fn schema_file(doc: &Value, entities: &[Value], ops: &[Value]) -> String {
    let mut s = ts::header(
        "This deployment's sources and field accessors, typed.",
        SCHEMA_SOURCE,
    );
    s.push_str(
        "import { src, type Src } from '../core/build.js';\n\
         import type { AnyFields, Value } from '../core/value.js';\n\
         import { f as anyFields } from './lang.js';\n\n",
    );
    s.push_str(&format!(
        "/** The descriptor this file was generated from. */\n\
         export const SCHEMA = {{ domain: {}, digest: {} }} as const;\n\n",
        ts::str_lit(doc["domain"].as_str().unwrap_or("")),
        ts::str_lit(doc["digest"].as_str().unwrap_or("")),
    ));

    // Sources. Entity and op names share one namespace in the language (a
    // source is "an entity or a registered op"), so they share one here.
    let mut used: BTreeSetOrder = BTreeSetOrder::default();
    s.push_str("// ── sources ──\n\n");
    for e in entities {
        let Some(name) = e["entity"].as_str() else {
            continue;
        };
        let bind = used.take(&ts::binding(name));
        s.push_str(&format!(
            "/** The `{name}` entity, as a source. */\nexport const {bind}: Src = src({});\n",
            ts::str_lit(name)
        ));
    }
    s.push('\n');
    for o in ops {
        let Some(name) = o["op"].as_str() else {
            continue;
        };
        // A write op is not a composable source; the language says so
        // (`read_pipeline` answers `None` for one), so the schema layer does
        // not offer one.
        if o["kind"].as_str() != Some("read") {
            continue;
        }
        let bind = used.take(&ts::binding(name));
        s.push_str(&format!(
            "/** The `{name}` read op, as a composition source. */\n\
             export const {bind}: Src = src({});\n",
            ts::str_lit(name)
        ));
    }

    // Field accessors.
    s.push_str("\n// ── field accessors ──\n\n");
    let refs = ref_targets(entities);
    let flat = flat_fields(entities);
    for (ent, fields) in &flat {
        s.push_str(&format!(
            "/** The fields of `{ent}`. */\nexport interface {}Fields {{\n",
            ts::pascal(ent)
        ));
        for (fname, target) in fields {
            match target {
                Some(t) if flat.iter().any(|(e, _)| e == t) => s.push_str(&format!(
                    "  readonly {}: Value & {}Fields;\n",
                    ts::key(fname),
                    ts::pascal(t)
                )),
                _ => s.push_str(&format!("  readonly {}: Value;\n", ts::key(fname))),
            }
        }
        s.push_str("}\n\n");
    }

    // The union every field name in the schema lives in. Ref fields carry
    // their target's fields; a name that is a ref in one entity and a scalar
    // in another keeps the ref typing, because the ref is the strictly more
    // capable shape and a scalar use of it still type-checks.
    s.push_str(&ts::doc(&[
        "Every field name in this deployment, with refs followed one hop.".to_string(),
        String::new(),
        "Field names are not unique across entities — the language scopes them \
         to a source, and a schema-wide accessor cannot. Where two entities \
         disagree about a name, the REF shape wins, because a value used as a \
         scalar still type-checks and a scalar used as a ref does not. The \
         compiler is the authority either way; this only decides what \
         completes."
            .to_string(),
    ]));
    s.push_str("export interface SchemaFields {\n");
    for (fname, target) in &refs {
        match target {
            Some(t) if flat.iter().any(|(e, _)| e == t) => s.push_str(&format!(
                "  readonly {}: Value & {}Fields;\n",
                ts::key(fname),
                ts::pascal(t)
            )),
            _ => s.push_str(&format!("  readonly {}: Value;\n", ts::key(fname))),
        }
    }
    s.push_str("}\n\n");
    s.push_str(
        "/**\n \
         * `f` — the same proxy the core exports, typed against this schema.\n \
         *\n \
         * The index signature stays, so a field the descriptor does not \n \
         * describe (an op's computed output, a name behind a scope the caller \n \
         * cannot see) is still reachable and still checked by the compiler \n \
         * rather than by this file.\n \
         */\n\
         export const f = anyFields as unknown as SchemaFields & AnyFields;\n",
    );
    s
}

/// Every `(field, ref target)` in the schema, deduplicated, refs winning.
fn ref_targets(entities: &[Value]) -> Vec<Field> {
    let mut out: std::collections::BTreeMap<String, Option<String>> =
        std::collections::BTreeMap::new();
    for e in entities {
        for f in e["fields"].as_array().into_iter().flatten() {
            let Some(name) = f["name"].as_str() else {
                continue;
            };
            let target = f["ref"].as_str().map(str::to_string);
            let slot = out.entry(name.to_string()).or_insert(None);
            if slot.is_none() {
                *slot = target;
            }
        }
    }
    out.into_iter().collect()
}

/// One field: its name, and the entity it refs (or `None` for a scalar).
type Field = (String, Option<String>);

/// Per-entity field lists, in declaration order.
fn flat_fields(entities: &[Value]) -> Vec<(String, Vec<Field>)> {
    entities
        .iter()
        .filter_map(|e| {
            let name = e["entity"].as_str()?;
            let fields = e["fields"]
                .as_array()?
                .iter()
                .filter_map(|f| {
                    Some((
                        f["name"].as_str()?.to_string(),
                        f["ref"].as_str().map(str::to_string),
                    ))
                })
                .collect();
            Some((name.to_string(), fields))
        })
        .collect()
}

/// Binding names, made unique by suffix.
///
/// Entities and ops share one namespace in the language, and nothing forbids
/// an op and an entity from having names that collide once `.` becomes `$`.
/// A silent collision would emit a file where the second `const` shadowed the
/// first, so the second gets a suffix and keeps its own Ordo name in the
/// string beside it.
#[derive(Default)]
struct BTreeSetOrder {
    seen: std::collections::BTreeMap<String, usize>,
}

impl BTreeSetOrder {
    fn take(&mut self, base: &str) -> String {
        let n = self.seen.entry(base.to_string()).or_insert(0);
        *n += 1;
        if *n == 1 {
            base.to_string()
        } else {
            format!("{base}_{n}")
        }
    }
}

fn index_file() -> String {
    let mut s = ts::header("Re-exports of the generated surface.", LANG_SOURCE);
    s.push_str(
        "export * from './vocabulary.js';\n\
         export * from './schema.js';\n\
         // `f` is exported by both halves and they are the same object: the\n\
         // language layer builds it, the schema layer re-types it against one\n\
         // deployment. The typed one wins here, and an author with no schema\n\
         // imports `f` from `./lang.js` directly.\n\
         export { f } from './schema.js';\n\
         export * from './lang.js';\n",
    );
    s
}
