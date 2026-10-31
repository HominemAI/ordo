# Named-Query Composition: Conformance Suite

Test specification for named-query composition (definition-language.md §5,
"Named-query composition"). Three parts: a per-component test matrix, a
testing-pipeline design, and a 120-case corpus with hand-computed expected
outputs against a fixed seed dataset.

Case kinds: **exec** (compile + run against seeded Postgres, compare rows) ·
**compile** (artifact-metadata assertions only) · **reject** (expected compile
error class) · **behavioral** (runtime protocol: pagination, `?live`).

---

## 0. Error classes (closed inventory)

Every reject case names one class. Classes are part of the compiler contract:
the harness asserts the class, not the message text.

**A class names what is wrong, never which pass noticed it** (ruling
2026-08-30, `corpus-rulings.md`). `ParseError`, `SchemaError` and `SemaError`
all carry a `code` drawn from this inventory, so the same defect keeps the same
class whether the grammar, the schema graph or the binder is the one that can
see it. The parser raises `parse_error` for text it could not read,
`bare_field_in_group` for a brace block naming something outside the closed
aggregate set, and `invalid_guard_walk` for a guard atom it read perfectly well
that R-G3 does not admit (`guards-conformance.md` §0).

| Class | Raised when |
|---|---|
| `parse_error` | text outside the closed grammar: synonym spellings, empty parens, mixed `and`/`or` without parens, brackets misuse. **Not** a non-boolean condition: the grammar reads that fine and the defect is its type, so it is `non_boolean_condition` (9x, 2026-09-03) |
| `unknown_source` | source name is neither an entity, a let, nor a registered query in the deployed definition set |
| `unbound_required_param` | a required callee parameter has no binding |
| `unknown_param` | binding names a parameter the callee does not declare |
| `duplicate_binding` | same parameter bound twice in one call |
| `param_type_mismatch` | binding expression's type ≠ declared parameter type |
| `argument_type_mismatch` | an expression function applied to an argument outside its domain (`lower(price)`, `length(created)`, `hash(price)`), or `least`/`greatest`/`coalesce` given two types that do not agree. **Not** `param_type_mismatch`, which is about a caller binding into a callee's declared parameter, and **not** `invalid_binding_expr`, which is about a write's right-hand side. Stretching either would make the class name stop predicting where to look. Bind time: F6's shape verification already failed these at LOAD, but as "the database would not describe this statement" rather than as the author's mistake, and possibly in an op nobody had just edited |
| `invalid_binding_expr` | binding expression references a field (bindings are row-free scalars) |
| `not_output_field` | condition, group/take key, or sort key names a field outside the callee's output shape |
| `projection_unknown_field` | projection names a field outside the callee's output shape, or two sources of one multi-source list project the same output name (`a[note] b[note]`; rename one, `b[n2: note]`). Ruled 2026-10-05 |
| `not_a_ref` | dot-path continues through a value-typed output field |
| `bare_field_in_group` | per-row field inside `{ }` after group/reduce |
| `append_misalignment` | append columns differ by name or exact type |
| `fan_out` | more than one independent many-path in a pipeline (composed sources included) |
| `let_scalar_requires_reduce` | scalar reference to a let whose body does not end in `reduce` |
| `cycle` | a query sources itself, directly or transitively (registration time) |
| `dependency_failed` | callee failed its own registration; dependent blocked, not independently diagnosed |
| `breaking_dependents` | a callee edit invalidates registered dependents; plan rejected listing them |
| `unacknowledged_unbounded` | op classifies `unbounded-host` without the `unbounded` marker |
| `check_beyond_row` | a `check` reaches past the row being written: a dot-path through a ref, or a tree-position test (`under`/`above`). DL §3 settles the ref-walking form, but a Postgres CHECK admits no subqueries, so that form is trigger work on the declaring write and on every entity the walk passes through. It is refused by name and never approximated (ruling 2026-09-05) |
| `check_time_dependent` | a `check` consults the clock: `today`, `now`, or a `since` window. A CHECK expression must be immutable; a time-dependent rule would pass at write and silently become false as the clock moves, which is worse than refusal. Time-dependent rules stay rule-tier |
| `check_sealed_field` | a `check` reads a sealed field. The column stores ciphertext; the database cannot evaluate a predicate over a value it cannot see, and a blind index answers equality-to-a-known-digest only, and a constraint expression cannot compute the keyed digest |
| `check_unknown_field` | a `check` names something that is not a field of the declaring entity. Its own class rather than `unknown_field`, which ruling 10f made internal: this one is authored and case-visible |
| `check_unsupported` | a `check` uses a construct outside the row-local slice: a parameter (there is no caller at constraint time), an expression function, `cond(…)`, division (produces `decimal`, and no implicit rounding happens anywhere), a fractional money literal, or a duration literal. Each message names the specific construct and the row-local spelling to use instead |
| `unbounded_take` | `take $n` whose parameter carries no `<=` bound, so nothing bounds what the op returns (`definition-language.md` §5, Other steps). Registration time, and **not** satisfiable by the `unbounded` marker: that marker acknowledges work growing with the *space*, and an unwritten take count is work growing with what the *caller* asks for |
| `bound_violation` | a parameter value outside its declared bound, refused **at the door before planning**, the other half of `unbounded_take`'s proof (`definition-language.md` §2, Parameters). Also a runtime wire class. A value longer than a `text(n)` **field** is `check_violation` instead: the field's bound is a row invariant |
| `sealed_keyset_pagination` | page-token request against an op whose final sort key is sealed (typed **runtime** rejection) |

**Schema build.** Raised while the declaration becomes a schema, before any
op is bound. `SchemaError` draws from this inventory like the rest (9x,
2026-09-03). These were emitted for a long time and named nowhere, which is
how a closed set stops being closed.

| class | when |
|---|---|
| `duplicate_entity` | two entities declared with one name |
| `duplicate_field` | one entity declares a field name twice |
| `unknown_ref_target` | a `->` names an entity the program does not declare |
| `tree_not_self_ref` | `tree` on a ref that does not point at its own entity (a tree is a self-edge or it is not a tree) |
| `multiple_owners` | more than one `owned` ref out of an entity; ownership is a single parent (`writing-data.md` §6) |
| `reserved_field_name` | declaring a name the platform provides on every entity (`id`, `rev`, `depth`) |
| `indexed_without_sealed` | `indexed` on a field that is not `sealed`. The blind index exists to make a sealed field findable, and on plain text it is a slower `=` |
| `refinable_without_sealed` | `refinable` on a field that is not `sealed`. Every open field already accepts `$refine` conditions, so the marker would be decoration. It exists to open a **sealed** field to them (`std-library.md` §2.1 rule 1), which is a disclosure and a cost, and both belong in the declaration |
| `unknown_index_field` | `index (…)` names a field the entity does not have |
| `unknown_unique_key_field` | `unique (a, b)` names a field the entity does not have |
| `unsupported_index` | `index (…)` over a `sealed` field: the stored bytes are an envelope with a fresh nonce, so an index over them orders and matches nothing |
| `unsupported_unique_key` | a composite `unique` this slice cannot lower |
| `unsupported_default` | a default outside the closed set the DDL can carry (only literals are lowered) |
| `duplicate_secret` | two `secret` lines name one secret. Put every host on one line: `secret llmKey for a.example.com, b.example.com` (`howto-functions.md`) |
| `invalid_function` | a `function` block the declaration alone can refuse: a duplicate name, a name an op or routine has, a domain that is not the program's, an answer other than `json`, a caller other than `member`, a code path that is absolute, climbs out with `..` or is not `.js`/`.mjs`, a host that is not a DNS name or is listed twice, a secret listed twice, or a limit outside the deployment's ceilings. The message names the line and the rule. Fix the block and apply again (`plan-backend-functions.md` §4.1) |

**Binding and planning.** Raised once a source has been resolved.

| class | when |
|---|---|
| `ambiguous_field` | a bare field name exists on more than one source in scope, so qualify it |
| `ambiguous_match` | more than one ref connects two entities, so the join is not determined; name the one meant |
| `no_match_path` | no ref connects them at all |
| `non_boolean_condition` | a bare field in condition position that is not `bool`. **Not `parse_error`**: the grammar read it perfectly well and the defect is its type, which is the parse/bind boundary ruling applied (2026-08-30, `corpus-rulings.md`) |
| `argument_type_mismatch` | an expression function applied to an argument outside its domain, or `least`/`greatest`/`coalesce` given two types that do not agree. Absorbs the former `type_error`, whose name could not predict where to look |
| `bad_aggregate` | an aggregate's own argument is wrong for it, such as `percentile` without a fraction strictly between 0 and 1 |
| `empty_projection` | a query that would produce no output field at all |
| `unpageable` | a page token requested for a plan with no ordering to seek within |
| `duration_unit_mismatch` | a comparison or binding across two duration units. See `definition-language.md` §1–2 for the type system this implements, and the divergence recorded in `audit-doc-contradictions.md` |
| `money_places_mismatch` | two `money` values with different places meet: compared, added, chosen between, or bound into a slot (`money(2)` beside `money(0)`). Different places almost always mean different currencies, and the compiler knows no exchange rate, so it converts nothing on its own (27ak, ruled 2026-10-04) |
| `reproducible_clock` | an op marked `reproducible` reads the clock: a `since` window in its own pipeline, or a `filled` parameter (the generated client fills it from the caller's clock). `definition-language.md` §13 (the op wrapper: "`reproducible` (forbids `since`)"), §4 "`since` and reproducibility", §2 Parameters ("`filled`"). Bound the window with a date parameter instead |
| `filled_type` | a parameter marked `filled` whose type is not `date` or `datetime`. The generated client fills a filled parameter from the caller's clock and can produce no other value on its own. `definition-language.md` §2 Parameters ("`filled`"), ruled 2026-10-05. Drop `filled` and let the caller pass the value |
| `reserved_param` | a declared parameter named `caller`: `$caller` is a session slot the door fills, never something a call site may supply (`guards-conformance.md` R-G13) |

### Internal vocabulary: never a conformance case's answer

Two classes are raised by the engine about **itself**, not about a program.
They are named here so the inventory is complete and so nobody asserts one in
a case, following the ruling that made `unknown_field` internal (10f): a class
a case can see is a contract, and these are diagnostics.

| class | when |
|---|---|
| `unscoped_source` | a planned statement reached the space audit naming a table it did not confine. An author cannot cause this. It means the planner emitted something the tenancy audit refuses, and the audit **fails closed**, so the result is a refusal and no leak |
| `unresolved_blind_index` | a blind-index digest site survived to execution unresolved. It means custody was not wired into this caller, not that the query was wrong. It is deliberately *not* a silent empty result |


**Three of these have no producer in the tree, and each is waiting on a
feature rather than on someone remembering to raise it** (audited 2026-08-29,
checklist 9q):

- `dependency_failed` and `breaking_dependents` both describe a deployment
  being edited: one callee's registration failing and blocking its dependents,
  a callee edit invalidating dependents already registered. There is no such
  thing yet. `Registry::from_program` builds the whole definition set from one
  source text in one shot and `check_registration` fails the *program*, so
  there is never a previously-registered set to be a dependent of, nor a diff
  to be breaking with respect to. Both become raisable with **incremental
  registration and the deploy diff** (checklist 9m), and not before.
- `sealed_keyset_pagination` needs the door to know that an op's **final sort
  key** is sealed. `OpSpec` carries output fields with a `sealed` flag, but no
  sort order at all, and the host marks every output field open when it builds
  specs, so the transport cannot tell a sealed final sort key from an open
  one, and refuses nothing. It becomes raisable when the **plan artifact
  publishes its ordering to the transport** (checklist 9m/9l).

---

## 1. Component test matrix

Each row: behavior → expected compiler/runtime behavior → covering corpus
cases. Rows without a case id are unit-only micro-checks.

### (a) Parser: call-form grammar

| Behavior | Expect | Cases |
|---|---|---|
| `name(bindings, conditions)[projection]` with mixed entries, any order | accept; entry kind decided by `$` alone | C005, C021 |
| bare composed source, no parens (nothing to pass) | accept | C001, C009 |
| omitted projection = all output fields | accept | C002, C006 |
| `[*]` / `[*, -f]` on composed source | accept; `[*]` ≡ omitted (ruling R-C2) | C009, C010 |
| composed source in multi-source list, `match any/none`, `let`, `append`, take-key lists | accept everywhere an entity source is legal | C011–C013, C067, C070 |
| empty parens `name()` | `parse_error` | C099 |
| synonym spellings (`from`, `.filter(…)`, `= `-bindings, `as`, `join`, keyword args) | `parse_error`; must fail, never parse to something else | C100–C109 |
| mixed `and`/`or` without parens inside call parens | `parse_error` | C042 |
| bare non-boolean field in condition position | `non_boolean_condition` | C105 |
| two paren groups / two projections on one source | `parse_error` | C106, C107 |
| `take[2]` (numeric in take brackets) | `parse_error` (brackets enclose field lists only) | C108 |
| trailing comma in parens | `parse_error` | C110 |

### (b) Binder: params

| Behavior | Expect | Cases |
|---|---|---|
| required param bound with literal | accept, typed | C003 |
| defaulted param omitted / overridden | accept; default applies when omitted | C016, C017 |
| caller `$param` flows through to callee binding | accept; types checked end-to-end | C018 |
| binding = closed expression (date ± duration, let scalar) | accept | C019, C020 |
| money param bound with bare int literal | accept (minor-units semantics) | C022 |
| required param unbound | `unbound_required_param` | C023 |
| binding to undeclared param | `unknown_param` | C024 |
| type mismatch (literal or flowed param) | `param_type_mismatch` | C025–C027 |
| same param bound twice | `duplicate_binding` | C028 |
| field reference inside a binding | `invalid_binding_expr` | C029 |

### (c) Inliner: one plan, callee-first semantics

| Behavior | Expect | Cases |
|---|---|---|
| composed op compiles to exactly one SQL plan, zero runtime view refs | artifact shows one planned statement | C052 |
| callee `sort`/`take` complete before caller conditions | post-take filtering; contrast with filter-then-take | C004, C044, C048 |
| caller sort/take operate on callee result | accept | C045, C046 |
| callee order preserved when caller adds none | callee sort order is the result order | C047 |
| depth-2 / depth-3 nesting | accept; identical semantics to hand-flattened form | C049–C051 |
| condition on non-output (internal) field | `not_output_field` | C040, C041, C043 |

### (d) Scope derivation: manifests

| Behavior | Expect | Cases |
|---|---|---|
| depth-1: manifest = base fields the flattened plan reads (post-pruning) | metadata | C116 |
| depth-2: caller projection prunes callee outputs from manifest | metadata | C085 |
| depth-3: manifest still bottoms out in base entities (incl. tree structure reads) | metadata | C117 |
| composition cannot widen scope: non-output fields unreachable | `not_output_field` + manifest excludes field | C118 |

### (e) Cost classes: max-propagation

| Behavior | Expect | Cases |
|---|---|---|
| blind-index callee → composite blind-index | metadata | C084, C085 |
| unbounded-host callee → composite unbounded-host; marker NOT inherited | `unacknowledged_unbounded` without caller's own `unbounded` | C086, C087 |
| sealed-field condition written in caller (over sealed output) | host-side, class per sealed rules, unchanged by position | C088, C091 |
| sealed-indexed equality in caller over an unbounded callee | max rule wins: composite stays unbounded-host (ruling R-C10) | C089 |
| sealed final sort key on composed op | registers with ack, non-pageable (ruling R-C12) | C090 |

### (f) Determinism

| Behavior | Expect | Cases |
|---|---|---|
| id tiebreaker survives inlining (sort ties) | stable total order | C053 |
| `take[]` with no sort ranks by id through composition | stable | C063 |
| unsorted composed pipeline orders by anchor id; unsorted group by key asc | rulings R-C4 | C002, C054 |

### (g) Runtime: `?live`, pagination

| Behavior | Expect | Cases |
|---|---|---|
| `?live` on composed op invalidates on writes to any inlined base entity | invalidation + refetch | C111 |
| no invalidation for entities not inlined | silence | C112 |
| keyset pagination over composed (incl. grouped) output | pages per sort + tiebreak (rulings R-C4, R-C7) | C113, C115 |
| cursor-stability label is derived from sort-key writability (R-C18) | `cursor: stable` vs `cursor: drifting` in artifact, catalog entry and descriptor | C114, C119, C120 |
| pagination under concurrent writes, **stable** cursor | no skips, no duplicates | C120 |
| pagination under concurrent writes, **drifting** cursor | a moved row may be skipped **or repeated** across a page boundary; pages stay individually correct and totally ordered | C114, C119 |
| composed post-take pagination | always `cursor: drifting` | C115 |
| page-token request on sealed-sort composed op | `sealed_keyset_pagination` (runtime) | C090 |

### (h) Registration

| Behavior | Expect | Cases |
|---|---|---|
| direct cycle | `cycle` | C092 |
| transitive cycle (introduced by re-registration) | `cycle`, chain listed | C093 |
| unknown callee | `unknown_source` | C094 |
| callee failed registration → dependents blocked | `dependency_failed` | C095 |
| callee edit → callee and all transitive dependents get new version hashes; others untouched | metadata | C096 |
| breaking callee edit (drops an output a dependent uses) | `breaking_dependents`, plan rejected | C097 |
| compatible callee edit (adds output) → dependents recompile, results unchanged | exec | C098 |

---

## 2. Testing pipeline

### 2.1 Fixture schema: domain `market`

Six entities; one tree (`categories`); one sealed-indexed field
(`customers.email`); one sealed plain field (`orders.memo`). Money in minor
units throughout.

```
domain market @ 1.0.0

entity vendors {
  name:   text
  region: text
  active: bool = true
}

entity categories {
  name:   text unique
  parent -> categories?  tree
}

entity products {
  vendor   -> vendors
  category -> categories
  name:   text
  price:  money
  active: bool = true
}

entity customers {
  name:  text
  email: text?  sealed indexed unique normalize(lower)
  tier:  text = "basic"
}

entity orders {
  customer -> customers
  placed: date
  status: text = "open"
  total:  money
  memo:   text = ""   sealed
}

entity orderItems {
  order   -> orders   owned
  product -> products
  qty:    int
  amount: money
}
```

### 2.2 Seed dataset (fixed, exhaustive)

Ids abbreviated (`vend_1` for the full typed UUIDv7). Rows are inserted in
suffix order, so **id order = suffix order** (time-ordered ids); expected
outputs below rely on this for tiebreaks.

**vendors**

| id | name | region | active |
|---|---|---|---|
| vend_1 | Acme | east | true |
| vend_2 | Blythe | west | true |
| vend_3 | Corvid | east | false |

**categories** (`depth` is the implicit tree field)

| id | name | parent | depth |
|---|---|---|---|
| cat_1 | goods | null | 0 |
| cat_2 | tools | cat_1 | 1 |
| cat_3 | handtools | cat_2 | 2 |
| cat_4 | services | null | 0 |
| cat_5 | repair | cat_4 | 1 |

**products**

| id | vendor | category | name | price | active |
|---|---|---|---|---|---|
| prod_1 | vend_1 | cat_3 | hammer | 1500 | true |
| prod_2 | vend_1 | cat_2 | drill | 9500 | true |
| prod_3 | vend_2 | cat_3 | saw | 3000 | true |
| prod_4 | vend_2 | cat_5 | tune-up | 12000 | true |
| prod_5 | vend_3 | cat_3 | wrench | 2000 | false |
| prod_6 | vend_3 | cat_4 | consult | 20000 | true |

**customers** (`email` sealed indexed; `memo` values reachable only host-side)

| id | name | email | tier |
|---|---|---|---|
| cust_1 | Ada | ada@example.com | pro |
| cust_2 | Bo | bo@example.com | basic |
| cust_3 | Cy | null | basic |

**orders**

| id | customer | placed | status | total | memo |
|---|---|---|---|---|---|
| ord_1 | cust_1 | 2026-01-10 | done | 4500 | "gift" |
| ord_2 | cust_1 | 2026-02-05 | done | 9500 | "" |
| ord_3 | cust_1 | 2026-03-15 | open | 1500 | "" |
| ord_4 | cust_2 | 2026-02-20 | done | 12000 | "rush" |
| ord_5 | cust_2 | 2026-03-01 | void | 3000 | "" |
| ord_6 | cust_3 | 2026-03-20 | open | 20000 | "" |
| ord_7 | cust_1 | 2026-04-02 | done | 2000 | "" |

**orderItems** (all `qty` 1; `amount` is the line total)

| id | order | product | qty | amount |
|---|---|---|---|---|
| item_1 | ord_1 | prod_1 | 1 | 1500 |
| item_2 | ord_1 | prod_3 | 1 | 3000 |
| item_3 | ord_2 | prod_2 | 1 | 9500 |
| item_4 | ord_3 | prod_1 | 1 | 1500 |
| item_5 | ord_4 | prod_4 | 1 | 12000 |
| item_6 | ord_5 | prod_3 | 1 | 3000 |
| item_7 | ord_6 | prod_6 | 1 | 20000 |
| item_8 | ord_7 | prod_5 | 1 | 2000 |

Handy derived facts (used repeatedly below): done orders = ord_1, ord_2,
ord_4, ord_7 · spend of cust_1 over done = 16000 (n 3), cust_2 = 12000 (n 1) ·
product revenue = prod_1 3000/2, prod_2 9500/1, prod_3 6000/2, prod_4 12000/1,
prod_5 2000/1, prod_6 20000/1.

### 2.3 Registered fixture queries (the callee library)

Registered with the schema before any corpus case runs. *(Repaired
2026-08-13: the row-cardinality gate (DL "the `unbounded` gate covers row
cardinality, not just venue [settled]") postdates this library's first
draft; the nine ops with no provable row bound now carry the
acknowledgment the settled rule requires. `top3Orders` bounds by `take`,
`customerByEmail` by unique addressing.)* Cases may register
additional local queries; those are shown inline.

```
op activeProducts unbounded =
  products(active == true)[name, price, vendor, category]

op cheapActive($cap: money) unbounded =
  activeProducts(price < $cap)[name, price, category]

op toolDeals($cap: money, $root: id) unbounded =
  cheapActive($cap: $cap, category under $root)[name, price]
  sort price

op recentOrders($since: date = 2026-03-01) unbounded =
  orders(placed >= $since)[placed, status, total]
  sort placed

op doneOrders($from: date) unbounded =
  orders(status == "done" and placed >= $from)[placed, total, customer]

op spendByCustomer($from: date) unbounded =
  doneOrders($from: $from)[total, customer]
  group customer { spent: sum total, n: count }

op monthlyRevenue($from: date) unbounded =
  doneOrders($from: $from)[placed, total]
  group month(placed) { revenue: sum total }

op productRevenue unbounded =
  orderItems[amount, product]
  group product { sold: count, revenue: sum amount }

op top3Orders =
  orders[placed, total, customer]
  sort -total
  take 3

op catInfo unbounded =
  categories[name, parent, depth]

op customerByEmail($e: text) =
  customers(email == $e)[name, tier]

op giftOrders unbounded =
  orders(memo == "gift")[placed, total, customer]

op orderMemos unbounded =
  orders[placed, total, memo]
```

Composition depths: `cheapActive` 2, `toolDeals` 3, `spendByCustomer` 2,
`monthlyRevenue` 2 (their callers reach 3–4).

### 2.4 Harness stages

1. **Parse**: case source through the parser. Reject cases assert the error
   class here or at the next stage, whichever the class belongs to.
2. **Compile**: plan against the fixture registry. Goldens snapshotted per
   case under `golden/CNNN.sql` (planned SQL, one statement per pipeline) and
   `golden/CNNN.meta.json` (cost class, field-scope manifest, callee list with
   depths, version hash, pageable flag, mode labels). Compile assertions run
   from `meta.json`, never by regexing SQL.
3. **Execute**: against a Postgres seeded exactly with §2.2 (single DDL apply
   + single seed script; database recreated per run). Execution clock pinned
   at `2026-05-01T00:00:00Z`, from which `since` windows are computed.
4. **Compare**: canonical JSON: array of row objects, object keys sorted,
   row order exactly the language's order (declared sorts, then implicit
   tiebreaks per rulings R-C4/R-C7). money/int as JSON integers, dates as ISO
   strings, refs as id strings, null as `null`. Byte-equal or fail.
5. **Assert metadata**: cost class, manifest, and dependency assertions from
   `meta.json` (compile cases stop after this stage).

### 2.5 CI tiers and golden maintenance

- **Compile tier**: every push. Runs stages 1–2 and 5 for all 120 cases (all
  reject cases complete here). No database. Seconds, not minutes.
- **Execution tier**: every PR merge queue + nightly. Postgres service
  container; runs stages 3–4 for exec/behavioral cases. Behavioral cases
  (`?live`, pagination) run against the op runtime, not raw SQL.
- **Goldens**: regenerated only via **`ORDO_UPDATE_GOLDEN=1 cargo test
  --workspace`**; the diff lands in the PR and requires review by a language
  owner. A golden SQL change without a corresponding spec/ruling change in the
  PR description fails review by policy: plan drift is a finding, not noise.
  Hash fields are asserted for *change/no-change* (C096), never for literal
  values.

  *Built today* (`ordo-conformance/src/golden.rs` is the helper; there is no
  snapshot-testing dependency, because a golden test is a string comparison):
  the market fixture's DDL and the `acl`-derived link entity's DDL separately
  (P2), per-case SQL under `crates/ordo-conformance/tests/golden/sql/CNNN.sql`
  for every case the slice executes (P4), and the whole `ordo plan` artifact
  under `crates/ordo-cli/tests/golden/market.artifact.json` (P7). *Not built*:
  the per-case `CNNN.meta.json` of stage 2. The metadata it would carry
  (cost class, guard set, callee reach, version hash, mode label) exists
  per **op** in the plan artifact and is goldened there, but the per-**case**
  rendering has no producer, so stage 5's metadata assertions still read the
  IR directly.

---

## 3. Rulings adopted by this suite

Points the spec left open, pinned here so every expected output is computable.
Each is a candidate to fold into the spec.

- **R-C1** Empty parens do not parse. Omit parens when nothing is passed; a
  callee with required params therefore always has parens.
- **R-C2** Omitted projection on a composed source = all output fields; `[*]` is
  the same projection written in the entity-form spelling (C009 pins the
  equivalence). Noted tension with one-spelling; both accepted.
- **R-C3** Binding expressions are row-free: literals, caller `$params`,
  reduce-let scalars, and pure functions over those. Any field reference is
  `invalid_binding_expr`.
- **R-C4** Ordering without `sort`: ungrouped pipelines order by the anchor
  entity's implicit id ascending; grouped pipelines order by group key(s)
  ascending, nulls last. (Generalizes the spec's "no sort yet → id order" for
  `take[]`.)
- **R-C5** A dot-path or expression group key exposes its final segment /
  function name as the output field name (`month(placed)` → `month`,
  `customer.tier` → `tier`), consistent with the spec's `monthlySpend`
  example.
- **R-C6** Conditions on ref-typed output fields may dot onward, and may use
  `under`/`above` when the ref targets a tree entity.
- **R-C7** Ungrouped composed output implicitly carries the callee anchor's id,
  for ordering and pagination tiebreaks only. It is never a named field and never a
  join key. Grouped output's tiebreak is its (unique) group key. Join keys
  are ref-typed outputs and grouped provenance keys, exactly as for lets.
- **R-C8** The entity-appears-once rule counts explicitly listed entity sources;
  composed sources are let-like (may share base entities). Joins involving
  composed sources go through provenance keys; an M:N key relationship
  violates the fan-out guard.
- **R-C9** The `unbounded` acknowledgment is per registered op and never
  inherited: a caller inlining an unbounded callee must carry its own marker.
- **R-C10** Cost class = max of inlined stages as classified at their own
  registration. Flattening never lowers a class (conservative, stable under
  callee-internal refactors). Corollary: write sealed conditions in the query
  nearest the base to earn the cheap class (C089 vs C084).
- **R-C11** Field-scope manifests are post-pruning: the base fields the
  flattened plan actually reads or returns.
- **R-C12** An op whose final sort key is sealed registers (with the required
  acknowledgment) but is non-pageable; a page-token request is the typed
  runtime rejection `sealed_keyset_pagination`.
- **R-C13** A callee edit that invalidates dependents is rejected at plan time
  listing them (`breaking_dependents`); compatible edits recompile dependents
  (new version hashes, C096/C098).
- **R-C14** Names resolve across the whole deployed definition set; registration
  order within one plan is immaterial. Cycles are detected over the set.
- **R-C15** `take` counts in this corpus are literals only; the spec is silent
  on parameterized counts and the suite does not presume them.
- **R-C16** Refs compare and sort by their id.
- **R-C17** Sealed-field **projection** costs a per-row host decrypt; if the
  result set is not provably bounded the op classifies `unbounded-host`
  (`orderMemos` carries the marker for this reason alone).
- **R-C18** **Keyset pagination is labeled, not guaranteed.** An op whose sort
  keys are *all* write-stable is labeled **`cursor: stable`** and the
  no-skip/no-duplicate property holds unconditionally; the implicit id
  tiebreak is always stable, so an unsorted or id-sorted op is always stable
  (the common case). An op with **any writable sort key** is labeled
  **`cursor: drifting`** in its artifact, its catalog entry and its
  descriptor: pages remain totally ordered and individually correct, but a row
  whose key a concurrent write moves may be **skipped or repeated** across a
  page boundary. Composed post-take pagination is *always* drifting, because
  paginating a caller's filter over a callee's `take` recomputes the window per
  page. The label is derived and costs no runtime machinery.
  *This ruling replaces the suite's earlier assertion of "no duplicates, ever"
  (C114), which is **withdrawn**.* An aggregate sort key such as `-spent` is
  computed from writable columns and is therefore writable; the unqualified
  no-skip/no-duplicate claim was never true for it, and
  `definition-language.md` §5 now says so explicitly ("the unqualified claim
  'no skips, no duplicates, ever' is not made anywhere in this document").
  The suite asserts the **label** and then asserts one witness of each
  behaviour. It asserts no guarantee the engine cannot make.

---

## 4. Corpus

Format: case id, then kind. Ordo source (fixture library assumed registered),
`params:` where any, `→` expected rows as tuples in projection order (top to
bottom = expected row order), or `→ reject:` class. Money in minor units.

### A. Call form and projection (C001–C015)

**C001: exec.** Reuse with projection subset.
```
activeProducts[name, price]
sort price
```
→ 5 rows: (hammer, 1500) · (saw, 3000) · (drill, 9500) · (tune-up, 12000) ·
(consult, 20000)

**C002: exec.** Binding; omitted projection = all output fields; unsorted →
anchor id order (R-C4).
```
cheapActive($cap: 5000)
```
→ 2 rows: (hammer, 1500, cat_3) · (saw, 3000, cat_3)

**C003: exec.** Binding only.
```
doneOrders($from: 2026-02-01)[placed, total]
sort placed
```
→ 3 rows: (2026-02-05, 9500) · (2026-02-20, 12000) · (2026-04-02, 2000)

**C004: exec.** Condition only; callee take completes first (post-take
filter). top3Orders yields ord_6/ord_4/ord_2; filter drops ord_6.
```
top3Orders(total < 15000)[placed, total]
```
→ 2 rows (callee `-total` order preserved): (2026-02-20, 12000) ·
(2026-02-05, 9500)

**C005: exec.** Binding + condition + dot through ref output.
```
doneOrders($from: 2026-01-01, total > 5000)[placed, total, customer.name]
sort -total
```
→ 2 rows: (2026-02-20, 12000, Bo) · (2026-02-05, 9500, Ada)

**C006: exec.** Omitted projection on a grouped callee; all outputs
(customer, spent, n).
```
spendByCustomer($from: 2026-01-01)
sort -spent
```
→ 2 rows: (cust_1, 16000, 3) · (cust_2, 12000, 1)

**C007: exec.** Rename in caller projection.
```
activeProducts[label: name, price]
sort price
```
→ 5 rows: (hammer, 1500) · (saw, 3000) · (drill, 9500) · (tune-up, 12000) ·
(consult, 20000); the first column is named `label`

**C008: exec.** Computed projection over output fields.
```
cheapActive($cap: 10000)[name, sale: price - 500]
sort sale
```
→ 3 rows: (hammer, 1000) · (saw, 2500) · (drill, 9000)

**C009: exec.** `[*]` on a composed source ≡ omitted projection (R-C2).
```
top3Orders[*]
```
→ 3 rows: (2026-03-20, 20000, cust_3) · (2026-02-20, 12000, cust_2) ·
(2026-02-05, 9500, cust_1)

**C010: exec.** Everything-minus on composed output.
```
spendByCustomer($from: 2026-01-01)[*, -n]
sort -spent
```
→ 2 rows: (cust_1, 16000) · (cust_2, 12000)

**C011: exec.** Composed source beside an entity source; join inferred on
`product` provenance; outer-preserving; bar filters.
```
products[name]  productRevenue[sold, revenue]
| sold >= 2
```
→ 2 rows (id order): (hammer, 2, 3000) · (saw, 2, 6000)

**C012: exec.** Composed source under `match any` (semi-join). Done orders
placed ≥ 2026-03-01: ord_7 only.
```
customers[name]
match any doneOrders($from: 2026-03-01)
```
→ 1 row: (Ada)

**C013: exec.** Composed source under `match none` (anti-join).
```
customers[name]
match none doneOrders($from: 2026-01-01)
```
→ 1 row: (Cy)

**C014: exec.** `distinct` step over composed output; rename with dot-path.
```
doneOrders($from: 2026-01-01)[tier: customer.tier]
distinct
sort tier
```
→ 2 rows: (basic) · (pro)

**C015: exec.** `cond()` in caller projection over composed output.
```
top3Orders[total, size: cond(total >= 15000 -> "huge", "big")]
```
→ 3 rows: (20000, huge) · (12000, big) · (9500, big)

### B. Binder: parameters (C016–C029)

**C016: exec.** Defaulted param omitted (`$since` defaults to 2026-03-01).
```
recentOrders[placed, total]
```
→ 4 rows: (2026-03-01, 3000) · (2026-03-15, 1500) · (2026-03-20, 20000) ·
(2026-04-02, 2000)

**C017: exec.** Defaulted param bound explicitly.
```
recentOrders($since: 2026-03-16)[placed, total]
```
→ 2 rows: (2026-03-20, 20000) · (2026-04-02, 2000)

**C018: exec.** Caller param flows through.
```
op ordersSince($d: date) unbounded =
  recentOrders($since: $d)[placed, total]
```
params: `$d = 2026-04-01`
→ 1 row: (2026-04-02, 2000)

**C019: exec.** Binding expression: date + duration.
```
recentOrders($since: $d + 7d)[placed, total]
```
params: `$d = 2026-03-10` (window opens 2026-03-17)
→ 2 rows: (2026-03-20, 20000) · (2026-04-02, 2000)

**C020: exec.** Binding from a reduce-let scalar (R-C3). max total = 20000.
```
let cap = orders[total] reduce { m: max total }

cheapActive($cap: cap.m)[name]
```
→ 4 rows (id order): (hammer) · (drill) · (saw) · (tune-up)

**C021: exec.** Condition written before binding; entry order is immaterial.
```
doneOrders(total > 5000, $from: 2026-01-01)[placed, total]
```
→ 2 rows (id order): (2026-02-05, 9500) · (2026-02-20, 12000)

**C022: exec.** money param bound with a bare integer literal (minor units).
```
cheapActive($cap: 3000)[name, price]
```
→ 1 row: (hammer, 1500)

**C023: reject.** Required param unbound.
```
doneOrders(total > 0)[total]
```
→ reject: `unbound_required_param` ($from)

**C024: reject.** Unknown parameter name.
```
recentOrders($cutoff: 2026-01-01)[placed]
```
→ reject: `unknown_param`

**C025: reject.** int literal bound to date param.
```
recentOrders($since: 5)[placed]
```
→ reject: `param_type_mismatch`

**C026: reject.** String is not a date literal (bare ISO is).
```
doneOrders($from: "2026-01-01")[total]
```
→ reject: `param_type_mismatch`

**C027: reject.** Flow-through type mismatch.
```
op bad($n: int) unbounded =
  doneOrders($from: $n)[total]
```
→ reject: `param_type_mismatch`

**C028: reject.** Same param bound twice.
```
doneOrders($from: 2026-01-01, $from: 2026-02-01)[total]
```
→ reject: `duplicate_binding`

**C029: reject.** Field reference in a binding (bindings are row-free, R-C3).
```
doneOrders($from: placed)[total]
```
→ reject: `invalid_binding_expr`

### C. Conditions over output fields (C030–C043)

**C030: exec.** Condition on an aggregate output.
```
spendByCustomer($from: 2026-01-01, spent > 12000)[customer, spent]
```
→ 1 row: (cust_1, 16000)

**C031: exec.** Ref-output equality.
```
productRevenue(product == $p)[revenue]
```
params: `$p = prod_3`
→ 1 row: (6000)

**C032: exec.** Dot-through condition on a ref output (R-C6).
```
productRevenue(product.vendor == $v)[product.name, revenue]
```
params: `$v = vend_1`
→ 2 rows (key order): (hammer, 3000) · (drill, 9500)

**C033: exec.** `in` membership over a ref output. top3Orders holds
ord_6 (cust_3), ord_4 (cust_2), ord_2 (cust_1); cust_3 excluded.
```
top3Orders(customer in ($a, $b))[total]
```
params: `$a = cust_1, $b = cust_2`
→ 2 rows: (12000) · (9500)

**C034: exec.** Null test on a nullable ref output.
```
catInfo(parent is null)[name]
```
→ 2 rows (id order): (goods) · (services)

**C035: exec.** `since` on an output date field; harness clock pinned
2026-05-01Z, so the 90d window opens 2026-01-31 (ord_1 excluded).
```
doneOrders($from: 2026-01-01, placed since 90d)[total]
```
→ 3 rows (id order): (9500) · (12000) · (2000)

**C036: exec.** Mixed booleans with mandatory parens.
```
top3Orders(total > 10000 or (customer == $a and total > 9000))[total]
```
params: `$a = cust_1`
→ 3 rows: (20000) · (12000) · (9500)

**C037: exec.** Tree operator on a ref output (R-C6). goods subtree =
{cat_1, cat_2, cat_3}; drops tune-up (cat_5).
```
cheapActive($cap: 20000, category under $root)[name, price]
sort price
```
params: `$root = cat_1`
→ 3 rows: (hammer, 1500) · (saw, 3000) · (drill, 9500)

**C038: exec.** Text operator on a text output.
```
activeProducts(name contains "a")[name]
```
→ 2 rows (id order): (hammer) · (saw)

**C039: exec.** Field-to-field comparison between outputs
(`sold * 5000` vs revenue).
```
productRevenue(revenue > sold * 5000)[product.name, revenue]
```
→ 3 rows (key order): (drill, 9500) · (tune-up, 12000) · (consult, 20000)

**C040: reject.** Condition on a callee-internal field (`status` is filtered
inside doneOrders, not projected).
```
doneOrders($from: 2026-01-01, status == "done")[total]
```
→ reject: `not_output_field`

**C041: reject.** Condition on a base field the callee did not output.
```
activeProducts(active == true)[name]
```
→ reject: `not_output_field`

**C042: reject.** Mixed `and`/`or` without parens inside call parens.
```
top3Orders(total > 10000 or customer == $a and total > 9000)[total]
```
→ reject: `parse_error`

**C043: reject.** A narrowing projection truly narrows the shape for all
downstream callers.
```
op slim = top3Orders[placed, total]      # registers: accept

slim(customer == $a)[placed]
```
→ reject: `not_output_field` (customer is not an output of slim)

### D. Inlining: callee-first order semantics (C044–C053)

**C044: exec.** Contrast query for C004: filter *then* take, written
directly. The rows differ because position decides.
```
orders(total < 15000)[placed, total]
sort -total
take 3
```
→ 3 rows: (2026-02-20, 12000) · (2026-02-05, 9500) · (2026-01-10, 4500)

**C045: exec.** Caller re-sorts the post-take result.
```
top3Orders(total < 15000)[placed, total]
sort placed
```
→ 2 rows: (2026-02-05, 9500) · (2026-02-20, 12000)

**C046: exec.** Caller take after callee take.
```
top3Orders[total]
sort total
take 2
```
→ 2 rows: (9500) · (12000)

**C047: exec.** No caller sort: callee order is the result order.
```
top3Orders[customer.name, total]
```
→ 3 rows: (Cy, 20000) · (Bo, 12000) · (Ada, 9500)

**C048: exec.** Nested take: callee's take-2 completes before caller filter.
```
op top2Cheap =
  cheapActive($cap: 1000000)[name, price]
  sort price
  take 2

top2Cheap(price > 2000)[name]
```
→ 1 row: (saw)   # filter-first would have yielded saw and drill

**C049: exec.** Depth-2 inlining (spendByCustomer over doneOrders).
```
spendByCustomer($from: 2026-02-01)[customer.name, spent]
sort -spent
```
→ 2 rows: (Bo, 12000) · (Ada, 11500)

**C050: exec.** Depth-3 inlining (toolDeals → cheapActive → activeProducts).
tools subtree = {cat_2, cat_3}.
```
toolDeals($cap: 10000, $root: cat_2)
```
params: `$root = cat_2`
→ 3 rows: (hammer, 1500) · (saw, 3000) · (drill, 9500)

**C051: exec.** Depth-3 via caller over monthlyRevenue.
Buckets: 2026-01 4500, 2026-02 21500, 2026-04 2000.
```
monthlyRevenue($from: 2026-01-01, revenue > 5000)[month, revenue]
```
→ 1 row: (2026-02, 21500)

**C052: compile.** Single-plan assertion for C050's registration artifact.
→ compile: exactly one planned SQL statement; callee list with depths
`{cheapActive: 1, activeProducts: 2}`; no view references, no runtime lookup

**C053: exec.** id tiebreaker survives inlining: expression sort with a tie
(2026-02 holds ord_2 and ord_4; id order breaks it).
```
doneOrders($from: 2026-01-01)[placed, total]
sort month(placed)
```
→ 4 rows: (2026-01-10, 4500) · (2026-02-05, 9500) · (2026-02-20, 12000) ·
(2026-04-02, 2000)

### E. Composed + group/reduce (C054–C060)

**C054: exec.** Group by dot through a ref output; unsorted group → key asc
(R-C4, R-C5).
```
doneOrders($from: 2026-01-01)[total, customer]
group customer.tier { n: count, rev: sum total }
```
→ 2 rows: (basic, 1, 12000) · (pro, 3, 16000)

**C055: exec.** Reduce over composed (post-take) output.
```
top3Orders[total]
reduce { grand: sum total }
```
→ 1 row: (41500)

**C056: exec.** Regroup a grouped callee; bucket bar; caller sort.
Category revenue: handtools 11000, tools 9500, repair 12000, services 20000.
```
productRevenue[product, revenue]
group product.category.name { catRev: sum revenue }
| catRev > 10000
sort -catRev
```
→ 3 rows: (services, 20000) · (repair, 12000) · (handtools, 11000)

**C057: exec.** Conditional aggregate over composed output; ref group key
orders by id (R-C4, R-C16).
```
doneOrders($from: 2026-01-01)[total, customer]
group customer { big: count(total > 5000), n: count }
```
→ 2 rows: (cust_1, 1, 3) · (cust_2, 1, 1)

**C058: exec.** Double aggregation across composition (distribution of n).
```
spendByCustomer($from: 2026-01-01)[n]
group n { custs: count }
```
→ 2 rows: (1, 1) · (3, 1)

**C059: exec.** Median over composed output. Revenues sorted: 2000, 3000,
6000, 9500, 12000, 20000 → (6000 + 9500) / 2.
```
productRevenue[revenue]
reduce { m: median revenue }
```
→ 1 row: (7750)

**C060: reject.** Bare per-row field inside braces.
```
doneOrders($from: 2026-01-01)[total, customer]
group customer { t: total }
```
→ reject: `bare_field_in_group`

### F. Composed + take[keys] (C061–C065)

**C061: exec.** Latest done order per customer.
```
doneOrders($from: 2026-01-01)[placed, total, customer]
sort -placed
take[customer] 1
```
→ 2 rows: (2026-04-02, 2000, cust_1) · (2026-02-20, 12000, cust_2)

**C062: exec.** Expression take-key over composed output (biggest done order
per month: 01 → ord_1, 02 → ord_4, 04 → ord_7).
```
doneOrders($from: 2026-01-01)[placed, total]
sort -total
take[month(placed)] 1
```
→ 3 rows: (2026-02-20, 12000) · (2026-01-10, 4500) · (2026-04-02, 2000)

**C063: exec.** take[] with no sort ranks by implicit id through composition
(R-C4, R-C7): first product per vendor by id.
```
activeProducts[name, vendor]
take[vendor] 1
```
→ 3 rows: (hammer, vend_1) · (saw, vend_2) · (consult, vend_3)

**C064: exec.** Dot-path take-key (best done order per tier: basic → ord_4,
pro → ord_2).
```
doneOrders($from: 2026-01-01)[placed, total, customer]
sort -total
take[customer.tier] 1
```
→ 2 rows: (2026-02-20, 12000, cust_2) · (2026-02-05, 9500, cust_1)

**C065: reject.** take-key outside the callee's output shape.
```
top3Orders[placed, total]
take[status] 1
```
→ reject: `not_output_field`

### G. Composed in let (C066–C069)

**C066: exec.** Let-reduce baseline over composed output, used in a composed
condition. avg revenue = 52500 / 6 = 8750.
```
let bar = productRevenue[revenue] reduce { a: avg revenue }

productRevenue(revenue > bar.a)[product.name, revenue]
sort -revenue
```
→ 3 rows: (consult, 20000) · (tune-up, 12000) · (drill, 9500)

**C067: exec.** Composed let joined back; outer-preserving keeps the
spendless customer with null.
```
let perCust = spendByCustomer($from: 2026-01-01)

customers[name]  perCust[spent]
sort name
```
→ 3 rows: (Ada, 16000) · (Bo, 12000) · (Cy, null)

**C068: exec.** Let chain over composed output, regrouped and joined back.
Vendor revenue: vend_1 12500, vend_2 18000, vend_3 22000.
```
let pr = productRevenue[product, revenue]
let byVend = pr group product.vendor { rev: sum revenue }

vendors[name]  byVend[rev]
sort -rev
```
→ 3 rows: (Corvid, 22000) · (Blythe, 18000) · (Acme, 12500)

**C069: reject.** Non-reduce let referenced as a scalar.
```
let x = top3Orders[total]

orders(total > x.total)[placed]
```
→ reject: `let_scalar_requires_reduce`

### H. Composed in append (C070–C074)

**C070: exec.** Two composed sources appended; columns lined up by rename.
```
giftOrders[at: placed, total]
append top3Orders[at: placed, total]
sort -total
```
→ 4 rows: (2026-03-20, 20000) · (2026-02-20, 12000) · (2026-02-05, 9500) ·
(2026-01-10, 4500)

**C071: exec.** Entity source appended with a composed source.
```
orders(status == "open")[at: placed, total]
append giftOrders[at: placed, total]
sort at
```
→ 3 rows: (2026-01-10, 4500) · (2026-03-15, 1500) · (2026-03-20, 20000)

**C072: exec.** Append + distinct over ref columns; refs order by id (R-C16).
```
giftOrders[c: customer]
append doneOrders($from: 2026-01-01)[c: customer]
distinct
sort c
```
→ 2 rows: (cust_1) · (cust_2)

**C073: reject.** Column-name misalignment.
```
giftOrders[placed, total]
append top3Orders[total]
```
→ reject: `append_misalignment`

**C074: reject.** Same name, different types (date vs money); no coercion.
```
giftOrders[k: placed]
append productRevenue[k: revenue]
```
→ reject: `append_misalignment`

### I. Ref-dotting through composed output (C075–C080)

**C075: exec.** Two-hop dot through a ref output.
```
productRevenue[product.category.name, revenue]
```
→ 6 rows (key order): (handtools, 3000) · (tools, 9500) · (handtools, 6000) ·
(repair, 12000) · (handtools, 2000) · (services, 20000)

**C076: exec.** Dot-path as caller sort key.
```
productRevenue[product.name, revenue]
sort product.name
```
→ 6 rows: (consult, 20000) · (drill, 9500) · (hammer, 3000) · (saw, 6000) ·
(tune-up, 12000) · (wrench, 2000)

**C077: exec.** Dot group key over composed output (R-C5).
```
doneOrders($from: 2026-01-01)[customer]
group customer.name { n: count }
```
→ 2 rows: (Ada, 3) · (Bo, 1)

**C078: exec.** Dot projection to an open field of the ref target.
```
doneOrders($from: 2026-02-01)[placed, customer.tier]
```
→ 3 rows (id order): (2026-02-05, pro) · (2026-02-20, basic) ·
(2026-04-02, pro)

**C079: reject.** Dot through a value-typed output field.
```
spendByCustomer($from: 2026-01-01)[spent.name]
```
→ reject: `not_a_ref`

**C080: reject.** Projection of a non-output field.
```
doneOrders($from: 2026-01-01)[status]
```
→ reject: `projection_unknown_field`

### J. Fan-out guard (C081–C083)

**C081: reject.** Two independent composed many-sides under one parent
(both per-order, keyed by customer). This is the star double-count, refused.
```
customers[name]
  doneOrders($from: 2026-01-01)[total, customer]
  giftOrders[total, customer]
```
→ reject: `fan_out`

**C082: exec.** The blessed restructure: group each side in a let, join the
one-sides back (R-C8).
```
let d = doneOrders($from: 2026-01-01)[total, customer]
        group customer { spent: sum total }
let g = giftOrders[total, customer]
        group customer { gspent: sum total }

customers[name]  d[spent]  g[gspent]
sort name
```
→ 3 rows: (Ada, 16000, 4500) · (Bo, 12000, null) · (Cy, null, null)

**C083: reject.** Entity source × ungrouped composed source over a shared
ref key is M:N (R-C8).
```
orders[placed]  doneOrders($from: 2026-01-01)[total, customer]
```
→ reject: `fan_out`

### K. Sealed fields, blind index, cost classes (C084–C091)

**C084: exec + compile.** Blind-index callee.
```
customerByEmail($e: "ada@example.com")
```
→ 1 row: (Ada, pro) · compile: cost class `blind-index`

**C085: compile.** Composite cost and pruned manifest (R-C10, R-C11).
```
op tierOf($e: text) =
  customerByEmail($e: $e)[tier]
```
→ compile: cost class `blind-index`; manifest `customers.{email, tier}`
(`name` read by the callee but pruned by the caller's projection)

**C086: reject.** Unbounded callee; caller lacks its own marker (R-C9).
```
op giftTotals =
  giftOrders(total > 1000)[total]
```
→ reject: `unacknowledged_unbounded`

**C087: exec.** Same op, acknowledged.
```
op bigGifts unbounded =
  giftOrders(total > 1000)[placed, total]
```
→ 1 row: (2026-01-10, 4500) · compile: cost class `unbounded-host`

**C088: exec.** Sealed condition written in the caller, over a sealed output
field. It is host-side either way.
```
op rushOrders unbounded =
  orderMemos(memo == "rush")[placed, total]
```
→ 1 row: (2026-02-20, 12000) · compile: cost class `unbounded-host`

**C089: compile.** Max rule is position-sensitive by construction (R-C10):
equality on the sealed-indexed field written *near the base*
(customerByEmail) classifies `blind-index`; the same equality written over an
unbounded callee's output inherits the callee's class.
```
op custEmails unbounded = customers[name, email, tier]   # R-C17: sealed projection

op emailLookup($e: text) unbounded =
  custEmails(email == $e)[name]
```
→ compile: emailLookup `unbounded-host` vs customerByEmail `blind-index`

**C090: behavioral.** Sealed final sort key: registers with ack, non-pageable
(R-C12).
```
op pagedMemo unbounded =
  orderMemos[placed, memo]
  sort memo
```
call with a page token
→ runtime reject: `sealed_keyset_pagination` (plain un-paged call succeeds)

**C091: exec.** `contains` on a sealed output field (host-side scan).
```
op rushLike unbounded =
  orderMemos(memo contains "ru")[placed, total]
```
→ 1 row: (2026-02-20, 12000) · compile: cost class `unbounded-host`

### L. Cycles and registration (C092–C098)

**C092: reject.** Direct cycle.
```
op selfLoop = selfLoop(total > 0)[total]
```
→ reject: `cycle`

**C093: reject.** Transitive cycle, introduced by edit: with
`query qa = orders[total]` and `query qb = qa[total]` registered, re-register
`qa` as:
```
op qa = qb[total]
```
→ reject: `cycle` (chain reported: qa → qb → qa)

**C094: reject.** Unknown callee (never in the definition set).
```
frequentBuyers(n > 2)[name]
```
→ reject: `unknown_source`

**C095: reject.** Dependent of a failed registration. Deploy a plan
containing C086's `giftTotals` (fails) and:
```
op giftTotalsTop = giftTotals[total] sort -total take 1
```
→ reject: `giftTotals` fails `unacknowledged_unbounded`; `giftTotalsTop`
reports `dependency_failed` (not an independent diagnosis)

**C096: compile.** Version-hash cascade. Edit `doneOrders` (tighten to
`placed >= $from and total > 0`, which preserves semantics on the seed).
→ compile: version hashes change for doneOrders, spendByCustomer,
monthlyRevenue, and every registered caller of those; hashes for
activeProducts/cheapActive/toolDeals unchanged

**C097: reject.** Breaking callee edit: re-register `doneOrders` without
`customer` in its projection.
→ reject: `breaking_dependents` (lists spendByCustomer and every other
dependent using `customer`); deployed set unchanged

**C098: exec.** Compatible callee edit: re-register `doneOrders` with an
added output (`[placed, total, customer, status]`). Dependents recompile
(new hashes); results unchanged:
```
spendByCustomer($from: 2026-01-01)[customer, spent, n]
sort -spent
```
→ 2 rows: (cust_1, 16000, 3) · (cust_2, 12000, 1)

### M. Parser rejects: synonym spellings must fail (C099–C110)

Each must fail to parse; none may parse to something else.

**C099: reject.** Empty parens (R-C1).
```
activeProducts()[name]
```
→ reject: `parse_error`

**C100: reject.** SQL spelling.
```
from activeProducts select name
```
→ reject: `parse_error`

**C101: reject.** Method-chain spelling.
```
activeProducts.filter(price < 5000)[name]
```
→ reject: `parse_error`

**C102: reject.** `=` where the binding colon belongs.
```
doneOrders($from = 2026-01-01)[total]
```
→ reject: `parse_error`

**C103: reject.** Binding without the `$` (not a condition either).
```
doneOrders(from: 2026-01-01)[total]
```
→ reject: `parse_error`

**C104: reject.** Alias spelling.
```
activeProducts as ap[name]
```
→ reject: `parse_error`

**C105: reject.** Bare non-boolean fields in condition position.
```
activeProducts(name, price)
```
→ reject: `parse_error`

**C106: reject.** Two projection groups.
```
activeProducts[name][price]
```
→ reject: `parse_error`

**C107: reject.** Two paren groups.
```
cheapActive($cap: 5000)(price > 2000)[name]
```
→ reject: `parse_error`

**C108: reject.** Numeric in take brackets (brackets enclose field lists
only; the failure is loud, never silent).
```
activeProducts[name]
take[2]
```
→ reject: `parse_error`

**C109: reject.** `join` spelling.
```
activeProducts join vendors [name]
```
→ reject: `parse_error`

**C110: reject.** Trailing comma in call parens.
```
doneOrders($from: 2026-01-01,)[total]
```
→ reject: `parse_error`

### N. Behavioral and scope metadata (C111–C120)

**C111: behavioral.** `?live` invalidation propagates to inlined base
entities. Subscribe `spendByCustomer($from: 2026-01-01)?live`, then:
```
set orders { customer: cust_3, placed: 2026-04-20, status: "done", total: 700 }
```
→ invalidation fires (orders is a base entity at inline depth 2); refetch
returns 3 rows (key asc): (cust_1, 16000, 3) · (cust_2, 12000, 1) ·
(cust_3, 700, 1)

**C112: behavioral.** No false invalidation: with the same subscription,
```
set vendors { name: "Dax", region: "north" }
```
→ no invalidation (vendors is not inlined by spendByCustomer)

**C113: behavioral + compile.** Keyset pagination over composed grouped
output (tiebreak = group key, R-C7).
```
op spendRanked =
  spendByCustomer($from: 2026-01-01)[customer, spent]
  sort -spent
```
page size 1 → page 1: (cust_1, 16000); page 2: (cust_2, 12000); page 3: empty

Compile: `spendRanked` is labeled **`cursor: drifting`** (R-C18), because `spent` is a
sum over `orders.total`, a writable column, so the sort key is writable. The
label appears in the artifact, the catalog entry and the descriptor. No page
above is wrong; the label is about what happens when a write lands *between*
them, which is C114 and C119.

**C114: behavioral.** Drift, the **skip** direction. After page 1 of C113,
`set orders { customer: cust_2, placed: 2026-04-25, status: "done",
total: 9000 }` lifts Bo to 21000, ahead of the cursor position
(`-spent` after (16000, cust_1)).
→ page 2: **empty**, and cust_2 is **never returned by this scan**. The
keyset predicate for page 2 asks for rows below (16000, cust_1) and Bo is now
above it. The row was **skipped**.
*Previously asserted: "no duplicates, ever; rows that move ahead of the cursor
are not revisited (documented behavior)".* The observed pages are unchanged;
the characterization was wrong twice over: it read a **skip** as evidence of
a no-duplicate guarantee, and it stated that guarantee unconditionally for an
op whose sort key is writable. Correct assertion: the pages are individually
correct and totally ordered, the op is labeled `cursor: drifting`, and this
scan lost a row.

**C119: behavioral.** Drift, the **duplicate** direction: the case the old
C114 wording claimed could not happen. Same op, same page 1
((cust_1, 16000)). Now a *refund* lands instead of a new order:
`set orders { id: ord_2, total: 500 }` drops Ada from 16000 (4500 + 9500 +
2000) to 7000 (4500 + 500 + 2000), **behind** the cursor.
→ page 2: (cust_2, 12000); page 3: (cust_1, 7000). Ada is returned a
**second time**, having already appeared on page 1, because the page-3 keyset
predicate (below (12000, cust_2)) now matches her. Page 4: empty. Every page is
individually correct against the data at the moment it was computed; the scan
as a whole double-counts. This is what `cursor: drifting` means, and it is why
the label states the property and promises no more.

**C120: behavioral + compile.** The **stable** contrast, so the suite does not
lose the guarantee it used to over-claim. `top3Orders` (see C115) and any
unsorted or id-sorted op sort on write-stable keys only.
```
op recentOrders unbounded = orders(status == "done")[placed, total]
```
Compile: labeled **`cursor: stable`**: no explicit `sort`, so the order is the
anchor's implicit id ascending (R-C4), and the implicit id tiebreak is never
writable.
Done orders in id order: ord_1, ord_2, ord_4, ord_7.
page size 2 → page 1: (2026-01-10, 4500) · (2026-02-05, 9500); page 2:
(2026-02-20, 12000) · (2026-04-02, 2000); page 3: empty.
Now run the same sequence with `set orders { id: ord_2, total: 99000 }`
landing **between** page 1 and page 2 → page 2 is unchanged and page 3 is
still empty: pages 1 and 2 partition the result set exactly, **no skips, no
duplicates**, because no write can move a row's position in an id ordering.
The changed `total` would show up only on a page that has not been served yet
(here it does not, because ord_2 was already returned on page 1), and it changes
nothing about *which* rows appear.

**C115: behavioral + compile.** Pagination bounded by the callee's take.
```
top3Orders[placed, total]
```
page size 2 → page 1: (2026-03-20, 20000) · (2026-02-20, 12000);
page 2: (2026-02-05, 9500); page 3: empty

Compile: labeled **`cursor: drifting`** unconditionally (R-C18), because composed
post-take pagination recomputes the callee's `take 3` window per page, so both
skips and duplicates are possible even when the outer keyset is well-behaved.
This holds regardless of whether the callee's own sort keys are write-stable.

**C116: compile.** Manifest, depth 1 (post-pruning, R-C11).
`cheapActive` → manifest `products.{active, price, name, category}`
(`vendor` output by activeProducts, pruned by cheapActive's projection).

**C117: compile.** Manifest, depth 3. `toolDeals` → manifest
`products.{active, price, name, category}` + `categories.(tree structure)`
(the `under` traversal reads the materialized path, no value fields).

**C118: compile + reject.** Composition cannot launder scope.
```
op pub = customers[name]           # manifest: customers.{name} only

op pubNames = pub[name]            # accept; manifest: customers.{name}

pub(email == $e)[name]
```
→ compile: both manifests exclude `email` · final query rejects:
`not_output_field`

---

## 5. Coverage map

120 cases. Components: (a) parser · (b) binder · (c) inliner · (d) scope
derivation · (e) cost classes · (f) determinism · (g) runtime · (h)
registration.

| Component | Cases |
|---|---|
| (a) parser | C001, C002, C005, C006, C009–C015, C021, C042, C099–C110 |
| (b) binder | C003, C016–C029 |
| (c) inliner | C004, C030–C041, C043–C052, C054–C059, C061–C064, C066–C068, C070–C072, C075–C078 |
| (d) scope derivation | C085, C116–C118 |
| (e) cost classes | C084–C091 |
| (f) determinism | C002, C047, C053, C057, C063, C072, C076 |
| (g) runtime | C090, C111–C115, C119, C120 |
| (h) registration | C043, C048, C085–C098 |
| guard interactions (fan-out, group, append, let, take[]) | C060, C065, C069, C073, C074, C079–C083 |

Feature-to-case index: simple reuse C001–C003 · binding-only C003, C016–C022 ·
condition-only C004, C030–C039 · both C005, C021, C037 · projection subsets
C001, C007–C010, C043, C080 · nested 2–3 deep C048–C052 · composed + group
C054–C059 · composed + take[keys] C061–C065 · composed in let C066–C069 ·
composed in append C070–C074 · ref-dotting through output C005, C032, C047,
C075–C078 · fan-out rejects C081, C083 · sealed across composition C084–C091 ·
cycles C092, C093 · unbounded acknowledgment C086, C087, C089 ·
pagination/live C090, C111–C115, C119, C120 · **cursor-stability labels**
(R-C18) C113–C115, C119, C120.

**What changed in this revision.** C114 asserted "Keyset semantics: no
duplicates, ever; rows that move ahead of the cursor are not revisited
(documented behavior, asserted here)". That claim is **withdrawn**: the main
spec now states that "the unqualified claim 'no skips, no duplicates, ever' is
not made anywhere in this document", and derives a per-op `cursor: stable` /
`cursor: drifting` label instead (R-C18). The case is kept, not deleted: its
observed pages are unchanged, its characterization is corrected (what it
actually witnessed was a **skip**, not evidence of a no-duplicate guarantee),
and two cases were added so the suite covers both directions of drift (C119,
the duplicate) and the guarantee that does survive unconditionally (C120,
`cursor: stable`).
