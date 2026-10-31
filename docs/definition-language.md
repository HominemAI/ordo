# Ordo Definition Language

Working spec for Ordo's declarative surface: shapes, queries, writes, triggers, and
platform definition blocks. Ordo preserves the relational data model while defining
stricter, deterministic language **and execution** semantics; everything compiles to
SQL/DDL/commands.

Status markers: **[settled]** agreed in design discussion · **[lean]** current
recommendation, not contested · **[open]** needs a decision.

---

## 1. Design principles [settled]

1. **Closed grammar.** One spelling per concept. No synonyms, no natural-language
   forms, no paraphrases. If a construct can be written two ways, it's designed
   wrong. (Anti-hallucination: the full grammar should fit in a page of system
   prompt; an LLM emitting a "plausible variant" must fail to parse, never parse to
   something else.)
2. **No precedence table.** Mixing different boolean operators without parentheses
   is a parse error. `a and b or c` does not compile; `(a and b) or c` does.
   Same-operator chains need no parens (`a and b and c`).
3. **Schema knows the joins.** Relationships are declared once as refs in shapes;
   queries traverse them (dot-paths, inferred match). Re-stating relationships per
   query (SQL's JOIN…ON ceremony) does not exist.
4. **Position is scope.** A condition's meaning comes from where it sits (source
   parens / pipeline bar / aggregate parens), never from extra keywords (no HAVING).
5. **Defaults or required.** Every field is either defaulted or required, visibly,
   at declaration. No third state.
6. **The server owns mechanics.** Primary keys, FK indexes, join planning, encryption
   placement, migration generation: all derived, never hand-written.
7. **Everything registers.** Queries and writes compile at deploy time into named,
   versioned API operations with declared scopes. No free-form query text at runtime
   (dev/ops mode excepted, behind elevated permission).
8. **One door in.** All writes compile to attributed commands through the gate.
   There is no raw UPDATE path in the language.

---

## 2. Lexical basics

### Typed ids [settled]
Globally unique, self-describing, server-generated. Prefix encodes entity type;
UUIDv7 underneath (time-ordered, index-friendly).

**Built (2026-10-06, 25e):** a deployment mints `<key>_w<ms><prefix>_<count>`,
in lower-case hex: the milliseconds since the epoch (12 digits), a random
64-bit prefix per write log, then a 12-digit count. So ids sort by age to the
millisecond, as a UUIDv7 does. The test suites mint from a plain counter.

```
txn_01J8ZKQ4X7...    acct_01J8ZL2M9P...    spc_01J8ZKXA01...
```

Any reference seen anywhere (command log, audit trail, debug session) is
determinable on sight. Primary keys are never declared and never appear in shapes.

### Literals [settled]
- Dates: bare ISO, such as `2026-07-01` or `2026-07-01T14:30:00Z`. **The date must be a
  real Gregorian date**, leap rule included: `2026-02-31`, `2026-04-31` and
  `2025-02-29` are lexical errors, `2024-02-29` is not. Shape alone was never
  the rule; a literal that cannot occur should not survive the lexer.
- Durations: `<int><unit>`, units exactly: `ms s m h d w mo y`
  (`m` = minutes, `mo` = months; spelling, never context, disambiguates)
- Strings: double quotes `"Amazon"`
- Money: minor-units integer semantics under a `money(p)` type, written in major
  units and converted at the declared places (display formatting is a client
  concern)

### Lexical formalization [settled]

Folds in engine-plan G1–G8 and G13 as spec text.

- **Comments**: `#` starts a comment to end of line, anywhere outside a string
  literal. No block comments (one spelling).
- **Identifiers [settled]**: **one rule for every declared name.** Entities,
  fields, ops, routines, domains, principals, clients, channels, apps, pages,
  and policies all match `[a-z][A-Za-z0-9]*`. That is lowerCamel, no underscores
  (`_` stays reserved for typed-id literals), **no hyphens**. Parameters are
  `$` + the same rule. Max 64 characters. The hyphen is not an identifier
  character anywhere: `lease-expiry` lexes as `lease` minus `expiry`, because
  unary/binary minus is legal in expressions and position cannot rescue it.
  Hyphenated names in older examples are **wrong, not an exception**. Use
  `leaseExpiry`, `spendWatch`, `cameraHub`. The only hyphens that survive are
  inside platform-fixed **attribution strings** (`source:feed-<name>`,
  `system:break-glass/<id>`), which are literals in the command log, not
  identifiers in the grammar.
- **Reserved words**: keywords are reserved and cannot name fields/entities:
  `and or not in is null true false match any none on group reduce sort take
  append let distinct under above since contains starts matches set del
  subtree each op entity routine domain when every trigger check index unique
  sealed indexed owned tree normalize whole acl access app page principal
  client auth policy compliance channel space unguarded unbounded
  reproducible nopause public filled cond`. These are the logic and pipeline words, and every
  word that can begin a declaration, a clause or a field modifier. Everything
  else is **contextual**, legal as a field name and read by position: the
  `access` verbs `read` and `write` and
  the explicit-open spelling `open` (G9; `open` is **not** a boolean literal
  and may not appear inside a guard walk or any expression), the shape line
  `std sealed`, the words inside `routine`, `auth`, `policy`, `app` and `page`
  blocks (`run method methods share rollup live table metric detail route`),
  and every aggregate and function name (`count sum avg min max median
  percentile joined coalesce round …`). Position always disambiguates, which
  keeps the reserved list short. The list is **closed and checked against the
  grammar**: it is `ordo_syntax::token::Keyword` in declaration order, and
  `crates/ordo-syntax/tests/reserved_words.rs` fails when the two differ in
  either direction. The same test holds every entry to a reject case
  (`entity x { access: int }` must fail) and every contextual word named here
  to a pass (`entity x { read: int }` is legal).
  After a dot, a reserved word is a path segment (`groupMembers.group`): nothing
  but a segment can follow a dot, so the position decides there too.
- **String literals**: double quotes; escapes exactly `\" \\ \n \t`, a closed
  set. No other escapes, no single quotes, no interpolation (concatenation
  is `+`).
- **Numeric & money literals**: `int` literals are decimal digits. Decimal
  literals (`10.50`) are money/decimal literals: exact decimal, never float.
  Money fields accept decimal literals in **major units** (`10.50`); the
  minor-units integer semantics above are storage, not spelling. The server
  owns those mechanics. Unary minus is legal in expressions (`-1050`, `-(a + b)`);
  after `sort` or projection-`*`, a leading `-` means descending/exclusion.
  Position disambiguates, as with brackets.
- **Line structure**: the grammar is line-oriented (one transform per line). A
  step ends at end-of-line **unless a bracket/paren/brace is open**. Open
  delimiters continue the line (this is what every multi-line `group { … }`
  and `cond(…)` example does). No continuation character. Blank lines and
  comment-only lines are insignificant. Indentation is style, not syntax.
- **Bare boolean fields as conditions**: a bare field of type `bool` is a
  complete condition (`alerts(active)` ≡ `active == true`); any other bare
  expression in condition position is a compile error.
- **`append` arity**: `append <source>` takes exactly one source form (entity
  or registered name, with parens/projection), never a nested pipeline.
  Multiple appends chain (`append a … append b`); a branch needing its own
  steps is a `let` first.
- **`since` anchor precision**: `since` on a `date` field compares against the
  execution-time UTC **date**; on a `datetime` field against the
  execution-time UTC **instant**.
- **`since <months>` clamping [settled]**: `since 1mo` means "at or after
  (execution anchor − 1mo)", and the subtraction uses the **same clamping rule
  as all month arithmetic** (see Duration semantics): the day-of-month is
  clamped to the target month's last day. On 2026-03-31, `since 1mo` anchors at
  2026-02-28. `mo`/`y` windows therefore have non-uniform lengths. That is the
  calendar, stated openly. Windows that must be uniform use `d`.

Text collation and the `hash(x)` algorithm are **ruled** (2026-08-30,
[`corpus-rulings.md`](corpus-rulings.md), "P0's last two gaps"); engine-plan
G11/G12 carry the cross-reference and their ⚠ marks are withdrawn. (G10's
duration comparability is ruled below.)

- **Text ordering [settled]** is **Unicode codepoint order over the stored
  NFC form**. It is the language's order, not the engine's, because it is the only
  order that is a function of the value alone, with no collation table
  on somebody else's release cadence. It is emitted as `COLLATE "C"` on every
  text ordering, which for valid UTF-8 is exactly codepoint order. Stated
  because it is user-visible: `Z` sorts before `a`, so Ordo does not sort text
  the way a phone book does. A locale-aware order per sort key is **refused**;
  an author who needs one stores the fold in an ordinary `text` field and
  sorts by that. Text canonicalization below is the other half of this rule:
  the order is codepoint order *over NFC*, and NFC-on-write is what makes the
  bytes deterministic given the text.
- **`hash(x)` [settled]** is the **SHA-256** digest of the value's UTF-8
  bytes, first eight bytes big-endian, top bit cleared (so
  `mod(hash(id), n)` is a bucket in `[0, n)` and not a signed remainder);
  `null` hashes to `null`; the domain is `text` and `id`. Frozen at first
  release and only ever added to. A different digest is a differently-named
  function, never a new meaning for this one, because changing it silently
  reshuffles every cohort and stripe already computed from it.

### Types [settled]

> **Money and decimals, ruled 2026-10-04 (checklist 27ak).** `money` stays
> whole minor units in storage, and each field declares its currency's
> places: `price: money(2)`, `yen: money(0)`. Bare `money` is `money(0)`,
> so a program written before places existed keeps its meaning, and
> `ordo check` warns at each bare `money` that new code should state its
> places (ruled 2026-10-05). The warning never fails the check.
> **12 places is the ceiling** for `money(p)` and `decimal(p)`, and a
> program asking for more fails to parse. It is a fact of the language and
> not a setting: 12 is the scale `/` and `avg` compute at, so a stored value
> widened into an expression is never rounded (ruled 2026-10-05).
> Source writes major units (`29.99`) and the compiler converts, as the
> literal rule below has always said. **Built:** places on fields and
> parameters, the literal conversion in conditions, writes, defaults,
> parameters, bounds and `check`, and `money_places_mismatch` when two
> places meet. A literal finer than a slot's places is `implicit_rounding`;
> in a comparison it is compared exactly. A new storable `decimal(p)` field
> holds rates and unit prices at any precision; it becomes `money` only
> through an explicit `round`. **Built:** `decimal(p)` fields and
> parameters, stored as `numeric(1000, p)`, exact on the wire as a decimal
> string; a decimal landing in a `money` or `int` slot without
> `round`/`floor`/`ceil` is `implicit_rounding`, and a value finer than a
> `decimal(p)` slot is too. The passage below that calls `decimal` an
> expression type only predates the ruling.
>
> How money meets other numbers: an `int` or a literal beside a money is
> in major units (`price > 30` is $30.00). A money times a count is a money.
> A money times a money is a decimal at every places value, `money(0)`
> included: `yen * yen` is a `decimal(0)`, and like `price * price` it needs
> a `round` to land in a money slot (ruled 2026-10-05).
> A money times a decimal, divided by anything, averaged, or passed to
> `round`/`floor`/`ceil` is read in major units (`price * 0.01` at places
> 2), so `round(price * 0.08, 2)` is a tax to the cent and lands in a
> `money(2)` slot. `round(x, n)` lands in `money(p)` when `n <= p`.
> `allocate` splits in the minor units of the total's places: `100.00` at
> places 2 is `33.34 / 33.33 / 33.33`, `100` at places 0 is `34 / 33 / 33`.
> That rule is built as `ordo_plan::allocate_at_places`; its `each` site is
> still lean, so no program calls it yet.

Closed inventory for fields and parameters: `text` (boundable `text(64)`),
`int`, `money` (with its currency places, `money(2)`; bare `money` is
`money(0)`), `decimal(p)` (an exact decimal with `p` places, for rates and
unit prices; `p` runs from 0 to 12 for both), `bool`, `date`, `datetime`, `duration` (unit-classed:
`duration(ms)` is the exact class, bare `duration` the calendar class; see
Duration semantics), `id`, `json` (a JSON document, stored as `jsonb`; see
"JSON documents" below). Durations are
first-class (`everyDays: duration = 7d`).

> **Not built yet [2026-10-04]: bare `duration` does not parse.** This is
> settled and owed by the compiler. The compiler requires `duration(<unit>)`
> and stores one unit per field. It has no calendar class, no
> `incomparable_duration` and no `duration_unit_class`. Building bare
> `duration` means building the two-component model in Duration semantics
> below, which replaces the engine's duration type
> (`audit-doc-contradictions.md` F4). `text(64)` is built: the database keeps
> the bound on an open column, the write path on every column, and the door on
> a parameter. Each counts characters. The door answers `bound_violation`; a
> field's bound is a row invariant, so a longer field value is
> `check_violation`.

**`decimal` is an expression type
only [settled]**. It is what `/` and `avg` produce, it has no field
declaration form, and it never lands anywhere by itself: every decimal that
reaches a `money`/`int` field, projection, or write RHS must be converted by an
explicit `round(x, n)` / `floor(x)` / `ceil(x)` (see Numeric semantics). Date
arithmetic is blessed in
expressions (`date ± duration`, `int * duration`), pure and deterministic.
Execution-relative dates are passed as parameters (`$today: date`, SDK-fillable)
and never computed in queries. There is no `now`; `since` remains the only nondeterminism.

**Numeric semantics [settled]:** `int` and `money` are storage types;
**expressions evaluate in exact decimal** (arbitrary precision, no float
anywhere) for `+ - *`.
`avg`/`min`/`max` of an empty set are null (sum stays 0, count stays 0).
Overflow on `int`/`money` is a typed runtime error, never wraparound.

**No implicit rounding, anywhere [settled].** A `decimal` value **landing in a
typed slot** (a projection declared `money`/`int`, a write RHS for a
`money`/`int` field, an aggregate argument that must return one) must be
converted by an explicit `round(x, n)`, `floor(x)`, or `ceil(x)`. An unconverted
decimal in such a slot is a **compile error**, class `implicit_rounding`.
**Comparisons are not typed slots**: `value > total / 3` widens both sides to
decimal and compares exactly at the pinned scale, with no conversion and no
rounding. Nothing is being stored, so nothing needs a mode. This deletes the
earlier rule ("rounding happens at the typed boundary using round-half-even").
Comparisons round nothing, so they need no mode at all.

**`round` is half-up, globally and unconfigurably [settled].** An earlier
version of this passage said the opposite: that half-even is the accounting norm
while half-up is required by several tax authorities, that no correct choice
spans jurisdictions, and that the language must therefore "make the author write
it." That obligation was never spellable. The closed function list offers only
`round(x)` and `round(x, n)`, neither of which contains a mode, and the parser
rejects a third argument. A mandatory author decision with no syntax is worse
than either default, because two conforming implementations may answer
`round(2.5, 0)` differently and nothing in the source distinguishes them
(M4-002).

So the language pins **half-up (away from zero)**, matching the scale-reduction
rule that division already uses, so there is exactly one tie-breaking behaviour
in the whole numeric surface, not two that look alike. Adding
`round(x, n, halfEven|halfUp)` was considered and declined: it reopens the
closed function list, and it puts a mode argument on the hot path of every
money projection to serve a case most applications never hit.

**Where half-even is genuinely required, the application computes it.**
Half-even is expressible from the primitives that exist, and an app whose
jurisdiction demands it writes that once in a `std`-style helper where a
reviewer and an auditor can both find it. That is the honest version of "the
author writes it, once, visibly." It was always going to live in application
code, and pretending the type system carried it hid the gap.

**Division: determinism across venues [settled].** `1/3` has no finite decimal
form, so exactness and venue-independence cannot both be unconditional, and
venue-independence wins: the cost-class system moves stages between SQL and the
host freely, and an expression whose *truth value* depends on where it ran is
not a language.

- **`/` and `avg` produce `decimal` at a fixed FRACTIONAL SCALE of 12**: twelve
  digits after the decimal point, not a significant-digit budget. The scale is
  the same number everywhere and is not configurable. (The previous pin, "38
  significant digits", is **withdrawn as unimplementable**. `NUMERIC(p, s)`
  expresses a fractional scale, not a count of significant digits, so the casts
  that pin prescribed could not have expressed it in either venue.)
- The pin is applied **identically host-side and in SQL**: generated SQL
  evaluates in PostgreSQL's **unconstrained `numeric`** and applies an explicit
  `round(value, 12)` at every division and average, and the host evaluates at
  the same fixed scale. (The earlier spelling, a `NUMERIC(38, 12)` cast, is
  **withdrawn** for the same class of reason as the pin before it, M4-001: a
  fixed typmod bounds *magnitude*, leaving only 26 integer digits, and so
  imposes a limit the arbitrary-precision `+ - *` algebra does not. It admitted
  expressions the host answers and SQL must overflow on. `round(…, 12)` fixes
  the fractional scale, which is the whole of what this pin asserts, without
  bounding magnitude at all.) Same expression, same rows, same answer,
  either side of the sealed boundary. (The alternative, true rationals with a
  forced host-side venue, was declined. It prices arbitrary-precision division
  as a silent `bounded-host`/`unbounded-host` promotion, which is a cost the
  cost-class system had not accounted for.)
- **Scale reduction at the division itself is half-up**, because that is what
  `NUMERIC` division does natively in Postgres and matching it is the only way
  the two venues agree digit-for-digit without reimplementing Postgres's
  arithmetic host-side. This is a **mechanic of the fixed-scale decimal type**,
  not the language rounding a result on the author's behalf: it happens at digit
  13 of an intermediate, it is the same in both venues, and it never decides how
  many cents land in a field. That remains an explicit `round`.
- **Irrationals cannot arise**, so repeating decimals are the entire problem. The
  expression grammar has no `sqrt`, `exp`, `log`, or general `pow`; every
  operator (`+ - * /`, the aggregates, the date parts) is closed over the
  rationals. A fixed fractional scale therefore bounds the *only* value class the
  language can produce that has no finite decimal form.
- The exactness claim is scoped accordingly: **comparisons never round *before*
  the pinned scale.** `value > 3 * baseline.a` is exact: no division, no
  rounding. `value > total / 3` is exact to scale 12 and then compared exactly.
  There is no other rounding step in between.
- **`/ 0` is a typed runtime error**, uniform with `mod(a, 0)`. Division is
  total on its domain and loud off it. There is no null-on-divide-by-zero
  convention.

**Splitting money is `allocate`, not `/` [settled].** Dividing money by a count
is almost never the operation an author means: they mean a split whose parts sum
back to the total, and `/` cannot promise that at any scale. The language
therefore blesses the primitive every ledger system ends up writing by hand,
`allocate(total, weights)` (Expression functions, §3), and leaves `/` to mean
division. `sum(x / 3)` is the sum of three scale-12 thirds and is a *number*, not
a split; `100.00` split three ways is `allocate`, which yields 33.34 / 33.33 /
33.33 and sums to exactly 100.00. The platform still never silently
redistributes remainders, but it no longer forces the author to hand-roll the
redistribution.

**Duration semantics [settled].** A duration is a **two-component value**
`(months, milliseconds)`. `mo`/`y` fill the months component, `ms s m h d w`
fill the milliseconds component. The components never convert into each other,
because no conversion is correct (a month is not 30 days and never was).
Everything else follows:

- **`==` / `!=` are structural component equality**. They are total, defined on every
  pair, with no errors. `1mo != 30d` and that is the whole story; `1y == 12mo` (same
  months, same ms); `7d == 168h` (same ms). This keeps the partition theorem
  (§4): every row is either equal or unequal, and a filter can never abort an op
  on one row's stored value.
- **Ordering is a property of the TYPE, not of a proof [settled].** A `duration`
  field or parameter declares a **unit class** at the declaration site:
  `duration(ms)` is the **exact** class (its values carry a zero months
  component by construction), and bare `duration` is the **calendar** class,
  which may carry months. The class is part of the type, so it is checked where
  it is declared and where values flow, not inferred from what happens to reach a
  comparison.
  - Writing a months-bearing value into a `duration(ms)` field or binding one to
    a `duration(ms)` parameter is a **typed rejection at the door**, class
    `duration_unit_class`. This is the one place the check cannot be static, because the
    value arrives at call time.
  - **`< <= > >=` and `sort` are legal only on the exact class.** Ordering or
    sorting a calendar-class duration is a **compile error** at the point of use,
    class `incomparable_duration`, and so is comparing across classes
    (`1mo < 30d`, where the literal `1mo` is calendar-class). Literals adopt the class
    of the other operand when they can: `30d` is exact-class in an exact-class
    comparison.
  - `==` / `!=` remain total across both classes (structural component
    equality), so the partition theorem is untouched.

  This replaces the earlier "the compiler proves every duration reaching the
  comparison is single-component" rule, which was **not provable and is
  withdrawn**. Any registered op may write `1mo` into a duration *field*, and a
  duration *parameter*'s value exists only at call time, so no static analysis
  over the op set could discharge the obligation. Making it a type property makes
  it decidable, and puts the diagnostic on the declaration the author controls.
  There is still no runtime *comparability* error class. The runtime rejection
  above is about admitting a value into a class, which happens at the door with
  every other bound check. (This answers engine-plan G10's request for a spec
  ruling. It is stronger than G10's "compile error where detectable, typed
  runtime error otherwise" in the place that matters, the comparison, and
  it is honest about the one place a value must be checked when it arrives.)
- **Month arithmetic clamps, and clamping is not associative.** `date + 1mo`
  keeps the day-of-month, clamping to the target month's last day:

  ```
  2026-01-31 + 1mo          -> 2026-02-28      # clamped
  2026-01-31 + 1mo + 1mo    -> 2026-03-28      # clamp, then keep 28
  2026-01-31 + 2mo          -> 2026-03-31      # one clamp-free step
  ```

  So `d + 1mo + 1mo ≠ d + 2mo`, and with `int * duration` blessed,
  `d + 2*(1mo) ≠ (d + 1mo) + 1mo`. This is a property of calendars, not a bug,
  and the language does not hide it: **write the arithmetic in one step when you
  mean one step.** Millisecond-component arithmetic is associative and exact.
- Month clamping is the same rule everywhere it appears: `+`/`-` on dates,
  `since <months>`, and any `duration` default.

**Content types [settled]:** `blob` is managed binary content. The row stores a
reference (blob id, mime, size, sealed integrity hash); bytes live in object
storage (R2/S3/GCS per deployment; per-space **BYO bucket** via the same
resolver pattern as BYO-DB). Blobs are immutable **because a blob id is
never reused, not because it is derived from the content** (see "Blob
addressing" below), so serving URLs are permanently cacheable.
Blobs are garbage-collected by reference
(compiler knows every blob field; nightly sweep reclaims unreferenced content),
included in space export, and crypto-erased with the space. `blob private` is
user-sealed custody: the client encrypts before upload with a passkey-PRF-
derived key, and the platform stores ciphertext and holds no key. No server-side
processing (vision, thumbnails, search) is possible, by design. `uri` is an
external pointer, no custody: address + optional integrity hash; no serving,
no GC, no export of bytes (consent page states "referenced, not held").

**Blob addressing [settled]:** a blob id is **128 bits drawn from a
per-space, per-epoch keyed stream**, namely `HMAC(salt("$blob",
"handle.v1:<epoch>"), 32 fresh discarded CSPRNG bytes)[..16]`. It is the same
derivation the sealed-payload handle uses, one domain over. It is **never**
derived from the bytes, the ciphertext, the mime type, the size, the row, or
a counter. Three consequences, each deliberate:

- **No deduplication.** Two uploads of one file store it twice. Dedup *is*
  the equality oracle: two rows naming one object would prove the two
  uploads equal, whatever the ids looked like. The cost is real and the
  shape summary prices it.
- **Integrity is a *sealed* digest** on the reference row, not a plaintext
  one. A plaintext digest column is the content hash by another name, and it
  answers "do these two tenants hold the same file?" to anyone with
  bucket-level read, forever, surviving crypto-erasure, because destroying
  a key does not destroy a hash.
- **Immutability comes from never reusing an id**, which a keyed-random id
  minted per upload and inserted (never upserted) gives exactly.

The earlier phrasing, "new content = new blob id", read as though content
addressing were the mechanism. It is not, and the same ruling was already
made and recorded once for the sealed-payload handle
(`security-logging-design.md` §7), which is why re-making it here rather
than leaving the sentence to be implemented from.

**What a `blob` refuses [settled].** A blob value is opaque, so the compiler
refuses the declarations and expressions that would promise otherwise. Each
is a named class, not a silent ignore:

| Class | Refused | Because |
|---|---|---|
| `blob_not_indexable` | `unique` / `indexed` on a `blob` | A blind index over a keyed-random handle indexes randomness; two uploads of one file compare unequal. Index `f.mime` or `f.size` instead. |
| `blob_default` | `= <literal>` on a `blob` | The only producer of a blob value is an upload; a literal here names an object nobody uploaded. |
| `blob_required` | a `blob` field without `?` | An upload addresses an existing row, so the field is empty between the row's creation and the upload. This is also what keeps the derived `std.E.create` insert-complete while excluding blob fields from its parameters. |
| `blob_not_normalizable` | `normalize(...)` on a `blob` | `normalize` canonicalizes text before equality and the blind index; a blob has neither. |
| `blob_parameter` | `$f: blob` | Bytes are not a JSON wire value. They arrive through the blob door, which is why that door exists. |
| `inline_without_blob` | `inline` on a non-`blob` field | `inline` chooses where **bytes** are stored, and nothing else has bytes. |
| `private_without_blob` | `private` on a non-`blob` field | `private` is client-sealed custody over **bytes**. To seal an ordinary field under the space's key, write `sealed`, which is different custody and a different consent page. |
| `conflicting_custody` | `blob sealed private` | `sealed` seals under the space's key (the platform can open it for a caller who passed the door); `private` seals under a key the platform never holds. Together they name two positions, not a stronger one. |

**`blob private` is declarable and not yet servable [settled that it is
declared; the ceremony is F7 Phase 6].** The parser admits `private`, the
schema carries it, the shape summary and the consent page can name the
tier. Every runtime path that would have to touch the plaintext
refuses **by name**, with the reason:

- the blob door refuses an upload, because it seals on the way in and
  skipping the sealer would put whatever the client sent into the bucket
  with nothing having checked it was sealed;
- the blob door refuses a read, because the platform holds no key and
  streaming ciphertext through a path whose contract is "these bytes are
  the file" would hand a caller ciphertext labelled as their document;
- an **open** export of a `private` object exports it **as stored**, which
  is the same honest partial answer `SealedMode::Ciphertext` gives for a
  space whose envelopes the deployment cannot open. It is not a file of nulls.

What is owed is the passkey-PRF derivation, the device wrapping and the
presigned direct-to-bucket upload (`f7-vault-plan.md` Phase 6). The seam is
built: the store never inspects a byte, so a client-sealed object rides the
identical `put`/`get`. Admitting the word and refusing the act is
deliberate. A language that cannot spell the tier cannot refuse it by
name, and "unknown modifier `private`" is a worse answer than "this is
client-sealed custody and the key ceremony has not landed".

A blob is also not comparable, not sortable, and not aggregatable except by
`count` (`param_type_mismatch` / `argument_type_mismatch`, which are existing
classes, not new ones; the standing rule is that a new refusal class
is an oracle until argued). `f.mime` and `f.size` are ordinary sealed values
and behave ordinarily: comparable, sortable, indexable.

> **Not built yet [2026-10-04]: `f.mime` and `f.size` do not resolve.** This
> is settled and owed by the compiler, which answers `not_a_ref` for
> `photo.mime`. The two values live in the `$blobref` row as sealed envelopes,
> for every blob field (`ordo-crypto/src/blobref.rs`). Reading one is a join to
> that table plus a host-side open, which the binder and the planner do not
> make. Comparing or sorting one is host work over a filter or a sort key,
> which is refused today for every sealed value. Indexing one needs a blind
> index over that row, which does not exist.

`uri` refuses none of these. It is an address, and an address is a value:
comparable, sortable, uniquable, sealable, blind-indexable, and a legal
parameter type validated at the door for shape (a scheme, a body, no control
characters, at most 2048 bytes) and for nothing else. The platform
references a `uri` and does not fetch it, so it has no opinion about where it
points.

**Blob storage pipeline [settled]:** one straight path from bytes to the sealer (per custody)
to the space's resolved store. The store resolves per space:
platform R2/S3 by default, **BYO bucket** as a first-class resolver target.
Two pins: (1) **attaching or changing a space's BYO bucket (or BYO
database) is itself a granted operation**, with space-owner consent, step-up
gating, and command logging. Storage custody never changes silently, and the
platform's credential for the target lives behind the same resolver pattern
as keys (`KeyProvider`-style, never in definitions). (2) **`blob inline`** is a
per-field developer choice to store the bytes *in the database row* instead
of object storage. The blob lives inside the sealed payload (inherits the
space DEK with zero extra machinery), serves only through the API (no
presigned path), exports and crypto-erases with the row, and is
GC-free (row-owned). Intended for small content (avatars, thumbnails,
attachments under the policy cap, default 256 KB, policy-tunable);
oversize writes are a typed rejection, and the shape summary prices the
choice (inline = DB storage + backup weight; external = the R2/S3 path).
The three custody tiers (`blob` / `blob private` / `uri`) compose with
`inline` orthogonally, except `private inline`. Client-sealed bytes ride
the row's payload identically, so it is legal and means "small,
user-keyed, in-row."

**Object storage always holds ciphertext** for sealed content. Who encrypts
differs: space-sealed = the platform seals with the space DEK before the bytes
reach the bucket (a leaked bucket is noise; serving decrypts in the platform
path after access check + key release); `private` = the client sealed it (the
platform streams ciphertext as-is; the client decrypts in a chunked stream format
so progressive/range reads work). Consequences: uploads for space-sealed blobs
proxy through the API (bytes must pass the sealer); `private` uploads may go
**presigned direct-to-bucket** (already ciphertext, so cheap for huge files).
BYO buckets get the BYO-DB custody choice: platform-sealed (their bucket holds
ciphertext they can't read alone) or their own perimeter.

**Share links [settled]:** minting is an op that is access-checked and command-logged
(who shared what, expiring when), with TTL bounded by space policy, revocable (link
ids checked on serve). Distinct from internal URLs (session-authenticated,
immutable-cacheable): share links are bearer, so expiry is mandatory. Delivery:
space-sealed content serves through the platform (decrypt path); `private`
content may serve as a presigned bucket URL with the decryption key in the URL
**fragment** (`…#k=…`, and fragments never reach any server). The platform
enforces availability (expiry, revocation, logging) while key material rides
client-to-client. Server-blind sharing with a countdown.

**Text canonicalization [settled]:** stored text is Unicode-normalized (NFC)
platform-wide. Fields may declare `normalize(lower)`, applied on write, so
equality, uniqueness, and **blind indexes** operate on canonical bytes
(`email: text? sealed indexed unique normalize(lower)` gives auth-grade email
identity; without it, case-variant duplicates are legal).

[lean] `enum("open", "done", "void")` as a field type, for validation, migration
checks, and generated-UI dropdowns. [lean] list params + `each` for atomic batches:
`$lines: list of { descr: text, amount: money }` consumed by
`set invoiceLines each $lines { invoice: $inv, descr: item.descr, … }`. This is the one
iteration construct, param-driven only, with all rows in the op's single command
group.

> **Not built yet [2026-10-04]: the compiler refuses `enum(...)`, `list of
> {...}` and `set ... each $list` today.** `enum` and `list` answer "the type
> set is closed"; the param-driven `each` write has no parser arm. (`each ->`
> inside a routine, which emits writes per row of a read, is built: see
> `howto-routines.md`.) The idiom that compiles
> for a closed set of words is a defaulted `text` plus a `check` listing them
> (`examples/syntax/04-enum-like-values.ordo`).

### JSON documents [built 2026-10-08, 28b]

`json` holds one JSON document: an object, an array, a string, a number or a
boolean. It travels on the wire as JSON itself, never as a string holding JSON.
`json?` admits null.

```
entity pages {
  title:    text
  settings: json = {}
  draft:    json?
}

op saveDraft($p: id, $rev: int, $draft: json) =
  set pages { id: $p, rev: $rev, draft: $draft }
```

- **Storage.** Postgres `jsonb`. A `sealed` json field is `bytea`, as a sealed
  `text` is: the compact JSON text, encrypted.
- **Defaults.** `= {}`, `= []`, or `= null` on a `json?` field. Anything else
  is a parse error or `json_default`.
- **Limits.** The door refuses a document over `ORDO_JSON_MAX_BYTES` (default
  262144, measured on the compact form) or `ORDO_JSON_MAX_DEPTH` (default 64;
  `{}` is depth 1) as `bound_violation`. The deployment owner sets both. A NUL
  in a string or key is `invalid_param`, because `jsonb` cannot store it.
- **Filtering inside a document [built 2026-10-08].** A condition can walk
  into a document with a path: `settings.theme == "dark"`. Segments after the
  json field are steps inside it. A key that is not a name goes in brackets,
  `settings["font size"]`, and an array position is its digits, `tags[0]`. A
  path may hop refs first and walk into a document at its end:
  `author.card.role == "admin"`.
- **A path takes its type from the other side.** Compared with text, it is
  read as text. Compared with an `int` or a `decimal`, it is read as an exact
  `numeric`, never a float. Compared with a bool, it is read as a bool. The
  other side may be a literal, a parameter or a typed field. Two paths
  compared with each other, a path compared with `null`, with a whole
  document or with another type, and a path alone as a condition are
  `json_path_type`.
- **The operators.** `== != < <= > >=`, `in (...)`, `starts`, `matches`,
  `contains`, `is null` and `is not null`.
- **A value of the wrong JSON type reads as absent.** `settings.size > 3`
  does not match `"size": "big"`, and raises no error. Since `!=` is
  `not ==`, an absent value is `!=` any value, as a null field is.
- **`is null` is true when the key is missing or holds JSON `null`.**
- **`contains` is settled by the value's JSON type.** An array contains an
  element equal to the other side: `tags contains "red"` matches
  `["red", "blue"]`. A string contains it as a substring. A number or a bool
  on the other side asks only the array question. So the two readings never
  meet on one value. A whole json field takes `contains` the same way.
- **Still opaque.** Sort, group key, aggregate, function and arithmetic refuse
  a document and a path into one, and so does a projection of a path. A
  whole document is compared with nothing but `is null`. The class is
  `json_opaque`. `unique`, `index`, `indexed`, `refinable`, `normalize` and
  `check` refuse a json field too. A path into a `sealed` json field is
  `sealed_unsupported`: the document is ciphertext. A `$refine` condition on
  a json field is `invalid_refinement`. No index serves a path yet, so a
  filter on one reads every row the rest of the condition leaves.
- **No mixing.** A text never lands in a json field, and a document never lands
  in another field: `param_type_mismatch`.
- **Writes.** A write replaces the whole document and converges by `rev`, as
  any field does.
- **"Exact" means the same JSON value.** `jsonb` drops whitespace, orders keys
  its own way and keeps the last of two equal keys. Strings are stored NFC.
- **Schema change.** `text` to `json` applies when every stored value parses,
  and is refused naming the rows otherwise. `json` to `text` always applies.
  Both move `rev` on every row. A sealed field changing between the two is not
  detected yet (open question in the plan).

Design, tests and open questions: `docs/plan-json-field.md`. Example:
`examples/syntax/19-json-documents.ordo`.

### Parameters [settled]
`$name`, always. Parameters are `$`-prefixed and fields never are, so the two can't
collide or be misread. Parameters are declared with type and bounds at operation
registration (wrapper syntax settled, see §13):

```
$min: int <= 100        $since: date        $payee: text(64)
```

Bounds are enforced at the door, before planning. `text(64)` counts
characters, and a longer parameter is `bound_violation`. On a field the same
bound is a row invariant, and a longer value is `check_violation`.

**`filled`, the SDK-supplied marker [settled].** A parameter may be marked
`filled`, meaning the caller *may* supply it and the generated client fills it
when the caller does not:

```
op thirsty($today: date filled) = …
```

It exists because execution-relative values (`today`, the caller's clock) are
banned from op bodies (the only nondeterminism is `since`), so they must
arrive as parameters, and a generated surface (`ui-gen.md`'s app manifest)
must bind every required parameter without a human typing today's date into a
form. `filled` marks exactly those. It is **not** a default (the value is not
in the artifact and does not appear in the command's resolved fields as one),
and it is **not** optional (the parameter is still required; the SDK simply
knows how to produce it). A caller may always override. `filled` marks only
a `date` or a `datetime`: those are the values a client can produce alone,
from its own clock. On any other type it is a compile error (class
`filled_type`, ruled 2026-10-05). Marked `filled` and
also `reproducible` on the same op is a compile error (class
`reproducible_clock`, the same class a `since` under `reproducible` raises):
filling from a clock is the nondeterminism `reproducible` forbids.

**`$caller` [settled]** is the one reserved parameter: the calling principal's
id, bound by the runtime from the session, **never suppliable by the
client** (a request carrying it is rejected). It is legal anywhere an id-typed
parameter is. This is how caller-scoped visibility is *written in the body*:

```
op myExpenses =
  expenses(member == $caller)[date, amount, category.name]
```

Visibility shaping therefore always lives in the op, in its manifest, its
hover, and its catalog entry, never in an invisible policy. `$caller`
conditions are ordinary indexed ref equalities (`sql` cost class).

---

## 3. Shapes (entities)

### Syntax [settled]

```
entity transactions {
  account -> accounts                 # ref: required (no default)
  amount:  money                      # required
  date:    date  = today
  cleared: bool  = false
  notes:   text  = ""        sealed
  payee -> payees?           sealed indexed
  category -> categories?
  index (account, date)
}
```

Field states, all three visible at a glance:

| Form            | Meaning                              |
|-----------------|--------------------------------------|
| `name: type`    | required                             |
| `name: type = v`| defaulted (v from closed default set)|
| `name: type?`   | nullable, sugar for `= null`         |

Closed default expression set: literals plus `today`, `now`, `newid`. No arbitrary
computation in defaults. On insert, resolved values (not "default") are recorded in
the command, so history never depends on knowing what a default used to be.

**Built today (2026-09-29):** literals only. `now`, `today` and `newid` are in
the set but the write path does not resolve them yet, so `ordo check` refuses
them with `unsupported_default` (it used to accept them and fail when the
schema was applied). Give the field its value on the statement that creates
the row.

### Row invariants: `check` [settled]
A shape may declare row-level invariants, compiled to database CHECK
constraints, so the *system* enforces them inside every write's transaction.
A violation is a typed rejection aborting the whole atomic group:

```
entity teams {
  seats: int
  used:  int = 0
  check used <= seats
}
```

This is the quota/seat-limit answer with no op-level precondition machinery:
the op `set teams { id: $t, used: used + 1 }` + `set members {…}` is one
atomic group: the check fires, the group aborts, and the seat never oversells.
Cross-row rules beyond one row remain rule-tier (maintained counters feeding
row checks). `require`-style op preconditions are thereby unnecessary.

**`check` may walk refs [settled, not built].** `ordo check` refuses this today (`check_beyond_row`); `docs/syntax-examples.md` marks it `# planned:`. An invariant is not always about one
row's own columns: *"the assignee must belong to the customer's
organisation"*, *"a line's currency must match its invoice's"*. These are
facts about a row **and the rows it points at**, and until now the language
had nowhere to put them. Guards answer visibility, and write conditions are
row-scoped. So `check` accepts dot-paths through the row's own refs:

```
entity tickets {
  customer -> customers
  assignee -> members
  check assignee.org == customer.org
}
```

Bounded deliberately: the walk follows **outgoing refs from the row being
written** (many→one, the same direction dot-paths always go), any depth, and
compiles to a constraint the database enforces inside the write's transaction.
A violation is a typed rejection that aborts the whole command group, like
any other `check`.

**Both sides are re-checked [settled].** `check assignee.org == customer.org`
can be falsified two ways: by writing the ticket, and by moving the assignee to
another org (a write to a *different entity* that never mentions tickets). An
invariant that only guards one of its own edges is not an invariant, so a
ref-walking `check` compiles to enforcement on **the declaring write and on
writes to every entity the walk passes through**. The parent-side check is
scoped to the rows that actually reference the changed row, and a violation
there rejects the *parent's* write with the same typed error naming the
invariant and the child rows that would have been broken. The cost is stated
openly: a walking `check` makes writes to referenced entities more
expensive in proportion to how many rows point at them, which is why the walk
is outward-only and why aggregate conditions stay rule-tier. What it cannot do is aggregate, count, or test existence
over a collection; those remain rule-tier, because they are questions about
other rows rather than about this one's neighbours.

> **Not built yet [2026-10-04]: a walking `check` is refused today**
> (`check_beyond_row`). This is settled and owed by the compiler. The
> row-local form is built. The ruling of 2026-09-05 (`corpus-rulings.md`)
> scoped the walking form to triggers: Postgres CHECK admits no subqueries, so
> both halves above are trigger work. One trigger guards the declaring write.
> Others guard writes to every entity the walk passes through, and name the
> child rows a parent-side change would break.

### Implicit identity [settled]
**Identity is uniform, so it needs no syntax.** Every entity carries two implicit
system fields, `id` (typed, server-minted) and `rev` (version). They are undeclarable,
always present, and have the same semantics everywhere. There is no primary-key concept in the
language: no `pk` marker, no composite keys, no key-choice decisions. Natural
keys are expressed as `unique` constraints, which are uniqueness claims *about* data,
distinct from identity *of* rows. The whole PK design space (natural vs
surrogate, composite identity, key stability) is deleted from user syntax; one
more instance of "the server owns mechanics."

### Refs [settled]
`field -> entity` declares the relationship once. Derived automatically: FK,
index, dot-path traversal, match inference. Refs point via typed ids.

- **Nullable-ref traversal keeps the row**: dotting through a nullable ref yields
  null fields, never a vanishing inner join.
- **Deletes restrict, except through `owned`**: deleting a row that others
  reference through a plain (association) ref errors, listing referrers.
  Rows that reference it through an `owned` ref are part of it and go with
  it (see Ownership). A tree's own children are the exception with their
  own word, `subtree`.
- **Uniqueness [settled]**: `unique` on a field, `unique (a, b)` composite.
  Violation = typed rejection ("duplicate"), retry-safe for sources. On sealed
  fields uniqueness rides the blind index.
- **Uniqueness is always scoped to the space [settled]**: every generated unique
  index carries the space column as its **leading column**, without exception and
  without syntax: `unique (a, b)` compiles to
  `UNIQUE NULLS NOT DISTINCT (space, a, b)`, and a `sealed indexed unique` field's
  blind index is `UNIQUE (space, hmac)` (the HMAC key is already per-space; the
  column makes the claim structural rather than key-dependent). There is no
  deployment-wide uniqueness and no way to ask for one: a domain cannot claim a
  global name across spaces it does not own. Spaces co-reside in one database by
  default, so this is what keeps two tenants' `externalId`s from colliding, and
  it is **newly load-bearing under `NULLS NOT DISTINCT`** (next bullet). A
  nullable unique field admits at most one null row, so without the space column
  the *first* space to store a null would consume that single slot for every
  other space in the database. **Arbitrary SQL-style conflict
  handling remains unexpressible; unique-key converge-or-create (§6) is the
  sole defined upsert semantics**. Insert-completeness rules out partial-update-
  on-conflict, and the static mode label makes it reviewable.
- **Uniqueness uses language equality [settled]**: the language has **one**
  equality (§4), and `unique` is a claim written in it. Since `null == null` is
  true, a nullable unique field admits **at most one null row**. Concretely:
  - Generated DDL is `UNIQUE NULLS NOT DISTINCT` (composite: any tuple of bound
    values, nulls included, collides with an equal tuple).
  - A `sealed indexed unique` field HMACs a **reserved null sentinel** for null
    values, so the blind index carries the same rule with no special case and
    `HMAC(null)` is never undefined.
  - **A converge-or-create whose addressing unique key has a null bound value is
    a typed rejection, class `null_key_address`**, never a create. Addressing a
    row by a key that is deliberately indistinct from every other unaddressed row
    is not an identity claim, and Postgres's default `NULLS DISTINCT` would
    otherwise silently create duplicates under the one blessed upsert.

  This is the deliberate departure from every SQL system, and it is the *only*
  answer that keeps the language to one equality. The alternative buys familiar
  uniqueness by reintroducing exactly the two-equalities problem §4 deletes.
  Every SQL-native author will expect the other behavior, so it has a `traps.md`
  entry.
- **Every row carries an implicit `rev`** (version, queryable) for
  compare-and-set (see Writes).

### Sealed fields [settled]
`sealed` = lives in the per-space encrypted payload; Postgres holds ciphertext.
Consequences the compiler knows and surfaces:
- not filterable/sortable in SQL: predicates on sealed fields run post-decrypt in
  the host (compiler annotates the op)
- `sealed indexed` = blind index (HMAC of value in an indexed column): **equality**
  lookups at SQL speed with keys never entering Postgres. Leaks only equality
  patterns, salted per-space. Ranges and `contains` on sealed fields remain
  post-decrypt; no order-revealing schemes, ever.
- Skeleton (ids, refs, dates, space links) is plaintext for indexing; everything
  semantic defaults to sealed in finance-grade domains.

**Sealed fields and derived std ops: `std sealed` [settled].** Derived
platform ops (the std family: `get`/`list`/`update`/`whole`/…, specified in
`std-library.md`) are *derived*, and disclosure in this design is structural
everywhere else (guards inline, manifests derive, scope expansion kills grants).
So the language side of the rule is a shape-level marker:

```
entity patients {
  name:      text  sealed
  diagnosis: text  sealed
  admitted:  date
  std sealed                    # opt in: derived std ops see sealed fields too
}
```

- **Default (no marker): an entity containing any sealed field derives its std
  ops over the OPEN SKELETON ONLY.** Sealed fields are absent from those ops'
  manifests, so granting the std family for such an entity cannot disclose a
  sealed value, structurally and not by convention.
- **`std sealed`** is the one greppable word that opts the whole entity's
  derived std ops into the sealed-inclusive manifest. It is a shape line, so it
  shows in source, in the deploy diff, in the shape summary, and in every
  consent page computed from the resulting manifests. Sealed disclosure becomes
  an **authored act**.
- The marker changes only *derived* ops. Hand-written ops project whatever they
  project and have always disclosed it through their own manifests; nothing
  about them changes.
- Cost: generated UI and the data browser render sealed markers for such
  entities until someone opts in. That is the correct default: a blank where a
  value would be is a visible prompt to make a decision, and a value that
  should not have been there is not.

(This is the language-side rule. The derivation itself, the op inventory, and
the profile-template consequences live in `std-library.md`.)

### Expression functions [settled]
Closed list, legal anywhere fields are. No user-defined functions ever
(extensibility lives in routines, not expressions). All pure and deterministic:
`now`/`today` exist only in shape defaults. Relative time in queries is
exclusively `since` or a parameter, so nondeterminism has one keyword.

- Date parts (bucketing): `year(d) month(d) week(d) day(d) hour(t)`, which are sortable
  scalars (`month(date)` → `2026-07`), plus `dow(d)` → int, ISO numbering
  (1 = Monday … 7 = Sunday)
- Numeric: `+ - * /`, `abs(x)`, `round(x)`, `round(x, n)`, `floor(x)`,
  `ceil(x)` (decimal → int), `mod(a, b)` (integer; truncated division, sign
  follows the dividend; `b == 0` = typed runtime error), `least(a, b)`,
  `greatest(a, b)` (binary, so nest for more; null operands are ignored, both
  null → null), `hash(x)` (stable 64-bit non-cryptographic hash → int, for
  bucketing/rollout striping; **stability across platform versions is a
  compatibility promise**)
- **Allocation: `allocate(total, weights [, mode])` [settled]** is the blessed
  money-split
  primitive, and the answer to "divide this amount among these parties." It
  splits `total` (a `money` or `int`) in proportion to `weights` and
  **guarantees the parts sum to exactly `total`**: each part is the floor of its
  exact share, and the remaining minor units are handed out by
  **largest-remainder**, ties broken by weight order (deterministic, so both
  venues agree). `allocate(100.00, (1, 1, 1))` is 33.34 / 33.33 / 33.33, never
  99.99. Division cannot make this promise and is not asked to.
  - **Which party absorbs the odd minor unit is the caller's choice, not this
    document's.** It is a business rule, not a mathematical one, and a
    contract, an accounting policy and a settlement agreement can each name a
    different answer. The optional third argument is a **mode**, one bare name
    from a closed set; omitted, it is `largestRemainder`.

    | mode | the leftover units go to | why you would name it |
    | --- | --- | --- |
    | `largestRemainder` | the largest fractional remainders, ties by weight order | **the default.** What accounting texts and regulators mean by "allocate"; it minimises the largest deviation from an exact share |
    | `first` | the earliest parties, in order | a contract that names a residual party first ("the lead bears rounding") |
    | `last` | the latest parties, in reverse order | the ledger habit: the difference is plugged into the final line |
    | `largestShare` | the parties with the largest shares (by \|weight\|), ties by weight order | puts the rounding where it is proportionally smallest |

    A mode is **not a second algorithm**. Every mode floors every exact share
    and then hands the leftover minor units out **one apiece**; a mode chooses
    only the *order of the queue*. Two properties follow from that and hold for
    every mode: the parts sum to the total exactly, and **no part is more than
    one minor unit from its exact share**. A "mode" that broke either would not
    be a mode.

    **A mode may depend only on `(total, weights)`.** That is the whole
    admission test for a new one. It is why **rotation** (spreading the odd
    penny across successive invoices so it evens out over time) is *not* a
    mode and will not become one. It has to remember what it did last time, and
    that memory is application state. An expression function that answered
    differently on a retry would break the determinism this language and the
    write protocol both rest on. A rotation the application owns, using a stored
    counter it passes in as the weights' order, is the blessed shape, and it is
    visible in the application's own data where an auditor can see it.

    Mode names are lowerCamel because §1 *Identifiers* is one rule for every
    name in this language. An unrecognised mode is a **compile error**, never a
    silent fall back to the default: the third argument position is a mode and
    nothing else, so a misspelling cannot be read as a field.
  - **Where it is legal:** inside an `each` emission over a **list parameter**.
    `weights` is written as the element field (`item.weight`); the function reads
    that field across **all** elements of the list and returns **this element's
    part**. The whole weight vector is a parameter, fully known at the door
    before planning, so this is arithmetic over bounded input, not inter-row
    computation over query results, which stays declined (§11). Anywhere else it
    is `invalid_allocate`.

  ```
  op splitBill($bill: id, $shares: list of { member: id, weight: int },
               $total: money) =
    set billShares each $shares {
      bill:   $bill,
      member: item.member,
      amount: allocate($total, item.weight)   # this element's part; parts sum to $total
    }

  # …and with the residual party named, because the contract named one:
  op splitFee($fee: id, $shares: list of { party: id, weight: int },
              $total: money) =
    set feeShares each $shares {
      fee:    $fee,
      party:  item.party,
      amount: allocate($total, item.weight, last)
    }
  ```

  (`each` over a list parameter is **[lean]**, not settled; see §2, Types. The
  allocation semantics above are settled; the spelling lands with that
  construct, and if `each` changes shape `allocate` moves with it.)

  > **Not built yet: the compiler refuses this today.** `list of {...}` and
  > `set … each` do not parse. Every `allocate` the compiler sees is outside
  > an `each`, so every one is `invalid_allocate`, in a read and in a write's
  > braces alike.

  Stated limitation: splitting by **query-derived** weights (allocate across the
  rows a pipeline produced) is not expressible, for the same reason running
  totals are not: it is neighbor-math over result rows. Those splits go through
  §11's blessed homes (client compute, or rule-materialized state), or the caller
  passes the weights it already fetched.
- Null: `coalesce(a, b)` is the one blessed escape from nullable fields
- **Conditional: `cond(c1 -> v1, c2 -> v2, …, velse)` [settled]** is the only
  conditional construct (no `if`, no CASE/WHEN/ELSE vocabulary). Arms evaluate
  in order, first match wins (each arm implicitly means "…and nothing above
  matched"). The final bare value is the mandatory else, so totality is
  compile-checked and there are no surprise nulls (want null? write it). All arm values one
  type. Arm conditions use the full condition grammar (booleans with
  parens-when-mixing, dot-paths, field-to-field comparisons, durations,
  params) but never existence tests, because conditions see the row, not other rows.
  `->` = "yields", the same arrow as triggers and `each ->`. Legal anywhere
  expressions are: projections, write RHS (per-row values in bulk writes and
  rules), aggregate arguments, sort keys.
- Text: `lower(t)`, `length(t)`, `trim(t)` (strips leading/trailing Unicode
  whitespace), and `+` for concatenation
  (`"lease ends " + end`; non-text operands render canonically); matching
  stays in operators

### Ownership [settled]

`->` is **mechanics**: "this field holds a reference to a row of
that entity." Its direction is forced by cardinality (a single-valued field can
only live on the many side), so refs always sit on the child pointing at the
one. Parenthood is a separate, opt-in claim: mark the one ref that is
composition with `owned`.

```
entity receiptLines {
  receipt -> receipts  owned      # PART OF its receipt, dies with it
  amount:  money
}
```

- **The existential test**: owned iff the row is *meaningless without exactly
  this parent*. If a second parent is even imaginable, it's association.
- **At most one ordinary `owned` ref per entity** (compile rule). Ownership is
  never inferred, only stated. Two structural exceptions, both
  compiler-checkable:
  1. **Link entities**: exactly two `owned` refs to *different* entities
     (extra scalar fields allowed; no further owned refs; may not itself be an
     ownership target). The link dies with either end. Many-to-many only
     exists this way: refs are many→one by construction, so ambient M2M is
     unrepresentable.
  2. **Tree + root owner**: one ordinary owner plus one nullable self-ref
     marked `tree owned`. Per row, exactly one is effective: root rows
     (null parent) are owned by the ordinary owner; nested rows by their
     parent. (The building/floor/room shape.)
- **No parent-side declaration.** The parent's collection view is *derived* and
  rendered by the compiler's shape summary
  (`receipts: owns receiptLines (via receiptLines.receipt); referenced by: …`),
  so both directions are readable while exactly one is stored. No redundancy,
  nothing to drift.
- **Defaults are conservative**: with no `owned` anywhere, everything is
  association: `whole` embeds shallow, deletes restrict. Adding `owned` later
  is additive.
- Deletes: owned cascades (part-of), association restricts
  (refuse-with-referrers), both derived from the same mark. **[settled,
  owner ruling 2026-09-29, `owned-delete`]** A plain `del` of a row deletes
  every row that references it through an `owned` ref, transitively, and that
  includes the access rows derived for an entity with an `acl` (their refs
  are `owned`). The cascade runs in the same command group, one command per
  row with its own before image, owned rows before their owner, so a revert
  restores all of them. Three consequences:
  - **All or nothing.** The caller must be allowed to delete every row the
    cascade takes, as `subtree` requires. One owned row they may not delete
    refuses the whole statement (`access_guard`, stating how many rows and
    not which). An entity with an `acl` therefore deletes only for a caller
    holding its top level, because the derived access rows need it.
  - **A plain ref into the cascade still refuses.** If a row the cascade
    takes is referenced through a plain ref from a row that is not itself
    going, the whole delete answers `has_referrers`. Cascading does not
    override an association.
  - **`subtree` stays the form for trees.** A plain `del` of a tree node
    with children still refuses (`tree_has_children`), `tree owned` or not.

### `whole` [settled]

`whole entity($id)` returns the member and its composition, **anchored and
directional**:

- **Downward, full**: the anchor plus its owned closure (ownership is a tree:
  one owner max, so no cycles; depth bounded by the modeling itself). Through
  double-owned links: expand into the link rows, then **one shallow hop** to the
  far side (the linked transaction's scalars, never further).
- **Upward, signpost only**: the anchor's own refs (including its owner) embed
  as one-hop shallow summaries, enough to know where you are and navigate.
  Ancestors' bodies are never included; ancestry is a query (`above`).
- **Incoming associations never expand** (an account's `whole` does not contain
  its ten thousand transactions; that's a query, not the account's body).

Asked mid-hierarchy, `whole` is therefore *the subtree from here plus a signpost
up*. Result shape is static, so it registers like any op, with a field-scope manifest,
sealed costing, cost class, and access check.

**The rollup closure is `whole`'s closure MINUS the shallow far-side hop
[settled].** Aggregating derivations over a composition (std's `rollup`, and
anything else that sums a `whole`) use a **strictly smaller** closure than
`whole` does: through a double-owned link, **link rows contribute their own
scalars only**, and the one shallow hop to the far side is excluded. This is not an
optimization. It is what makes the no-double-count property true. `whole`'s
closure is a *display* closure and may show the far side twice (two links from
one anchor to the same transaction, or a far row that also belongs to another
root's closure) without lying about anything, because it is showing, not adding. An
aggregate over that same closure would **double-count within one anchor and
across anchors**, which is precisely the star-aggregation sin the fan-out guard
exists to prevent.

**The no-double-count property is per-anchor, and only per-anchor [settled].**
What holds: with the far-side hop excluded, a rollup anchored at one root visits
each row of that root's owned closure **exactly once**. The closure reached from
a single anchor is a tree: every entity has at most one ordinary `owned` ref, a
`tree owned` row has exactly one effective owner, and a link row is reached from
a given anchor through exactly one of its two owned refs. So there is no second
route by which a row can enter the same rollup. That is the whole claim, and it
is what the excluded far-side hop buys.

The stronger claim previously stated here, "every row is owned by at most one
root", is **false and is withdrawn.** A link entity has two `owned` refs by
construction, so a link row sits in the owned closure of *both* its ends; that is
the definition of a link, not an edge case. The consequence is stated openly: **summing rollups across anchors can legitimately count a link row
once per end**, so a cross-anchor total over a link-bearing closure must be
computed from the link entity directly (one pipeline, one grain) and never by
adding per-anchor rollups. Rollups compare and combine within an anchor.
(`std-library.md` states the same property for the `rollup` derivation it owns;
this document is the normative source, and the false one-owner proof must not
survive there either.)

### Trees [settled]
Free-form recursion is unexpressible. A self-ref marked `tree` makes the platform
do the dangerous parts:

```
entity projects {
  name:   text
  parent -> projects?  tree
}
```

Platform maintains the materialized path inside the command layer, rejects cycles
at write time, enforces a depth cap. Queries get exactly two closed operators:
`under` and `above` (see §5).

Tree entities carry an implicit read-only **`depth` field** (int, root = 0),
maintained with the materialized path, and usable anywhere fields are
(`categories(depth <= 2)`, `sort depth`). Path-string rendering stays a
client/`whole` concern.

**`tree` and `owned` are orthogonal and compose:**
- `parent -> projects? tree owned` gives hierarchy *and* composition: sub-projects
  are part of their parent; `whole` expands the subtree; `del … subtree` is a
  true composition cascade.
- `parent -> folders? tree` (no `owned`) gives hierarchy of *independent* things;
  `whole` does not drag descendants; `under` still queries them; deletes
  refuse-with-children.

**Deleting tree parents [settled]:** refuse by default. Deleting a node with
children errors, stating the count. Subtree deletion must be explicit:
`del projects { id: $id } subtree` (greppable, scope-gated; compiles to
per-row commands, so revertible). Reparenting is not a delete mode. It's
`set projects(parent == $id) { parent: $new }` first.

### `members`, the principal entity [settled]

Guards, `acl`, and every `$caller` comparison hinge on one entity, so it is
declared here rather than assumed.

**`members` is platform-provided, per space, and undeclarable**, with the same
status as the implicit `id` and `rev` fields. A domain may not write
`entity members { … }` (the name is reserved in the entity namespace); it
**refs** the entity like any other (`assignee -> members?`,
`owner -> members`). One row per principal admitted to the space.

Fields (all read-only to domain code, queryable like any field):

| Field | Meaning |
|---|---|
| `id` | `mem_…`, the space-local identity of a principal. **This is what `$caller` binds to.** |
| `principal` | the deployment-wide principal id (`usr_…`, `agt_…`, `svc_…`), `unique` within the space |
| `displayName` | `text`, never null: the human-readable name to render for this member, or the group's name for `kind: group` |
| `kind` | `principal` or `group` (see below) |
| `memberCount` | int, read-only: `1` for a principal, the group's current principal count for a group (see Groups) |
| `role` | the membership role (owner / manager / editor / viewer), or null for guests |
| `status` | `active` / `suspended` / `left` / `deleted`. Suspension is a command, not a delete. `left`: the person left the tenant. `deleted`: a left seat whose person then deleted their account. Neither is a membership |
| `joined` | datetime the row was minted |

**`displayName` and where it comes from [settled]:** every leaderboard, byline,
assignee chip, and progress table needs a name, and before this field there was
nowhere in-language to get one. Domains were inventing shadow name fields or
teaching examples that render a `mem_…` id. It is platform-maintained, like every
other members field, and is set **when the row is minted**, from exactly one
source per minting path: space creation and invitation acceptance take the
identity service's profile name (the OIDC `name` claim, falling back to
`preferred_username`, falling back to a platform-generated handle; never null,
never empty); auto-onboard takes the same; **guest binding takes the name the
guest consented to disclose at accept time**, which is the only cross-space
disclosure and is therefore a consent decision, not a lookup; a `kind: group` row
takes the name given when the group was created. Changes are ordinary attributed
platform commands (a principal renaming themselves updates their members row in
every space they belong to, each as its own logged command). A seat keeps its
`displayName` when its person leaves, unless the app developer sets
`ORDO_MEMBER_LEFT_NAME`. How a left or deleted seat is shown is the app's choice,
read from `status`.

It is **plaintext skeleton, not sealed**. Bylines, `sort displayName`, and
`joined(displayName, ", ")` all have to work in SQL, and a name a space's members
already see is not a confidentiality boundary. It is **read-only to domain
code**: a domain refs `-> members` and reads `assignee.displayName`; it cannot
write it, and it must not copy it into its own entities (a copy drifts the moment
someone renames).

**`$caller` ↔ member reconciliation [settled]:** `$caller` binds to the caller's
**`members.id` in the space the request is bound to**, not the deployment-wide
principal id. That is why `member == $caller` type-checks against a
`-> members` ref with no coercion anywhere: a principal's cross-space identity
is `principal`, its per-space face is the members row, and guards are always
per-space. A principal with no members row in the space binds nothing, and every
guard evaluates false (see anonymous callers below).

**Who creates the row [settled]:** the platform does, as an ordinary attributed
command in the space's chain, at exactly four moments: (i) space creation, for
the creating principal, with role `owner`; (ii) invitation acceptance; (iii)
auto-onboard, where the deployment enables it (role from the space's policy,
never `owner`); (iv) **guest binding**, when an inter-space share is accepted.
A guest gets a members row with a null `role` so acl rows and guards can name
them, and their guest-ness (no role, no profile, external to the space) is
platform state, not a domain concern. There is no domain-authored path to
minting one, and no path at all for `routine:`, `worker:`, `source:`, or
`system:` principals (see "Routines are not callers" in §8).

### `nopause`: answering while the tenant is paused [settled 2026-10-05, built 27ar]

A tenant can be paused. A paused tenant refuses its ops with `space_paused`.
The app developer marks the ops that keep answering with one word on the op:

```
op openInvoices nopause =
  invoices(status == "open")[number, total]
  take 50

op markPaid($number: text) nopause =
  set invoices(number == $number) { status: "paid" }
```

- The word sits between the name and `=`, beside `unguarded` and `unbounded`,
  in any order. It is reserved, so nothing can be named `nopause`.
- It may mark a read or a write. A write marked `nopause` changes a paused
  tenant's rows, so mark only what a paused tenant truly needs.
- The manifest carries `flags.nopause: true`, and `ordo check` prints
  `(nopause: answers while the tenant is paused)` beside the op.
- Derived `std.*` ops carry no flag and are refused while paused.
- Platform ops follow their own list: export, delete, resume, the member
  commands and the chain proof answer; sharing, invitations, buckets,
  attestation changes and leaving are refused.

Pausing itself is a platform op, not syntax. See `howto-pause.md`.

### `download`: in a person's own download [ruled 2026-10-22, built, held for review]

A person's own download (`GET /download`) is off until the app developer
marks read ops with `download`:

```
op allNotes unbounded download =
  notes[id, title, body]
```

- Each marked op is one entity in the zip. Its projection is the fields, its
  conditions are the filter, and a blob field brings the files.
- The door runs the op as the person, so the row guard decides the rows.
- The word sits between the name and `=`, in any order. It is contextual, not
  reserved: a field or an op can still be named `download`.
- The manifest carries `flags.download: true`, and `ordo check` prints
  `(download: in a person's own download)` beside the op.
- Compile classes: `download_write`, `download_params`, `download_public`,
  `download_source`, `download_entity_twice`. See `howto-leave.md`.

### `public`: readable by anyone [settled 2026-10-03, built]

A field, an entity or an op marked `public` can be read with no token. A
visitor's call runs as `$anonymous` in the deployment's own space. Phase 1
builds public reads. Phase 2 builds public writes: a public op may create
rows and nothing else.

```
entity plans {
  name:   text     public
  price:  money(2) public
  margin: money(2)                 # private: no public op may read it
}

entity pages {
  slug: text unique
  body: text
  public*                          # this entity and the entities it owns
}

entity pageImages {
  doc   -> pages owned
  image: blob?                     # public, because its row is public
}

op pricing public =
  plans(price > 0)[name, price]
  sort name
  take 50
```

- On a field, `public` sits at the end of the line, after `sealed` and `indexed`.
- On an entity line of its own, `public` marks every field of that entity.
- `public*` marks the entity and every entity it owns, through `owned` refs
  only. A plain ref does not carry it.
- A child that writes its own `public` or `public*` line replaces the inherited one.
- On an op, `public` is a flag beside `unguarded` and `unbounded`.
- `public` is reserved, so nothing can be named `public`.
- A file follows its row. A `blob` field on a public row is served with no token.
- A public op reads public fields only. That covers the projection, every
  condition, every sort key and every composed op.
- A public op has no guard walk and writes no audit row. The compiler's check
  is what makes that safe.
- `ordo check` prints `(public: anyone may call it, with no token)` beside the op.
- The CDK spells it `t.text().public()`, entity `.public()` and `.publicAll()`,
  and op `.public()`.

Compile classes:

| Class | Refused |
|---|---|
| `public_sealed` | `public` on a `sealed` field, directly or through an entity line |
| `public_private` | `public` on a `blob private` field |
| `public_guarded` | `public` on an entity that has `access read` or `acl` |
| `public_reads_private` | a public op reading any field that is not public, in a projection, a condition or a sort. Also an entity with no public field |
| `public_composes_private` | a public op that composes an op that is not public |
| `public_caller` | `$caller` in a public op |
| `public_write_not_create` | a statement in a public write that updates, converges or deletes, or that creates on a unique field (it would tell a visitor whether a value is taken) |
| `public_write_guarded` | a public write into an entity that has a guard or an `acl` |
| `public_write_ref` | a public write that sets a ref from a visitor's value to an entity with no public field (the answer would show whether that row exists) |

A condition on a private field is a read. `plans(margin > 100)[name]` would
tell a visitor one bit of the margin per call.

Row-by-row visibility (a draft, then published) is a condition in the op on a
field that is itself `public`. A `public` row condition on the entity line is
ruled and not built.

Not built: the `deployment` entity line, so `public_tenant_entity` is not
enforced. The public path reads the deployment's space. See `howto-public.md`
and `design-public-path.md`.

### `release member`: leaving a tenant [settled 2026-10-05, built 27aq]

Whether a member may leave a tenant on their own is the app developer's rule.
Ordo provides the mechanism (`std.space.leave`) and builds no override past it.

```
release member open                  # any member may leave
release member locked                # nobody leaves on their own
release member when not invoices(owner == $member and status == "open") {
  remove drafts.author
  reassign invoices.owner to people(member == $member).manager or owner
}
```

- **No line means locked.** A program with no `release` line lets nobody leave
  on their own. `ordo check` prints this under `RELEASE`, so a developer
  wondering why members cannot leave finds it there.
- **One line per program.** A second is `duplicate_release`.
- **`$member` is the seat being released.** It binds like `$caller` and cannot
  be supplied. `$caller` inside a release rule is `release_subject`. Outside a
  release rule, `$member` is an ordinary op parameter name.
- **Conditions are guard walks.** Each is `[not] entity(walk)`, joined with
  `and` and `or`, parenthesized when mixed. `not` applies to one condition. A
  rule longer than a line is written inside parentheses. A sealed field
  anywhere is `release_on_sealed`; an undeclared entity is
  `release_unknown_entity`.
- **What happens to rows that point at the leaver.** By default they stay and
  point at a seat that keeps its name and reads `status = left`. The block may say otherwise
  for a ref to `members`:
  - `remove e.f` deletes the rows whose `f` is the seat.
  - `reassign e.f to <target> [or <target>]` points `f` at the first target
    that names an active member other than the leaver. A target is `owner`
    (the tenant's longest-standing other owner) or a lookup
    `entity(walk).field`. With no target found, the leave is refused.
  - An effect on anything but a plaintext ref to `members` is
    `invalid_release_effect`; two on one ref are `duplicate_release_effect`.
- **The platform's own conditions always apply**, before the rule: the last
  active owner cannot leave (`last_owner`).
- Changing the rule is a class A change: no migration, it applies on
  activation.

The mode is also to become a layered setting (`member.release`, ruled
2026-10-05); that half waits on the settings machinery (checklist 24a). How a
leave and an account deletion run is `howto-leave.md`.

### Groups [settled]

A space may define named **principal collections**. A group is *a members row
with `kind: group`*, so it is nameable in exactly the places a principal is
nameable, with no new type and no grammar change:

- memberships, access profiles, and acl rows all name a members row, and a
  group *is* one. `grantBoardsAccess($board: $g, …)` is the same op it always
  was. Guard walks name members rows too, but what a walk *matches* is ruled
  below: `== $caller` is the caller's own row, never a group they belong to.
- **`groupMembers` is the platform's group-membership link entity [settled]**:
  `groupMembers { group -> members owned, member -> members owned }`, per space,
  platform-maintained and undeclarable (same status as `members` itself), but
  **queryable and walkable like any entity**, which is what makes group-aware
  guards and headcounts writable. Both refs point at
  `members`, so a reverse step into it must name the ref it arrives by (see G1's
  `entity.ref(…)` reverse-step spelling).
- **`== $caller` never expands groups [settled].** `x == $caller` means exactly
  one thing everywhere it can be written (guard walk, op body, `cond` arm,
  `check`, routine): `x` is the caller's **own** members row. It does not
  match a group the caller belongs to. This **replaces** the earlier rule (guard
  equality resolved through groups, op-body equality did not), which gave one
  spelling two truth values depending on where it sat: an author reading
  `member == $caller` in a guard and in `op myExpenses` was reading two different
  predicates, which is exactly what principle 1 exists to forbid, and the
  divergence was silently permissive in the guard direction.
- **Group resolution happens in the access layer, and only there [settled]**: an
  acl row or an access profile naming a group row applies to that group's
  members. A **guard** that wants group semantics says so with an explicit
  membership walk. That is one extra disjunct, still an indexed semi-join, still `sql`
  cost class, and visible in the source instead of implied by a rule:

  ```
  access read  project.projectMembers(member == $caller)
               or project.projectMembers(member.groupMembers.group(member == $caller))
  ```

  The acl-derived guards carry exactly this second disjunct, and the shape
  summary renders it in full. The derivation is the access layer, so it expands
  groups, and it discloses that it does by showing the walk it generated.
- **Built**: group commands are now platform ops: `std.space.createGroup`,
  `addToGroup`, `removeFromGroup`, `deleteGroup` and `groups`, each a command
  group in the tenant's chain. A group grants ops through the tenant's profile
  and confers no role. See `howto-roles.md`.
- `callersOf` resolves through groups, so "who can call X" stays one exhaustive
  query and answers with humans, not group names.
- **Counting people, not rows: `memberCount` [settled].** Every members row
  carries an implicit read-only `memberCount: int`, which is `1` for `kind: principal`,
  and for `kind: group` the number of principal rows currently linked by
  `groupMembers`. It is platform-maintained alongside the membership commands
  (same machinery as a tree's `depth`), so a quorum or seat rule can count
  humans, not acl rows that each happen to be one group:

  ```
  projectsAccess(project == $p)[member]
  reduce { seats: sum member.memberCount }
  ```

  **Stated limitation:** that is a **cheap upper bound**, not a headcount. A
  principal holding access both directly and through a group (or through two
  groups) is counted once per path. Where the exact distinct headcount matters
  (billing seats, quorum), it is a two-source pipeline over `groupMembers` with a
  `distinct` step, which is expressible because `groupMembers` is an ordinary
  queryable entity and nesting is forbidden (depth 1, so one expansion suffices):

  ```
  let direct = projectsAccess(project == $p)[member] | member.kind == "principal"
  let viaGroup = projectsAccess(project == $p)[] groupMembers[member]
                 match groupMembers.group          # both refs target members

  direct[member]
  append viaGroup[member]
  distinct member
  reduce { people: count }
  ```
- **Group membership changes are ordinary audited commands.** They are the same runtime
  state as roles and profiles, in the chain, never in IaC. Creating a group,
  adding a member, removing one: three commands, three log entries.
- **Groups may not contain groups in v1.** Nesting is rejected at write time
  (`group_nesting`). Depth 1 keeps the guard expansion a single join and keeps
  `callersOf` a flat enumeration. Nesting is a v2 candidate, and the cost of
  adding it later is a query change, not a model change.

This is the answer to org hierarchy: a department, a contractor cohort, a
project team is a group; grants name the group; the org chart lives in group
membership, which is data.

### Access guards [settled]

Row-level, data-driven authorization, declared in the shape and evaluated
against live data at every call. The rule is code; the facts are rows.
**This subsection is normative**; `guards-conformance.md` asserts these rules
and originates none of them.

```
entity tasks {
  project -> projects owned
  title: text
  access read   project.projectMembers(member == $caller)
  access write  project.projectMembers(member == $caller and canWrite)
}
```

**G1: Grammar (closed).** `access read` / `access write` take a
**`$caller`-anchored relationship walk** (the one non-walk form is the bare
keyword `open`, ruled in G9). It is an existence test over declared refs,
compiled to an indexed semi-join (`sql` cost class). The predicate grammar is
closed to: forward ref hops (`project.`), reverse collection steps
(`projectMembers(…)` where that entity declares a ref to the current position),
`==` tests against `$caller` or literals, bare `bool` fields, combined with
`and`/`or` under the parens-when-mixing rule. **No other operators, no
functions, no arithmetic, no `!=`, no ordering, no `in`.** Violations are
`invalid_guard_walk`. `or`-combinations are legal
(`project.visibility == "public" or project.projectMembers(…)`).

**Reverse steps name their ref when there is a choice [settled].** Where the
stepped-into entity declares **two or more** refs to the current position (the
platform's `groupMembers { group -> members, member -> members }` is the case
that forces this), the bare step is ambiguous and the walk must name the arriving
ref in the same `entity.ref` spelling §5 uses for duplicate refs:
`member.groupMembers.group(member == $caller)` means "rows of `groupMembers`
whose **`group`** ref points at the members row we are standing on, filtered by
`member == $caller`". One ref, no ambiguity, the bare form stays legal and is
`ambiguous_reverse_step` when it is not.

**G2: At least one disjunct must reference `$caller`.** A guard that never
consults the caller is a hidden filter wearing an authorization costume; hidden
filters belong in a named query, which is visible in the op's body. Class
`invalid_guard_walk`.

**G3: Guards may not touch sealed fields.** A guard predicate referencing a
sealed field, directly or anywhere along the walk, is a compile error
(`guard_on_sealed`). Guards walk the plaintext skeleton only. A guard that
cannot push down is a guard that silently degrades every read's cost into the
host, and authorization is the last place that should be a surprise.

**G4: `$caller` is legal in write RHS** (`assignee: $caller`,
`createdBy: $caller`). It binds the session principal like any id-typed
parameter. It remains **illegal in shape defaults** (`caller_in_default`):
defaults are evaluated without a session.

**G5: Guards attach to the entity, wherever it is read.** Every entity the
flattened plan reads contributes its read guard as a **conjunctive semi-join**,
dot-path targets included. A row whose dotted-through target fails its guard is
**removed**, never null-blanked (blanking would misreport data absence as data).
All contributing guards appear in the manifest, per entity.

**G6: Inheritance down owned subtrees.** A child entity with no `access` clause
takes its nearest guard-bearing ancestor's clauses, **rebased through the
ownership path**. For example, `taskNotes` (owned by `tasks`, owned by `projects`) reads
under `task.project.projectMembers(member == $caller)`. This is identical for
hand-rolled `access` clauses and `acl`-derived guards; there is one inheritance
rule, not two.

**G7: Link entities never inherit.** An entity with two `owned` refs has two
candidate inheritance sources, so it has none: a link entity either declares its
own `access` clauses or is unguarded. (The `acl`-derived link is not an
exception, because it gets *derived* guards, G16.)

**G8: A child `access` declaration replaces the inherited guard entirely,
and it is a complete clause.** No composition algebra, ever. The old ambiguity
("does declaring only `access read` also drop the inherited *write* guard?")
is dissolved, not answered: **an `access` declaration is both verbs or
neither** (G9), so whole-clause replacement and per-verb replacement are the
same rule. There is no partial clause to disagree about.

**G9: Read-guarded, write-open is not spellable [settled].** On an entity
declaring `access read`, **omitting `access write` is a COMPILE ERROR**,
class `incomplete_access_clause`. Genuinely open writes have an explicit
spelling, and that spelling is the word **`open`**:

```
entity announcements {
  body: text
  access read   viewers(member == $caller)
  access write  open                # deliberate: anyone who may call the op may write
}

entity viewers {
  announcement -> announcements owned
  member: text
}
```

`open` is a **clause body, not an expression** (§2 reserved words: it is
contextual, read by its position after `access read` or `access write`), and
the distinction is load-bearing:

- It **cannot appear inside a walk**: not as an operand, not as an `==` RHS,
  nowhere. `access write open` is the entire clause or it is a parse error, so
  there is no partially-open guard and nothing to misread.
- It is **greppable**: one word, one meaning, findable in every schema review
  and rendered in the shape summary and every manifest that inlines the entity.
- It **does not satisfy the guard-bearing test** for guest exposure (§8,
  Inter-space sharing). An entity whose verb is `open` is not guarded, so an op
  over it is not guard-bearing, so exposing it to guests remains a compile
  error. This is why the spelling had to be a keyword rather than a literal: a
  guard-bearing check that accepts `access read <literal>` can be defeated in
  one word, silently, by the author most likely to be reaching for it.

The previous spelling shown here, `access write true`, **did not parse and is
withdrawn.** G1 admits literals only as an `==` right-hand side and G2 requires a
`$caller`-referencing disjunct, so a bare `true` was never a legal walk; the
rule prescribed a form the grammar rejected. The symmetric case (`access write`
with no `access read`) is the same error and the same fix (`access read open`).

The rule exists because the failure it prevents is
**silent and it looks like success**: an entity carrying an `access` line reads
as guarded to every reviewer skimming a schema, and a missing verb is invisible.
Open-by-omission must not be spellable; open-by-declaration must be greppable.

**G10: Create-mode guards evaluate against the would-be row.** A creating write
checks the write guard against the row **post-defaults, pre-insert**, which is the row
that would exist. Rejection aborts before anything is inserted, and before any
other statement in the group.

**G10a: A creating write is exempt from a write guard that can only be
satisfied by the row it is creating [settled].** There is nothing to protect on a
row that does not exist yet, and a guard whose subject is that row is asking the
author to already hold access rows *for a row with no rows*. The test is static
and per-entity, so the compiler decides it at registration:

> A write guard is **create-exempt** iff **every** `$caller`-referencing disjunct
> of its walk begins with a reverse collection step rooted at the entity being
> created, i.e. every path to `$caller` runs through rows that reference the new
> row and therefore cannot exist yet. Such a guard is **not evaluated** on a
> creating write. It applies in full from the next touch onward.

- This covers the **acl-derived write guard on an acl root** exactly (its walk is
  `<entity>Access(member == $caller …)`, a reverse step into the derived link).
  That closed the create deadlock for good: the derived
  guard no longer refuses the create that the same-group grant exists to make
  reachable.
- It covers **explicitly-declared `access write` guards on the same shape**:
  `access write documentsAccess(member == $caller and canWrite)` on `documents`
  is create-exempt for the same reason and by the same rule. There is one rule,
  not one for `acl` and one for hand-rolled clauses.
- It does **not** exempt guards rooted at an already-existing row. `tasks`'
  `access write project.projectMembers(member == $caller and canWrite)` starts at
  the `project` the create supplies, that row exists, and the guard is evaluated
  normally, because there the question is "may you add a child *here*", which is
  a real question with a real subject. A guard with a mix of disjuncts (one
  self-rooted, one parent-rooted) is **not** exempt: the parent-rooted disjunct is
  answerable, so it is answered.
- What bounds a create for which no guard applies is the **door** (whether the
  caller may call the op at all, G18) plus the op's own body. The artifact says
  so per entity (`create: guard-exempt (self-rooted guard)`), so a reviewer sees
  which creates are door-gated only.

(This is also the honest answer to "join a club": the guard cannot refuse the
creating write, so a domain that does not want anyone creating memberships
withholds the op, or guards the *parent* (see the `acl` bootstrap rule).)

**G11: Bulk writes range over the caller's write view.** The write guard
composes into the bulk selection; an empty selection is the ordinary vacuous
no-op. "Never a silent skip" governs **addressed** writes, where a named row is
being refused; a bulk condition is a description, and describing zero rows has
always been a no-op. Without this rule, caller-scoped bulk ops would be
inexpressible, since write conditions are row-scoped.

**G12: A row you cannot read answers `not_found`, whatever you were trying to
do to it [settled, reverses the earlier ruling].** An addressed read or write
on a row outside the caller's **read** view is `not_found`, byte-identical to
the answer for an id that does not exist. `access_guard` narrows to its honest
meaning: *you can see this row and may not write it*.

The earlier rule answered `access_guard` for both and argued the blur was
unnecessary because server-minted ids are unguessable. That argument is weaker
than it looks. Ids leak into logs, support tickets, URLs, exports, and a former
member's mirror, and once one is in hand, a distinguishable error turns it
into an oracle answering *"does this row exist, and is it in a space I am not
in?"* That is exactly the question a tenancy boundary exists to refuse. The
diagnostic loss is real and small: an honest caller who cannot read a row does
not need to know which of two reasons applied, and the operational log records
the true cause for the operator.

**Creates inherit the same rule [settled].** A create naming a parent the
caller cannot read (`set tasks { project: $p, … }` where `$p` is outside their
view) answers `not_found` on the parent, never a class that distinguishes
"exists but not yours" from "no such project". Otherwise the parent's id space
stays enumerable through the one verb that was never addressed at it. Only once
the parent resolves does the would-be row's own guard run, and *its* failure is
`access_guard`, which is informative exactly when the caller has already been shown the
row.

**Ownership, so this never splits again** (V-010): **this document owns the
rule**, `api-engine-design.md` §4.1 owns the wire rendering, and
`guards-conformance.md` R-G10 owns the cases. Those two state it; neither may
re-rule it. GC016 previously asserted the opposite and has been corrected.

**G13: Guards apply before the op's own `sort`/`take`.** The guard is part of
the entity's **body semantics**: "top N of what the caller may see", never "the
caller's slice of the global top N". Under composition the order is fixed:
**callee guard → callee pipeline (including its `take`) → caller conditions.**
There is no position that outruns a guard.

**G14: `unguarded` composition rules.** Bypass is explicit:
`op allTasks unguarded = …` is greppable and manifest-flagged, for
admin/reporting ops. The marker is scoped and load-bearing:

- It bypasses guards **only for entity sources written directly in the op's own
  body**. A composed callee's guards are callee-body semantics (G13) and cannot
  be stripped from outside.
- Marking an op whose own body touches no guarded entity source is
  `unguarded_superfluous`. The flag must mean something at the point of bypass.
- **Composing a callee whose bypass is live requires the caller to carry its own
  `unguarded`** (`unacknowledged_unguarded`), mirroring the `unbounded` marker's
  rule. Acknowledgment does not launder through composition.
- The manifest lists the bypassed guards alongside the flag.

**G15: Anonymous callers.** `anonymous` = no principal, hence no members row,
hence **every guard evaluates false**, as a defined rule and not as a consequence
of null comparison. Reads return empty; writes reject `access_guard`.
**Unguarded ops do answer anonymous callers** (guards are bypassed; the door is
a separate layer), which is exactly why the `unguarded` flag is loud.

**G16: Guard entities are inlined base entities.** Entities reached by a guard
walk are part of the op's inlined base-entity set: they appear in the **scope
manifest** and are subscribed by **`?live`**. A membership or acl write
therefore invalidates guarded live queries, so permission changes are visible
changes.

**G17: Guard internals are not outputs.** Composition sees a callee's output
shape only; guard-only fields (`canWrite`, a level column) are not in it, and a
caller's condition on one is `not_output_field`. There is no introspection
channel through which a caller probes another principal's guard facts.

**G18: The door layer is upstream.** Roles, profiles, per-member overrides,
and capability allowlists decide whether the op may be called **at all** (§8
Access model); guards decide which rows it sees once it runs. `$caller` binds
post-door. Denial at the door is `access_denied`; denial by a guard is
`access_guard`; they are different answers to different questions and are never
conflated.

Two properties follow. First, the compiler **inlines
guards into every op touching the entity**: read guards filter, and write guards
are preconditions whose failure is a loud typed rejection, never a silent skip.
Second, guards appear in the op's scope manifest, hover, and catalog entry,
*including* inherited ones, so a child's ops always disclose them. Guards vary
results by `$caller` the way any op varies by parameters: the meaning is uniform
and written down. The no-hidden-filter rule is untouched, because guards are authored
in source by the domain; spaces still cannot mutate op semantics at runtime.

**Guards are not a cryptographic boundary in client-first spaces [settled].**
Everything above describes enforcement by a server that can read values. At the
client-first stop on the custody dial the server holds ciphertext and every
enrolled device holds the space key, so guards there are a UI and convention
boundary: they shape what the app shows and what a well-behaved client writes,
and they cannot stop a member from decrypting a row or submitting a write the
guard would have refused. Violations become *detected and revertible*, and are not
prevented. This is the disclosed trade of that custody position, not a
defect (see `implementation.md`, "What blindness costs"). Model accordingly: a
space whose members must not read each other's rows belongs at a custody stop
where the server can still referee.

### `acl`, derived permission machinery [settled]

The common case in one line:

```
entity projects {
  name: text
  acl (read, write, admin)
}
```

Derives, visibly (rendered in the shape summary like ownership):

- A real link entity `projectsAccess { project -> projects owned,
  member -> members owned, level: enum(…), unique (project, member) }`,
  which is queryable, writable, and extendable like any entity (the hand-rolled escape
  hatch stays open; `access` clauses remain for custom shapes).
- **The derived names are a rule, not an example [settled].** Ops, guards, and
  hand-written queries all have to spell this entity, so it is not allowed to be
  illustrative: the link entity is **`<entity>Access`**, the acl-bearing
  entity's name with `Access` appended, lowerCamel preserved (`projects` →
  `projectsAccess`, `careTeams` → `careTeamsAccess`). Its member ref is always
  **`member`**; its ref back to the acl root is the root's name with a **trailing
  `s` dropped when present** (`projects` → `project`, `careTeams` → `careTeam`,
  `staff` → `staff`). This is a mechanical rule, deliberately not English
  pluralization, because derivation must be predictable from the name alone. A
  domain declaring an entity or field that collides with a derived name is a
  compile error, class `reserved_derived_name`.
- **Guards on the entity and its entire `owned` subtree.** Children
  inherit the root's ACL through the ownership graph. An explicit `access`
  clause on a child **replaces** the inherited guard entirely, with no
  composition algebra, ever.
- Admin ops (`grantProjectsAccess`, `revokeProjectsAccess`, level changes) are
  ordinary registered ops with manifests. **The highest declared level
  gates them** (only `admin` grants/revokes), so meta-permissions are pinned.
  `grantProjectsAccess` is **converge-or-create** on
  `unique (project, member)` (mode label `creates-or-converges`: regranting an
  existing holder converges their level); `revokeProjectsAccess` is an addressed
  delete by the same key. These are **op names**, not language verbs, and there is
  no `grant` statement (see "Seeding access is an ordinary write" below).
- **Levels are a full ordering, left to right** (read ⊂ write ⊂ admin). Each
  level includes every lower level's rights, for reads *and* writes, not "for
  read purposes" only. Their names are the app's vocabulary; their order is the
  declaration order.
- **The three thresholds are POSITIONS in the ladder, not names [settled].**
  A derived guard tests "the caller holds a level **at or above** X", and X is
  found by position, because the order is what is pinned and the names are the
  app's:
  - **read** is the **first** level: any level reads the root and its owned
    subtree.
  - **write** is the **second** level: writing the root or its owned subtree
    needs that rung or higher.
  - **admin** is the **last** level: writing the derived link entity itself,
    directly or through the derived admin ops, needs the top and only the top.

  On a two-level `acl (read, write)` the second and last coincide, so `read`
  reads and `write` both writes and administers, which is what those two names
  mean. On a **longer** ladder the second rung is still the write threshold:
  `acl (read, comment, write, admin)` lets `comment` write, because `comment`
  is declared above `read`. An author who wants a rung that reads and does not
  write declares it first. This is stated and not inferred from the names,
  because the platform must never read meaning into `comment`.

  Each threshold compiles to a **closed disjunction of `==` over the qualifying
  levels** inside one reverse step, which is how an ordering fits in G1's closed
  grammar (`level >= "write"` is unspellable there and is not becoming
  spellable). The bottom rung emits no level test at all: every declared level
  qualifies, and the pinned enum is what makes that a tautology and not a
  gap.
- **A caller's effective level is the MAXIMUM of every acl row that applies to
  them [settled].** A caller may match a row directly *and* through one or more
  groups; levels are a full ordering, so the max is well-defined and total, and
  it is the rule. There is no "most specific wins", no "direct beats group", and
  no subtraction: a group grant can raise a caller above their direct row, which
  is the point of granting to a group. The derived guard therefore takes the
  highest level among {the caller's own row} ∪ {rows naming a group the caller
  belongs to}.
- **`revoke` reports every surviving grant path [settled].** Because access can
  arrive by several paths, deleting one row can leave the principal with access
  and an admin believing otherwise. `revokeProjectsAccess` therefore returns the
  paths that **remain** after the delete. For each, it names the acl row that still
  applies, whether it is the principal's own row or a group row (and which
  group), and the level it confers. The derived admin UI must show them.
  Silence here is the failure mode that matters: "I revoked Dana" must not mean
  "Dana is still in Contractors, which still holds write". Revoking group access
  for one person is removing them from the group, which is a `groupMembers`
  command, and the report says so.
- **Derived guards on the link entity itself**: read requires any level on the
  parent row (members may see who else holds access, which keeps "who
  can call X" one honest query); write, whether direct or through the derived
  admin ops, requires the **top level**. The derived walks expand groups
  explicitly (§3 Groups) and the level test compiles to a closed disjunction of
  `==` over the qualifying levels, so the generated guard stays inside G1's
  grammar and is rendered in full in the shape summary.

**Bootstrap: the creator gets the top-level row [settled].**

A freshly created acl-bearing row has no access rows, and the derived admin ops
are gated on holding the top level *on that row*. Without a rule, nobody,
including the creator, can open the door. This is a rule, not a
convenience:

1. **The creator is the principal attributed on the creating command**, that
   is, `$caller` at create time, resolved to their members row exactly as
   everywhere else. Not the op's author, not the domain, not the space owner:
   the identity the command log records. First to make is fine.
2. **The access row is minted in the SAME command group as the create.** One
   group, both facts, both or neither. So there is no instant at which the row
   exists unreachable, and a group revert takes the grant with it. It is an
   ordinary `set projectsAccess` command with an ordinary actor, visible in the
   chain and in `callersOf`, and the derived shape summary states it
   (`acl (read, write, admin): creator receives admin at create`). It is derived
   and disclosed, not declared in config: the commitments hold.
3. **The creating write is not refused by the guard it is trying to
   bootstrap [settled].** The acl-derived write guard on an acl root is
   **create-exempt** under G10a, because every path from it to `$caller` runs through
   the derived link rows, which cannot exist before the row does. This is the
   deadlock's actual close. The earlier
   formulation only *moved* it: a create evaluated against the would-be row (G10)
   would be refused by the very guard the same-group access row exists to
   satisfy, so the create failed before rule 2 could ever run. The guard binds
   from the next touch onward. The same exemption applies to a hand-rolled
   `access write` whose walk runs through the row being created. There is one rule for
   both, stated in G10a.
4. **A creator who is not a member of the space gets no auto-grant
   [settled].** The creator-gets-admin rule applies to **members of the space
   only**. A **guest** principal (bound to the space through an inter-space
   share, holding a members row with a null `role`) who creates an acl-bearing
   row receives **no** access row: the space owner administers it (rule 6), and
   the guest reaches it only through whatever the space grants them. Otherwise a
   guest invited into one corner of a space could manufacture rows they
   exclusively administer, and then extend access to principals of their
   choosing, inside a space they are a visitor in. This matches the
   routine/system rule below: **auto-grant follows
   space membership, not authorship.**
5. **When the create is attributed to a routine, system, or source principal
   (any principal with no members row), NO auto-grant is minted.** There is no
   human to grant to, and inventing one would be the platform declaring consent.
   Such rows are reachable through rule 6 and through whatever ops the domain
   registers; a domain that ingests rows for humans to own says so with an
   ordinary write to the derived link entity (next subsection).
6. **Space owners administer any acl row in their space, and the check is at the
   DOOR [settled].** A space owner may grant, revoke, and change levels on every
   acl-derived link entity in the space **without holding an access row on
   anything**. The mechanism matters, because a per-row reading of this rule
   reintroduces the deadlock it exists to break: the owner capability is
   evaluated **at the door (G18), once per call, before planning**, not as an
   extra disjunct in each row's guard. When it applies, the derived link
   entity's guards are not consulted for that call, and the command is logged
   with the owner as actor and flagged as owner administration.

   **This does not contradict "roles have no implicit hierarchy."** That pin
   (stated in `glossary.md` and `security-logging-design.md`) says access
   *profiles* confer nothing by role rank: a profile lists ops per role, and
   owner does not silently inherit manager's ops. The rule here is a **space-level
   capability the platform attaches to the owner role**. It is one capability, scoped
   to one space, over one thing (acl administration in that space), alongside the
   other owner-only capabilities already pinned (export, deletion, BYO custody
   changes). It confers no op, no level, and no rank over any other role. The two
   readings currently in circulation are the reason this is spelled out here; the
   sibling docs are being aligned to it.

   It covers the routine/system-created rows of rule 5, the guest-created rows of
   rule 4, and the "everyone with admin left the company" case, and it means no
   state of the data can ever lock a space's owner out of their own space. The
   disclosed consequence is stated here: an owner
   can always reach any row in their own space by granting themselves, and every
   such act is in the chain.

The alternative considered was promoting `let x = set …` (bind the server-minted
id and grant in one op body), which is more general and also solves
create-parent-and-child atomically; it is a larger surface and remains a
candidate for that second problem, but it makes day one of every multi-tenant
app a hand-written incantation, and the bootstrap belongs in the derivation.

**Seeding access is an ordinary write, and there is no `grant` statement
[settled].** The acl rules once told a domain ingesting rows for humans to "say
so with an explicit `grant` statement." **No such statement exists and none is
being added**: the language has exactly two verbs, `set` and `del` (§6), and
`grant` is not reserved. That prescription is **deleted**. It was also
unnecessary, because an acl access row is an ordinary derived entity: seeding a
co-owner is a `set <entity>Access { … }` in the same op, in the same command
group, with the same atomicity and the same command-log entry any other write
gets.

```
op assignInvoice($invoice: id, $owner: id) =
  set invoices       { id: $invoice, owner: $owner }
  set invoicesAccess { invoice: $invoice, member: $owner, level: "admin" }
```

Both statements are one atomic group (§6), so the invoice never changes hands
without the access row landing with it. The op-level mode label is
`converges + creates-or-converges`, and `invoicesAccess`'s own write guard
applies to the second statement in the ordinary way: the caller must hold the
top level on that invoice, or be the space owner at the door.

Rule 5's ingest case (the routine-created row with no human to grant to) uses
the same write, from the listener that already knows which rows landed:

```
routine seedInvoiceOwners in billing {
  trigger when billing.invoices.ingested
  run():
    invoices(id in $trigger.ids)[id, owner]
    each -> set invoicesAccess { invoice: id, member: owner, level: "admin" }
}
```

> **Built up to `$trigger.ids`.** The `routine` form and `each ->` bodies are
> built (§8, Routines). This block stops at `$trigger.ids`: reading `$trigger`
> in a body is not built, so `ordo check` refuses it there. The
> `op assignInvoice` form above compiles.

The created access rows are owned by invoices in `$trigger.ids`, so the run stays
inside the listener bound of §8 (a created row is within the bound when the row
it is owned by is). Note what this pattern does **not** need: no new verb, no
platform magic, and no exception to the write guard. The routine writes under
the `routine:` principal, and G10a exempts the creating write from the guard that
could not yet be satisfied.

**Naming a freshly created row in the same op [settled].** A create, and
only a create, may be bound:

```
op startClub($name: text, $coHost: id) =
  let club = set clubs { name: $name }        # CREATE mode, so it binds
  set clubsAccess { club: club.id, member: $coHost, level: "admin" }
```

The restriction is what makes this safe, and it is **statically decidable
because write mode already is**: `let` binds a `set` whose derived mode is
CREATE, because a create produces exactly one row with exactly one fresh
server-minted id. The other modes bind nothing and are a compile error
(`unbindable_write`): CONVERGE and CAS address a row whose id the caller
already holds (there is nothing to learn), and BULK affects an unknown number
of rows, so there is no single `.id` to name.

- The bound value is a **row descriptor** (`id`, `rev`, and the fields the
  statement wrote), usable in *later statements of the same op*, never in the
  same statement and never in a condition of the statement that produced it.
  Statements in a command group already execute in order; this adds no new
  ordering question.
- Writes remain statements. `let x = set …` is a statement that names its
  result, not an expression usable mid-condition. That distinction keeps
  the write grammar closed.
- It composes with the create-exemption (G10a) exactly as the example shows:
  the create is guard-exempt, the dependent write is not, and both land in one
  atomic command group.

This closes the shape every multi-tenant author reaches for first (create a
thing, immediately say who else may touch it) and the general
create-parent-then-child case, without reopening the ordering questions that
kept writes out of expression position.

### Indexes [settled]
Ref fields index automatically: one `(space, ref)` index per ref, named
`entity$field$ix` as the field modifier `index` names it. It is left out where
a declared `index (ref, …)`, a `unique (ref, …)` or a `unique` ref already
leads with the ref, since that index answers the same lookups (reported fix
25y). Composite/extra indexes declared with
`index (a, b)`. A single open field takes the field modifier `index`
(`status: text index`), the one-column form of the same index; `index (a)`
is refused at the spelling, as `unique (a)` is (reported fix 25w). On a
sealed field the only index is the blind index, `sealed indexed`: `index` on
one is `index_on_sealed`, and `indexed` on an open field is
`indexed_without_sealed`. Every generated index leads with the space column.
Build-time advisors (including LLM analysis of registered query
patterns) may *suggest* indexes and sealing changes. Suggestions land in the shape
file for human approval, never at runtime.

---

## 4. Conditions

### The three scopes [settled]

| Where                  | Scoped to                                    |
|------------------------|----------------------------------------------|
| `entity(cond)`         | that entity's rows, before joining           |
| `\| cond` (pipeline bar)| the stream as it exists at that line         |
| `agg(cond)` in `{ }`   | rows inside the bucket being aggregated      |

A condition must leave the source parens only when it cannot live there:
cross-entity comparisons (both sides exist only after the join) and
post-aggregation conditions (the value doesn't exist until `group` makes it).
Entity-local conditions in parens vs. an immediate bar are equivalent (planner
pushes down either way); parens are preferred style.

### Boolean operators [settled]
`and or not` are words, one spelling each (`xor`/`nor` are dropped: rare, expressible,
and plausible-misuse magnets). Parens required when mixing (principle 2).
`true`/`false` are ordinary boolean literals, legal in any condition. Comma is
**not** a boolean operator (it separates projection fields and sort keys only).

### Comparisons [settled]
`== != < <= > >=` · `in` (membership: `category in ($a, $b)` is **exactly sugar
for a chain of `==`** [settled], so it inherits the truth table without
exception: a null in the list matches a null value, an empty list is `false`
for every row, and SQL's `NOT IN (… NULL …)` returning nothing has no
spelling here because `not (x in …)` is the strict negation of that chain) ·
`contains`, `starts`, `matches $pattern` (text; `matches` uses a linear-time
regex engine with Rust `regex` semantics: no backreferences or lookaround, so
pathological backtracking is structurally impossible; always scan-cost,
host-side on sealed fields) ·
`is null`, `is not null` (the only null tests) ·
`since <duration>` (dates: within the trailing window of execution time) ·
`under $id`, `above $id` (tree entities only).

### Null semantics [settled]
Two-valued logic. Comparisons against null are simply **false** (never
"unknown"); `is null / is not null` are the only null tests; `sum` of an empty
set is 0, `count` of nothing is 0. SQL's three-valued folklore is deleted.

**`!=` is strict negation of `==`**, and the full truth table is fixed:

| Expression     | Result |
|----------------|--------|
| `null == null` | true   |
| `null == 5`    | false  |
| `null != null` | false  |
| `null != 5`    | true   |
| `null < 5` (any ordering op) | false |

Equality is reflexive (null is a comparable sentinel for equality only);
ordering predicates always reject null; `==`/`!=` partition every row;
`x != y` ≡ `not (x == y)` with exactly one meaning however spelled.

**Planner note: two-valued logic is a compilation obligation [settled].**
Two-valued semantics do not survive naive SQL generation: `NOT (a = b)` under
SQL's 3VL yields `UNKNOWN` when either side is null and the row is **silently
dropped**, which is the opposite of `null != 5` = true. Therefore every
comparison returns exactly the rows of the distinctness family:
`IS NOT DISTINCT FROM` for `==`, `IS DISTINCT FROM` for `!=` and for
`not (==)`. This holds **everywhere**, without exception: pipeline conditions, source
parens, `cond` arms, `check` constraints, guard semi-joins, join `ON` clauses,
and the predicates behind bulk writes. A plain `=` in generated SQL is a bug
wherever a nullable operand can reach it and change the answer.

How it is spelled (owner ruling 2026-10-26, `guard-equality-uses-no-index`):
Postgres never serves the distinctness family from an index or a hash, so a
guard's semi-join scanned its whole table once per row. The planner therefore
spells `==` as a plain `=` wherever that returns the same rows, and only
there. In a filter (where a null answer drops the row as false does), one side
proven never null gives `a = b`; both sides possibly null give
`a = b OR (a IS NULL AND b IS NULL)`; `!=` with one side possibly null gives
`NOT (a = b) OR a IS NULL`. Under `not` and inside `match none`, a null answer
would be inverted, so only `=` between two columns the database declares
`NOT NULL` is used. Anything unproven keeps the distinctness form. "Never null"
is one rule (`ordo_plan::never_null`): a `NOT NULL` column of a row certain to
exist (not reached through an optional ref, a guarded hop, or an outer join),
`id`, `$caller`, a non-null literal, or an operation that cannot return null
over such inputs. A deployment whose database has lost a `NOT NULL` the rule
relies on refuses to load, naming the column; a parity test runs both forms
over null-filled data and requires the same rows. `check` constraints are
compiled separately and keep the distinctness form.

Ordering comparisons keep their SQL form and are wrapped so a null operand
yields false rather than `UNKNOWN`. This is the one place where the language's
cleanest rule costs the planner the most vigilance, and it is stated here so the
executor need not rediscover it per operator.

### Batch semantic pins [settled]
Rulings on existing syntax the corpora kept re-hitting. They add no new constructs:

- **Field-to-field comparisons** are legal in conditions and cond-arms:
  `commitDate < receiptDate`, `amount > account.limit`. Both sides may be any
  row-scoped expression.
- **`date - date → duration`**, `datetime - datetime → duration`; date-part
  functions accept datetime (`day(ts) → date`, `month(ts) → month`).
- **Expression keys**: `group`, `take[…]`, and `sort` keys accept the closed
  expression grammar (`group month(placed)`, `take[week(at)] 1`,
  `sort lower(name)`, `sort cond(status == "urgent" -> 0, 1)`).
- **Nulls sort last, always, regardless of direction.** One rule, no modifier.
- **Write RHS = the full expression grammar** (`{ armed: not armed }`,
  `{ n: n + 1 }`, `cond`, functions).
- **`distinct` accepts a key list**: `distinct (customer, product)` yields distinct tuples.
- **`week()` = ISO-8601 week** (Monday start, week-year semantics).
- **Addressed writes return the written row** (post-defaults, post-`rev`) as a
  descriptor. This is protocol behavior, not syntax; bulk writes return affected ids.
- **Converge-or-create RHS reads the pre-write row on converge and the
  field's defaults on create.** So with `n: int = 0`, the counter upsert is
  `set counters { key: $k, n: n + 1 }`: create yields 1, converge
  increments. No branch syntax is needed. A read field with no declared default
  is refused at compile time (`invalid_binding_expr`, naming the field); an
  optional field is not read as null (owner, 2026-10-31).

### `since` and reproducibility [settled]
`since` anchors to execution time, so the same op returns different rows tomorrow.
Correct for dashboards and routine conditions. Where reproducibility matters
(audit views, statements), `since` is disallowed in that context; use explicit
bounds or parameters.

---

## 5. Queries (read pipelines)

### Core form [settled]

```
entity(conditions)[projection]
```

Source, restriction, and selection in one breath. Steps follow, one per line.
No `FROM`, no aliases, no `->` arrows between steps.

### Dot-paths [settled]
"Follow the arrow, read the field." `payee.name` walks the transaction's payee ref
and reads `name` there; `category.group.name` chains two hops. Legal anywhere a
field is legal: projections, conditions, group keys. Each hop compiles to a join.

**The one-direction rule:** dot-paths only walk toward "exactly one" (many→one).
A payee has many transactions, so `payees.transactions.amount` is not a value and
does not parse. Walking into collections is what `match any/none` (existence) and
`group` (aggregation) are for. *Values dot; collections match.*

### Projection [settled]
```
[date, amount, payee.name]      # named fields (joins compiled invisibly)
[*, -notes]                     # everything minus
[net: amount - fee]             # computed + renamed    [expressions: partial, see §10]
[at: created]                   # rename (used by append to line columns up)
```

### Match [settled]
Implicit whenever listed entities connect unambiguously through declared refs,
including **transitively, iff exactly one path exists**. With one path through
the ref graph, the compiler inserts the bridge entities itself (`projects
receipts` compiles as `projects ← tasks ← receipts`); with two or more, the
error **enumerates the paths** and the fix is listing a bridge entity to
choose; with zero, the usual no-connection error.

**What "path" means [settled].** The ref graph is a directed multigraph with
parallel edges and self-loops, so the word needs pinning:

- A path is a **simple path in the UNDIRECTED ref multigraph**. Every declared
  ref is an edge usable in either direction, and no entity repeats along a path.
  Undirected because the spec's own bridged example traverses two *reverse* hops
  (`projects ← tasks ← receipts`); simple because non-simple paths admit
  infinitely many walks through any cycle.
- **Parallel edges are distinct paths.** `transfers.fromAccount` and
  `transfers.toAccount` are two edges between the same pair, so `transfers
  accounts` is ambiguous, which is exactly the duplicate-ref case the language
  already handles with `match transfers.fromAccount`. Collapsing parallel edges
  would delete that case.
- **Tree self-loops are excluded.** A `tree` self-ref is not an edge for
  inference purposes; `under`/`above` are the operators for walking it.
- **A unique length-1 path wins.** If exactly one *direct* ref connects two
  listed sources, that edge is the inferred path and longer paths are not
  considered. The direct relationship is what the author means, and this is the
  only rule that makes the intuitive case decidable. Without it,
  `orders products` becomes ambiguous the moment any bridge entity refs both,
  and the prescribed remedy ("list a bridge") **cannot name a direct edge**,
  leaving an error with no expressible fix. Ambiguity *among* direct edges is
  still ambiguity (previous bullet), resolved by `match <ref>`.
- With the direct edge excluded or absent, two or more simple paths remain an
  error that **enumerates the paths**; the fix is listing a bridge entity as a
  source. The enumerate-the-paths error is kept for genuine ambiguity. It is a
  good error and it should fire only when the author really has a choice to make.

**Three or more sources: the minimal connecting subgraph [settled].** Everything
above is defined *pairwise*, and "exactly one path exists" has no meaning for
three or more listed sources: there is no single path to be unique about, and
resolving each pair independently can accept a set of pairwise-unique choices
that do not agree on one join. The rule for the general case:

- Inference considers every **subgraph of the undirected ref multigraph that
  connects all listed sources** (the same graph as above: parallel edges
  distinct, tree self-loops excluded).
- Take those with the **fewest edges**. **Exactly one** ⇒ that is the inferred
  join. **Two or more** ⇒ ambiguity error **enumerating the alternatives**, with
  the same fix as the pairwise case: list a bridge entity as a source, which adds
  a source and usually makes the minimum unique. **Zero** ⇒ the ordinary
  no-connection error.
- **The pairwise rules are exactly the two-source instance of this**, not a
  separate mechanism: with two sources, the minimum-edge connecting subgraphs are
  precisely the shortest simple paths between them, so "unique shortest path
  wins, ties are ambiguity" is the same sentence, and "a unique length-1 path
  wins" is the case where the minimum is one edge.
- Two properties fall out and are worth stating. A minimum-edge connecting
  subgraph is always a **tree** (any edge on a cycle could be dropped and the
  sources would still connect), so inference never fabricates a cyclic join. And
  because the criterion is a global minimum, **listing an extra bridge source can
  only remove ambiguity**. It can never silently substitute a longer join for a
  shorter one.

Guardrails on inference, all compile-time:
- **Pinned at registration** (as with duplicate refs): the resolved path bakes
  into the artifact; later ref additions can never re-route a deployed op.
  If a recompile's inferred path *differs* from the previous version's, the
  deploy diff surfaces it as reviewable drift (like cost-class drift).
- **Disclosed everywhere**: inferred bridges appear in the plan, the scope
  manifest (their guards inline too), hover/inlay, and the catalog.
- **Fan-out accounting**: inferred hops count against the fan-out guard
  identically to listed ones.
**Brackets are optional when a source projects nothing [settled].** `[]` means
"listed, nothing projected", and a source that projects nothing may spell it
either way: `customers[] orders[]` and `customers orders` are the same
pipeline, and a **fully bare source parses**: `plants reduce { n: count }` is
legal, as is `transactions group month(date) { … }`. This closes three
questions at once: an explicitly listed **bridge** takes `[]` or nothing; a
**grouped anchor** that contributes only its key (`let o = customers[]
orders[] group customers.id { … }`) is legal, because it is projecting nothing
and that is now a general permission, not a bridge-only exemption; and
the single-source no-projection form needs no ceremony. The projection-of-nothing
error is retired, because there was never a way for `[]` to mean anything else.
Unqualified field names are legal only when unique across sources; otherwise
qualification is mandatory.
Ambiguity errors surface when compiling new/edited
queries, which is where they belong. The word `match` appears only when it
carries information:

```
match transfers.from_account     # choose between duplicate refs
match any receipts               # semi-join: has at least one (nothing projected)
match none receipts              # anti-join: filter by absence
```

### Aggregation [settled]

```
group payees.name { n: count, refunds: count(amount > 0), spent: sum amount }
| refunds >= 3
```

- Aggregates: `count sum avg min max`, `median field`,
  `percentile(field, p)` (0 < p < 1 literal; continuous interpolation, exact
  decimal, empty set ⇒ null), `distinct field` (count-distinct), and
  `joined(field, ", ")` (ordered string aggregation of a text field; order =
  the pipeline's current sort, id tiebreak)
- **Aggregate arguments admit expressions**: `sum (price * (1 - disc))`,
  `avg (qty * weight)` (same closed expression grammar as projections)
- `distinct` is also a **step**: bare `distinct` after a projection dedupes rows
  (SELECT DISTINCT)
- Legal **only** inside `{ }` after `group`/`reduce`, and nowhere else in the grammar
- **Only aggregates and renames of aggregates are legal inside `{ }`**. A bare
  per-row field there is a compile error; carry per-row values by putting them
  in the (comma-separated, multi-key) group key: `group invoices.id, total { … }`
- Conditional aggregates: `agg(cond)`, where the condition chooses which rows
  participate in the math
- The bar **after** a group filters buckets (SQL's HAVING, deleted by position)
- `reduce { ... }` = aggregate to exactly one row (a group with no key)

### Multi-source join semantics [settled]
The explicit multi-source form **preserves rows from the one side**
(outer-preserving): `vendors[name] invoices[total]` keeps vendors with no
invoices. Their child columns are null, and aggregates see an empty bucket
(`sum` 0, `count` 0). Consistent with nullable-ref dot-paths keeping the row.
Beware the null-comparison trap this sets: a parent with no children has null
child fields, and under two-valued logic `at <= $x` is false for null, so "never
watered" needs `or at is null` to count as thirsty. (Conformance case.)

**One output name, one column [settled 2026-10-05].** Each source's
projection adds its fields to one output list, and each output name may
appear in it once. `customers[note] orders[note]` is refused as
`projection_unknown_field`. Rename one side to keep both:
`customers[note] orders[order_note: note]`. Every reader of a result finds a
column by its name, so two columns with one name would leave the caller with
one of them and no way to know which.

### Fan-out guard [settled]
A pipeline may contain **at most one many-path**. Joining two independent
many-sides under a shared parent (accounts × transactions × receipts-of-account)
is the classic star-aggregation double-count. In Ordo it fails to parse.
Compose with `let`, `match any/none`, or separate queries instead.

**The guard counts BRANCHES, not hops [settled].** A chain of one-to-many steps
that keeps descending the same line, such as `projects ← tasks ← receipts` (the inferred
bridge in this document's own flagship match example), is **one many-path, not
two**. Depth is free; branching is not. Precisely: the guard fails when the join
tree contains an entity with **two or more child many-branches**, or when two
independent many-chains are joined. It does not fail on a longer chain.

The reason is what the guard is for. Two *independent* many-sides under one
parent multiply into a Cartesian product of unrelated multiplicities, and an
aggregate over it counts each row once per row of the other side, giving a wrong
number silently. A chain does not do that: each step refines the grain of the
previous one, the result rows are rows of the deepest entity, and `sum` over them
counts each deepest row exactly once. Requiring the flagship example to be
rewritten (or worse, letting it fail to parse) would have cost the language its
most-used inference for no correctness gain.

The rollup rule above (§3, `whole`) is the same distinction seen from the other
side: within one anchor the owned closure is a tree, so a chained descent counts
each row once; two branches would not.

### Determinism [settled]
The typed id is auto-appended as the final key to every `sort`, `take[…]`
ranking, and pagination ordering. Total order holds everywhere and ties are impossible.

**Cursor stability is conditional, and the condition is provable [settled].**
Keyset pagination gives no-skip / no-duplicate results **iff every sort key of
the op is write-stable**. The compiler can decide this: it knows every registered
write's braces, so it knows the closed set of fields any op can move.

**Write-stability is judged against AUTHORED writes only [settled].** A field is
**write-mutable** iff some **authored** `set`/`del` statement in the deployment
(an op body or routine body a person wrote) can write it. The std family's
derived full-row converges are **excluded from the analysis**. Without this
exclusion the label is vacuous and provably so: `std.<entity>.update` is derived
for *every* entity and lists *every* declared field, so every non-id sort key in
every deployment would be write-mutable, `cursor: stable` would be unreachable
except for id order, and a label no op can carry is not a label. What the
exclusion costs is stated openly: a deployment that **grants**
`std.<entity>.update` to callers can move any field, so `cursor: stable` means
"no authored write moves this key", not "no command in the universe can". The
artifact therefore also records, per sort key, whether the std family could move
it, so a reviewer granting that family sees which paginated ops become drifting
in fact.

Consequently:

- **All sort keys write-stable ⇒ the op is labeled `cursor: stable`** and the
  no-skip/no-duplicate guarantee holds unconditionally. The implicit id
  tiebreaker is always stable, so an unsorted or id-sorted op is always stable.
  That is the common case and the std default.
- **Any sort key writable ⇒ the op is labeled `cursor: drifting`** in its
  artifact, its catalog entry, and its descriptor. Pages remain totally ordered
  and individually correct; a row whose key a concurrent write moves may be
  skipped or repeated across page boundaries. Sort `-spent`, page 1 ends at
  `spent = 16000`, a write drops that customer to 5000: page 2's predicate
  matches the same row again. That is a property of keyset pagination over
  mutable keys, and the label is the honest form of it.
- **Composed post-take pagination is always `cursor: drifting`.** Paginating a
  caller's filter over a callee's `take 100` recomputes the window per page, so
  both skips and duplicates are possible even when the outer keyset is
  well-behaved.

The label is derived, costs no runtime machinery, and is the reason the
unqualified claim "no skips, no duplicates, ever" is not made anywhere in this
document.
`append` is UNION ALL: columns matched by name, types must agree exactly (no
coercion, no null-filling). Misalignment is a compile error; use projections and
renames. An entity may appear **once** as a source per pipeline; two-role access
goes through distinct refs and dot-paths (`transfers.from_account.name` vs
`transfers.to_account.name`); true self-joins are intentionally unexpressible
until proven needed. `let` results may be referenced as scalars **only when the
body ends in `reduce`**. Otherwise the reference is a compile error.

### Let outputs as sources [settled]
A `let` result is a first-class source: list it beside entities (or other
lets), restrict it with parens, project it, and **join it back** via
provenance-tracked keys. Group-key columns remember where they came from (an
entity's `id`, or a ref such as `dept`); match inference connects a let's key
column to any listed source of that entity (or sharing that ref) exactly as if
it were a declared ref. Explicit `match on <col>` resolves ambiguity; joins are
outer-preserving like all multi-source joins. A let may be referenced by any
number of later pipelines in the op.

```
let deptAvg = employees[salary] group dept { a: avg salary }

employees[name, salary, dept.name]  deptAvg[a]     # match inferred: dept
| salary > a
```

```
let o = customers[] orders[]  group customers.id { nOrders: count }
let r = customers[] reviews[] group customers.id { nReviews: count }

customers[name]  o[nOrders]  r[nReviews]           # both matched on customers.id
```

### Cost classes [settled]
Every registered op classifies by its worst stage:
`sql` (fully pushed) · `blind-index` · `bounded-host` · `unbounded-host`.
**Bounded means statically provable**: the compiler can prove a maximum
candidate count from identity/unique addressing, `take`, a bounded list
parameter, or a declared relationship cap.

**`take` bounds *returned* rows, not *examined* ones [settled].** The
distinction decides whether an op needs the marker, so it is stated here. `take 50` after a filter proves the caller receives fifty rows;
it proves nothing about how many were looked at to find them. So `take` bounds
the host-side work in a **projection** (decrypting the sealed fields of the
rows that survive) and bounds nothing when the sealed work is in a **filter or
a sort key**, because those touch every candidate before `take` can discard
anything. Practically: `entity(open filter)[sealed field] … take 50` is bounded;
`entity(sealed field == $x) … take 50` and `sort <sealed field> … take 50` are
not, and need the acknowledgment. A `reduce`-ended op is never bounded by
`take`, since it has no `take` to give. A date predicate *filters* but
proves no maximum, so sealed aggregates over date ranges are `unbounded-host`.
Sealed sort keys with keyset pagination are **prohibited** (cursor stability
cannot be guaranteed host-side); paginate on open keys or acknowledge a bounded
window.

**A keyset page is a row bound [settled 2026-09-05].** A plan served as a
keyset page over a total order is row-bounded (`corpus-rulings.md`, "Keyset
paging counts as a row bound"). A many-side join below the order breaks the
total order, so that plan is not. The page bounds a sealed **projection** the
way `take` does: each request opens the payloads of one page. A `group` or
`reduce` over rows that carry a sealed field opens nothing when no aggregate
reads the sealed value: `count` folds the rows away, no sealed value
survives, so the plan is plain SQL, releases no key and needs no `unbounded`
(ruled 2026-10-05). An aggregate over the sealed value itself (`sum total`,
`min total`, `avg total`) and any expression over it (`coalesce(total, 0)`)
is refused as `sealed_unsupported`. The database holds only the ciphertext
and cannot compute on it, and opening every row on the host to fold it is
not built. This is what lets a sealed-inclusive `std.E.list`
register as `bounded-host` (`std-library.md` §5, rules 6 and 7).

**Served [2026-10-05].** The executor serves a `bounded-host` read. Each page
takes one read key release for the tenant and the request, opens that page's
envelopes, and returns plaintext. A read answered whole needs a bound the
plan proves. An `unbounded-host` read is refused at run time as
`sealed_unsupported`, naming the sealed field and the fix.

**The `unbounded` gate covers row cardinality as well as venue [settled].**
Cost *classes* rank where work happens. Scanning is a separate dimension, and
for a while this document claimed the venue gate covered both. It does not: an
op that aggregates every row of every entity in a space classifies `sql`,
because every stage pushed down, and would have needed no acknowledgment at all.
"Nobody ships an O(space) dashboard by accident" was false for open fields, so
the gate is extended to make it true:

- **Registration computes a row-cardinality bound alongside the cost class**,
  from the same evidence (identity/unique addressing, `take`, bounded
  parameters, declared relationship caps, tree depth caps).
- **An op with no provable row bound fails registration unless acknowledged with
  `unbounded`**, independent of venue. `unbounded-host` and unbounded-`sql` are
  the same acknowledgment word, because they are the same author decision: *this
  op's work grows with the size of the space, and I mean it.*
- The artifact carries both facts (`cost: sql, rows: unbounded`), so a reviewer
  reading a deploy diff sees a full-space scan appear as a change.

The claim is now true as stated for every field, sealed or open: an O(space)
dashboard is shippable, greppable, and never accidental.

**Refinement and effective cost class [settled].** Every program read accepts
caller-supplied refinement (`$refine`, see `api-engine-design.md` §7), with no
op-level word and no off switch (ruled 2026-10-05). Writes, platform ops and
public reads take none. The registered class is not automatically the served
class, so the door computes an **effective cost class per request**: the
registered class **⊔** the class implied by the refinement's own conditions.
The language-side rules that make that decidable:

- **An open field's condition is `sql` and is never gated.** It compiles into
  the op's plan like a parameter's condition, whatever the operator.
- **Refinement over a sealed field is opt-in per field at declaration**
  (`sealed indexed refinable`), so it lands in the field's shape summary, in the
  op's manifest, and on the consent page. Default is not refinable.
  `refinable` is a mark on sealed fields only; on an open field it is
  `refinable_without_sealed`.
- **Where enabled, sealed refinement is equality-only**: it rides the blind
  index. Caller-controlled post-decrypt *ordering* or range refinement is the
  oracle, so neither is expressible.
- **Aggregate-only ops accept no sealed refinement at all** (`invalid_refinement`).
  A `count` that pairs with a `list` inherits the list's manifest for field
  *names*, but a sealed condition under an op that returns only a number is a
  per-row equality oracle with a row-count readout: *if you may not read the
  rows, you may not measure them.*
- **A refinement that would push the effective class to `unbounded-host` is
  rejected**, `refinement_unbounded`, unless the op was registered with the
  `unbounded` acknowledgment. A raise to `blind-index` is served. The
  declaration-time gate and the request-time gate are complementary; either
  alone leaves the hole open in one direction.
- **A sealed condition only the host can answer is refused at the door** as
  `sealed_unsupported`, naming the field, until host-side filtering is built.

`ordo check` prints each read's refinable fields and what each allows (any
operator, equality only, refused). What the door refuses only at run time,
and how to avoid each refusal, is `docs/howto-cost.md`.

### Other steps [settled]

```
sort -total, date          # multi-key; - = descending
take 20                    # query semantics only ("the top 20 IS the result").
                           # Pagination is NOT syntax: any op's results page at
                           # call time via an opaque token in the request
                           # envelope (keyset, derived from the op's sort +
                           # id tiebreaker; stateless; stability per the op's
                           # derived cursor label; see Determinism)
take $n                    # legal: the count may be a parameter, and the
                           # parameter must carry a bound ($n: int <= 200) so
                           # the op's cost class is still statically provable.
                           # An unbounded parameter here is `unbounded_take`.
take[accounts.id] 1        # grouped limit: "from each set defined by the keys,
                           # take N", with ranking = the pipeline's current sort (no
                           # sort yet -> id order, still deterministic). Keys are
                           # any fields/dot-paths incl. the implicit id; multi-key:
                           # take[property.id, kind] 1. Brackets always enclose a
                           # field list: after a source it's the projection,
                           # after take it's the bucket keys; anywhere else they
                           # fail to parse (loud, never silent).
append <source...>         # union replacement; columns line up by name
let name = <pipeline>      # named intermediate step; replaces subquery nesting
```

### Live queries [settled]
Every named query is live-capable by nature (named ⇒ callable ⇒ sourceable ⇒
subscribable). There is no definition flag. **Liveness is a call-time choice**: the
caller opts in (`?live`), holds a session subscription, and may pause or drop it.
Live sessions stay fresh via **invalidation**:
a session-owned listener on the command-log envelope (same machinery as
triggers, so it inherits its blindness: it knows *that* something relevant changed,
never *what value*); on match the server signals stale and the client refetches
through the normal op under its own scope. Transport is modular: delta-push can
replace refetch behind the same subscription interface, and does for the ops a
device keeps in sync. The IndexedDB mirror is that pattern one level up. A
device keeps named ops in sync, and for each the server sends row-level changes
to that op's guarded result for the caller, so a device holds only what an
ordinary call would return; a principal whose reads are unguarded may sync
whole entities, which is the whole-space feed as a special case (owner-ruled
2026-09-14, `plan-client-cache.md` §2). Per-query subscriptions serve screens
nothing syncs.

**Time-relative ops carry a derived re-evaluation tick [settled].**
Invalidation is commit-driven, but an op containing `since` changes its result
by **the passage of time, with no commit**, so a subscription to
`transactions(date since 30d)` would otherwise display a 31-day-old row forever:
a staleness false negative in the one feature whose entire point is not being
stale. The rule: **an op containing `since` carries a re-evaluation tick,
derived from the finest granularity in the op**: day for a `date` anchor, the
smallest addressed unit for a `datetime` anchor. The subscription
re-signals on that boundary in addition to on commit. Derived, never configured;
an op with no `since` has no tick and costs nothing.

### Named-query composition [settled]

Every registered read query is one concept, two faces: an **API operation**
(called by name with params) and a **source** (other queries build on it by
name). There is no separate "view" construct, and composition adds no
keywords. A registered query name is legal anywhere an entity can appear, in
exactly the entity source form:

```
name(bindings-and-conditions)[projection]
```

- **Parens: dollars bind, fields filter.** Comma-separated, two kinds of entry,
  told apart by the `$`: parameter bindings `$param: expression` and
  conditions. Every required parameter of the callee must be bound, and the compile
  error otherwise; defaulted parameters may be omitted. Binding expressions use
  the closed expression grammar and may reference the caller's own `$params`
  (parameters flow through). Conditions see the callee's **output fields
  only**: its result shape, never its internals or base entities. Parens are
  omitted entirely when there is nothing to pass (`activePosts[title]`); empty
  parens do not parse.
- **Projection selects output fields.** `[…]` names a subset of the callee's
  output fields; omitted = all of them. A ref-typed output field remains a
  typed ref, so dot-paths keep walking through it downstream; value fields are
  plain values. Renames and computed projections over output fields follow the
  ordinary projection grammar.
- **Semantics: the callee finishes first.** Composition is compile-time
  inlining: the composed query flattens to **one SQL plan**; nothing consults
  a view at runtime (materialization = later optimization flag; the semantics
  here are the contract either way). The callee's full pipeline, including its
  `sort`/`take` if any, completes first; the caller's conditions apply to that
  **result** (post-take), and the caller's pipeline continues from there
  (further conditions, `group`, `sort`, `take`, `append`, `let` all legal).
  "Top 10, then filter" and "filter, then top 10" are different queries, and
  position says which.
- **Depth unlimited, cycles never.** Queries compose to any depth. A query
  sourcing itself, directly or transitively, is a compile error at
  registration.
- **Everything derives through the flattening.** The field-scope manifest
  derives from base fields through any depth, so composition cannot launder
  scope. The composite's cost class is the max of everything inlined.
  Determinism holds: the implicit id tiebreaker survives inlining, so ordering
  stays total on composed ops. Cursor stability does **not** automatically
  survive: a composed op inherits the weakest cursor label of everything
  inlined, and any pagination applied *after* a callee's `take` is
  `cursor: drifting` by construction (see Determinism). `?live` on a
  composed op subscribes invalidation to the base entities of everything
  inlined.
- **No special cases.** A composed source is a source: it counts against the
  fan-out guard like any other, and sealed/blind-index rules are unchanged. A
  condition on a sealed output field costs what it always costs (see cost
  classes), wherever it is written.

```
op monthlySpend =
  transactions(amount < 0)[amount]
  group month(date) { spent: sum amount }

op heavyMonths($floor: money) =        # condition on the callee's OUTPUT
  monthlySpend(spent < $floor)[month, spent]
  sort month
```

```
op recentTransactions($from: date) =
  transactions(date >= $from)[date, amount, payee]   # payee: still a typed ref

op recentByPayee($from: date) =        # $from flows through; ref dots onward
  recentTransactions($from: $from)[amount, payee]
  group payee.name { n: count, spent: sum amount }
```

```
op topSpend =
  transactions(amount < 0)[date, amount, payee]
  sort amount
  take 100

topSpend(date since 30d)[payee.name, amount]   # filters the top-100 RESULT,
sort amount                                    #   not "top 100 of the last 30d"
```

### Worked examples

Simple:
```
transactions(amount < 0 and date since 90d)[date, amount, payee.name]
sort amount
take 10
```

Star join, four entities, zero ceremony (dot-path form):
```
transactions(date >= $from)[date, amount, account.name, category.name, payee.name]
sort -date
```

Same star, explicit multi-source form (needed when aggregating across levels):
```
transactions(date >= $from)[date, amount]  accounts[name]  categories[name]
sort -transactions.date
```

Aggregate + bucket filter:
```
payees[name]  transactions(amount < 0)[amount]
group payees.name { n: count, spent: sum amount }
| n >= $min
sort -n
```

Anti-join, transactions with no receipt:
```
transactions(date since 90d)[date, amount, payee.name]
match none receipts
```

Scalar baseline via `let`, spends over 3× average:
```
let baseline = transactions(amount < 0) reduce { a: avg amount }

transactions(amount < 3 * baseline.a)[date, amount, payee.name]
sort amount
```

Latest-per-group, each account's most recent transaction:
```
accounts[name]  transactions[date, amount]
sort -transactions.date
take[accounts.id] 1
```

Append, one feed from two shapes:
```
alerts(active)[title, at: created]
append reminders(due)[title, at: due_date]
sort -at
take 20
```

Named-query composition (formalized above), where dollars bind and fields filter:
```
monthlySpend(month >= $from)[month, spent]
frequentPayees($min: 5, spent < -1000)[name, spent]
```

Cross-entity condition (can't live in either source's parens):
```
accounts[name, limit]  transactions[amount]
| transactions.amount > accounts.limit
```

Tree traversal (projects domain):
```
tasks(project under $root and status == "open")[title, project.name]
```

Time bucketing, the finance report:
```
transactions(amount < 0)[amount]
group month(date) { spent: sum amount }
sort month
```

---

## 6. Writes

### One verb: make true [settled]

`set` means **make the described state true**. Its mode is derived statically
from the statement's content. The compiler decides it, never the runtime:

```
set txns { account: $a, amount: $amt }         # no identity → CREATE
                                               #   (a new member is the only way
                                               #    to make this state true)
set txns { id: $id, category: $c }             # id → CONVERGE that member
                                               #   (absent = error; ids are
                                               #    server-minted, so absence
                                               #    means deleted; no resurrection)
set txns { id: $id, rev: $rev, category: $c }  # rev present → compare-and-set
set txns { externalId: $e, amount: $amt, ... } # unique key → CONVERGE-OR-CREATE
                                               #   (idempotent ingestion:
                                               #    one statement per record)
set txns(payee == $p and category is null) { category: $c }
                                               # parens → BULK CONVERGE all
                                               #   selected; empty selection is
                                               #   vacuously true (no-op)
del txns { id: $id }                           # remove addressed member
del txns(status == "void")                     # bulk remove
```

Two verbs total: `set` (make true) and `del` (remove). Removal is a genuinely
different operation. No `add`, no `ensure`, no `one`.

### Safety rules [settled]: all compile-time or structural

1. **Mode is static and displayed.** The compiler labels every registered write
   op `creates` / `converges` / `creates-or-converges` / `bulk` in registration
   output and in deploy diffs. Reviewers read a machine-checked label, not a
   human-remembered convention. **Multi-statement ops carry per-statement labels
   [settled]**: the artifact lists one label per `set`/`del` statement, in
   statement order, and the **op-level label is the union** of them (a transfer
   that creates a ledger row and converges two balances is
   `creates + converges`). A single label would have to lie about at least one
   statement, and the reason the label exists is that a reviewer can trust it.
   The wire response reports the op-level union; the artifact and the catalog
   carry the per-statement list.
2. **Creation-capable statements must be insert-complete**: any statement whose
   mode can create (no-identity, or unique-key form) must statically cover all
   required fields. There are no fair-weather writes that work only when the row exists.
   Compile error, class `insert_incomplete`, naming every uncovered field.
   "Required" is the schema's word: neither optional nor defaulted, and not
   implicit. `id`/`rev`/`depth` are the platform's and are never covered by
   the caller. **Both checks run at registration** (the same gate as the
   `unbounded`/`unguarded` acknowledgments), so a statement that could only
   fail on the create branch never deploys and never surfaces as a NOT
   NULL violation to whichever caller reaches it first.
3. **Bulk braces may not contain identity or unique-key fields** (compile
   error, class `bulk_identity_field`). Addressed and bulk cannot blur, and
   one unique value across a range asks every matched row to collide on that
   key. A `sealed indexed unique` field counts: the claim rides the blind
   index. Deletes carry no braces, so the rule has nothing to say about them.
4. **No resurrection**: converge on an absent id is an error, never a create.
5. **Bare `del entity` does not parse**; delete-everything is `del entity(true)`.
6. **Plain refs restrict deletes** (refuse-with-referrers); **owned rows go
   with their owner** (owner ruling 2026-09-29, `owned-delete`): a plain `del`
   deletes every row that references the target through an `owned` ref,
   transitively, derived acl rows included, in the same command group. A tree
   node's own children need the explicit `subtree` form. Rows a plain ref
   still points at from outside the cascade refuse the whole delete.
7. **The condition is the write's entire specification.** Row counts are outputs
   of reads, never inputs to writes, so there are no cardinality bounds anywhere. A caller
   that wants the number first runs the same condition as a count query
   (ordinary read, obviously advisory); bulk-confirm UIs compose exactly that.
8. **Identity precedence**: `id` in braces → id-addressing, always (other
   unique fields are just values). No id + exactly one covered unique key →
   converge-or-create on that key. No id + **two or more covered unique keys →
   compile error** (ambiguous identity). At runtime, if the addressing key
   identifies row A but another unique value collides with row B, the write is
   rejected as `conflicting_identity` and nothing is applied. If the addressing key's
   bound value is **null**, the write is rejected as `null_key_address`
   (see Uniqueness, §3), never a create.
9. **`rev` states a belief about the addressed row, and is legal alongside
   `id` *or* a unique key [settled, extends the earlier id-only rule].**
   `rev` was previously legal only with `id`, which left a real hole: a
   unique-keyed write is converge-or-create, so
   `set txns { externalId: $e, amount: $amt }` called twice with different
   amounts **silently updates** the first row. The caller who meant "record
   this once" had no way to say so, and the platform's own idempotency key
   only covers transport retries, not a second call made in earnest.

   Two beliefs, one spelling:

   - `rev: $r` means *"the row is at version r."* Converge if so; `cas_miss`
     otherwise. Now available on keyed writes as well as id-addressed ones:
     "update the row with this external id, but only if it is still the
     version I read."
   - `rev: null` means *"there is no such row."* The write must **create**; if the
     key already matches a row, it is a typed rejection (`expected_absent`)
     and nothing is applied. This is create-once, and the mode label the
     compiler prints becomes `creates` instead of `creates-or-converges`,
     so the intent is machine-checked and visible in the deploy diff.

   `id` with `rev: null` is a compile error: an id is only obtainable from a
   row that existed, so asserting its absence is incoherent. Everything else
   follows the existing rules: `rev` still requires *some* addressing, and a
   bulk form still may not carry identity fields.
10. **Converge touches only described fields**: unlisted fields keep their
    current values (the create branch applies defaults). "Make the described
    state true" describes fields, not the whole row.

### Atomicity [settled]

A registered write op is **atomic across all its statements**: a transfer is one
op containing two addressed `set` calls, and both land or neither does. Commands from
one invocation form a **command group** (shared correlation id) committed in a
single database transaction. **Triggers and live-query invalidations fire only
on commit**, so intermediate states are unobservable, and revert operates on whole
groups. Routine proposal batches apply identically.

### Concurrency & idempotency [settled]

- **Compare-and-set**: include `rev` in an addressed converge; a rev miss is an
  error, the client refetches. Generated forms and agents CAS by default; lost
  updates become explicit conflicts.
- **RHS field references**: `{ n: n + 1 }` is computed server-side inside the
  row's transaction, giving atomic increments without read-modify-write races.
- **Idempotency & duplicates** are decomposed, with each piece owned where it
  is solvable. Ids are **server-minted, always** (typed format as settled; no
  client-born identity). *Transport retries*: an idempotency key in the request
  envelope, auto-generated and auto-reused by the SDK retry loop (plumbing, not
  data identity; the gate dedups: same key, same result, no re-execution).
  *Ingestion duplicates* (webhook redelivery): converge-or-create with a
  declared unique external id. *Semantic duplicates* (a human legitimately
  entering the same purchase twice): unsolvable by the platform by definition;
  belongs to UX. Bulk `set` has no CAS by design: its condition evaluates
  atomically at commit, so it is last-writer-wins, declared; callers needing more pin it
  in the condition.

### Blessed patterns (system over syntax) [settled]

- **Soft delete / default scopes**: no entity-level filter construct. Define a
  named query (`activePosts = posts(deleted is null)`) and source everything
  from it; composition is the scoping mechanism, and scope-flattening keeps
  manifests honest.
- **Orphan/conditional-existence cleanup**: write conditions stay row-scoped
  (no existence predicates). Cleanup is a rule: query with `match none` →
  `each -> del …`, which is attributed, scheduled, and revertible.
- **Quotas/limits**: counter field + `check` invariant (above), inside one
  atomic group.
- **External effects ("call Stripe, write the result") use the outbox
  pattern**: the language has no I/O, so an effect is *data first*. The
  domain op writes an **intent row** (an ordinary entity: kind, payload
  fields, `status = "pending"`, unique idempotency fields) atomically with
  its domain state in one command group, so intent and cause commit or abort
  together. A blind listener trigger wakes a **tier-2 effector**
  (service-principal, capability token, declared egress) that reads pending
  intents through a registered query, performs the outside call, and records
  the outcome with converge-or-create on the intent's unique key, which is idempotent
  under redelivery and retries by construction. **The provider-side idempotency
  key is derived from the intent id** [settled]. The effector sends the
  intent's typed id (or a deterministic function of it) as the provider's
  idempotency header, so a redelivered intent is deduped *by the provider* as well as by us. Ordo's own converge-or-create makes the *record* idempotent;
  only the derived provider key stops the outside world from being charged
  twice. Timeouts/retries ride
  trigger modifiers + an `attempts` counter with a `check` cap; compensation
  is another intent. The consent page shows the effector's egress; the
  command log shows the whole causal chain
  (`user → op → intent → source:stripeEffector → outcome`).

### Compilation [settled]

Every write compiles to per-row, actor-attributed commands through the gate,
with before/after images captured, hash-chained per space, grouped per invocation.
Even a bulk `set` gone wrong is one group-revert away; the command log is the
safety net under the whole write family.

---

## 7. Triggers [settled]

> **Built since 2026-10-08 (checklist 12a), in the routine's spelling.** A
> trigger is written inside a routine, as `trigger every 5m`, `trigger every
> day 09:00Z` or `trigger when <domain>.<entity>.<kind> [debounce 60s]
> [coalesce]` (§8). The platform runs it in every tenant. Two compile classes
> guard it: `invalid_routine` and `routine_out_of_bound`. See
> `howto-routines.md`.
>
> **Not built yet:** the top-level `when … -> run x` line shown below is
> refused with a pointer to the routine form. A wildcard entity in a change
> trigger, reading `$trigger` in a body and rate caps are not built. The design
> text stands.

Blind and logicless: the trigger layer sees only shape and time, never values.
Two primitives (everything else reduces to them, and webhooks and manual/agent
requests enter as appended events):

```
when finance.transactions.ingested -> run spendWatch    debounce 60s
every day 02:00Z                   -> run nightlySweep
every 5m                           -> run heartbeat coalesce
```

- **Listener**: pattern on the command envelope only (event kind, space, entity
  type, actor, all fields outside the encryption boundary by construction)
- **Clock**: cron-grade schedules
- Modifiers (no data needed): `debounce <dur>`, `coalesce` (at most one pending
  run), rate caps

**Event kinds are a closed set [settled].** A listener pattern is
`<domain>.<entity>.<kind>`, and `<kind>` comes from this vocabulary and no
other. An unrecognized kind is a compile error (`unknown_event_kind`), not a
pattern that silently never fires:

| Kind | Fires on |
|---|---|
| `created` | a command whose mode created the row |
| `converged` | a command that changed an existing row |
| `deleted` | a row removal (including the per-row commands of a `subtree` cascade or of an `owned` cascade) |
| `written` | the union of the three above (the common case, one word) |
| `ingested` | a create attributed to a `source:` principal (webhook, sensor, mail slot) |
| `fed` | a write attributed to `source:feed-*` (an inter-space feed copy landing) |
| `reverted` | a group revert touching the entity |

Wildcards are legal at each position (`finance.*.written`,
`*.transactions.ingested`), and only at a whole position, with no prefix globbing.
`ingested` and `fed` are *narrowings* of `created`/`written` by actor kind, not
separate events: a listener on `written` sees feed copies too, which is what
makes feeds locally reactive, and the anti-loop rule in §8 is what keeps that
from being a cycle.

Modifier example, since both are named more often than shown:

```
when estate.readings.written -> run meterAnomaly   debounce 5m coalesce
```

`debounce 5m` is trailing-edge: the run fires 5 minutes after the last event in
the burst, with a declared max-wait; `coalesce` caps pending runs at one.

Anything that must *see a value* ("field A > 5") is not a trigger. It is the
first line of a routine, running under a grant, with its key release logged.

**Key release [settled]:** there is no separate key-release grant to manage.
Release is **derived from op access**: a caller allowed to invoke an op whose
manifest includes sealed fields thereby (and only thereby) triggers release
for exactly those fields, for that run. Every release is logged (principal,
op, fields, space, instant). No op access, no path to a key: revoking the
op revokes the release; manifest expansion kills grants as usual.
Wasted wake-ups are mitigated by debounce and by routines' declared retained
state (rolling aggregates readable without re-slicing the ledger).

**Blind but addressable [settled]:** the run receives an opaque `$trigger`
context holding the command-group id, **affected row ids**, batched event ids, and timestamp.
All plaintext-skeleton data (no field values), so trigger blindness is intact,
but rules can scope to what changed (`events(id in $trigger.ids and …)`)
instead of rescanning history. Clock-fired runs get an empty id set.

**Operational semantics [settled]:** delivery is **at-least-once** with
idempotent runs (a run is keyed by **batch id × routine version**, so a debounced
batch is one identity and redelivery is a no-op);
failures retry with backoff, then dead-letter as an operator alert; ordering is
guaranteed **per space only**; `debounce` is trailing-edge with a declared
max-wait; schedule times **must be written with the `Z` suffix**
(`every day 08:00Z`), so UTC is explicit at the keyboard and never silently implied;
per-space local scheduling is a possible later extension, DST sidestepped.

Listener cursors interact with log pruning, and the interaction is what makes
the guarantee true: **the archive floor never passes the slowest registered
consumer, and a listener cursor is never evicted from that calculation.**
At-least-once is the same promise as the floor rule, seen from the routine's
side. A stalled *mirror* may be evicted so one
abandoned device cannot pin a space's history forever
(`security-logging-design.md` §7, `consumerStall`); a listener may not, because
evicting one and resuming it at the current floor would silently turn
at-least-once into zero-times. A listener that stops making progress
dead-letters as an operator alert, which loses nothing; a listener that should
stop pinning the floor is **deregistered**, explicitly and attributably.

The command log serves as the event bus today: ordered, attributed, per-space.
This is an implementation choice and not a permanent promise; a broker could replace the
transport without changing the language model.

---

## 8. Platform definition blocks

All illustrative syntax is **[lean]**: concepts are settled and the file format is not final.

### Routines, two tiers [settled]

> **Built since 2026-10-08 (checklist 12a): a routine with a write body.**
> `routine <name> [in <domain>] { trigger ... run[(...)] [unguarded] [nopause]:
> <write statements> }` compiles and runs. Each run lands as one command group
> attributed `routine:<name>@<version>`. Parameters are `$now` and `$today`
> only. A change routine may touch the changed rows and rows one declared ref
> away, unless `run` says `unguarded`. A paused tenant's runs are skipped and
> recorded unless `run` says `nopause`. Two compile classes guard it:
> `invalid_routine` and `routine_out_of_bound`. See `howto-routines.md`.
>
> **Built since 2026-10-08: `each ->` bodies over a read pipeline.** A run
> handles at most `ORDO_ROUTINE_EACH_MAX_ROWS` rows, 500 by default, and the
> log says when more matched. Tier-1 blocks that read `$trigger` in a body
> stop there: reading `$trigger` is not built. A routine added or retimed by a hot deploy takes effect at the next
> restart; an edited body takes effect at once. The design text stands.

**Tier 1: rules, written in `.ordo` itself.** Most routines are "query → per
row → propose," which the language already expresses; one construct added:
`each ->` (per-row proposal emission).

```
routine leaseExpiry in estate {
  trigger every day 08:00Z
  run($today: date):
    leases(status == "active" and end is not null and end <= $today + 60d)
      [id, end, unit.name]
    each ->
      set alerts { key: "leaseExpiry:" + id, severity: "warning",
                   title: unit.name + " lease ends " + end }
}
```

Consequences: compiled once per version like every op; **scopes are derived**
from the ops the body uses (no hand-declared reads/writes to drift); sandboxing
is moot because the language has no I/O primitives, so a rule cannot misbehave; and
user-authored automations are just rules (same grammar, same limits). Retained
`state` may still be declared (enumerated, user-visible, deletable,
re-derivable).

**`each ->` scope [settled].** Inside the emission braces, the names in scope
are exactly **the fields of the row the pipeline just produced**, and no more. After
a `group`, that means **the group keys and the aggregate names**, and nothing
else: the per-row fields that went into the bucket are gone, because the bucket
is the row now. Carry a per-row value into an emission by putting it in the
group key, which is the same rule as §5's "only aggregates and renames of
aggregates are legal inside `{ }`". `$trigger` and the run's own `$params`
remain in scope throughout, as parameters always are.

```
routine cohortDigest in reading {
  trigger every day 07:00Z
  run($today: date):
    progress(at since 1d)[pages]  clubs[name]
    group clubs.id, clubs.name { pages: sum pages }
    each ->
      set digests { key: "daily:" + id, club: id,
                    title: name + " read " + pages + " pages" }
}
```

#### Routines are not callers [settled]

A routine has no membership row, so under G15 every guard would evaluate false
and **every clock-driven rule in a guarded domain would be a silent no-op,
forever, with no error**. That is the wrong failure, and the fix is to say what
a routine actually is:

**A tier-1 routine runs as a distinguished `routine:` principal that access
guards evaluate as trusted platform code, so guards pass.** It is not "the routine holds
rows" and not "the routine is exempt from a rule it would otherwise fail". Guards
constrain **callers**: they answer "which rows may *this outsider* see through
this op". A tier-1 routine is not a caller. It is the domain acting on itself,
authored in the same file as the shapes, deployed in the same artifact, running
inside the same trust boundary as the guards themselves. Asking a domain's own
rule to prove membership to its own data is asking the lock to present a key to
itself.

What actually bounds a routine, therefore, is not guards:

- **Its DERIVED SCOPE MANIFEST is its real constraint.** The routine can read
  and write exactly the entities and fields its body touches. These are computed from the
  body, never declared, greppable in the artifact, rendered on the consent page,
  and pinned into every grant. A routine that starts reading a new entity gets a
  new manifest and is dead for every space until re-consent. This is a *tighter*
  and far more legible bound than a membership row would be, because it is
  per-field and it cannot drift.
- **Its trigger is its second constraint.** A routine runs when its clock or its
  listener says so, on the rows `$trigger` names, and never on demand from an
  outsider. There is no path by which a caller invokes a routine to launder
  their own visibility: routines are not registered ops and are not callable.
- **Its attribution is its third.** Every command a routine writes is attributed
  `routine:<name>@<version>` in the space's chain, so the audit question is
  answerable in the ordinary way.

**Trigger provenance sets the bound [settled, supersedes the blanket rule
above].** "Guards pass" is correct only where nobody outside chose when the
routine runs. Two reviewers independently found the hole: `when` listeners
fire on `ingested` (source principals) and `fed` (copies from another space),
and `$trigger.ids` hands the run exactly the rows an outsider just wrote. So
a guard-exempt routine becomes a **confused deputy**, doing work an outsider
steered on rows the outsider could never touch. The bound therefore follows
the trigger, not the principal:

| Woken by | Bound |
|---|---|
| **clock** (`every …`) | guards pass, because nobody outside chose the moment |
| **listener** (`when …`) | the run may read and write **only rows reachable from `$trigger.ids`**; every other row is invisible exactly as a guard would make it |

A listener routine that must legitimately range wider says so with the
existing `unguarded` flag on its `run` header. That is one greppable word, a decision
someone made on purpose. This keeps what the blanket rule was protecting (a
clock-driven rule can never silently do nothing) and removes what it created
(an outsider-steerable master key). Scope manifests are unchanged: they bound
*which fields*, and were never a row bound. That conflation was the error.

**Tier-2 service principals are the opposite case and keep guards.** An external
categorizer holding a capability token *is* an outsider calling registered ops;
it gets a members row like any other principal, and guards apply to it in full.
The distinction is trust boundary, not automation.

**The `run` header takes flags [settled].** Acknowledgments that are expressible
on ops are expressible on routines, in the same words and the same position:

```
routine nightlyAudit in finance {
  trigger every day 02:00Z
  run($today: date) unguarded unbounded:
    allTransactions[...]                 # composes an unguarded op
    each -> set auditFindings { ... }
}
```

Flags sit between the parameter list and the `:`, and are the same closed set as
§13's op flags: `unguarded`, `unbounded`, `reproducible`. This closes the hole
where G14 required a composing op to carry `unguarded` and the routine grammar
had nowhere to write it: the required acknowledgment is now writable, so the
bypass stays greppable at the point of composition. Whether a routine's own
direct entity sources need `unguarded` follows the trigger, per the provenance
rule above: a **clock-woken** routine's direct sources need nothing (guards pass
for it), while a **listener-woken** routine's sources are bounded to
`$trigger.ids` and reaching past that bound is exactly what `unguarded` on the
`run` header acknowledges. Composing a callee whose bypass is live requires the
marker in either case, because that marker is about the callee's semantics, not
the caller's identity.

**Grant pinning [settled].** A grant pins **per-op manifest hashes**, not one
combined hash. Per-op rather than combined because an unrelated op's change must
not invalidate a consent that never mentioned it.

**A pinned manifest carries the field set AND the row-scope shape [settled].**
Comparing field sets alone is decidable and **wrong**, and the counterexample is
not exotic. It is the most ordinary edit in the language. Delete
`member == $caller` from an op body, or add `unguarded` to its header, and the
op's **output field set is byte-identical**: same columns, same types, same
order. A field-set comparison reads that as "not an expansion", classifies it as
narrowing, and **applies it silently**. A grant a person consented to as *"this
app may read my expenses"* becomes *"this app may read everyone's expenses"* with
no re-consent prompt, no audit flag, and no visible change anywhere. Field sets
answer **which columns**; they never answered **which rows**, and rows are what
the consent was about.

So the pinned manifest carries both, per op:

1. **Field set**, as before, per entity, derived through composition.
2. **Row-scope shape**, for every entity the op reads or writes: the **guards
   that apply** (entity, verb, and the identity of the walk, including inherited
   and acl-derived ones), whether the op body carries a **`$caller` restriction**,
   and the **`unguarded` flag**. For a routine, the shape additionally carries the
   **trigger class** (clock vs listener), because that is what bounds which rows
   the run may touch (§8, Trigger provenance).

**Narrowing = field-set subset AND row scope no weaker.** Both conditions, or it
is not narrowing:

- *No weaker* means: every guard present in the consented shape is still present
  and is the same walk; a `$caller` restriction present then is present now;
  `unguarded` is not newly set. **A trigger-class change alone does not kill a
  grant [settled].** Clock↔listener changes *when* a routine runs and which
  rows it may reach, but a grant is consent to a **kind** of access, not to a
  schedule, so the class change matters only when it changes the grant kinds
  the routine needs (a listener reaching past `$trigger.ids` must set
  `unguarded`, and *that* flag is a row-scope weakening that does kill the
  grant; a clock routine narrowing to a listener bound is strengthening and
  applies silently). The rationale: killing every
  consent whenever an author retimes a routine would train owners to click
  through re-consent, which is how consent screens stop being read.
- **Strengthening row scope is narrowing** and applies silently like any other
  narrowing: adding a guard, adding a `$caller` restriction, or clearing
  `unguarded` all shrink what the grant reaches.
- **Any weakening of row scope kills the grant.** Dropping a guard,
  dropping the `$caller` restriction, or setting `unguarded` makes the grant
  **dead for that space until re-consent**, *even when the field set is
  unchanged or strictly smaller*. There is no combination of column narrowing
  that buys a row-scope widening.
- A **superset or disjoint** field set is expansion, exactly as before.

This remains decidable from the two manifests alone, with no registry history.
The pre-round-1 formulation quietly required that history, since it promised that
narrowing applied silently *and* that any hash change killed the grant, and
narrowing changes the hash.

**Tier 2: heavy intelligence lives outside the platform.** LLM categorizers,
vision analysis, model retraining are **external service-principals** calling
the generated API: capability tokens, registered queries in, registered writes
out, egress declared on the consent page. The platform hosts no foreign code in
v1 (in-process WASM hosting deferred until third-party in-process logic is
actually wanted).

### Functions and secrets [built, increment 1]

A `function` is for what the language cannot express, such as calling an
outside API. Its code is a JavaScript file bundled with the program. It runs
in a fresh WebAssembly instance per call. It holds no database URL and no
secret value. `howto-functions.md` is the user guide.

```
function suggestTitle in notes {
  call($text: text(20000)) -> json by member
  code "fn/suggest-title.js"
  egress api.anthropic.com
  secrets llmKey
  limit wall 30s, cpu 2s, memory 64 mb
  nopause
}

secret llmKey for api.anthropic.com
```

Grammar:

```
function <name> [in <domain>] { <clause> ... }
clause = call(<param>, ...) -> json [by member]
       | code "<path>.js"
       | egress <host>, <host>, ...
       | secrets <name>, <name>, ...
       | limit <wall|cpu> <duration> | memory <size>, ...
       | nopause
secret <name> for <host>, <host>, ...
```

- `function` and `secret` head a declaration only at the start of a top-level
  line. They are not reserved words, so a field may be named `secret`.
- Clauses go in any order. Inside the braces a line break ends nothing.
- `call` and `code` are required, and each appears once. `call` takes typed
  parameters like an op, checked at the door. It answers `json`.
- `by member` is the default and the only caller built. `by public` and any
  other caller are `invalid_function`.
- `code` is a relative path ending in `.js` or `.mjs`, at most 20 MB. It
  stays inside the program's directory. `ordo apply` hashes the file into
  the manifest.
- `egress` lists the hosts `fetch` may reach. A host with a hyphen goes in
  quotes: `"my-model.example.com"`. Any other host is refused before DNS.
- `secrets` lists the secrets the code may use, by name. Each must be a
  declared `secret` bound to at least one `egress` host.
- `limit` takes `wall` and `cpu` (durations such as `30s` or `500ms`) and
  `memory` (a size with a space, `64 mb`). Defaults: wall 60 s, cpu 10 s,
  memory 128 MB. Ceilings: 15 min, 60 s, 512 MB. Memory under 8 mb and a
  zero `wall` or `cpu` are refused.
- `nopause` keeps the function answering while the tenant is paused.
- `secret name for host, host` binds the value to exact host names, and at
  least one is required. A URL, a port or a wildcard is
  `invalid_secret_host`. A host whose first label starts with a digit is
  quoted. The value is never in the program. The deployment owner sets it
  with `ordo secret set`, from standard input.
- `trigger`, `uses` and `webhook` on a function are not built. The parser
  refuses them and says so.

Refusals:

| Class | When |
|---|---|
| `invalid_function` | a duplicate name, a name an op or routine has, a domain that is not the program's, an answer other than `json`, a caller other than `member`, a bad code path, a bad or repeated host, a repeated secret, a limit outside its bounds, or a listed secret that is undeclared or not bound to an `egress` host |
| `duplicate_secret` | two `secret` lines name one secret. Put every host on one line |
| `invalid_secret_host` | a host that is a URL, has a port or a wildcard, or is not a DNS name |

The page calls a function with `POST /fn/<name>` or the SDK's
`callFunction`. Run history is `ordo fn runs` and `ordo fn log`.

### Domain package

> **Not built yet [2026-10-04]: the compiler refuses the block form today.**
> Only the header line `domain finance @ 2.1.0` parses; a `{` after it answers
> "expected a declaration". The `shapes`, `ops`, `routines` and `bootstrap`
> lines below have no parser arm. The design text stands.

```
domain finance @ 2.1.0 {
  shapes   accounts, transactions, categories, payees, receipts
  ops      register, spendingByPayee, monthlySpend, setCategory
  routines categorizer, spendWatch, receiptMatcher, missedPayments
}
```

**A package may declare the root rows a space cannot function without
[settled].** Many domains have exactly one row that everything else hangs off:
the organization, the household, the practice. Before this block the only
way to get one was a second call after space creation, which is the state
`bootstrap.md` exists to abolish. The `bootstrap` block names those rows and the
fields the creating caller must supply:

```
domain incidents @ 1.0.0 {
  shapes    organizations, incidents, responders
  ops       …
  bootstrap organizations { name, timezone }     # singleton root rows
}
```

Rules, and they are narrow on purpose: a bootstrap entity must be an **`acl`
root** or unowned; its listed fields are **required inputs** to space creation
(and to enabling the package on an existing space); its row is created **inside
the genesis command group** with the bootstrap actor as creator, so the settled
creator-gets-top-level rule (§3) mints its `acl` row in the same group. **A
bootstrap block declares rows, never grants, never principals, never
memberships**, so the IaC boundary is unmoved.

### Principals [settled]
A principal is anything whose actions must be attributable and permissible. It is the
unit identity, grants, and the command log hang off. Taxonomy:

| Kind     | Examples                          | Provisioned via                          |
|----------|-----------------------------------|------------------------------------------|
| human    | users                             | identity service (invite / auto-onboard);|
|          |                                   | IaC only for the bootstrap admin         |
| routine  | spendWatch, categorizer          | implicit, from manifest                  |
| worker   | platform pods                     | implicit, infrastructure                 |
| agent    | an assistant/LLM client w/ token  | **IaC**                                  |
| source   | camera hub, bank webhook sender   | **IaC**                                  |
| client   | finance PWA, web shell            | **IaC** (see below)                      |

Only externally-originating identities are declared; everything platform-born is
implicit. **Agents** are outside callers using what domains expose: registered
ops (HTTP or MCP) plus trigger requests; no special machinery, identity exists
so scope and attribution apply. Delegation recorded: `agent:<name> on behalf of
user:<id>`. **Delegation is itself a grant [settled]**: acting on behalf
of a user requires that user's own consent (user → agent, expiring,
revocable, command-logged), distinct from the capability token's op
allowlist. A manager granting an agent ops never grants it *someone else's
face*; without a live delegation grant the agent acts only as itself. **Sources** are mail slots: write-only outsiders (bank webhooks,
camera hubs, email-ingest, sensors), authenticated per sender, scoped to event
kinds, attributed in the log (`source:cameraHub`); never readable access.

```
principal cameraHub { kind: source, writes: [security.event] }
principal ellanor    { kind: agent }
```

> **Not built yet [2026-10-04]: the compiler refuses `principal` blocks
> today** ("`principal` is reserved and not implemented"). The same holds for
> `client`, `auth`, `channel` and `route` below, and for the `policy` blocks
> further down. The design text stands.

### Identity clients (OIDC relying parties) [settled]
One Ordo deployment, one OIDC issuer (horizontally scaled pods are one logical
platform). A *client* is not a domain: domains are server-side cargo; a client is
a thing that authenticates and calls ops, and a web shell serving several domains can
be a single client. **Apps are plural per deployment [settled]**: an *app* (a
named UI surface manifest, see `ui-gen.md`) sits above domains; several apps
share the issuer and may share a client (sign in once, every app live). An app
may reference ops from any enabled domain, since the manifest compile-check spans
domains, and apps targeting the same space are just windows onto one world
(same membership, profiles, chain). App ↔ domain ↔ space are deliberately
many-to-many.

```
client appWeb {
  redirect  https://app.example.com/cb
  tokens    access 15m, refresh 30d
  stepUp    [export.all]        # ADDS to what these ops already require
}
```

**Step-up is a property of the op, not of the client [settled, corrects the
earlier client-config reading].** A client may **add** or **tighten** a
requirement and can never subtract one: requirements combine by *strongest*
(presence beats none; the shorter freshness window wins), the deployment's
window is a hard ceiling, and a token's `azp` names the client so the door can
look up the op's intrinsic minimum independently. The earlier arrangement kept the
step-up list only in the client block, which let a caller present a
permissive client and reach a step-up-gated op through it. Protection then
depended on which door the caller used.



**Login methods [settled]:** one human identity, multiple linked authenticators.
Methods are pluggable and per-deployment configurable:

- **Default-on:** passkey (RP = the deployment's registrable apex domain,
  which is **deployment configuration** set by `ORDO_APEX_DOMAIN` [proposed], see
  `licensing.md`. Changing it strands every enrolled authenticator, so it is
  a one-time decision per deployment.
  Always required for step-up), username/password, email OTP (reuses the
  platform email channel)
- **Configurable:** OAuth providers (Google, Discord, …) per deployment

```
auth {
  methods  passkey, password, email-otp        # defaults
  method   google  { client: $GOOGLE_CLIENT }  # opt-in per deployment
  method   discord { client: $DISCORD_CLIENT }
}
```

### Access model [settled]
Definitions declare what exists and what it touches (field-scope manifests,
which are disclosure metadata derived from bodies). **Who may call what is runtime state,
never syntax, never IaC**, the same boundary as grants. Four layers:

1. **Membership roles** are per space and user-managed: owner / manager / editor /
   viewer. **Space owners additionally hold one platform capability: acl
   administration in their own space** (§3, bootstrap rule 6). It is **checked
   here, at the door, once per call**, and not as a per-row disjunct in every
   derived guard. So no arrangement of the data can lock an owner out of their
   own space, and the check cannot itself deadlock on the rows it exists to
   repair. It is a **space-level capability, not a role tier**: it grants no op,
   no acl level, and no rank over manager/editor/viewer, so it is consistent with
   the "roles have no implicit hierarchy" pin in `glossary.md` and
   `security-logging-design.md`. Profiles still confer ops per role with no
   inheritance between roles.

   **No acl row is private from the space owner, and that is the definition of
   an owner [settled, resolves Z-004].** An owner who can administer acl rows
   can grant themselves any level on any row, so nothing inside their own space
   is cryptographically or structurally hidden from them. This is a *disclosed
   property*, not a hole: a space is the unit of custody, and the person who
   holds custody is by construction able to reach what they hold, the same way
   a database owner can read their database. Two obligations follow rather than
   a fix: every such reach is an ordinary attributed command in the space's
   chain (the owner cannot look without leaving a record), and the consent page
   for anyone joining a space they do not own must **say so plainly**. Members
   who require rows the owner cannot reach need a different space, not a
   different rule.
2. **Groups** are named principal collections, per space and user-managed (§3). A
   group may be named anywhere a principal may (memberships, profiles, acl
   rows), and `callersOf` resolves through them, so "who can call X" still
   answers with humans. **Group expansion lives in this layer only**: `== $caller`
   never expands groups anywhere in the language (§3), so a *guard* that wants
   group semantics writes the membership walk explicitly, while acl rows and
   profiles naming a group apply to its members by this rule. Group membership
   changes are ordinary audited commands. Groups are what covers departments,
   contractor cohorts, and org hierarchy; the four roles stay fixed and the org
   chart lives in data.
3. **Access profiles** are a role→ops mapping, per space, runtime data. A domain
   ships a default profile as a *template*, applied when a space enables the
   domain, then owned by the space (managers tighten/loosen). Profile changes
   are commands, so permission history is auditable.
   **Per-member overrides [settled]**: a profile may add or remove ops for a
   named member on top of their role. The four roles stay fixed; overrides
   carry the long tail of "this person specifically." Same runtime state,
   same audited commands; "who can call X" remains one exhaustive query.
   **Visibility is ops, never mutation of ops [settled]**: a space tunes
   what a member sees by which ops it exposes to them (domains ship op
   families at different disclosure levels; `$caller`-scoped bodies give
   per-row self-visibility). A space can withhold an op or expose a
   narrower one, but it can never attach a hidden filter that makes the same
   op silently mean less. Allow, deny, or a different *visible* op.
3. **Capability tokens** apply to synthetic callers (agents, API clients), which never inherit
   role defaults. Their token carries an explicit op allowlist, enumerated by a
   human at grant time on the consent page.

Enforcement is one check at the door before planning: token → principal →
(role → profile) or allowlist → op included? Denials logged with the attempted op.

### Inter-space sharing [settled]

Spaces never merge; sharing goes through one of two doors. Rule of thumb:
**window vs copy**.

**Guest access (the window).** A guest is a person who holds a **seat**
in the granting space: a `members` row with a null `role`, keyed by their
deployment principal and minted by the platform when they accept a share. The
guest passes exactly the guards their ACL rows satisfy, calls the granting
space's ops under their own identity, and is attributed in the granting
space's chain as themselves. Same rows, same guards, same `callersOf` audit.
The member just isn't from around here. What a seat reaches comes from a share
grant and from nothing else (`docs/plan-sharing.md`). Pins:

- **Guests hold no role and no profile.** A session token for a seat with no
  role is refused at the door (`NoRole`); the seat reaches only the ops a share
  grant lists, through a window token. An op exposed to guests must be
  **guard-bearing** (an `access` guard or `$caller` condition), enforced when
  the share is registered (`std.space.share`), so exposing an unguarded op to
  guests is a refusal (`guest_exposure_unguarded`) before any ticket exists,
  not an incident. A share is read-only until a guest may write, and never lists
  a platform op. **`access read open` / `access write open` are not
  guard-bearing** (G9): `open` is a keyword declaring the absence of a guard, so
  an op over such an entity fails this check exactly as an unmarked entity
  would. This is why the explicit-open spelling is a keyword and not the literal
  `true`. A literal that satisfied the walk grammar would have defeated this
  check in one word.
- **A guest creating an acl-bearing row gets no auto-grant** (§3, bootstrap rule
  4): the creator-gets-admin rule follows space membership, not authorship, so a
  visitor cannot manufacture rows they exclusively administer and then extend
  access from inside someone else's space. The space owner administers such rows.
- **Invitation, never enumeration**: shares flow link/invite → accept →
  principal binding. Ids are exchanged only on accept; acceptance is recorded
  in the granting space's chain and in the deployment's security stream against
  the person (owner, 2026-09-29: the other party is a person who may hold no
  tenant, so nothing is written to a tenant of theirs); no cross-space
  principal lookup exists.
- **Shareability is computed from custody** and shown at share time: a
  window requires the granting space's key-release path (sealed fields), so
  user-keyed / client-first content is unsharable-by-window. Offer a copy
  instead.
- Revocation closes the window at the next call. It stops access, not
  memory (a guest's mirror may hold what it already fetched). This is stated
  plainly.

**Consented feeds (the copy).** A standing routine replicating a *defined
slice*, which is a named query's output so the shared surface has a manifest,
into the receiving space. Pins: **one-way**; both ends consent (a feed
nobody asked for is spam); receiver owns the copies outright (attributed
`source:feed-<name>`, no write-back in v1, because bidirectional sync is a
different product); revocation stops the feed, already-copied data stays,
stated at consent. Feeds out of client-first spaces compute on the
sharer's device, best-effort and device-gated. Copies land as ordinary
writes on the receiver's chain (`source:feed-<name>`), so **the
receiver's own triggers and rules fire on them**. Cross-space
reactivity becomes local reactivity, which makes feeds the blessed
app-to-app interface (routines and triggers are space-local by design;
the only other automated bridge is a tier-2 service principal holding
capability tokens for both spaces, consented on each side).

Budget sharing, concretely: partner on managed custody → window (an ACL
row on the budget's subtree; live, revocable). Read-only snapshot out of a
client-first space → copy (computed on-device, receiver's custody).

### Space bootstrap [settled]

Creating a working space is **one platform operation**, `std.space.create`, and
this subsection states the guarantees the rest of the corpus may rely on. The
full input inventory, the three entry paths, and the failure semantics live in
`bootstrap.md`; the rules are here because guards, `acl`, and the access model
all assume the state it produces.

- **One surface.** Space creation, the owner's `members` row, domain
  enablement, access-profile instantiation, the domain's bootstrap rows, and
  their `acl` creator grants are **one op with one input set**. There is no
  sanctioned sequence of smaller calls that reaches the same state, and no
  documented order to get wrong.
- **One command group.** Every fact in the previous bullet lands in a **single
  command group at chain position 0** of the new space's chain, with an ordinary
  actor, ordinary before/after images, and `revert_group` semantics. It is
  single-space like every group: the deployment-level facts (slug uniqueness,
  the space registry) are database constraints and control-plane records, not
  space-chain commands. There is no `unguarded` in it and no synthetic "the
  platform did this" actor.
- **A space always has at least one owner, minted at creation [settled].** The
  genesis group mints exactly one `members` row with `role: owner`,
  `status: active`, for a **human** principal, and a space can never exist
  without it. This is not a convenience: §3's `acl` bootstrap rule 6 makes the
  space owner the universal repair path for orphaned `acl` rows, checked at the
  door, so a space with zero owners is the one data state from which nothing
  can be repaired from inside. The owner is minted even when that human has
  never logged in (an invited owner's principal is provisioned first); "pending
  owner" is not a state.
- **Ordering is fixed**: validate everything, reserve the space, provision
  keys, provision storage and schema, write the genesis group, activate. **A
  space that has not reached `active` is not callable by anyone**, including for
  reads and including by its owner, so no half-created space is ever a usable
  space. Detection and removal of half-created spaces is
  `bootstrap.md` §4.
- **`std.space.create` binds no space [settled].** It declares
  **`scope deployment`**, the one op class whose door check consults a
  deployment capability rather than a space's access profile, because there is
  no space yet whose profile could list it. `$space`/`X-Ordo-Space` on such a
  call is an error, not a hint; everything else about the door is unchanged.
- **Custody and jurisdiction have no defaults [settled].** Both are required
  inputs to space creation. Deployment config may pre-fill the form; it may
  never satisfy the input. Every other position on the custody dial than
  `managed` additionally requires the acknowledgement token that proves the
  computed capability list and the envelope disclosure were shown.
- **Bootstrap grants nothing beyond the owner.** No capability tokens, no
  additional memberships, no per-member overrides, no group memberships, no
  outbound sinks. The access-profile **template** is the only permission-shaped
  thing that crosses from a package into a space, and instantiating it is a
  copy, not a grant. Consent remains runtime state a human creates, which is the same
  boundary that keeps grants and memberships out of IaC.

### Exit ops [settled]
Full space export and space deletion always exist, and they are **owner-only,
step-up gated, command-logged ops** (export = total disclosure; deletion =
the crypto-erasure path, DEK destruction after an explicit confirmation
window). BYO-DB spaces use the same ops with the same gating. Custody changes where
the bytes live, never who may end them.

### Policies (deployment owner decisions, never user consent)

> **None of the policy blocks in this section parse yet [2026-09-16].** The
> word `policy` is lexed and the trail ends there: there is no parser arm, no
> AST node, and no consumer in `ordo-schema` or `ordo-plan`. That holds for
> every block below, and for `policy compliance` further down. The status
> markers say whether a design is agreed, which these are; they say nothing
> about whether anything reads them, and until now this document gave a
> reader no way to tell the two apart. Building the grammar is roadmap
> **18g**, ruled 2026-09-16 (`policy-grammar`): mark them here now, and
> schedule the grammar when policies are actually wanted, so the first caller
> does not get to shape it around the narrowest case. The spellings below
> stand; what is missing is a producer for them.

**`policy spaces`: who may make a space, and how many [settled].** Space
creation is the one write that happens *before* a space exists, so it cannot be
governed by a space's own profile. It is governed by the deployment, and the
governing is a policy and not a special grant kind:

```
policy spaces {
  creators   owner, agent(provisioner)   # who may call std.space.create
  allowance  data                        # how many: a runtime number, not a constant
  defaults   custody: managed, jurisdiction: US, retention: standard
  requires   acknowledgement(custody), acknowledgement(jurisdiction)
  genesis    managed | client-first      # the dev's choice; see bootstrap.md
}
```

The split follows the pattern the rest of the platform uses: **the rule is
code, the facts are data**.

- **The rule** lives here: which principal kinds may create at all, which
  defaults apply, which acknowledgements are mandatory, and whether a space may
  be born client-first or must be created managed and converted.
- **The allowance is runtime state**, one number per owner, moved by whatever
  the deployment uses to move it, such as a billing integration, the deployment owner, or a
  free-tier default. "Three spaces on the free plan, more when you subscribe"
  is therefore not a platform feature and needs no platform vocabulary. It is a
  counter a service updates, enforced by the ordinary quota shape (a counter
  field plus a `check` invariant, §3), inside the same atomic group as the
  creation. Overselling is structurally impossible for the same reason seats
  are.
- **Capability tokens need no new scope.** An agent that provisions spaces is
  named in `creators` and holds an ordinary capability grant for
  `std.space.create`; the grant is deployment-bound because the *op* is
  deployment-scoped (`scope deployment`, see `api-engine-design.md`), not
  because grants gained a second kind of scope. The pinned rule that a grant
  names exactly one space is untouched: deployment-scoped ops name none.

Enforcement sits at the deployment door, before the space registry row is
reserved, and a refusal is an ordinary logged denial naming the attempted op.
It is `quota_exceeded` when the allowance is the reason, so a caller can tell "you
may not" from "not right now."


```
policy commandlog {
  undoHorizon    90d         # fine-grained undo guaranteed
  checkpoint     1mo         # signed chain checkpoints (enable pruning below)
  archive        r2://ordo-archive, bundle 1mo   # JSONL per space per month
  attest         every 1000 commands or 1h       # external anchoring cadence
  consumerStall  30d         # cursor idle this long leaves the floor calc
}
# This block is the closed key list for `policy commandlog`. Every key here is
# specified in security-logging-design.md (§7 for the first three and
# consumerStall, §6 for attest); a key that document introduces and this list
# does not carry is a bug in this list.

policy vault {                   # client-first spaces only
  snapshot  every 5000 commands or 7d   # sealed client-side, chained, verifiable
  keep      3                           # snapshots retained before pruning
}
# A device cold-starts from the newest snapshot it can open and folds forward
# from there; segments below the newest snapshot that every enrolled device has
# acknowledged may be discarded. An enrolled client-first device is a
# non-evictable consumer; un-enrol it to stop it pinning the floor.
# (implementation.md, "Snapshots and compaction")
policy llm-egress { redact [accountNumbers, notesPatterns] }
policy retention {
  events   keep 90d                    # age-based
  logs     keep 100 per user           # count-based, per key
}
# enforced by the platform's nightly sweep as attributed system deletes
# (actor: system:retention). "Keep last N" is policy, never a query
```

### Compliance pack [lean, draft, needs legal review]

> **Unparsed, as above [2026-09-16]:** `policy compliance` has no producer
> either, and waits on roadmap 18g with the rest.
Per-space jurisdiction flag drives defaults; platform features map to legal rights:

```
policy compliance {
  jurisdiction  EU | US | US-CA        # per space, set at creation
  erasure       crypto                 # GDPR Art.17 / CCPA delete → DEK destruction
  portability   export-bundle          # GDPR Art.20 → full space export (JSON + files)
  breachWindow  72h                    # GDPR Art.33 notification clock
  retention {
    finance.transactions  7y  legalHold-capable   # financial/tax record norms
    commandlog            per commandlog policy
    notifications         90d
  }
}
```

Known tension, resolved per-jurisdiction: GDPR data-minimization vs. financial
record retention (5–7y norms). Retention schedules are per-entity-class, with
legal-hold flags suspending erasure where law requires. Consent-page language must
state retention per class. (Determined by engineering reading of GDPR/CCPA; get
counsel before commercial tenants.)

### Notification channels [settled]
Platform-owned delivery; domains never touch providers and emit `notify`
proposals, the platform routes. Transports: web push (user's registered devices),
email, SMS, outbound HTTP hooks (machine consumers), Discord DM (later). Platform
manages provider credentials, templates, per-user routing preferences, payload
minimization (detail revealed in-app), and delivery receipts in the command log.
Email OTP login reuses the email channel.

```
channel email { provider: resend }
channel push  { provider: webpush }
route alerts.critical -> [push, sms] fallback email
```

> **Not built yet [2026-10-04]:** `channel` is reserved and not implemented;
> `route` is contextual and has no parser arm. The design text stands.
>
> **First piece built [2026-10-22]:** a routine's `notify <level> "<text>"`
> line emails the holders of an acl level on the rows a run wrote, through
> the deployment's mail channel. The text is a literal, so the payload is
> minimized by construction. Email only, no routing. See `howto-routines.md`,
> "Telling people by email".

**Outbound hooks are egress [settled]:** providers and routes are deployment owner
policy, but an outbound **HTTP hook destination carrying a space's data is a
per-space granted sink**. It needs space-owner consent naming the endpoint, with payload
class stated (minimized vs full), revocable, and every delivery logged. A
config file can route; only a grant can point a space's data at an outside
URL (mirror of the source principal: sources write in, sinks are consented
out).

---

## 9. Deployment model [settled]

One engine, Terraform-shaped, across **all** definition kinds:

1. `plan` diffs declared vs. deployed (shapes, ops, routines, clients, policies)
2. Classify each change:
   - **additive** (new entity, defaulted field, new op) → auto-applies
   - **transforming** (rename, type change, required-field addition) → requires a
     declared mapping; runs as a backfill job. Sealed-field transforms route
     through the worker (decrypt → transform → re-encrypt), never SQL.
   - **destructive** (drops) → explicit confirmation; the apply itself is a
     command with an actor. For BYO-DB spaces: **space owner** confirms drops;
     the domain developer owns everything else.
3. `apply` executes; every apply lands in the command log.

Defaulted-field additions split into four classes: **metadata-only** (`= null`, truly additive), **constant backfill** (deterministic literal, additive, bulk
write), **execution-time backfill** (`= today`, `= newid`, a *transforming* job
where each existing row's resolved value is computed and recorded, consistent with
resolved-value recording), and **mapping-required** (developer-supplied
transform). Required-field addition is always transforming. Each space's database carries its shape
version, and the same diff engine replays per target on connect (BYO-DB).

**The IaC boundary:** definitions, policies, and registrations are code-owned and
live in these files. Spaces, memberships, grants, and data are user-owned runtime
state and **never** appear in IaC. A grant declared in a config file would be
consent granted by a file, which the platform exists to prevent.

---

## 10. North star: out-of-the-box apps [settled as goal]

A self-hoster (or their LLM) writes ~two pages of `.ordo` (shapes, a few ops,
a routine or two), deploys, and has a working app: API, auth, permissions, audit,
notifications included. Two platform pieces this demands beyond what's designed:

1. **Generated UI** (later phase, direction pinned): rendered **on the fly from
   schema + op metadata**. A registered op knows its result shape, so
   table/detail/form surfaces derive at runtime; styling is layered per app.
   Custom UIs replace generated ones per-surface, not all-or-nothing.
2. **The grammar as a prompt**: the closed grammar published as an LLM system
   prompt with a budget of **two pages**: one of syntax, one of the semantic rules
   (values-dot-collections-match, condition scopes, null behavior, fan-out guard,
   let-scalar rule, sealed cost awareness) that plausible-variant emitters drift
   on. Rigidity rules exist precisely so generated definitions either parse or
   fail loudly, never drift semantically. **Written 2026-10-29:**
   `docs/ai-guide.md`, with the platform an app also needs (tenants, roles,
   sign-in, the SDK). Its examples are checked by
   `crates/ordo-cli/tests/ai_guide_check.rs`.

3. **Reserved UI annotations**: shapes and ops accept optional presentation
   metadata now (`label`, `desc`, `order`, `choices`, `danger` on destructive
   ops) so the generated-UI phase doesn't discover the kernel lacks channels for
   it. Inert until that phase.

## 11. Declined: inter-row computation (`extend`) [settled decision]

Window-family computation (running totals, prev/next, rank-as-value, moving
frames) is **deliberately outside the language**. A rigidity-compatible design
exists (`extend[keys] { rank, prev field, next field, runsum field, share
field }`, with fixed frames, no OVER/PARTITION ceremony, and tie-free by total order)
and is preserved here as the v2 candidate should third-party demand
materialize. Evidence for declining: a full finance application (statements,
anomaly detection, recurrence analysis) shipped with zero server-side windows.
The three blessed homes for neighbor-math:

1. **Client compute** over fetched rows (screens that show a window already
   hold it).
2. **Paged running balances**: one `reduce` for the opening value
   (`transactions(placed < $pageStart) reduce { opening: sum amount }`) +
   client accumulation within the page.
3. **Rule-materialized state** for durable derived series (balances, baselines,
   session tables), which is attributed, revertible, and queryable as plain fields.

## 12. Next artifact [settled]

Surface syntax pauses here. The next artifact is a **conformance suite**.
Each case has Ordo source, resolved types + join graph, generated
SQL/execution stages, expected rows, expected registration errors, and cost class.
Priority cases: optional refs, duplicate-ref pinning, the null truth table
(`null == null`, `null != null`), uniqueness on nullable fields incl. nullable
blind indexes, `conflicting_identity` (two unique keys, two rows), empty
`avg/min/max`, division at fixed scale 12 (host vs SQL parity, `/ 0`),
`implicit_rounding` rejection at typed boundaries, `allocate` remainder
distribution (largest-remainder, parts sum to total), int/money overflow,
`+ 1mo` on Jan 29–31, duration unit classes (`duration(ms)` ordering legal,
calendar-class ordering rejected, `1mo` into `duration(ms)` rejected at the
door), space-scoped uniqueness (two spaces, one null, both admitted),
minimal-connecting-subgraph inference over three sources (unique, ambiguous,
disconnected), chained fan-out accepted vs branched fan-out rejected,
create-exempt write guards (G10a: self-rooted exempt, parent-rooted evaluated),
`access … open` rejected as guard-bearing for guest exposure, acl level
combination by max across direct and group rows, `revoke` surviving-path report,
`memberCount` vs distinct headcount, grant pinning row-scope weakening (drop
`$caller`, add `unguarded`: identical field set, grant dies), return types of
date-part functions, Unicode/case behavior across equality, `lower`,
uniqueness, and blind indexes, outer-preserving multi-source + the
null-comparison trap (`or … is null`), sealed-aggregate cost classification,
`$trigger.ids` batches with multiple rows, sealed filter × sort/take,
pagination ties, CAS conflicts, mode-label derivation, take[] ranking, failed
addressed converges, retried idempotency keys, defaulted-field migrations,
trigger replay, named-query composition (scope flattening, post-take
filtering, cycle rejection, cost-class max through depth), let-join provenance (ref keys,
id keys, ambiguity), `matches` cost classification, median/percentile on
empty and even-sized sets, `cond` ordering/totality/type-agreement and
nested-in-aggregate forms, `least/greatest` null-ignoring, `mod` with
negative operands, `dow` numbering, `hash` stability across versions. When the suite starts, this doc splits into
three: language spec, execution spec, platform spec.

## 13. Open items

- ~~Op/registration wrapper~~ **[settled]**: one keyword for every
  registration, read or write: `op name($params) flags = <body>`. Flags
  between name and `=`: `reproducible` (forbids `since`), `unbounded`
  (cost-class acknowledgment), `unguarded` (guard bypass), `nopause`
  (answers while the tenant is paused, 27ar). The op's **kind
  is derived from the body** (any `set`/`del` ⇒ write; pipeline ⇒ read),
  never asserted; the compiler prints it on the artifact, only read ops are
  legal composition sources, and the REST layer enforces kind as the HTTP
  method (reads `GET`, set-writes `POST`, pure deletes `DELETE`; a wrong
  method fails; see api-engine-design). "Query" survives in prose for read
  ops; it is not syntax. **No authorization in syntax**: access is runtime
  platform data (see Access model).
- **Such-that bar `|`** was chosen over `?`; boolean OR is the word `or` (no
  symbol collision). Confirm no regrets after more writing.
- ~~Generated UI~~ **[settled elsewhere, 2026-09-07]**. This entry said
  *"surface inventory, theming, override mechanism (north star dependency;
  undesigned)"*, and it was out of date: `ui-gen.md` §1–§4 are `[settled]`
  and answer all three: descriptors instead of components (§1), a headless
  runtime with the skins layered above it (§2–§3), and per-surface ejection
  as the override mechanism (§4). The contradiction was recorded in
  `audit-board-coverage.md` #19 and is **resolved toward `ui-gen.md`**, the
  later and more specific document. This list keeps the row struck through
  and does not delete it, because a reader who followed the old claim needs to
  find out what happened to it. Two things genuinely remain open and they
  live in `ui-gen.md` §10, not here: the **app manifest's** block set and
  namespacing, and the **theming token inventory** (which `--ordo-*`
  variables are the contract). The B1 interface descriptor those descriptors
  project from is built: `ordo interface`, `GET /interface`.
- **Compliance pack**: the engineering draft is above; it needs counsel review before
  commercial tenants.
- ~~Text search beyond contains/starts~~ **[settled]**: not in the grammar.
  Search-box UX (fuzzy, ranking) runs client-side over the IndexedDB mirror,
  where plaintext legitimately lives, with zero key releases and instant results. Server
  full-text on open fields is a future capability; `matches` reserved.
