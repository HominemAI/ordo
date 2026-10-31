# `@hominemai/ordo-cdk`

The typed TypeScript builder for Ordo. Define a domain in code; `synth()`
emits canonical `.ordo` text, and that text is the artifact that is reviewed,
diffed, committed and deployed.

```ts
import { domain, entity, op, synth, t, f, g, agg, lit, src } from '@hominemai/ordo-cdk';

const vendors = entity('vendors', {
  name: t.text(),
  region: t.text(),
  active: t.bool().default(lit(true)),
});

const activeProducts = op('activeProducts', {}, (b) =>
  b.source(src('products').args(f.active.eq(lit(true))).pick('name', 'price')),
).unbounded();

console.log(synth(domain('market', '1.1.0'), vendors, activeProducts));
```

```ordo
domain market @ 1.1.0

entity vendors {
  name: text
  region: text
  active: bool = true
}

op activeProducts unbounded =
  products(active == true)[name, price]
```

**A frontend, never a second semantics.** Everything downstream is unchanged:
same compiler, same manifests, same cost classes, same registry. The platform
never knows which frontend authored a definition. `.ordo` text and this
builder are two doors into one room.

---

## Installing it in an app outside this repository

The package is private, published to GitHub Packages under the HominemAI
organization.

1. Add this line to the app's `.npmrc`:

   ```
   @hominemai:registry=https://npm.pkg.github.com
   ```

2. Create a personal access token (classic) with the `read:packages` scope.
   Keep it out of git: put `//npm.pkg.github.com/:_authToken=<your token>` in
   your user `~/.npmrc`, not the project's.

3. Install:

   ```
   npm install @hominemai/ordo-cdk
   ```

A release is a version bump in `package.json` merged to `main`; the
`Packages` workflow publishes it.

---

## The boundary: skeleton and vocabulary

This package has two halves, and knowing which one you are in is the whole of
using it well.

| | **Skeleton**: `src/core/` | **Vocabulary**: `src/generated/` |
|---|---|---|
| Written by | a person, once, reviewed | `ordo gen ts`, from the parser's own tables |
| Contains | the IR, the chaining shape, the argument order, the guard algebra, the printer | every word: types, operators, functions, aggregates, units, modes, entities, fields |
| Changes when | somebody redesigns the API | the language changes |
| Edited by hand | yes | **never**, the next `ordo gen ts` discards it |

The rule that draws the line: **a shape is the skeleton's, a word is the
generator's.** That a comparison has a left, an operator and a right is a
shape. That the operators are `== != < <= > >=`, and that they are spelled
`.eq() .ne() .lt() .le() .gt() .ge()`, is vocabulary.

It is drawn there because of what each side costs when it drifts. A *word*
that drifts is a builder that can express something the language cannot, or
cannot express something it can. That is caught by the isomorphism corpus
and prevented by generating it from the one table the parser reads. A *shape*
that drifts is an API redesign, which is a human decision and should not
arrive as a diff on a generated file.

Two seams cross the line, both in `src/core/value.ts`:

- `Value` is declared as an interface with **no members**, and
  `src/generated/lang.ts` merges the operators into it. So the skeleton can
  type parameters, field paths, sort keys and write values without naming one
  operator.
- `installValue()` is the run-time half: the generated file hands over its
  `Val` constructor at import time, so a value typed as having `.eq()` really
  has it.

## Three gates a new keyword must pass

`crates/ordo-gen/src/spell.rs` matches `Keyword` (and `ScalarTy`, `CmpOp`,
`TextOp`, `BinOp`, `DurationUnit`, `EventKind`) **exhaustively, with no
wildcard arm**. So:

1. **`cargo build` fails** if a word added to the lexer has no decision here.
   The generator does not compile, so `ordo gen ts` cannot run, so neither
   TypeScript package can be regenerated until somebody writes down what the
   new word is called. (`crates/ordo-gen/tests/ratchet.rs` scans the source so
   the arms stay wildcard-free, and records the three mutations it was
   checked against.)
2. **`npm run check` fails** if a decision has no implementation:
   `src/generated/vocabulary.ts` publishes every word with its spelling or the
   reason it has none, and `test/vocabulary.test.ts` walks that list and
   asserts every offered name resolves on this package's surface.
3. **The corpus fails** if an implementation says something the parser
   refuses (see below).

"It is not offered, and here is why" is a legal answer, and it is the one most
declaration keywords get. `vocabulary.ts` carries all of them.

## Isomorphism, and what is actually checked

Track C's bar is that *anything expressible in the builder is expressible in
`.ordo` and vice versa*, as a conformance-tested property. The loop:

| Leg | Runs in | Does |
|---|---|---|
| A | `cargo test` | parses a corpus program, machine-translates its AST into builder calls, compares against `test/corpus/<name>.gen.ts` |
| B | `npm run check` | imports that module, calls `synth()`, compares against `test/corpus/<name>.synth.ordo` |
| C | `cargo test` | parses that `.ordo` and asserts the AST equals the original's |

The translation is **machine-written** (`ordo_gen::emit`) on purpose. A
hand-written set of builder programs would exercise only the nodes whoever
wrote it thought of; a walk that matches every AST enum exhaustively cannot
skip one silently. A node kind with no builder spelling either fails the
build or is reported by name.

**Score: 41 of 41 corpus programs round-trip, with no builder misses.** The
corpus is both conformance fixtures (`market`, `wide`) and every fenced block
of `docs/syntax-examples.md` that `parse_program` accepts.

**"Canonical".** The plan says `synth` emits canonical `.ordo` *via
`ordo fmt`*. There is no `ordo fmt` (Track A1 is unbuilt), so the property
checked is **parse-and-reparse equality**: `parse(original)` and
`parse(synth(…))` are the same AST, spans excepted. That is weaker than byte
equality and stronger than nothing, and it is the claim the test makes. When
`ordo fmt` lands it becomes the normalizer on both sides and the claim
tightens to bytes.

## What this builder does not offer, and why

Every exclusion below is also in `src/generated/vocabulary.ts`, machine
readable, with the same reason.

**Because the language does not parse it.** An absent method is the
language's own refusal, arriving one layer earlier. Nothing is owed until the
grammar moves.

- `principal`, `client`, `auth`, `policy`, `compliance`, `channel`
  : `definition-language.md` §8's *"All illustrative syntax [lean]: concepts
  settled, file format not final"*. A declaration form **is** the file format.
- `app` / `page`: `ui-gen.md` §5, whose block set is explicitly still open.
- `space`, `filled`: reserved so they cannot head a clause by accident; no
  declaration form uses them.
- Every **event kind** (`created converged deleted written ingested fed
  reverted`) as a word of its own. They are written inside the `when('…')`
  pattern string, `<domain>.<entity>.<kind>`, and the parser checks them
  against its closed set.

**Out of scope for this increment.** This is a real debt, tracked in
`docs/roadmap-checklist.md` 14f.

- **`whole`.** It is a special form, not a pipeline step, and it does not
  compose. A builder for it would be a second skeleton, not vocabulary added
  to this one, and the corpus excludes `whole` blocks by name.
- **`filled`.** The language accepts it on a parameter since 2026-10-04
  (`$today: date filled`), and the generated client fills a `date` or
  `datetime` one. The builder has no way to mark a parameter `filled` yet,
  so a program that needs one is written in `.ordo`.

**Covered: writes.** `set` and `del` are in the op grammar, so they are here:
`b.set(entity, fields)`, `b.del(entity, cond)`, `.where()`, `.subtree()`, and
`.let_()` for naming a created row.

And the routine (12a). Its body is a write op's body, built with the same
calls. The trigger is `every('15m')`, `daily('09:00Z')` or
`when('estate.events.created', { debounce: '30s', coalesce: true })`, and the
flags chain after it as on an op:

```ts
routine('staleAlerts', { in: 'estate', trigger: every('15m'), params: { today: t.date() } }, (b, p) => [
  b.set('alerts', { status: lit('dismissed') }).where(f.raised.lt(p.today.sub(durationLit(30, 'd')))),
]).unguarded();
```

A routine that reads rows and writes once per row wraps its query and
writes in `each`, exported from the package. Inside the writes, row fields are
`f.id` and `path('unit', 'name')`:

```ts
routine('expire', { in: 'shop', trigger: daily('08:00Z'), params: { today: t.date() } }, (b, p) =>
  each(b.source(src('leases').where(f.end.lt(p.today)).pick('id')), [
    b.set('leases', { id: f.id, status: lit('expired') }),
  ]),
);
```

And the release rule (27aq): `release.member.open()`, `.locked()` and
`.when(rel.none('invoices', g.eq('owner', MEMBER)), [rel.remove('drafts.author')])`.
Inside a release rule the guard's subject prints as `$member`; `MEMBER` and
`CALLER` are the same node, and the position decides the word.

## Divergences from the demo page's sketch

`site-animations/process-flow.html` shows a "planned" pane, and
`tooling-and-embedding-plan.md` Track C shows a different one. Where they
disagreed with each other or with the grammar, this is what was built and why.
The demo page should be corrected to match.

| Sketch | Built | Why |
|---|---|---|
| `q.from(plants, ['name'])` / `q.source(vendors.pick("name"))` | `b.source(src('plants').pick('name'))` | Both sketches, reconciled. The grammar's word is *source*, and a source list is one line of them. |
| `.bar(f.billed.gt(0))` | `.where(cond)` | `.bar` named the `\|` character and not what the step does. The other sketch already used `.where`. |
| `.where('at <= $today - everyDays or at is null')`, a **string** | `.where(or(f.at.le(…), f.at.isNull()))` | A string predicate is a second parser. The closed combinator set leaves no escape into an arbitrary predicate, because there could not be one that compiled. |
| `.sort('-waterings.at')` | `.sort(desc(f.waterings.at))`, and `.sort('-waterings.at')` also works | The string form is kept as sugar; the emitter uses the explicit one. |
| `.take('plants.id', 1)` | `.take([f.plants.id], 1)` | Keys are expressions, not strings, and the grammar writes `take[keys] n`, keys first. |
| `t.duration('ms').default('7d')` | `t.duration('ms').default(durationLit(7, 'd'))` | A default is an expression, and `'7d'` as a string literal is a different Ordo value than a duration literal. |
| `alerts.upsert({ … })` | absent | There is no `upsert` in the language. Create-or-converge is derived from the write's identity fields, never spelled. |
| `routine(…)` | `routine(name, { in, trigger, params }, (b, p) => [...])` | Built (12a). The trigger is one of `every`, `daily`, `when`. A body of `each(query, writes)` runs the writes per row. |
| `route(…)` | absent | See the exclusions above. |
| `entity('x', { … })` then `w.set(alerts, { key: 'thirsty:$plant' })` | `w.set(alerts, { key: … })` with a real expression | `'$plant'` inside a string is not interpolation in Ordo; a parameter reference is `p.plant`. |
| `q.join(waterings, ['at'])` | `.matchAny/.matchNone/.matchRef/.matchOn` | The language has no `join`. A source list is the join; `match` is the four disambiguating forms. |

## Regenerating

```
cargo run -p ordo-cli --bin ordo -- gen ts <dir|file.ordo>
```

Writes `src/generated/` in both this package and `clients/ordo-sdk`. The
language layer (`vocabulary.ts`, `lang.ts`) is the same bytes for every
deployment; the schema layer (`schema.ts`) is this deployment's entities, ops
and field accessors.

```
npm run check        # tsc + vitest
npm run gen:corpus   # re-synthesize the isomorphism fixtures (needs node)
```

`crates/ordo-gen/tests/generated_files.rs` asserts the checked-in generated
half is byte-identical to a fresh run, so a table change that nobody
regenerated fails `cargo test`.

## Known sharp edges

- **`f.total.eq` is the operator, not a field called `eq`.** The collision is
  only ever on a second path segment: the top-level `f` carries no
  operators, so `f.eq` really is a field. Where a nested field name collides,
  `path('total', 'eq')` is the escape.
- **Ref-following in `f`'s types is capped at the schema graph's own edges**
  and does not chase a tree self-edge forever. Anything the descriptor does
  not describe still resolves, and the compiler is the authority either way.
- **`noUncheckedIndexedAccess` is off in this package**, and on everywhere
  else. `f` is a total proxy (every access answers, by construction) and an
  index signature is the only way TypeScript can say that; with the flag on,
  every field reference in every program written with this package would carry
  a `!`. A blanket non-null assertion is strictly worse than the flag being
  off here.
