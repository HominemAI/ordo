# Audit — coverage, 2026-09-07

Re-run of `audit-board-coverage.md` (2026-08) against today's state, plus
three directions that audit did not have: the checklist's own prose, the
owner's answer file, and the decisions board's accuracy about itself.

The instruction was *"go through our docs — and our webdoc — and make sure
nothing has been missed."* Four sweeps, each stated so a later reader can
falsify it rather than take it on trust.

---

## 0 · The one-sentence finding

**The project's tracking is accurate about work it has *done* and unreliable
about work it has *named*.** Nearly every remainder found below was already
written down — in a closed item's "remainders" paragraph, in a ruling's
"found while building" list, or in a plan marked `[open]`. None of them was
reachable by asking the checklist what is open, because a `[x]` item's prose
and a `[~]` item's tail match no sweep for `- [ ]`. That is why an obligation
the owner approved on 2026-09-06 was still unbuilt on 09-07 with nobody
tracking it, and why a settled subsystem with zero lines of code has had no
row since August.

---

## 1 · What was checked, and how

Falsifiable, in the order run.

| # | Check | Method | Result |
|---|---|---|---|
| 1 | Every file in `docs/` enumerated | `ls docs/` — 57 files | 3 audit records and 4 critique/hardening records excluded as records, per the prior audit's own rule; the rest read or status-censused |
| 2 | Status-mark census across every `.md` | count of `[settled]` / `[open]` / `[proposed]` / `[lean]` / `[pinned]` per file | found the two `std-library.md` `[open]`s nobody tracked; confirmed `traps.md` / `syntax-examples.md` hits are false positives (`[opened, priority]` in sample code) |
| 3 | All 28 findings of `audit-board-coverage.md` §2 | each re-checked against `crates/` by grep and against the checklist by name | 8 still had no row anywhere → now §11 |
| 4 | Every unchecked checklist item | `grep -n "^\s*- \[ \]"` | 14 before (one of them stale — `check` row invariants, closed by 9e two days earlier and never ticked); 29 after |
| 5 | `~/Desktop/decisions.json`, all 23 answers | each traced to code, a ruling, or a checklist item | 1 decided-and-unactioned, 1 decided-and-misreported |
| 6 | Every row of `docs/decisions.html` | 82 rows parsed and printed with `ref` / `status` / `sub`; the substantive ones read in full | 13 rows stale or contradicting themselves |
| 7 | Corpus coverage claim | `EXECUTED` / `NOT_RUN` / `DIVERGENT` counted out of `guards_corpus.rs` | claim was 85/100; truth is 86/100 |
| 8 | Named remainders from the last three days | each of the 16 named in the brief traced to a home | 6 homed in a closed item's prose; 3 genuinely orphaned |
| 9 | `decisions.html` integrity after every edit | row count + per-line quote parity + brace/bracket balance + unique ids, by script | green after every commit; 82 rows → 89 |

**Grep receipts** — the ones a reader should re-run first, because each is a
single claim that a single command settles:

```
grep -rn chain_proof crates/                 # → nothing. Layers 2–4 unbuilt.
grep -rniE '\breap\b' crates/ --include=*.rs # → nothing. No reconciler.
grep -rn log_position crates/                # → nothing. No snapshot reads.
grep -rn observed_cost crates/               # → nothing. plan-tiers item 2.
grep -rn secevent crates/ordo-cli            # → nothing. Nothing verifies it.
grep -rn '"dev"' crates/ordo-cli/src/main.rs # → no `dev` verb. No hot loop.
grep -rniE 'break.?glass' crates/ --include=*.rs  # → 5 comments, 0 producers.
```

---

## 2 · What was missing, and where it now lives

### 2.1 Decided by the owner and not acted on — the two that matter

**(a) The backfill audit encoding.** On 2026-09-06 the owner approved schema
evolution and added: *"the audit line should split and a summary should be
created — the idea is that the data integrity is guaranteed so data before =
data now in a different structure."*

That is an answer to the one question `plan-schema-evolution.md` §9 leaves
open. It was **never recorded**: §9 still reads `[open]`, still recommends
option (1) — full per-row images, the *opposite* of the answer —
`corpus-rulings.md` has no entry, and checklist 13c still says "blocked on
§9". Worse, the decisions board asserted the opposite of the truth: the
`schema-evolution` row read *"Your audit-line ruling … is recorded in the
plan for increment 3."* It is recorded nowhere.

The answer also collides with a recorded defect the owner has not been shown:
summarised batch groups are settled **for creates** ("creates have no before,
and the after is the database"), and a backfill is all **converges**, whose
before image is the only record of what the value was. Z-014 records the
consequence — a corrected rerun cannot restore the old value.

- → new decisions row **`backfill-audit`**, in the *Blocked* group, putting
  the collision to the owner with a recommendation (option 2 plus a
  **content hash of the pre-transform values**, which makes "data before =
  data now" a checkable claim rather than an assertion, at one hash per
  chunk rather than one image per row) and a disclosure obligation on
  `ordo plan`.
- → the false claim in the `schema-evolution` row retracted in place.

**(b) The owner-administration flag.** DL §3 rule 6 requires an
owner-administration command to be *"flagged as owner administration"*. The
owner approved adding the field on 2026-09-06. `CommandGroup`
(`ordo-commandlog/src/command.rs:154`) carries `correlation`, `actor`,
`commands` and no flag. Without it an auditor cannot distinguish "the owner
used their rule-6 capability" from "the owner happened to hold a level",
which is the one distinction the rule exists to make.

- → checklist **15c**; the `owner-admin` row's `sub` now says so, and its
  `suggest` explains *why* it slipped (8z is `[~]`, so its tail matches no
  open-item sweep).

The other 21 answers were traced and are acted on. Two worth recording: the
`secevent` answer asked for a performance and cost audit, which was done
(~1.0 µs per denial, growth per collapsed window rather than per request);
and the `sharing` answer asked for three options with ELI2 explanations and
diagrams, which `sharing-options.html` delivers — three architectures, three
inline SVGs, and a 22-item consent surface.

### 2.2 Settled subsystems with no row anywhere — eight, now §11

Each re-verified by grep today, not inherited from the August audit.

| New item | The finding | Why it stayed invisible |
|---|---|---|
| **11-11** Key rotation & epoch lifecycle | No re-seal worker, no SMK-rotation re-index, no undo-horizon-driven destroy. So an epoch can never actually be retired, and *"DEK rotation is mandatory on every membership revocation"* rotates a key that still opens everything. | 8b/8c/8d closed the *naming* half and read as the whole. `f7-vault-plan.md` §3.4 closes a departed member's equality oracle on a mechanism with no code. |
| **11-12** Attestation layers 2–4 | `grep chain_proof` → nothing. No webhook, no witness, no transparency log. `chainProof` is `[settled]` in `implementation.md`. | The board's `Layer-1 attestation pinning` row reads as though layer 1 were the whole story — and layer 1 genuinely shipped. |
| **11-15** Space provisioning lifecycle | `reap`, `reconciler`, `bootstrap_timeout` match nothing. A half-created space holds keys and storage nobody reclaims. | `std.space.create` exists as an op name, which looks like the feature. |
| **11-16** Deployment-scoped ops | `bootstrap.md` §8 marks the `CapabilityGrant` addition `[open]`, *explicitly awaiting an owner ruling* — and the ruling had never been asked. | An `[open]` inside a settled document, with no mechanism that surfaces it. |
| **11-17** Log-positioned reads | `log_position` matches nothing; 14b is specified on top of it. §14's own preamble names it and then opens four items instead of five. | Easily confused with 8q's layer-1 attestation pin, which *is* a position on a response and is not a read snapshot. |
| **11-22** Deployment operational security log | The third artifact of the three-artifact split. A principal sweeping a hundred spaces appears in **none** of their streams, correctly and by design. | §7 shipped the per-space stream, which reads as full coverage. |
| **11-25** `plan-tiers.md`'s two cheap-now items | No `tier` field in the artifact, no observed cost from the executor. | The document is `[proposed]` and "not v1", so the whole file read as deferred — including the two things it says not to defer. |
| **11-26** Break-glass operator mode | Zero implementation, and **five modules name it in prose as an act the custody stream records**. A reader of that code concludes the capability exists and is audited. | The class-with-no-producer shape, in comment form. |

### 2.3 Obligations that lived only in prose — now §15

- **15a** — the schema-diff engine's three: `same_type` canonicalises the
  typmod away (`t.split('(').next()`, so `numeric(10,2)` and `numeric(20,12)`
  are one type — latent, because `ddl_for` emits unparameterised types, and
  live for BYO-DB or a hand-altered database); no taxonomy code for `check`
  in either direction, which **13b needs before a check can be added to a
  live deployment**; and `tenancy()` running a `count(*)` per entity on every
  `ordo plan --against`, a full scan per entity on the command sold as the
  cheap one.
- **15b** — `fixtures/seed.sql` seeds every sealed column with
  `'\x00'::bytea`, a placeholder rather than an envelope, under a comment
  that stopped being true when F0 landed.
- **15c** — the rule-6 flag (§2.1b).
- **15d** — refinement is shipped and unreachable: no op-level `refinable`
  marker exists in the language, so every op registers `RefineTier::None`;
  the effective class is not traced; the executor ignores `$refine`; and 5c's
  host-side filtering needs a disclosure rule nobody has written. All four
  lived inside two `[x]` items.
- **15e** — `ordo dev` does not exist and its bar is `[settled as bar]`:
  save-to-serving under one second, *"anything over a second on save is a bug,
  not a tuning opportunity."* The binary dispatches `check`, `plan`,
  `interface` and the operator commands. Honest sequencing noted: the budget
  is dominated by DDL apply, so this waits on 13b.
- **15f** — §14's group names five surfaces and opens four items.
- **15g** — `std-library.md`'s two `[open]`s, both marked *before first ship*:
  `whole`'s per-collection cap (and how a renderer shows a `truncated`
  collection — now concrete, because 14a's descriptor is what a renderer
  reads) and the rollup promotion criteria.

### 2.4 A new finding, found while checking §7

**7d — nothing verifies the security-event stream, and no owner can read it.**

`ordo-secevent::chain` has `verify_stream` and `verify_stream_from`. The
stream is hash-chained, checkpointed, and signed under its *own* key with its
own domain tag — 7a argues for the second key on the grounds that one key
would mean "compromising the routine attestation key is also the ability to
forge the record of the compromise". And `grep -rn secevent crates/ordo-cli`
returns nothing: `ordo verify` walks the command chain only. **The verifier
exists and no command, route or job has ever called it.**

Separately, §6.1's members-only routing exists precisely so a space owner sees
the denials in their own space, and the only route is `/security/alerts` —
the deny-volume monitor's firings, not the stream. There is no read surface.

- → checklist **7d** with a done-when (one command, two artifacts, two
  answers — a merged verdict hides which chain moved); the `secevent` board
  row moved from `Done` to `Partial` and now carries the question.

---

## 3 · Webdoc corrections

`docs/decisions.html`: **82 rows → 89**. Integrity re-verified after every
edit (row count, per-line quote parity, brace/bracket balance, unique ids);
the file still opens from `file://` with no build step.

**Seven rows added** — one blocked question and six subsystems that were
settled and tracked nowhere:

| Row | Group | The question put to the owner |
|---|---|---|
| `backfill-audit` | Blocked | His own answer, collided with Z-014, with a fix that keeps his guarantee and makes it checkable |
| `attestation` | Designed, not built | Which of layers 2–4 — recommends `chainProof` + offline verifier first (the bytes already leave in the export bundle), then the webhook; recommends *against* the public transparency log |
| `rotation` | Designed, not built | Re-seal worker now vs. after 13c's write barrier — and strike the vault claim that rests on a mechanism with no code |
| `provisioning` | Designed, not built | Whole lifecycle, or the minimum that stops a failure leaking keys and storage |
| `breakglass` | Designed, not built | Build or decline — with the argument that declining is not actually available, and that either way five comments must change today |
| `deployment-scope` | Designed, not built | Distinct grant kind vs. an optional space field — recommends distinct, because `None` meaning *everything* is the worst default shape in an authorization record |
| `opslog` | Designed, not built | Its own chained artifact or nothing — and who may read the one record that belongs to no space |

**Thirteen rows corrected.**

| Row | Was | Now |
|---|---|---|
| `row-bound-paging` | `Salvaged` · "carries an unexplained 1,803-line golden move" | `Done` — ruled, and the golden explained as `row_scope` on 16 listings, audited op by op |
| `auth` | `Done` pill over text saying no passkey provider exists and step-up is unsatisfiable | `Done`, with what landed — and a new ask: `licensing.md` says the engine must refuse to start without `ORDO_APEX_DOMAIN`, the checklist files it under "left to the deployment", and those cannot both stand |
| `triggers` | `Done` pill over text saying nothing drains the queue | `Partial` — §12 landed; every loop step is built and **nothing calls any of them from a long-running task**, so a trigger written today enqueues and sits forever |
| `delegation` | `Done (core)`, no `decided`, ask still "does the product need agents acting for users?" | `Partial`, `decided` recorded, ask replaced with the `$caller` rebinding that decides whether the feature returns rows at all |
| `export` | `Done (core)`, ask still "Build it?" | `decided` recorded; ask is now the **BYO-DB plaintext-backup consent gap**, with proposed wording, plus the other five remainders named |
| `effective-cost` | `Done` pill over text saying nothing computes it | `Partial` — built and **unreachable**, because no op-level `refinable` marker exists in the language |
| `secevent` | `Done` | `Partial` — §2.4 |
| `uigen` | `In flight` · "step 1 building now" | Step 1 landed as 14a; the new blocker is a **language change** (§10.3's five UI annotations are inert and unlexed), which is a decision, not a task |
| `schema-evolution` | claimed the audit-line ruling "is recorded in the plan" | claim retracted, pointed at `backfill-audit` |
| `corpus` | "85/100 executing" | 86/100 (counted out of `guards_corpus.rs`), and GC048 flagged as open across three audits |
| `vault` | `Planned` over a suggest reading "here is what I will build" | says plainly that §5.3 is *ruled* and **not one line of F7 is built** — 26–36 engineer-weeks, and Q7 is a defect in the spec |
| `owner-admin` | "Built — one remainder" | "you approved the flag on 6 Sep and it is still not there" |
| `playground` | `Partial`, empty ask and suggest | `Done` as 4d |

`docs/corpus.md` corrected from 85/100 to 86/100, with the verification method
named so the next reader can re-count rather than re-trust.

One stale checkbox closed rather than added: the `check` row invariants
sub-bullet under 9B was still `- [ ]` on 09-07, two days after 9e built it.
The blocked-state reasoning under it is kept verbatim — it is the record of
why `check_violation` had to be minted before the lowering, and both of its
"things the next attempt should know" proved exactly right.

---

## 4 · Verified as covered — the negative results

Recorded because an audit that only lists gaps cannot be checked.

**The August audit's findings that have since landed**, each confirmed in code
today rather than from the checklist: triggers and the provenance bound (#1,
§12); delegation (#4, 11-4 + `ordo-identity/src/delegation.rs`); auth
providers (#5, `passkey.rs` / `webauthn.rs`, 93 tests, 5h); effective cost
class (#8, 11-8 — and `refinement_unbounded` now exists, which was the August
audit's own board-accuracy note #1); schema evolution increment 1 (#9, §13a);
exit ops (#10, 11-10); capability catalog (#13); refinement envelope (#14);
interface descriptor (#24, 14a). Inter-space sharing (#2, #3) and blob custody
(#7) and domain packages (#18) and egress (#6) correctly remain owner
questions rather than work items.

**Named remainders from the last three days, each traced to a home.** Six were
already homed in 11-10's remainder list and are *not* new findings, contrary
to a first pass that missed them on wording: blob bytes, bundle delivery,
BYO-DB plaintext backups, the confirmation window, legal hold, and
`PlatformExecutor` dropping the durable `ReplayLease`. `$caller` rebinding is
11-4a. §13b–13e and §12e exist as written. The six sharing contradictions are
in `sharing-options.html` §5 — but only contradiction 1 needed *ruling* before
anything can start, and the `sharing` row did not say so; it does now.

**Documents checked and found fully represented**, beyond the August list:
`plan-idempotency.md` (8q, closed — and the audit changed the plan's own
recommendation, which is recorded); `plan-delegation-surface.md` (increments
map to 11-4a–d); `plan-export.md` (11-10); `corpus.md`; `salvage-2026-09-05.md`
(all seven branches landed, and the record says so); `howto-*.md`;
`typed-id-keys.md`; `writing-data.md`; `deploying.md`; `glossary.md`,
`modeling-guidance.md`, `tutorial.md`, `syntax-examples.md`, `traps.md` and
the two `sql-translation-corpus` files, which carry no status marks and are
teaching material rather than capability specifications.

**Two things deliberately left alone.** `eval-harness-plan.md` remains a plan
with no row, which was the August audit's own conclusion and is still right.
`migration.md` §1–§5 (`ordo introspect` / `ordo import`) remains `[proposed]`;
`ordo import` is named where it bites — it is why an export cannot be
re-sealed under a destination key — and opening an item for a proposal nobody
has ruled on would be the mistake 9e′ caught.

---

## 5 · What still needs the owner, in the order I would ask

1. **`backfill-audit`** — blocks 13c, and the group encoding cannot change
   after it has written a chain.
2. **`sharing`** contradiction 1 — decides *where* the feature is built.
3. **`breakglass`** — because whichever way it goes, five code comments are
   lying today.
4. **`auth`** — `ORDO_APEX_DOMAIN`: one boot assertion against a class of loss
   that has no repair.
5. **`delegation`** — `$caller` rebinding, without which the shipped feature
   returns empty results.
6. **`corpus` GC048** — two settled cases that cannot both be right, open
   across three audits.
7. `deployment-scope`, `attestation`, `rotation`, `opslog`, `provisioning`,
   and `uigen`'s annotation-lexing question — no deadline, but each is a
   subsystem the documents settle and the board could not see.

---

## 6 · The mechanism this audit suggests changing

Three of the thirteen stale rows and every item in §15 share one cause: **a
remainder recorded inside a closed item is not tracked, it is buried.** The
project's convention of writing "Remainders, named rather than left to
discovery" at the end of a `[x]` item is good discipline and produces the
opposite of its intent, because the sweep everyone actually runs is
`grep '- \[ \]'`.

The cheapest fix is the one §12e already models: when a closed item names a
remainder, open a sibling item for it in the same commit. §15 is this audit's
retrofit of that rule over the items that did not follow it. It should not
need a second retrofit.
