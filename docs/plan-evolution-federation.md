# Evolution across deployments: externals, peers and readers under versions

Status: **the shared-database parts are replaced (G8 of `plan-federation-grants.md`, 2026-11-26).** One Ordo deployment now reaches another only through its door (owner, board `federation-ordo-deployments-share-data-through`, asks 1 and 2: "Remove"). What is left of this plan, and what replaced the rest:

| Here | Now |
|---|---|
| §1, §3.1 to §3.5: the `reader` line, grants by column, the renamed-column comment, `changing` notes, `/health/ready` `external.changing` | **Replaced.** A holder opens ops with `grant`; a consumer reads through `peer notes { … }`. A field a consumer reads that the holder renames or stops declaring is refused by name at the consumer's read (`peer_refused`) |
| F1: the plan lists the readers of a changed table | **Kept, on grants.** The plan lists each `grant` that opens the table (`consumer_grants::tables_read`), outside the digest. No grant end date: there is no column grant |
| §4: plain externals | **Unchanged** |
| §5, F6: the release check names peers and readers by reading their `$ordorelease` | **Replaced.** Each side keeps the release the other said through the door (`$peeruses` on the holder, `$peerholders` on the consumer), recorded at the consumer's apply |
| F4: a rollback is checked against the external tables | **Kept for plain externals.** A peer is checked by its holder at each read |
| `seal_read_by_reader`, `peer_tenant_database` | **Removed** with the path they guarded |

The rest of this file is the plan as it was built, kept for its reasoning.

Was: **plan started 2026-11-23; Q1, Q2 and Q16 ruled 2026-11-23; F1 to F6 built, save soaks closing together on a shared database (11b); see §6.** Written
by the review of the evolution plans (`review-evolution-plans-2026-11-23.md`).
It is increment 7b of `plan-versions.md` §4. Its open questions are Q1, Q2
and Q16 of that review.

Read this first:

- Two deployments in one database can read each other's tables since
  2026-11-19 (`peer`, `reader`, external entities).
- Each deployment has its own versions, pins and rollouts.
- A pin is a promise inside one deployment. It promises a reader nothing.
- So one deployment's schema change can break another, or worse, leave it
  reading a column nobody writes any more.
- This plan makes that an error with a named fix, never wrong data.

Words used here: **the read app** owns the table and writes the `reader`
line. **The reader** declares the `peer` and the external, and reads.

---

## 1 · What is built, and what it does at a version change

Built (`definition-language.md`, "`peer`" and "`reader`";
`plan-external-entities.md` §7):

- The reader's translation maps the read app's real columns by name.
- The read app grants the reader's database role `SELECT` on each table
  its `reader` line names. The grant covers the whole table.
- Grants are made equal to the **active** version's file at every apply
  and every start (`deploy.rs`, "only for the active version").
- The reader checks each mapped column against the live table at every
  apply, and on a timer for the deployment's own database
  (`ORDO_EXTERNAL_WATCH_SECS`, 300).
- A mapped column renamed, retyped or dropped holds the whole entity with
  `external_changed`, naming the column and the fix.
- A peer must be in the same database. A tenant in a database of its own
  does not reach a peer.

What that does when the read app moves from X to Y:

| The read app's change | Today | Under `plan-versions.md` §2 |
|---|---|---|
| Adds a field | Nothing. The reader does not map it | The same |
| Renames a field the reader maps | The old column is gone after the apply; the reader is held | The old column stays through the soak and stays current. After the soak it stays, **and nobody writes it** |
| Removes a field the reader maps | The same as a rename | The column is orphaned: kept, never written. **The reader keeps reading it** |
| Retypes a field | Held | The same as a rename |
| Removes a table from the `reader` line | Revoked at the apply; the reader is refused at its next start | Revoked when "active" flips (Q2) |

The 2 bold cells are the gap. Orphan-instead-of-drop is ruled (2026-10-01).
It is right inside one deployment. Across 2 it turns a loud break into a
silent one.

---

## 2 · The rule

**A reader reads only columns the read app still writes.**

- A column the read app writes for every tenant: readable.
- A column it has stopped writing for any tenant: not readable. The reader
  is held with `external_changed`.
- The reader never gets wrong or frozen data. It gets an error that names
  the column, the new name when there is one, and the command that fixes
  its file.

This is the external entities rule (owner, 2026-11-15: a human is involved
only when an op would break) carried across a soak.

---

## 3 · The design

### 3.1 Grants by column

- The read app grants the reader `SELECT` on the columns its served
  versions declare, per table named on the line. No longer on the whole
  table.
- While X and Y are both served, the grant is the union: old column and
  new column.
- When the last tenant's soak on that table closes, the old column leaves
  the grant in the same transaction that writes `$orphans`.
- The reader's check already treats a lost grant as `external_changed`.
  The message gains the reason: "the read app stopped writing `title` on
  2026-12-01; it is now `heading`."
- `$readergrant` records columns, so a removed line still revokes
  everything.

### 3.2 The window the reader has

In a shared database, tenants move one at a time and share the table.

1. Expand: the new column exists. Empty for tenants still on X.
2. Each tenant's backfill fills it. Dual writes keep both current.
3. **Every tenant backfilled:** both columns are full and current. The
   reader may switch from here.
4. **First soak closes:** by §2 the old column stops being readable.

- The reader's window is from step 3 to step 4.
- **Ruled 2026-11-23 (Q1):** on a table a `reader` line names, the soaks close
  together, when the last one is due. That makes the window at least
  `soak.length` long (7 days by default).
- A soak length of 0 on such a table needs the one-way confirmation,
  naming the readers.

### 3.3 The read app is told before, the reader is told during

- `ordo evolve` lists, for each step that renames, retypes or removes a
  field of a table on a `reader` line: the reader's nickname, the field,
  and the date its grant ends. It is a warning. It refuses nothing.
- The read app cannot see the reader's file, so it lists every reader of
  the table, whether or not that reader maps the field.
- The reader's watcher reports a peer column that is in the grant's union
  state as "changing", with the new name, on `/health/ready` and in the
  log. It holds nothing yet.
- **Built 2026-11-24 (part 3):** the read app writes the comment
  `ordo: renamed to <new column>` on the old column of each rename on a
  read table (`ordo_exec::peer::Renamed`). Every role reads
  `pg_description`, so the reader reads it. Its check notes the column as
  `changing:` in the plan and the log while it is written, and names the
  new column in `external_changed` once it is not. **2026-11-24 (part 4):**
  `/health/ready` names them too, for the deployment's people, under
  `external.changing`, beside `external.held`.
- The fix on the reader's side is the built one: `ordo introspect
  --external` again, then apply. The reader's own version changes.

### 3.4 The reader's versions

- The reader may serve 2 versions with 2 translations. Each loaded version
  is checked against the peer, not the active one alone. A version whose
  translation no longer matches is held for the tenants pinned to it. The
  others serve.
- `ordo rollback --dry-run` on the reader runs the check for the version
  it would return to, and says when that version can no longer read its
  peer.
- The same check runs in the promotion gate at each target, against that
  environment's peer (`plan-versions.md` §3.1).

### 3.5 When the `reader` line itself changes

- Grants follow one version of the read app's file. A grant is a fact
  about the deployment, never about a tenant.
- **Ruled 2026-11-23 (Q2):** a line removed or narrowed takes effect when the
  version is applied, before any tenant moves. A line added or widened
  takes effect then too. A rollback puts the earlier line back.
- The reason: taking consent away should not wait for a rollout to end.
- A staged version on a branch tenant or a tenant build changes no grant.
  The apply report says so.

---

## 4 · Plain externals (a table no Ordo deployment owns)

- A version pin does not cover the foreign table. A rollback of the
  program does not roll the table back.
- So `ordo rollback --dry-run`, the promotion gate and every per-tenant
  gate run the external check for the version they are about to serve.
  All 3 use `plan_with`, where the check already is.
- A foreign table kept per tenant database is checked when a program is
  applied to that database, and by the timer: **built 2026-11-24 (F3)**,
  each tick reads every tenant with a database of its own, in its own
  database, and a hold found there holds that tenant only.
- A staging clone carries no foreign table (`plan-versions.md` §3.4).

---

## 5 · Ordo releases on the 2 sides

- `$caller` on a peer's table reads 4 of the peer's sign-in tables:
  `members`, `$principal`, `$authenticator`, `$authconnection`.
- Those are Ordo's tables. Their shape moves with the Ordo release, not
  with a program version.
- 2 deployments in one database can run 2 Ordo releases.
- **Ruled 2026-11-23 (Q16):** the columns a reader may be granted in those 4
  tables are a named contract. A release that changes one keeps the old
  column readable for `editions.load_window`. The release check
  (`plan-versions.md` §2.8) names each peer and reader on another release.
- **Built 2026-11-24 (F6):** the contract is `ordo_exec::sign_in_contract`.
  Each deployment records its release in `$ordorelease` and grants it to
  its readers. A reader grants the read app nothing, so the read app's
  check names each reader by its role, without its release.
- A sign-in provider removed from one side's `auth` block ends the match
  for people who used it. They read nothing; nothing leaks. The pre-check
  on that version says: "this version removes `google`, the only provider
  shared with peer `journal`."
  **Built 2026-11-24** (branch `evo-small-remainders`): the note names each
  peer either version reads. It cannot say "the only provider shared",
  because a peer's `auth` block is not readable from here, so it says the
  peer matches people by their sign-in and whose owner to tell.

---

## 6 · Build steps

| Step | Size | What | Needs |
|---|---|---|---|
| F1 | S | `ordo evolve` lists readers per changed field, with the date the grant ends. **Built 2026-11-23** (6d93397c): the date is the render's plus the 7-day soak default until soaks exist (11a) | increment 8 |
| F2 | M | Grants by column; the union while 2 versions serve; the revoke at soak close; `$readergrant` by column; soaks on a read table close together. **Part built 2026-11-23** (increment 14): a table holding an orphaned column is granted by column, without it, and the reader's check names the column it lost (`external_changed`). With no soaks yet, the revoke comes when the column is orphaned. **More built 2026-11-24** (increment 11a): a renamed field's old column is orphaned at the soak's close, not at the step, and the grants are made again there, so the whole-table grant holds both columns (the union) through the soak and loses the old one at the close (Q1). A tenant with a database of its own has no reader there (a peer is in the same database), so on it the revoke grants nothing new; the same close serves a shared database once 11b moves one. **Part 3, 2026-11-24** (branch `evo-7b-part3`): on the apply path, a shared database's rename has no soak (11b), so it ends when a version drops the `prev` mark. That version stops writing the old column, so the apply records it in `$orphans` (`schema_apply::record_orphans`) and the reader loses it by a column grant. The read app comments each renamed old column on a read table, and the reader's check names the new column (§3.3). **Part 4, 2026-11-24** (branch `evo-7b-part4`): `$readergrant` records each table and column it granted (`grants`). A line removed or narrowed revokes exactly those, and leaves a grant a person made by hand. The schema's `USAGE` is recorded as Ordo's only when Ordo gave it, and revoked only then, with the line gone; a usage the role held before stays, so a table granted by hand still reads. A row from an earlier release still revokes every table, as then, and never the usage. Test: `reader_grants.rs` (`a_removed_line_revokes_what_it_recorded_and_nothing_granted_by_hand`). **Not built:** soaks on a read table closing together (11b) | increment 11a |
| F3 | S | The reader checks every loaded version against its peers and externals; a mismatch holds that version's tenants only; the timer covers tenant databases. **Built:** every loaded version (with 3); **2026-11-24** (branch `evo-7b-part3`) the timer reads each tenant with a database of its own, in its own database (`ExternalGate::watch_tenants`), and holds that tenant only. Test: `external_drift.rs` (`the_timer_reads_each_tenant_database_and_holds_that_tenant_only`) | increment 3 |
| F4 | S | The check in `ordo rollback --dry-run` and in the promotion gate; the "changing" notice on `/health/ready`. **Built 2026-11-23** (9856f637): `std.deployment.rollbackCheck` and `ordo rollback --dry-run`; the gate's check was built in 4b. **Part 3, 2026-11-24:** the "changing" notice in the plan and the watcher's log (§3.3). **Part 4:** on `/health/ready` too | increments 4b, 8 |
| F5 | M | The example test in §7. **Part built 2026-11-24** (branch `evo-7b-part2`): note-viewer e2e project `rename` (`tests/rename/title-to-heading.spec.ts`, fixture `notes-heading`, 0.3.1) is test 1: notes renames `title` to `heading` with `prev`, and the viewer shows a note from before and one written after, each with its title. **Part 3, 2026-11-24** (branch `evo-7b-part3`): tests 3 and 4 in project `rename`: notes lands the rename (0.3.2 drops the mark), Postgres refuses the viewer `title`, and the page names `title (it is now heading)` and the fix; the viewer maps `heading` (0.1.1) and shows every note. Test 1 also checks the viewer's log says the column is changing. Test 5 in project `reader-line` (`tests/reader-line/line-narrowed.spec.ts`): notes takes `notesAccess` off its line, Postgres refuses it at once, and a viewer node started then is refused quoting the line. Test 6 as a host test (`reader_grants.rs`): `rollback_check` on the reader's version that maps `title` names it after the rename lands. **Part 4, 2026-11-24** (branch `evo-7b-part4`): the harness signs the developer in as the notes e2e does, with the helper both share (`examples/e2e-shared/developer.ts`); both doors take tokens from the identity service's `main`. Test 2: the developer stores 0.3.1 and `ordo evolve --from 0.3.0 --to 0.3.1` names `note_viewer` and the date. Test 6: `ordo rollback --version 0.1.0 --dry-run` on the viewer is refused naming `title (it is now heading)`, and 0.1.1 stays active. Test 1 also reads `external.changing` on the viewer's `/health/ready`. Every test of §7 runs through the example | F2 |
| F6 | S | The sign-in column contract and the release check's peer list. **Built 2026-11-24** (branch `evo-7b-part2`): `ordo_exec::sign_in_contract`. `CONTRACT` lists the 13 columns a reader is granted in the 4 tables; a release that changes one marks it `Retired` with its date, and it stays granted until that date plus `editions.load_window`, measured against the build's release date. Each start records the release in `$ordorelease`, whose `release` column every reader is granted. `ordo release-check` lists each peer with its release (`ANOTHER RELEASE` when it differs) and each reader by its role, and names a column this release retires with its end date. Neither fails the check. Tests: the contract's unit tests; `reader_grants.rs` (`the_sign_in_contract_is_granted_and_the_release_check_names_the_peer_on_another_release`); note-viewer e2e `tests/api/release.spec.ts`. **Not built:** a reader's release, which the read app cannot see (asked) | increment 6 |

What sets the pace:

- The owner's answers to Q1 and Q2. F2 cannot start without them.
- F2 changes live grants. One rehearsal on the 2 test namespaces, which
  hold nothing (owner, 2026-11-18).
- About 4 sessions and 5 CI rounds in all.

---

## 7 · Tests

Through the examples (owner, 2026-11-23), in note-viewer's own e2e, with
notes-v2 as the read app:

1. Notes moves to v3, which renames `notes.title` to `heading` with
   `prev`. A browser on note-viewer reloads through the expand, the
   backfill and the soak. Every note shows, with its title.
2. `ordo evolve` on notes printed note-viewer's nickname and the date.
3. The soak closes. Note-viewer answers `external_changed`, and the page
   shows the developer's error state. The message names `heading`.
4. Note-viewer regenerates its translation and applies. The page shows
   every note again.
5. Notes removes `notesAccess` from its `reader` line and applies.
   Note-viewer's next start is refused and quotes the line to add.
6. Note-viewer rolls back to its earlier version. `--dry-run` says that
   version can no longer read `title`.

Host tests, beside `external_drift.rs`:

- The grant is the union while 2 versions serve, and exactly Y's columns
  after the soak.
- A reader with 2 loaded versions holds only the tenants of the one that
  no longer matches.
- A removed line revokes every column `$readergrant` recorded.

---

## 8 · Not in this plan

- Moving 2 deployments in one step. They are separate deployments by
  ruling (2026-11-15). The protocol is: the read app expands, the reader
  switches, the read app contracts.
- Tying the reader's spaces to the peer's. Filed as `peer-spaces-tied`.
- Writes to a peer. Read-only by ruling.
- Peers in different databases. Not built, and no ruling asks for it yet.

---

## 9 · Open questions for the owner

- **Q1. When does a reader lose a column the read app renamed or removed?**
  Recommendation: **when the soak closes, by a column revoke, with the
  soaks on that table closing together.** The reader then gets a named
  error and at least one full soak to switch. The other choice, leaving
  the orphaned column readable, serves frozen data with no error.
- **Q2. When does a changed `reader` line take effect during a rollout?**
  Recommendation: **when the version is applied, before any tenant
  moves.** A grant belongs to the deployment, and removing consent should
  not wait hours for a rollout to finish.
- **Q16. Are the 4 sign-in tables' reader columns a contract between Ordo
  releases?** Recommendation: **yes, kept readable for
  `editions.load_window` after a change.** Otherwise upgrading one
  deployment can stop `$caller` from matching in the other, and nothing in
  either file changed.
