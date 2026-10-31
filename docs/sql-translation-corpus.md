# SQL to Ordo translation corpus, set 1

This is 118 real SQL and PostgreSQL patterns, each translated to Ordo and given
a verdict. Ordo leaves a lot of SQL out on purpose. This list is how the owner
chose what: where a pattern had no clean translation, the question was whether
that is a hole to fill or a limit to keep. Set 2 (`sql-translation-corpus-2.md`)
adds 93 patterns skewed toward what app developers ship.

Counts as of 2026-10-30: **78 direct, 6 problematic, 34 not possible** (27 by
design, 7 gaps). `scripts/check-sql-corpus.py` runs the Ordo code of 86 entries
through `ordo check` and tries a probe for each not-possible verdict it can, so
a verdict that goes stale fails the script. The other 32 entries are prose
verdicts with no code to run.

The patterns come from the TPC-H query set, the window-function interview
canon, recursive-CTE patterns and classic exercise corpora. Every entry carries
a verdict:

- ✅ **direct**: translates cleanly (often shorter)
- ⚠️ **problematic**: expressible with caveats (restructuring, cost class,
  or semantics differ)
- ❌ **not possible**: no Ordo form (marked *by design* where deliberate,
  *gap* where it is a real hole or the feature is not built yet)

Reference schema used throughout (current syntax, one field per line):

```
domain corpus @ 0.1.0

entity customers {
  name: text
  email: text? sealed indexed unique normalize(lower)
  country: text
  joined: date
  balance: money(2)?
}
entity categories {
  name: text unique
  parent -> categories? tree
}
entity products {
  name: text
  price: money(2)
  category -> categories
  active: bool = true
}
entity orders {
  customer -> customers
  placed: date
  status: text = "open"
  total: money(2)
}
entity orderItems {
  order -> orders owned
  product -> products
  qty: int
  price: money(2)
}
entity payments {
  order -> orders
  paid: date
  amount: money(2)
}
entity reviews {
  product -> products
  customer -> customers
  stars: int
  body: text sealed
}
entity departments {
  name: text unique
}
entity employees {
  name: text
  dept -> departments
  manager -> employees? tree
  salary: money(2)
  hired: date
}
entity shipments {
  order -> orders
  shipped: date?
  carrier: text
}
entity tags {
  name: text unique
}
entity productTags {
  product -> products owned
  tag -> tags owned
  unique (product, tag)
}
```

`total`, `amount` and `salary` are not `sealed` here. A sealed field cannot
filter by range, sort, aggregate or compute on the database side (compile error
`sealed_unsupported`), so reports over money need plain fields.

Entries that name other entities (`lineitems`, `bookings`, `events` and so on)
use the extra entities listed in `scripts/check-sql-corpus.py`.

---

## A. Filtering & basics

**A1. Simple filter + sort + limit**
```sql
SELECT name, price FROM products WHERE price > 50 ORDER BY price DESC LIMIT 10;
```
```
products(price > 50.00)[name, price]
sort -price
take 10
```
✅ direct.

**A2. Multiple conditions with OR**
```sql
SELECT * FROM orders WHERE status = 'open' OR (status = 'held' AND placed < '2026-01-01');
```
```
orders(status == "open" or (status == "held" and placed < 2026-01-01))[*]
```
✅ direct (parens mandatory when mixing: by design).

**A3. IN list**
```sql
SELECT * FROM orders WHERE status IN ('open','held','review');
```
```
orders(status in ("open", "held", "review"))[*]
```
✅ direct.

**A4. NOT IN list**
```sql
SELECT * FROM orders WHERE status NOT IN ('done','void');
```
```
orders(not (status in ("done", "void")))[*]
```
✅ direct for a **literal list with no nulls**, and now ruled for the rest.

**The general immunity claim is restored, on a stated basis** (X-011 / Z-008
closed; corpus resynced per M4-006). The earlier text said the nullable and
empty cases were *unruled* and this corpus could not assert immunity the
language had not granted. `definition-language.md` §5 has since granted it:
`in` is **exactly sugar for a chain of `==`** `[settled]`, so it inherits the
two-valued truth table without exception. Three consequences the translation
must honour:

- **A literal list of non-null constants** compiles direct, as above, there
  is no null to poison it.
- **A list that may contain a null** compiles to an OR-chain of
  `IS NOT DISTINCT FROM` (or an equivalent null-safe semi-join), never to
  SQL's `IN`. This is what makes `not (x in …)` the strict negation of the
  chain rather than SQL's always-empty `NOT IN (… NULL …)`, which has no
  spelling here.
- **An empty membership list** compiles to `FALSE` for every row, and its
  negation to `TRUE`. This is reachable through a list parameter bound to an
  empty list; the literal spelling `x in ()` does not parse, which is
  consistent with the language's general refusal of empty parens.

Owed: nullable-member, empty-list and `not`-of-both goldens in this corpus.

**A5. BETWEEN**
```sql
SELECT * FROM orders WHERE placed BETWEEN '2026-01-01' AND '2026-06-30';
```
```
orders(placed >= 2026-01-01 and placed <= 2026-06-30)[*]
```
✅ direct (no `between` keyword: one spelling, two comparisons).

**A6. IS NULL / IS NOT NULL**
```sql
SELECT * FROM shipments WHERE shipped IS NULL;
```
```
shipments(shipped is null)[*]
```
✅ direct.

**A7. Computed column in projection**
```sql
SELECT name, price * 0.9 AS sale_price FROM products;
```
```
products[name, salePrice: price * 0.9]
```
✅ direct, and **exact**: `+ - *` are closed over `money` and round nothing.
Nothing rounds implicitly anywhere: `/` and `avg` yield `decimal` at a fixed
**fractional scale of 12** (not "38 significant digits", which was withdrawn as
unimplementable), and a `decimal` may not land in a `money` field slot on its
own, that is the compile error `implicit_rounding`. The fix is an explicit
`round(x, n)` / `floor` / `ceil`, and **`round` is half-up, away from zero:
globally and unconfigurably** (M4-002; the earlier "the mode is the author's to
state" is withdrawn, because the closed function list has no slot to state it
in). Postgres `round(numeric, int)` is half-away-from-zero natively, so the
generated SQL is the direct call and the host matches it; no emulation either
side. A projection alias like `salePrice` above is not a field slot, so no
rounding is required or applied. Scale reduction *at the division itself* is
half-up too, which is now the same rule rather than a coincidence: one
tie-breaking behaviour across the whole numeric surface.

Owed: goldens at `round(2.5, 0)`, `round(-2.5, 0)` and `round(0.125, 2)`
asserting both venues agree.

**A8. DISTINCT single column**
```sql
SELECT DISTINCT country FROM customers;
```
```
customers[country]
distinct
```
✅ direct (post-ruling): `distinct` settled as the dedup step.

---

## B. Joins

**B1. Inner join, two tables**
```sql
SELECT o.id, c.name FROM orders o JOIN customers c ON c.id = o.customer_id;
```
```
orders[id, customer.name]
```
✅ direct: dot-path; join invisible.

**B2. Three-table chain**
```sql
SELECT oi.qty, p.name, o.placed FROM order_items oi
JOIN orders o ON o.id = oi.order_id JOIN products p ON p.id = oi.product_id;
```
```
orderItems[qty, product.name, order.placed]
```
✅ direct.

**B3. Four-table star with filter**
```sql
SELECT o.placed, c.name, p.name, oi.qty FROM order_items oi
JOIN orders o ON ... JOIN customers c ON ... JOIN products p ON ...
WHERE o.status = 'done';
```
```
orderItems(order.status == "done")[qty, product.name, order.placed, order.customer.name]
```
✅ direct.

**B4. LEFT JOIN keeping parents without children**
```sql
SELECT c.name, o.id FROM customers c LEFT JOIN orders o ON o.customer_id = c.id;
```
```
customers[name]  orders[id]
```
✅ direct: multi-source is outer-preserving from the one side [settled].

**B5. LEFT JOIN … WHERE child IS NULL (anti-join)**
```sql
SELECT c.* FROM customers c LEFT JOIN orders o ON o.customer_id = c.id
WHERE o.id IS NULL;
```
```
customers[*]
match none orders
```
✅ direct: and clearer than the SQL idiom.

**B6. Semi-join (EXISTS)**
```sql
SELECT c.* FROM customers c WHERE EXISTS
 (SELECT 1 FROM orders o WHERE o.customer_id = c.id AND o.status = 'open');
```
```
customers[*]
match any orders(status == "open")
```
✅ direct.

**B7. RIGHT JOIN**
```sql
SELECT o.id, c.name FROM orders o RIGHT JOIN customers c ON c.id = o.customer_id;
```
```
customers[name]  orders[id]        # reorder: right joins are left joins read backwards
```
✅ direct via reordering.

**B8. FULL OUTER JOIN**
```sql
SELECT c.name, s.name FROM customers c FULL OUTER JOIN suppliers s ON c.email = s.email;
```
❌ **not possible (by design)**: no full outer, and no joining on arbitrary
equality between unrelated entities (ref-only joins). Full-outer reconciliation
reports need restructuring (two anti-joins + append) or don't belong in the
language.

**B9. CROSS JOIN**
```sql
SELECT c.name, p.name FROM customers c CROSS JOIN products p;
```
❌ **not possible (by design)**: no relationship, no join. Cartesian products
are unexpressible.

**B10. Self-join, two roles via distinct refs**
```sql
SELECT e.name, m.name AS manager FROM employees e JOIN employees m ON m.id = e.manager_id;
```
```
employees[name, manager.name]
```
✅ direct: role access through the declared ref.

**B11. Self-join, same entity twice as sources (peers in same dept)**
```sql
SELECT a.name, b.name FROM employees a JOIN employees b
ON a.dept_id = b.dept_id AND a.id < b.id;
```
❌ **not possible (by design)**: an entity may appear once as a
source; pairwise self-comparison (dedup pairs, similarity) has no form. Known,
deliberate v1 exclusion.

**B12. Join through a link table (M2M)**
```sql
SELECT p.name, t.name FROM products p
JOIN product_tags pt ON pt.product_id = p.id JOIN tags t ON t.id = pt.tag_id;
```
```
productTags[product.name, tag.name]
```
✅ direct: the link entity is the source.

**B13. M2M filtered on the far side**
```sql
SELECT DISTINCT p.* FROM products p JOIN product_tags pt ... JOIN tags t ...
WHERE t.name = 'organic';
```
```
products[*]
match any productTags(tag.name == "organic")
```
✅ direct.

**B14. Two independent child sets under one parent (the fan-out bug)**
```sql
SELECT c.name, COUNT(DISTINCT o.id), COUNT(DISTINCT r.id)
FROM customers c LEFT JOIN orders o ON ... LEFT JOIN reviews r ON ...
GROUP BY c.name;   -- classic double-count trap in naive form
```
```
let o = customers[id, name]  orders[]
  group customers.id, customers.name { nOrders: count }
let r = customers[id]  reviews[]
  group customers.id { nReviews: count }
customers[name]  o[nOrders]  r[nReviews]   # matched on customers.id provenance
```
✅ direct (post-ruling): the fan-out guard refuses the naive form (two
many-sides under one parent), and the **let-join ruling** completes the safe
form: each side is grouped in its own `let`, then joined back on the parent id.

---

## C. Aggregation

**C1. GROUP BY + COUNT/SUM/AVG**
```sql
SELECT c.country, COUNT(*), SUM(o.total), AVG(o.total)
FROM orders o JOIN customers c ON ... GROUP BY c.country;
```
```
orders[total]
group customer.country { n: count, revenue: sum total, avgOrder: avg total }
```
✅ direct. (`total` is not sealed in the reference schema. A sealed field cannot be
aggregated: compile error `sealed_unsupported`. Changed 2026-10-30; the entry
once said sealed money aggregates host-side.)

**C2. HAVING**
```sql
... GROUP BY c.country HAVING SUM(o.total) > 100000;
```
```
orders[total]
group customer.country { revenue: sum total }
| revenue > 100000.00
```
✅ direct: position replaces HAVING.

**C3. Conditional aggregation (FILTER / CASE-in-SUM)**
```sql
SELECT country, COUNT(*) FILTER (WHERE status='done') AS done,
       COUNT(*) FILTER (WHERE status='void') AS voided FROM orders ... GROUP BY country;
```
```
orders[status]
group customer.country { done: count(status == "done"), voided: count(status == "void") }
```
✅ direct.

**C4. COUNT(DISTINCT x)**
```sql
SELECT country, COUNT(DISTINCT o.customer_id) FROM orders o ... GROUP BY country;
```
```
orders[customer]
group customer.country { buyers: distinct customer }
```
✅ direct (post-ruling): `distinct` settled.

**C5. GROUP BY expression (month)**
```sql
SELECT date_trunc('month', placed), SUM(total) FROM orders GROUP BY 1;
```
```
orders[total]
group month(placed) { revenue: sum total }
```
✅ direct.

**C6. Multi-key GROUP BY**
```sql
SELECT country, status, COUNT(*) FROM orders o JOIN customers c ... GROUP BY 1, 2;
```
```
orders[status]
group customer.country, status { n: count }
```
✅ direct.

**C7. GROUPING SETS**
```sql
SELECT country, status, COUNT(*) FROM ... GROUP BY GROUPING SETS ((country), (status), ());
```
❌ **not possible (gap, low priority)**: no grouping-sets form. Workaround:
three ops (or three lets + append with null-filled columns, but append
forbids null-filling, so truly three ops).

**C8. ROLLUP (subtotals + grand total)**
```sql
SELECT country, status, SUM(total) FROM ... GROUP BY ROLLUP (country, status);
```
❌ **not possible (gap, low priority)**: same family as C7. Subtotal rows are
a presentation concern; arguably belongs in the generated UI, not the language.

**C9. CUBE**: ❌ **not possible (gap, low priority)**: same as C8.

**C10. string_agg (concatenate group members)**
```sql
SELECT p.id, string_agg(t.name, ', ') FROM products p JOIN ... GROUP BY p.id;
```
✅ direct (post-ruling): `group products.id { tagList: joined(tag.name, ", ") }`.

**C11. array_agg**: ❌ same as C10 (no array type at all, by design).

**C12. Median / percentile_cont**
```sql
SELECT percentile_cont(0.5) WITHIN GROUP (ORDER BY total) FROM orders;
```
✅ direct (post-ruling): `reduce { m: median total }` /
`percentile(total, 0.5)`; continuous interpolation, empty ⇒ null.

**C13. MIN/MAX with the row that owns it ("which order was the biggest?")**
```sql
SELECT * FROM orders WHERE total = (SELECT MAX(total) FROM orders);
```
```
orders[*]
sort -total
take 1
```
✅ direct: and sidesteps the ties ambiguity (id tiebreaker).

**C14. Double aggregation (TPC-H Q13: distribution of counts)**
```sql
SELECT c_count, COUNT(*) FROM
 (SELECT c.id, COUNT(o.id) AS c_count FROM customers c LEFT JOIN orders o ... GROUP BY c.id) t
GROUP BY c_count ORDER BY 2 DESC;
```
```
let perCustomer = customers[id]  orders[]
                  group customers.id { c: count }

perCustomer[]
group c { custs: count }
sort -custs
```
✅ direct: `let` output re-grouped. (Confirms lets are sourceable; see B14 for
the *matching* gap.)

---

## D. Window functions

**D1. ROW_NUMBER per partition, keep first (DISTINCT ON)**
```sql
SELECT DISTINCT ON (customer_id) * FROM orders ORDER BY customer_id, placed DESC;
```
```
orders[*]
sort -placed
take[customer] 1
```
✅ direct: `take[]` *is* DISTINCT ON, done right.

**D2. Top-N per group**
```sql
SELECT * FROM (SELECT *, ROW_NUMBER() OVER (PARTITION BY category_id ORDER BY price DESC) rn
FROM products) t WHERE rn <= 3;
```
```
products[*]
sort -price
take[category] 3
```
✅ direct.

**D3. RANK exposed as a column ("show each product's rank")**
```sql
SELECT name, RANK() OVER (ORDER BY price DESC) FROM products;
```
❌ **not possible (by design (window family, declined, DL §11))**: `take[]` selects by rank but
cannot *project* the rank number. "Position as a value" has no form. Common in
leaderboards. The designed form is `extend[keys] { rank }`, kept as the v2
candidate if third-party demand appears.

**D4. DENSE_RANK / ties semantics**: ❌ **not possible (by design)**, same family as D3 (and Ordo's forced
id-tiebreaking means SQL's tie ranks don't even have an equivalent concept:
document, don't emulate).

**D5. LAG, compare each row to the previous**
```sql
SELECT placed, total, total - LAG(total) OVER (ORDER BY placed) FROM orders;
```
❌ **not possible (by design (window family, declined, DL §11))**: no inter-row references.
Deltas, churn detection, month-over-month change: all unexpressible in a single
op. Workarounds: client-side over fetched rows, or a routine maintaining
derived state. This is the largest hole the corpus surfaces, and the owner
decided to leave it (a full finance app shipped with zero server-side windows).

**D6. LEAD**: ❌ **not possible (by design)**, same as D5.

**D7. Running total**
```sql
SELECT placed, SUM(total) OVER (ORDER BY placed) FROM orders;
```
❌ **not possible (by design, window family)**: cumulative aggregates need
window frames. Extremely common (balances over time!). Note: for *balance* cases
specifically, the domain answer is a maintained field or routine-derived
state, which is arguably more honest than recomputing a window every read.

**D8. Moving average (7-day)**
```sql
SELECT placed, AVG(total) OVER (ORDER BY placed ROWS BETWEEN 6 PRECEDING AND CURRENT ROW) ...
```
❌ **not possible (by design, window family)**: window frames. Dashboard staple; today it's
client-side or precomputed by a rule into a stats entity.

**D9. Percent of total per row**
```sql
SELECT country, s, s / SUM(s) OVER () FROM (... GROUP BY country) t;
```
```
let total = orders[] reduce { t: sum total }

orders[total]
group customer.country { s: sum total }
| true
```
✅ direct (post-ruling): let scalars in post-group projections:
`[country, s, share: s / total.t]` (reduce-ended lets are scalars anywhere
expressions are legal). `share` is a `decimal` at fractional scale 12, and
`/ 0`, an empty or all-zero denominator, is a **typed runtime error**,
uniform with `mod(a, 0)`. There is no null-on-divide-by-zero folklore to rely
on, so guard the denominator or accept the typed rejection. If the intent is to
*split* a total rather than to compute a ratio, the operation is
`allocate(total, weights)`, whose parts sum back to the total exactly; `/`
cannot promise that at any scale.

**D10. NTILE (quartiles)**: ❌ **not possible (by design, window family)**: rank-family.

**D11. FIRST_VALUE per partition alongside rows**
```sql
SELECT *, FIRST_VALUE(total) OVER (PARTITION BY customer_id ORDER BY placed) FROM orders;
```
❌ **not possible (by design, window family)**: window projection onto un-collapsed rows.
Partial substitute: `take[]` for the first row itself, but not as a column on
every row.

**D12. Gaps and islands (consecutive-day streaks)**
```sql
-- classic: date - ROW_NUMBER()*interval trick, then GROUP BY the difference
```
❌ **not possible (by design, window family)**: requires row-number arithmetic.
Sessionization/streaks are analytics-tier; the honest answer is a routine
materializing sessions as rows, then trivial queries over them.

**D13. Sessionization by 30-min inactivity**: ❌ **not possible (by design)**, same as D12.

**D14. Cumulative distinct users by day**: ❌ **not possible (by design)**, windows + distinct; same family.

**D15. Compare each row to its group's average ("above dept average")**
```sql
SELECT e.* FROM employees e WHERE salary > (SELECT AVG(salary) FROM employees e2
WHERE e2.dept_id = e.dept_id);
```
✅ direct (post-ruling):
```
let deptAvg = employees[salary] group dept { a: avg salary }
employees[name, salary]  deptAvg[a]
| salary > a
```

**D16. Windowed dedup keeping latest version per key**
```sql
SELECT DISTINCT ON (doc_id) * FROM revisions ORDER BY doc_id, created DESC;
```
```
revisions[*] sort -created take[doc] 1
```
✅ direct.

---

## E. Subqueries & CTEs

**E1. Scalar subquery in WHERE**
```sql
SELECT * FROM products WHERE price > (SELECT AVG(price) FROM products);
```
```
let baseline = products reduce { a: avg price }
products(price > baseline.a)[*]
```
✅ direct.

**E2. Scalar subquery in SELECT list**
```sql
SELECT name, (SELECT COUNT(*) FROM orders o WHERE o.customer_id = c.id) FROM customers c;
```
```
customers[name]  orders[]
group customers.id, customers.name { n: count }
```
✅ direct: correlated-count-per-row is just outer-preserving group.

**E3. IN (subquery)**
```sql
SELECT * FROM products WHERE id IN (SELECT product_id FROM order_items WHERE qty > 10);
```
```
products[*]
match any orderItems(qty > 10)
```
✅ direct.

**E4. NOT EXISTS (correlated)**: ✅ `match none` (see B5).

**E5. Derived table (subquery in FROM)**
```sql
SELECT t.country, t.revenue FROM (SELECT ... GROUP BY country) t WHERE t.revenue > 1000;
```
```
let t = orders[total] group customer.country { revenue: sum total }
t(revenue > 100000)[country, revenue]
```
✅ direct.

**E6. Chained CTEs (pipeline of transforms)**
```sql
WITH a AS (...), b AS (SELECT ... FROM a ...), c AS (SELECT ... FROM b ...) SELECT * FROM c;
```
```
let a = …
let b = a … 
let c = b …
c[*]
```
✅ direct: `let` chains are the CTE pipeline, top-down.

**E7. CTE referenced twice**
```sql
WITH s AS (SELECT ...) SELECT ... FROM s JOIN s s2 ON ...;
```
✅ direct (post-ruling): lets are referenceable by any number of later
pipelines; the same-entity-once rule applies to entities, not lets.

**E8. LATERAL join (top 2 orders per customer, correlated)**
```sql
SELECT c.name, o.* FROM customers c CROSS JOIN LATERAL
 (SELECT * FROM orders WHERE customer_id = c.id ORDER BY placed DESC LIMIT 2) o;
```
```
customers[name]  orders[placed, total]
sort -orders.placed
take[customers.id] 2
```
✅ direct: the main LATERAL use case is exactly `take[]`.

**E9. ALL / ANY comparisons**
```sql
SELECT * FROM products WHERE price >= ALL (SELECT price FROM products WHERE category_id = 3);
```
```
let cap = products(category == $c) reduce { m: max price }
products(price >= cap.m)[*]
```
✅ direct (ALL/ANY reduce to min/max).

**E10. Correlated subquery for latest-related-row value**
```sql
SELECT o.*, (SELECT paid FROM payments p WHERE p.order_id = o.id ORDER BY paid DESC LIMIT 1) ...
```
```
orders[*]  payments[paid]
sort -payments.paid
take[orders.id] 1
```
⚠️ problematic: works, but orders *without* payments survive only because of
outer-preservation, and their `paid` is null: fine, but note `take[]` on a
group whose rows are all-null child columns must still keep one row. Conformance
case (spec implies it; not stated).

**E11. WHERE col = (subquery returning one row) with error-on-many**
```sql
SELECT * FROM orders WHERE customer_id = (SELECT id FROM customers WHERE email = 'x@y.z');
```
```
let c = customers(email == "x@y.z") reduce { theId: min id }
orders(customer == c.theId)[*]
```
⚠️ problematic: SQL errors if the subquery returns 2 rows; Ordo's `reduce`
can't express "assert exactly one." `min id` silently picks one. A
`one`-asserting reduce doesn't exist. Minor gap; unique constraint on email
makes it moot here, but the general form differs semantically.

**E12. Subquery with EXCEPT-style exclusion by key**: ✅ `match none` covers
the key-based case (general EXCEPT: see G4).

---

## F. Recursive

**F1. All descendants of a manager (org chart)**
```sql
WITH RECURSIVE sub AS (SELECT * FROM employees WHERE id = $x
UNION ALL SELECT e.* FROM employees e JOIN sub s ON e.manager_id = s.id) SELECT * FROM sub;
```
```
employees(id under $x)[*]
```
✅ direct: `under` replaces the whole recursive CTE.

**F2. All ancestors (path to root)**
```sql
WITH RECURSIVE up AS (...) SELECT * FROM up;
```
```
employees(id above $x)[*]
```
✅ direct.

**F3. Depth/level annotation per node**
```sql
WITH RECURSIVE t AS (SELECT ..., 1 AS depth ... UNION ALL SELECT ..., depth+1 ...) ...
```
✅ (post-ruling): tree entities carry an implicit `depth` field
(`employees(id under $x)[name, depth]`).

**F4. Recursive sum up a tree (BOM cost rollup / category totals)**
```sql
-- roll product counts up the category tree
```
⚠️ **problematic**: expressible per-anchor (`products(category under $c)
reduce { n: count }`) but "every category with its rolled-up subtree total" in
one op needs per-row `under` correlation, which doesn't exist. Rule-tier
materialization is the honest answer.

**F5. generate_series (calendar spine: every day, even empty ones)**
```sql
SELECT d::date FROM generate_series('2026-01-01','2026-12-31','1 day') d;
```
❌ **not possible (by design)**: no table-valued functions; the language
only reads entities. Zero-filled time series (months with no orders) therefore
can't be produced server-side. Common report need. Options: client zero-fill
(trivial), or a platform-provided calendar entity. Document the recommended
pattern.

**F6. Graph reachability (non-tree: shortest path, friend-of-friend)**
❌ **not possible (by design)**: only `tree` self-refs get traversal; general
graph recursion is deliberately outside the language.

---

## G. Set operations

**G1. UNION ALL (stack compatible shapes)**: ✅ `append` (columns matched by
name, no coercion).

**G2. UNION (dedupe)**: ✅ `append` + `distinct` (settled).

**G3. INTERSECT (rows in both sets)**
```sql
SELECT customer_id FROM orders WHERE placed > $a
INTERSECT SELECT customer_id FROM reviews;
```
```
customers[id]
match any orders(placed > $a)
match any reviews
```
✅ direct *when the intersection is by entity key* (the common case). General
row-tuple INTERSECT: ❌ not possible (by design, acceptable).

**G4. EXCEPT**: same split: key-based ✅ via `match any` + `match none`
chained; general tuple EXCEPT ❌ by design.

**G5. Symmetric difference**: ⚠️ two key-based pipelines + `append`; verbose
but expressible.

---

## H. Date/time

**H1. Orders in the last 90 days**: ✅ `orders(placed since 90d)`.

**H2. Month bucketing**: ✅ `group month(placed) { … }` (C5).

**H3. Week bucketing with ISO weeks**
```sql
SELECT date_trunc('week', placed), COUNT(*) FROM orders GROUP BY 1;
```
✅ `group week(placed) { n: count }`: but the conformance suite must pin
`week()`'s definition (ISO Monday-start?) since SQL dialects disagree.

**H4. Age/tenure computation**
```sql
SELECT name, CURRENT_DATE - hired FROM employees;
```
```
employees[name, tenure: $today - hired]
```
✅ direct (post-pin): `date - date → duration` pinned.

**H5. Rows where two ranges overlap (double-booking)**
```sql
SELECT * FROM bookings a JOIN bookings b ON a.room = b.room
AND a.id < b.id AND a.start < b."end" AND b.start < a."end";
```
❌ **not possible (by design)**: self-join pairwise comparison (B11 family). Overlap
detection is a real scheduling need; rule-tier or constraint-tier answer
(exclusion constraints) rather than query-tier.

**H6. First order per customer per month**: ✅ `sort placed` +
`take[customer, month(placed)] 1`, **if** computed expressions are legal as
take-keys. Spec says keys are "fields/dot-paths"; `month(placed)` is neither.
✅ (post-pin): expression keys legal in take[]/group/sort.

**H7. Day-of-week analysis**: ✅ `dow()` (post-ruling, ISO 1–7).

**H8. Duration comparison in conditions**: ✅ (post-pin).

---

## I. Text & patterns

**I1. Substring search**: ✅ `products(name contains "usb")`.

**I2. Prefix search**: ✅ `starts`.

**I3. Case-insensitive equality**: ✅ via `normalize(lower)` on the shape, or
`lower(name) == lower($q)` in conditions.

**I4. Regex match**
```sql
SELECT * FROM customers WHERE name ~ '^[a-z]+ [a-z]+$';
```
✅ direct (post-ruling): `customers(name matches $re)[*]`, linear-time
engine (Rust regex semantics; no backrefs/lookaround, so pathological
backtracking is impossible); scan-cost. On a sealed field such as `email` the
same filter is refused, `sealed_unsupported` (2026-10-30: the entry used
`email` and said sealed fields match host-side).

**I5. Full-text search with ranking (tsvector)**
❌ **not possible (by design for v1)**: `matches` reserved; search-box UX is
client-side over synced data per earlier ruling.

**I6. Trigram similarity / fuzzy match**: ❌ **not possible (by design)**, same as I5.

---

## J. Postgres-specifics

**J1. JSONB field extraction/filtering**
```sql
SELECT data->>'plan' FROM accounts WHERE data @> '{"active": true}';
```
❌ **not possible (by design)**: no JSON type. Ordo's stance: schema or it
doesn't exist. Migration path for JSON-shaped legacy data: entities.

**J2. Array column membership**: ❌ by design (no arrays; link entities).

**J3. unnest()**: ❌ by design (nothing to unnest).

**J4. DISTINCT ON**: ✅ `take[key] 1` (D1).

**J5. SELECT … FOR UPDATE (row locking)**
❌ **not possible (by design)**: no interactive transactions in the language;
concurrency is CAS (`rev`) + atomic op groups. Different model, deliberately.

**J6. Advisory locks**: ❌ by design (platform internal only).

**J6a. CHECK constraint referencing another table**
```sql
-- not expressible as a plain CHECK in Postgres; the usual answers are a
-- trigger, or denormalizing the parent's column onto the child
ALTER TABLE tickets ADD CONSTRAINT assignee_in_org
  CHECK (assignee_org = customer_org);   -- after copying both columns down
```
```
entity tickets {
  customer -> customers
  assignee -> staff
  check assignee.org == customer.org
}
```
❌ **not possible today (gap)**: the compiler refuses this `check` with
`check_beyond_row`: a database CHECK sees one row, and a walk through refs
needs triggers on every entity the walk passes, which are not built
(`crates/ordo-schema/src/check.rs`, `docs/traps.md`). Changed 2026-10-30. This
entry once said `check` walked outgoing refs to any depth and was settled.
The design intent is unchanged (the walk would follow outgoing refs only, never
aggregate or count), but the form does not compile.

What works now: a `check` over the row's own columns, plus a copied column
kept in step by the write. Put `org` on the ticket and have the op set it from
the parent. "Lines must sum to the invoice total" and "at most 10 open tickets"
stay rule-tier: a maintained counter field and a row-scoped `check` (see set 2,
T2).

**J7. TABLESAMPLE / random sampling**
```sql
SELECT * FROM orders TABLESAMPLE BERNOULLI(1);
```
❌ **not possible (by design)**: also no `random()` (nondeterminism by
design). Sampling for ML/exports would be an op-envelope feature if ever
needed, not grammar.

**J8. RETURNING full row**
```sql
UPDATE orders SET status='done' WHERE id=$1 RETURNING *;
```
✅ (post-pin): addressed writes return the written row descriptor.

---

## K. DML

**K1. Plain INSERT**: ✅ `set orders { customer: $c, placed: $d, total: $t }` (CREATES). A creating
write must cover every required field, and `today` is not yet a lowered
default, so `placed` is passed in (2026-10-30).

**K2. Multi-row INSERT (VALUES list)**: ❌ **not possible today (gap)**: the
design is `set tags each $names { name: item.name }` over a list parameter, but
`list` is not a type yet (the closed type set has no list), so it does not
compile. Changed 2026-10-30. Several `set` statements in one op are atomic and
cover a fixed number of rows.

**K3. INSERT … SELECT (copy rows from a query)**
```sql
INSERT INTO archive_orders SELECT * FROM orders WHERE placed < '2020-01-01';
```
❌ **not possible (gap)**: writes take braces of values/params, not a query
source. Archival/backfill jobs want this. Arguably belongs to migrations and
ops-mode, not the app language, but decide and document.

**K4. UPDATE with WHERE**: ✅ bulk `set orders(status == "held") { status: "open" }`.

**K5. UPDATE … SET x = x + 1 (self-referential)**: ✅ RHS field refs:
`set products(id == $p) { … }` → wait, addressed form: `set products { id: $p,
views: views + 1 }` ✅ [settled].

**K6. UPDATE with values from another table (UPDATE … FROM)**
```sql
UPDATE order_items oi SET price = p.price FROM products p WHERE p.id = oi.product_id;
```
❌ **not possible (gap)**: RHS field references are same-row only; cross-row
"copy from the referenced row" (`price: product.price`) is unstated/illegal.
Common repricing/denormalization op. Candidate ruling: allow dot-path *reads*
on the RHS of bulk converges. Real decision needed, it widens the write
scope surface.

**K7. DELETE with join condition (DELETE USING)**
```sql
DELETE FROM order_items oi USING orders o WHERE o.id = oi.order_id AND o.status='void';
```
```
routine purgeVoidItems in corpus {
  trigger every day 02:00Z
  run:
    orderItems(order.status == "void")[id]
    each -> del orderItems { id: id }
}
```
⚠️ **problematic**: changed 2026-10-30. The one-statement delete with a
dot-path in its condition no longer compiles: a write condition is
row-scoped, so a path through a ref is refused (`invalid_binding_expr`). The read that finds the rows may use
the dot-path, so cleanup is a rule: a query feeding `each -> del`, attributed
and revertible.

**K8. ON CONFLICT DO UPDATE (upsert)**: ✅ converge-or-create:
`set customers { email: $e, name: $n }`.

**K9. ON CONFLICT DO NOTHING (insert-or-ignore)**
```sql
INSERT INTO tags(name) VALUES ($n) ON CONFLICT DO NOTHING;
```
```
set tags { name: $n }
```
✅ direct: converge-or-create where the described state *is* the key: if
present, converging the key fields to themselves is a no-op. (Worth one
conformance case to bless explicitly.)

**K10. MERGE (multi-action WHEN MATCHED/NOT MATCHED)**: ❌ **by design**:
the folklore generator Ordo exists to refuse. Converge-or-create covers the
sane subset.

**K11. Swap two rows' values in one statement**
```sql
UPDATE t SET v = CASE id WHEN 1 THEN (SELECT v FROM t WHERE id=2) ... END ...
```
⚠️ one atomic op with two addressed converges (values passed as params): ✅
in practice via the client reading both then CAS-writing both in one op;
the pure server-side swap without reading first is ❌ (K6 family). Fine.

**K12. TRUNCATE**: ✅ `del orders(true)` (stated, scope-gated).

---

## L. TPC-H selections (the analytics gauntlet)

**L1. Q1, pricing summary (multi-aggregate, computed expressions)**
```sql
SELECT returnflag, linestatus, SUM(qty), SUM(price*(1-disc)), AVG(qty), COUNT(*) ...
GROUP BY returnflag, linestatus;
```
```
lineitems(shipped <= $cutoff)[qty, price, disc]
group returnflag, linestatus {
  sumQty: sum qty, revenue: sum (price * (1 - disc)), avgQty: avg qty, n: count }
```
✅ direct: aggregate expression arguments settled (`sum (price * (1 - disc))`).

**L2. Q2, min-cost supplier per part (correlated min)**
```
let minCost = partSupp
  group part { m: min cost }
partSupp[part, cost]  minCost[m]
| cost == m
```
✅ direct (post-ruling): the `let` output joins back on the `part` ref.

**L3. Q4, orders with at least one late lineitem**: ✅ `match any
lineitems(commitDate < receiptDate)`, field-to-field comparisons now pinned. ✅ (post-pin).

**L4. Q5, revenue by nation through a 6-way chain**: ✅ chain joins via
dot-paths + group. (Field-to-field comparisons now pinned. Money here is not
sealed: a sealed field cannot be aggregated.)

**L5. Q13, customer order-count distribution**: ✅ (C14).

**L6. Q15, top supplier by revenue (view + max)**: ✅ named query + `sort
-revenue take 1` (ties: id tiebreaker picks one; SQL returns all tied, a
semantic difference worth documenting).

**L7. Q18, customers with any order over quantity threshold**: ✅
`match any` + group.

**L8. Q22, customers above avg balance with no orders (anti-join + scalar)**
```
let bar = customers(balance > 0) reduce { a: avg balance }
customers(balance > bar.a)[country, balance]
match none orders
group country { n: count, cap: sum balance }
```
✅ direct: three constructs compose cleanly.

---

## Verdict summary

Counted from the entries by `scripts/check-sql-corpus.py --counts`, 2026-10-30.

| Verdict | Count | Share |
|---|---|---|
| ✅ direct | 78 | 66% |
| ⚠️ problematic | 6 | 5% |
| ❌ not possible | 34 | 29% |
| Total | 118 | 100% |

Of the 34 not possible, 27 are by design and 7 are gaps.

Problematic (6): E10, E11, F4, G5, K7, K11.

**Changed on 2026-10-30, when the entries were re-run against the compiler:**
J6a went from direct to gap (`check` over refs is refused, `check_beyond_row`),
K2 from problematic to gap (no list parameters yet), K7 from direct to
problematic (write conditions are row-scoped). The reference schema lost
`sealed` on money fields (sealed fields cannot aggregate) and `= today`
defaults (not lowered yet), and was rewritten in current syntax.

**❌ by design (27):**
- Joins Ordo does not have: cross join (B9), full outer (B8), pairwise
  self-join and overlap detection (B11, H5).
- The window family, declined in `definition-language.md` §11: rank as a value,
  lag and lead, running totals, moving averages, ntile, first value on every
  row, gaps and islands, sessionization, cumulative distinct (D3 to D8, D10 to
  D14). The designed form, `extend`, is kept as a v2 candidate.
- No table-valued functions, so no calendar spine (F5). No general graph
  recursion, only `tree` (F6).
- No full-text ranking or fuzzy match in the grammar (I5, I6). Search runs
  client-side.
- No JSON, arrays or unnest (J1 to J3, C11): schema or it does not exist.
- No interactive transactions or locks (J5, J6): concurrency is CAS plus atomic
  op groups.
- No random sampling (J7): no nondeterminism in queries.
- No MERGE (K10): converge-or-create covers the sane subset.
- General tuple INTERSECT and EXCEPT: only the key-based form translates (G3,
  G4).

**❌ gaps (7), most painful first:**
1. Ref-walking `check` (J6a, and T7a in set 2). The design is in
   `definition-language.md` §3. The compiler refuses it until the trigger
   substrate exists. Counters plus a row-scoped `check` work today.
2. UPDATE from a related row (K6) and INSERT from a query (K3). Write-side
   decisions, not made.
3. Multi-row insert (K2). Needs list parameters, which are not a type yet.
4. GROUPING SETS, ROLLUP and CUBE (C7 to C9). Low priority; subtotals are a
   presentation job.

Settled since the first draft and still true: let-join provenance (B14, C14,
D15, L2), `joined` (C10), `median` and `percentile` (C12), `matches` (I4),
`distinct` (A8, C4), expression keys in `take[]`, `group` and `sort` (H6),
`dow` (H7), `depth` on trees (F3).

**Sources:** grounded in the [TPC-H specification query set](https://www.tpc.org/tpc_documents_current_versions/pdf/tpc-h_v2.17.1.pdf), [DataLemur's window-function interview set](https://datalemur.com/blog/sql-window-functions-interview-questions), [recursive CTE pattern surveys](https://www.stratascratch.com/blog/learn-to-use-a-recursive-cte-in-sql-query), and the classic PostgreSQL exercise corpora.
