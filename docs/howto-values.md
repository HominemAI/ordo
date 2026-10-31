# Six places Ordo deliberately differs from SQL

Ordo has chosen rules that diverge from SQL and from plausible guesses. Each difference buys something: determinism, keyset pagination correctness, or audibility. These are the rules that catch people who know SQL and who haven't read this page yet.

---

## Two-valued null logic

In Ordo, `null == null` is **true**. Every comparison either is or isn't; there is no "unknown" state. This is the opposite of SQL's three-valued logic and the source of a class of bugs that SQL silently creates.

**The truth table, in full:**

| Expression     | Result |
|---|---|
| `null == null` | true   |
| `null == 5`    | false  |
| `null != null` | false  |
| `null != 5`    | true   |
| `null < 5` (any ordering op) | false |

The identity `x != y` ≡ `not (x == y)` holds everywhere with exactly one meaning. Ordering comparisons always yield false for a null operand; they never answer "unknown".

**An example from the null-logic conformance suite:**

```
categories(parent != "cat_1")[id]
```

This query returns every category whose parent is not `"cat_1"`, including categories whose `parent` is null. In SQL with three-valued logic, `null != "cat_1"` is unknown, and those null rows vanish, which is silent data loss. In Ordo, it is true, and the rows stay. The two spellings of "not equal" give the same result:

```
categories(parent != "cat_1")[id]        # direct
categories(not (parent == "cat_1"))[id]  # negated
```

**Why:** SQL's three-valued logic was designed for aggregate queries where null means "not present in the group". That is a sensible local rule that becomes a trap everywhere else. Ordo is declarative: every query lives in the command log and every result is auditable. So Ordo gives null exactly one meaning everywhere: a value that is equal to itself and not equal to anything else. The language's one equality is reflexive. Keyset pagination depends on this: `OFFSET` is gone and cursors seek by "after this row in the language's order". That order must partition every row into "equal" or "not equal"; three-valued logic allows rows to fall out of both.

**Implementation note:** every comparison returns exactly what `IS NOT DISTINCT FROM` (for `==`) and `IS DISTINCT FROM` (for `!=` and their negations) return. The planner writes a plain `=` only where that cannot change the answer: in a filter, with one side proven never null, such as a required field or `$caller`. That is what lets a guard use an index. A plain SQL `=` or `<>` that a nullable value can reach and change the answer is a bug.

---

## Text sorts by codepoint

`COLLATE "C"`, always. `Z` sorts before `a`. The same op returns rows in the same order on every deployment, forever. This is determinism over legibility, a deliberate trade.

**The sorting order:**

```
Zoë
Åsa
ana
```

Read wrong to an author used to phone-book order. And that's fine, because the alternative is a sort order that differs between deployments. Ordo rejects locale-aware sorting per sort key (`sort name collate "en_US"`), because such a table ships with somebody else's software on a release cadence the platform does not control. ICU changes weights with CLDR; glibc has changed even `en_US.UTF-8` across releases. A language whose answers must be the same on 2026-09-04, 2027-09-04, and in five deployments must not import an external versioned table inside its semantics.

**Why:** keyset pagination. A cursor means "strictly after this row **in the language's order**". If the order changes under a library upgrade, the next page silently skips or repeats rows. An order definable by nothing but the value alone is the only order the language can own.

**If legible sorting really is required:** store a phone-book sort key (a `sortName` field with `normalize(lower)` applied) and sort by that. It is deterministic because it is a value; it is reviewable because it is in the schema; an author makes the locale decision once, at write time, instead of once per caller. This is data, not a collation table; no permission prompt needed, no library dependency, no surprise.

**Reference:** `corpus-rulings.md` G11, settled 2026-08-30.

---

## Text is NFC on write

A text field holding the string "é" (U+00E9 composed, `é`) and the string "é" (U+0065 + combining acute) is storing one value, not two. Both spellings normalize to the same bytes on write, in this order, without exception.

```
composed = "\u{00e9}"        # é
decomposed = "e\u{0301}"     # e + combining acute

# Both normalize to:
"\u{00e9}"     # é (NFC form)
```

**Why:** NFC is the second half of the codepoint-order rule. Codepoint order makes the byte sequence deterministic **given the bytes**; NFC-on-write makes the bytes deterministic **given the text**. Without it, two people type the same name, Postgres stores two byte sequences, and `COLLATE "C"` faithfully reproduces their arbitrary difference.

`UNIQUE` columns and keyset pagination both depend on this. Two rows storing different byte sequences that mean the same text are silent duplicates. `UNIQUE` would accept both (you didn't store exactly the same bytes). A cursor seeking "after this row" by byte sequence would find the composed form and miss the decomposed. The language fixes text on write so that it fixes this.

**NFC, not NFKC.** NFC composes characters that are canonically equivalent (`é` and `e` + acute combining into `é`) and changes nothing else. NFKC would also fold compatibility characters (`ﬁ` to `fi`, full-width to ASCII), turning different text into the same text. Ordo normalizes to equivalence, not to compatibility.

**Reference:** `crates/ordo-syntax/src/text.rs`, checklist 9u. The stored text assertion is in `definition-language.md` §2 under "Text canonicalization [settled]".

---

## Money declares its places

Each money field says how many decimal places its currency has: `price: money(2)` for dollars, `yen: money(0)` for yen. The column stores whole minor units (cents), and you write major units (dollars). The compiler converts (ruled 2026-10-04, checklist 27ak).

```ordo
entity items {
  price: money(2)
  fee:   money(2) = 1.50          # stored as 150
}

op cheap($cap: money(2)) =
  items(price < $cap)[price]      # call it as cheap($cap: 29.99)

op addItem($price: money(2)) =
  set items { price: $price }     # 29.99 is stored as 2999
```

- A literal meeting a money becomes minor units: `price > 29.99` compares with 2999 cents.
- An `int` beside a money is in dollars too: `price > 30` means $30.00.
- A literal with more places than the field holds is refused in a parameter, a write or a default, class `implicit_rounding`: `29.999` does not fit `money(2)`. In a filter it is compared exactly instead, because a comparison stores nothing.
- Money with different places does not mix: `money(2)` beside `money(0)` is `money_places_mismatch`.
- Bare `money` is `money(0)`: whole units. Programs written before places existed keep their meaning, and `29.99` against one is refused.

**Changing a field's places.** `ordo migrate --apply` refuses a change of places until you state `--places <entity>.<field>=rescale|keep` (25aa).

- `rescale` multiplies or divides every stored amount by 10^n. 29.99 stays 29.99.
- `keep` leaves the stored numbers as written. 2999 at `money(2)` then reads 0.2999 at `money(4)`.

**A revert past a rescale** (ruled 2026-10-06, 25ad). An old image holds the old minor units, so the revert converts them.

- Money only. A `decimal` is stored as its value, so there is nothing to convert.
- Each restored amount is converted by the net factor of the rescales since that group. 2999 at `money(2)`, rescaled to `money(4)`, comes back as 299900.
- A chain adds up: `money(2)` to 3 to 5 is 10^3. A group written between two rescales is converted by the later one only.
- A `keep` converts nothing. It said the number was meant as written.
- A deleted row restored by a revert is converted the same way. `ordo revert --dry-run` shows the converted amounts.
- A conversion that would lose digits is refused, class `places_revert_loses_digits`, naming `entity.field` and the row. Example: 299995 at `money(4)` reverted to `money(3)` is 29999.5.
- Rounding is off, and there is no setting for it yet. The engine can round half to even (29999.5 becomes 30000), but only Rust code can turn it on.
- Why half-even: half-up pushes every tie the same way, so the totals of many converted amounts drift upward. Half-even has no such bias.
- Two more refusals: `places_revert_overflows` (the amount does not fit a bigint) and `places_revert_unplaced` (a `$places` row has no chain position).
- These are revert refusals printed by `ordo revert`, not wire error codes.
- There is no setting yet. The engine type `ordo_exec::undo::RevertPolicy` holds it (per field over program-wide over the default `Refuse`), and only Rust code reaches it.
- Neither the program nor a variable nor an `ordo revert` flag spells it. `ordo revert` uses the default, so it refuses.
- Until it has one, write the amount you want with an ordinary command.

A money times a count is a money. A money times a rate, or divided by anything, is a `decimal` in major units, and it lands back in a money field only through an explicit rounding.

```ordo
op addTax($i: id) =
  set items { id: $i, fee: price * 0.08 }            # ✗ compile error: implicit_rounding
  set items { id: $i, fee: round(price * 0.08, 2) }  # ✓ rounded to the cent

op setPerHead($total: money(2), $heads: int) =
  set budgets { perHead: $total / $heads }           # ✗ compile error: implicit_rounding
  set budgets { perHead: round($total / $heads, 2) } # ✓ stated explicitly
```

`round(x, n)` lands in a `money(p)` field when `n` is at most `p`. `floor` and `ceil` work in major units: `floor(price)` is whole dollars.

**Rates and unit prices are `decimal(p)`.** A `decimal(4)` field stores an exact decimal with four places, as a Postgres `numeric`. It compares exactly and never becomes money on its own: `fee: round(price * rate, 2)` is the conversion, and `fee: rate` is `implicit_rounding`. A value with more places than the field holds is refused, not rounded.

```ordo
entity items {
  price: money(2)
  rate:  decimal(4) = 0.0825
}
```

Division holds a fixed scale of 12 (twelve digits after the decimal point), and scale reduction at the division is half-up. This is identical everywhere, host and SQL, so both venues answer the same value to twelve decimals even when intermediate results have more.

**Why:** no silent data loss. A decimal produced by division is exact to twelve places. Storing it in a `money` field means truncating it. The alternative would be a compiler that silently drops digits at the boundary where cents are decided, which is a silent rounding that an auditor could never find. Ordo refuses and forces the rounding into a line of code. Where the jurisdiction is genuinely split on rounding mode (half-even vs. half-up), the application computes the mode it needs from the primitives and writes it once, where a reviewer can see it.

**When shares must sum to the total, use `allocate`, not division:**

```
allocate(100.00, (1, 1, 1))  # → 33.34 / 33.33 / 33.33, sums to 100.00
100.00 / 3 * 3               # → 99.99 (three rounding errors)
```

`allocate` is the blessed money-split primitive, and it is legal only inside an `each` emission over a list parameter. It distributes the total by largest-remainder, ties by weight order, and is deterministic across venues.

**Reference:** `definition-language.md` §2, "Numeric semantics [settled]" and "Splitting money is `allocate`, not `/`".

---

## `since` anchors to execution time

`orders(placed since 30d)[total]` on 2026-06-01 shows orders from 2026-05-02 onward. The same op on 2026-07-01 shows orders from 2026-06-01 onward. The window moves with the clock.

**The binding-time guarantee:** the window is folded to a date literal at the moment the op binds to a request. The SQL contains `DATE '2026-05-02'`, not `NOW()`. Two runs of the same op are identical given the same clock. This is what makes `since` different from a true nondeterminism. It is nondeterministic by design (the window legitimately changes day to day) and reproducible by construction (the value is bound at request time).

```
orders(placed since 30d)[total]
# On 2026-05-01: placed >= DATE '2026-04-01'
# On 2026-06-01: placed >= DATE '2026-05-02'
```

The window is a date (`since 30d`) or a datetime (`since 2h`), depending on the type of the field being filtered. Window clamping for months uses the same rule as all month arithmetic: `2026-01-31 + 1mo = 2026-02-28` (clamped to the last day of February).

**Why:** dashboards and routine triggers need a moving window. An export or audit trail needs the same data every time, so `since` is forbidden in an op marked `reproducible` (class `reproducible_clock`). Use explicit bounds or parameters there instead. The binding-time folding is what makes this safe: the plan is reproducible because the window is data, not a function call.

**Reference:** `crates/ordo-conformance/tests/clock.rs`, and `definition-language.md` §2, "`since` anchor precision [settled]".

---

## `hash(x)`: stable, not keyed, only for text and id

`hash(x)` is the SHA-256 digest of the value's UTF-8 bytes, taking the first eight bytes big-endian and clearing the top bit so the result fits in a signed 64-bit int unsigned (`[0, 2^63)`). This is frozen at first release and must be held stable across platform versions.

```
hash("text")         # SHA-256 of "text"'s UTF-8, masked to [0, 2^63)
hash(mem_12345)      # SHA-256 of the id's UTF-8 bytes
hash(price)          # ✗ compile error: domain is text and id only
```

The function is defined for `text` and `id` only, because a bit-stability promise is a promise about bytes, and those are the two types whose bytes the language fixes (UTF-8, NFC by the text rule above). Promising stability over the rendering of a `date` or a `money` would mean freezing a rendering that appears nowhere in the spec.

`null` hashes to `null`.

**Why:** the value gets baked into consequences the platform cannot see: cohorts, stripes, and bucket columns an author computed once and stored. Changing the algorithm reshuffles all of them silently (a 5% rollout stays 5% and becomes a different 5%, with no error). So the function is frozen, full stop, and any future need for a different hash is a different function name.

The algorithm is not ours. SHA-256 is an external frozen standard (FIPS 180-4) with published test vectors, in Postgres core since version 11, in every programming language. "Stable" means "we did not invent it and therefore cannot drift from it". Postgres's own `hashtext` is explicitly documented as not stable across server versions.

The truncation details matter: first eight bytes, big-endian, top bit cleared. An unmasked digest over a 64-bit signed int would make `mod(hash(id), 100)` land in `(-100, 100)`, half the width of every rollout stripe an author wrote. Masking the sign bit removes a trap the obvious use walks straight into, and the cost is one bit of an already-large hash.

**Reference:** `corpus-rulings.md` G12, settled 2026-08-30.

---

## Unverified claims

None. Every claim above comes from the files listed in the source inventory at the end of this document.

## Sources per section

1. **Two-valued null logic:** `docs/definition-language.md` §4 "Null semantics [settled]"; `crates/ordo-conformance/tests/null_logic.rs` (truth table and examples)
2. **Text sorts by codepoint:** `docs/corpus-rulings.md` G11 (2026-08-30)
3. **NFC normalization:** `crates/ordo-syntax/src/text.rs` (module doc comment); `docs/definition-language.md` §2 "Text canonicalization [settled]"
4. **Money declares its places:** `docs/definition-language.md` §2 "Numeric semantics [settled]", "Splitting money is `allocate`, not `/`"
5. **`since` anchors to execution time:** `crates/ordo-conformance/tests/clock.rs` (execution-time anchoring); `docs/definition-language.md` §2 "`since` anchor precision [settled]"
6. **`hash(x)`:** `docs/corpus-rulings.md` G12 (2026-08-30)
