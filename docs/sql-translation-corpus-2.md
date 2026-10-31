# SQL to Ordo translation corpus, set 2

This is 93 more SQL patterns, disjoint from set 1 (`sql-translation-corpus.md`)
and skewed toward what app developers ship: product analytics, operational
queries, reporting, data quality, temporal modeling and utility functions. Each
has a verdict, and the failures are tallied in a prevalence study at the end.
That study is how the owner ranked what to build and what to leave out.

Counts as of 2026-10-31: **62 direct, 5 problematic, 26 not possible** (24 by
design, 2 gaps). `scripts/check-sql-corpus.py` runs the Ordo code of 66 entries
through `ordo check` and tries a probe for each not-possible verdict it can, so
a stale verdict fails the script. The other 27 entries are prose verdicts.

Verdicts are ✅ direct, ⚠️ problematic and ❌ not possible (*by design* or
*gap*), as in set 1. The reference schema is set 1's, plus `sessions`,
`events`, `accounts`, `counters` and a few small entities, listed in the
script.

---

## M. Product analytics

**M1. DAU (daily active users)**
```sql
SELECT date_trunc('day', started), COUNT(DISTINCT customer_id) FROM sessions GROUP BY 1;
```
`sessions[customer] group day(started) { dau: distinct customer }`
✅ direct (post-pin): date-part functions accept datetime.

**M2. MAU / rolling 30-day actives per day**: ❌ **by design (window family, declined, DL §11)**:
rolling-window distinct counts; per-day evaluation of a trailing window.

**M3. Day-1/Day-7 retention cohorts**
```sql
-- cohort by signup week, measure return activity in week N
```
✅ direct (post-pin): cohort assignment is ✅ (`let cohort = customers group week(joined) …`,
let-join back), but "active in week N after *their own* cohort week" compares
`week(started)` against a per-row cohort value → needs **date arithmetic
between row values** (`started - customers.joined <= 7d`), legal if
field-to-field comparisons are blessed, now pinned. ✅ (post-pin).

**M4. Funnel: viewed → carted → purchased conversion**
```
let v = events(kind == "view")[customer]    distinct
let c = events(kind == "cart")[customer]    distinct
let p = events(kind == "purchase")[customer] distinct
v reduce { viewers: count }
```
⚠️: per-stage sets are fine; the *counts* are fine (`reduce { n: count }`);
strict *ordered* funnels (cart AFTER view) need per-customer event ordering →
❌ window family for the strict version.

**M5. Churned customers (no activity in 60 days, active before)**
```
customers[*]
match any orders(placed < $cutoff)
match none orders(placed >= $cutoff)
```
✅ direct: chained match verbs.

**M6. Month-over-month revenue growth**: ❌ **by design (window family, declined, DL §11): lag on
grouped output)**. The monthly totals are trivial; the *delta between adjacent
rows* is not expressible. (Tallied, this exact shape recurs.)

**M7. Year-over-year same-month comparison**
```
let cur  = orders(placed >= $curStart  and placed < $curEnd)  reduce { r: sum total }
let prev = orders(placed >= $prevStart and placed < $prevEnd) reduce { r: sum total }
cur[curRev: r, growth: round(r / prev.r, 4)]
```
✅ direct: explicit periods as params beat implicit lag here; growth ratio via
let-scalars in a projection.

**M8. RFM segmentation (recency/frequency/monetary per customer)**
```
customers[id, name]  orders[placed, total]
group customers.id, customers.name {
  recency: max placed, freq: count, monetary: sum total }
```
✅ direct. (Scoring into quintiles → ❌ ntile; bucket client-side or with
explicit thresholds ✅.)

**M9. Histogram with fixed buckets (width_bucket)**
```sql
SELECT width_bucket(total, 0, 1000, 10), COUNT(*) FROM orders GROUP BY 1;
```
✅ direct (post-ruling): `group floor(total / 100) { n: count }`.

**M10. Pareto: which products make 80% of revenue**: ❌ **by design (window
family: cumulative share)**. Top-N by revenue ✅; the 80% cutoff needs a
running total.

**M11. Most-common first product bought**
```
orderItems[product: product.name, customer: order.customer, at: order.placed]
sort at
take[customer] 1
group product { n: count }
sort -n
```
✅ direct: take[] then re-aggregate; genuinely nicer than the SQL.

**M12. Average order value by acquisition channel**: ✅ plain group +
dot-path.

**M13. Time-to-first-purchase distribution**
```
customers[id, joined]  orders[placed]
sort placed
take[id] 1
group … { med: median (placed - joined) }
```
✅ direct (post-pin): `date - date → duration` pinned; durations are ordered
scalars.

**M14. Sessions longer than 30 minutes**: ✅ (post-pin) `sessions(ended is
not null and ended - started > 30m)`.

**M15. Bounce rate (single-event sessions)**
```
sessions[*]
match none events(seq > 1)
```
✅ direct: sessions with no second event; count them with `reduce { n: count }`.

---

## N. Operational app queries

**N1. Unread notification badge count**: ✅ `notifications(user == $u and
read == false) reduce { n: count }`.

**N2. Soft-delete filtering everywhere**
```sql
SELECT * FROM posts WHERE deleted_at IS NULL;
```
✅ pattern (post-ruling): no entity filter construct; the blessed pattern is
composition: `activePosts = posts(deleted is null)`, everything sources it.

**N3. Page 5 of results (OFFSET)**
```sql
SELECT ... ORDER BY placed DESC LIMIT 25 OFFSET 100;
```
❌ **by design**: keyset-only pagination; no random page jumps. Defensible
(OFFSET is broken under writes anyway) but *jump-to-page UIs* can't be built.
Document loudly.

**N4. ORDER BY expression (case-insensitive sort)**
```sql
SELECT * FROM customers ORDER BY lower(name);
```
✅ direct (post-pin): expression keys legal in sort/take[]/group.

**N5. NULLS LAST ordering**
```sql
SELECT * FROM shipments ORDER BY shipped DESC NULLS LAST;
```
✅ (post-pin): nulls sort last, always, no modifier.

**N6. Search across several columns**
```
customers((name contains $q) or (country contains $q))[*]
```
✅ direct. (`email` is sealed, so `contains` on it is refused with
`sealed_unsupported`; changed 2026-10-30.)

**N7. Clamp a value on update (LEAST/GREATEST)**
```sql
UPDATE accounts SET credit = LEAST(credit + $x, limit) WHERE id=$a;
```
✅ direct (post-ruling): `set accounts { id: $a, credit: least(credit + $x, limit) }`.

**N8. Conditional update value (CASE in SET)**
```sql
UPDATE orders SET status = CASE WHEN paid >= total THEN 'done' ELSE 'partial' END ...;
```
✅ direct (post-ruling): `status: cond(paid >= total -> "done", "partial")`.

**N9. CASE in projection (display bucketing)**
```sql
SELECT name, CASE WHEN stars >= 4 THEN 'good' WHEN stars >= 2 THEN 'ok' ELSE 'bad' END ...
```
✅ direct (post-ruling): `cond(stars >= 4 -> "good", stars >= 2 -> "ok", "bad")`.

**N10. Duplicate detection (same email, count > 1)**: ✅ `group email
{ n: count } | n > 1` (moot with `unique`, but the pattern generalizes).

**N11. Orphan cleanup**: ✅ `del drafts(…) `+ `match none`?, careful:
`del` takes a condition, **not a pipeline**: `match none` is a step, not a
condition. ✅ pattern (post-ruling), write conditions stay row-scoped; cleanup is a
rule (`match none` query, `each -> del`). System does it, attributed and
scheduled.

**N12. Keep only the latest 100 rows per user (delete the rest)**
```sql
DELETE FROM logs WHERE id NOT IN (SELECT id ... ROW_NUMBER() ... <= 100);
```
❌ **not possible today (gap)**: changed 2026-10-30. This entry said
retention policies (`policy retention { logs keep 100 per user }`) were
settled. The compiler refuses `policy`: it is reserved, specified in
`definition-language.md` §8 and not built (checklist 19f). Until it is built,
trimming is a routine that finds the rows past the limit and deletes them, and
"past the 100th per user" needs a rank, which Ordo does not have. A time limit
(`logs(at < $cutoff)`) is the form that works today.

**N13. Batch chunked delete (DELETE … LIMIT)**: ❌ **by design**: `del`
has no `take`; bulk deletes are atomic groups, chunking is the platform's
concern. Fine, but write it down.

**N14. Optimistic-lock read-then-write**: ✅ the native model (`rev` CAS).

**N15. Job-queue claim (FOR UPDATE SKIP LOCKED)**: ❌ **by design**: queues
are platform machinery, not app queries. Apps declare routines/triggers.

**N16. Rate limiting: requests in the last minute**
```
events(actor == $a and at >= $since)
reduce { n: count }
```
✅ direct: changed 2026-10-30. `at since 60s` is refused: `since` over units
smaller than a day needs a datetime clock, which is not built. The caller
passes the window start as a datetime parameter.

**N17. Feature-flag percentage rollout**: ✅ (post-ruling), deterministic
bucketing: `customers(mod(hash(id), 100) < $pct)[*]`; `random()` stays out by
design.

**N18. Exists-check before insert**: ✅ obsoleted by converge-or-create.

---

## O. Reporting

**O1. Pivot months into columns (crosstab)**
```sql
SELECT product, SUM(...) FILTER (WHERE month=1) AS jan, … ;
```
```
orderItems[price]
group product.name {
  jan: sum (cond(month(order.placed) == 1 -> price, 0)),
  feb: sum (cond(month(order.placed) == 2 -> price, 0)) }
```
⚠️: conditional aggregates *can* pivot a fixed, known column set (verbose but
works); **dynamic** pivots (columns from data) ❌ by design, presentation
tier's job.

**O2. Unpivot (columns→rows)**: ❌ by design (no column-to-row reshaping;
schema should have been long-form).

**O3. Running account balance statement**: ❌ **by design (window family)** (running
sum). The ledger answer: balance as maintained state (rule/field), which the
platform makes safe. Tallied.

**O4. Statement with opening/closing balance per month**: ⚠️, closing =
`let` per period ✅; but per-month opening chained from prior closing without
windows = client-side or maintained state.

**O5. Top mover: this month's top product vs last month's rank**: ⚠️, both
months' rankings via two lets + let-join ✅; *rank numbers* themselves ❌ (D3
family). Comparison by revenue works; by rank position doesn't.

**O6. Weighted average**
```sql
SELECT SUM(price*qty)/SUM(qty) FROM order_items;
```
`orderItems reduce { w: sum (price * qty), q: sum qty }` then `w / q` in
projection, ✅ direct (expression args now settled).

**O7. Average of per-group sums (avg order value per customer, then overall)**
```
let per = orders group customer { t: sum total }
per reduce { a: avg t }
```
✅ direct.

**O8. Contribution % of each category to total revenue**: ✅ (let-scalar in
post-group projection, settled in D9 ruling).

**O9. Min/max/median/p90 order value in one row**: ✅ one `reduce` with four
aggregates.

**O10. Count of distinct pairs (customer, product)**: ✅ (post-pin):
`distinct (customer, product)`.

---

## P. Data quality

**P1. Dedup keep-oldest per email**: ✅ `sort joined  take[email] 1`.

**P2. Find near-duplicate names (similarity)**: ❌ by design (fuzzy matching
is client/ops tier).

**P3. Invoice total ≠ sum of lines**
```
let lineSums = invoiceLines group invoice { s: sum amount }
invoices[number, total]  lineSums[s]
| total != s
```
✅ direct (let-join): and `!=`'s null totality means invoices with *no* lines
surface too (s null → `total != null` … wait: `total != s` with s null is
**true** under the truth table → they appear, correct and deliberate; worth a
conformance case as the truth table paying rent).

**P4. Rows violating cross-field rules (end < start)**: ✅ (post-pin).

**P5. Sequence gaps (missing invoice numbers)**: ❌ **by design (window family, lag)**.
Tallied.

**P6. Orphaned refs**: ✅ `match none` / impossible by construction with FK
enforcement, mostly moot in Ordo.

**P7. Whitespace/trim cleanup**: ✅ `trim(t)` (post-ruling).

**P8. Mixed-case dupes**: ✅ `normalize(lower)` at the shape.

---

## Q. Temporal modeling

**Q1. Current effective record (SCD: valid_to IS NULL)**: ✅ condition.

**Q2. As-of query (record effective on date X)**: ✅ `prices(from <= $x and
(to is null or to > $x))`.

**Q3. Latest version per document**: ✅ `take[doc] 1`.

**Q4. Overlapping reservations (pairwise)**: ❌ **by design**, self-join family (B11).
Platform answer: exclusion at write time via a rule, not a query.

**Q5. Merge adjacent/overlapping ranges (islands)**: ❌ **by design**, window family.

**Q6. Duration accumulated per day across sessions crossing midnight**: ❌
**by design**: requires row-splitting (generate_series family, set 1 F5).

---

## R. Hierarchy redux

**R1. Subtree size for *every* node**: ⚠️ (corpus-1 F4), per-anchor ✅;
all-nodes-at-once needs correlated `under` → rule-materialized counts.

**R2. Path-to-root as text ("A > B > C")**: ❌ **by design**: tree entities
expose `depth` (F3), and `definition-language.md` §3 leaves path-string
rendering to the client. Fetch the ancestors with `above`, sorted by `depth`,
and join the names there. Changed 2026-10-30: this entry called it a gap.

**R3. Immediate children only**: ✅ `categories(parent == $x)`.

**R4. Leaves (no children)**: ✅ `match none categories` (self-ref reversed:
needs confirming match-none works on self-refs; conformance case).

**R5. Siblings of a node**: ✅ `categories(parent == $p and id != $x)`.

---

## S. Utility functions & expressions

**S1. ROUND to cents**: ✅ `round(x, 2)`.
**S2. CEIL / FLOOR**: ✅ (post-ruling).
**S3. ABS**: ✅.
**S4. MOD (even/odd, striping)**: ✅ `mod(a, b)` (post-ruling).
**S5. POWER/SQRT/LN**: ❌ by design (scientific math is not app-query tier;
routines/tier-2 for stats).
**S6. GREATEST/LEAST**: ✅ `least(a, b)`, `greatest(a, b)` (N7). Changed 2026-10-30: this line still said gap.
**S7. NULLIF(a, b)**: ✅ `cond(a == b -> null, a)` (post-ruling).
**S8. COALESCE chains of 3+**: ✅ `coalesce(a, coalesce(b, c))`, or pin
n-ary coalesce (one-liner).
**S9. CAST text→int**: ❌ by design (schema types or bust; ingestion
transforms belong to tier-2/migrations).
**S10. to_char date formatting**: ❌ by design (display is client tier).
**S11. Interval literal arithmetic (`now() - interval '7 days'`)**: ✅
`$today - 7d` / `since 7d`.
**S12. EXTRACT(dow), EXTRACT(hour)**: ✅ `hour()`, `dow()` (ISO 1–7, post-ruling).
**S13. String split_part('a/b/c', '/', 2)**: ❌ by design (string
surgery in queries is a smell; `matches` + client handle the rest).
**S14. LENGTH filter (`length(bio) > 200`)**: ✅.
**S15. Concatenated display name (`first || ' ' || last`)**: ✅ `+`.

---

## T. Write-side patterns redux

**T1. Insert with returning full row**: ✅ (post-pin), addressed writes
return the written row descriptor.
**T2. Conditional insert (insert only if under quota)**: ✅ system (post-ruling), counter field + `check used <= seats` invariant
(compiled to a DB CHECK): increment + insert in one atomic group; the check
aborts the group on violation. No op-level precondition machinery.
**T3. Move children then delete parent**: ✅ two statements, one atomic op.
**T4. Toggle a boolean**: ✅ (post-pin), RHS = full expression grammar.
**T5. Bulk archive flag by age**: ✅ `set logs(at <= $cutoff) { archived: true }`.
**T6. Reparent a subtree**: ✅ `set categories { id: $n, parent: $new }`
(cycle check enforced by platform).
**T7. Cascade-cancel children when order voided**: ✅ rule (trigger on
update + bulk set), the platform's whole point.
**T7a. Cross-row invariant enforced at write time ("the line's currency must
match its invoice's")**
```sql
-- Postgres: a trigger, or copy invoice.currency onto every line and CHECK it
CREATE TRIGGER … BEFORE INSERT OR UPDATE ON invoice_lines …
```
```
entity invoiceLines {
  invoice -> invoices  owned
  currency: text
  amount:   money(2)
  check currency == invoice.currency
}
```
❌ **not possible today (gap)**: changed 2026-10-30. This entry said `check`
walked outgoing refs. The compiler refuses it, `check_beyond_row`: a database
CHECK sees one row, and a ref walk needs triggers that are not built. Same as
set 1, J6a. Kept working: a `check` over the row's own columns. Copy the
currency onto the line in the write and compare the two columns of that row.
Aggregates over other rows ("lines must sum to the invoice total") stay
rule-tier, as T2's counter does.

**T9. Create a row and grant access to it in one op**: ✅ (post-ruling):
`let x = set …` binds a write whose derived mode is CREATE and yields a row
descriptor (`id`, `rev`, the fields written) usable in later statements of the
same op:
```
op openBoard($n: text, $co: id) =
  let b = set boards { name: $n }
  set boardsAccess { board: b.id, member: $co, level: "write" }
```
One atomic command group. `let` on a converge, a CAS write or a bulk write is
the compile error `unbindable_write`, those address a row whose id the caller
already holds, or affect an unknown number of rows. This is the closest Ordo
form of `INSERT … RETURNING id` feeding a second `INSERT` in one transaction.

**T8. Increment-or-insert counter (upsert += 1)**
```sql
INSERT ... ON CONFLICT (key) DO UPDATE SET n = counters.n + 1;
```
```
set counters { key: $k, n: n + 1 }
```
✅ **direct**: changed 2026-10-31. Addressed by the unique `key`, it is one
`INSERT … ON CONFLICT DO UPDATE`. A create reads each field as its default, so
the first write stores 1 (owner, 2026-10-31). A read field with no default is
refused when the op registers, naming the field. By id,
`set counters { id: $c, n: n + 1 }` increments an existing row.

---

## U. Windows redux (tallying prevalence, distinct concrete asks)

**U1. Nth-highest salary (classic)**: ❌ **by design (window family)**, rank. Top-N is
`sort -salary` then `take N`, but the Nth alone needs a rank or an offset, and
Ordo has neither.
**U2. Rank products within each year**: ❌ **by design (window family)**, rank-as-value.
**U3. Previous purchase gap per customer**: ❌ **by design (window family)**, lag.
**U4. 7-day moving average of signups**: ❌ **by design (window family)**, frames.
**U5. Cumulative % of goal**: ❌ **by design (window family)**, running sum.
**U6. First/last value shown on every row**: ❌ **by design (window family)**, window projection.

---

## Verdict summary (set 2)

Counted from the entries by `scripts/check-sql-corpus.py --counts`, 2026-10-30.

| Verdict | Count | Share |
|---|---|---|
| ✅ direct | 62 | 67% |
| ⚠️ problematic | 5 | 5% |
| ❌ not possible | 26 | 28% |
| Total | 93 | 100% |

Of the 26 not possible, 24 are by design and 2 are gaps (N12, T7a).

Problematic (6): M4, O1, O4, O5, R1, T8.

**Changed on 2026-10-30, when the entries were re-run against the compiler:**
S6 (`least`, `greatest`) went from gap to direct, M3 from problematic to
direct. N12 went from direct to gap (`policy retention` is reserved, not
built). T7a went from direct to gap (ref-walking `check` is refused). T8 went
from direct to problematic (create-or-increment on a unique key is refused).
N16 changed form (`since 60s` needs a datetime clock; pass the window start).
N6 changed fields (`contains` on a sealed field is refused).

**❌ by design (24):** the window family (M2, M6, M10, O3, P5, Q5, U1 to U6),
keyset-only pagination (N3), chunked delete (N13), queue claims (N15), unpivot
(O2), fuzzy matching (P2), pairwise overlap (Q4), row-splitting (Q6), and
path-to-root text (R2), and scientific math, casts, date formatting and string
surgery (S5, S9, S10, S13).

**❌ gaps (2):** retention policy (N12) and ref-walking `check` (T7a).

Set 2 is a little harder on Ordo than set 1 (66% direct in both, but more
windows). The same two holes dominate: windows, decided against, and
cross-row `check`, not built yet.

---

## Prevalence study of the failures

Method: cross-referencing an [empirical analysis of real-world query corpora](https://arxiv.org/pdf/2605.07840)
(feature frequencies over production-style workloads), the
[window-function interview canon](https://datalemur.com/blog/sql-window-functions-interview-questions)
(a proxy for what employers consider everyday SQL), and pattern-frequency
signals from [practitioner guides](https://interviewpilot.dev/blog/sql-leetcode).
Ratings: how likely a *typical application built on Ordo by a third party*
hits the pattern.

| Failed pattern (family) | Corpus hits | Prevalence | Evidence & judgment |
|---|---|---|---|
| ~~CASE/if()~~ **SETTLED via `cond(…)`** (N8, N9, S7) | 3 + latent | ~59% of real queries use CASE/IF, though most of that share migrates client-side under Ordo's architecture; the irreducible server core (bulk per-row values, rules, aggregate args, paginated sort keys) is what `cond` serves. | Flat, ordered, total, first-match-wins; no CASE vocabulary. |
| **Window family** (M2, M6, M10, O3, P5, Q5, U1–U6, + set-1 D3–D8, D10–D14) | 23 | HIGH for analytics apps | **DECLINED [settled]**: `extend` designed, preserved as v2 candidate, not admitted. Blessed patterns documented in spec §11 (client compute; opening-balance reduce + accumulate; rule-materialized state). Decision evidenced by the reference finance app using zero server-side windows. |
| ~~Default row filters~~ **SETTLED as pattern** (N2) | 1 | HIGH prevalence | Named-query composition is the scoping mechanism; no entity filter construct. |
| **OFFSET / jump-to-page** (N3) | 1 | **MEDIUM**: infinite scroll dominates modern UIs, but admin tables still ship page numbers. | Keep keyset-only; document the "no page 47" stance and its reasons. |
| ~~utility tranche~~ **SETTLED** (N7, M9, S2, S4, S6, S12, P7, N17) | 8 | n/a | `least greatest floor ceil mod dow trim hash` admitted; eight entries are ✅ (S6 flipped 2026-10-30). |
| **Write preconditions / counter upserts** (T2, T8) | 2 | MEDIUM | T2 is **settled**: a counter field plus a row-scoped `check`, no `require`. T8 is ✅ (2026-10-31): the one-statement create-or-increment on a unique key compiles. |
| **Retention trimming** (N12, N13) | 2 | MEDIUM | N13 stays by design (no chunked delete). N12 is a **gap** (2026-10-30): `policy retention` is specified but reserved and not built. Time-based trimming is a routine until then. |
| **Pairwise self-join (overlaps, dedup-similarity)** (Q4, P2, set-1 B11/H5) | 4 | **MEDIUM-LOW**: scheduling/booking domains hit it hard; most apps never do. | Keep unexpressible; overlap integrity belongs at write time (exclusion rules). Revisit only with a booking-domain user. |
| **Dynamic pivot / unpivot / grouping sets** (O1, O2, set-1 C7–C9) | 5 | **LOW-MEDIUM**: BI-tool territory; app queries rarely need server-side pivots. | Stay out; generated UI/report layer's job. |
| **Row-splitting / calendar spines** (Q6, set-1 F5) | 2 | **MEDIUM for reporting** (zero-filled series is a chart staple). | Provide the platform calendar-entity pattern in docs; no grammar change. |
| **Graph recursion beyond trees** (set-1 F6) | 1 | **LOW** for the target audience. | Hold. |
| **JSON/array in-row collections** (set-1 J1–J3, U-schema pressure) | 3 | **MEDIUM as migration friction**: greenfield apps model entities happily; apps *porting from* Postgres+JSONB will feel it. | Hold the schema-or-nothing line; write the migration cookbook. |
| **Scientific math / regex-replace / string surgery** (S5, S13, S10) | 3 | **LOW** in queries (belongs in routines/clients). | Hold. |
| **Ref-walking `check`** (T7a, set-1 J6a) | 2 | **MEDIUM-HIGH**: every multi-tenant app has a rule like "the assignee is in the same org". | **Gap.** The design walks outgoing refs only; the compiler refuses it (`check_beyond_row`) until the trigger substrate exists. Denormalize the column and `check` the two columns of one row. |
| **Multi-row insert / list parameters** (set-1 K2) | 1 | **MEDIUM**: batch imports and bulk adds. | **Gap.** `list` is not a type yet. |

**Where this leaves the action list (2026-10-30):**
- Done: `cond(...)`, the utility tranche, and the one-liner pins the corpus
  kept re-hitting (field-to-field comparisons, `date - date`, expression keys,
  the right-hand-side grammar, nulls last in sort, `distinct`).
- Decided against: a closed window construct (`extend`, kept as a v2
  candidate, `definition-language.md` §11).
- Open, in order of pain: ref-walking `check` (trigger substrate), retention
  policy, list parameters, a
  calendar-entity pattern in the docs.
