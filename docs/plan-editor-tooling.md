# Ordo editor tooling — design & build plan

Companion to `tooling-and-embedding-plan.md` **Track A** (the standalone
compiler and the language server — A2 is the LSP; Track C is the *builder
frontends*, a different subject), `adoption-dx-plan.md` §3.2 and §8 items 6
and 7, `definition-language.md` §1–§2 (the closed grammar and its lexical
rules), and `docs/syntax-examples.md` (the visual test corpus).

Markers: **[settled]** restated from a settled document · **[proposed]**
adopted here, pending owner review · **[open]** named and not answered.

Status: build plan. **Increment 0 has landed** — `editors/ordo.tmLanguage.json`
and `crates/ordo-syntax/tests/editor_grammar.rs`, verified against the real
TextMate engine over the whole doc corpus (§3.5). Increments 1–5 are unbuilt.
Every claim about the current code was checked against the code and carries
its file.

---

## 0 · The problem, stated precisely

### 0.1 Ordo source is uncoloured everywhere it is read

There is no highlighting for `.ordo` in this repository and never has been.
Checked, not assumed:

- **The playground has none.** `crates/ordo-playground/src/index.html` edits
  Ordo in three bare `<textarea>` elements (`#ordo-in`, `#sql-in`,
  `#write-in`). Its `<script>` block does `fetch`, `JSON.stringify` and
  `esc()`, and nothing else. The "ordo reference" panel fakes highlighting by
  hand-writing `<i>` tags around comments inside `<pre>`. So increment 0
  **replaces nothing** — the playground is a consumer with an empty slot, not
  an incumbent to be migrated.
- **The docs are uncoloured.** Every fence in `docs/syntax-examples.md` and
  `docs/tutorial.md` is a bare ``` ``` ``` with no language tag;
  `crates/ordo-syntax/tests/docs_parse.rs` says so in a comment and treats
  every bare fence as a candidate because of it. A renderer has nothing to key
  off and no grammar to key it to.
- **No editor knows the extension.** There is no `editors/` directory before
  this plan, no `.vscode` language contribution, and no `*.tmLanguage.*`
  anywhere in the tree.

For a language whose adoption case is "the editor answers the questions the
internet can't yet" (`adoption-dx-plan.md` §3.2), the editor currently answers
nothing at all — not even which words are reserved.

### 0.2 What this document is not

It is not the LSP's *design*. `tooling-and-embedding-plan.md` §A2 and
`adoption-dx-plan.md` §3.2 already settle what the language server does and
why the closed grammar makes it unusually strong; §5 below sequences the
build and states the two facts they left out (what the diagnostics are made
of, and what the parser does not yet accept), rather than restating them.

It is not the formatter (`ordoc fmt`, Track A1) and not the interface
descriptor (B1, built 2026-09-07) — though §5 spends the descriptor rather
than recomputing it.

---

## 1 · The starting state, verified

Every increment below is a delta on these facts. They were read, not assumed.

**The keyword inventory is closed and now enumerable.**
`crates/ordo-syntax/src/token.rs` holds 61 fully-reserved words in
`Keyword`, and a deliberately *unreserved* contextual tier documented on the
enum: `read write admin` (positional after `access`, inside `acl (…)`),
`run method methods share rollup live table metric detail route`, and every
aggregate and function name. The two tiers are load-bearing —
`lexer.rs::contextual_word_is_a_legal_field_name_reserved_word_is_not` asserts
that `entity account { read: int }` binds a field named `read` while
`{ access: int }` does not. Any highlighter that colours `read` as a keyword
teaches the opposite of the truth.

**The literals are unusual, and three of them are not what they look like.**
`lexer.rs::number` leads four ways off a digit: a calendar-checked date, a
datetime whose `Z` is mandatory, a duration whose unit comes from a closed
eight-word set, or a decimal kept as exact source digits. Beyond that:

- **A typed id is not a token.** `_` is not an identifier character
  (`check_name` refuses it, `definition-language.md` §2 says `_` "stays
  reserved for typed-id literals"), and a name must start lowercase. So
  `task_01J8ZKQ4X7…` cannot lex as a bare token anywhere; it exists only
  *inside a string literal* or as the value of an `id` parameter. Every
  typed id in `syntax-examples.md` is inside quotes, and the three bare ones
  in `definition-language.md` §2 are an illustration in a prose fence.
- **A domain version is three tokens.** `domain estate @ 1.0.0` lexes as
  `At`, `Decimal("1.0")`, `Dot`, `Int(0)`; `declparse.rs::version_text`
  reassembles them.
- **A schedule time does not lex at all.** `every day 08:00Z` reaches the
  lexer as `Int(8) Colon Int(0)` and then a capital `Z`, which is the
  identifier refusal. Schedules are parsed out of a standalone string by
  `trigger.rs::Schedule::parse`, and `trigger.rs`' module doc explains why:
  §7 and §8 spell the trigger declaration two irreconcilable ways, so the
  *enclosing declaration form* is deliberately absent.

**`parse_program` accepts three declarations, not thirteen.** It takes
`domain`, `entity` and `op` and nothing else (`declparse.rs`), so `routine`,
`when`, `every`, `channel`, `route`, `app`, `page` and `principal` are
reserved words with no production behind them; `docs_parse.rs` records their
doc blocks as legitimately unparsed. This is the single biggest constraint on
increment 2's diagnostics and it is stated again there.

**Diagnostics already carry classes and spans — except one.**
`ordo_syntax::ParseError`, `ordo_sema`'s error and `ordo_schema::SchemaError`
each carry `code: &'static str`, `message: String`, `span: Span`.
`ordo_syntax::LexError` carries **message and span but no code**, so the
lexical half of the refusal inventory has no class to put in an LSP
diagnostic's `code` field. `Span` itself carries `start`/`end` byte offsets
but line/column of the **start only** — an LSP range needs both ends.

**There is one binary and it is `ordo`.** `crates/ordo-cli` declares
`[[bin]] name = "ordo"`. `ordo-host` declares none (checklist 15a, blocker
0a) and `Cargo.lock` is gitignored (blocker 0b) — both of which matter the
moment a signed `ordo-lsp` binary has to be reproducible.

**CI already runs the check.** `.github/workflows/ci.yml` runs
`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -D
warnings` and `cargo test --workspace`. Increment 0's parity test is an
ordinary workspace test, so it is in CI with no workflow change, and the
grammar's JSON validity is asserted by the test's own parse.

---

## 2 · The artifact: one grammar, four hosts

The whole plan turns on a single decision, made once here.

**A TextMate grammar is the primary artifact, and there is exactly one of
them.** It is the only highlighting format consumed unchanged by all four
places Ordo source is read:

| Host | How it consumes `editors/ordo.tmLanguage.json` |
|---|---|
| VS Code | `contributes.grammars` in the extension manifest — the native format |
| JetBrains | Settings → Editor → TextMate Bundles imports the directory as-is |
| Docs / site | shiki loads TextMate grammars directly; it is what shiki *is* |
| Playground | `monaco-textmate` / `shiki`-backed CodeMirror, or nothing yet |

Four hosts, one file, one keyword list, one place a new keyword lands. Every
alternative considered is ruled out in §10, and each is ruled out by the same
argument: it serves fewer hosts for the same maintenance, and a second
vocabulary list is a second answer waiting to differ.

---

## 3 · Increment 0 — the TextMate grammar — **BUILT**

`editors/ordo.tmLanguage.json` (scope `source.ordo`, file type `.ordo`) and
`crates/ordo-syntax/tests/editor_grammar.rs`.

### 3.1 The scope design

Scopes are split by **what a word does**, not flattened onto one `keyword`
colour, because the language's own distinctions are the ones a reader needs:
a declaration head, a clause head, a pipeline verb and a write verb are four
different things and a theme should be able to say so.

| Family | Words | Why it is its own colour |
|---|---|---|
| `keyword.declaration.ordo` | `domain entity op routine app page principal client auth policy compliance channel space` | the things that begin a top-level declaration; the file's skeleton |
| `keyword.other.clause.ordo` | `trigger when every check index unique acl access` | clause heads inside a block |
| `keyword.control.pipeline.ordo` | `group reduce sort take append let distinct each whole` | the query verbs — one transform per line, so the line's verb should read first |
| `keyword.other.write.ordo` | `set del subtree` | every mutation in a program is one of three words. `grep` finds them; so should the eye |
| `keyword.control.match.ordo` / `keyword.operator.match.ordo` | `match cond` / `any none on` | the join and conditional forms |
| `keyword.operator.logical.ordo` / `.word.ordo` | `and or not in is` / `contains starts matches under above since` | word-operators, quiet by design |
| `storage.modifier.ordo` | `sealed indexed owned tree normalize` | field modifiers — the declaration's second column |
| `storage.modifier.flag.ordo` | `unbounded reproducible filled` | registration flags: author decisions, not syntax |
| `storage.modifier.flag.unguarded.ordo` | `unguarded` | **its own scope.** §8.5 says bypass is "explicit and greppable"; a theme can make it impossible to skim past, and a scope shared with `unbounded` could not |
| `constant.language.ordo` | `null true false` | |

Beside them, the things a plain keyword list would miss:

1. **Typed ids are matched inside string literals**
   (`constant.other.typed-id.ordo`), because that is the only place they can
   occur (§1). A rule for bare typed ids would colour something the lexer
   refuses.
2. **`$caller` and `$trigger` are `variable.language.ordo`**, separate from
   `variable.parameter.ordo`. They are platform-supplied and
   never client-suppliable (§8.6); an author's `$from` and the identity the
   guards are anchored to should not look alike.
3. **Guards are a begin/end clause, not a keyword match.** `access read|write|
   admin` opens a rule that runs to end of line *unless a delimiter is open* —
   which is G6 encoded as a grammar rule, since the nested paren rule keeps
   the outer `end` untested while it is on the stack. Inside it, `open` gets
   `keyword.other.access-open.ordo`: "the word is `open`", greppable, in the
   diff, and *only* in the position where it means that.
4. **Aggregates are coloured only inside `group`/`reduce` braces**, and only
   in the value position — because `count` is a legal field name and a
   grammar that coloured it everywhere would teach that it is not.
5. **Declaration sites get entity scopes** via begin/end and capture rules:
   the name after `entity` is `entity.name.type.ordo`, after `op`/`routine`
   `entity.name.function.ordo`, after `domain` `entity.name.namespace.ordo`,
   and `x -> entities` in a shape gets the ref arrow plus the target entity.
6. **The version after `@` is coloured whole** (`constant.numeric.version.ordo`)
   even though the lexer sees three tokens. Rendering what the author wrote
   is the honest choice; the file's comment records the divergence.
7. **The grammar refuses what the lexer refuses.** `invalid.illegal.*` for
   snake_case and Uppercase names, names over 64 characters, `!`, single
   quotes, unknown escapes, unterminated strings, unknown duration units,
   `$3`, a schedule time without its `Z`, an event kind outside the closed
   seven, an impossible date, a datetime without `Z` or with an offset. The
   editor states the refusal before the compiler is asked.
8. **The date regex carries the calendar, as far as a regex can.**
   `2026-02-31`, `2026-04-31`, `2026-11-31`, `2026-13-01` and `2026-01-00`
   are refused by month length exactly as `lexer.rs::is_real_date` refuses
   them. `2026-02-29` is not: the leap rule needs arithmetic on the year.
   That is the one calendar case the file gets wrong on purpose, and it says
   so in its own comment.

**Scope names are two-layer on purpose.** Every leaf sits under a standard
TextMate prefix (`keyword.`, `storage.modifier.`, `constant.`, `support.`,
`invalid.`), so an untuned theme colours it sensibly by prefix, while the
distinctive leaf (`keyword.other.access-open.ordo`) lets an Ordo theme say
something a generic theme cannot. Ordo ships no theme; it ships scopes a
theme can use.

### 3.2 What the parity test pins

`crates/ordo-syntax/tests/editor_grammar.rs`, eight tests, in the same
two-directional spirit as `docs_parse.rs` — the list is checked from both
ends and cannot pass vacuously.

The grammar carries a manifest (`ordoInventory`) of every word it treats as
vocabulary, split into `reserved`, `contextual` and `dual`. Against it:

1. **`reserved` ∪ = `Keyword::ALL`, exactly, both ways.** A keyword added to
   the lexer fails by name ("an author who writes it will see it uncoloured
   and never learn it is reserved"); a word in the grammar that is not a
   keyword fails too.
2. **`reserved` is a partition** — one word, one semantic family, or it has
   two colours and neither teaches anything.
3. **Every `contextual` word satisfies `Keyword::parse(w).is_none()`** — the
   other half of the two-tier discipline, and the half that protects
   `count`, `read` and `open` as field names.
4. **`dual` is exactly the intersection.** Today it is `["distinct"]`, which
   really is both a pipeline step and an aggregate (`declparse.rs::agg_decl`
   handles it by hand). Anything else appearing there is a word whose tier
   changed, and it needs a decision rather than an exemption.
5. **Every manifest word is matched by a capturing group somewhere** — a
   manifest listing words the grammar never colours would pass (1)–(4) while
   colouring nothing.
6. **Every word a pattern colours as vocabulary is in the manifest** — so a
   word cannot be coloured as a keyword forever without anything checking it
   against the compiler.
7. **Five borrowed closed sets are pinned to their producers, not retyped**:
   event kinds against `EventKind::ALL`, expression functions against
   `function_names()`, aggregates against `AGGREGATE_NAMES`, allocation
   modes against `ALLOCATE_MODES`, duration units against
   `DurationUnit::ALL`.
8. **The grammar names the language version it tracks**, checked against
   `ordo-syntax`'s own version — see §9.

Making (1) and (7) possible without inventing a fourth list required
collapsing three vocabularies onto one table each, which is a net deletion:
`Keyword` and `DurationUnit` are now declared once through a `word_enum!`
table, so the enum, `parse`, `as_str` and `ALL` are projections of one word
list; `AGGREGATE_NAMES` replaces a display string that was a second list of
the aggregate names; `function_names()` exposes the function set without
leaking the arities nothing outside the parser needs.

**Done when** *(met)*: eight tests green in `cargo test --workspace`; five
independent mutations run and each fails the intended test and no other (a
manifest word the grammar never matches, a keyword dropped from the manifest,
a keyword added to the lexer, a stray word coloured as a modifier, a
contextual word that is actually reserved, a borrowed set drifting).

### 3.3 Known blind spots — what a TextMate grammar cannot do

Stated because the next person will otherwise try to fix them with a regex.

- **Hyphen versus minus.** `lease-expiry` is genuinely `lease` minus
  `expiry`, and `definition-language.md` §2 says so in as many words: "unary/
  binary minus is legal in expressions and position cannot rescue it." No
  regex can know which was meant. There is no rule for it and there should
  not be.
- **The leap rule.** `2026-02-29` colours as a date (§3.1, item 8).
- **No precedence table.** `a and b or c` is a parse error. A highlighter
  sees two legal word-operators; only a parser sees the refusal.
- **Everything semantic.** A sealed field read by name, an op with no
  provable bound and no `unbounded` marker, a `check` that aggregates, an
  ambiguous match, a fan-out — all of them are compile errors that no lexical
  rule can reach. This is the boundary increment 2 exists to cross.
- **Contextual words in unusual positions.** A field literally named `count`
  used in the value position inside a `group` brace, or a field named `open`
  on an `access` line, will be coloured as the vocabulary word. Position is
  the only signal a TextMate grammar has, and the trade is deliberate: the
  common reading is right and the rare one is merely mis-coloured, never
  mis-parsed.
- **`group` with its brace on the following line** ends the pipeline clause
  early. Every occurrence in the corpus opens the brace on the same line;
  when one does not, the fix is the LSP's semantic tokens, not a cleverer
  regex.
- **Use sites are not resolved.** The grammar colours *declaration* sites; a
  bare `leases` in a pipeline is an ordinary identifier, because whether a
  name is an entity, a registered op or a field is a question for the binder.
- **`route`/`channel` bodies and `app`/`page` blocks** are coloured only at
  the head, because the parser does not accept them (§1) and inventing scopes
  for a form the grammar has not settled would be inventing grammar.

Every one of these is exactly the set that **semantic tokens** from the LSP
close (§5.3). That is the honest resolution: the TextMate grammar is the
instant, dependency-free, four-host layer, and the compiler refines it.

### 3.4 Two divergences found on the way, recorded not fixed

- **`definition-language.md` §2's reserved list and the lexer disagree.**
  The doc list carries `run`, `open`, `route`, `read`, `write` and `std`,
  which `Keyword::parse` does not reserve — `token.rs` deliberately keeps
  them contextual and its own doc comment says why. It also omits
  `compliance`, `space` and `auth`, which the lexer does reserve. The doc
  promises the list is "mechanically regenerated from the grammar"; it is
  not, and the grammar's manifest is now the thing that is. **[open]** —
  fixing the prose is a doc change, not a language change, and belongs to a
  documentation-truth pass, not here.
- **Four doc defects the highlighter found.** Tokenizing every fence in
  `definition-language.md` surfaced `match transfers.from_account`,
  `append reminders(due)[title, at: due_date]`, `$GOOGLE_CLIENT` and
  `$DISCORD_CLIENT` — all four illegal under §2's one identifier rule.
  `docs_parse.rs` does not cover `definition-language.md`'s fences, so
  nothing had read them mechanically before. **[open]**: either fix the four
  spellings and add the file to `docs_parse.rs`, or state why that file's
  fences are prose. Not done here because it is a corpus change with its own
  allow-list consequences.

### 3.5 How it was verified

Not by eye. Every ```` ```ordo ```` and bare fence in `syntax-examples.md`,
`tutorial.md` and `definition-language.md` was tokenized with
**`vscode-textmate` + `vscode-oniguruma`** — the exact engine VS Code runs —
and the output was checked two ways: a scope census (57 distinct scopes, in
the expected proportions) and a list of every `invalid.*` hit. There were
twelve, all correct, and four of them are the doc defects above. A separate
stress corpus of the lexer's own reject cases confirmed that each refusal
rule fires on exactly its case and nothing else.

**Owed to increment 1:** the tokenizer harness lives in a scratch directory,
not in the repo. It should become a checked-in fixture test (`editors/tests/`
or a `#[test]` shelling out to node) so that "the corpus highlights
correctly" is asserted rather than remembered. Named here rather than done,
because it introduces a node toolchain dependency into a Rust workspace and
that is a decision, not a detail — the same decision `@ordo/react`'s "the JS
build is not wired into cargo or CI" (checklist 14b) already owes an answer
to, and the two should be answered together.

---

## 4 · Increment 1 — the VS Code extension, shell only

`editors/vscode-ordo/`. A day's work, no Rust, no server.

```
editors/vscode-ordo/
  package.json                     contributes.languages + contributes.grammars
  language-configuration.json      comments, brackets, autoclosing, folding
  syntaxes/ordo.tmLanguage.json    a build-step COPY of editors/ordo.tmLanguage.json
  README.md  CHANGELOG.md  icon.png
```

The grammar is **copied at package time, never forked**. `vsce` cannot follow
a symlink out of the extension root, so the copy is a build step and CI
asserts byte-equality with the source of truth — otherwise the four-host
claim in §2 quietly becomes three plus a fork.

`language-configuration.json`, all of it derived from settled lexical rules:

- `comments.lineComment: "#"`, and **no `blockComment`** — there are none,
  one spelling.
- `brackets` / `autoClosingPairs` / `surroundingPairs`: `()`, `[]`, `{}`, and
  `"` with `notIn: ["string"]`. **No single quote**, because `'` is not a
  string delimiter and offering to close one would teach a refusal as a
  feature.
- `folding` by `indentationRules` — indentation is style, not syntax, so
  folding markers cannot be structural. Brace and bracket folding comes free
  from `brackets`.
- `wordPattern` = `[a-z][A-Za-z0-9]*` plus `\$[a-z][A-Za-z0-9]*`, so
  double-click selects a parameter whole and does not select across a `_`
  that cannot be there anyway.
- `onEnterRules` to continue a `#` comment block. Nothing else: there is no
  auto-indent worth guessing at when the language has no indentation rule.

**Done when**: installing the `.vsix` colours every block of
`syntax-examples.md` correctly in both a light and a dark stock theme;
`.ordo` is associated; `#` comment toggling, bracket matching and
auto-closing work; the packaged grammar is byte-equal to
`editors/ordo.tmLanguage.json` and CI says so; the extension activates with
**no** `activationEvents` beyond the language contribution (a highlighting-only
extension must cost nothing until an `.ordo` file is opened).

---

## 5 · Increment 2 — `ordo-lsp`, the compiler as the analyzer

### 5.1 The shape, and why the "compiler extension" is not an extension

The owner's question was whether there should also be a "VS Code compiler
extension". The answer is that the compiler should not live in the extension
at all: it should be a **native binary crate, `crates/ordo-lsp`**, that
depends on `ordo-syntax`, `ordo-sema`, `ordo-schema` and `ordo-plan`
directly, and the extension should be a thin client that starts it. Three
reasons, in order of weight:

1. **The compiler *is* the analyzer.** Diagnostics are not a second opinion
   about the source; they are the closed refusal classes with their real
   messages and their real spans, which `ParseError`, `SchemaError` and
   `ordo_sema`'s error already carry as `code` + `message` + `span` (§1).
   An LSP written against those types cannot disagree with `ordo check`,
   because it is the same call.
2. **One server serves every editor.** JetBrains, Neovim, Helix and Zed come
   free (§7). An extension-hosted analyzer serves one.
3. **A second parser is a second answer waiting to differ** — the argument
   §10 makes against a TypeScript LSP and the same argument the rest of this
   repository makes against every second walk.

What the extension *does* own is the CLI-shaped work, and it owns it by
**shelling out to the existing `ordo` binary**, not by reimplementing it:
`ordo plan`, `ordo apply`, `ordo interface` as VS Code tasks and commands,
with the terminal as the output channel. Nothing about those needs to be in
the editor's process.

### 5.2 Two facts the existing plan did not have to face

- **`parse_program` accepts `domain`, `entity` and `op`.** A language server
  pointed at a real `.ordo` file today would refuse `routine`, `when`,
  `every`, `channel`, `route`, `app`, `page` and `principal` — words it
  colours as declarations and cannot parse. **This is the gating dependency
  for increment 2**, not an inconvenience: an editor that underlines a third
  of the settled language as an error is worse than no editor. The ladder in
  §5.3 therefore starts at *files the parser accepts* and grows with the
  parser, and the increment does not start until the trigger and routine
  declaration forms are settled (`trigger.rs`' module doc names the
  §7-versus-§8 contradiction that blocks them).
- **`LexError` has no class, and `Span` has no end position.** Two small,
  concrete pieces of work that belong to this increment and nothing else:
  give `LexError` a `code` from the same inventory the parser's rejections
  use, so the lexical half of the refusals can populate an LSP diagnostic's
  `code`; and give the LSP a byte-offset → `(line, character)` mapper (UTF-16
  code units — the LSP's unit, not Rust's), since `Span` carries the start's
  line/column and both ends' byte offsets but not the end's line/column.

### 5.3 The capability ladder

Each rung ships on its own and is useful without the next. The ordering is by
*what the compiler already knows*, cheapest first.

| Rung | Capability | What it is made of |
|---|---|---|
| **L1** | **Diagnostics, on save** | `lex` → `parse_program` → bind. The closed refusal classes, verbatim, with their real spans. `code` = the class, `message` = the compiler's own text, which `adoption-dx-plan.md` §3.3 already commits to being rule-then-fix. Publish on save first; on-change once §5.2's parse recovery is honest |
| **L2** | **Document symbols + folding ranges** | A pure walk of the parsed `Program`. Entities, ops, routines, their fields and params. Nearly free once L1 parses, and it is what makes a 500-line domain navigable |
| **L3** | **Semantic tokens** | The exact answer to every blind spot in §3.3 — a name resolved to *entity*, *op*, *field*, *param*, *ref target*, delivered as a token type over the TextMate layer rather than replacing it. This is the rung that makes the grammar's approximations invisible |
| **L4** | **Go-to-definition / find-references** | Safe because references are closed and resolvable: a ref's target entity, an op named in a composition, a field in a projection, a routine named after `-> run` |
| **L5** | **Completions from the schema graph** | The rung the closed grammar makes *complete* rather than heuristic: fields after `.` along the ref path, an op's output fields after its name, `$params` in scope, the aggregate set inside `group { }` and only there, the closed function set with its arities, the access verbs after `access`. Every one of these is a set the compiler holds; none is a guess |
| **L6** | **Hover = derived truth** | An op's mode label, terminal verb, cost class, row bound, output-field manifest; an entity's ownership summary. **Read from the interface descriptor, not rewalked** — `ordo_plan::interface` is already a projection of `op_artifact` (checklist 14a), and hover is a projection of *that*. A hover that walked the binder itself would be the third walk of a fact the repository worked to make one |
| **L7** | **Inlay hints** | Inferred join path on a dot-path; cost-class chip per op. `adoption-dx-plan.md` §3.2 calls this how "schema knows the joins" stops being a slogan |
| **L8** | **Rename, formatting** | Rename is safe for the same reason L4 is. `textDocument/formatting` is `ordoc fmt` (Track A1) once it exists — and because one spelling per concept makes formatting *canonicalization*, the formatter has zero options and the LSP exposes none |

**Done when** (per rung): L1 — every refusal class the compiler can raise on
a fixture appears at the right range with the right `code`, asserted by a
test over the conformance fixtures rather than by opening an editor. L3 — the
§3.3 blind-spot list is demonstrably closed on the same corpus. L6 — hover
text is byte-derived from `interface.json` and a golden test says so.

**L5 and L6 have a producer now, and it is not the LSP's to write.**
Checklist 14d/14f landed `crates/ordo-gen` (2026-09-07), whose whole job is
to be the *second* reader of the same two sources this ladder's top rungs
need — `ordo-syntax`'s closed tables and `interface.json`. Concretely:

- **L5's "closed function set with its arities"** is `ordo_syntax::functions()`,
  which the CDK's generation forced into existence (it used to be
  `function_names()`, arities private). The aggregate set with its argument
  shapes is `AGGREGATE_SHAPES`, the same way. A completion provider that
  retyped either would be the fourth statement of a list `editor_grammar.rs`
  already pins in two directions.
- **L6's hover** is a projection of the descriptor, as this table already
  says — and `ordo_gen::sdk` is now a worked example of exactly that
  projection, including the doc text it renders per op (mode label, cost
  class, terminal verbs, the `unguarded` disclosure). The overlap is real
  and it is the *good* kind: hover and the generated doc comment should say
  the same sentence, because they are answering the same question from the
  same row. Where the LSP wants that text, it should read
  `ordo_gen`'s renderer rather than write a second one.

Nothing about the gating changes: the LSP is still blocked on the
trigger/routine declaration form, because `parse_program` accepts `domain`,
`entity` and `op` and nothing else. `ordo-gen`'s `vocabulary.ts` now names
that gap word by word, with a reason per word, which is a ready-made list of
what a server would have to underline as an error today.

---

## 6 · Increment 3 — the VS Code LSP client

`vscode-languageclient`, ~150 lines. Server discovery in this order: a
`ordo.server.path` setting, then a binary bundled in the `.vsix` for the
current platform, then `ordo-lsp` on `PATH`. Settings for trace and for
which `.ordo` roots to treat as one program. A status item naming the server
version and the language version it implements (§9).

**Done when**: opening a folder of `.ordo` files shows diagnostics without
any configuration; killing the server restarts it; the client adds no
analysis of its own — if the client computes anything about Ordo source, it
is a bug.

---

## 7 · JetBrains — the honest ladder

Three rungs, and the recommendation is to walk the first two and stop.

**J1 — the TextMate bundle, zero extra work, available the day increment 0
lands.** JetBrains IDEs import TextMate bundles natively: *Settings → Editor →
TextMate Bundles → +*, pointed at a directory containing
`ordo.tmLanguage.json` and a `package.json` naming it. `editors/` gets that
two-field `package.json` in increment 1 and the same directory serves both
IDE families. Highlighting, `.ordo` association and comment toggling, in
every IntelliJ-platform IDE, for the cost of one file and one paragraph of
documentation. **Done when**: a documented three-step import in
`editors/README.md`, and a screenshot in the release notes.

**J2 — LSP via the platform's own API, once `ordo-lsp` exists.** IDEA
2023.2+ ships `LspServerSupportProvider`; a plugin that does nothing but
point at the same `ordo-lsp` binary is a few hundred lines of Kotlin and
inherits every rung of §5.3 for free. It is worth building the moment L1
ships, because it costs one file per capability the server already has.
Caveat, stated rather than discovered later: the platform LSP API is
**IntelliJ IDEA Ultimate and the other paid IDEs**, not Community — so J1
remains the answer for Community users and the docs must say which is which.

**J3 — a full IntelliJ platform plugin. Ruled out for now.** It means a
second lexer and a second parser written in Kotlin against JFlex and
Grammar-Kit, a second PSI tree, and a second implementation of every rule —
which is exactly the "second answer waiting to differ" this repository
refuses everywhere else, only this time in another language and another
build system. It buys structural refactoring and inspections that L3–L8
already largely cover through the LSP. **Revisit when** someone with a
JetBrains-only team asks for something J2 provably cannot do; not before.

---

## 8 · The other three hosts

- **The docs site.** Tag the fences ```` ```ordo ```` (`docs_parse.rs`
  already prefers a tagged fence and treats `ordo` as the candidate tag) and
  hand shiki `editors/ordo.tmLanguage.json`. This is the highest-leverage
  consumer per unit of work in the whole plan: the corpus is already written,
  the grammar is already built, and every code sample in the manual gains the
  same colours the editor shows.
- **The playground.** `crates/ordo-playground/src/index.html` gains nothing
  from a full editor and should not grow one; the honest options are (a)
  leave the textareas alone and colour only the read-only `<pre>` outputs
  with shiki, or (b) swap the input for CodeMirror 6 with a TextMate bridge.
  (a) is a page-weight-free improvement and should come first. **[open]**
  which, and it depends on whether the playground ever grows a WASM compile
  tier (Track B4) — a highlighter without diagnostics next to a compiler
  button is a curious thing to build.
- **The site's marketing samples.** Same shiki path as the docs; nothing
  extra.

---

## 9 · Distribution and versioning

**The grammar carries the language version it tracks.**
`ordoInventory.languageVersion` is pinned by the parity test against
`ordo-syntax`'s own version, so the artifact that ships to four hosts cannot
be anonymous: "which release of Ordo does my editor understand?" has an
answer inside the file. This is the same identity discipline the interface
descriptor already has with `schema_version` + `digest` (checklist 14a).

**Extension version tracks the language's minor.** `x.y.*` where `x.y` is the
language it highlights and `*` is the extension's own patch stream. A
language release with a new keyword is a new minor of the extension, by
construction, because the parity test will not let the old one build.

**Two marketplaces, one release, and never only one.** VS Code Marketplace
(`vsce publish`) **and** Open VSX (`ovsx publish`) — the second is what
VSCodium, Gitpod, Eclipse Theia and Cursor read, and publishing to only the
first quietly excludes every non-Microsoft build of the editor. Both from the
same GitHub Actions release job, both gated on the workspace test suite,
which is where the parity test lives.

**The server binary.** Per-platform builds attached to a GitHub release,
consumed either as platform-specific `.vsix` targets (the rust-analyzer
pattern — larger artifact matrix, zero first-run network) or downloaded on
first activation. Recommend the former; the latter fails in exactly the
locked-down environments this platform's audience works in. Both need
`Cargo.lock` committed (checklist 15a blocker 0b) before a build is
reproducible enough to sign.

**`npx ordo` and Homebrew** already appear in `tooling-and-embedding-plan.md`
§A2's distribution note; nothing here changes them, and the LSP binary rides
the same channels as the CLI rather than inventing its own.

---

## 10 · Ruled out, with reasons

- **A highlight.js grammar as the primary artifact.** It serves the docs site
  and nothing else: no VS Code, no JetBrains, no Monaco. A TextMate grammar
  serves shiki *and* both IDE families from one file. If a highlight.js
  definition is ever wanted for a third-party site, it should be *generated*
  from `ordoInventory` — never hand-maintained beside it, or the keyword list
  exists twice and the parity test guards only one copy.
- **A Monaco-specific Monarch grammar for the playground.** Monarch is
  Monaco-only, and the playground is the smallest of the four hosts (§0.1:
  it has no highlighting at all today). Building a second grammar for the
  least-demanding consumer inverts the effort. Monaco and CodeMirror both
  consume TextMate via `monaco-textmate`/`shiki`; if the bridge's weight is
  the objection, the answer is §8's option (a), not a second grammar.
- **Writing the LSP in TypeScript.** The compiler is in Rust and the
  diagnostics are the compiler's. A TypeScript server means a second lexer, a
  second parser and a second set of refusal messages — a second answer
  waiting to differ, in the one place where "the editor and the compiler
  disagree" is the most expensive failure a new user can hit. It also
  forfeits every non-VS-Code editor.
- **A full IntelliJ platform plugin, now.** §7, J3.
- **Shipping a colour theme.** Ordo ships scopes, not colours. A bundled
  theme would look wrong in every user's carefully chosen environment and
  would have to be maintained against two marketplaces' theme conventions;
  the two-layer scope naming (§3.1) exists so that stock themes already work.
- **Snippets in increment 1.** A snippet is a hand-maintained fragment of
  grammar that nothing checks — the same defect the parity test exists to
  prevent, one layer up. Completions from the schema graph (L5) are the
  version of this feature that cannot go stale, and they are three rungs away
  rather than three years.

---

## 11 · The open questions this plan does not close

- **[open] When does the trigger/routine declaration form settle?** §5.2
  makes it the gate on increment 2, and `trigger.rs` records why it is not
  settled: §7 and §8 spell it two irreconcilable ways and every routine
  spelling is `[lean]`. The LSP cannot diagnose a form the grammar has not
  chosen, and choosing one to unblock tooling would be the tail wagging the
  language.
- **[open] Does a node toolchain enter the Rust workspace's CI?** §3.5 needs
  it to assert corpus highlighting mechanically, and checklist 14b already
  owes the same answer for `@ordo/react`. One answer should cover both.
- **[open] `definition-language.md` §2's reserved list.** §3.4. A doc change
  with no code consequence, and the manifest is now the mechanically checked
  version of the same claim.
- **[open] The playground's editor.** §8.

---

## 12 · Sequence

| # | Increment | Effort | Gated on |
|---|---|---|---|
| 0 | TextMate grammar + parity test | **done** | — |
| 1 | VS Code extension, shell only | S (a day) | 0 |
| 1b | Docs-site shiki + tagged fences | S | 0 |
| 1c | JetBrains TextMate bundle, documented | S (an hour) | 1's `package.json` |
| 2 | `ordo-lsp` L1–L2 (diagnostics, symbols) | M | the trigger/routine form (§11) |
| 3 | VS Code LSP client | S | 2 |
| 3b | JetBrains LSP plugin | S | 2 |
| 4 | L3–L5 (semantic tokens, definition, completions) | M | 2 |
| 5 | L6–L8 (hover, inlays, rename, format) | M | 4, `ordoc fmt` |

0, 1, 1b and 1c are about 4 agent sessions that put coloured, refusal-aware Ordo
in front of every reader in every host. Everything after them waits on a
language decision that is owed anyway.
