# Review of the evolution plans, 2026-11-23

What this is: the evolution plans read against everything that landed on
`bleeding` from 2026-11-02 to 2026-11-23 (311 commits, PRs 13 to 32, the
rulings of 2026-11-05 to 2026-11-23).

- The plans: `plan-versions.md`, `plan-schema-evolution.md`,
  `plan-evolution-plans.md`, `plan-space-routing.md`,
  `plan-syntax-additions.md`.
- Every edit is marked "(review 2026-11-23)" where it was made.
- Text marked **Proposed** is not ruled. Each has a question below.
- Nothing was built, committed or changed on the board.

---

## 1 · What changed, and why

In `plan-versions.md` unless another file is named.

**From rulings (clear, edited directly):**

- **Every evolution command is a door op.** §1.8. The CLI goes through the
  door (ruled 2026-11-05). `ordo migrate --apply` still runs only with
  `--direct`; increment 8 replaces it. Also §6 conflict 9.
- **`prev` no longer waits on a syntax design.** §2.3, §4 row 15, the pace
  list. It was ruled into the source on 2026-10-04, with `ordo evolve`.
- **`prev` on an external is refused.** §1.15, and
  `plan-syntax-additions.md` §5. Ruled 2026-11-15 (ask 7); the syntax plan
  still said "not settled".
- **Pruning is `ordo evolve --prune`.** §2.7, and
  `plan-syntax-additions.md` §6, which said `ordo plan --prune`.
- **The backfill audit question is closed.** `plan-schema-evolution.md` §9
  now carries the rulings of 2026-09-08 and 2026-09-20. The board task
  said this was owed.
- **The write barrier is replaced.** `plan-schema-evolution.md`, increment
  3, points at the new backfill plan.
- **The plan digest leaves out what an instance binds.** §2.3. An
  external's or a peer's schema is a per-environment setting (ruled
  2026-11-17), so it is bound at the target like other settings (D6).
- **The `requires` check runs wherever a plan is rendered.** §1.13, §2.3,
  §3.1. Ruled 2026-11-14 (ask 6): refusal in the planning phase.
- **The gate includes the external check.** §2.1. It is already in
  `plan_with`.
- **The promotion gate runs the target's own checks.** §3.1, §4 row 4b:
  `requires`, function secrets, externals, readers.
- **The release check never stops a boot.** §2.8. An upgrade must not stop
  a running deployment (ruled 2026-11-06).
- **Tests go through the examples.** §4, a new table. Ruled 2026-11-23.
- **The status line said ask 8 was open.** It was ruled 2026-11-03.
- **`plan-space-routing.md` C-M4** now says the tenant's pinned version,
  not "the activated schema version".
- **`plan-evolution-plans.md` §4** has a note for `ordo evolve`, externals
  and readers.

**From what was built (facts the plan did not have):**

- **Backend functions under a pin.** §1.10. Code is per version by hash
  (built). The function table is the active version's alone (to fix in
  increment 3, session 3). Calls in flight, secrets and egress are stated.
- **Sign-in is one per deployment, never per pin.** New §1.13. The `auth`
  block is versioned with the program (PR 13), but one `AuthApp` serves
  everyone.
- **The public cache belongs to the deployment's own space.** New §1.14.
  Today every install drops it. A tenant's pin switch must not. §6
  conflict 11.
- **Device copies record no program version.** §1.9. A row of the old
  shape can outlive a move.
- **Externals, peers and readers.** New §1.15, and a new plan.
- **Per-tenant databases and buckets.** §2.1, §3.4. Shared databases
  expand once; blob keys hold no field name; a clone never lands in the
  tenant's own storage.
- **Records.** New §1.12: which evolution act is recorded where, by the
  ops log's own admission rule.
- **The identity service.** §1.13, §3.1, §3.4. It moves with the Ordo
  release. The person who promotes is its subject.
- **Reader grants and `AuthApp` follow "active".** §6 conflict 12. Right
  when an activation moved everyone; open once rollouts exist.

**Build order, §4:**

- 10 split into **10** (core, L) and **10c** (strategies, M).
- 11 split into **11a** (dual writes on one tenant, L, research) and
  **11b** (every tenant, M).
- New **7a** (evolution through the examples, M), **7b** (federation, M
  in 2 parts), **17** (restore across versions, M).
- 6, 6b, 8, 9, 10b, 14, 15 re-described against their new designs. 13
  now needs 11b.
- A suggested order: 3 (session 3), 4a, 7, 7a first.

---

## 2 · New plans started

| Plan | Where | Covers | Size |
|---|---|---|---|
| The backfill worker and dual writes | `plan-backfill-dual-writes.md` (new file) | increments 10b, 11a, 11b: phases, the `$backfills` cursor, the move overlay, shared databases, the audit group, backfill functions | 2 L and 4 M steps; the overlay is research, 2 to 3 tries |
| Evolution across deployments | `plan-evolution-federation.md` (new file) | increment 7b: a reader must never read a column the other app stopped writing | 4 S and 2 M steps |
| The release check and editions | `plan-versions.md` §2.8 | increments 6, 6b: what is checked, the `ordo release-check` mode of the one image, the boot-time run | 2 M and 2 S steps |
| Rollouts | `plan-versions.md` §3.7 | increments 10, 10c: `$rollouts`, tenant states, the leading worker, door ops | L and M |
| Restore and import across versions | `plan-versions.md` §2.9 | increment 17, board `snapshot-restore` | M and S |

Each names its tests through an example app: notes-v2 to a new notes-v3,
note-viewer, teamspace.

**The most important finding.** Orphan-instead-of-drop is right inside
one deployment. Across 2 it is a silent failure: after a rename's soak,
the old column stays and nobody writes it, and note-viewer keeps reading
it with no error. `plan-evolution-federation.md` §2 and Q1.

---

## 3 · Still thin, not started here

- **Increment 12b, signed rehearsal.** Provisional by the owner's own
  note. Left as it is.
- **Increment 5, branch tenants, and 16, tenant builds.** Designed in
  `plan-environments.md` §2.2. Not re-read line by line.
- **Increment 13, sealed backfills.** Rides the new worker. Its own design
  is `plan-schema-evolution.md` §7.3, from before dual writes.
- **Moving a live space between databases (18e).** Post-release. Only the
  rule that it never overlaps a version move was added (C-M6).

---

## 4 · For whoever picks this up

- **3 documents are new** and are not in the board's document list:
  `plan-backfill-dual-writes.md`, `plan-evolution-federation.md`, this
  file. `board.py docs set` is owed with the commit.
- **The 16 questions below are not on the board.** They are owed there as
  asks.
- **2 board tasks are stale.** `db-lag-bound` and `db-behind-schema` still
  owe "serve one version back". D15 replaced that bound on 2026-11-02.
- **`schema-evolution`'s `where`** says §9 "needs the ruling folded in".
  It is folded in now.
- `roadmap-checklist.md` was not touched. The split increments (10c, 11a,
  11b, 7a, 7b, 17) have no checklist rows yet.

---

## 5 · Open questions for the owner

Each stands alone. The recommendation is first, in bold.

**Federation**

- **Q1. When does a reader lose a column the read app renamed or removed?**
  **At the soak's close, by a column revoke, with soaks on that table
  closing together.** The reader gets a named error and a full soak to
  switch. Leaving the column readable serves frozen data with no error.
- **Q2. When does a changed `reader` line take effect during a rollout?**
  **When the version is applied, before any tenant moves.** A grant
  belongs to the deployment, and removing consent should not wait for a
  rollout to end.
- **Q16. Are the sign-in columns a reader is granted a contract between
  Ordo releases?** **Yes, kept readable for `editions.load_window` after a
  change.** Otherwise upgrading one deployment can stop `$caller` matching
  in the other.

**Rollouts**

- **Q3. May 2 rollouts be open on one deployment at once?** **No, one.** A
  second apply is refused, naming `ordo rollout status` and `abort`. Two
  at once means a tenant can be between 3 versions.
- **Q4. When does the deployment's own space (public pages) move in a
  rollout?** **First, as a named step, with `rollout.deployment_space`
  to choose `last`.** It is the developer's own data, so it is the
  cheapest place to find a bad version.
- **Q10. A tenant whose own database cannot be reached: waiting or
  failed?** **Waiting.** The failure threshold defaults to 1, so one
  tenant's offline database would pause every rollout.

**Sign-in**

- **Q5. When does a new version's `auth` block take effect in a rollout?**
  **When "active" flips, at the rollout's end.** One rule with new
  tenants, and a half-rolled version cannot change how everyone signs in.
- **Q6. A version removes a sign-in method some people rely on: warn or
  refuse?** **Warn, with the count of people it leaves with no method.**
  Their sessions stay; removing a method is the developer's call.

**Clients**

- **Q7. What does the SDK do with a device copy when the tenant's version
  changes?** **Drop that tenant's copy and fill it again.** The same one
  rule the public cache has on a deploy; a per-shape diff can come later.

**Upgrades and promotion**

- **Q8. Where does the release check run?** **As `ordo release-check`, a
  mode of the new image, read-only on the control database, recorded like
  a `--direct` use.** Only the new binary can compile under the new
  release, and the old host is the one serving.
- **Q11. How does a promotion target fetch the source from the step
  before?** **With a machine key whose grant reads stored versions and
  plans only.** No environment then holds another's database credential.
- **Q12. Who may sign in to a cloned tenant in staging?** **Only a staging
  deployment owner; staging is its own deployment at the identity
  service.** A clone carries members and no sign-in link.
- **Q13. May a restore pin a tenant below `versions.minimum`?** **Yes, and
  the tenant waits closed for its move.** Refusing the restore would block
  a recovery to enforce a housekeeping setting.

**Records and settings**

- **Q9. Which rollout acts go on the deployment's hash-chained stream?**
  **Acts by a person: start, pause, resume, skip, retry, abort, finish,
  one-way confirmation, prune, `--move-to`.** Per-tenant moves stay in
  `$pins`; 5,000 chained entries per rollout would bury the record.
- **Q15. Where do the rollout, backfill, soak, promotion and clone
  settings live?** **Stored in Ordo beside the 3 version settings, set
  through the door.** They differ per environment, so the file is wrong
  for them, and the 2026-11-05 reason (they depend on what is applied)
  holds.

**Backfills**

- **Q14. May a backfill function call out or read secrets?** **No by
  default, with `backfill.function_egress` as the opt-in.** A function
  that calls out can differ between staging and production, which the
  rehearsal exists to rule out.

---

## 6 · Answers, 2026-11-23

On the board the same day. 13 ruled, 3 asked again. `corpus-rulings.md`
has each answer in the owner's words.

- **As recommended:** Q1, Q2, Q4, Q5, Q7, Q8, Q9, Q10, Q13, Q15, Q16.
- **Q6, changed:** the warning stops the evolve until the deployment owner
  approves it, and it gives the migration steps.
- **Q12, wider:** any staging deployment admin or a person staging allows.
- **Asked again:** Q3 (one tenant moved to a hotfix during a rollout), Q11
  (put in plain words), Q14 (with the cases a backfill may need secrets or
  the network for).
