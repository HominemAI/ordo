# Ordo Traps

The ambush list. Every entry: the trap, why it exists (one line), the fix.
These are not bugs. Each is a deliberate rigidity rule that will surprise you
exactly once if you read this page first. Spec references are to
`definition-language.md` unless noted.

**Read the first three entries before you write anything.** They are the ones
that have actually broken real builds (including an LLM-authored app that
compiled into a data leak): where `today` may appear, how `let` is spelled, and
why a missing `access write` is a compile error rather than a quiet hole.

| Area | Entries |
|---|---|
| Time & parameters | `today`/`now`; `m` vs `mo`; duration unit classes |
| Pipelines | `let` + the fan-out guard (and comparing two aggregates); brackets; empty parens; nulls-last sorts; outer-join nulls; pagination |
| Numbers | division leaves `money`; no implicit rounding; `allocate` |
| Writes | `set` modes; converge touches only what you describe; write conditions are row-scoped; `check` sees the row's own columns only (ref walks are refused today) |
| Authorization | missing `access write`; the `acl` first row; **a denied write on an invisible row says `not_found`**; a routine's bound is its trigger; groups |
| Vocabulary | `distinct` vs `unique`; `and`/`or` parens; no `if`; `$caller` |

---

## `today` and `now` live in shape defaults, nowhere else

**Trap:** `at: date = today` is legal *as a field default*. `today` and `now` are
legal **nowhere else**, not in a query condition, not in a projection, and
**not on the right-hand side of a write**. All three of these fail to parse:

```
| at <= today - everyDays                    # ✗ not in conditions
set waterings { plant: $plant, at: today }   # ✗ not in a write RHS
op recent = waterings(at >= now - 7d)[at]    # ✗ `now` is not query vocabulary
```

The write-RHS case is the dangerous one: the right-hand side of a `set` *is* the
full expression grammar, so it looks like `today` should work there. It does
not. `today` is not part of the expression grammar. It is one of three closed
default expressions (`today`, `now`, `newid`, §3).

**Why:** execution-relative values are nondeterminism, and Ordo gives
nondeterminism exactly two visible homes: `since` in a condition, and a
**declared parameter** in the op's signature. A parameter is typed, appears in
the catalog, is recorded in the command log with its resolved value, and can be
pinned for replay. A bare `today` scattered through bodies is none of those.

**Fix:** take the value as a parameter and let the SDK fill it at call time.

```
op thirsty($today: date) =                   # ✓ declared, typed, visible
  plants[name, everyDays]  waterings[at]
  sort -waterings.at
  take[plants.id] 1
  | (at <= $today - everyDays) or at is null

op water($plant: id, $by: text, $today: date) =
  set waterings { plant: $plant, by: $by, at: $today }   # ✓ RHS is a param
```

**The honest wrinkle** (and it is a real one): a **converge never re-applies
defaults**, because converge touches only the fields you describe. So a
converge-or-create statement cannot stamp a server-side date at all. If the
row's timestamp must move, you must describe the field, which means passing
`$today`, which means the timestamp is caller-supplied. Today only SDK
convention keeps that honest; a server-stamped-on-converge form is an open
question, not a settled feature. Don't build an audit guarantee on it. The
command log's own timestamp is the attributable one.

(§2 Parameters, §3 Shapes/defaults, §6 Writes. This exact trap produced two
hard errors in a real generated app.)

## `distinct` vs `unique`

**Trap:** they look interchangeable; they live in different worlds. `distinct`
is query vocabulary. A bare step after a projection (SELECT DISTINCT) and an
aggregate (`distinct field` = count-distinct; `distinct (a, b)` = distinct
tuples). `unique` is a shape constraint. A uniqueness claim about stored data
(`unique (property, name)`), which also selects converge-or-create write mode.
**Why:** one English word, two grammatical homes. The grammar keeps them apart
so neither can be misread as the other.
**Fix:** in a shape, write `unique`; in a pipeline, write `distinct`. (§3 Refs,
§5 Aggregation.)
**Also:** `uniq` does not exist in this language, as a word, an abbreviation or
an alias. The step and the aggregate are both spelled `distinct`, in full. Anything
shorter is a parse error, on purpose (one spelling per concept).
**And `unique` is always space-scoped.** `unique (property, name)` means unique
*within one space*, never across the deployment. The shared DDL carries the
space in every uniqueness key, because two tenants choosing the same invoice
number is not a conflict and must never read as one. There is no deployment-wide
uniqueness spelling, deliberately: a constraint that spans tenants is a
cross-tenant information channel (you learn a value is taken elsewhere), so the
language does not offer one. Read every `unique` in these docs with "in this
space" appended. (§3 Refs/constraints.)

## `m` vs `mo`

**Trap:** `1m` is one **minute**; one **month** is `1mo`. A retention window of
`3m` is three minutes.
**Why:** spelling, never context, disambiguates. Duration units are exactly
`ms s m h d w mo y`.
**Fix:** write `mo` for months and re-read any duration literal containing `m`.
Note also that `30d != 1mo` is *true*, not an error: months are
calendar-relative, and `==`/`!=` are structural component equality.
(§2 Literals, §2 Duration semantics. `engine-plan.md` G10 previously carried an
engine pin here; the spec ruled it and the pin is withdrawn.)

## Durations only *order* within one unit class

**Trap:** `<`, `>`, `<=`, `>=` and `sort` on a duration are legal only when both
sides are provably the **same unit class**, either both calendar (`mo`, `y`) or
both elapsed (`ms s m h d w`). Mix them and it does not compile:

```
| everyDays > 1mo          # ✗ everyDays is elapsed-class, 1mo is calendar-class
sort retention             # ✗ if `retention` can hold either class
```

**Why:** a duration is a two-component value `(months, milliseconds)` and the
components never convert, because no conversion is correct. A month is not 30
days and never was. So there is no total order across the two classes, and the
language does not invent one (it would have to pick 30.44 or similar).
**Fix, and this is the part to memorize, because the class is part of the
type, declared, not inferred:**

| Declaration | Class | Ordering / `sort` |
|---|---|---|
| `duration(ms)` | **exact**, zero months component by construction | **legal** |
| `duration` (bare) | **calendar**, may carry months | **compile error** (`incomparable_duration`) |

So anything you will compare or sort must be declared `duration(ms)`:

```
entity plants { everyDays: duration(ms) = 7d }              # ✓ orderable
op addPlant($everyDays: duration(ms) <= 60d) = …            # ✓ bound is legal
op addPlant($everyDays: duration <= 60d) = …                # ✗ calendar class
```

Binding a months-bearing value to a `duration(ms)` parameter, or writing one
into a `duration(ms)` field, is a typed rejection **at the door**, class
`duration_unit_class`. The one place the check cannot be static, because the
value only exists at call time. Literals adopt the other operand's class where
they can, so `30d` is exact-class inside an exact-class comparison.

**`==` and `!=` stay total across both classes.** They are structural
component equality, defined on every pair, and never raise an error (`1y == 12mo`
is true; `1mo != 30d` is true). When you want "is this older than a month",
model the boundary as a **date** and compare dates (`at <= $today - 1mo` is date
arithmetic, which is well-defined), not as a duration ordering. (§2 Duration
semantics.)

## `sealed indexed` = equality only

**Trap:** a blind index gives you `==` (and `in`, and uniqueness) in the
database on a sealed field, and *nothing else*. Ranges, `contains`, sorting, and
aggregation over sealed fields run post-decrypt on the host, and sealed sort
keys with keyset pagination are prohibited outright.
**Why:** the index is an HMAC of the value. Equality survives hashing; order
does not, and no order-revealing scheme is ever used.
**What sealed costs you:**

| Operation            | Open field       | Sealed field                              |
|----------------------|------------------|-------------------------------------------|
| filter `==` / `in`   | `sql`            | `sql`-speed **only if `sealed indexed`**; else host-side |
| filter range / text  | `sql`            | host-side (bounded or unbounded)          |
| sort / paginate      | `sql`, keyset OK | host-side; **keyset pagination prohibited** |
| aggregate            | `sql`            | host-side; over a date range = `unbounded-host` (needs `unbounded`) |

**Fix:** keep fields you filter, sort, or aggregate on open; seal what you only
display; the compiler's cost class tells you when you got it wrong. (§3 Sealed
fields, §5 Cost classes.)

## Brackets mean what their position means

**Trap:** `[…]` after a source is a projection; `[…]` after `take` is the
grouped-limit bucket keys; anywhere else, brackets fail to parse, loudly,
never silently.
**Why:** position is scope. The same delimiter never has two readings in one
position.
**Fix:** projection directly after the source (`plants[name]`), keys directly
after take (`take[plants.id] 1`). (§5 Other steps.)

## Mixing `and` / `or` requires parens

**Trap:** `a and b or c` does not compile.
**Why:** there is no precedence table. A "plausible" reading must fail to
parse, never parse to something else.
**Fix:** `(a and b) or c` or `a and (b or c)`. Same-operator chains need no
parens (`a and b and c`). (§1 principle 2, §4 Boolean operators.)

## No `if`, no CASE: `cond` with a mandatory else

**Trap:** `if`/`CASE WHEN` vocabulary does not exist; reaching for it is a
parse error.
**Why:** one conditional construct, total by construction. No surprise nulls
from a missing ELSE.
**Fix:** `cond(c1 -> v1, c2 -> v2, velse)`. Arms evaluate in order, the first
match wins, the final bare value is the mandatory else (want null? write
null). All arm values one type. (§3 Expression functions.)

## No OFFSET, no page 47

**Trap:** there is no OFFSET and no random-access page number, in the language
or the protocol.
**Why:** offset pagination is unstable under writes: rows shift between pages
as data changes. Keyset cursors do not shift.
**Fix:** `take N` when the top-N *is* the result; otherwise page at call time
via the envelope's opaque keyset token (derived from the op's sort + id
tiebreak). "Jump to page 47" is a UX pattern Ordo does not make cheap, because
it cannot make it correct. (§5 Other steps; engine-plan G14.)

**And the caveat that used to be missing:** "no skips, no duplicates" holds only
while the **sort key doesn't move**. Keyset pagination remembers "the last row I
saw sorted like *this*"; if a write changes a row's sort-key value between page 1
and page 2, that row can appear twice or vanish. Sorting on `id`, on a
create-stamped date, or on any field your ops never update is stable. Sorting on
`status`, a running total, or `updatedAt` is not.

The compiler is meant to tell you which one you wrote: where it can prove no
registered write moves the sort key, the op's artifact says the cursor is stable;
where it cannot, the artifact is labelled **`cursor: drifting`**. This is a
disclosure and the build still passes. Read the label before you promise a client exactly-once paging;
when it says `drifting`, either sort on something immutable or make the page
idempotent on the consumer side.

**Read `cursor: stable` for exactly what it claims, because the scope is
narrower than the word.** Write-stability is judged against **authored** writes
only. The `set`/`del` statements a person wrote. The std family's derived
full-row converges are deliberately excluded, because `std.<entity>.update`
lists every declared field, so including it would make every non-id sort key
mutable everywhere and the label unreachable. So `cursor: stable` means *"no
authored write moves this key"*, not *"no command in the universe can"*, and if
you **grant `std.<entity>.update`** to callers, you have granted exactly the
thing the analysis excluded. The artifact records, per sort key, whether the std
family could move it; read that line before you grant the std family on a
paginated entity.

Two more cases that are always `drifting`, so you don't discover them at the
page boundary: any op with a writable sort key, and **composed post-take
pagination** (paginating your filter over a callee's `take 100` recomputes the
window per page, so it skips *and* duplicates, even when the outer keyset is
fine).
If exactly-once paging is a promise you make to somebody else, sort on `id` or a
create-stamped date, where the property holds by construction and no analysis is
involved. (§5 Determinism; glossary, `cursor: drifting`.)

## `set` mode is derived, not declared

**Trap:** the same keyword performs five different writes, decided statically
by what's in the braces:

| Statement shape                          | Mode                    |
|------------------------------------------|-------------------------|
| no identity in braces                    | CREATE                  |
| `id` present                             | CONVERGE (absent id = error, never resurrect) |
| `id` + `rev`                             | compare-and-set CONVERGE |
| covered unique key, no id                | CONVERGE-OR-CREATE      |
| parens on the entity (`set e(cond) {…}`) | BULK CONVERGE           |

**Why:** "make the described state true" has one spelling; the description
itself determines the mechanism.
**Fix:** you never guess. **The mode is printed on the artifact** (`creates` /
`converges` / `creates-or-converges` / `bulk`) and hover shows it; deploy diffs
flag mode drift. Creation-capable statements must cover all required fields.
(§6 Writes.)

## Converge touches only the fields you describe

**Trap:** every converging mode (CONVERGE, CAS-CONVERGE, CONVERGE-OR-CREATE,
BULK) writes **exactly the fields in the braces** and leaves every other field
of the row alone. That is the point, and it is also how a lifecycle silently
stops working. The classic:

> **Built since 2026-10-08 (checklist 12a).** `routine` compiles and runs with `trigger` inside it, and `each ->` bodies are built; see `howto-routines.md`.
```
routine thirstCheck in plantcare {
  trigger every day 09:00Z
  run($today: date):
    …
    each ->
      set alerts { key: "thirsty:" + plants.id,           # ✗ SILENT BUG
                   title: name + " needs water" }
}
```

Day one this creates the alert. You water the plant, some op sets
`status: "resolved"`, and from then on the routine converges the same row
forever **without ever touching `status`**, so the alert never re-raises. No
error, no warning, just a nag that stops nagging.

**Why:** defaults are applied on *create* only. A converge that re-applied
defaults would silently undo deliberate edits to fields you never mentioned,
which is strictly worse.

**Fix:** describe every field whose value the statement is asserting, including
the ones whose value is "back to the starting state":

```
      set alerts { key: "thirsty:" + plants.id,
                   title: name + " needs water",
                   status: "active" }                     # ✓ re-raising is stated
```

Rule of thumb: if you can name a field whose value would be *wrong* after a
second run, it belongs in the braces. (§6 Writes; see also the `today` entry.
This is the same rule seen from the other side.)

## Null: two-valued, reflexive equality, orderings always false

**Trap:** three-valued-logic instincts do not apply here. Quick card:

| Expression                     | Result |
|--------------------------------|--------|
| `null == null`                 | true   |
| `null == 5`                    | false  |
| `null != null`                 | false  |
| `null != 5`                    | true   |
| `null < 5` (any ordering op)   | false  |

`!=` is strict negation of `==`; `is null` / `is not null` are the only null
tests; **nulls sort last, always, regardless of direction**; `sum` of nothing
is 0, `count` of nothing is 0, `avg`/`min`/`max` of nothing are null.
**Why:** two-valued logic with one meaning per operator. There is no "unknown"
value.
**Fix:** trust the table. Never write `== null` expecting three-valued
behavior. You get reflexive equality, which is usually what you meant anyway. (§4 Null
semantics.)

## Nulls sort last, including the row you most wanted on top

**Trap:** "nulls sort last, always, regardless of direction" is easy to nod at
and expensive to forget, because the null row is usually the *most* interesting
one. Sorting by staleness is the canonical case:

```
sort at            # ✗ never-watered plants (at = null) land at the BOTTOM
```

You asked for "oldest watering first" to get the most neglected plants on top,
and the plants that were **never** watered (the most neglected of all) sort
below everything.

**Why:** a total order needs nulls somewhere, and one fixed position (last) that
does not flip with `-` is the only rule you never have to look up. "Nulls first
when ascending, last when descending" is a coin-flip you'd re-learn per query.
**Fix:** sort on an expression that says where nulls go, then on the field:

```
sort cond(at is null -> 0, 1), at        # ✓ nulls first, then oldest first
```

`cond` is the language's only conditional. Arms run in order, the first match
wins, and the final bare value is the mandatory else. Expression sort keys are
legal, and this is what they are for. (§4 Null semantics, §3 Expression functions,
§5 Other steps.)

## Outer-preserving joins keep the row and hand you a null

**Trap:** multi-source form preserves the one side: `plants[…] waterings[at]`
keeps plants with no waterings, with null child fields. Then two-valued logic
bites: `at <= $today - everyDays` is **false** for null, so the never-watered
plant, the thirstiest of all, silently drops out.
**Why:** keeping the parent row is correct (consistent with nullable-ref
dot-paths); null comparisons being false is correct; their composition is the
trap.
**Fix:** `(at <= $today - everyDays) or at is null`. The thirsty-plant
condition from the capstone. Whenever a parent may have no children, ask what
the null row should do. (§5 Multi-source join semantics.)
**And before you do arithmetic on it:** what `null * 2` means is not yet pinned
in the spec. Until it is, `coalesce` anything that came from the preserved side
of a join before you multiply, subtract, or compare it:
`coalesce(finished, 0) * 2 < readers`. Choose the coalesce value that makes the
*safe* thing happen when the child rows are missing.

## One entity once + one many-path per pipeline, and how `let` fixes it

**Trap:** an entity may appear only **once** as a source per pipeline, and a
pipeline may contain at most **one** many-path. Two independent many-sides
under a shared parent is a compile error, not a wrong answer.
**Why:** the star-aggregation double-count (accounts × transactions ×
receipts) is the classic silent corruption; Ordo makes it unrepresentable.

**Fix, and this is the part that used to be missing from the docs: `let`.**
The fix was named everywhere and its spelling was written down nowhere, so
authors abandoned features. The spelling follows.

`let` binds a **named pipeline** inside an op. The bound name is then an
ordinary source you may list, restrict, and project like an entity:

```
let <name> = <pipeline>
```

Say a club tracker wants, per club, both its book count and its member count.
Those are two independent many-sides hanging off `clubs`. The fan-out guard
rejects it outright:

```
op clubHealth =                                        # ✗ COMPILE ERROR
  clubs[name]  clubBooks[id]  memberships[id]
  group clubs.id, clubs.name { books: count, members: count }
```

Compute each side in its own `let`, then join both back:

```
op clubHealth =
  let bookCounts   = clubs[id]  clubBooks[bookId: id]     group clubs.id { books: count }
  let memberCounts = clubs[id]  memberships[memberId: id] group clubs.id { members: count }

  clubs[name]  bookCounts[books]  memberCounts[members]
  sort -books
```

Each `let` is its **own** pipeline with its own one many-path, so the guard is
satisfied three times over instead of violated once. The join back is inferred:
a `let`'s group-key columns remember where they came from (here `clubs.id`), and
match inference connects that provenance to the `clubs` source exactly as if it
were a declared ref (§5 "Let outputs as sources"). `match on <col>` disambiguates
if two lets could attach in more than one way.

**This trap most often shows up in one shape, and a worked recipe covers it.**
"Fewer than half the members finished", "more blocked than open", "reached
quorum", "refunds over 10% of charges" are all **two aggregates, grouped
independently, then compared**, and all of them hit this trap. Two builds
abandoned their app's central feature because they could not find the spelling.
The full recipe is `modeling-guidance.md` §C7. It covers both `let`s, the
provenance join, the null that a missing group hands you, and why you compare by
multiplying instead of dividing. The skeleton:

```
op stalledClubs =
  let memberCount = clubs[id]  memberships[id]
                    group clubs.id { members: count }

  let doneCount   = clubs[id]  readingProgress(finished == true)[id]
                    group clubs.id { done: count }

  clubs[name]  memberCount[members]  doneCount[done]
  | coalesce(done, 0) * 2 < coalesce(members, 0)     # ← both coalesces matter
  sort name
```

Note the two `coalesce`s: a club nobody started has **no row** in `doneCount`,
the outer-preserving join hands you `null`, and `null * 2 < members` is
**false**, so the club you most wanted to report on drops out silently. And
note the multiplication: `done / members < 0.5` would divide, which yields
`decimal` and is a typed error on an empty club.

Two more things worth knowing about `let`:

- **A `let` that ends in `reduce` is a scalar**, referenced by dot:
  ```
  op bigSpends =
    let baseline = transactions(amount < 0) reduce { a: avg amount }

    transactions(amount < 3 * baseline.a)[date, amount, payee.name]
    sort amount
  ```
  A `let` that does *not* end in `reduce` can only be used as a source.
  Referencing it as a scalar is a compile error.
- **One `let`, many uses.** A `let` may be referenced by any number of later
  pipelines in the same op.

Other escapes from the same trap: `match any` / `match none` when you only need
*existence* (no aggregate, no fan-out), or write two ops. Two-role access to one
entity goes through distinct refs and dot-paths
(`transfers.from_account.name`). (§5 Fan-out guard, Let outputs as sources,
Determinism.)

**Settled, and worth knowing so you do not use `let` where none is needed:
the guard counts BRANCHES, not hops.** A chain that keeps
descending the same line, `clubs ← clubBooks ← progress`, is **one**
many-path, not three, and it compiles. Depth is free; branching is not.
Precisely: the guard fails when one entity has two or more child many-branches,
or when two independent many-chains are joined. The reason is the one above.
Two independent many-sides multiply into a Cartesian product and an aggregate
over it counts each row once per row of the other side, whereas each step of a
chain refines the grain of the last and `sum` counts each deepest row exactly
once.

## Division leaves `money`, and nothing rounds behind your back

**Trap:** `+`, `-` and `*` on money stay money and stay exact. **`/` does
not.** Division (and `avg`) yields **`decimal` at a fixed fractional scale of
12**, and there is **no implicit rounding anywhere**, so the moment you try to
store or project that decimal as `money`, the compiler stops you:

```
op setPerHead($budget: id, $rev: int, $total: money, $heads: int) =
  set budgets { id: $budget, rev: $rev, perHead: $total / $heads }
                                        # ✗ compile error: implicit_rounding
```

**Why:** the alternative is a compiler that silently drops digits at the
boundary where your cents are decided. It refuses, so the truncation is a line
of code a reviewer and an auditor can both find. What the language *does* pin
is the **mode**: `round` is half-up, away from zero, everywhere. The same
tie-break division already uses when it reduces scale, so the whole numeric
surface has one tie-breaking behaviour (M4-002). An application whose jurisdiction requires half-even expresses
it from the primitives, in its own code. (The fixed scale of 12 is the other
half of the story: same number everywhere, so an expression means the same thing
whether the stage ran in SQL or host-side.)

**Fix:** say how it rounds. `round(x, n)`, `floor`, or `ceil`. One of them,
explicitly, at the boundary:

```
op setPerHead($budget: id, $rev: int, $total: money, $heads: int) =
  set budgets { id: $budget, rev: $rev,
                perHead: round($total / $heads, 2) }      # ✓ stated
```

**Comparisons are not typed slots**, so they need no conversion:
`spent > $total / 3` widens both sides and compares exactly at the pinned scale.
Nothing is being stored, so nothing needs a mode. (One mechanical exception,
and it is not a decision about your money: scale reduction *at the division
itself* is half-up at digit 13, matching what Postgres `NUMERIC` division does
natively so both venues agree digit-for-digit. It never decides how many cents
land in a field.)

**When the shares must add up, do not divide. Use `allocate`.** Rounding
three shares of `100.00` independently gives `33.33 × 3 = 99.99`, and the cent
is gone. `allocate(total, weights)` is the money-split
primitive: it splits `total` in proportion to the weights, each part is the
floor of its exact share, the remaining minor units go by largest-remainder
(ties by weight order, so both venues agree), and the parts **sum to `total` by
construction**. `allocate(100.00, (1, 1, 1))` is 33.34 / 33.33 / 33.33, never
99.99.

```
op splitBill($bill: id, $shares: list of { member: id, weight: int },
             $total: money) =
  set billShares each $shares {
    bill:   $bill,
    member: item.member,
    amount: allocate($total, item.weight)   # ✓ THIS element's part; the
  }                                         #   function reads item.weight
                                            #   across the whole list
```

**You decide who gets the odd penny.** An optional
third argument names it, `allocate($total, item.weight, last)`, from the
closed set `largestRemainder` (the default, and what accounting texts assume) ·
`first` · `last` · `largestShare`. Every mode floors the shares and hands the
leftover minor units out one apiece; the mode picks only who is first in line,
so the parts still sum exactly and no part is ever more than one minor unit
from its exact share. A misspelled mode is a **compile error**. It never falls
back quietly to the default.

The one you will look for and not find is **rotation**, giving the odd penny to
a different party each invoice so it evens out. It is not a mode and will not
become one: it has to remember what it did last time, and an expression function
that answers differently on a retry breaks determinism. Keep the counter in your
own data and pass the weights in the order it says; then the rotation is visible
to an auditor and is not hidden in the compiler.

**Where `allocate` is legal is narrow, so read this before you reach for it:**
inside an `each` emission over a **list parameter**, with the weight written as
the element field (`item.weight`). The whole weight vector is then a parameter,
known at the door before planning, so the split is bounded arithmetic. It is
not neighbor-math over query results. Anywhere else it is `invalid_allocate`. Splitting by
*query-derived* weights (allocate across the rows a pipeline produced) is not
expressible, for the same reason running totals are not: the caller passes the
weights it already fetched, or the split is materialized by a rule.
(`each` over a list parameter is itself **[lean]**. The allocation semantics
are settled; the spelling moves if `each` does.)

Two more things about division worth knowing before they find you:

- **`/ 0` is a typed runtime error**, uniform with `mod(a, 0)`. There is no
  null-on-divide-by-zero rule. Guard the denominator, or restructure so
  there isn't one, which is usually the better move.
- **Comparisons rarely need division at all.** `done / members < 0.5` is
  division, a possible `/ 0`, and a decimal; `done * 2 < members` is exact
  integer arithmetic that cannot fail. Cross-multiply. (§2 Numeric semantics,
  Division; §3 Expression functions.)

## `$caller` is never client-suppliable and never expands a group

**Trap:** `$caller` looks like a parameter; it is not one you can pass. A
request carrying it is rejected.
**Why:** the runtime binds it from the session. It is the one reserved
parameter, so caller-scoped visibility is written in the op body and cannot be
forged from outside.
**Fix:** use it in bodies (`expenses(member == $caller)`); never put it in
a call. (§2 Parameters.)

**The second half of this trap is the expensive one: `== $caller` is row
equality, and it never expands groups.** `$caller` binds to the caller's own
`members` row in this space. If a task is assigned to the group `designTeam`
and you are in that group, `tasks(assignee == $caller)` returns **nothing** for
it. The assignee is the group's row, you are your own row, and the two are not
equal. Group resolution happens in the **access layer only**: guards, acl rows,
and grants expand groups; op bodies do not.

```
op myTasks =
  tasks(assignee == $caller and done == false)[title]   # ✓ tasks assigned to ME
                                                        # ✗ NOT tasks assigned to
                                                        #   a team I belong to
```

The same asymmetry has a reporting consequence: **`count` over an acl link
entity counts a group as one row**, so a quorum computed that way disagrees with
anyone who counts heads. The fix is in the language. Every `members` row
carries a read-only `memberCount` (1 for a person, the group's size for a
group):

```
projectsAccess(project == $p)[member]
reduce { seats: sum member.memberCount }        # ✓ counts humans, not rows
```

Stated limitation, worth knowing before you bill anyone for it: that is a
**cheap upper bound**, because someone reachable both directly and through a
group is counted once per path. Exact distinct headcount is a two-source
pipeline over `groupMembers` with a `distinct` step (§8, Groups).
(§2 `$caller`, §8 Access model.)

## `access read` without `access write` is a **compile error**

**Trap:** you guard the reads, ship, and feel safe. The schema *reads* as
guarded and is wide open to writes, because
`access read` says nothing about writing and std still derives
`create`/`update`/`delete` for the entity.

```
entity clubs {
  name: text
  access read  clubMembers(member == $caller)     # ✗ WON'T COMPILE, and
}                                                 #   good, because the old
                                                  #   behaviour was worse
```

**Why:** this is not hypothetical. A generated app declared `access read` on
three of six entities and `access write` on none; a reviewer skimming it saw an
access line on every guarded entity and concluded it was locked down. The verdict
was *"it looks secure and isn't."* Open-by-omission is the one failure mode that
survives review, so the language refuses to let omission mean anything.
**Fix:** say what you mean, in both directions. Every guarded entity spells both
verbs:

```
entity clubs {
  name: text
  access read   clubMembers(member == $caller)
  access write  clubMembers(member == $caller and canEdit)
}
```

If writes really are open (a public sign-up sheet, a sensor drop-box) that is a
legitimate design, and it must be **written**, not implied:

```
entity signups {
  email: text
  access read   organizers(member == $caller)
  access write  open                    # deliberate: anyone may add a signup
}
```

**The word is `open`, and `access write true` does not parse.** They look
interchangeable and are not: `true` is the boolean literal, and a guard position
that accepted it would also have to accept `false`, `not false`, and every other
"plausible" bool, at which point the compiler can no longer tell a *decision to
open* from an *expression that happens to be true today*, and the guest-exposure
check (which must refuse to hand an unguarded surface to an outsider) has nothing
reliable to look for. `open` is a keyword in exactly one position and means
exactly one thing.

`access write open` is greppable, reviewable, and shows up in the diff the day
someone adds it. Silence never grants anything. (§3 Access guards.)

## An `access` clause on a child replaces the inherited guard entirely

**Trap:** an entity in an `acl` root's owned subtree inherits that root's guards.
The moment you write **any** `access` clause on the child, the inherited guard is
gone. It is replaced, not merged and not intersected. There is no composition algebra.
**Why:** guard composition across an ownership chain is the feature that makes
every row-level-security system unauditable; "the clause you can see is the
clause that runs" is worth more than convenience.
**Fix:** when you override a child, restate the whole rule. Both verbs, the
inherited condition included if you still want it. Reading the child's shape must
be enough to know what protects it. And **link entities never inherit at all**
(they have two owners, so there is no single root to inherit from). A link
entity always declares its own guards. (§3 `acl`, Access guards.)

## A denied write on a row you cannot see says `not_found`, not `access_guard`

**Trap:** you write to a row by id, get a `404`, and go looking for a bug in
your id handling. The row exists, but you cannot see it.

**Why:** if a forbidden row answered differently from an absent row, anyone
holding an id could tell *exists but forbidden* from *never existed*, over the
whole deployment, without ever reading a byte of the row. Collect ids from a
share link, a stale export, a URL somebody pasted, and you have an enumeration
oracle. So on a **guarded** entity the two answers are made identical: same
status, same code, same message.

| You addressed a row that… | Answer |
|---|---|
| does not exist | `not_found` (404) |
| exists, is **guarded**, and is **outside your read view** | `not_found` (404), **byte-identical** to the line above |
| exists, you **may read** it, you may not write it | `access_guard` (403). The informative case |
| …and every refusal at the **door** (unknown op, forbidden op, bad token, unmet step-up) | `access_denied` (403), identically |

**The exception is unguarded entities.** An entity with no `access` clause and
no inherited `acl` has no read view to be outside of, so there `not_found`
means only "absent". Non-enumerability is a property of *guarded* data only.

**Fix:** stop treating `404` as "my id is wrong". On a guarded entity it means
"absent **or** not yours", and the disambiguation you want is not on the wire
by design. It is in the space's security-event stream, which records
`not_found`-because-guard distinctly from `not_found`-because-absent. When you
are debugging, read the log, not the status code. When you are writing client
code, do not build a UI that distinguishes the two, because it cannot.

## `check` sees the row's own columns, and it cannot count

**Trap:** `check` looks like a general-purpose invariant hook, so people reach
for `check count(tickets) <= 10` or `check sum(lines.amount) == total`. Neither
compiles.

**Why:** `check` compiles to a constraint the database enforces **inside the
write's transaction**. What it can see is the row being written and **the rows
that row points at**, outgoing refs, many→one, the same direction dot-paths
always go, at any depth:

```
entity tickets {
  customer -> customers
  assignee -> members
  check assignee.org == customer.org      # ✓ two outgoing refs, one comparison
}
```

**Today this form is refused**, `check_beyond_row`: a database CHECK cannot
read other rows, so a ref walk needs triggers on every entity the walk passes
through, and that is not built (`crates/ordo-schema/src/check.rs`). A `check`
over the row's own columns (`check used <= seats`) is built.

Aggregating, counting, or testing existence over a **collection** is a question
about *other rows*, not about this row's neighbours. That is a different tier
and a different cost, and letting it hide inside a per-row constraint is how
write latency becomes unpredictable.

**Fix:** keep a counter and check the counter. The counter is this row's own
column, so it is row-scoped:

```
entity teams {
  seats: int
  used:  int = 0
  check used <= seats            # ✓ row-scoped
}
op join($t: id, $m: id) =
  set teams   { id: $t, used: used + 1 }
  set members { … }              # one atomic group: check fires, group aborts,
                                 #   the seat never oversells
```

That is also why there are no `require`-style op preconditions: the invariant
lives on the shape, where every write path meets it, instead of on the one op
whose author remembered.

## A routine's bound is its **trigger**, not its principal

**Trap:** you assume one rule for routines and guards. There isn't one. There
are two, and **which one applies depends on how the routine was woken**:

| Woken by | Bound |
|---|---|
| **clock**, `trigger every day 02:00Z` | **guards pass.** Nobody outside chose the moment, so the run sees every row its scope manifest allows. |
| **listener**, `trigger when finance.transactions.ingested` | the run may read and write **only rows reachable from `$trigger.ids`**. Every other row is invisible, exactly as a guard would make it. |

**Why:** the old rationale, "a routine holds no members row, so guards would
evaluate false and every rule would silently do nothing", was true about the
clock case and dangerously wrong about the listener case. Listeners fire on
`ingested` (a source principal such as a webhook or a sensor) and on `fed` (copies
arriving from another space). Those are **outsiders**, and they choose both the
moment a guard-free process runs and, through `$trigger.ids`, the rows it starts
from. A globally guard-exempt listener is therefore a **confused deputy**: it
does work an outsider steered, on rows the outsider could never touch. So the
exemption belongs to the clock, not to the `routine:` principal.

**Fix:** write listener bodies against the rows you were handed, and mean it. The run is already bounded to them:

```
routine securityWatch in estate {
  trigger when estate.events.created
  run():
    events(kind == "motion")[id, at, device.location.name]
    each -> set alerts { … }                 # ✓ scoped to what actually changed
}
```

A listener that must legitimately range wider says so with `unguarded` on the
`run` header. The same greppable flag ops use, in the same slot
(`run($today: date) unguarded unbounded:`). If you reach for it on a routine
woken by ingested or fed data, re-read the paragraph above first.

**Unchanged and orthogonal: the derived scope manifest.** It bounds *which
entities and fields* a routine may touch, derived from the body, never
hand-written, disclosed at deploy time, pinned into every grant, and it was
never a **row** bound. Conflating "which fields" with "which rows" is what
produced the old rule. Keep bodies narrow: a routine that projects an entire
entity has granted itself that entity. Every write is attributed
`routine:<name>` in the command log.

And when you audit "who can see this data", the answer still has three parts:
profiles, guards, and the domain's routines. (§8 Routines, §7 Key release.)

## `acl` derives everything except the first row, so the platform grants it

**Trap:** `acl (read, write, admin)` derives a link entity, guards, and
grant/revoke ops gated at `admin`. A brand-new row has **zero** rows in that link
entity. Read literally, nobody, including the person who just created the row, 
holds `admin` on it, so nobody can grant the first permission and the tenant can
never be started. Two independent authors hit exactly this and both shipped an
app that could not be used.
**Why:** the derivation is per-row and honest; the bootstrap is the one thing a
per-row rule cannot derive from data that doesn't exist yet.
**Fix. Three platform rules you don't write and should know:**

1. **A creating write on an acl-root entity is exempt from the derived write
   guard.** It has to be: the guard walks to an access row that cannot exist
   before the row does, so without the exemption the very first `set` is
   rejected and nothing is ever creatable. The exemption is narrow. It covers
   the *creating* write on the *acl-root* entity, nothing else.
2. **The creating principal gets the top-level access row, in the same command
   group as the row itself.** Create a club, and you are its admin, atomically.
   One group, one revert, disclosed in the shape summary and recorded in the log
   as an ordinary grant command. Two limits on who "the creator" can be:
   - it must be a space **member**. A guest holds a members row (with a null
     role) so acl rows can name them, but a guest cannot mint an acl root and
     become its admin;
   - rows created by a **routine, source, or the platform** get **no** access
     row (nobody is "the creator"), which is why seeding data means an
     explicit `set …Access` write afterwards.
3. **A space owner can always administer any acl row in their space.** This is the
   escape hatch for every "everyone with admin left the company" story, and it is
   the reason an acl-bearing entity can never lock its own space out. It is
   checked **at the door**, where the space is already bound. One decision on
   one surface, not a special case buried in the row walk. It is a power, so it
   is logged like one.

**And there is no `grant` statement.** Prose in earlier drafts told you to
"grant explicitly"; no such keyword exists and none is coming. The language has
two write verbs, `set` and `del`. The derived link entity is an ordinary entity,
so seeding somebody is an ordinary write, guarded in the ordinary way:

```
op assignClub($club: id, $rev: int, $owner: id) =
  set clubs       { id: $club, rev: $rev, status: "active" }
  set clubsAccess { club: $club, member: $owner, level: "admin" }
# one op = one atomic command group: the club never changes hands without the
# access row landing with it. The derived grantClubsAccess op (gated at `admin`)
# is the surface for ordinary permission management.
```

**Creating a row and seeding access for it in the same op does work, and the
spelling is `let`:** a `set` whose derived mode is CREATE may be bound, and the
binding is a row descriptor (`id`, `rev`, the fields that statement wrote)
usable in later statements of the same op.

```
op startClub($name: text, $coHost: id) =
  let club = set clubs { name: $name }          # CREATE mode, so it binds
  set clubsAccess { club: club.id, member: $coHost, level: "admin" }
```

The trap that remains is expecting `let` to bind the *other* write modes. It
does not: `let x = set clubs { id: $c, rev: $r, … }` is a compile error
(`unbindable_write`), and so is `let` on a converge or a bulk write. CONVERGE
and CAS address a row whose id the caller already holds. There is nothing to
learn, and a bulk write affects an unknown number of rows, so there is no
single `.id` to name. The binding is also not usable in the statement that
produced it, nor in that statement's condition: statements in a command group
execute in order, and `let` adds no new ordering question.

Design consequence: don't write guards that make *creation*
impossible. A write guard is evaluated against the **would-be row**, so
`access write club.clubMembers(member == $caller)` on a membership entity means
you must already be a member to become one. Nobody can ever join. Guard the
parent's admin ops instead, and let the derived grant machinery add members.
(§3 `acl`.)

## Groups are principals too

**Trap:** an ACL row or a guard walk looks like it must name a person, so people
model teams by writing one ACL row per human and re-writing them all whenever the
team changes.
**Why:** a **group** is a named collection of principals, and it is usable
anywhere a principal is: in an acl link row, on the far side of a guard's
relationship walk, in a grant. Membership of the group is ordinary runtime state,
so changing who is in "the design team" is one command, not N.
**Fix:** when the sentence you are modeling contains a plural noun ("the
reviewers", "contractors", "the finance team"), grant to a group and manage the
group. Three limits to keep in mind:

- groups are *collections of principals*, not a role hierarchy. The four space
  roles (owner / manager / editor / viewer) are still fixed, and no group
  inherits another group's grants;
- a group is scoped to the space that owns it;
- **groups expand in the access layer and nowhere else.** Guards, acl rows and
  grants resolve group membership; `== $caller` in an op body does not, and
  `count` over an acl link entity counts a group as one row (use
  `sum member.memberCount` to count humans). See the `$caller` entry. This
  catches people who model "my team's work" as a body condition;
- **groups cannot contain groups** in v1. Nesting is rejected at write time,
  which also keeps one expansion sufficient everywhere.

See `modeling-guidance.md` §D for when a group beats a hand-rolled membership
entity. (§8 Access model.)

## Write conditions are row-scoped, no existence tests

**Trap:** a bulk write's condition sees the row and nothing else: no `match
none`, no subqueries, no "delete orphans where no referrer exists" in one
statement.
**Why:** the condition is the write's entire specification, and it must be
row-local to be statically priced and safely revertible.
**Fix:** cross-row invariants are `check` constraints or maintained counters;
cleanup is a rule (a query with `match none` feeding `each -> del …`,
attributed and revertible); authorization is guards. (§6 safety rule 7, Blessed
patterns.)

## Empty parens never parse, empty brackets now do

**Trap:** `entity()` is a parse error everywhere. Omit the parens when there is
nothing to pass; "nothing here" is spelled by absence, not by an empty pair.

```
op openOrders = workOrders()[opened]        # ✗ empty parens never parse
op openOrders = workOrders[opened]          # ✓
```

**Empty *brackets* are a different story, and the rule got simpler.**
`entity[]`, projecting nothing, is now legal generally, not just on a bridge
source. All three of these parse:

```
units[] leases(status == "active")[start]           # bridge: listed for the join
let o = customers[] orders[] group customers.id { … }   # grouped anchor
plants reduce { n: count }                          # fully bare source
```

**Why:** there was never a second reading available for `[]`, so the old
projection-of-nothing error rejected programs that could only have meant one
thing. It is retired.
**Fix:** nothing to avoid any more, but note that a bare or `[]` source
contributes only its join path and its group key, so if you want a field in the
output, project it. And unqualified field names are legal only when unique
across sources; otherwise qualify (`waterings.at`). (§5 Match, Named-query
composition.)
