# Typed-id keys

Every row's id is a **typed id**: a short entity prefix, an underscore, and
a time-ordered UUIDv7 (`prod_01J8ZKQ4X7…`). The prefix makes any id seen
anywhere (a log line, an audit trail, a debug session) self-describing.
Ids are server-generated; authors never choose them. This page documents
where the prefix comes from.

## Derivation

The key derives from the entity name unless you override it:

1. Split the name at camelCase boundaries: `orderItems` → `order`, `items`.
2. Abbreviate each word by the table below, or truncate it to its first
   four characters. Words of four letters or fewer pass through whole.
3. Join multiple words with a hyphen.

```
products      → prod
vendors       → vend
tags          → tags
orderItems    → orde-item
transactions  → txns        # table entry
appointments  → appt        # table entry
```

## Collisions fail the compile

Two entities may never share a key. The prefix goes into every id ever
logged, so it can never quietly mean two things. A collision is a compile
error naming **both** offenders and the fix:

```
[id_key_collision] `orders` and `orderings` both derive id key `orde`
— give one an explicit key: `entity orderings (key: …)`
```

## Explicit override

Declare the key alongside the name; the compiler enforces the same
uniqueness rule over explicit keys:

```
entity projects (key: proj) {
  name: text
  …
}
```

## Your own abbreviation table

Point `ORDO_KEY_ABBREVIATIONS` at a file of your own entries. One
`word=abbreviation` per line; `#` starts a comment; your entries win over
the built-ins. A malformed line is a **loud error**, never silently
dropped, because a dropped abbreviation would surface years later as a
surprising id prefix.

```
# house style
transactions = trx
widgets      = wdg
```

## Built-in table

Curated from the industry's typed-id convention (Stripe's prefix
registry) and the classic general-English shortening lists, filtered to
words plausible as entity names. An entry earns its slot only where
plain truncation is wrong. Two deliberate absences: `revisions` (`rev`
collides with the implicit revision field) and `standards` (`std` is the
library namespace).

| word | key |
|---|---|
| `accounts` | `acct` |
| `addresses` | `addr` |
| `administrators` | `admin` |
| `amounts` | `amt` |
| `apartments` | `apt` |
| `appointments` | `appt` |
| `balances` | `bal` |
| `buildings` | `bldg` |
| `charges` | `chg` |
| `configurations` | `cfg` |
| `corporations` | `corp` |
| `departments` | `dept` |
| `directors` | `dir` |
| `divisions` | `div` |
| `documents` | `docs` |
| `employees` | `emp` |
| `environments` | `envs` |
| `estimates` | `est` |
| `figures` | `fig` |
| `governments` | `govt` |
| `headquarters` | `hq` |
| `information` | `info` |
| `invoices` | `inv` |
| `laboratories` | `lab` |
| `languages` | `lang` |
| `licenses` | `lic` |
| `locations` | `loc` |
| `management` | `mgmt` |
| `manufacturers` | `mfr` |
| `messages` | `msgs` |
| `miscellaneous` | `misc` |
| `notifications` | `notif` |
| `numbers` | `num` |
| `organizations` | `orgs` |
| `packages` | `pkg` |
| `parameters` | `params` |
| `payments` | `pay` |
| `permissions` | `perms` |
| `positions` | `pos` |
| `publications` | `pub` |
| `quantities` | `qty` |
| `receipts` | `rcpt` |
| `records` | `rec` |
| `references` | `refs` |
| `registrations` | `reg` |
| `repositories` | `repos` |
| `requirements` | `reqs` |
| `schedules` | `sched` |
| `sequences` | `seq` |
| `signatures` | `sig` |
| `spaces` | `spc` |
| `statistics` | `stats` |
| `subscriptions` | `subs` |
| `temperatures` | `temp` |
| `transactions` | `txns` |
| `universities` | `univ` |
| `versions` | `ver` |
| `volumes` | `vol` |
