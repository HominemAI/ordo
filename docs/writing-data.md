# Writing data

Reads describe a shape you want back. Writes describe a change you want
made, and because every row in Ordo is guarded, a write is also a claim
about *who you are*. This page covers both halves: how you spell a change,
and what the platform does with it before anything lands.

Everything here executes today. Where something is deliberately absent, it
says so.

---

## 1 · There is no `insert`, `update`, or `delete`

There are two verbs, `set` and `del`, and **the mode is derived from the
statement, never spelled**:

```
op addTask($project: id, $title: text) =
  set tasks { project: $project, title: $title }        # create

op renameTask($task: id, $title: text) =
  set tasks(id == $task) { title: $title }              # addressed converge

op purgeDone($project: id) =
  del tasks(project == $project and status == "done")   # bulk delete
```

- **No condition** → a **create**. There is no row to address, so this
  makes one.
- **`id == …`** → **addressed**. Exactly one row, named. A delete may also
  spell this `del tasks { id: $task }`; the braces address, they do not
  assign.
- **Any other condition** → **bulk**. A range, however many rows that is.

A delete always states its target: **bare `del tasks` does not parse.**
Delete-everything is `del tasks(true)`. You say it on purpose, so a reviewer
can grep for it and a dropped condition cannot quietly become it.

Modes are derived because a declared mode can disagree with its own
statement. Here the shape of what you wrote *is* the mode, so there is
nothing to get out of sync.

A write op is **called, never composed**. It cannot be a source in another
pipeline, and another op cannot read from it. The compiler refuses both
by name. Reads compose; changes do not.

## 2 · What you may put on the right-hand side

Literals, parameters, `$caller`, and **expressions**: the same closed set
of operators and functions a read uses, computing the same way:

```
op claimTask($task: id) =
  set tasks(id == $task) { assignee: $caller }

op claimSeat($team: id) =
  set teams { id: $team, used: used + 1 }        # the row's own value

op remind($plant: id, $kind: text) =
  set plants { id: $plant, note: $kind + ": " + name }
```

`$caller` is the principal the door resolved, not a parameter, not
something the caller can supply. An op that assigns `assignee: $caller`
cannot be tricked into assigning someone else, because there is no
spelling for "some other caller".

### A field reference means the row's current value, atomically

`used + 1` reads the row **as it is before this write** and computes the
new value in that row's own `UPDATE`, under the lock the statement already
holds. There is no read-then-write for a second caller to slip between, so
two people claiming the last two seats at the same instant get one seat
each, never one seat twice. Pair it with a `check` and the cap is the
database's:

```
entity teams (key: team) {
  seats: int
  used:  int = 0
  check used <= seats
}
```

The claim past the cap comes back `check_violation`, naming the check.

**Two things a field reference cannot do**, both refused when you register
the op rather than on some later call:

- **Appear in a statement that can create.** A create has no current row to
  read, and neither does the create half of a converge-or-create. Address
  the row, or write the value out.
- **Reach another row.** `owner.name` is a read of somebody else; a write
  sees the row it writes and nothing more. Fetch it first and pass it as a
  parameter.

An expression with **no** field reference, `"reminder: " + $note`,
`round($total / $heads, 0)`, has an answer before the write runs, so it is
legal everywhere, creates included.

Fields you do not mention keep their values on a converge, and take their
**schema defaults** on a create. `id` and `rev` are the platform's; naming
either is `reserved_derived_name`.

## 3 · The guards run first, and the order is the point

Every statement passes two checks before anything is written, **in this
order**:

1. **Can you see it?** A row outside your read view answers `not_found`,
   *byte-identical* to the answer for a row that never existed. You learn
   a row exists only if you may reach it.
2. **May you change it?** A row you can see but not write answers
   `access_guard`. This is the informative case, and the safe one: you
   already knew the row was there, so naming the guard tells you nothing
   new.

Getting that order wrong turns every write into an existence oracle: try
to rename a row you cannot see, and a `access_guard` answer would confirm
it exists. So the invisible case must fail exactly like the absent case,
and conformance asserts the two error classes are identical.

**Bulk statements never refuse.** A bulk range is `read view ∩ your
condition ∩ write view`, and rows you cannot write are outside it.
The same silence as rows a guard hides from a read. `purgeDone` run by
someone with no write rights deletes nothing and reports `0`, because
"delete every done task I may delete" is answered by "none".

An addressed write is loud; a ranged write is quiet. That asymmetry is
deliberate: you named the one row, so you deserve to hear about it.

### Creates prove the guard against a row that does not exist yet

A create has no row to test, so the platform tests the **would-be** row:
values you supplied fold host-side, and guard steps that walk through a
parent you named pin that parent and continue in SQL. Every parent you
reference must also be **readable by you**. An invisible parent is
`not_found`, so you cannot discover ids by attaching children to them.

Some guards are unsatisfiable at create time by construction. A board's
guard walks `boardsAccess` (rows that reference the board), and none can
exist before the board does. Such a guard is **create-exempt**, and the
platform answers for it with the **bootstrap**: the creator gets a
top-level access row *in the same command group as the row it governs*.
Neither exists without the other.

## 4 · Every accepted write is recorded

One op invocation is one **command group**: one correlation id, one actor,
one transaction, one revert unit. Each row touched contributes a command
carrying its images:

| kind | before | after |
|------|--------|-------|
| create | none | full row |
| converge | full row | full row |
| delete | full row | none |

Groups are hash-chained per space, so removing or editing history is
detectable. Sealed values never appear in
an image; an opaque handle stands in their place.

`rev` starts at 1 and increments on every converge. It is the row's
version, and the basis for compare-and-set when a caller wants one.

## 5 · Ids are minted, and they say what they are

A new row's id is `<key>_<unique>` where `<key>` comes from the entity:

```
entity projects { … }                 # key: proj  → proj_01J8…
entity taskNotes (key: note) { … }    # explicit   → note_01J8…
```

Keys derive from the entity name (words abbreviated or truncated to four
characters, hyphen-joined), and a collision between two entities is a
**compile-time error naming both**, never a silent disambiguation. Full
rules, including the overridable abbreviation table, are in
[typed-id-keys.md](typed-id-keys.md).

The point is that an id is self-describing forever. A log line, a support
ticket, or a foreign key that reads `note_01J8…` cannot be mistaken for a
task, and no lookup is needed to know what it is.

## 6 · Atomicity, stated exactly

The whole protocol (precondition checks, row locks, mutations) runs
inside **one transaction**:

- Addressed and bulk statements take `SELECT … FOR UPDATE` on their rows
  **before** the guards are evaluated, so nothing changes between "the
  guard held" and "the update ran".
- Any refusal rolls the entire group back. A create whose bootstrap fails
  leaves no orphan behind, and a refused write appends no command group.

Everything in this document executes.

## 6a · Trees maintain themselves

A tree entity's position (its materialized path and its `depth`) is
derived from the parent chain and written by the platform. There is no
spelling by which a caller claims a position it did not earn through a
real parent.

- **Create** derives both from the parent (a root starts at depth 0).
- **Reparenting** (`set categories(id == $c) { parent: $p }`) moves the
  node *and everything under it*: a descendant's path contains its
  ancestors' ids, so the whole subtree is rewritten in one prefix
  substitution.
- **Cycles are refused.** A node cannot move under itself or under any of
  its own descendants. The test is a prefix comparison against the node's
  own path, which is exactly what "inside my subtree" means.
- **Depth is capped**, and the cap applies to the deepest leaf a move
  would push down, including the node you named.
- **Deleting a node with children is refused, with the count.** Subtree
  deletion is a separate, greppable spelling; a plain `del` never slides
  into one.
- **The cascade is `del <entity> { id: $id } subtree`**, and it is all or
  nothing. Guards are evaluated over every row in the subtree, including
  the anchor: one descendant you may not delete refuses the whole
  statement (`access_guard`, stating how many rows blocked it and not
  which), because deleting the reachable part would leave orphans whose
  parent is gone and whom you cannot reach to repair. Rows *outside* the
  subtree that still point into it block it exactly as they block a single
  delete (`has_referrers`). What lands is **one command per deleted row**,
  deepest first, each with its own before image, which is what makes a
  cascade revertible. Each tree row also takes the rows it owns (§6b).
- **`subtree` is spellable on a tree entity only.** On an entity with no
  `tree` ref, `ordo check` and registration refuse it with the compile error
  `subtree_not_tree`, so an op that could never run does not deploy. (Until
  2026-09-29 it deployed and answered `internal` to the first caller.)
- **A ranged reparent is refused.** Moving many nodes at once cannot keep
  overlapping subtrees consistent, so nodes move one at a time.

## 6b · A delete takes what the row owns, and refuses what still points at it

**Owned rows go with their owner** (owner ruling 2026-09-29,
`owned-delete`). A plain `del` of a row deletes every row that references it
through an `owned` reference, and every row that owns, in turn. That includes
the access rows Ordo derives for an entity with an `acl`: deleting a board
deletes its cards and its access rows, the deleter's own among them.

- **One group, one command per row.** The owned rows are deleted in the same
  command group as the row, each with its own before image, owned rows before
  their owner. Reverting the delete restores all of them, the owner first.
- **All or nothing.** You must be allowed to delete every row the cascade
  takes. One row you may not delete refuses the whole statement with
  `access_guard`, stating how many rows and not which. An `acl` entity's
  access rows need its top level, so only a holder of that level can delete
  the row.
- **Sealed fields** on a cascaded row are imaged as a direct delete images
  them, bound to their own row. Blobs a cascaded row referenced are released
  by the sweep, as they are for a direct delete.
- **A ranged delete** (`del <entity>(…)`) takes the owned rows of every row
  it matches.

**Plain references still restrict deletes.** Deleting a row another row
references through an association (no `owned`) answers `has_referrers`,
naming a referrer and the field that points at it, in the platform's
vocabulary, before the statement runs. The database's own foreign-key error
never reaches you: a constraint name is physical schema, and the error
inventory does not promise it.

This holds for the whole cascade. If a row the cascade would take is
referenced by an association from a row that is not itself going, the delete
answers `has_referrers` and nothing is deleted. Cascading does not override
an association.

A tree node with children still refuses a plain `del` (`tree_has_children`);
`subtree` is the form for trees (§6a).

## 7 · Error classes you can actually get

| class | status | when |
|-------|--------|------|
| `not_found` | 404 | the row, or a parent you named, is outside your read view, or absent |
| `access_guard` | 403 | you can see the addressed row but may not change it, or a delete would take an owned row you may not delete |
| `unbound_required_param` | 400 | a parameter has no value and no default |
| `projection_unknown_field` | 400 | you assigned a field the entity does not have |
| `reserved_derived_name` | 400 | you tried to write `id` or `rev` |
| `invalid_binding_expr` | 400 | a right-hand side the statement cannot mean. It reads the row in a statement that can create, or reads it into a field the platform must hold the value of, or dots to another row |
| `ambiguous_identity` | 400 | the braces cover two unique keys, which can name two different rows |
| `tree_cycle` | 409 | a move that would put a node inside its own subtree |
| `tree_has_children` | 409 | deleting a tree node that still has children |
| `tree_depth_exceeded` | 409 | a create or move past the depth cap |
| `tree_bulk_reparent` | 400 | a ranged move; tree nodes move one at a time |
| `has_referrers` | 409 | deleting a row that another row still references through a plain (not `owned`) reference |

Plus the identity refusals `duplicate`, `conflicting_identity`, `cas_miss`,
`expected_absent` (409) and `null_key_address` (400), which §6 rules 8 and 9
describe.

The inventory is closed. A write cannot fail in a way that does not have a
name here.

**Read the status as well as the code.** The split is one question: *did the
stored data refuse you, or did your statement?* Data refused a `tree_cycle`.
The tree happens to be shaped that way, and moving the child out first makes
the identical call land. That is **409**, and it is worth retrying after you
change something. Your statement refused a `tree_bulk_reparent`. A ranged
move is refused for every dataset there is, so retrying is pointless. That is
**400**. The two tree classes sit one row apart in the table and answer
different families for exactly that reason.

Nothing in this table is ever a **500**. A 500 from a write means the platform
has a bug, not that you asked for something it refuses. If you see one for a
rule described in this document, that is the bug.

## 8 · Two things the compiler refuses, so no caller ever meets them

Part of why the runtime inventory stays short is that some mistakes are
facts about the *statement*, not about the data, and a fact about the
statement is knowable at registration. Two of them are refused there,
which means a deploy fails, not a request.

**A statement that can create must supply every required field**
(`insert_incomplete`). A create fills unmentioned fields from their
schema defaults and leaves optional ones null; a field that is neither
has nothing to fall back on.

```
op orphanTask($s: text) =
  set tasks { status: $s }
# insert_incomplete: `orphanTask` can create `tasks` but supplies no
# value for `project`, `title`
```

Without the check this registers, deploys, and answers a Postgres NOT NULL
violation to whichever caller reaches it first, with the database answering a
question the compiler could already answer. Note what does *not* trip it:
a defaulted field, an optional field, and the platform's `id`/`rev`/`depth`
are all covered without being written. Converge and bulk statements are
not creation-capable at all, so they may describe one field and no more.

**A ranged write may not assign `id` or a unique field**
(`bulk_identity_field`). Addressed and bulk cannot blur:

```
op renameAll($n: text) =
  set categories(parent is null) { name: $n }
# bulk_identity_field: `renameAll` is a ranged write on `categories` and
# assigns the unique field `name`
```

`categories.name` is unique, so this asks every matched row to take the
same value for a key no two rows may share: a no-op when one row matches,
a guaranteed `duplicate` when two do, and which one depends on data the
author is not looking at. The addressed form, `set categories(id == $c)
{ name: $n }`, is one row and one value, and stays legal.
