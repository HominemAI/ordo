# The rules generated code drifts on

Each rule below is one `ordo check` refuses, or one that compiles and means
something other than what was meant. The examples add to the `studio`
program in `language.md`.

**`today` and `now` are not values.** Take the date as a param.

```ordo-refused
# refused: unknown_field
op stale = notes(posted <= today)[body]
```

**`and` with `or` needs parentheses.** There is no precedence.

```ordo-refused
# refused: parse_error
op mixed unbounded = notes(pinned and status == "open" or posted > 2026-01-01)[body]
```

**Null is two-valued.** `null == null` is true, `null != 5` is true, and
every ordering with null is false. Nulls sort last in both directions. A
row kept by a join has nulls on the missing side, so a filter like
`posted < $d` drops it: add `or posted is null` when it should stay.
`count` and `sum` of nothing are 0; `avg`, `min`, `max` of nothing are null.

**One many-branch per pipeline.** Two children of one parent in one
pipeline would multiply rows, so it is refused. Count each side in a `let`
and join the lets back. A `let` ending in `group` is a source, joined on
its group key; list its parents with `[]` so no column clashes.

```ordo-refused
# refused: fan_out
op both unbounded =
  projects[name]  notes[id]  tags[id]
  group projects.id { n: count }
```

```ordo
op busy unbounded =
  let noteCount = projects[]  notes[]  group projects.id { n: count }
  let tagCount  = projects[]  tags[]   group projects.id { t: count }
  projects[name]  noteCount[n]  tagCount[t]
  | coalesce(t, 0) * 2 < coalesce(n, 0)
  sort name
```

**A `let` ending in `reduce` is a scalar**, read by dot. Any other `let` is
a source only.

```ordo
op manyPinned =
  let total = notes reduce { n: count }
  notes(pinned)[body]
  | total.n > 3
```

**Compare by multiplying, not dividing.** `/` yields `decimal`, fails on
`/ 0`, and never lands in a `money` or `int` slot without `round`, `floor`
or `ceil` (`implicit_rounding`).

**A unique field turns a create into an upsert.** `set codes { code: ... }`
converges the row that already has that code. So `let` cannot bind it:

```ordo-refused
# refused: unbindable_write
op twoCodes($c: text) =
  let first = set codes { code: $c, label: $c }
  set codes { code: $c + "2", label: first.id }
```

**A create covers every required field**, refs included.

```ordo-refused
# refused: insert_incomplete
op orphanNote($p: id, $body: text) =
  set notes { project: $p, body: $body, posted: 2026-01-01 }
```

**A defaulted param in a converge is always written.** Left out, it writes
its default. An "edit" op with `$lead: id = null` clears the lead whenever
the page leaves it out. Split such fields into their own ops.

**`null` from the page means "use the default".** It never clears a field by
itself. Clearing is `{ field: null }` written in the op.

**Words that do not exist:** `if` and `CASE` (use `cond`), `uniq` (use
`distinct`), `enum(...)`, `JOIN`, `OFFSET`, `LIMIT` (use `take`), `grant`,
`today` and `now` in ops, `true` as a guard (use `open`), `_` in names.

```ordo-refused
# refused: parse_error
op sorted unbounded = notes[body] sort if(pinned, 0, 1)
```

```ordo
op sorted unbounded =
  notes[body, posted]
  sort cond(pinned -> 0, 1), -posted
```

**A listing read without `unbounded`** is refused when nothing bounds its
rows. Write `unbounded`, or `take n` with a literal or a `<=`-bounded param.
