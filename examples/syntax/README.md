# Syntax examples

One small `.ordo` file for each kind of syntax the compiler accepts today.
Every file passes `ordo check`. A last section, "Proposed, not built", shows
syntax that is ruled but does not compile yet.

## How to check a file

```
cargo build -p ordo-cli
target/debug/ordo check examples/syntax/10-writes-set-and-del.ordo
```

`ordo check` prints the entities, their guards and every op with its mode, cost
class and the fields it touches.

`ordo check` exits 1 when any op in the file cannot compile, and prints that op
as `DOES NOT COMPILE [class] reason` under OPS (checklist 25p). A file in this
directory passes when the exit code is 0. To check them all:

```
for f in examples/syntax/*.ordo; do
  target/debug/ordo check "$f" > /dev/null 2>&1 && echo "ok   $f" || echo "FAIL $f"
done
```

Syntax markers in `docs/definition-language.md`: **[settled]** agreed,
**[lean]** current recommendation, **[open]** undecided. A marker says whether
a design is agreed. It does not say whether the compiler reads it. The table
below and "What the compiler does not accept" say that.

## Index

| File | What it shows | Reference |
|---|---|---|
| [01-entities-and-fields.ordo](01-entities-and-fields.ordo) | Entities, field types, required / defaulted / optional, typed id prefix | DL 2 "Types", 3 "Syntax" |
| [02-refs-and-ownership.ordo](02-refs-and-ownership.ordo) | Refs, `owned`, link entities, dot-paths, delete cascade | DL 3 "Refs", "Ownership" |
| [03-unique-index-check.ordo](03-unique-index-check.ordo) | `unique`, `unique (a, b)`, `index`, `check`, atomic counters | DL 3 "Row invariants", "Indexes" |
| [04-enum-like-values.ordo](04-enum-like-values.ordo) | A closed set of words with `check` (the `enum` type is not built) | DL 2 "Types" [lean], 3 "check" |
| [05-sealed-fields.ordo](05-sealed-fields.ordo) | `sealed`, `indexed`, `unique`, `normalize(lower)`, `refinable` | DL 3 "Sealed fields", 5 "Cost classes" |
| [06-reads-filter-sort-take.ordo](06-reads-filter-sort-take.ordo) | Read form, projection, conditions, `sort`, `take`, parameters | DL 4, 5 "Core form", "Other steps" |
| [07-aggregates.ordo](07-aggregates.ordo) | `group`, `reduce`, aggregates, bucket filter, `take[key]` | DL 5 "Aggregation" |
| [08-joins-let-append.ordo](08-joins-let-append.ordo) | Inferred joins, `match any/none`, `match <ref>`, `let`, `append` | DL 5 "Match", "Let outputs", "Fan-out guard" |
| [09-composition.ordo](09-composition.ordo) | Ops that read other ops, bound parameters, post-take filters | DL 5 "Named-query composition" |
| [10-writes-set-and-del.ordo](10-writes-set-and-del.ordo) | `set` and `del`: create, converge, CAS, upsert, bulk, cascade | DL 6, docs/writing-data.md |
| [11-computed-writes.ordo](11-computed-writes.ordo) | `used + 1`, `$caller`, `cond`, `round`, `let x = set` | docs/writing-data.md 2, DL 3 "Naming a freshly created row" |
| [12-access-guards.ordo](12-access-guards.ordo) | `access read` / `access write`, `open`, inheritance | DL 3 "Access guards" |
| [13-acl-levels.ordo](13-acl-levels.ordo) | `acl (read, write, admin)` and what it derives | DL 3 "`acl`" |
| [14-flags-unguarded-unbounded.ordo](14-flags-unguarded-unbounded.ordo) | Op flags `unbounded`, `unguarded`, `reproducible` | DL 3 G14, 5 "Cost classes", 13 |
| [15-trees.ordo](15-trees.ordo) | `tree`, `depth`, `under`, `above`, `subtree` | DL 3 "Trees" |
| [16-files-blob-and-uri.ordo](16-files-blob-and-uri.ordo) | `blob`, `blob inline`, `blob private`, `uri` | DL 2 "Content types" |
| [17-members-and-groups.ordo](17-members-and-groups.ordo) | The platform `members` entity, `$caller`, groups | DL 3 "members", "Groups" |
| [18-expressions-and-functions.ordo](18-expressions-and-functions.ordo) | Arithmetic, dates, text, `coalesce`, `cond`, durations | DL 2 "Literals", 3 "Expression functions" |
| [19-json-documents.ordo](19-json-documents.ordo) | `json`, `json?`, defaults `{}` and `[]`, `sealed`, whole-document writes, `is null`, filtering on a path inside a document | DL 2 "JSON documents" |
| [20-routines.ordo](20-routines.ordo) | `routine` with `trigger every`, `every day 09:00Z`, `when ... debounce`; `$now`, `$today`; `each ->` bodies; the change bound | DL 7, 8, docs/howto-routines.md |
| [21-public.ordo](21-public.ordo) | `public` on a field, `public` / `public*` on an entity, the `public` op flag, and what is refused | DL 3 "`public`", docs/howto-public.md |

DL is `docs/definition-language.md`.

## What each file shows

### 01 Entities and fields

An entity is a table. Every field is required (`name: text`), defaulted
(`stock: int = 0`) or optional (`sku: text?`). The type set is closed: `text`,
`int`, `money`, `bool`, `date`, `datetime`, `duration(unit)`, `id`, plus `blob`
and `uri` (file 16) and `json` (file 19). `id` and `rev` exist on every entity and are never declared.
`text(120)` bounds a field or a parameter to 120 characters. A longer
parameter is `bound_violation`, and a longer field value is `check_violation`.

```
entity products (key: prod) {
  name:    text(120)
  stock:   int = 0
  sku:     text?
  restock: duration(d) = 14d
}
```

### 02 Refs and ownership

`field -> entity` declares a relationship once, on the child. A plain ref
blocks deleting its target. `owned` makes the row part of its parent, so the
parent's delete takes it too. A link entity has two `owned` refs.

```
entity orderLines {
  order   -> orders  owned
  product -> products
}
op cancelOrder($order: id) = del orders { id: $order }
```

### 03 Unique, index and check

`unique` on a field or on a pair, `index` on a field or `index (a, b)` on a
pair for lookups, and `check` for row invariants the database enforces on
every write. A ref field is indexed without being asked.

```
check used <= maxUses
op redeemCoupon($coupon: id) = set coupons { id: $coupon, used: used + 1 }
```

### 04 A field with a closed set of values

`enum(...)` is [lean] in the reference and does not compile. The idiom that
does: a defaulted `text` plus a `check` that lists the words.

```
status: text = "open"
check status == "open" or status == "doing" or status == "done"
```

### 05 Sealed fields

`sealed` encrypts a field at rest. `sealed indexed` adds a blind index for
equality lookups. `normalize(lower)` makes case variants one value.
`refinable` marks a sealed field a caller may narrow by equality. Every read
takes a refinement on its open fields with no mark. `std sealed` lets the derived std ops see the
sealed fields; without it they see the open fields only.

```
email: text? sealed indexed unique normalize(lower)
std sealed
```

### 06 Reads: filter, sort, take

`entity(conditions)[projection]`, then one step per line. `and` and `or` never
mix without parentheses. A leading `-` on a sort key means descending.
`$today: date filled` is a parameter the generated client fills from the
caller's clock when the caller leaves it out.

```
tasks(status == "open")[title, points, project.name]
sort -points, title
take 5
```

### 07 Aggregates

`group key { ... }` gives a row per key. `reduce { ... }` gives one row. A bar
after a group filters the groups.

```
customers[name]  orders[total, status]
group customers.name { orders: count, spent: sum total }
| orders >= 2
```

### 08 Joins, let and append

Sources listed side by side join through declared refs. `match none` and
`match any` filter by absence or presence. `let` names an intermediate result.
`append` stacks two sources by column name.

```
customers[name]
match none orders
```

### 09 Composition

A read op is a source for other reads. Dollars bind, fields filter.

```
cheapActive($cap: $cap)[price, vendor.name]
topTen(price < $cap)[name, price]
```

### 10 Writes: set and del

Two verbs. The mode comes from the statement and `ordo check` prints it.

| Statement | Mode |
|---|---|
| `set reviews { product: $p, stars: $s }` | `creates` |
| `set products(id == $p) { name: $n }` | `converges` |
| `set products { id: $p, rev: $r, price: $x }` | CAS, `converges` |
| `set products { sku: $s, name: $n, price: $x }` | `creates-or-converges` |
| `... rev: null` | `creates` (create-once) |
| `set products(active and price < $f) { active: false }` | `bulk` |
| `del products { id: $p }` / `del products(true)` | `deletes` / `bulk deletes` |

### 11 Computed writes

A field name on the right is the row's current value, read and written
atomically. `$caller` is the signed-in member. `let x = set ...` binds a created
row for the next statement.

```
set teams(id == $team) { used: used + 1 }
let team = set teams { name: $name, seats: $seats }
set members2 { team: team.id, who: $first }
```

### 12 Access guards

Guards live on the entity. A walk starts at `$caller` and follows refs. Both
verbs are required, and `open` is the explicit spelling of an open verb.

```
access read   project.projectMembers(member == $caller)
access write  project.projectMembers(member == $caller and canWrite)
access read   open
```

### 13 acl

One line derives the access entity (`boardsAccess`), the guards, and the grant
ops. Levels are ordered left to right. The creator gets the top level in the
same write.

```
entity boards { name: text   acl (read, write, admin) }
set boardsAccess { board: b.id, member: $teammate, level: "write" }
```

### 14 Op flags

`unbounded` says the work grows with the tenant. A keyset page counts as a
bound, so a plain listing needs no word; a many-side join does. `unguarded`
bypasses guards for the entity sources in this op's own body. An op that
composes an unguarded op repeats the word. `reproducible` refuses `since` and
`filled` (`reproducible_clock`).

```
op allTasksEverywhere unbounded unguarded = tasks[title, project.name]
```

### 15 Trees

A self-ref marked `tree` gets a path, a `depth`, cycle checks and a depth cap.

```
parent -> categories? tree
categories(id under $root)[name, depth]
del categories { id: $cat } subtree
```

### 16 Files: blob and uri

A `blob` field is a reference to a stored file and must be optional. Upload goes
through the blob door (`PUT /blob/{entity}/{field}/{id}`), never through an op.

```
photo:     blob?
thumbnail: blob inline?
manual:    uri?
```

### 17 Members and groups

`members` is provided per tenant. A domain refs it (`owner -> members`) and
reads `displayName`, `role`, `kind`, `memberCount`.

```
notes[body, author.displayName, author.role]
projectTeams(project == $project)[member] reduce { seats: sum member.memberCount }
access read  projectTeams(member.groupMembers.group(member == $caller))
```

`group` is a reserved word. After a dot it is read as the field name, because
nothing else can follow a dot.

### 18 Expressions and functions

A closed list of pure functions: `abs round floor ceil mod least greatest`,
`year month week day hour dow`, `lower length trim`, `coalesce`, `cond`, and `+`
on text. `/` yields a decimal that needs `round`, `floor` or `ceil` before it is
stored.

```
tier: cond(total >= 1000 -> "gold", total >= 100 -> "silver", "bronze")
perUnit: round(total / qty, 2)
```

### 19 JSON documents

`json` holds one JSON document, stored as `jsonb` and carried on the wire as
JSON. The default is `{}`, `[]`, or `null` on a `json?` field. A write replaces
the whole document. A condition can look inside one with a path,
`settings.theme == "dark"`, typed by its other side. Sort, group and function
refuse a document (`json_opaque`). The door refuses a document
over `ORDO_JSON_MAX_BYTES` or `ORDO_JSON_MAX_DEPTH` as `bound_violation`.

```
entity pages {
  title:    text
  settings: json = {}
  draft:    json?
}
op saveDraft($p: id, $rev: int, $draft: json) =
  set pages { id: $p, rev: $rev, draft: $draft }
```

### 21 Public

`public` makes data and ops readable with no token. A field, an entity or an op
carries it. A public op may read public fields only, and the compiler refuses
anything else. `public*` on an entity line reaches the entities it owns. A
public write may only create rows. Others are refused (`public_write_not_create`,
`public_write_guarded`, `public_write_ref`).

```
entity plans {
  name:   text  public
  margin: money
}
op pricing public =
  plans[name]
  take 50
```

## Syntax that lives at call time, not in the file

These have no `.ordo` spelling. They are part of how an op is called.

**Paging.** Any read pages with an opaque cursor token, never with syntax.
`take` is the query itself ("the top 20 is the result"). The response carries
`next` while another page exists, and the caller sends it back as `$page` (a
query key on a GET) or in the `X-Ordo-Page` header. Absence of `next` is the
only end. Reads are labelled `cursor: stable` or `cursor: drifting` depending on
whether any authored write moves a sort key (DL 5 "Determinism",
docs/api-engine-design.md 8).

```
GET /op/openTickets                          -> { "rows": [...], "next": "<token>" }
GET /op/openTickets   X-Ordo-Page: <token>   -> the next page
```

**Refinement.** A caller can narrow a result with a structured, narrowing-only
envelope. It is data, never query text. Comparison words: `eq ne lt le gt ge in
is_null`. Every read accepts one, with no flag and no off switch. It applies
after the op's own `take`, and pages come back full. On a sealed field it works
only where the field is declared `sealed indexed refinable`, and only for
equality (file 05). `ordo check` prints each read's refinable fields; what the
door refuses, and why, is `docs/howto-cost.md`.

```
{ "$refine": { "where": [ { "field": "status", "op": "eq", "value": "open" } ],
               "pick":  [ "subject", "status" ] } }
```

**Liveness.** Every named read is live-capable. The caller chooses at call time:
`GET /op/openTickets?live=1` opens a server-sent stream that sends `invalidate`
and no data. The client re-reads through the normal op.

**Sharing.** Windows (guests), share links and feeds have no syntax in `.ordo`.
They are runtime acts by ops such as `std.space.share` and `std.space.shareLink`,
and the platform checks the ops they list against the program: read-only, not
`unguarded`, and guard-bearing. See `examples/concepts/04-sharing-a-board.ordo`.

## What the compiler does not accept

The reference marks these as settled, lean or illustrative. The parser reads
`domain`, `entity`, `op` and `routine` at the top level. Each line shows what
`ordo check` says.

| Construct | Reference | What `ordo check` says |
|---|---|---|
| `routine` whose `each ->` body reads `$trigger` (such as `$trigger.ids`) | DL 8 | `$trigger` is not built (`each ->` itself is built, file 20) |
| `when finance.transactions.ingested -> run x`, `every 5m -> run y` | DL 7 | refused with a pointer to the routine form (`trigger` inside a routine, file 20) |
| `principal`, `client`, `policy`, `channel`, `route`, `app`, `page` | DL 8 | `` `principal` is reserved and not implemented ... `` (one message per word) |
| `domain finance @ 2.1.0 { shapes ... ops ... }` | DL 8 "Domain package" | `expected a declaration (...), found {` (only `domain name @ version` parses) |
| `enum("open", "done")` | DL 2 [lean] | `unknown type enum, the type set is closed` |
| `$lines: list of {...}` and `set e each $lines {...}` | DL 2 [lean] | `unknown type list`; `expected { opening the field block, found each` |
| `restock: duration = 7d` (bare `duration`) | DL 2 "Types" [settled], owed | `expected ( , duration declares its unit`; spell `duration(d)` |
| `check assignee.org == customer.org` | DL 3 "check may walk refs" [settled], owed | `check_beyond_row`: a CHECK sees one row, the ref-walking form is owed |
| `products[photo.mime, photo.size]` | DL 2 "Content types" [settled], owed | `not_a_ref`: `photo` is not a ref, so `.mime` has nothing to walk to |
| `since 2h` on a `datetime` | DL 5 "since" [settled] | `unsupported`: sub-day `since` needs a datetime clock |
| `patients(diagnosis == $d)` on a sealed field with no index | DL 3 "Sealed fields" | `sealed_unsupported`: `diagnosis` is sealed: the database holds only its ciphertext, so it cannot filter, sort, aggregate or compute on it (the message goes on to name what works). The fixture `market.ordo` op `giftOrders` shows the same line |

## Findings: the reference and the compiler disagree

Found 2026-10-03 while writing these files (board `dl-spec-gaps`, checklist
27aj). The owner ruled: settled syntax the compiler refuses is a compiler debt,
and lean syntax that is unbuilt is marked "Not built yet" in the reference.
Each entry says where it stands on 2026-10-04.

### Fixed

1. **`ordo check` exited 0 when an op did not compile.** It now exits 1 and
   names the op (checklist 25p).
2. **`text(64)` on fields and parameters.** It parses on both. The door refuses
   a longer parameter. The write path refuses a longer field value, sealed or
   open. An open column also carries a `CHECK` on `char_length`. All three
   count characters. The door answers `bound_violation`, the field side
   `check_violation`. File 01.
3. **`$today: date filled`.** It parses, rides the artifact as `filled`, and
   stays required on the wire. The generated TypeScript client fills an
   omitted `date` with today on the caller's clock, and an omitted `datetime`
   with now. File 06.
4. **`std sealed`.** One shape line. With it the derived std ops include the
   sealed fields. Without it they see the open fields only. It shows in the
   artifact and in the `ordo check` shape summary. File 05.
5. **A hand-written group walk.** `member.groupMembers.group(member == $caller)`
   parses: a reserved word after a dot is a path segment. File 17.
6. **`reproducible` refuses `since` and `filled`**, class `reproducible_clock`.
   It looks in the op's reads, `let`s, filters and write conditions. File 14.
7. **`allocate` in a write's braces** is now `invalid_allocate`. Every read
   position already refused it.
8. **The `unbounded` gate.** This finding was half wrong. The gate does apply
   to open fields: an op with no row bound is refused without the word. A
   keyset page counts as a row bound (ruling 2026-09-05), so a plain open
   listing registers. The defect was on the sealed side. A paged sealed
   projection was classed `unbounded-host`, although the page bounds it as
   `take` does. It is now `bounded-host`. A projection that a `group` or
   `reduce` folds away still needs the word. DL 5 "Cost classes" now states
   the paging bound.
9. **The reserved-word list in DL 2** matches the lexer.
   `crates/ordo-syntax/tests/reserved_words.rs` fails if they drift
   (checklist 27ag).
10. **`docs/howto-custody.md`** no longer says a filter on an unindexed sealed
    field is allowed (2026-10-03).

### Settled and owed, and bigger than a parser fix

Each is marked "Not built yet" in DL with the reason.

11. **Bare `duration`.** It needs the two-component duration model in DL 2
    "Duration semantics". That replaces the engine's per-unit duration type
    (`audit-doc-contradictions.md` F4).
12. **A `check` that walks a ref.** It needs triggers on the declaring entity
    and on every entity the walk passes through (ruling 2026-09-05).
13. **Blob `.mime` and `.size`.** They need a join to the `$blobref` row and a
    host-side open of its sealed envelopes. Indexing them needs a blind index
    over that row.

### Undecided

14. **`refinable` (decided 2026-10-05).** Every read accepts refinement, with
    no op-level word and no off switch. `refinable` stays a mark on sealed
    fields only. Kept here so the numbering below holds.
15. **Which types `filled` may mark.** DL says `filled` is for
    execution-relative values. The client fills `date` and `datetime`. On any
    other type the parameter stays required. Whether the compiler should refuse
    `filled` on other types is not decided.

### Out of scope here

16. **Money literals** (`docs/howto-values.md` against DL). Ruled 2026-10-04 as
    a new design, checklist 27ak.
17. **A blind-index lookup that also projects a sealed field** gives
    `unresolved_blind_index` under `ordo check`. The plan needs key material
    only at runtime, so `ordo check` does not count it as a failure (25p).

## Proposed, not built

Everything below comes from `docs/plan-syntax-additions.md` (ruled 2026-10-03
except where its header says an ask is open). `public` and `public*` are built
since then and moved out of this list. The rest is not built. These are fenced
snippets and not `.ordo` files, because `ordo check` rejects them. The plan adds
5 more reserved words: `deployment`, `prev`, `settings`, `external`, `apikey`.
Create-only public writes are built (2026-10-11); see `docs/howto-public.md`.

### Built: `public` and `public*`

Not proposed any more. `public` on a field, an entity line or an op, and
`public*`, are built (2026-10-10). See [21-public.ordo](21-public.ordo) and
`docs/howto-public.md`. The `deployment` line in the blocks below is still
unbuilt.

### Built (public writes) and proposed (the rest): create-only public writes

Plan section 2. A contact form: anyone may write a row, nobody reads it back.
A public write may only create, and every text parameter states its bound.

```
entity contacts {
  name:     text
  email:    text  sealed
  message:  text  sealed
  received: datetime
  deployment
}

op contact($name: text(120), $email: text(254), $message: text(4000),
           $received: datetime filled) public =
  set contacts { name: $name, email: $email, message: $message, received: $received }
```

A converge, bulk or `del` in a public op is `public_write_mode`. The other path
is a form grant the deployment owner mints at runtime
(`std.deployment.formLink`), which never appears in a file.

### Proposed, not built: deployment entities

Plan section 3a. An entity whose rows live in the deployment's own space, with
no rows in any tenant. With no `access` clause, any signed-in person reads it.

```
entity announcements {
  title: text
  body:  text
  deployment
}
```

A pipeline that lists a tenant entity and a deployment entity is refused
(`cross_space_join`).

### Proposed, not built: `$tenant` guards

Plan sections 3b and 3c. A second reserved parameter, the row of the tenant the
request is bound to, and guard atoms that test facts about the caller or the
tenant. They compile to one constant per call.

```
entity subscriptions {
  tenant -> tenants unique
  plan:   text                     # "free" / "pro"
  deployment
}

entity betaNotes {
  title: text
  body:  text
  deployment
  access read   subscriptions(tenant == $tenant and plan == "pro")
                or $caller.role == "owner"
  access write  open
}
```

### Proposed, not built: apikey profiles

Plan section 4. The program declares the ceiling of ops a key may name and a
quota default. Keys are issued at runtime against the profile, by a tenant owner
for the tenant's customers or by the deployment owner. No quota line means
unlimited.

```
apikey storefront {
  ops   listProducts, getProduct, createOrder, orderStatus
  quota.requests = 100000 per 1d   tenant <= 100000 per 1d
}
```

Runtime: `std.space.issueKey`, `std.space.revokeKey`, `std.space.keys`,
`std.deployment.issueKey`.

### Proposed, not built: `prev`

Plan section 5. Says a renamed entity or field is the same thing as a name in
the previous version, so the plan renders a rename and not a drop-and-add. Plan
ask 1 (source or plan command line) is still open. It reaches one version back.

```
entity customers (key: cust, prev: clients) {      # was `clients`
  fullName: text  prev(name)                       # was `name`
  note:     text? prev(orders.shippingNote)        # moved here from another entity
}
```

Orphans (plan section 6) need no syntax: a field the program stops declaring is
kept, unserved and listed by the plan. `examples/concepts/05-version-2.ordo`
shows it by omission.

### Proposed, not built: the `settings` block

Plan section 7. One block per program sets the app developer's value for a
setting from Ordo's closed catalogue, and says which lower layers may change it
and by how much. Today these are deployment environment variables (`ORDO_*`).
Per-op settings (plan ask 13) wait for a later build.

```
settings {
  session.life            = 12h     tenant <= 12h   person <= 12h
  share.linkLife          = 7d      tenant <= 30d
  commandlog.undoHorizon  = 90d     tenant >= 90d
  admission.defaultRole   = "viewer"
  live.refetchPace        = 250ms   person >= 100ms
  public.writeRate        = 60 per 1h
}
```

A tenant owner changes an exposed value with `std.space.setSetting`, a person
with `std.person.setSetting`. Both are checked against the bounds above.

### Proposed, not built: external entities

Plan section 8. A generated, read-only translation of a table Ordo does not own.
`<-` maps a field to a column. Plan ask 12 (the `<-` mark) is still open.

```
external customers (key: cust) from "crm"."customers" {
  id              <- "customer_id"
  name:   text    <- "full_name"
  email:  text    <- "email"             public
  since:  date    <- "created_at"
  access read   region == $tenant.region
  access write  open
}
```

### Proposed, not built: `ordo plan --prune`

Plan section 6. A removed declaration is orphaned, never dropped. Dropping is a
choice on the plan, with a second confirmation that names each pruned field and
its row count.

```
ordo plan . --against prod --prune customers.fax --prune legacyNotes
```

`ordo prune` already exists and trims the command log. The plan flag lives under
`ordo plan`, so the two never share a command line.

## What was left out

- Reading `$trigger` in a routine body, the top-level `when ... -> run x` line, `principal`, `client`, `policy`,
  `channel`, `route`, `app`, `page`, the `domain { shapes ... }` package form
  and `bootstrap`: no `.ordo` file can hold them today (table above).
- `whole entity($id)` is a standalone anchored read, not an op body. The parser
  has an entry for it and `op x = whole p($id)` does not compile, so it has no
  file here.
- `std.*` ops are derived from the shapes, not written. `ordo check` lists them
  under OPS. `docs/std-library.md` describes them.
