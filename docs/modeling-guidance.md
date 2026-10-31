# Ordo Modeling Guidance

Architecture decisions the language doesn't make for you. Companion to
`definition-language.md` (cited as **§spec-N**), `std-library.md`,
`implementation.md`, and `ui-gen.md`; terms in `glossary.md`, sharp edges in
`traps.md`. Three questions every Ordo app hits before its first entity is
right:

- **A.** How many spaces? (One shared space, or space-per-user?)
- **B.** Which sharing mechanism, when data must cross people or apps?
- **C.** Where do aggregates live, when reporting needs outgrow `group`?
- **D.** How is authorization modeled: `acl`, hand-rolled guards, profiles,
  groups? (New, and the one most apps get wrong; if you read one section,
  read this one.)

---

## A. Space granularity

### What a space actually is

A space is not a folder. It is simultaneously the unit of:

- **custody**: one DEK, one position on the sovereignty dial (managed →
  BYO-DB/bucket → client-first E2E), converted *in place, as a whole*
  (`implementation.md`, "Space custody conversion");
- **membership**: one member list, one role set, one access-profile
  (§spec-8 "Access model");
- **the chain**: one hash-chained command log: attribution, revert,
  ordering guarantees, and the trigger bus are all per-space (§spec-6,
  §spec-7);
- **exit**: export and crypto-erase deletion are whole-space, owner-only
  ops (§spec-8 "Exit ops").

So the granularity question has a non-arbitrary answer: **a space is the
unit of shared custody, membership, and history.** Draw the boundary where
those three should coincide.

### Rules of thumb

- **Collaborative app → one space per household / team / project.** The
  collaborators genuinely share custody and history; differentiation
  *inside* the space is what `acl`, access guards, `$caller`, and profiles
  are for (part D).
  **Caveat, stated because it is unmeasured:** chain append takes a per-space
  advisory lock (`implementation.md`), so every write in a space serializes at
  one point, and no throughput number has been published. Nobody yet knows
  where "one space per team" stops being comfortable. Until that number exists,
  treat very large single spaces as an open risk and not a validated
  pattern, and prefer the space boundary the *deletion* story implies.
- **Personal-data app → one space per user.** Each person owns their
  custody dial, their export, their deletion. Sharing happens through the
  doors in part B (guest windows, feeds), deliberately, per slice, never
  by co-locating.
- **When unsure, split by the deletion story.** Ask: "when this person
  leaves / this relationship ends, what should one button remove?" That
  button is space deletion; its blast radius is the space.

### The todo list, both ways

The same tiny domain, modeled at both granularities (read the differences,
not the code).

**One shared space (the household):**

```
domain todo @ 0.1.0

entity lists {
  name: text
  acl (read, write, admin)
}

entity todos {
  list  -> lists   owned
  title: text
  done:  bool = false
  assignee -> members?
}

op myTodos =
  todos(assignee == $caller and done == false)[title, list.name]
  sort title

op addTodo($list: id, $title: text) =
  set todos { list: $list, title: $title }
```

The `acl` line derives the permission machinery (§spec-3 "acl"): a real
`listsAccess` link entity, per-list grant/revoke ops gated at `admin`, and
guards inherited by the whole owned subtree, so `todos`, owned by its
list, is covered without another word. `$caller` scopes visibility in the
op body, where it's disclosed in the manifest (§spec-2 "$caller").

*Easy here:* sharing is a role or an ACL row: instant, live, revocable;
everyone's writes interleave on one chain, so "who did what" is one query
(`std.space.callersOf`, `std.todos.history`); one billing/quota boundary;
`?live` collaboration with no cross-space machinery.

*Hard here:* the custody dial is shared. If one member wants client-first
E2E, the *whole household* converts or nobody does; "export my stuff" does
not exist: export is whole-space and owner-only; a member who leaves takes
nothing with them without a feed; per-person data (a private grocery list)
needs guards to *simulate* privacy that a separate space would give
structurally.

**Space-per-user (the personal tracker):**

```
domain todo @ 0.1.0        # same domain; each user's space enables it

entity lists {
  name: text
  acl (read, write)        # kept: ACL rows are also how guests enter (part B)
}

entity todos {
  list  -> lists   owned
  title: text
  done:  bool = false
}
```

No `assignee`, no `$caller` filtering. The space boundary *is* the
visibility rule; every op is already "mine."

*Easy here:* each user sets their own custody (your partner stays managed,
you go client-first; nobody negotiates); export and deletion are exactly
"my data"; billing and quotas fall out per user; abuse blast radius is one
space.

*Hard here:* sharing a list is now an explicit act through a door, a
**guest window** (the friend calls *your* space's guard-bearing ops,
attributed as themselves in *your* chain) or a **consented feed** (they get
copies in their space); both real mechanisms, both more ceremony than "add
member"; a genuinely co-owned list still has one home space (one custody,
one export) and the other person is forever a guest in it; any cross-user
view ("everything assigned to me across households") requires a feed or a
tier-2 aggregator, because no query spans spaces.

### Choosing, and changing your mind

The honest summary: **one shared space makes sharing trivial and
individuality expensive; space-per-user makes individuality trivial and
sharing explicit.** Match the boundary to which of those your app does all
day.

Split into more spaces later when you observe:

- **custody divergence**: one party needs E2E/BYO and the others don't
  (conversion is per-space and all-or-nothing);
- **membership divergence**: a subset needs its own membership and
  meta-permissions, which a guard does not give (a sub-team that manages itself);
- **export/deletion boundaries**: offboarding demands "remove exactly this
  person's data," which only a space boundary makes structural.

And know the constraint before you rely on refactoring: **spaces never
merge** (§spec-8 "Inter-space sharing"), and there is no split primitive.
Reshaping granularity later means exporting/feeding data into new spaces
and abandoning the old chain's continuity. The sharing doors in part B are
the pressure valve that makes an imperfect early choice livable; they are
not a migration tool. Spend the hour up front.

---

## B. Sharing: the chooser

Ordo has exactly four ways for data to reach another party, and they do not
overlap. The tree:

```
Is the other party (naturally) a member of the same space?
│
├─ YES → in-space access: roles + profiles; acl / access guards /
│        per-member overrides; `$caller`-scoped ops.        (§spec-3, §spec-8)
│
└─ NO, the data lives in a space they don't belong to:
   │
   ├─ Want a LIVE VIEW, no copies, and the granting space's custody
   │  permits key release? (client-first spaces cannot open windows)
   │        → GUEST ACCESS, the window.                     (§spec-8)
   │
   ├─ Copies wanted or required, receiver-side reactivity, offline
   │  independence, or an E2E/client-first space on either end?
   │        → CONSENTED FEED, the copy.                     (§spec-8)
   │
   └─ Tight, ongoing app-to-app coupling, both spaces, programmatic,
      possibly bidirectional?
            → TIER-2 SERVICE PRINCIPAL, dual capability tokens.
                                              (§spec-8, Routines, two tiers)
```

### One worked example each

**Same space: the household budget.** Partner is a member with the
`editor` role; the profile maps roles to ops. Salary detail is a guarded
entity: `access read` walks to an ACL the partner isn't on, or an op
family at two disclosure levels, exposing the narrower one to their role.
The pin that keeps this honest: a space can withhold an op or expose a
narrower one. It can never attach a hidden filter that makes the *same* op
silently mean less (§spec-8 "Access model").

**Guest window: the accountant at tax season.** Your finance space grants
`taxSummary` and `std.transactions.list` via an ACL row naming the
accountant's principal (invite → accept → binding; ids exchange only on
accept). They call *your* space's ops under their own identity; every read
is attributed to them in your chain. Ops exposed to guests must be
guard-bearing. The compiler rejects exposing an unguarded op to a guest,
so over-sharing is a compile error, not an incident. In May you revoke: the
window closes at their next call. Honestly stated: revocation stops
*access*, not *memory*. Their client may hold what it already fetched.

**Consented feed: the budget snapshot to a partner's own space.** A
standing routine replicates a *named query's output*, say `monthlySpend`
(so the shared surface has a manifest, and consent names exactly those
fields), into the partner's space. One-way; both ends consent. The copies
land as ordinary writes on the receiver's chain (`source:feed-monthlySpend`),
which means **the receiver's own triggers and rules fire on them**.
Cross-space reactivity becomes local reactivity. This also works out of a
client-first space (computed on the sharer's device, best-effort,
device-gated), which windows structurally cannot. Revoking stops the feed;
already-copied rows stay, and the consent page said so up front.

**Tier-2 service, meal planner ↔ grocery app.** A service principal holds
capability tokens for both spaces, each consented by its own space with an
explicit op allowlist and declared egress. It reads registered queries from
one, writes registered ops into the other, and appears in both chains as
itself. This is the only automated bridge besides feeds. Routines and
triggers are space-local by design.

### Summary table

| | Same space (acl/guards) | Guest window | Consented feed | Tier-2 service |
|---|---|---|---|---|
| **Freshness** | immediate; `?live` native | live at each call; `?live` works | feed cadence, copies lag | whatever the service polls/subscribes |
| **Where data lives** | the one space | stays in the granting space | receiver owns copies outright | each space keeps its own; service sees both in transit |
| **Custody constraints** | one shared dial for all members | requires granting space's key-release path, **unavailable from client-first spaces** | works even from client-first (computed on sharer's device) | both spaces' tokens disclose scope; service is an external principal |
| **Revocation semantics** | profile/ACL change, immediate, audited command | window closes at next call; fetched mirrors persist | future copies stop; existing copies stay (stated at consent) | revoke either token; each side independent |
| **Who audits** | the space's chain, one place | granting space's chain, guest attributed as themselves | both chains: sharer's feed reads, receiver's `source:feed-*` writes | both chains, actor = the service principal |
| **Reach for it when** | parties naturally co-own custody & history | live read-only-ish visibility across a boundary | independence, reactivity, offline, or E2E on either end | ongoing programmatic coupling between apps |

One mnemonic carries the whole section: **window vs copy** (§spec-8). If
neither fits, you're either inside one space (use guards) or building an
integration (use a service principal).

---

## C. Reporting & aggregation recipes

Ordo's pipeline covers grouped aggregation well and deliberately refuses
the window-function family (§spec-11: running totals, prev/next,
rank-as-value and moving frames are declined, with the design preserved for a
future v2). This section is the honest cookbook: what to write for each
common reporting need, and where the language's edge genuinely is.

Working schema for the recipes (fields deliberately open, not sealed; see
the cost note at the end):

```
entity accounts { name: text }
entity payees   { name: text }

entity transactions {
  account -> accounts
  payee   -> payees?
  date:    date = today
  amount:  money                 # signed: expenses negative
}
```

### C1. Monthly rollup: `group month()`

```
op monthlySpend =
  transactions(amount < 0)[amount]
  group month(date) { spent: sum amount }
  sort month
```

Date-part functions are bucketing keys (`year month week day hour`,
§spec-3 "Expression functions"); `month(date)` yields a sortable scalar
(`2026-07`). Group keys take expressions, so no projection gymnastics.

### C2. Latest-per-key: `take[keys] 1`

```
op lastActivity =
  accounts[name]  transactions[date, amount]
  sort -transactions.date
  take[accounts.id] 1
```

The greatest-n-per-group idiom: sort, then keep N per bucket. Ranking is
the pipeline's current sort; determinism is total (implicit id tiebreak).
Remember the outer-preserving join: accounts with no transactions survive
with null child fields. Filter them or handle the nulls (see `traps.md`).

### C3. Running balance: paged `reduce` + client accumulation

Running totals are **not server-expressible** (§spec-11). The blessed shape
is: one scalar for the opening value, one reproducible page of rows, and a
trivial client fold. Shown in full because it's the pattern people doubt:

```
op opening($account: id, $before: date) =
  transactions(account == $account and date < $before)
  reduce { opening: sum amount }

op statement($account: id, $from: date, $to: date) reproducible =
  transactions(account == $account and date >= $from and date <= $to)
    [date, amount, payee.name]
  sort date
```

`reduce` is a group with no key and returns exactly one row (`sum` of an empty
set is 0, so a fresh account opens at zero, typed and correct). The
`reproducible` flag forbids `since` in the body: a statement must mean the
same thing every time it renders (§spec-4 "since and reproducibility").

Client side (SDK sketch; exact decimal, never float):

```ts
const [{ opening }] = (await ordo.opening({ account, before: from }).fetch()).rows;
let bal = money(opening);
for await (const page of ordo.statement({ account, from, to }).pages()) {
  const lines = page.rows.map(r => ({ ...r, balance: (bal = bal.add(r.amount)) }));
  render(lines);
}
```

The screen that shows a running balance already holds the rows in order;
the fold is three lines, and the server never grew a window-function
planner for it. That trade is deliberate.

### C4. Leaderboard / top-N: `group` + `sort` + `take`

```
op topPayees($from: date) =
  payees[name]  transactions(amount < 0 and date >= $from)[amount]
  group payees.id, payees.name { n: count, spent: sum amount }
  sort spent                    # expenses negative: most spent sorts first
  take 10
```

Group by id *and* name: id so same-named payees never merge, name so the
row is displayable (per-row values ride the group key; bare fields inside
`{ }` don't parse). Rank-as-a-*value* ("show each row's rank number") is
window-family and declined. The client numbers the rows it renders.

### C5. Baseline comparison: `let` + `reduce`

```
op bigSpends =
  let baseline = transactions(amount < 0) reduce { a: avg amount }

  transactions(amount < 3 * baseline.a)[date, amount, payee.name]
  sort amount
```

A `let` ending in `reduce` is referenceable as a scalar (§spec-5); anything
else it's a joinable source (group-key provenance makes `let` outputs join
back like declared refs; see "Let outputs as sources"). This pair replaces
most correlated-subquery habits.

### C6. Materialized state: routines writing durable series

When a derived series must be *durable* (dashboards over years, values
you'll join against, history that must survive schema-era changes), don't
recompute it per query. Materialize it with a rule (§spec-11 home #3):

> **Built since 2026-10-08 (checklist 12a).** `routine` compiles and runs with `trigger` inside it, and `each ->` bodies are built; see `howto-routines.md`. A block that reads `$trigger.ids` stops at `$trigger`, which is not built.

```
entity dailyBalances {
  account -> accounts
  day:     date
  balance: money
  unique (account, day)          # idempotent snapshots; `unique` is always
                                 #   space-scoped, never deployment-wide
}

routine balanceSnapshot in finance {
  trigger every day 00:15Z
  run($today: date):
    accounts[id]  transactions[amount]
    group accounts.id { bal: sum amount }
    each ->
      set dailyBalances { account: accounts.id, day: $today, balance: bal }
}

op balanceHistory($account: id, $from: date) =
  dailyBalances(account == $account and day >= $from)[day, balance]
  sort day
```

Why this is better than a cache: the writes are **attributed** (actor:
the routine), **revertible** (ordinary command groups), **idempotent**
(the unique `(account, day)` key makes re-runs converge-or-create and trigger
redelivery is a no-op), and it is **clock-woken**, so guards pass and it sees
every account. The bound that matters here is its derived scope manifest (§D,
"Routines are bounded by their trigger"). The same rule written as a
`when …` listener would see only rows reachable from `$trigger.ids`. And the
result is **queryable as plain fields**:
`balanceHistory` is a boring `sql`-class op over an ordinary entity. This is
also the sanctioned answer for "per-key baselines" and anything else tier-1
query expressiveness can't produce in one pipeline.

For the narrow case of "sum the numeric fields of an owned subtree"
(receipt totals, project budgets), check `std.E.rollup` first
(`std-library.md` §4). It may already exist for your entity, derived.

### C7. Comparing two independently grouped aggregates: quorum, thresholds, "fewer than half"

This is the recipe that was missing, and its absence cost two real builds their
central feature. The shape is everywhere once you notice it:

> *"clubs where **fewer than half** the members have finished the book"* ·
> *"projects where **more tasks are blocked than open**"* · *"proposals that
> reached **quorum**"* · *"accounts whose refunds exceed **10% of** charges"*

Every one of them is **two counts, grouped independently, then compared**. The
first instinct is one pipeline, and it is a compile error, and the error is correct, because
`memberships` and `readingProgress` are two independent many-sides hanging off
`clubs`, and joining them in one pipeline multiplies the rows (the fan-out
guard, `traps.md`). The second instinct, give up and do it in the client, is
the one that lost the feature.

Working schema for this recipe:

```
entity clubs { name: text }

entity memberships {              # who is in the club
  club   -> clubs    owned
  member -> members  owned
  unique (club, member)           # space-scoped, like every `unique`
}

entity readingProgress {          # who finished the book
  club     -> clubs    owned
  member   -> members  owned
  finished: bool = false
  unique (club, member)
}
```

The wrong version, so you recognize it:

```
op stalledClubs =                                       # ✗ COMPILE ERROR
  clubs[name]  memberships[id]  readingProgress[id]     #   two many-paths
  group clubs.id, clubs.name { members: count, done: count }
  | done * 2 < members
```

The version that compiles: **one `let` per aggregate, joined back, then
compared**:

```
op stalledClubs =
  let memberCount = clubs[id]  memberships[]
                    group clubs.id { members: count }

  let doneCount   = clubs[id]  readingProgress(finished == true)[]
                    group clubs.id { done: count }

  clubs[name]  memberCount[members]  doneCount[done]
  | coalesce(done, 0) * 2 < coalesce(members, 0)
  sort name
```

Five things are load-bearing, and skipping any one of them is how this goes
wrong:

1. **One many-path per `let`.** Each `let` is its own pipeline, so each
   satisfies the fan-out guard on its own. Nothing multiplies, because the two
   many-sides never meet in one pipeline (§spec-5 "Fan-out guard").
2. **The join back is by provenance, not by hand.** A `let`'s group-key columns
   remember where they came from (here both are `clubs.id`) and match
   inference connects that provenance to the `clubs` source exactly as if it
   were a declared ref (§spec-5 "Let outputs as sources"). You write no join
   condition. If two `let`s could attach in more than one way, `match on <col>`
   disambiguates; group both `let`s on the **same** key expression and the
   ambiguity never arises.
3. **The joined-back `let`s are not many-paths.** Each produces **at most one
   row per club** (that is what `group clubs.id` guarantees), so listing two of
   them alongside `clubs` is a to-one join, and the final pipeline has zero
   many-paths. This is exactly why the pattern is legal and the one-pipeline
   version is not.
4. **An empty group reads 0; a missing `let` row reads null.** Each `let`
   here starts from `clubs[id]`, and the multi-source form is
   outer-preserving, so a club nobody has started still gets a row in
   `doneCount`, with `done` = 0 (the spec: "aggregates see an empty bucket
   (`sum` 0, `count` 0)"). `avg`/`min`/`max` of the empty bucket are null. A
   `let` that starts from the child (`readingProgress[club] group club {...}`)
   has no row for a club with no children, and the join back hands you `null`.
   Then two-valued logic bites: every ordering comparison against null is
   **false**, so that club silently drops out. Same trap as the never-watered
   plant (`traps.md`, "Outer-preserving joins"). Here `coalesce(done, 0)` is
   defensive, and costs nothing; it is what you need the moment a `let` is
   keyed from the child side or aggregates `avg`/`min`/`max`.
5. **Compare by multiplying, not by dividing.** `done * 2 < members` is exact
   integer arithmetic. `done / members < 0.5` drags in division, which yields
   `decimal` at the pinned scale (§spec-2) and is a typed error when `members`
   is 0 (an empty club, which is precisely the row you were reporting on).
   Cross-multiply and the whole class of problems disappears.

**Thresholds other than half** are the same shape with the ratio moved into a
parameter, still without dividing:

```
op belowQuorum($pct: int) =                      # $pct = 60 means "under 60%"
  let memberCount = clubs[id]  memberships[]
                    group clubs.id { members: count }

  let doneCount   = clubs[id]  readingProgress(finished == true)[]
                    group clubs.id { done: count }

  clubs[name]  memberCount[members]  doneCount[done]
  | coalesce(done, 0) * 100 < $pct * coalesce(members, 0)
  sort name
```

**Two aggregates over the same entity** work the same way, and are the reason
"one entity once per pipeline" is not the obstacle it looks like. The
restriction is *per pipeline*, and each `let` is a pipeline:

```
op refundHeavyPayees =
  let charges = payees[id]  transactions(amount < 0)[]
                group payees.id { charged: count }

  let refunds = payees[id]  transactions(amount > 0)[]
                group payees.id { refunded: count }

  payees[name]  charges[charged]  refunds[refunded]
  | coalesce(refunded, 0) * 10 > coalesce(charged, 0)
  sort name
```

**Both sides can be sums as well as counts.** `group clubs.id { pledged: sum
amount }` against `group clubs.id { paid: sum amount }` compares two money
totals with no division and no rounding anywhere (§spec-2: `+ - *` on money are
exact; only division leaves `money`).

**One thing to get right before you ship a quorum rule: `count` counts rows,
and a row may be a group.** If the thing you are counting is a `members` ref or
an acl link entity, one row can stand for a whole team, so "half the members"
becomes half the *rows*, and the quorum disagrees with anyone who counts heads.
Three options, in increasing order of exactness:

- **Keep quorum-bearing membership entities person-only** and `count` honestly.
  Simplest, and usually right.
- **`sum member.memberCount`**: every `members` row carries a read-only
  `memberCount` (1 for a person, the group's current size for a group), so
  `group clubs.id { members: sum member.memberCount }` counts humans instead of
  rows with no extra machinery. Note the stated limitation: it is a **cheap
  upper bound**, because a principal reachable both directly and through a group
  is counted once per path.
- **Exact distinct headcount**: a two-source pipeline over `groupMembers` with
  a `distinct` step, which is expressible because `groupMembers` is an ordinary
  queryable entity and groups cannot nest (see `definition-language.md` §8,
  Groups, for the pattern). Reach for it where the number is billable or
  binding.

Whichever you pick, pick it deliberately: this is the line where a reporting
recipe quietly becomes an authorization statement.

### C8. The boundary, stated plainly

**Declined, and staying declined for v1 (§spec-11):** running totals,
prev/next row, rank-as-value, moving windows/frames, server-side. The
three blessed homes are: client compute over fetched rows (C3's fold),
paged `reduce` + accumulation (C3), and rule-materialized state (C6).
If a need fits none of the three, treat it as a signal to rethink the model.

**Cost honesty:** these recipes assume open numeric fields. Seal `amount`
and every aggregate over a date range becomes host-side with no provable
bound, `unbounded-host`, which fails registration unless you acknowledge
`unbounded` explicitly (§spec-5 "Cost classes"). In aggregate-heavy
domains, the sealing decision *is* a reporting decision: keep the columns
you aggregate open (skeleton-tier), or budget for acknowledged host passes,
or materialize (C6) into open fields and seal only the sources.

**Sometimes Ordo is the wrong tool.** Exploratory BI,
ad-hoc slicing by analysts, ML feature pipelines, heavy OLAP over millions
of rows: Ordo's closed grammar, per-op registration, and custody model are
exactly wrong for that workload, on purpose (no free-form query text at
runtime is principle §spec-1.7). The supported path
out is **export to a warehouse**: the whole-space export bundle for
one-shots (§spec-8 "Exit ops"), or a standing tier-2 exporter. The exporter
is a service principal that reads registered queries (or the command-log via
`std.E.history`) and loads DuckDB/BigQuery/ClickHouse, with its egress named
on the consent page and every read attributed. Analyze there; keep Ordo as
the system of record with the audited door. Using the definition language
as an OLAP engine fails first at the `unbounded` markers, as designed.

### C9. Where an invariant lives: `check`, or a maintained counter

The recipes above are about *reading*. The mirror-image decision on the write
side is where a rule that must always hold gets enforced, and Ordo gives it two
homes with a hard line between them.

**`check`: this row, and the rows it points at.** A `check` compiles to a
constraint the database enforces **inside the write's transaction**, and it may
walk **outgoing refs** from the row being written. Many→one, the same direction
dot-paths always go, at any depth:

```
entity tickets {
  customer -> customers
  assignee -> members
  check assignee.org == customer.org
}
```

That covers a class of rule that otherwise needs a trigger or a
denormalized shadow column: *the assignee must belong to the customer's
organisation*, *a line's currency must match its invoice's*. Both of the usual
workarounds drift; the `check` cannot, because it is evaluated against the live
parent on every write.

**A maintained counter: anything about a collection.** `check` deliberately
cannot aggregate, count, or test existence over a collection. Those are
questions about *other rows*, not about this row's neighbours, and answering
them per-write is how write latency stops being predictable. Model them as a
counter field on the container, which *is* row-scoped:

```
entity teams {
  seats: int
  used:  int = 0
  check used <= seats
}
op join($team: id, $member: id) =
  set teams   { id: $team, used: used + 1 }
  set members { … }
```

One atomic group: the increment and the insert land together, the `check` fires
against the incremented row, and a violation aborts the whole group. The seat
never oversells. This is also why the language has no `require`-style op
precondition: the invariant belongs on the shape, where *every* write path meets
it, and not on the one op whose author remembered to guard it.

**The decision rule:** if you can state the invariant as a comparison between
this row's columns and the columns of rows it *points at*, it is a `check`. If
stating it needs the word "any", "all", "how many", or "no other", it is a
counter (plus a `check` on the counter) or it is rule-tier (a routine
maintaining durable state, C6).

---

### C10. Hot fields: what the command log preserves forever

A field rewritten on every interaction (a running summary, a view counter, a
`lastSeenAt`, a rolling score) writes a command **every time**, and the log
keeps every one until pruning. Reads never slow down (state and history are
different objects in Ordo: a row edited ten thousand times reads exactly as
fast as one edited once), but the log, the archive, and that row's
`std.E.history` grow in proportion to churn and not to information.

Ordo does not pay a read cost for this. The storage cost remains, so model for
it:

| Shape | Cost | When it is right |
|---|---|---|
| Ordinary field | one command per write, every intermediate value kept forever | the value's *history* is the point (a price, an assignee, a status) |
| Ordinary field, written coarsely | fewer commands | debounce or round at the write site: `lastSeenAt` to the hour rather than the second is the same product with a hundredth of the log |
| **Derived state** (a routine writes it into its own entity) | the *rows* prune under `policy retention`; the commands that wrote them still accrue, so this bounds the table, not the log; the real saving is that the churn no longer rides a wide business row | running summaries, rollups, scores, anything recomputable |
| Not stored at all | none | anything a query can answer at read time |

**The rule of thumb**: before adding a field that changes on every
interaction, ask *"would I want to read this field's value from eighteen
months ago?"* If the answer is no, it is derived state, not a field, and its
every intermediate value should not be preserved in an attributed,
hash-chained record built for exactly the opposite purpose.

**Converge does not help here, and an earlier version of this section claimed
it did, and was wrong in the expensive direction.** A converge *mutates* only the
fields it describes (`definition-language.md` §6), but the command it records
carries **both full images**, before and after
(`security-logging-design.md` §7; enforced in `ordo-commandlog`'s
`Command::validate`, where `Converge` requires both). So bumping one counter on
a row with a large text body writes that body **twice** (once in each image)
every single time.

That strengthens the guidance above: a hot field on a *wide* row
is the worst case in the system, because the cost is proportional to the row's
size rather than to the field's. If a row has both a large field and a
frequently-changing one, they belong in different entities.

## D. Authorization modeling

The question this section answers: given a schema, *where* does "who may see
this row" live? Ordo offers four mechanisms and they are not alternatives.
They stack, and picking the wrong layer is the most common modeling error.

### The four layers, in the order you should reach for them

| Layer | Grain | Lives in | Reach for it when |
|---|---|---|---|
| **Profiles** (role → ops) | which *operations* exist for you | runtime state, per space | Always. Every space has one; it is the door. |
| **`acl (…)`** on a root entity | which *rows* of a tree you may touch | the shape; rows in the derived link entity | The app has containers people are added to: projects, clubs, budgets, boards. |
| **`access read` / `access write`** guards | which rows, by any declared relationship | the shape | The rule isn't membership-in-a-container: ownership (`member == $caller`), a status field, a two-hop reachability. |
| **Groups** | who counts as "you" | runtime state, per space | Departments, teams, org hierarchy: anywhere naming people one at a time would drift. |

Rules of thumb, in one line each:

- **Container ⇒ `acl`.** If a human would say "add Sam to the project," the
  project is a container; `acl` derives the link entity, the guards, and the
  grant/revoke ops. Do not hand-roll it.
- **Relationship ⇒ guards.** If the rule is a fact about the row itself
  ("mine", "assigned to me", "public"), write `access read` with a
  `$caller`-anchored walk. Hand-rolled membership tables that duplicate `acl`
  are the reinvention to avoid.
- **Owned children inherit; link entities do not.** A child `owned` by an
  `acl`-bearing root is guarded automatically; write nothing. A link entity
  (two owned refs) never inherits: it must declare its own guards, and the
  omission is the classic hole.
- **Both verbs, always.** `access read` without `access write` is a compile
  error precisely because "read-guarded, write-open" reads as secure and
  isn't. If writes really are open, spell it (`access write open`) so a
  reviewer sees a decision instead of an absence. (`open` is the word; `true`
  does not parse in that position.)
- **Groups resolve in the access layer only.** A group is usable anywhere a
  principal is (an acl row, the far side of a guard walk, a grant), and
  removing someone from the group removes their access everywhere it was
  granted. But **`== $caller` never expands a group**: it is row equality
  against the caller's own `members` row. So `tasks(assignee == $caller)` does
  **not** return tasks assigned to a team you belong to, and `count` over an
  acl link entity counts a group as **one**. Model group-addressed work as a
  guard or an acl row (which do expand) rather than as an `== $caller`
  condition in a body (which does not), and never build a quorum on a row
  count that might contain groups (see C7's closing caveat).
- **Never encode people in shapes.** The rule is code; the facts are rows.
  `access read project.projectMembers(member == $caller)` names no one, and
  membership changes are ordinary audited writes that take effect on the next
  call, including invalidating live subscriptions.

### The bootstrap pattern

The first access row is the one nobody can grant themselves. Ordo closes it
in three ways, and knowing which applies saves an afternoon:

1. **A space member creates the row** ⇒ the creating principal receives the
   top-level access row in the *same command group* as the create. Nothing to
   write; this is why `acl` is one line. The mechanism is a narrow exemption,
   not a hole: **a creating write on an acl-root entity is exempt from the
   derived write guard** (there is no access row to walk to yet), and the
   creator's grant lands in the same group. The exemption is available to a
   space **member**, not to a guest. A guest holds a members row with a null
   role so acl rows can name them, but they cannot mint an acl root and become
   its admin.
2. **A routine, source, or system creates it** ⇒ **no** auto-grant is minted
   (there is no human to grant to, and inventing one would be a lie in the
   chain). The row exists, access-less, until someone with authority opens it.
3. **Always available**: a **space owner may administer any `acl` row in
   their space**, regardless of level. This is the universal escape. It
   covers case 2, covers "the last admin left," and means an `acl` deadlock
   is not reachable. The override is checked **at the door**, with the space
   binding, not inside the row walk, so it is one decision on one surface,
   logged as the power it is.

Corollary: a space always needs at least one owner, and
owner is membership state, not `acl` state. Space creation mints it.

**Seeding the second person is an ordinary write.** There is no `grant`
statement in the language. The derived link entity is a real entity, so
`set <entity>Access { … }` is the whole mechanism, subject to that entity's own
derived write guard like any other write:

```
op assignProject($project: id, $rev: int, $owner: id) =
  set projects       { id: $project, rev: $rev, status: "active" }
  set projectsAccess { project: $project, member: $owner, level: "admin" }
```

Statements in one op are one atomic command group, so the project never changes
hands without the access row landing with it. The derived `grantProjectsAccess`
op exists too and is the right surface for ordinary permission management,
gated at `admin`.

**Seeding access for a row created in the same op: bind the create.** A `set`
whose derived mode is CREATE may be bound with `let`, and the binding is a row
descriptor (`id`, `rev`, and the fields that statement wrote) usable in later
statements of the same op:

```
op startProject($name: text, $coLead: id) =
  let p = set projects       { name: $name, status: "active" }
  set projectsAccess { project: p.id, member: $coLead, level: "admin" }
```

The create is guard-exempt (G10a), the dependent write is not, and both land in
one atomic command group. Only creates bind: `let` on a converge, a CAS write
or a bulk write is `unbindable_write`, because those either address a row whose
id you already hold or touch an unknown number of rows.

Three older shapes remain useful and are sometimes the better model: (i) let the
intended owner create the row and take the auto-grant, when the creator *is* the
owner; (ii) create with a unique external key and have a listener routine seed
access from `$trigger.ids`, which is the only shape available for **ingested**
rows since a source principal has no members row to grant to; (iii) grant by id
in a second op, when the grant is a separate human decision.

### Routines are bounded by their trigger, not by guards

A tier-1 routine is the domain acting on itself, and the bound on what it may
touch follows **where its wake-up came from** (§8):

| Woken by | Bound |
|---|---|
| **clock** (`every day 02:00Z`) | guards pass. Nobody outside chose the moment, so a clock rule can never be steered |
| **listener** (`when finance.transactions.ingested`) | the run may read and write **only rows reachable from `$trigger.ids`**. Everything else is invisible exactly as a guard would make it |

The reason is the confused-deputy problem: a listener
fires on writes an **outsider** made (`ingested` from a source principal, `fed`
from another space's feed). If such a run were globally guard-exempt, an
outsider would be choosing when a guard-free process runs and, via
`$trigger.ids`, which rows it starts from, steering work on rows they could
never touch themselves. So only the clock is granted the exemption.

Model accordingly:

- **Write listener bodies against `$trigger.ids` deliberately**, not
  incidentally. `id in $trigger.ids` is the row bound and the readability aid
  at once.
- A listener that must legitimately range wider says so on its `run` header
  with `unguarded`, one greppable word and a decision somebody made on purpose.
  If you find yourself reaching for it on a listener woken by ingested or fed
  data, re-read the paragraph above first.
- The **derived scope manifest** is unchanged and orthogonal: it bounds *which
  entities and fields*, and never bounded rows. Conflating the two was the
  original error.
- Do **not** create a "system user" row and grant it access to everything. That
  pattern widens the blast radius for no gain and hides the routine's real
  bound.

Tier-2 services are the opposite case: they *are* outsiders, they hold
capability tokens, they get an ordinary members row, and guards apply to them in
full. The distinction follows the trust boundary.

### Choosing per-space visibility

A space tunes what a member sees by **which ops it exposes to them**, never by
altering an op's meaning. So model visibility as a *family of ops* at
different disclosure levels (`myExpenses` / `teamExpenses` / `expenseSummary`)
and let profiles assign them. If you find yourself wishing you could "make
this op return less for this user," you want a second op, and per-row
narrowing inside one op is what `$caller` guards are for.

### The smell test

If a reviewer cannot answer *"who can read this row, and why"* from the shape
file plus one query against membership, the model is too clever. Ordo's
authorization is meant to be boring and greppable; that property is worth more
than any elegance you might gain by concentrating it somewhere else.
