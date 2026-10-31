# Ordo Syntax Examples

Pure syntax, no prose. Domain: `estate` (property stewardship). Exercises the
constructs below: ownership, trees, link entities, sealed/blind-index,
ensure-mode ingestion, query shapes, all write modes, money arithmetic and
`allocate`, `whole`, routines, row invariants (`check` over a row's own
columns), authorization (`acl`, guards, both access verbs, the acl bootstrap,
`let x = set …`), groups. Capstone: a complete micro-app.

**Every block is checked.** A block whose first line is `# planned:` names a
construct that is not built, and `ordo check` is not run on it. Every other
block compiles, except two that show output and never take source: the shape
summary in section 1 and the result illustration under `whole` in section 3.
The test `syntax_examples_check` runs `ordo check` on each one
against the §1 ontology. A block marked `# standalone` is its own program.

Every callable thing is wrapped in `op name($params) flags = body`. There is
one registration keyword and no top-level statement form. Identifiers, routine
names included, are lowerCamel `[a-z][A-Za-z0-9]*`.

---

## 1. Ontology

```
domain estate @ 1.0.0

entity properties {
  name:     text
  address:  text        sealed
  acquired: date
}

entity locations {                       # building / floor / room
  property -> properties   owned         # a location is meaningless without its property
  parent   -> locations?   tree owned    # hierarchy AND composition
  name:     text
  kind:     text                         # "building" | "floor" | "room" | "area"
  index (property, kind)
}

entity units {                           # rentable unit
  property -> properties   owned
  location -> locations?                 # association: unit sits somewhere
  name:     text
  unique (property, name)
}

entity people {
  name:  text          sealed
  email: text?         sealed indexed unique
  phone: text?         sealed
}

entity leases {
  unit ->  units                         # association: lease references, never owns
  start:   date
  end:     date?
  rent:    money        sealed
  status:  text = "active"               # "active" | "ended" | "pending"
}

entity leaseParties {                    # pure link: lease <-> person, M2M
  lease  -> leases    owned
  person -> people    owned
  role:   text = "tenant"                # "tenant" | "guarantor"
  unique (lease, person)
}

entity trades { name: text unique }      # "plumbing", "electrical", ...

entity vendors {
  name:   text
  email:  text?        sealed
  active: bool = true
}

entity vendorTrades {                    # pure link: vendor <-> trade
  vendor -> vendors   owned
  trade  -> trades    owned
  unique (vendor, trade)
}

entity workOrders {
  location -> locations                  # association
  vendor   -> vendors?                   # association, assigned later
  opened:   date
  status:   text = "open"                # "open" | "scheduled" | "done" | "void"
  priority: int  = 3                     # 1 urgent .. 5 someday
  descr:    text        sealed
}

entity workItems {
  workOrder -> workOrders   owned
  descr:     text           sealed
  cost:      money?         sealed
  done:      bool = false
}

entity inspections {
  unit      -> units
  workOrder -> workOrders
  at:        date
}

entity buildings {
  property -> properties   owned
  name:     text
  seats:    int                          # capacity
  used:     int = 0                      # maintained counter
  check used <= seats                    # row-scoped: this row's own columns
  check used >= 0
  # What `check` CANNOT do: aggregate, count, or test existence over a
  # collection. "at most 10 open work orders per building" is a question about
  # OTHER rows, not about this row's neighbours, so it stays rule-tier: a
  # maintained counter feeding a row-scoped check, exactly as `used` does here.
  # The increment and the insert go in one atomic op; the check aborts the
  # group on violation, so the seat never oversells and no `require`-style op
  # precondition is needed.
}

entity invoices {
  vendor -> vendors
  number: text
  issued: date
  total:  money         sealed
  unique (vendor, number)
}

entity invoiceLines {
  invoice  -> invoices     owned
  workItem -> workItems?                 # association back to the work
  amount:   money           sealed
}

entity payments {
  invoice -> invoices
  paid:    date
  amount:  money        sealed
}

entity meters {
  unit -> units   owned
  kind: text                             # "water" | "power" | "gas"
}

entity readings {
  meter -> meters
  at:    datetime
  value: int
  unique (meter, at)                     # ingestion identity
}

entity devices {
  location   -> locations
  kind:       text                       # "camera" | "door" | "leak"
  externalId: text unique
  armed:      bool = false
}

entity events {
  device -> devices
  at:     datetime
  kind:   text                           # "motion" | "open" | "leak" | "offline"
}

entity alerts {
  kind:     text
  key:      text unique                  # upsert-by-key lifecycle
  severity: text = "warning"             # "info" | "warning" | "critical"
  title:    text        sealed
  status:   text = "active"              # "active" | "resolved" | "dismissed"
  raised:   date
}

entity pages {
  title:    text
  settings: json = {}                    # a JSON document, stored as jsonb
  draft:    json?                        # absent until written
}
```

A `json` field is read and written whole. A condition can look inside it: the
path takes its type from the value on its other side. A key that is not a name
goes in brackets, and so does an array position. Sort, group and aggregate
still refuse a document (`json_opaque`). See
`examples/syntax/19-json-documents.ordo`.

```
op darkPages unbounded =
  pages(settings.theme == "dark" and settings["font size"] > 12)[title]

op taggedPages($tag: text) unbounded =
  pages(settings.tags contains $tag or settings.tags[0] == $tag)[title]
```

A `check` that walks refs is **planned, not built**. `ordo check` refuses it
today with `check_beyond_row`, because a database CHECK constraint sees one
row. A `check` over the row's own columns (the `buildings` entity above) is
built.

```
# planned: not built. `ordo check` answers check_beyond_row.
entity inspections {
  unit      -> units
  workOrder -> workOrders
  at:        date
  check workOrder.location.property == unit.property
  # `check` would walk OUTGOING refs from the row being written, any depth,
  # and abort the whole command group on a violation, like any other check.
}
```

Every `unique` above is **space-scoped**: `unique (vendor, number)` means unique
*within one space*, never across the deployment. There is no deployment-wide
uniqueness spelling. Two tenants choosing the same invoice number is not a
conflict, and a constraint that spanned them would be a cross-tenant channel.

Compiler-derived shape summary (displayed, never written):

```
properties   owns: locations, units
locations    owns: locations (tree), referenced by: units, workOrders, devices
leases       owns: leaseParties · people owns: leaseParties     # link dies with either end
workOrders   owns: workItems, refs: locations, vendors?
invoices     owns: invoiceLines, referenced by: payments (restrict)
inspections  refs: units, workOrders
buildings    owned by: properties · check: used <= seats, used >= 0
```

---

## 2. Queries, basic to gnarly

Registration computes a **row-cardinality bound** next to the cost class, from
identity/unique addressing, `take`, bounded parameters, declared relationship
caps and tree depth caps. **An op with no provable bound fails registration
unless it says `unbounded`**. The same word serves host and SQL, because it is the
same author decision. A filter on a non-unique field bounds nothing: `status ==
"open"` can match every row in the entity. So most of the ops below carry the
marker, and the ones that do not are the ones whose bound the compiler can
actually prove. The presence or absence of `unbounded` carries information.
The spec owes an answer on one caveat: `take` is listed as evidence
for the bound, which is true of rows *returned*, but a `sort … take 10` over an
unfiltered entity still *examines* everything (finding X-068).

```
op openOrders unbounded =                        # status is not unique: the
  workOrders(status == "open")[opened, priority, descr, location.name, location.property.name]
  sort priority, opened                          #   match set grows with the space
```

```
op ordersUnderBuilding($loc: id) unbounded =    # tree traversal: whole subtree
  workOrders(location under $loc and status != "done")[opened, status, descr]
  sort -opened                                  #   a subtree is not a static cap
```

```
op recentOrders =                                # BOUNDED, and this is how: an
  workOrders(status == "open")[opened, priority, descr]   #   explicit take is
  sort -opened                                   #   evidence the compiler accepts
  take 50                                        #   so no acknowledgment is needed
```

```
op vacantUnits unbounded =                       # anti-join: filter by absence
  units[name, property.name]
  match none leases(status == "active")
```

```
op occupiedUnits unbounded =                     # semi-join: existence only
  units[name]
  match any leases(status == "active")
```

```
# planned: an aggregate over a sealed value is refused today (sealed_unsupported);
# host-side folding is not built. `count` over sealed rows is built.
op vendorSpend($from: date) unbounded =          # sealed aggregate over a date
  vendors[name]  invoices(issued >= $from)[total]   #   range: unbounded-host,
  group vendors.id, vendors.name {                  #   acknowledged. group by id
                                                    #   (+name for display): same-named
                                                    #   vendors never merge
    n:      count
    billed: sum total
    large:  count(total > 100000)
  }
  | billed > 0
  sort -billed
```

```
op latestReadings unbounded =                            # greatest-n-per-group
  meters[kind, unit.name]  readings[at, value]
  sort -readings.at
  take[meters.id] 1
```

```
# planned: an aggregate over a sealed value is refused today (sealed_unsupported);
# host-side folding is not built. `count` over sealed rows is built.
op monthlyMaintenance unbounded =                # time bucketing (sealed sum
  invoices[total]                                   #   over all history: acknowledged)
  group month(issued) { spent: sum total }
  sort -month
```

```
op anomalousReadings($meter: id) =               # let + reduce scalar baseline
  let baseline = readings(meter == $meter) reduce { a: avg value }

  readings(meter == $meter and value > 3 * baseline.a)[at, value]
  sort -at
```

```
op activityFeed($vendor: id) =                   # append: two shapes, one feed
  workOrders(vendor == $vendor)[title: status, at: opened]
  append invoices(vendor == $vendor)[title: number, at: issued]
  sort -at
  take 50
```

```
# planned: an aggregate over a sealed value is refused today (sealed_unsupported);
# host-side folding is not built. `count` over sealed rows is built.
op overpaidInvoices unbounded =                  # cross-entity bar condition
  invoices[number, total]  payments[amount]
  group invoices.id, invoices.number, invoices.total { paid: sum amount }
  | paid > total                                    # per-row fields ride the key;
                                                    #   bare fields in { } don't parse
```

```
op vendorsBillingLag =                           # TWO independently grouped
  let orderCount   = vendors[id]  workOrders(status == "done")[wo: id]  # aggregates,
                     group vendors.id { done: count }               # compared;
                                                                    # one many-
  let invoiceCount = vendors[id]  invoices[inv: id]              # path per let
                     group vendors.id { billed: count }

  vendors[name]  orderCount[done]  invoiceCount[billed]
  | coalesce(billed, 0) * 2 < coalesce(done, 0)  # invoiced for fewer than half
  sort name                                      #   the completed orders
# one pipeline listing both many-sides is a FAN-OUT COMPILE ERROR. Each let is
# its own pipeline (one many-path each); the join back is by group-key
# provenance (`vendors.id`), so no join condition is written; each let yields at
# most one row per vendor, so the final pipeline has no many-path at all.
# Each let starts at vendors[id], which is outer-preserving: no invoices reads
# billed = 0, not null. coalesce is defensive here, needed if a let starts from
# the child side (no row, null). Compare by multiplying, never by dividing:
# `billed / done < 0.5` divides (decimal) and errors on done == 0.
# Worked out in full: modeling-guidance.md §C7.
```

```
op expiringSoon =                                # since + dot-paths through links
  leases(status == "active"                      # a step ends at END OF LINE unless
         and end is not null                     # a delimiter is open (G6), so both
         and end <= 2026-12-31)[start, end, rent, unit.name, unit.property.name]
  sort end                                       # breaks above are inside the parens
# The projection belongs to the source and cannot begin a line of its own. Drop it
# and `leases(…)` is still a complete step (one that reads the WHOLE row), so a
# wrapped `[…]` would let the next line silently narrow what the line above it
# disclosed. Break inside the parens (or inside the brackets); never between them.
```

```
# planned: composes vendorSpend above, which is planned.
op bigSpenders($min: int) =                      # named-query composition:
  vendorSpend($from: 2026-01-01, billed > $min)[name, billed]   # $ binds, fields filter
  take 10
```

```
op orders =                                      # pagination is call-time, not syntax:
  workOrders(status != "void")[opened, priority, descr]
  sort -opened, priority                         # callers page via envelope token;
                                                 # keyset derived from this sort.
                                                 # `status` is write-mutable, so the
                                                 # artifact would carry `cursor:
                                                 # drifting` if it were the sort key
```

```
op searchVendors($q: text) =                     # open-field text ops
  vendors(active == true and (name contains $q or name starts $q))[name]
  sort name
# note: `descr contains $q` on workOrders would classify bounded/unbounded-host
# (sealed). The compiler labels the op; unbounded requires the `unbounded` marker.
```

```
# planned: an aggregate over a sealed value is refused today (sealed_unsupported);
# host-side folding is not built. `count` over sealed rows is built.
op orderCostRollup($order: id) =                 # computed projection + coalesce
  workItems(workOrder == $order)[descr, cost: coalesce(cost, 0)]
  reduce { items: count, est: sum cost }
```

---

## 3. `whole`

```
whole workOrders($id)
# → { workOrder fields,
#     workItems: [ {descr, cost, done}, ... ],          owned → expanded
#     location: {name, kind},                            upward signpost, shallow
#     vendor:   {name} }                                 association, shallow

whole locations($room)                                  # asked mid-hierarchy:
# → { room fields,
#     locations: [ ...owned tree DOWN from here... ],    subtree from anchor
#     parent:   {name, kind},                            one-hop signpost up
#     property: {name} }                                 never grandparents

whole leases($id)
# → { lease fields,
#     leaseParties: [ { role, person: {name, email} } ], owned link → shallow far side
#     unit: {name} }
```

---

## 4. Writes: every `set` mode (make-true)

```
op createOrder($loc: id, $opened: date, $descr: text, $priority: int <= 5) =
  set workOrders { location: $loc, opened: $opened, descr: $descr, priority: $priority }
# mode: CREATES (no identity; required fields covered; defaults fill the rest)

op assignVendor($order: id, $rev: int, $vendor: id) =
  set workOrders { id: $order, rev: $rev, vendor: $vendor, status: "scheduled" }
# mode: CONVERGES (id) with compare-and-set (rev)

op ingestReading($meter: id, $at: datetime, $value: int) =
  set readings { meter: $meter, at: $at, value: $value }
# mode: CREATES-OR-CONVERGES (covers unique (meter, at)): idempotent ingestion;
# webhook redelivery is a no-op by construction

op closeStale =
  set workOrders(status == "open" and opened <= 2025-12-31) { status: "void" }
# mode: BULK. The condition is the entire specification

op completeItem($item: id) =
  set workItems { id: $item, done: true }

op raiseAlert($kind: text, $key: text, $title: text, $sev: text, $today: date) =
  set alerts { kind: $kind, key: $key, title: $title, severity: $sev,
               status: "active", raised: $today }
# creates-or-converges on alerts.key: the alert lifecycle upsert. Creation-
# capable, so every required field is covered; `status` restated so a resolved
# alert re-raises (converge touches only the fields you describe)
```

```
# planned: `list of { … }` parameters are [lean]; the parser refuses them today.
op recordInvoice($vendor: id, $number: text, $issued: date, $total: money,
                 $lines: list of { descr: text, amount: money }) =    # [lean]
  let inv = set invoices { vendor: $vendor, number: $number,
                           issued: $issued, total: $total }
  set invoiceLines each $lines { invoice: inv.id, descr: item.descr,
                                 amount: item.amount }
  # one op, many statements → ONE atomic command group; triggers fire on commit
  # `let x = set …` [settled] binds a CREATE-mode write and yields a row
  # descriptor (id, rev, the fields written) usable in LATER statements of the
  # same op. Only creates bind: `let` on a converge, a CAS write or a bulk
  # write is a compile error (`unbindable_write`).
  # [lean]: `each $list` repeats the statement per item with `item.` field access
```

Money arithmetic: `+ - *` stay exact `money`; `/` and `avg` yield `decimal` at a
fixed fractional scale of 12, and **nothing rounds implicitly**:

```
op setRent($lease: id, $rev: int, $annual: money) =
  set leases { id: $lease, rev: $rev, rent: round($annual / 12, 0) }
# `rent: $annual / 12` alone is `implicit_rounding`. A decimal never lands in a
# money slot on its own. Only round(x, 0), floor and ceil convert: money IS
# minor units, so `round(x, 2)` asks for digits finer than the smallest unit
# there is, and is the same error.
# `round` is HALF-UP (away from zero), globally. Division uses the same tie-break
# when it reduces scale. Need half-even? Express it in app code.
# `/ 0` is a typed runtime error, uniform with mod(a, 0).
```

```
# planned: `list of { … }` parameters are [lean]; the parser refuses them today.
op splitInvoice($invoice: id, $split: list of { workItem: id, weight: int },
                $total: money) =
  set invoiceLines each $split {
    invoice:  $invoice,
    workItem: item.workItem,
    amount:   allocate($total, item.weight)   # THIS element's part
  }

op splitFee($fee: id, $split: list of { party: id, weight: int },
            $total: money) =
  set feeLines each $split {
    fee:    $fee,
    party:  item.party,
    amount: allocate($total, item.weight, last)   # residual to the LAST party
  }
# allocate(total, weights [, mode]) is the blessed money split: parts sum to
# `total` EXACTLY (floor each share, hand out the remaining minor units one
# apiece down a queue). allocate(100.00, (1,1,1)) = 33.34/33.33/33.33, never
# 99.99. The optional mode names WHICH party absorbs the odd minor unit
# largestRemainder (the default) | first | last | largestShare, because that is a
# business rule and has no mathematical answer. An unknown mode is a compile error,
# never a silent default. `rotation` is deliberately not a mode: it would have
# to remember what it did last time, and that is application state.
# Legal ONLY inside an `each` over a LIST PARAMETER, with
# the weight as the element field. Anywhere else gives `invalid_allocate`.
# Query-derived weights are not expressible; the caller passes what it fetched.
# (`each` over a list parameter is [lean]; the allocation semantics are settled.)
```

Every `del`, like every other callable thing, lives inside an `op` wrapper.
There is no top-level statement form:

```
op removeItem($item: id) =
  del workItems { id: $item }                       # addressed remove

op pruneTelemetry($before: date) =
  del events(at <= $before)                         # bulk remove (old telemetry)

op removeOrder($id: id) =
  del workOrders { id: $id }                        # takes owned workItems;
                                                    #   invoiceLines.workItem refs RESTRICT →
                                                    #   error if billed work exists

op removeBuilding($bldg: id) =
  del locations { id: $bldg } subtree               # explicit tree+owned cascade

op purgeEvents =
  del events(true)                                  # delete-everything: stated, greppable
```

---

## 5. Triggers: blind, clock or listener, envelope only

The top-level `when … -> run x` and `every … -> run x` lines in this block do
not parse. A trigger is written inside a routine, as a `trigger` clause (checklist 12a, 2026-10-08), and section 6 shows the form.

```
# planned: top-level `when`/`every` lines do not parse; use a routine `trigger`.
when estate.readings.created           -> run meterAnomaly    debounce 5m
when estate.events.created             -> run securityWatch
when estate.workOrders.created         -> run orderDispatch
when estate.payments.created           -> run paymentReconcile
every day 08:00Z                        -> run leaseExpiry
every day 02:00Z                        -> run invoiceAging
every 15m                              -> run deviceHeartbeat coalesce
```

---

## 6. Routines: tier 1 rules (pure .ordo; scopes derived, sandbox moot)

What a routine may touch follows its **trigger provenance**: clock-woken
(`trigger every …`) means guards pass; listener-woken (`trigger when …`) means
the run sees only rows reachable from `$trigger.ids`, unless the `run` header
carries `unguarded`. §1's ontology carries no guards, so the bound is
unobservable there. All three routines below are nonetheless written as they
must be once §8's guarded version of the same domain is deployed, which is the
version you should copy.

> **Built since 2026-10-08 (checklist 12a), with a write body.** A routine
> whose body is write statements compiles and runs: the two below. A body
> that reads a pipeline and emits per row with `each ->` is built too. A
> change run is already bounded to the rows the change reached, so the
> blocks after them need no `$trigger` to say so. See `howto-routines.md`.

```
routine staleAlerts in estate {                  # CLOCK-woken: guards pass
  trigger every 15m
  run($today: date):
    set alerts(status == "active" and raised < $today - 30d) { status: "dismissed" }
}

routine dailyResolve in estate {                 # once a day, 02:00 UTC
  trigger every day 02:00Z
  run:
    del alerts(status == "dismissed")
}

routine motionSeen in estate {                   # LISTENER-woken: bounded to
  trigger when estate.events.created debounce 30s  #   the new events and rows
  run:                                           #   one ref away
    set events(kind == "motion") { kind: "motion-seen" }
}
```

```
routine leaseExpiry in estate {                  # CLOCK-woken: guards pass
  trigger every day 08:00Z
  run($today: date):
    leases(status == "active" and end is not null and end <= $today + 60d)
      [id, end, unit.name]
    each ->
      set alerts { kind: "lease", key: "leaseExpiry:" + id,
                   severity: "warning", status: "active", raised: $today,
                   title: unit.name + " lease ends " + end }
}
# converge-or-create on alerts.key: creation-capable, so every REQUIRED field
# must be covered (`kind`, `title`), and `status` is restated so a resolved
# alert re-raises (converge touches only described fields)
```

```
routine securityWatch in estate {                # LISTENER-woken: the run is
  trigger when estate.events.created             #   bounded to the new events
  run($today: date) unguarded:                   # a create-by-key row is not
    events(kind == "motion" and device.armed == true)   # reachable from the new
      [id, at, deviceId: device.id, device.location.name]   # events, so say so
    each ->
      set alerts { kind: "security", key: "motion:" + deviceId,
                   severity: "critical", status: "active", raised: $today,
                   title: "Motion at " + name }
}
# the condition a trigger is too blind to hold is the rule's first line
```

```
routine meterAnomaly in estate {
  trigger when estate.readings.created debounce 5m
  run($today: date) unguarded:                          # ← the listener bound,
                                                      #   acknowledged. Why:
    let baseline = readings reduce { a: avg value }   # this let reads ALL
                                                      # readings, which is NOT
                                                      # reachable from the
                                                      # new rows. A listener
                                                      # run is bounded to the
                                                      # changed rows, so ranging
                                                      # wider must be written.
                                                      # (Also a global
                                                      # baseline: per-meter
                                                      # baselines need a
                                                      # per-group let, which is a known
                                                      # tier-1 limit.)
    readings(value > 3 * baseline.a)
      [id, value, meterId: meter.id, meter.kind, meter.unit.name]
    each ->
      set alerts { kind: "meter", key: "meter:" + meterId,
                   severity: "warning", status: "active", raised: $today,
                   title: kind + " anomaly in " + name }
}
# `avg` yields decimal; `3 * baseline.a` is compared exactly against `value`.
# No rounding step exists in between, and none is applied implicitly
```

Tier 2 (heavy intelligence such as LLM categorization, vision and retraining) is not
hosted: external service-principals call the generated API with capability
tokens, egress declared on the consent page.

---

## 7. Notifications: platform-owned delivery

```
# planned: `channel` and `route` are refused today (checklist 19g, 17r).
channel email { provider: resend }
channel push  { provider: webpush }
channel sms   { provider: twilio }

route alerts.critical -> [push, sms] fallback email
route alerts.warning  -> [push]      fallback email
route alerts.info     -> [in-app]

# domains emit `notify` proposals; the platform routes, minimizes payloads
# ("Alert: Unit 4B", with detail revealed in-app), records delivery receipts as
# commands. No provider credentials ever touch domain code.
```

---

## 8. Authorization: guards, `acl`, principals

§1's ontology is single-tenant: no entity carries a guard, so every op sees
every row. Below is the same domain declared for a space several people share.
Authorization is written **in the shape**, never in a policy file.

### 8.1 `acl` on a root entity

```
entity properties {
  name:     text
  address:  text        sealed
  acquired: date
  acl (read, write, admin)               # ← derives the whole permission system
}

# derives, visibly: a real link entity
#   propertiesAccess { propertie -> properties owned, member -> members owned,
#                      level, unique (property, member) }
# plus guards on `properties` AND its entire owned subtree (locations and units,
# with nothing written on them), plus grantPropertiesAccess / revokePropertiesAccess
# gated at the highest level (admin). Levels order left→right.
#
# Bootstrap, platform-derived (not written). See 8.8 for the full rule set:
# the creating write is exempt from the guard it is bootstrapping; a creating
# space MEMBER receives the `admin` row in the SAME command group; guests,
# routines, sources and the system receive none; a space owner may always
# administer any acl row in their space, checked at the door.
```

### 8.2 Hand-written guards: the entity outside the owned subtree

`leases.unit` is an **association**, not `owned`, so `leases` is not in
`properties`' owned subtree and inherits nothing. It guards itself, both verbs:

```
entity leases {
  unit ->  units
  start:   date
  end:     date?
  rent:    money        sealed
  status:  text = "active"

  access read   unit.property.propertiesAccess(member == $caller)
  access write  unit.property.propertiesAccess(
                  member == $caller and (level == "write" or level == "admin"))
}

# a guard is a $caller-anchored relationship WALK: dot-paths toward "exactly
# one", then one reverse collection step, then an existence test, which is an indexed
# semi-join of `sql` class. The closed guard grammar has no ordering operator, so
# levels are enumerated as an or-disjunction (note the parens: mixing and/or
# without them does not parse).
#
# `access read` with no `access write` is a COMPILE ERROR. Write-open must be
# spelled, never implied.
```

### 8.3 Link entities never inherit

Two `owned` refs means no single root to inherit from, so a link entity always
declares its own guards:

```
entity leaseParties {
  lease  -> leases    owned
  person -> people    owned
  role:   text = "tenant"
  unique (lease, person)

  access read   lease.unit.property.propertiesAccess(member == $caller)
  access write  lease.unit.property.propertiesAccess(
                  member == $caller and (level == "write" or level == "admin"))
}
```

### 8.4 Deliberately open writes

```
entity maintenanceRequests {                 # tenant portal: anyone with the
  unit ->  units                             #   link may file one
  descr:   text
  filed:   date

  access read   unit.property.propertiesAccess(member == $caller)
  access write  open                         # explicit, greppable, in the diff
}
# the word is `open`. `access write true` does NOT parse. The boolean literal
# is not a guard, and a position that accepted it could not tell a decision to
# open from an expression that is true today
```

### 8.5 Bypass is explicit and greppable

```
op allLeases unguarded unbounded =           # admin/reporting op: guards skipped
  leases[start, end, status, unit.name]      #   the skip is flagged in the manifest, and
  sort -start                                #   any op composing it must carry
                                             #   its own `unguarded` marker.
                                             # TWO markers, two decisions: one
                                             #   about authority, one about work.
                                             #   Neither implies the other.
```

### 8.6 `$caller` in a body

```
op myUnits unbounded =
  leaseParties(person == $caller)[lease.unit.name, lease.unit.property.name]
  sort name
# `$caller` scoping is authorization, not a cardinality bound: the compiler
# cannot prove how many units any given caller holds, so the acknowledgment
# stands even though the result is small in practice.

# guards decide what exists for this caller; `$caller` in the body decides what
# they asked for. $caller is the one reserved parameter and is never client-suppliable.
```

### 8.7 Principals and groups (runtime consent, shown for completeness)

```
# planned: the `principal` declaration is refused today. The mechanism is built.
principal simplefinHook { kind: source, writes: [estate.readings] }
principal cameraHub     { kind: source, writes: [estate.events] }
principal ellanor       { kind: agent }

# capability token minted at consent time (runtime, never IaC):
#   agent:ellanor may call { openOrders, vendorSpend, createOrder } on spc_01J...
#
# a GROUP is a named collection of principals, usable anywhere a principal is,
# so one row in propertiesAccess can name the group `fieldTechs` at level
# "write", and group membership (runtime state, one command) is what changes
# when the crew changes. Groups are not a role hierarchy: the four space roles
# (owner/manager/editor/viewer) stay fixed and separate. Groups expand in the
# ACCESS LAYER ONLY: `== $caller` in an op body is row equality and never
# matches a group you belong to, and `count` over propertiesAccess counts the
# group as ONE row.
#
# ROUTINES are bounded by TRIGGER PROVENANCE, not by their principal:
#   clock-woken (`trigger every …`)  -> guards pass; nobody outside chose when
#   listener-woken (`trigger when …`) -> may read/write ONLY rows reachable from
#                                        $trigger.ids
# because a listener fires on writes an OUTSIDER made (`ingested` from a source
# principal, `fed` from another space). A globally guard-exempt listener is a
# confused deputy steered by that outsider. A listener that must range wider
# says so on its run header: `run() unguarded:`. The derived scope manifest is
# orthogonal: it bounds which FIELDS, never which rows.
```

### 8.8 Seeding the second person: there is no `grant` statement

```
op assignProperty($property: id, $rev: int, $name: text, $manager: id) =
  set properties       { id: $property, rev: $rev, name: $name }
  set propertiesAccess { propertie: $property, member: $manager, level: "write" }
# one op = ONE atomic command group. `propertiesAccess`'s own derived write
# guard applies to the second statement in the ordinary way: the caller holds
# `admin` on that property, or is the space owner (checked at the door).
```

```
# BOOTSTRAP, all platform-derived (nothing below is written):
#  - the creating write on an acl-ROOT entity is exempt from the derived write
#    guard. It must be, since every path from that guard to $caller runs
#    through link rows that cannot exist before the row does. The exemption
#    covers the creating write and binds from the next touch onward;
#  - the creator receives the top level in the SAME command group, but only if
#    they are a space MEMBER. A guest (members row, null role) creating an
#    acl-bearing row gets NO access row: auto-grant follows space membership,
#    not authorship;
#  - routine/system/source-created rows get none either;
#  - the space-owner override is checked AT THE DOOR, once per call, before
#    planning, and not as an extra disjunct inside each row's guard. No acl row is
#    private from the space owner: that is the definition of ownership, every
#    such reach is an attributed command in the chain, and the consent page for
#    joining a space you do not own says so.
```

Creating the container and seeding a co-owner in one op is spelled with `let`
[settled]: a `set` whose derived mode is CREATE binds to a row descriptor.

```
op openProperty($name: text, $addr: text, $acquired: date, $coOwner: id) =
  let prop = set properties { name: $name, address: $addr, acquired: $acquired }
  set propertiesAccess { propertie: prop.id, member: $coOwner, level: "admin" }
# the create is guard-exempt (G10a); the creator takes the auto-grant; $coOwner
# gets admin; all in ONE atomic command group. `prop` carries id, rev and the
# fields this statement wrote, and is usable only in LATER statements, never
# in this one, and never in this statement's condition.
#
# `let` binds CREATE mode only. On a converge, a CAS write (`id` + `rev`) or a
# bulk write it is a compile error, `unbindable_write`: those either address a
# row whose id the caller already holds, or affect an unknown number of rows.
```

### 8.9 Leaving a tenant: `release member`

```
# standalone program: these entities replace the estate ones
entity people {
  member  -> members unique
  manager -> members?
}

entity invoices {
  owner  -> members
  status: text = "open"
}

entity drafts {
  author -> members
  body:   text
}

release member when not invoices(owner == $member and status == "open") {
  remove drafts.author
  reassign invoices.owner to people(member == $member).manager or owner
}
# One rule per program. `open` lets any member leave, `locked` lets nobody,
# and `when …` lets a member leave once every condition holds. `$member` is the
# seat being released; `$caller` has no meaning here. With no `release` line
# a program's members are LOCKED (see howto-leave.md).
# Rows that point at the leaver stay by default and point at the seat, which keeps its name and reads status left.
# The block may remove them or reassign them; `owner` is the tenant's
# longest-standing other owner. The leave is `std.space.leave`.
```

### 8.10 Answering while the tenant is paused: `nopause`

```
# standalone program: these entities replace the estate ones
entity invoices {
  number: text unique
  status: text = "open"
  total:  int
}

op openInvoices nopause =                    # still answers while paused
  invoices(status == "open")[number, total]
  sort number
  take 50

op markPaid($number: text) nopause =         # so a paused tenant can pay up
  set invoices(number == $number) { status: "paid" }

op addInvoice($number: text, $total: int) =  # refused `space_paused`
  set invoices { number: $number, total: $total }
# A paused tenant refuses every op with `space_paused`, except the ones the
# app marks `nopause`. The word sits beside `unguarded` and `unbounded` and is
# flagged in the manifest. Export, resume and delete answer regardless. See
# howto-pause.md.
```

### 8.11 Anyone may read it: `public`

```
entity notices {                             # unguarded: no `access`, no `acl`
  title:    text        public               # one field marked
  body:     text        public
  postedBy: text                             # private: no public op may read it
}

entity noticeFiles {
  notice -> notices owned
  file:     blob?       public               # served with no token, on a public row
  public                                     # this entity line marks every field
}

op noticeBoard public =                      # a visitor calls it with no token
  notices[title, body]
  sort title
  take 50

op noticeAuthors =                           # not public: it reads `postedBy`
  notices[title, postedBy]
  sort title
  take 50
# A tokenless call to a public op runs as `$anonymous` in the deployment's
# space. A public op reads public fields only, in a projection, a condition or
# a sort (`public_reads_private`). Refused: `public` on a sealed field
# (`public_sealed`), on an entity with `access read` or `acl`
# (`public_guarded`), `$caller` in a public op (`public_caller`), and a public
# write that is not a plain create (`public_write_not_create`), that writes into
# a guarded entity (`public_write_guarded`) or that aims a ref at a private row
# (`public_write_ref`). `public*` on an entity line also marks the
# entities it owns. See howto-public.md.
```

---

## 9. Capstone: complete micro-app in two pages, `plantcare`

```
domain plantcare @ 0.1.0

entity plants {
  name:      text
  room:      text?
  everyDays: duration(ms) = 7d               # watering cadence, a first-class duration
  notes:     text = ""     sealed
}

entity waterings {
  plant -> plants   owned
  at:    date
  by:    text = ""
}

entity alerts {                        # the routine's nags are ordinary rows
  key:      text unique                #   upsert-by-key lifecycle
  severity: text = "warning"
  title:    text
  status:   text = "active"
  raised:   date
}

op thirsty($today: date) unbounded =  # execution-relative date passed in (SDK fills)
  plants[name, room, everyDays]  waterings[at]
  sort -waterings.at
  take[plants.id] 1
  | (at <= $today - everyDays) or at is null   # never-watered has null at.
  sort cond(at is null -> 0, 1), at            # Nulls sort LAST always, so the
                                               #   never-watered plants need an
                                               #   explicit key to reach the top

op history($plant: id) =
  waterings(plant == $plant)[at, by]
  sort -at
  take 30

op water($plant: id, $by: text, $today: date) =
  set waterings { plant: $plant, by: $by, at: $today }
  set alerts(key == "thirsty:" + $plant and status == "active") { status: "resolved" }

op addPlant($name: text, $room: text, $everyDays: duration(ms)) =
  set plants { name: $name, room: $room, everyDays: $everyDays }

op pruneLogs($before: date) =            # maintenance op: prune old logs
  del waterings(at <= $before)

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
                   raised: $today,
                   status: "active" }   # converge touches only described fields:
                                        #   without this, a resolved alert never
                                        #   re-raises
}

# Deploy: `ordo check`, then `ordo apply` → API ops (thirsty, history, water,
# addPlant, pruneLogs), the daily check, auth, permissions and audit. The
# generated list/detail/form UI and push notifications are planned (`app`,
# `page`, `channel` and `route` are refused today).
#
# Note what this app is NOT: multi-tenant. plantcare has one user, so it
# declares no guards. The moment a second person shares the data, §8's
# authorization constructs stop being optional (see `tutorial.md` Part 2).
```
