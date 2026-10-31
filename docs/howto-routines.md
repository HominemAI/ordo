# How to write a routine

For app developers. Built in checklist 12a, 2026-10-08. A routine is work the platform runs by itself, on a clock or after a change.

The design is `definition-language.md` sections 7 and 8. A full example file is `examples/syntax/20-routines.ordo`.

## What a routine is

- A routine is a named block with one `trigger` and one `run` body.
- The platform runs it in every tenant. Nobody calls it.
- It is not an op. It is not in the interface descriptor.
- Its writes land as one command group, attributed `routine:<name>@<version>`.
- The writes go through the write protocol: locks, images, chain append.

## The shape

```ordo
routine <name> [in <domain>] {
  trigger <one trigger>
  run[($now: datetime, $today: date)] [unguarded] [nopause]:
    <write statements>
}
```

The name is lowerCamel and cannot match an op. The optional `in <domain>` must name the domain of the file.

## The three triggers

Every N units of time:

```ordo
routine expireOld in shop {
  trigger every 15m
  run($now: datetime):
    set orders(status == "new" and placed < $now - 7d) { status: "expired" }
}
```

- Units are `s`, `m`, `h`, `d`, `w`.
- `mo`, `y` and `ms` are refused.
- A schedule fires once per period for the whole deployment. It enqueues one run per tenant.

Once a day, at a UTC time:

```ordo
routine morningSweep {
  trigger every day 09:00Z
  run($now: datetime):
    set orders(status == "new" and placed < $now - 7d) { status: "expired" }
}
```

- The `Z` is required. `09:00` without it is refused by name.
- UTC is never implied.

After a change:

```ordo
routine reviewNew {
  trigger when shop.orders.created debounce 5s coalesce
  run:
    set orders(reviewed == false) { reviewed: true }
}
```

- The form is `when <domain>.<entity>.<kind>`.
- Kinds: `created`, `converged`, `deleted`, `written`, `ingested`, `fed`, `reverted`.
- `written` means any of created, converged and deleted.
- An unknown kind is `unknown_event_kind`.
- `debounce 60s` waits for a quiet minute.
- `coalesce` keeps at most one run waiting.
- The change is read from each tenant's own chain.
- A routine first deployed into a tenant with history starts at the head of that chain. It reacts to changes from then on, not to the past.
- A tenant the server could not reach at boot starts at the head too, when it is first reached.
- Only a tenant created after the version was activated starts at the beginning of its chain. All of its chain is new.

## What the body may do, and as whom

- The body is write statements, exactly as in a write op: `set`, `del`, `let x = set`.
- Or a read pipeline followed by `each ->` and write statements. See "Reading rows with each".
- It runs as the routine itself, not as a person.
- `$caller` is refused. A routine has no caller.
- Every entity and field the body writes must exist.

## Reading rows with each

A body may read rows first and write once per row.

```ordo
routine expire in shop {
  trigger every day 08:00Z
  run($today: date):
    leases(status == "active" and end < $today)[id]
    each ->
      set leases { id: id, status: "expired" }
}
```

- The shape is `run[($now, $today)] [unguarded] [nopause]: <pipeline> each -> <write statements>`.
- A plain write body, with no pipeline, works as before.
- In the emission braces `{ ... }`, and in the `{ id: ... }` of an addressed `del`, the names in scope are the fields of the row the pipeline produced.
- The run's own `$now` and `$today` are in scope too.
- A dotted name the pipeline wrote reaches the same field: `unit.name` for a projected `[unit.name]`, `plants.id` for the `id` of source `plants`.
- After a `group`, the row is the group keys and the aggregate names.
- A statement's condition, `set e(...)` or `del e(...)`, keeps the scope of the entity it writes, as every condition does.
- A name the row does not carry is `invalid_routine`: "is not a field of the row the pipeline produces".
- A `blob` or `json` row field cannot be carried into a write. That is `invalid_routine`.
- The emission passes the same write rules as a write op: `insert_incomplete`, `bulk_identity_field` and the rest.
- One run is one command group, attributed `routine:<name>@<version>`, whatever the number of rows. Its revert undoes the whole run.

The row limit:

- A run handles at most `ORDO_ROUTINE_EACH_MAX_ROWS` rows. The default is 500.
- It is a deployment setting. Unset, zero or not a number means the default.
- When more rows matched, the log says "handled N rows ... and more matched".
- Nothing is carried to the next run.
- A body whose writes take a row out of its selection, like `expire` above, finishes the rest over the next runs.
- A body whose writes leave rows matching meets the same first rows again each run. An example is converge-or-create of an alert by key.

The bound for change routines:

- A clock-woken run passes guards.
- A change-woken run reads only rows reachable from the changed rows: the changed rows and rows one declared ref away. `unguarded` on the `run` header lifts that.
- An emission may converge a row by `id` without `unguarded`: `set projects { id: project, lastActivity: $today }`.
- That row is written when the change reaches it and left alone when it does not.
- A `set` with no condition and no `id` is a create, or a converge-or-create by unique key. In a change routine it is `routine_out_of_bound` unless `run` says `unguarded`.

`ordo check` lists the routine in its ROUTINES section, for example `thirstCheck (every day 09:00Z, each row of plants waterings, at most 500 a run)`.

## The bound for change routines

- A scheduled routine runs with guards passing.
- A routine woken by a change may touch only the changed rows and rows one declared ref away. The ref may point either way.
- The platform narrows every statement's condition to those ids.
- A `set` with no condition creates a row. A change routine cannot do that.
- `unguarded` on the `run` header lifts the bound and allows the create.

```ordo
routine rebuildAll {
  trigger when shop.orders.written
  run unguarded:
    set lines(stale == true) { stale: false }
}
```

- The refusal is `routine_out_of_bound`.

## Parameters

- Only `$now: datetime` and `$today: date` exist.
- The platform fills both from the run's own instant.
- Declare only the ones the body uses.
- Any other parameter is `invalid_routine`.

## Telling people by email: `notify`

Built 2026-10-22, for the notes app's reminders. A routine can email the people on the rows it writes.

```ordo
routine fireReminders in notes {
  trigger every 30s
  notify admin "A reminder on one of your notes is due"
  run($now: datetime):
    reminders(due <= $now and fired == null)[id]
    each ->
      set reminders { id: id, fired: $now }
}
```

- The line is `notify <level> "<text>"`, between the trigger and `run`.
- After a run commits, each row it created or changed is followed to the row its `acl` lives on.
- That is the row itself, or the owner it inherits its acl from (`reminders.note -> notes owned`).
- Each active member who holds exactly `<level>` there gets one email. Several rows for one person in one run are one email.
- The subject is the text. The body is the text and the line "Open the app to see it."
- Nothing from a row is in the mail. The text is a literal, so the detail stays in the app.
- A person is mailed at the address they sign in with by emailed code. A person with no email sign-in, or a deleted account, is mailed nothing.
- It needs the mail channel, `ORDO_MAIL_KIND` (`howto-sign-in.md` §9). Without it the log says how many people were not told, and names the setting.

Once, and only once:

- A schedule fires once for the deployment, however many servers run.
- The notices are queued after the commit. A run that rolled back tells nobody.
- Write a body that marks its rows, as `fired` above, so the next run no longer matches them.
- A crash between the commit and the queue loses that run's mail. It is never sent twice.

`invalid_routine` refuses:

- a text that is empty, over 200 bytes, or more than one line;
- a level the acl does not declare;
- a `set` on an entity with no acl, of its own or inherited;
- a body that only deletes.

`ordo check` lists it: `fireReminders (every 30s, ..., emails the holders of `admin`)`.

Not built: other channels (push, SMS), a template, a link in the mail, and the CDK spelling.

## Paused tenants

- A paused tenant's runs are skipped.
- A skipped run is finished. It is not retried.
- Each skip is recorded in the control database's `$routineskips` table, with the reason `paused`, and in the log.
- `nopause` on the `run` header runs the routine anyway.
- The default is not to run.

```ordo
routine heartbeat {
  trigger every 1h
  run nopause:
    set lines(stale == true) { stale: false }
}
```

See `howto-pause.md` for what a pause is.

## Failures and retries

- Runs are at-least-once. Write bodies that give the same result when run twice.
- A failing run retries after 1s, 2s, 4s, 8s and 16s.
- After 5 attempts it is dead-lettered.
- The server keeps serving.
- The reason is in the log.

## The two compile classes

- `invalid_routine`: a duplicate name, or a name that matches an op. A wrong `in` domain. A change trigger on another domain. A wildcard or unknown entity. A parameter other than `$now` or `$today`. `$caller`. An entity or field the body writes that does not exist. In an `each` body: a name that is not a field of the row the pipeline produces, and a `blob` or `json` row field carried into a write.
- `routine_out_of_bound`: a change routine's `set` with no condition and no `id`, without `unguarded`.
- `invalid_routine` also refuses a bad `notify` line. See "Telling people by email".

`ordo check` prints either one as `DOES NOT COMPILE [class] reason`.

## How to see them

- `ordo check` prints a ROUTINES section.
- The startup log prints a `routines` line.
- A skipped run is a row in `$routineskips`.
- A program with no routines runs the `triggers.*` workers idle, so a later version that adds one starts it with no restart. Idle costs no query.
- A Rust embedding that sets its own wiring (`Runtime::with_triggers`) keeps it across switches. With none, the workers are disabled and the log says "the program declares no routine".

## How often the platform looks

- The change routines' drain visits each tenant once a second while its chain moves.
- A tenant whose chain has not moved is visited less often: after 2 s, then 4 s, then 8 s, up to a ceiling.
- The ceiling is `ORDO_TRIGGERS_IDLE_MAX_SECS`, 10 s by default. It is a deployment setting.
- So a change in a quiet tenant can wait up to the ceiling before its routine is woken. Debounce counts from when the change is read.
- The first change seen puts the tenant back to once a second.
- `ORDO_ROUTINE_EACH_MAX_ROWS` is the other routine setting: the most rows one `each` run handles, 500 by default.
- The scheduled-routine clock reads the tenant list only when a schedule is due.

## A hot deploy

- Any change to a routine, its body, its trigger or its flags, is a new program version. Apply and activate it like any other change.
- An edited body takes effect once that version is active.
- A routine added, retimed or removed by a hot deploy takes effect when each node switches to the version, with no restart. The log prints a `routines:` line with the version.
- `ordo apply` waits for that switch on every node before it returns (`deploying.md` §7, "When a new version is served").
- A routine kept under its name whose trigger changed gets a new cursor when each tenant is next reached after the switch.
- The new cursor starts where the old trigger's cursor stopped. Changes the old one had not read are handed to the new one.
- Once the new cursor passes the old one, the old one is retired and the act is recorded in the tenant's security stream as `listener_deregistered`.
- Only a server on the new version retires it.
- A renamed or deleted routine leaves its cursor behind. The deployment owner retires it with `ordo triggers deregister`.

## What is not built yet

- The top-level `when ... -> run x` line. It is refused with a pointer to the routine form.
- A wildcard entity in a change trigger.
- Reading `$trigger` in a body.
- Rate caps.
