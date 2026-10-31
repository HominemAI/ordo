# Build an Ordo app, annotated

Two build-alongs, in order. **Both are the tutorial.** Please do not stop
after the first one.

- **Part 1, `plantcare`** (sections 0–7): the capstone app from
  `syntax-examples.md` §9, a plant-watering tracker. Shapes, queries, writes,
  a routine, an app manifest, about two pages of `.ordo`, ending in a
  working app surface: API, generated UI, auth, audit, and a daily nag, none
  of which you write. Plantcare has exactly one user, which keeps the
  language in the foreground.
- **Part 2, `teamtrack`** (sections 8–14): a smaller app with *other people
  in it*. `acl`, guards and how they're inherited, both access verbs, link
  entities, `$caller`, the day-one bootstrap, groups, and guests. This part
  exists because the jump from one user to several is where real Ordo apps,
  human- and machine-written, have actually gone wrong.

Written for a mid-level web developer arriving cold. Every concept is
explained the first time it appears, with a pointer to the section of
`definition-language.md` (cited below as **§spec-N**) that owns it. Companion
reading, referenced by name where relevant: `std-library.md`, `ui-gen.md`,
`implementation.md`, `modeling-guidance.md`, plus `glossary.md` (terms) and
`traps.md` (sharp edges).

---

# Part 1: `plantcare`, the language

---

## 0. What you're building and an honest status note

Plantcare tracks plants and when they were watered. It answers one question
well, *which plants are thirsty right now?*, logs every watering, and nags
you daily about the ones you've neglected.

**Status, honestly (written before the engine existed; the update below is current):** there was no installable engine. Ordo today is a
settled language and platform specification; `ordo plan` / `ordo apply` /
`ordo dev` describe designed behavior (§spec-9, `implementation.md`), not a
binary you can download. What this tutorial gives you is the real thing one
level up: every code block below is written to compile against the current
spec, current `op` wrapper, current keywords, current semantics, and can be
checked against it line by line. The machine-checkable version of that claim
is the conformance suite, which is the project's next artifact (§spec-12).

**Update:** the engine is built now. `ordo check` and `ordo dev` run the
blocks below, and `quickstart.md` gets you running in minutes. Two things
here are specified and not built yet, and `ordo check` refuses them: a
`today` default (§1.2) and the `app` manifest (§6, checklist 19h).
Reading `$trigger` in a routine body (§13) is not built either.

The blocks below show what building on Ordo is like. They are precise
enough to serve as test cases for the engine.

---

## 1. Shapes first

Everything in Ordo starts from **shapes**, entity declarations (§spec-3).
Plantcare needs three.

### 1.1 `plants`

```
domain plantcare @ 0.1.0

entity plants {
  name:      text
  room:      text?
  everyDays: duration(ms) = 7d
  notes:     text = ""     sealed
}
```

Four fields, and each one demonstrates a rule.

**Every field is required, defaulted, or nullable, visibly (§spec-1 rule 5,
§spec-3).** `name: text` is required: you cannot create a plant without one.
`room: text?` is nullable. The `?` is sugar for `= null`, so a plant with no
room is a legal, stated possibility. `everyDays: duration(ms) = 7d` is defaulted.
There is no fourth state, and you can read a field's obligation off the
declaration without looking anywhere else.

**Defaults come from a closed set (§spec-3).** Literals plus `today`, `now`,
`newid`. No arbitrary computation. When a row is created, the *resolved*
value (not the word "default") is recorded in the command log, so history
never depends on knowing what a default used to be.

**`duration` is a first-class type (§spec-2).** `7d` is seven days, written
with the closed unit set `ms s m h d w mo y`. Watering cadence is a duration,
not an int named `days`, which is what lets query code later say
`$today - everyDays` and have the arithmetic be typed.

**And the `(ms)` is doing real work.** A duration has two components, months
and milliseconds, and they never convert into each other, because no
conversion is correct (a month is not 30 days and never was). So the **unit
class is part of the type**: `duration(ms)` is the *exact* class, whose values
carry no months at all, and bare `duration` is the *calendar* class, which may.
Only the exact class can be ordered or sorted; ordering a calendar-class
duration is a compile error (`incomparable_duration`). `everyDays` is declared
`duration(ms)` because section 2.5's thirst test orders it
(`at <= $today - everyDays`). `==` and `!=` work across both classes and always will.

**`sealed` puts a field inside the space's encrypted payload (§spec-3).**
Postgres holds ciphertext for `notes`; the database can never be asked what a
note says. The compiler knows the consequences, sealed fields aren't
SQL-filterable, and any op touching them says so in its manifest.

**What you did not write:** a primary key. Every entity implicitly carries
`id` (a typed, server-minted id like `plnt_01J8...`) and `rev` (a version
counter), undeclarable, always present (§spec-3 "Implicit identity").

### 1.2 `waterings`

```
entity waterings {
  plant -> plants   owned
  at:    date = today
  by:    text = ""
}
```

**`->` declares a ref. The relationship, once (§spec-3 "Refs").** `plant ->
plants` means each watering points at exactly one plant. From this one line
the compiler derives the foreign key, the index, and query traversal:
`plant.name` in any query walks the ref. You do not write a JOIN...ON
clause, because the schema already knows the join (§spec-1 rule 3).

**`owned` is a separate, opt-in claim (§spec-3 "Ownership").** The arrow
gives the mechanics. `owned` says a watering is *part of* its plant and is
meaningless without exactly this plant. The existential test: if a second
parent is even imaginable, it's association, not ownership. Ownership drives
two behaviors: deleting a plant cascades to its waterings (part-of dies with
the whole), and `whole plants($id)` includes them as the plant's body. An
un-`owned` ref would instead *restrict* deletes, refuse, listing referrers.

**The parent declares nothing.** There is no `waterings: [...]` collection
field on `plants`; the parent-side view is derived and displayed by the
compiler's shape summary (`plants — owns: waterings`), never written. One
direction stored, both directions readable, nothing to drift.

### 1.3 `alerts`

The daily routine (section 4) needs somewhere to put its nags. That is an
ordinary entity. Alerts are rows.

```
entity alerts {
  key:      text unique
  severity: text = "warning"
  title:    text
  status:   text = "active"
  raised:   date = today
}
```

**`unique` is a claim about data (§spec-3).** Rows are
identified by their implicit `id`; `unique` on `key` says no two alerts may
share a key. Violations are a typed rejection (`duplicate`). The reason
`key` exists at all is a write trick you'll meet in section 3: a unique key
makes "create or update" a single idempotent statement.

**That's the whole schema.** Three entities, fourteen lines. You have not
written keys, indexes for the refs, migrations, or an audit table. All
derived ("the server owns mechanics", §spec-1 rule 6).

---

## 2. Queries: from one line to "which plants are thirsty?"

A query is a pipeline (§spec-5). The core form puts source, restriction, and
selection in one breath:

```
entity(conditions)[projection]
```

then steps follow, one per line, reading top to bottom. Each line transforms
the stream the previous line produced.

### 2.1 One line

```
plants(room == "Kitchen")[name, everyDays]
```

Rows of `plants`, restricted in the source parens, projected in the
brackets.

### 2.2 Walk a ref

```
waterings(at since 30d)[at, by, plant.name]
```

**Dot-paths follow the arrow and read the field (§spec-5).** `plant.name`
walks the watering's ref and reads the plant's name; the join is compiled
invisibly. The one-direction rule: dot-paths only walk toward "exactly one"
(many→one). `plants.waterings.at` is not a value and does not parse,
*values dot; collections match.*

**`since` is the language's one nondeterminism (§spec-2, §spec-4).**
`at since 30d` means "within the trailing 30 days of execution time". The
same query returns different rows tomorrow. That's correct for dashboards;
where reproducibility matters, `since` is disallowed and you pass explicit
bounds (you'll see the `reproducible` flag in section 5).

### 2.3 Sort and take: the watering history

```
waterings(plant == $plant)[at, by]
sort -at
take 30
```

**`$plant` is a parameter (§spec-2).** Parameters are always `$`-prefixed,
fields never are, so the two can't collide. Types and bounds are declared
where the query is registered (section 5) and enforced at the door, before
any planning.

`sort -at` is descending; `take 30` means *the answer is 30 rows*, take is
query semantics, not pagination. Pagination is a call-time envelope concern
on any op, keyset cursors derived from the sort (§spec-5 "Other steps"). And
determinism is total: the implicit id is auto-appended to every sort as a
tiebreaker, so ordering never has ties and cursors never skip.

### 2.4 Two sources and a group

```
plants[name]  waterings[at]
group plants.id, plants.name { n: count, last: max at }
| n == 0
```

Several new things at once, so slowly:

**Listing two sources joins them by declared refs, match is inferred
(§spec-5 "Match").** `waterings.plant` is the only connection between these
entities, so the compiler inserts it. Had there been two possible paths, the
compile error would enumerate them and you'd list a bridge; ambiguity
surfaces at compile time, where it belongs.

**The join preserves the one side (§spec-5 "Multi-source join semantics").**
A plant with no waterings stays in the stream with `at = null`. Remember
this; it is the hinge of the thirsty query.

**`group` buckets the stream; braces hold aggregates only.** Inside `{ }`
only aggregates (`count sum avg min max` and friends) and renames of
aggregates are legal. A bare per-row field there is a compile error. Per-row
values you want to keep ride the group key, which is why the key is
`plants.id, plants.name`: grouping by id means two plants that share a name
never merge; carrying `name` alongside makes it displayable.

**The bar after a group filters buckets.** `| n == 0` keeps plants with zero
waterings. The bar filters by position, with no keyword
(§spec-1 rule 4: *position is scope*). A condition's meaning comes from where
it sits: source parens scope to that entity's rows, a pipeline bar scopes to
the stream as it exists at that line, parens inside an aggregate scope to
the bucket. The grammar is the same in all three positions.

So this query reads: plants joined to their waterings, bucketed per plant,
keep the buckets with no waterings. The result is the never-watered plants.
It works only because the join preserved them.

### 2.5 The thirsty query

The app's centerpiece: for each plant, look at its *latest* watering and
decide whether it's overdue.

```
plants[name, room, everyDays]  waterings[at]
sort -waterings.at
take[plants.id] 1
| (at <= $today - everyDays) or at is null
sort at
```

Read it top to bottom:

1. **Join** plants to waterings, outer-preserving, so never-watered plants
   ride along with `at = null`.
2. **`sort -waterings.at`** orders newest watering first.
3. **`take[plants.id] 1`** is the grouped limit (§spec-5 "Other steps"):
   from each bucket keyed by plant, keep 1 row under the current sort.
   I.e. each plant's most recent watering. Brackets after `take` always
   enclose bucket keys (after a source they enclose the projection; anywhere
   else they fail to parse).
4. **The bar** is where the actual thirst test lives, and it *must* live on
   the bar: `at <= $today - everyDays` compares a waterings field against a
   plants field, and a cross-entity comparison can't sit in either source's
   parens. Both sides exist only after the join (§spec-4 "The three
   scopes"). Note the typed arithmetic: `date - duration → date`, and a
   field-to-field comparison, both blessed (§spec-4 "Batch semantic pins").
5. **`or at is null`** is the null trap, handled (§spec-5, and see
   `traps.md`). Ordo has two-valued logic: comparisons against null are
   false (§spec-4 "Null semantics"). A never-watered plant has
   `at = null`, so `at <= anything` is false. Without the explicit null arm,
   your most neglected plants would be silently excluded. `is null` /
   `is not null` are the only null tests. Note the parens: mixing
   `and`/`or` without them is a parse error (§spec-1 rule 2). Here the
   left side is parenthesized and only `or` joins the terms.
6. **`sort at`** puts the longest-unwatered first. One honest wrinkle:
   nulls sort last, always, regardless of direction (§spec-4), so the
   never-watered plants land at the *bottom*. If you want them on top,
   expression sort keys are legal: `sort cond(at is null -> 0, 1), at`
   (`cond` is the language's only conditional, arms in order, first match
   wins, mandatory else; §spec-3 "Expression functions").

### 2.6 Where `$today` comes from, and the rule about `today` and `now`

This is the single most-missed rule in the language, so it gets its own
heading:

> **`today` and `now` exist in exactly one place: a shape's field default.**
> They are not query vocabulary, and they are not part of the write
> expression grammar. Every other execution-relative value arrives as a
> **declared parameter**.

Legal (a default, §spec-3):

```
entity waterings {
  plant -> plants   owned
  at:    date = today          # ✓ resolved once, at create time
}
```

Not legal, and all three are parse errors:

```
| at <= today - everyDays                      # ✗ not in a condition
set waterings { plant: $plant, at: today }     # ✗ not in a write RHS
waterings(at >= now - 7d)[at]                  # ✗ `now` is not query vocabulary
```

The middle one deserves a beat, because it is the one people get wrong. The
right-hand side of a `set` *is* the full expression grammar, which makes
`at: today` look fine. It is a parse error: `today` is not an expression.
It is one of three closed **default** expressions (`today`, `now`, `newid`),
and defaults live in shapes.

So the thirsty query takes `$today: date` as a parameter, which the SDK
fills at call time. This is the design.
Nondeterminism gets exactly two visible homes: `since` in a condition, and a
declared parameter in the signature. A parameter is typed, bounded, listed
in the catalog, recorded in the command log with the value it actually
resolved to, and pinnable for replay. A bare `today` sprinkled through
bodies would be none of those.

One honest consequence, since you will hit it: **a converge never re-applies
defaults**, so a converge-or-create statement cannot stamp a server-side
date. If a timestamp must move on update, describe the field and pass
`$today`. (`traps.md`, "`today` and `now` live in shape defaults".)

### 2.7 The fan-out guard, and `let`, the escape you will need

One guardrail you didn't hit but will: **a pipeline may contain at most one
many-path** (§spec-5 "Fan-out guard"). Joining two independent child
collections under one parent (the classic star double-count) is a compile
error, not a wrong answer.

Plantcare is too small to trip it, so borrow a shape from the club tracker
you'll build in Part 2: per club, count both its books and its members.
Those are two independent many-sides hanging off `clubs`:

```
op clubHealth =                                       # ✗ COMPILE ERROR
  clubs[name]  clubBooks[id]  memberships[id]
  group clubs.id, clubs.name { books: count, members: count }
```

The fix is `let`, and here is its syntax. You bind a named pipeline, and
the name is then an ordinary source:

```
let <name> = <pipeline>
```

```
op clubHealth =
  let bookCounts   = clubs[id]  clubBooks[id]   group clubs.id { books: count }
  let memberCounts = clubs[id]  memberships[id] group clubs.id { members: count }

  clubs[name]  bookCounts[books]  memberCounts[members]
  sort -books
```

Each `let` is its **own** pipeline with its own single many-path, so the
guard is satisfied three times instead of violated once. The join back
is inferred: a `let`'s group-key columns remember their provenance (here
`clubs.id`), and match inference reconnects them to the `clubs` source
exactly as if it were a declared ref (§spec-5 "Let outputs as sources").
Use `match on <col>` if two lets could attach more than one way.

Two more facts about `let`:

- **A `let` ending in `reduce` is a scalar**, read with a dot:

  ```
  op unusualWaterings($plant: id) =
    let baseline = waterings(plant == $plant) reduce { n: count }

    waterings(plant == $plant)[at, by]
    | baseline.n > 10
    sort -at
  ```

  A `let` that doesn't end in `reduce` is a source only, referencing it as
  a scalar is a compile error.
- **One `let`, many uses**, later pipelines in the same op may all
  reference it.

The other escapes from the same guard: `match any` / `match none` when you
only need existence, or two separate ops.

**One step further, and it is the step people get stuck on.** Above, the two
counts are only *displayed* side by side. Suppose you want to **compare**
them: "clubs where fewer than half the members finished the book", quorum,
thresholds, "more blocked than open". Two more rules apply. First, the
joined-back `let`s are outer-preserving, so a club with no matching rows
hands you `null` instead of `0`. Then `null * 2 < members` is **false**,
silently dropping the row you cared about most. Second, compare by
multiplying, because `/` yields `decimal` and errors on a zero denominator.
The fully worked version has both `let`s, both `coalesce`s, and the threshold
variant. It is `modeling-guidance.md` §C7. It is worth the detour: two real
builds abandoned their app's central feature for want of it.

**You now have every read the app needs.** Next: writing.

---

## 3. Writes: one verb, modes derived

Ordo has two write verbs: `set` (make the described state true) and
`del` (remove) (§spec-6). You do not choose between insert and update.
The compiler derives the **mode statically from the statement's shape** and
prints it on the artifact. Here is each mode as plantcare needs it.

### 3.1 Create

```
set plants { name: $name, room: $room, everyDays: $everyDays }
```

No identity in the braces → the only way to make this state true is a new
row → **mode: CREATES**. Creation-capable statements must be
*insert-complete*: all required fields statically covered (here `name`;
`room` and `everyDays` have defaults, `notes` defaults to `""`). A write
that only works when the row already exists cannot be written by accident.

```
set waterings { plant: $plant, by: $by }
```

This also creates. `at` defaults to `today`, and the command records the
resolved date, not the word "today".

### 3.2 Converge with compare-and-set

```
set plants { id: $plant, rev: $rev, everyDays: $everyDays }
```

An `id` in the braces addresses an existing row → **mode: CONVERGES** that
member: touch only the described fields, leave the rest. Converge on an
absent id is an error and never a create. Ids are server-minted, so absence
means deleted, and there is no resurrection (§spec-6 safety rule 4).

Including `rev` upgrades it to **compare-and-set**: if the row's version
moved since you read it, the write rejects with `cas_miss` and you refetch.
Lost updates become explicit conflicts instead of silent overwrites.
Generated forms CAS by default (`ui-gen.md` §1).

### 3.3 Bulk

```
set alerts(key == "thirsty:" + $plant and status == "active") { status: "resolved" }
```

Parens on a write → **mode: BULK**: converge every selected row. The
condition is the write's *entire* specification. Row counts are outputs of
reads and never inputs to writes. A UI that wants "this will affect 3 rows"
runs the same condition as a count query first (§spec-6 safety rule 7). An
empty selection is vacuously true, so the write is a no-op and not an error.
Bulk braces may not contain identity or unique-key fields, so addressed and
bulk writes cannot blur.

(That condition builds the alert key by concatenation. `+` on text with a
non-text operand renders it canonically, so `"thirsty:" + $plant` matches
the key the routine will write in section 4.)

### 3.4 Converge-or-create

You will meet the fourth mode in the routine. Here is the shape:

```
set alerts { key: $key, title: $title, status: "active" }
```

No `id`, but the braces cover a declared unique key (`key`) → **mode:
CREATES-OR-CONVERGES** on that key. One statement is the whole alert
lifecycle: first time it creates, every later time it converges the same
row. This is the only upsert semantics in the language. The static mode
label makes the behavior reviewable at deploy time (§spec-3 "Refs",
uniqueness).

**Why there is no update/insert split:** the statement's *shape* decides the
mode. The compiler labels it (`creates` / `converges` /
`creates-or-converges` / `bulk`), and the label appears in registration
output and deploy diffs. A reviewer reads a machine-checked fact and does
not need to remember a convention.

### 3.5 Delete

```
del waterings(at <= $before)          # bulk remove: prune old logs
del plants { id: $id }                # takes its owned waterings with it
```

`del` is genuinely different from `set`, so it's a separate verb. Owned
children cascade (a watering is part of its plant); association refs
restrict: deleting a row referenced by others errors and lists the referrers.
Bare `del plants` does not parse; delete-everything must be stated as
`del plants(true)`, greppable on purpose.

**And every write (all of them) compiles to attributed, before/after-imaged
commands, hash-chained per space, grouped per invocation (§spec-6
"Compilation").** A bulk `set` gone wrong is one group-revert away. This is
the safety net under everything above, and you did not build it.

---

## 4. A routine and its trigger

> **Status:** the `routine` and `trigger` forms compile and run since
> 2026-10-08 (checklist 12a), and so do `each ->` bodies that read a pipeline
> and emit per row. `ordo check` accepts this block. A run handles at most 500
> rows by default (`ORDO_ROUTINE_EACH_MAX_ROWS`). See `howto-routines.md`.

The daily nag. Two platform ideas compose here: **triggers**, which are
blind, and **routines**, which see values under a grant.

```
routine thirstCheck in plantcare {
  trigger every day 09:00Z
  run($today: date):
    plants[id, name, everyDays]  waterings[at]
    sort -waterings.at
    take[plants.id] 1
    | (at <= $today - everyDays) or at is null
    each ->
      set alerts { key: "thirsty:" + plants.id,
                   title: name + " needs water",
                   status: "active" }
}
```

**Triggers see shape and time, never values (§spec-7).** Two primitives
only: a clock (`every day 09:00Z`. The `Z` is mandatory; UTC is explicit at
the keyboard, never implied) and a listener on the command-log envelope
(`when plantcare.waterings.created -> run ...`, with `debounce`/`coalesce`
modifiers). Anything that must *see a value* ("is this plant overdue?") is
not a trigger. It is the first line of a routine.

**The routine body is the language you already know.** Tier-1 routines are
plain `.ordo` (§spec-8): the query is the thirsty query from section 2.5,
and `each ->` (the one iteration construct) emits one proposal per row.
Since the language has no I/O primitives, a rule *cannot* misbehave;
sandboxing is moot.

**The write is the converge-or-create from 3.4.** `key: "thirsty:" +
plants.id` gives each plant one stable alert row: day one creates it, day
two converges the same row instead of stacking duplicates. And `status:
"active"` is load-bearing: converge touches only described fields, so
without it, an alert you resolved (via the `water` op's bulk set, section
5) would stay `"resolved"` forever even as the routine re-fired. Re-raising
must be *stated*.

**The name is an identifier, so it is lowerCamel.** Write
`thirstCheck`, not `thirst-check`. Routine names obey the same rule as every
other identifier (`[a-z][A-Za-z0-9]*`, §spec-2). A hyphen there lexes as
*minus* and the declaration fails to parse. (Older examples across these docs used hyphens;
they were wrong and have been corrected.)

**Two things constrain a routine, and neither is a `$caller` guard.**

*First, its trigger.* Plantcare has no guarded entities yet, so this doesn't
bite here, but the rule is worth learning where it is simple, because Part 2
adds guards and it starts mattering immediately. **What a routine may touch
follows how it was woken** (§spec-8):

| Woken by | Bound |
|---|---|
| **clock**, `trigger every day 09:00Z` | guards pass. Nobody outside chose the moment, so the run sees every row its scope allows. `thirstCheck` is this one. |
| **listener**, `trigger when plantcare.waterings.created` | the run may read and write **only rows reachable from `$trigger.ids`**, the rows that actually changed. |

The asymmetry has a reason. A listener fires on writes an **outsider**
made (a webhook, a sensor, a feed arriving from another space), so an
unbounded listener would let that outsider choose both when a guard-free
process runs and which rows it starts from. That is a confused deputy, and it
is why the exemption belongs to the clock rather than to the `routine:`
principal. (You may meet an older explanation in other projects' notes,
"a routine holds no membership row, so guards would evaluate false." That
rationale was retired: it described the clock case and got the listener case
dangerously wrong.)

*Second, its derived scope manifest.* The
routine's read/write scope comes from its body, so nothing hand-written can drift.
This routine may touch `plants`, `waterings`, and `alerts` (those fields and no
others), disclosed at deploy time, with every write attributed to
`routine:thirstCheck` in the command log. Note what the manifest bounds:
**which entities and fields**, never which *rows*. The trigger does rows; the
manifest does fields; conflating them is the mistake the old rule made. Keep
routine bodies narrow. A routine that projects a whole entity has granted
itself that entity. Key release follows the same rule as everything else:
derived from op access, every release logged (§spec-7 "Key release").

**`run` takes flags**, in the same slot and with the same meaning as an op's:
`run($today: date) unbounded:` acknowledges an O(space) pass, and
`run() unguarded:` is how a *listener* says it must legitimately range
beyond `$trigger.ids`. Plantcare's routine needs neither.

Delivery of the nag is the platform's job. Domains never touch notification
providers (§spec-8 "Notification channels"):

```
route alerts.warning -> [push] fallback email
```

**You now have an app that acts on its own, and every action it takes is an
attributed, revertible command.**

---

## 5. Expose it: the `op` wrapper

So far the pipelines and statements were bodies. Registration wraps each in
the one construct the platform serves, settled in §spec-13:

```
op name($params) flags = body
```

The complete plantcare surface:

```
op addPlant($name: text, $room: text, $everyDays: duration(ms)) =
  set plants { name: $name, room: $room, everyDays: $everyDays }

op water($plant: id, $by: text) =
  set waterings { plant: $plant, by: $by }
  set alerts(key == "thirsty:" + $plant and status == "active") { status: "resolved" }

op reschedule($plant: id, $rev: int, $everyDays: duration(ms)) =
  set plants { id: $plant, rev: $rev, everyDays: $everyDays }

op thirsty($today: date) unbounded =
  plants[name, room, everyDays]  waterings[at]
  sort -waterings.at
  take[plants.id] 1
  | (at <= $today - everyDays) or at is null
  sort cond(at is null -> 0, 1), at    # 2.5's wrinkle, fixed: nulls sort last
                                       #   always, so the never-watered plants
                                       #   need an explicit key to reach the top

op history($plant: id) =
  waterings(plant == $plant)[at, by]
  sort -at
  take 30

op plantCount =
  plants[name]
  reduce { n: count }

op pruneLogs($before: date) =
  del waterings(at <= $before)
```

Three details in the wrapper itself:

- **Params declare type and bounds** (`$limit: int <= 100`), enforced at the
  door before planning. A caller sending 500 gets a typed `bound_violation`,
  and your query code never sees it. `int`, `money`, `decimal` and `date`
  take bounds; a `duration` bound is not built yet, so `addPlant` takes
  none.
- **Flags** sit between name and `=`: `reproducible` (forbids `since`,
  for audit views), `unbounded` (acknowledges an O(space) cost class),
  `unguarded` (explicit guard bypass). Plantcare needs one: `thirsty` walks
  every plant, so it says `unbounded`.
- **The op's kind is derived from the body, never asserted**: any
  `set`/`del` makes it a write; a pipeline makes it a read. `water` is a
  write containing two statements, and a multi-statement op is **one
  atomic command group** (§spec-6 "Atomicity"): the watering and the alert
  resolution land together or not at all, and triggers fire only on commit.

`water` is worth pausing on: it's *the whole interaction*. Log the watering,
resolve the nag, atomically. If the plant has no active alert, the bulk arm
is vacuously true and nothing extra happens.

### What the compiler now knows

Compilation happens once per op per version at deploy time
(`implementation.md`). Each artifact carries these, all derived and none
hand-written:

| Derived | For `thirsty`, concretely |
|---|---|
| **Kind → HTTP method** | read → `GET /op/thirsty` (writes are `POST`, pure dels `DELETE`; wrong method is a 405 before anything else, `api-engine-design.md` §1) |
| **Mode label** | reads: live-capable, pageable. For `water`: the write-mode labels (`creates`; `bulk`) printed in registration output and deploy diffs |
| **Field-scope manifest** | exactly `plants.name`, `plants.room`, `plants.everyDays`, `waterings.at`. What the consent page discloses, what key release is scoped to |
| **Cost class** | `sql`. Every stage pushes down; no sealed field is touched. Had the bar filtered on `notes`, the compiler would label the host-side stage and price it (§spec-5 "Cost classes") |
| **Result shape** | `name: text, room: text?, everyDays: duration, at: date?`, and at load, the planned SQL runs once against an empty shadow schema so the observed shape must match the inferred one exactly (`implementation.md`, "Empty-set execution") |
| **Ordering & pagination** | total order from `sort at` + implicit id tiebreak; keyset cursor tokens derive from it, stable under writes |
| **Catalog entry** | the op joins the deployment's capability catalog, machine JSON for SDK generation, plus an LLM-ready rendering, scoped to what each caller may call |

One thing registration deliberately does *not* include: **who may call it**.
Access is runtime state (membership roles, per-space profiles, capability
tokens). It is never syntax and never IaC (§spec-8 "Access model"). The op exists;
exposing it is a separate, audited act.

**You now have a typed, versioned, self-describing API you didn't write a
route for.**

---

## 6. The app manifest: instant UI

Generated UI is not codegen. Every registered op already emits a **surface
descriptor** (columns, formats, row identity, actions, pagination, derived
from the manifest and result shape; `ui-gen.md` §1). The `app` manifest just
names pages and puts ops on them (`ui-gen.md` §5, grammar is [lean],
i.e. illustrative):

```
app plantcare {
  page home {
    metric plantCount
    table  thirsty live
  }
  page plant($id) {
    detail std.plants.whole($id: $id)
    table  history($plant: $id)
  }
}
```

What one page of manifest buys:

- **Compile-checked against the catalog.** An unknown op, a missing required
  param, a wrong param type are all build failures.
  (`$today` on `thirsty` is the execution-relative-date pattern from §2.6:
  SDK-filled at call time.) Bindings are always **named**,
  `$param: expression`, as in `history($plant: $id)` and
  `std.plants.whole($id: $id)`. Positional binding does not parse, and
  neither do empty parens.
- **`metric plantCount`**. The body ends in `reduce`, so the result is
  statically one row; the descriptor becomes a metric card. Structural
  detection, never a naming convention.
- **`table thirsty live`**. Liveness is a call-time choice on any read op
  (§spec-5 "Live queries"). The page subscribes, a commit anywhere relevant
  signals stale, the client refetches under its own scope. Watering a plant
  removes it from the thirsty table in front of you.
- **Rows auto-link.** Every list row carries its typed id even when
  unprojected; an id of type `plnt_` links to `page plant($id)` because that
  page claims the type. Typed ids make navigation derivable.
- **Forms and actions come from writes.** `water` declares an id-typed param
  matching `plants` rows, so it surfaces as a row action; `addPlant` params
  become a create form; commits are CAS-by-default with typed conflict
  handling (`ui-gen.md` §1).

App manifests are definitions. They ride the same `plan`/`apply` engine,
version-hashed and diffed like everything else.

**You now have screens.** They are derived, and you can replace them per
surface later (`ordo eject surface <op>`).

---

## 7. What std gave you for free

The moment `ordo apply` ran, before any of *your* ops existed, the platform
derived the **std library** for every entity. A pure function of the schema
(`std-library.md`). You never wrote get-by-id, list, or an audit view.
Plantcare's free surface includes:

| Op | What it is |
|---|---|
| `std.plants.get($id)` / `std.waterings.get($id)` / `std.alerts.get($id)` | full row by id; typed `not_found` on absence |
| `std.plants.list` (+ `count`) | keyset-paged listing, newest-first, Tier-0 refinable, with structured conditions over any field |
| `std.plants.whole($id)` | the plant *and its waterings*. The owned closure you declared in section 1.2, walked for you |
| `std.plants.create` / `update` / `delete` | baseline writes: `update` is a full-row CAS converge (read → mutate → commit in the SDK); field-subset updates like `reschedule` are authored ops *by design*. An op that touches one field says so in its name and manifest |
| `std.alerts.put` | derived because `alerts` declares exactly one unique key. The converge-or-create baseline |
| `std.plants.history($id)` | every command that ever touched this plant, actor, before/after images, newest first. The audit pane you didn't build |
| `std.space.members()` / `profile()` / `callersOf($op)` / `std.catalog()` | the access model's promise that "who can call X" is one exhaustive query, shipped as ops |

Two honesty rules worth knowing (`std-library.md` §5):

- std ops are **registry-present but exposure opt-in**: deriving an op and
  granting it are different acts. `ordo apply` created `std.plants.delete`;
  nobody can call it until someone says so.
- std's manifests are the **whole entity, over the open skeleton**. For an
  entity carrying sealed fields, the derived `get`/`list`/`update` cover the
  open fields only; `notes` is not in `std.plants.get`'s manifest, so
  granting std does not grant your private observations about the fern.
  Sealed disclosure is always an authored act: either an op you write whose
  body names the field, or the explicit opt-in marker on the shape that says
  "derive std over the sealed fields too". Either way it is one greppable
  line in source and one line in the deploy diff.

Narrower access than std is an authored op with a narrower manifest, which
is what `thirsty` and `history` are.

And note what the generated UI in section 6 was standing on: table =
`std.plants.list`, detail = `std.plants.whole`, form = `std.plants.create`.
The UI works before your first authored op exists.

---

---

# Part 2: `teamtrack`, the same skills, with other people in the room

Plantcare had one user, so it needed no authorization at all. Almost nothing
real is like that. Part 2 builds a second, smaller app: a project tracker
several people share. It exists because **the gap between these two
apps is where generated and hand-written Ordo apps have actually failed.**

The failure is worth naming before you write a line. A real app was
generated from Part 1's material alone. It declared `access read` on half
its entities and `access write` on none. It shipped a schema where every
entity carried an access line and half the app was open to any caller who
could reach it. A human reviewer read it as locked down, and it was not.
Part 2 is here to make that outcome impossible for you.

Nine things get built, and each closes one hole in that story:

| Beat | Section |
|---|---|
| `acl` on a root entity. The permission system in one line | 8 |
| Guards inherited by the owned subtree | 8 |
| `access read` **and** `access write`, always both | 9 |
| Link entities declare their own guards | 9 |
| `$caller` in an op body, and what it does *not* do | 10 |
| Day one: who can do anything at all? (the bootstrap) | 11 |
| Seeding the second person. There is no `grant` statement | 11.1 |
| Groups, and letting an outsider in | 12 |
| A routine in a guarded app: trigger provenance | 13 |

---

## 8. Shapes: one `acl` line, and what it derives

```
domain teamtrack @ 0.1.0

entity projects {
  name:         text
  status:       text = "active"
  lastActivity: date?                 # a routine will maintain this in §13
  acl (read, write, admin)
}

entity tasks {
  project  -> projects  owned
  title:    text
  done:     bool = false
  assignee -> members?
}
```

A dozen lines, and one of them is the whole permission system.

**`acl (read, write, admin)` derives machinery, visibly (§spec-3 "acl").**
The result is real, inspectable artifacts:

- **A real link entity**, `projectsAccess { project -> projects owned,
  member -> members owned, level, unique (project, member) }`. It is an
  ordinary entity: queryable, writable, extendable. Permissions are rows.
- **Guards on `projects` and its entire `owned` subtree.** `tasks` is owned
  by `projects`, so `tasks` is guarded too, **and you wrote nothing on
  `tasks` to get that.** Every op touching tasks, including the derived
  `std.tasks.*` family, has the guard inlined by the compiler with no
  exception path.
- **Admin ops** (`grantProjectsAccess`, `revokeProjectsAccess`, level
  changes) are ordinary registered ops with manifests, gated by the
  **highest** declared level. Only `admin` may grant.
- **Level ordering** runs left to right: `admin` includes `write` includes
  `read`. The names are your app's vocabulary, and the ordering is the
  declaration order.

**`members` is the platform's entity.** Do not declare it. It
holds one row per principal belonging to the space; the platform creates the
row when someone joins and retires it when they leave (its `status` reads
`active`, `suspended`, `left` or `deleted`; `howto-leave.md` and
`howto-pause.md` cover leaving a tenant, deleting an account and pausing a
tenant). Its job in your schema
is to be the thing `-> members` refs point at. Also,
**`$caller` compares equal to the caller's own `members` row**, so
`member == $caller` inside a guard or an op body is a plain comparison.
You never reconcile principal ids against member ids by
hand.

**What you can read off this file, without running anything:** a task is
readable exactly by the principals holding a row in `projectsAccess` for its
project, because tasks are owned by projects and projects carry an `acl`.
That sentence is derivable from eleven lines.

### 8.1 One more thing shapes can hold: invariants that walk refs

Authorization answers *who may touch this row*. `check` answers *what must be
true of it*, and it is worth meeting now because it can see further than you
would expect, through the row's own **outgoing refs**, at any depth:

```
entity tasks {
  project  -> projects  owned
  title:    text
  done:     bool = false
  assignee -> members?
  check assignee.status == "active"      # no assigning work to a suspended member
}
```

**Status:** the ref-walking `check` above is not built yet. `ordo check`
refuses it with `check_beyond_row`, so today a `check` may test only the
row's own fields.

A `check` compiles to a constraint the database enforces **inside the write's
transaction**. A violation is a typed rejection that aborts the whole command
group. This is the same all-or-nothing behavior as every other failure in a group.

Two limits, and they are the useful part:

- **Outgoing only.** The walk follows refs *from* the row being written
  (many→one, the direction dot-paths always go). It never walks backwards into
  a collection.
- **No counting.** `check` cannot aggregate, count, or test existence over
  other rows. "At most ten open tasks per project" is a question about *other
  rows*, so it lives one tier up: keep a counter column on `projects` and
  `check` the counter, incrementing it in the same atomic op that creates the
  task. That is also why the language has no `require`-style op precondition.
  The invariant belongs on the shape, where every write path meets it.

---

## 9. Both verbs, always, and the entities that don't inherit

### 9.1 The compile error that saves you

Some entities need a rule of their own. A watcher list is the classic: it's a
**link entity** joining a task to a member, so it has two owners and there is
no single root to inherit from. **Link entities never inherit guards.** Write
one and you must guard it yourself.

Here is the version a careful-looking author writes:

```
entity taskWatchers {                            # ✗ DOES NOT COMPILE
  task    -> tasks    owned
  watcher -> members  owned
  unique (task, watcher)

  access read  task.project.projectsAccess(member == $caller)
}
```

That is a compile error, and the diagnostic says why:

```
a.ordo:17:8: schema error [incomplete_access_clause]: `taskWatchers` declares one `access` verb without the other — both or neither; open access is spelled `access <verb> open`
```

**Why the language refuses.** Read that error next to the story at the top of
Part 2. Before this rule existed, the missing line meant *writes are open*,
silently, with no flag, no manifest note, and an access line sitting right
there making the entity look guarded. It is the one failure mode that
survives code review, because the reviewer's eye confirms "yes, there's a
guard" and moves on. So omission no longer means anything. Open writes
are still legal, and they must be stated.

The version that compiles:

```
entity taskWatchers {
  task    -> tasks    owned
  watcher -> members  owned
  unique (task, watcher)

  access read   task.project.projectsAccess(member == $caller)
  access write  task.project.projectsAccess(
                  member == $caller and (level == "write" or level == "admin"))
}
```

Three details in those two lines:

- **The walk.** A guard is a `$caller`-anchored **relationship walk**:
  dot-paths toward "exactly one" (`task` → `tasks`,
  `.project` → `projects`), then one reverse collection step into
  `projectsAccess`, then an existence test. It compiles to an indexed
  semi-join (`sql` cost class, no host-side pass).
- **`(level == "write" or level == "admin")`, spelled out.** The guard
  grammar is closed: ref walks, `==` against `$caller` or a literal, and bare
  bools. There is no `>=` over levels for hand-written guards, so you
  enumerate. Note the parens: mixing `and` with `or` without them is a parse
  error here, as it is everywhere else (§spec-1 rule 2).
- **Both verbs, restated in full.** An explicit `access` clause **replaces**
  the inherited guard and does not compose with it. There is no composition
  algebra. Whether that replacement is per-verb or whole-clause is
  still being pinned; writing both verbs is correct under either answer, so
  write both. Always. The rule to carry away: *reading an entity's shape must
  be enough to know what protects it.*

### 9.2 When writes really are open, say so

Feedback anyone with a link may file:

```
entity feedback {
  project -> projects  owned
  body:      text
  fromEmail: text?
  access read   project.projectsAccess(member == $caller)
  access write  open              # deliberate: anyone may file feedback
}
```

`access write open` is a legitimate design. It is one greppable word. It
appears in the file and in the deploy diff the day someone adds it, and
nobody can arrive at it by forgetting a line.

**The word is `open`.** `access write true` does not parse. `true` is the
boolean literal, and a guard position that accepted it would also have to
accept `false`, `not false`, and anything else that evaluates to a bool. Then
neither a reviewer nor the compiler's guest-exposure check could tell *a
decision to open* from *an expression that happens to be true today*. `open`
is the one keyword for this, in one position.

---

## 10. Ops: `$caller`, and guards you never mention again

```
op createProject($name: text) =
  set projects { name: $name }

op addTask($project: id, $title: text) =
  set tasks { project: $project, title: $title }

op myTasks unbounded =
  tasks(assignee == $caller and done == false)[title, project.name]
  sort project.name, title

op projectBoard($project: id) unbounded =
  tasks(project == $project)[title, done, assignee.displayName]
  sort done, title

op finishTask($task: id, $rev: int) =
  set tasks { id: $task, rev: $rev, done: true }
```

Read what is *not* there: no permission checks. Not one. The guards from
sections 8 and 9 are inlined by the compiler into every one of these ops.
Read guards filter rows away. Write guards reject loudly and never skip
silently. *Which* loud rejection you get depends on whether you could
see the row in the first place:

- a row you **can read** but may not write → **`access_guard`** (403). The
  informative case, and the safe one: you already knew the row was there.
- a **guarded** row **outside your read view** → **`not_found`** (404), and it
  is byte-identical to the answer for a row that never existed. That is
  deliberate. Without it, anyone holding an id could tell *exists but
  forbidden* from *never existed*, across the whole deployment, just by
  writing to it.

This costs a little diagnosability. In return, the wire cannot be
used to enumerate a space. On an **unguarded** entity there is no read view to
be outside of, so there `not_found` means only "absent". No position
in a pipeline outruns the guards, and there is no way to compose around
them: an op that calls another op inherits the callee's guards too.

**`$caller` is the one reserved parameter** (§spec-2). It is bound by the
runtime from the session and **can never be supplied by a client**. A
request carrying it is rejected outright. So `myTasks` means "tasks assigned
to whoever is asking", and there is no way to ask on someone else's behalf.
Note also that `$caller` is not the *guard*: the guard already restricted
`myTasks` to tasks in projects you may read. `assignee == $caller` narrows
that further to the ones that are yours. Guards decide what exists for you;
`$caller` in a body decides what you asked for.

**`assignee.displayName`, not `assignee.name`.** `members` is the platform's
entity and its field list is fixed; the human-readable field on it is
`displayName`. This is the field to project in every list, board and audit view.
A domain that invents its own profile table to supply a name has created a
second identity source and a second bootstrap problem.

**And one sharp edge, because it costs people an afternoon: `== $caller` never
expands a group.** `$caller` binds to *your* `members` row. If a task is
assigned to the group `designTeam` and you are in that group, `myTasks` does
**not** return it. Your row and the group's row are different rows, and they
are not equal. Group membership is resolved by the **access layer** (guards,
acl rows, grants) and nowhere else. If "work assigned to my team" is a view
your app needs, express it as a guard or an acl relationship, not as an
equality in a body.

**Write guards evaluate against the would-be row.** In `addTask`, the guard
walks `project` (a field the statement describes) so the check is "may you
write in *that* project", answered before anything is inserted. This has a
consequence worth internalizing, because it has bitten real authors: never
put a guard on an entity that requires you to already be in it. A membership
entity guarded by `access write ownMembership(member == $caller)` means you
must be a member to become one. Nobody can ever join, and nothing warns you.
Guard the parent's admin ops instead, and let the derived grant machinery add
people.

---

## 11. Day one: who can do anything at all?

Section 8's schema raises a question that stops many multi-tenant designs.
`createProject` runs. A `projects` row now exists. Its
`projectsAccess` link entity contains **zero rows**. Nobody holds `admin` on
it. The derived grant op requires `admin` on that row. So who grants the
first permission?

Read literally, nobody, forever. Worse, there is a second lock behind the
first: `projects` carries an `acl`, so it carries a **write guard**, and that
guard walks to `projectsAccess` looking for your row. On a row that does not
exist yet, the walk finds nothing. So the `set` that creates the project is
itself rejected. You cannot even get to the granting problem.

Three platform rules close both doors, and none of them is something you write:

**1. A creating write on an acl-root entity is exempt from the derived write
guard.** It has to be, for exactly the reason above. The exemption is narrow
and worth stating precisely: it covers the *creating* write, on the *acl-root*
entity, and nothing else. Every other write on that entity, and every write on
its owned children, is guarded normally.

**2. The creating principal gets the top-level row, in the same command
group.** Call `createProject`, and the project *and* your `admin` row in
`projectsAccess` land together. One atomic command group, one revert if you
undo it, disclosed in the compiler's shape summary for the entity, and
recorded in the log as an ordinary grant command. You create it, you
administer it, and the audit trail says so in the normal vocabulary.

```
op createProject($name: text) =
  set projects { name: $name }
# you write one statement; the command group contains two commands:
#   1. create  projects        (write guard exempt: this is the creating write)
#   2. create  projectsAccess  { project: <new>, member: $caller, level: "admin" }
```

Three corollaries that matter:

- **The creator must be a space member.** A guest holds a `members` row, with
  a null `role`, so acl rows can name them, but a guest cannot mint an
  acl-root row and hand themselves `admin` on it. Bootstrap is a member's
  power.
- **System-, source- and routine-created rows get no access row.** There is no
  creator to grant to. A row a nightly routine mints starts with an empty
  access list, so seeding data means an explicit `set …Access` write
  afterwards.
- The rule applies to the **acl-bearing** entity. Owned children like `tasks`
  never needed a row of their own; they inherit.

**3. A space owner can always administer any acl row in their space.** This
is the universal bootstrap escape, and it is the answer to "everyone who held
admin on this project has left the company." It is checked **at the door**.
That is the same place the request's space is bound, before planning. It is
not a special case inside the guard walk, so it is one decision on one
surface. It is a real power, so it is logged like one: an owner reaching into a
project they hold no row on appears in the chain as exactly that.

Put together: an acl-bearing entity can never lock its own space out, and no
app needs a hand-written "superuser" flag.

### 11.1 The second person, and why there is no `grant` statement

You created the project and you administer it. Now put your colleague on it.

There is no `grant` keyword: **`grant` is a concept and has no statement.**
It does not need syntax, because `projectsAccess` is a *real entity*. The `acl` line derived it, and you write it like any other:

```
op addProjectMember($project: id, $member: id) =
  set projectsAccess { project: $project, member: $member, level: "write" }
```

That is an ordinary write and an ordinary command in the chain.
`projectsAccess`'s own derived write guard applies to it, so the caller must
hold `admin` on that project, or be the space owner (checked at the door).

And because statements in one op are **one atomic command group**, "move the
row and move its access together" is one op with two statements:

```
op handOffProject($project: id, $rev: int, $newLead: id) =
  set projects       { id: $project, rev: $rev, status: "active" }
  set projectsAccess { project: $project, member: $newLead, level: "admin" }
```

Either both land or neither does, and one revert undoes both.

Two notes:

- **`$member` is a `members.id`**: the space-local identity of a principal,
  which is also what `$caller` binds to. It is not a deployment-wide principal
  id and not an email address.
- **The derived ops exist and are usually the right surface.**
  `grantProjectsAccess` / `revokeProjectsAccess` came from the same `acl` line,
  are gated at `admin`, and appear in the catalog. That is what a "manage
  members" screen calls. Write `projectsAccess` by hand when you need the write
  to be atomic with something else, as above.

**The shape you will reach for first is to create the row and seed a co-owner
in one op. It has a spelling.** A `set` whose derived mode is CREATE may be
bound with `let`, because a create produces exactly one row with exactly one
fresh server-minted id:

```
op startProject($name: text, $coLead: id) =
  let p = set projects       { name: $name, status: "active" }
  set projectsAccess { project: p.id, member: $coLead, level: "admin" }
```

You create the project, guard-exempt under G10a, the rule this section opened
with. You take the auto-grant as its creator, and `$coLead` gets `admin` in
the *same* command group. Both land or neither does.

Three rules bound it, and all are statically decidable:

- **Only creates bind.** `let x = set …` on a converge, a CAS write, or a bulk
  write is a compile error (`unbindable_write`). CONVERGE and CAS address a row
  whose id you already hold, so there is nothing to learn. A bulk write
  touches an unknown number of rows, so there is no single `.id` to name.
- **The bound value is a row descriptor** holding `id`, `rev`, and the fields
  that statement wrote. It is usable in *later* statements of the same op, and
  never in the same statement or in a condition of the statement that produced it.
- **`let x = set …` is still a statement.** It names its result and cannot
  appear mid-condition. The write grammar stays closed.

The three older shapes still work and are sometimes the right answer:
let the intended owner create the row and take the auto-grant; create with a
unique external key and let a listener routine seed access from `$trigger.ids`
(the right shape for ingested rows, which have no human creator); or grant by
id in a second op.

---

## 12. More than one person: groups, and letting an outsider in

### 12.1 Groups

Grant to a set, not to a list of humans:

> A **group** is a named collection of principals. It is usable anywhere
> a principal is: in an acl row, on the far side of a guard's walk, or in a
> grant.

So `grantProjectsAccess` can name the group `designTeam` at level `write`.
That is one row, and everyone in the group has write on that project. When
somebody joins the design team next month, that is one membership change, with
no ACL edits and no redeploy. When somebody leaves, their access is gone
everywhere the group was granted.

The rule of thumb is grammatical. If the sentence you are modeling has a
plural noun in it ("the reviewers", "contractors", "the finance team"),
grant to a group and manage the group. If it names a person, grant to the
person.

Three limits apply. Groups are collections of *principals* and form no
role hierarchy. The four space roles (owner / manager / editor / viewer) are
still fixed and still separate from all of this. A group belongs to one space.
And **groups expand in the access layer only**: a guard walk, an acl row and a
grant all resolve group membership, but `== $caller` in an op body does not
(section 10), and `count` over `projectsAccess` counts a group as **one** row.
When you want humans rather than rows, sum the `members` row's read-only
`memberCount` (1 for a person, the group's size for a group):

```
op projectSeats($project: id) =
  projectsAccess(project == $project)[member]
  reduce { seats: sum member.memberCount }
```

(This is a cheap upper bound: someone who holds access both directly and
through a group counts once per path. Exact headcount is a `distinct` pipeline
over `groupMembers`, §spec-8, Groups.)
`modeling-guidance.md` §D covers when a group beats a hand-rolled membership
entity, which is the other way to model the same idea.

### 12.2 A guest

A client wants to watch one project. They are not in your space and should
not be. Two doors exist and only two (`modeling-guidance.md` §B):

- **A window (guest access).** An acl row in `projectsAccess` names their
  principal at level `read`. They call **your** space's ops under **their own
  identity**; the same guards run; every read they make is attributed to them
  in your chain. Revoke, and the window closes at their next call. Because
  guards are what let them in, the compiler refuses to expose an *unguarded*
  op to a guest at all. Over-sharing is a build failure.
- **A feed (a consented copy).** A standing one-way copy of a named query's
  output into their space, both ends consenting. They own the copies; their
  triggers fire on them.

Use a window when they should see live truth in your world, and a feed when
they need their own. Caveat on both: revocation stops future access and does
not erase memory. What their client already fetched, it keeps.

---

## 13. A routine, now that the data is guarded

> **The `routine` declaration and `each ->` bodies are built (checklist 12a,
> 2026-10-08).** Everything this section teaches about *what a
> routine may see* is settled and is the point of the section. The trigger is
> written inside the routine, as `trigger when …`. A routine woken by a change
> may touch only the changed rows and rows one ref away, unless `run` says
> `unguarded` (`routine_out_of_bound`). Read the block below for the
> authorization rule. The block needs no `$trigger` to be scoped: a
> change-woken run is bounded to the rows reachable from the changed rows,
> and its pipeline reads only those.

Part 1's `thirstCheck` ran in an app with no guards, so the question "what may a
routine see" never came up. It comes up now. Add a rule to `teamtrack`: when a
task is finished, mark its project's `lastActivity`.

```
routine touchProject in teamtrack {
  trigger when teamtrack.tasks.converged
  run():
    tasks(done == true)[id, project]
    each ->
      set projects { id: project, lastActivity: today }   # ✗ see below
}
```

That block is wrong in one instructive way and right in another. Take the right
part first.

**Right: it is scoped to the changed rows, and it had to be.** This routine is
**listener-woken**, and a listener-woken run may read and write **only rows
reachable from the changed rows**. These are the rows that actually changed.
This is the authorization bound. A listener
fires on writes an outsider can cause (a webhook `ingested`, a feed `fed` from
another space), so an unbounded listener would hand that outsider the choice of
when a guard-free process runs and which rows it starts from. Guards pass for
**clock**-woken routines (nobody outside chose the moment), and that is the
whole of the exemption:

| Woken by | Bound |
|---|---|
| `trigger every day 09:00Z` | guards pass |
| `trigger when teamtrack.tasks.converged` | rows reachable from `$trigger.ids` only |

`tasks.project` is a declared ref, so `projects` reached through it is
*reachable from* the trigger ids, so writing that project row is inside the
bound. Reading, say, every project in the space would be outside it and would
need `run() unguarded:` on the header. That is one greppable word and a decision
someone made on purpose. If you find yourself reaching for it on a routine
woken by ingested or fed data, stop and re-read this paragraph.

**Wrong: `today` is not part of the expression grammar.** It is a *field
default* and appears nowhere else: not in a condition, and not on the
right-hand side of a write (`traps.md`, first entry). A routine takes
parameters exactly as an op does, so the fix is one you already know:

```
routine touchProject in teamtrack {
  trigger when teamtrack.tasks.converged
  run($today: date):
    tasks(done == true)[id, project]
    each ->
      set projects { id: project, lastActivity: $today }   # ✓
}
```

Two smaller things are already familiar. The write is a **converge** (it
carries `id`), so it touches `lastActivity` and leaves `name` and `status`
alone. Every write is attributed `routine:touchProject` in the chain, which
is how "who moved this timestamp" stays answerable. The routine's **derived
scope manifest** (`tasks.id`, `tasks.project`, `projects.lastActivity`) is the
other half of its bound, and it bounds *fields*, never rows. The trigger
bounds rows, and the manifest bounds fields.

---

## 14. What Part 2 bought you

Twenty-odd lines of shape and five ops, and you can read the security
properties off the file:

| Question | Where the answer lives |
|---|---|
| Who can read a task? | `projects`' `acl` line, inherited through `owned` |
| Who can write a watcher row? | `taskWatchers`' own two `access` lines |
| Can anyone write anywhere by accident? | No, omission is a compile error |
| How does the *first* row get created at all? | The creating write on an acl root is guard-exempt |
| Who administers a brand-new project? | Its creator (a member, never a guest), granted in the same command group |
| How does the second person get access? | `set projectsAccess { … }`. There is no `grant` statement |
| What if all the admins leave? | The space owner, checked at the door, logged |
| Who sees "my tasks"? | `$caller`, bound by the runtime, never client-supplied |
| Does "my tasks" include my team's tasks? | **No**, `== $caller` never expands a group |
| How does a whole team get access? | One group grant |
| How does an outsider get in? | A window (acl row) or a feed. Nothing else |
| What may a routine touch? | Its trigger decides: clock ⇒ guards pass; listener ⇒ `$trigger.ids` only |

Status: the authorization model above **executes**. `acl` and `access` parse
and bind, guard resolution and acl derivation run in the schema, guards inline
into every read as nested `EXISTS` semi-joins, and the write path runs them as
preconditions (`not_found` for a row outside your read view, `access_guard`
for one you may see but not change). The door (membership roles, per-space
access profiles, capability allowlists in `crates/ordo-identity`) sits in
front of all of it, now behind verified bearer tokens.

The rejections in this chapter are rejections *today*. The compiler refuses a
schema that declares one access verb without the other, an `unguarded` flag
that strips nothing, and an op that composes a live guard bypass without
acknowledging it. The blocks above are test cases.

Still owed is breadth: guests-at-door, the fuller group expansion, and
dotted-target guards beyond the shapes the conformance cluster exercises.

---

## 15. Where to go next

Across both parts you built: 5 shapes, 12 ops, 1 routine, 1 route, 1 app
manifest, roughly two and a half pages. You received a typed versioned
API, generated UI, row-level authorization with an audited grant path, a
hash-chained audit log with group revert, live queries, daily automation,
push notifications, and a std baseline (§spec-10).

From here:

- **`traps.md`**. The sharp edges. You already met four of them (null
  comparisons in 2.5, `today` in 2.6, the fan-out guard in 2.7,
  write-open-by-omission in 9.1). Twenty minutes with it covers the rest.
- **`definition-language.md`**. The spec this tutorial cited throughout.
  Read §3 (shapes, guards, `acl`), §4 (conditions) and §6 (writes) in full;
  they are short and they are the language.
- **`modeling-guidance.md`**. The decisions this tutorial made for
  you: how many spaces (§A), which sharing mechanism (§B), where aggregates
  live when your needs outgrow `group` (§C, and **§C7** for comparing two
  independently grouped aggregates, the recipe most apps need and few find),
  and when to reach for `acl` vs hand-rolled guards vs profiles (**§D**, the
  section to read before your second app).
- **`std-library.md`** and **`ui-gen.md`**. The two derivation layers you
  just used.
- **`implementation.md`**. Custody, client-first/E2E mode, and how compiled
  ops deploy and hot-swap.
- **`glossary.md`** for terms.
- The conformance corpora (`guards-conformance.md`,
  `composition-conformance.md`, `sql-translation-corpus.md`), where blocks
  like yours become executable expectations. Skim `guards-conformance.md`
after Part 2: it holds Part 2's rules as ~87 executable cases.

Neither app needed these, but the language has them: sealed blind indexes
(`sealed indexed`, encrypted equality at SQL speed), `tree` refs with
`under`/`above`, named-query composition, consented feeds across spaces, and
per-member profile overrides. Each has a section in the spec and follows the
grammar you now know.
