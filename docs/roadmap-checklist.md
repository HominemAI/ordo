# Ordo — remaining roadmap

The live checklist. Items are checked off as they land, each with the
commit that did it. Everything above the line already executes end to end:
compiler (reads + writes), guards, sealing, cost gate, pooled executor,
HTTP engine, keyset paging, and the ops-truth catalog.

Status legend: `[ ]` not started · `[~]` in progress · `[x]` done.

---

## 1 · Real credentials (the last deployment blocker) — **DONE**

Was: tokens were the conformance `member:<id>` form — an asserted
principal, not a proven one. `ordo-identity` already had a complete
`AccessEngine`; what was missing was the layer beneath it that turns a
bearer string into verified `TokenClaims`. Now built and at the door.

- [x] **1a · Token layer.** — `d4aab37` Mint and verify signed bearer tokens →
      `TokenClaims`. Domain-separated MAC, explicit expiry, constant-time
      compare. Every rejection one opaque failure (a talkative verifier is
      an oracle).
- [x] **1b · `IdentityAccess`.** — `d4aab37` An `AccessChecker` over `AccessEngine`:
      verify → claims → `check(claims, space, op, now)` → principal.
      Replaces `MemberAccess` in the host. Denials stay one wire answer;
      the *reason* is operator-facing only.
- [x] **1c · Suspension and capability paths executed.** — `d4aab37` A suspended
      member is refused though their token is valid; a capability token is
      confined to its allowlist and its space.
- [x] **1d · Grants pinned to the Stage-8 manifest.** — `d4aab37` Feed `catalog()`'s
      per-op manifests through `combined_manifest_hash` so a grant dies
      when the scope it consented to changes. This is the join that makes
      ops truth load-bearing rather than decorative.

## 2 · The playground catches up — **DONE**

Was: read-only, single anonymous caller, page one and stop. Now every
surface the platform has is something you can watch happen.

- [x] **2a · Writes and callers.** — `802af36` A caller selector drives
      reads AND writes; the write pane runs the real Stage-5 protocol
      against its own `ordo_playground` database (reseeded at boot, so a
      session never corrupts the suites beside it) and prints the command
      group it appended, before/after images included.
- [x] **2b · Manifest panel.** — `802af36` Renders cost, kind and mode,
      entities reached through composition, each guard with its
      provenance, and the bypass disclosure in red for `unguarded` ops.
- [x] **2c · Paging.** — `802af36` A page-size selector and a Next-page
      button that follows the cursor; the end of an order says so rather
      than going quiet.

## 3 · Documentation truth pass — **DONE**

Six stages landed since the docs were last reconciled; several say a
thing is unbuilt that now runs.

- [x] **3a · Reconcile the F-ledger.** — `1dc708c` Audited docs/ for
      statements the last six stages falsified, then corrected them:
      `engine-plan.md` (F5 partial-built marker, the crate map, and the
      "the interfaces cannot express this" passage — the seam was replaced,
      so two of AA-004's three legs now commit together and the owed third
      is named), `corpus-rulings.md` (the dated Stage-5 gap list and the
      venue-gate landmine, now live), `tutorial.md` (authorization is no
      longer "specified, planned, not implemented" — the chapter's
      rejections are rejections today), `ordo-system.drawio` (the entire
      build-status page described a pre-first-commit tree), and the AA-004
      ledger entry in `critique-consolidated-4.md`, marked partially closed
      with the surviving leg stated.
- [x] **3b · User-facing write docs.** — `2f31861`
      [`writing-data.md`](writing-data.md): derived modes, the two guard
      checks and why their order is load-bearing, creates proving their
      guard against a row that does not exist, the bootstrap, command
      groups, typed-id minting, atomicity stated exactly, closed error
      inventory.
- [x] **3c · Deployment story.** — `2f31861`
      [`deploying.md`](deploying.md): pool sizing as the concurrency bound,
      threads, transaction ownership, page size and cursor binding, two-level
      key custody and the `KeyProvider` contract, credentials and the
      one-answer rule, grants dying on scope change, the deploy-time gates.

## 4 · Adversarial review of everything since the last audit — **DONE**

Two independent reviewers, instructed to break it rather than admire it.
Four real defects; full account in
[`corpus-rulings.md`](corpus-rulings.md) under the 2026-08-15 sweep.

- [x] **4a · Guard/write soundness sweep.** — `a6125c0` Found two
      escalations, both by design rather than plumbing: re-pointing a ref
      walked rows across the authorization boundary (the guard was proven
      against the pre-image only), and `create_exempt` swallowed
      satisfiable disjuncts. Also found the create parent check keying off
      a string-shape heuristic instead of the declared type.
- [x] **4b · Paging and cursor sweep.** — `a6125c0` Found two proven row
      losses: a union's order is total only up to row identity (duplicates
      past the first were dropped), and a repeated sort key shifted every
      hidden key column, destroying the tiebreak. Clean: the seek
      predicate's NULL logic, type coercion across all eight scalar types,
      hidden-column containment, and guards surviving paging.
- [x] **4c · Fix what it finds.** — `a6125c0` All four fixed with a
      regression apiece, plus two honesty repairs: the harness's psql path
      no longer pretends to be transactional (separate processes made
      `BEGIN` a no-op), and `standard_conforming_strings` is pinned so
      escaping cannot depend on a server setting.

## 4d · The playground caught up with the language (2026-09-04) — **DONE**

Was: 1267 lines of source and one test, drifted behind six stages of
compiler change. Five defects, four of which changed rows silently on the
surface built to make translation visible; full account in
[`corpus-rulings.md`](corpus-rulings.md) under the 2026-09-04 sweep.

- [x] **4d-1 · Null logic and connectives in the reverse compiler.** SQL's
      three-valued `=`/`<>` decompiled to Ordo's two-valued `==`/`!=`
      (`nickname <> 'B'` — one row in Postgres, five in its decompilation),
      and `a AND b OR c` decompiled as `a and (b or c)` while `(a OR b) AND
      c` was refused outright. Both fixed: an `is not null` bridge emitted
      where the schema says the operand can be null, and a precedence-correct
      grammar with parenthesised factors.
- [x] **4d-2 · Silent drops became named refusals.** A renamed group key, a
      selected non-key column, and a snake_case `AS` alias each produced
      wrong output or unparseable Ordo without saying so.
- [x] **4d-3 · The paged path skipped `resolve_blind`.** A `sealed indexed`
      lookup ran unpaged and refused with `unresolved_blind_index` the
      moment a page size was set.
- [x] **4d-4 · The reverse pane now runs both sides.** It claimed a divergence
      would be "visible, not hidden" while showing only the decompilation's
      rows. It now prints both row sets and a `ROWS DIVERGE` verdict.
- [x] **4d-5 · Read panes stopped borrowing the suites' databases**, the
      pinned execution clock is disclosed on the page, and the reference
      card's stale claims (two-valued equality, no precedence table, 3+
      sources) are corrected.
- [x] **4d-6 · The handlers became testable.** 375 lines lived in `main.rs`,
      where no test could reach them; they are now
      `ordo_playground::server`. One test became 51 across three oracles —
      shape (no database), rows (real Postgres, on the queries the corpus
      does not contain), and the API's own answers.

## 4e · The reverse compiler's disclosed limitations, decided (2026-09-05) — **DONE**

Was: 4d closed five defects and filed two under *"recorded, not fixed"* —
an `INNER JOIN` becoming an outer-preserving dot-path (a different row set)
and `group` always projecting its keys (a different column set). Both were
disclosed in notes and returned different answers anyway, on the surface
whose whole purpose is making a translation visible. Both turned out to be
translatable. Full account in [`corpus-rulings.md`](corpus-rulings.md) under
the 2026-09-05 sweep.

The rule the file is now held to: **a decompilation may differ from its SQL
only in ways that cannot change which values come back.** Anything else is
translated faithfully or refused by name — a note is not a licence to return
different rows. **One standing exception, stated rather than buried:** the
text-collation pin (4e-5) *can* change which rows come back under a `LIMIT`
and stays a note anyway, because Ordo has no spelling for another collation
and refusing every text ordering would refuse a third of the corpus. That is
a cost judgement, not a case the rule does not cover.

- [x] **4e-1 · `INNER JOIN` is emitted as the filter it is.** The subset only
      accepts `fk = joined.id` against a declared ref, so the foreign key
      makes `fk is not null` *exactly* the rows the join drops — no
      duplication, no reordering. Folded into the WHERE as a conjunct so it
      renders through the same precedence-correct printer. An inner join on a
      required ref adds nothing.
- [x] **4e-2 · `group`'s keys stopped being forced into the projection.**
      Ordo has no post-`group` projection step, but `let` binds a pipeline and
      the outer source line projects a subset of it. `SELECT count(*) AS n …
      GROUP BY team` is now `let grouped = … group team { n: count }` +
      `grouped[n]`. The same mechanism fixes the column ORDER, which no
      row-multiset oracle could ever have seen.
- [x] **4e-3 · Null placement in an ordering.** Postgres sorts nulls FIRST
      under DESC; Ordo emits `NULLS LAST` always; and an explicit `NULLS
      FIRST`/`NULLS LAST` was parsed and discarded. `ORDER BY nickname DESC,
      id LIMIT 3` was three rows in each, none of them the same three. Bridged
      with a leading `cond(k is null -> 0, 1)` key.
- [x] **4e-4 · Four more places the emitter wrote text rather than meaning.**
      Two projected paths sharing a leaf name (a column silently lost —
      refused; `ordo-sema` does not catch it either, recorded); `SELECT *`
      (nineteen physical columns against twelve declared open fields —
      refused, with the explicit projection in the message); string literals
      emitted unescaped into a lexer with its own escape set (`'a\nb'` became
      a newline); temporal literals and column identifiers emitted verbatim.
- [x] **4e-5 · Two divergences that stay notes, and say why.** Ordo's
      `COLLATE "C"` pin against the database's collation (no Ordo spelling
      exists; a test asserts the two collations still disagree, so the note
      stays backed) and `avg`'s pinned scale (the same number, printed
      differently). Only the second is value-preserving; the first is the
      standing exception named above.
- [x] **4e-6 · The row oracle grew an ordering oracle and a refusal oracle.**
      `row_multiset` sorts both sides, so it cannot see an ordering defect —
      which is where 4e-3 hid. 46 statements for the row set (the count is
      asserted, so a case that starts being refused fails rather than
      vanishing), ten sequences for the order, and four refusals each run as
      the translation it replaced and required to disagree. Every fix was
      reverted in turn and every one failed something — but **not all of them
      fail a ROW oracle**: the collation, the backslash half of the escaping
      fix, and 4e-7's tiebreak are shape assertions, for reasons
      `corpus-rulings.md` states case by case under *Deliberately not
      proven*.
- [x] **4e-7 · Two the sweep itself got wrong, found on review.** Nobody had
      read the sweep — the agent that wrote it died before reviewing any of
      it. (a) A JOINED side's trailing `id` was being eaten as the implicit
      tiebreak (`col == "id"` with no look at the alias), so `ORDER BY a.name,
      a.id` dropped the assignee's id and claimed Ordo appends it — Ordo
      appends the *task's*. Under a LIMIT, different rows. (b) Ungrouped
      `count(col)` came back `null` where SQL says `0`: `sum cond(…)` is an
      exact spelling of `count` only where the input has rows, and Ordo's
      `reduce` braces admit aggregates only, so there is no `coalesce(sum …,
      0)` to bridge it with — refused.

**What is still not done here**, so it is not mistaken for finished:

- **There is no paginated decompilation.** `OFFSET` is now refused *by name*
  (it was coming back as "trailing tokens: `Num(2)`", having eaten `OFFSET`
  as the table's alias), and the message says why: Ordo pages with a cursor,
  which answers a different question from a skip count once rows are written
  between pages. Reaching Ordo's `page` from SQL is not attempted.
- **Ungrouped `count(col)` is now a refusal rather than a translation.** The
  bridge is a `coalesce` Ordo's aggregate braces will not take. Closing it
  needs a language change, not an emitter change.
- **`ordo-sema` still accepts a projection with two outputs of the same
  name**, silently returning one column where two were asked for. The
  decompiler refuses it; the forward compiler does not. Owed, and not this
  item's to fix.
- **The stage-two round-trip is 29 of 37.** The eight outside the subset are
  printed by the suite, never skipped silently.

## 4f · The many-branch feed: literal columns and `append` (2026-09-07) — **DONE**

Was: an owner pasted a `UNION ALL` feed with a literal discriminator and got
`expected an identifier, found Str("order")` — a Rust `Debug` of a token, on
the page whose job is making a translation visible. It named neither of the
two constructs it could not translate, and both had exact Ordo spellings.
Full account in [`corpus-rulings.md`](corpus-rulings.md) under the 2026-09-07
entry.

- [x] **4f-1 · A literal select column is a literal projection entry.**
      `'order' AS kind` → `kind: "order"`, through the same closed escape set
      the condition literals use — the one path where a literal the reader
      typed becomes a VALUE RETURNED rather than a filter. String, integer,
      decimal, boolean and `NULL` are supported because the forward compiler
      accepts each in a projection and each agrees on Postgres; a literal with
      no `AS` name is refused by name (SQL's `?column?` has no Ordo form),
      except in a later union arm where SQL takes the name from the first.
- [x] **4f-2 · `UNION ALL` is `append`, and plain `UNION` is `append …
      distinct`.** SQL names a union from the FIRST arm and matches the rest
      by POSITION; Ordo matches by NAME, so each later arm is emitted under
      the first arm's names — SQL's own rule, written in the language that
      only has the other one. Plain `UNION` was supported only after four
      differential cases proved the dedupe identical (within an arm, across
      arms, and on a key that is null on both sides); the collation caveat
      that makes it true only under a deterministic collation is disclosed.
- [x] **4f-3 · Where the union subset ends, refused by name.** A self-union
      (DL §5 — and the message says the rows would AGREE, because
      `ordo-sema` does not enforce the rule; it is a language refusal, not a
      divergence one, and a test pins that reading), per-arm ORDER BY / LIMIT
      / GROUP BY / aggregate / DISTINCT, `INTERSECT`, `EXCEPT`, mixed
      `UNION`/`UNION ALL` chains, arm width and type mismatches, and a
      qualified or unknown ORDER BY key after a union.
- [x] **4f-4 · The tokenizer-soup class, audited.** Six more well-formed-SQL
      routes into `expected an identifier, found <token>` besides the owner's
      literal: bind parameters (`$1`, `?`, `:name` — what every pasted
      application query contains), column ordinals, `t.*`, an unknown
      function, a `::` cast, and `CASE`. Each says what it is now.
- [x] **4f-5 · One the sweep found that nobody was looking for.** `WHERE x =
      NULL` was emitting `x == null` — the OPPOSITE predicate. SQL's is
      three-valued and matches nothing; Ordo's is `IS NOT DISTINCT FROM` and
      matches exactly the null rows. Zero rows against four on the shipped
      seed, in silence. Refused, and in the refusal oracle.

**What is still not done here:**

- **`ordo-sema` does not enforce DL §5's once-per-pipeline rule.**
  `orders[…] append orders[…]` compiles and runs. The decompiler refuses the
  shape; the forward compiler does not. Second entry on the same list as the
  duplicate-output-name gap above, and not this item's to fix.
- **Column ordinals are refused, not translated.** `ORDER BY 1` after a
  union is the portable spelling and the output names are known, so it
  *could* be resolved — it is refused by name instead, because a positional
  ordering is one more place for an off-by-one to change which rows a LIMIT
  keeps, and the refusal names the column to write instead.
- **`INTERSECT` / `EXCEPT` are not attempted.** Reconstructing which declared
  ref a `match any` / `match none` would walk is not something a two-SELECT
  set operation carries.

---

## Landed (for context)

- Playground · literal columns and `UNION ALL` → `append` — this sweep
- Playground · INNER JOIN and `group` translated, not disclosed — this sweep
- Playground · null logic, precedence, both-sides verification — `62cf870`
- Adversarial sweep · 2 escalations + 2 row losses fixed — `a6125c0`
- Playground · callers, writes, manifests, paging — `802af36`
- Real credentials · signed tokens + engine at the door — `d4aab37`
- Stage 1–2 · compiler + cost gate — `ordo` CLI checks and plans
- Stage 3 · authorization, visibility matrix executing — `0cd070c`
- Stage 5 · writes + durable command log — `8615236`
- R-G8 registration gates — `5e06f9b`
- F4 · AEAD sealing, sealed writes — `c3da6c7`
- Stage 7 · HTTP engine wired to the compiler — `1f8b181`
- Stage 6 · pooled tokio-postgres executor — `3fe60bd`
- AA-004 closed · single-transaction writes with row locks — `de25eda`
- Keyset pagination — `6f7df77`
- Stage 8 · ops truth (manifest + catalog) — `149ec88`

---

## 5 · The honest remainder

What the four completed tracks left standing, in the order the risk
argues for.

- [x] **5a · AA-004's third leg — CLOSED.** — `e460bd7` The seam changed
      rather than the ordering: `OpExecutor::execute` now receives a
      `ReplayLease`, and an executor that honours it reads the durable
      journal *inside* the transaction that will write the effect and
      records the key's completion in that same transaction. Commit
      publishes both or neither. `ExecOutcome::AlreadyApplied` is the
      truthful answer for a retry whose effect already landed, and the
      router turns it into a defined 409 rather than re-executing. The
      in-memory response cache survives as verbatim-replay convenience;
      it is no longer what makes the guarantee. AA-005 (fencing) is
      **not** closed by this and stays open below.
- [x] **5a-bis · AA-005 fencing — CLOSED.** — `eaabfa5` A claim takes a
      generation and a takeover bumps it; completion is conditional on the
      generation the holder claimed. A superseded holder cannot finish and
      finds out, so its transaction rolls back rather than committing work
      nobody can attribute under a key someone else now owns. The healthy
      path pays nothing.
- [x] **5b · Tree writes.** — `986520b` Path and depth derived from the
      parent chain on create; reparenting rewrites the whole subtree in one
      prefix substitution; cycles refused (including transitively, by
      prefix test); the depth cap applied to the deepest leaf a move would
      push down; deletes refuse with the child count; ranged reparents
      refused rather than half-applied. Nothing is refused by class any
      more. The test also exposed a real gap it did not set out to find:
      deleting a referenced row leaked a raw Postgres foreign-key error, so
      **refuse-with-referrers** (settled, §write rules 6) is now
      implemented in the platform's own vocabulary.
- [x] **5c · Sealed reads.** — `86a49af` Projection opens host-side under
      the envelope's own entity/field AAD; every other position still
      refuses, because the database cannot reason about what it cannot
      read. Obligations propagate through composition by output name, so a
      wrapped callee's sealed value never decays into served ciphertext.
      The venue gate went live with it — `unbounded-host` demands the
      acknowledgment however small the row bound, which is exactly the
      landmine ledgered when host classes were unreachable. Host-side
      FILTERING stays unbuilt: it would examine rows the caller never
      asked for, and that needs its own disclosure rule.
- [x] **5d · Triggers and the job queue.** — `e9b7b6a` Every cross-pod
      concern rides Postgres, no broker. **Fires-only-on-commit needed no
      machinery**: the enqueue happens inside the write transaction, so an
      aborted write un-enqueues itself and `NOTIFY` (delivered at commit,
      discarded on rollback) agrees by construction. The queue is `FOR
      UPDATE SKIP LOCKED`, so replicas compete without double delivery and
      a dead worker's claim returns to the queue. Clock triggers keep
      **at-most-one-holder** (advisory lock) and **fires-once** (the
      schedule row's own claim + advance) as the two separate properties
      the review said they were — a crash before firing leaves the work
      owed, a crash after leaves it advanced.
- [x] **5e · F6's core.** — `e061e3f` **Empty-shadow shape verification**:
      every registered op's planned SQL is described against a scratch
      schema built from the same DDL, and the compiler's inferred shape
      must match the columns and types Postgres reports or the load fails.
      Zero rows are read, so it is safe on every boot and hot swap. And
      the **catalog is scoped to the asker**: the same check the door runs,
      per op, omitting rather than marking — a listed-but-forbidden op
      still tells the asker it exists. A capability token's catalog is
      exactly its allowlist. Two renderings from one source (machine JSON
      and an LLM-ready document).

- [~] **5f · F6's operational remainder — mechanisms built, measurements
      blocked.** — `f294459`, `6aa16aa` The item bundled two kinds of thing and only
      one of them needs a deployment. **The mechanisms do not**: they are
      testable against the conformance cluster and are now built and tested
      there. **The three measurements do**, and two of them are blocked on
      more than a deployment — see below.

      **Built.** `ordo apply` / `rollback` / `deploys` / `prune`; the
      Postgres artifact registry; hot swap; pruning and checkpoint
      scheduling; and the separate attestation cadence. Four rulings became
      mechanisms rather than prose:

      *The registry's pointer is an append-only log.* `$artifacts` is
      content-addressed by program version hash and never deleted from;
      `$activations` records one row per re-point. A single-row pointer
      updated in place answers only *what is serving now*, and the question
      during an incident is *what was serving at the time*. **Rollback is
      then not a special operation** — it is an ordinary activation naming
      an older version, which is the same mechanism forwards, and it leaves
      a record saying a rollback happened. A foreign key makes "old
      artifacts are retained" a schema property rather than a promise.

      *Hot swap keeps the F6 load gate, by type.*
      `ordo_api::RegistryHandle::swap` took any `Arc<dyn OpRegistry>`, so
      the gate — enforced by type at boot, since `PlatformState::new` needs
      a `ShapeReport` and only verification produces one — could be walked
      around by booting verified and then swapping in something no gate had
      seen. `Deployment::swap` takes a `Verified` and nothing else, and its
      only constructors run the gate. The deeper defect was that the program
      lived in **three** places nobody moved together — the door's specs,
      `FixtureExecutor`'s fixture, and the executor's own schema and
      registry — so a swap of the first would have left the door dispatching
      new specs into an executor planning the old program, a half-swap whose
      halves are each individually valid. `Loaded` holds all of them plus
      the shape report behind one lock, and `Deployment` **is** the
      `OpRegistry`, so *"the catalog republishes atomically with the swap"*
      is a consequence of where the fields live.

      *The empty shadow cannot see the live database, and now something
      does.* Both sides of shape verification descend from the artifact's
      own DDL, so what 5e proves is that the **planner and Postgres** agree
      — never that the live schema is the one the program was compiled for.
      An artifact deployed ahead of its migration verifies perfectly and
      fails on the first request. `ordo_exec::shape::Pool::live_drift` asks
      the second question on the same shadow while it exists: the gate
      *observes* drift (published on `/health/deep`) and the deploy path
      *refuses* it, because a swap is the moment the deployment starts
      depending on the live schema being right. It is one-way on purpose —
      extra live tables and columns are not drift, or `migration.md`'s
      additive-then-swap sequence would refuse the state it is built around.

      *Attestation does not inherit `checkpoint`'s cadence.* Four documents
      state that ruling in almost the same words, which is a rule somebody
      keeps breaking, so it is built to be inexpressible: two schedule rows,
      two periods, two tables, two types. The table separation is the
      load-bearing one — a checkpoint is the **only legal prune anchor**,
      attestations arrive a thousand times more often, and one table holding
      both would put "anchor a prune at an attestation" one `WHERE` clause
      away. The job queue was the real hazard: `claim_due` advances *every*
      due schedule by *one* period, so two cadences through one call means
      the loser adopts the winner's — the ruling broken by machinery rather
      than by misreading. `claim_schedule` claims one named schedule.

      **Pruning and the client pin — the interaction, and the ruling.** A
      layer-1 pin (8q) is `(upto, head, key_id, signature)` kept by a client
      that verifies by recomputing *forward*. A prune deletes exactly the
      bytes such a recomputation needs. **The floor rule does not protect
      outstanding pins and cannot**: it never passes the slowest
      *registered* consumer, and a pin is deliberately not one —
      `bootstrap.md` promises layer 1 "asks nothing of anyone" and "requires
      no configuration at all", which means structurally that the deployment
      does not know a pin exists. Registering them would let one abandoned
      browser tab pin a space's history forever. So the loss is **disclosed,
      not prevented**, which is the posture the whole attestation design
      takes: every answered response carries `x-ordo-verifiable-from`, the
      chain's first retained index, and a returning client compares it with
      its own `upto` in one request rather than by fetching entries that are
      gone. It is **outside the signature**, and that is the ruling rather
      than an omission: the pin's signature stays `Checkpoint::signed_bytes`
      so one routine checks a pin and a stored checkpoint, and an unsigned
      floor cannot make a client *accept* anything — too low and it fetches
      entries that are not there, too high and it gives up early, which a
      deployment could achieve by serving nothing. It is an instruction
      about feasibility. The evidence for a lawful cut is the signed
      checkpoint at that position, which the prune requires by construction.
      Pruning also runs `ordo_crypto::sealed::collect_pruned` **in the
      delete's own transaction** — the collection is an anti-join against
      `$commandlog`, so a delete that committed without it would leave
      payloads nothing can name *and* nothing can ever find again.

      **Two defects the build found.** (i) **A prune could empty the
      chain.** A checkpoint at the head becomes a legal cut point as soon as
      every entry is older than the undo horizon — the ordinary state of a
      quiet space at ninety days — and cutting there took the whole table.
      An empty chain is then read two different wrong ways: from genesis
      `verify` answers *Ok, vacuous*, because that is what a space nobody
      ever wrote to answers; from its own checkpoint it answers **Tamper**,
      because nothing is at or after the anchor. A lawful prune reporting
      both of the two things it is not — and `verifiable_from` would have
      said `0`, "recompute from genesis", about a chain retaining nothing.
      A prune now never removes the last entry, as a fourth bound in the
      floor (`FloorHolder::RetainedTail`) rather than a clamp inside the
      delete. (ii) **An ambiguous `ORDER BY` would have silently disabled
      layer-1 pinning.** Reading both chain endpoints in one round trip put
      an `idx` column in the projection beside the table's own; Postgres
      refused the statement, and `PgAttestation` answers `None` on any query
      failure *by design*, so every response would have read as "this
      deployment does not attest" rather than as an error. Caught by the
      test, not by the compiler.

      **Not built, and why.** *Archive delivery*: no sink exists to point
      at, so `policy commandlog`'s archive destination has no home. Pruning
      still models an archive **consumer**, so a deployment that registers
      one pins the floor and never prunes past it — the correct behaviour
      for a sink that cannot confirm it holds the bytes. *Layer-2
      attestation (the webhook)*: an outbound POST from inside the
      deployment's perimeter, which `security-logging-design.md` §7b pins
      six ways (https only, resolve-then-pin re-checked at connect, no
      redirects ever, per-sink signing secret, an egress identity with no
      `KeyProvider` reachability, a consented per-space allowlist).
      Shipping the POST before those pins exist is the SSRF-into-key-custody
      path §7b was written to close, so the *record* ships and the delivery
      is owed. **Since resolved (2026-09-08, item 11-12): the pins shipped
      first, as `ordo-egress`, and the delivery on top of them** — which is
      the standing "egress ships as the first increment of its first
      consumer" rule doing what it was written to do, with attestation as
      that consumer. *`policy commandlog { … }` parsing*: a declaration form, not
      this item; `LogPolicy` carries the spec's own defaults and the prune
      report prints them, so an operator can see the horizon that produced
      a floor rather than assume theirs was read. *Consumer registration as
      an operator surface*: mirrors, listeners and archive sinks register
      themselves, and none of those subsystems exists, so a command today
      would manage an empty table.

      **The three measurements: two now blocked on a deployment only.**
      A number from this laptop's conformance cluster is **not** the release
      gate and must never be recorded as one — the gate is a number from a
      deployment under a load shape somebody committed to. No harness that
      produces one is included here, deliberately. Beyond that:
      - *Single-space write throughput under the chain-append lock* — the
        **instrumentation is now built**; the measurement is blocked on a
        deployment and on nothing else. `ordo-host`'s registry holds
        histograms beside its counters, `Metrics::quantile` computes what
        Prometheus's `histogram_quantile` computes (the same algorithm, so a
        number in a test and a number on a dashboard are one number), and
        `TxConn::append_group` times the `pg_advisory_xact_lock` statement
        **alone** — the wait, not the append. `ordo-exec` reports it through
        a one-method trait rather than depending on `ordo-host`, the same
        decorator seam `MeteredKeys` takes.
        **It is instrumented as a pair.** `ordo_chain_append_seconds` measures
        the critical section, because a wait is only interpretable against how
        long the section is: a wait that grew while the section stayed flat is
        more writers, and a wait that grew because the section grew is
        `implementation.md`'s *"anything that widens the critical section"*,
        which was previously something only code review could catch.
        **Proved against a real contended lock** (`ordo-host/tests/lock_wait.rs`):
        one transaction holds, a second blocks, the server's own `pg_locks`
        confirms an ungranted advisory lock, and the histogram accounts for
        the wait — with the uncontended population asserted to stay three
        decades below it, so the instrument can distinguish rather than merely
        record. An unobserved pool is **silent, not zero**, because a lock-wait
        series stuck at zero reads as *"never contended"*, which is the wrong
        conclusion drawn confidently.
        A laptop run is **not the gate and is not recorded as one**, and no
        harness that produces one ships here. What the pair did establish, on
        this machine, is that it works as a diagnosis: single-space append rate
        was flat across 1→16 concurrent writers while the median wait grew
        roughly linearly with writer count and the critical section stayed
        invariant — the textbook signature of a single serialization point,
        which is the sentence the counters could never have written.
        **The pair's first use as an argument, and it corrected the
        argument** (2026-09-06, `plan-idempotency.md` §3.2). The idempotency
        audit rested on the `$replay` journal's `complete_lease` UPDATE
        being *"small, bounded"* inside the critical section. Measured
        (`lock_wait.rs`, the journal test; `-- --nocapture` prints it):
        wait ~80us, section ~300us, the UPDATE ~85us — about **29% of the
        section**, which is not small. It costs nothing anyway, because it
        is **not in the section at all**: the lock is transaction-scoped
        and taken as late as possible, so the completion record is already
        written when the append acquires it. The test proves that from
        `pg_locks` — no advisory lock held while the UPDATE runs, one held
        the instant the append returns — rather than from reading the call
        order, since the call order is what a later edit changes without
        noticing. The 29% stays printed as the counterfactual: what a
        statement of that size would cost if it were placed after the
        append, which is how every widening starts.
      - *p99 op latency* — the same: `ordo_op_duration_seconds{op,outcome}`
        exists, recorded by a `MeteredExecutor` decorator so `ordo-api`'s
        five-trait boundary stays five traits. Its doc states what the span
        does **not** cover (token verification, the access decision, param
        binding, refinement validation, serialization), because a p99 read as
        end-to-end when it is not is a wrong number rather than a partial one.
        `outcome` is the refusal's **wire class**, never its message: the class
        inventory is closed, so the label set is bounded by construction.
      - *Restore wall time* — blocked on a deployment **and on the restore
        mechanism itself**. `implementation.md`'s drill asserts "exactly one
        discontinuity marker", but the `system:restore` group that produces
        one is unbuilt and `verify` has no concept of it, so the drill's
        step 2 cannot run and step 5 has nothing to publish. Step 3 —
        re-running shape verification against the restored schema — is the
        one part of the drill this item made executable, via `live_drift`.
- [~] **5g · F7 client-first vault — PLANNED.** Build plan in
      [`f7-vault-plan.md`](f7-vault-plan.md): nine phases, each with a
      done-when clause, 26–36 agent sessions plus 8–12 of prerequisites
      (Postgres `Store`, Ed25519 `Signer`, items 6a–6f, the embedded
      engine). Phase 4 — the deterministic evaluator — is a third of the
      work and carries all the schedule risk. Twelve open protocol
      questions each carry a recommendation, and **Q7 is a defect in the
      spec**: snapshot `state_digest` is defined over sealed bytes, but
      seals use a fresh random nonce, so two devices with identical state
      produce different digests — the field cannot do the job it exists
      for. Not started.
- [x] **5g-pre · The client runtime and its local store. Filed 2026-09-12,
      because 5g cannot exist without it and nothing tracked it.**
      `f7-vault-plan.md` §6 names "the embedded/WASM engine" among F7's
      prerequisites and then says "Not counted below. Call it another 8 to
      12 engineer-weeks." The **local store** is named only in the body
      (state "persisted to OPFS/IndexedDB, sealed at rest under the device
      key") and appears in no phase, no estimate and, until now, no row.

      **Nothing in the tree is a client.** `grep` for `indexeddb`, `opfs`
      or `localstorage` across every crate returns nothing.
      `ordo-wasm` compiles the *compiler* for the browser and
      `ordo-playground` is a surface for writing pipelines; neither holds a
      replica of a space's data.

      **What this blocks, beyond 5g.** Every product claim that depends on
      a device holding data: offline reads, the client-first vault, and
      revocable local caching (**5g-rev** below). A claim about what a
      device does is a claim about software somebody wrote, and today
      nobody wrote it.

      **Ordered before 5g, ruled 2026-09-12:** "cache system needs to be
      built before f7 as it will affect how f7 functions." The cache is
      not a subset of the vault, it is the substrate the vault's own
      choices sit on, so building F7 first would fix those choices against
      a store that did not exist yet. It is also much the smaller piece: a
      client that holds a sealed replica and reads from it needs none of
      Phase 4's deterministic evaluator, which is a third of F7 and
      carries all of its schedule risk.

      **Planned 2026-09-14 in [`plan-client-cache.md`](plan-client-cache.md).**
      Found while planning: every read response already carries the space's
      chain position, and the SDK ignores it; the position is read after the
      query rather than inside it, so it is an upper bound, and fixing that
      (C1) is what lets a cache refuse to show an older answer. Three
      questions for the owner, on the decisions doc.

      **Ruled 2026-09-14: replication, like Jazz, and IndexedDB caching on
      by default.** The plan is being revised to a replica of rows, which
      needs a change feed (20d), a device engine, and a feed scoped to each
      principal so a guard change can hide rows a device already holds.

      **Plan revised 2026-09-14 to the ruling.** The design documents
      already specify most of the mirror (`implementation.md` client
      storage rules and log-positioned reads; `definition-language.md` live
      queries). Two questions for the owner: what a device may receive
      (recommended: op-scoped sync, with the whole-space feed as its special
      case) and how it answers new questions in v1 (recommended: local
      refinement now, PGlite with F7).

      **C1 built 2026-09-14.** A read's chain position is now taken in the
      same snapshot as its rows (`OpClass::SnapshotRead`, repeatable read,
      read-only), and the door signs the head at that position. Before
      this the position was read after the query, an upper bound on the
      rows' version, so the layer-1 pin could claim a head the rows
      predated.

      **All three replica questions ruled 2026-09-14, on the
      recommendations:** op-scoped sync (the server sends each synced op's
      guarded row changes; a whole-space feed is the special case for
      unguarded reads), local refinement now with PGlite arriving with F7,
      and a device-local at-rest key shaped for 5g-rev. `bootstrap.md` input
      33 and `definition-language.md`'s mirror sentence amended to match.

      **C2 built 2026-09-14.** `@ordo/sdk` reads the position and signed head
      off every response (`OpResponse.pin`). Next is C3, the device store.

      **C3 built 2026-09-14.** `LocalReplica` in `@ordo/sdk`: per space and
      principal, sealed at rest under a non-extractable device key, rows
      keyed by entity and id with positions, a lower position never
      replacing a higher one. IndexedDB and memory backends. Next is C4,
      the delta feed, which gets its own design pass first.

      **C4 design pass written 2026-09-14** (`plan-replica-feed.md`). The
      manifest already says which results are stored rows of one entity
      (`result.anchor`, `grouped`), so syncability comes from what exists.
      The feed is a sync mode on the live stream: a full first frame, then
      upserts and deletes per invalidation, each at its position. Three
      questions for the owner: the server's state model, whether a synced
      op must project its id, and the size cap.

      **Ruled 2026-09-15:** the subscription remembers what it sent and a
      reconnect reloads; a synced op must return its id; no size cap, large
      loads arrive in paced pages with honest progress. Q-4, resuming without
      a reload, is on the board with a recommendation to leave it for later.

      **C4a built 2026-09-15.** `ordo_plan::artifact::syncability` decides
      from the bound plan; the artifact and `interface.json` carry it as
      `result.sync`, and the door's `OpSpec.sync` holds it through one
      crossing in `ordo_host::syncability`. A refusal names its rule. Every
      read op re-versions once, because `result.sync` is in the contract.
      Building it found that Q-2 conflicts with `ui-gen.md` §1 (the anchor's
      id rides every list row even when unprojected); that is Q-5, on the
      board.

      **C4b built 2026-09-15** (`ordo-api/src/sync.rs`). `?live=1&sync=1`
      sends `load` frames page by page with progress and a 50 ms wait
      between pages (not yet measured), then `change` frames of upserts and
      deletes when a bus signal leaves the result different. Queued signals
      are answered by one pass. On real Postgres, removing a membership
      reaches a synced device as deletes of the rows it hid. Next is C4c,
      the SDK consuming the frames into the device replica.

      **C4c built 2026-09-15** (`@ordo/sdk`, `src/replica/sync.ts`).
      `syncOp` applies `load` and `change` frames to `LocalReplica`, which
      now records which synced ops hold each row and removes a row only when
      none does. A completed load releases what the op held and the load
      left out, so a reconnect catches deletes made while the device was
      away. The generated op table says which ops can be synced. C4 is done;
      next is C5, the sync manager that keeps a device's chosen ops in sync
      by default and reconnects.

      **C5 built 2026-09-15.** `SyncManager` in `@ordo/sdk` holds the ops a
      device syncs, keeps each one connected and reopens a feed that ends. It
      publishes one view whose position is the lowest any live op has reached,
      so a screen never mixes one op's newer rows with another's older ones,
      and that position never decreases. A refusal the door names stops that
      op rather than being asked again, while `internal` and a serialization
      conflict back off and reconnect. Dropping an op releases what only it
      held. Building it found that this package decoded refusals from a
      `class` field the door has never written, so every refusal it ever
      received read as `internal`; the door's own `code` is now what it reads.
      Next is C6, local refinement over the synced rows.

      **C6 built 2026-09-15.** `refineOp` in `@ordo/sdk` filters, picks and
      sorts the rows a synced op holds, with the Tier-0 operators the
      refinement envelope already defines, so a filter keystroke asks the
      server for nothing. Values compare by the type the executor gave them,
      so money and ints compare as numbers rather than as text. The generated
      op table now carries `refine: { tier, fields }`. Building it found that
      Tier-0 refinement is specified, validated, cost-gated and served by
      nothing: every op ships `tier: "none"`, and `ordo-host`'s executor takes
      a validated refinement and ignores it, as its own test records. So this
      is the first working Tier 0 in the system and it runs on the device,
      which is why `refine-tier-local` is on the board. Next is C7, purge and
      revocation.

      **Ruled 2026-09-16, three answers.** `refine-tier-local`: "Take
      suggestion", so C6 stands as built (the device reads `refine.fields`,
      not `refine.tier`), and server-side Tier 0 is scheduled with the
      refinement feature rather than left implicit. `feed-resume`: "Can it
      share a diff hash or version hash and if they disagree on reconnect
      reconcile. Add it to C4 work." That reverses the recommendation to
      defer resume, and it is the cheaper answer, because the subscription
      already keeps sixteen bytes of SHA-256 per row; the device returns the
      digests it was given rather than computing its own, so no canonical
      serialization has to be invented. Filed as **C4d** in
      `plan-replica-feed.md`, and owed before 5g-pre is done. `uigen`: 14c
      moves to the post-deploy roadmap as 18f.

      **C7's device half built 2026-09-16.** `LocalReplica.poison()` purges
      and latches: a poisoned replica serves nothing and stores nothing. A
      test caught what the design missed — gating reads alone left it sealing
      new records to disk under keys held in memory, so the latch covers the
      writes too. A refusal in `access_denied`, `access_guard` or
      `delegation_required` releases what that op held; `not_found` and
      `unknown_op` deliberately do not, or an op renamed in a deploy would
      discard rows the caller still owns. `syncOp` refuses `std.space.*`
      before the wire, because the permission map is syncable by shape and a
      stolen device must not yield it.

      **What C7 does not build, and why.** `policy mirror` scope and
      eviction. `policy` is lexed and never parsed: no parser arm, no AST
      node, no consumer. That holds for every policy block the language
      documents (`spaces`, `commandlog`, `vault`, `llm-egress`, `retention`,
      `compliance`), so mirror scope would be the first policy Ordo ever
      parses — a language increment, not a device one. On the board as
      `policy-grammar`. The server-released key and leases stay 5g-rev.

      **C4d built 2026-09-16.** A reconnect says what it holds, and an
      unchanged result sends no rows. The completing `load` frame carries
      `result`: sixteen bytes folded from the per-row digests the
      subscription already keeps, sorted by id and length-prefixed so it
      names a set rather than an order. The device stores it against the op
      AND its version, returns it in `x-ordo-resume`, and the server pages
      the result without sending it — equal means one empty completing frame
      marked `resumed`, unequal means an ordinary paced load.

      **Two things worth remembering.** The plan's design was revised before
      building: the first sketch had the device send `(id, digest)` pairs,
      but the feed opens with GET so thousands of pairs cannot ride it, and
      the device had no digests to send anyway because the frames carry rows.
      The owner's words were narrower and are what got built — one hash. And
      a resume is a completing load with no rows, which is exactly the shape
      C4c releases everything on, so `resumed: true` exists to stop the
      mechanism meant to spare the device from wiping it. The honest cost: a
      matching resume is one read and no rows; a mismatching one is two
      reads, because the scan discards rows rather than buffering a result.

      **C4e built 2026-09-17, in two pushes.** The wire first, then the
      document, so nothing declared a column before it was delivered.
      `project_row_key` puts the anchor's id into the projection when the
      author did not, under the reserved `$id`, and all four sites that plan
      a read call it: both executor paths, the artifact, and the host's shape
      gate. It PROJECTS the id rather than reading a hidden cursor cell on the
      way past, which is what makes the paged path, the unpaged path, the SQL,
      the manifest and the shape gate agree with no special cases.

      **55 of 165 ops are syncable, up from 32**, and exactly 23 versions
      moved: a per-op version hashes that op's own manifest, and the other 142
      are unchanged byte for byte. Every hand-written list is now cacheable on
      a device; before this they were locked out because nobody typed `id`.

      **Three things the build found.** `syncability` and the attach had to
      share one decision, or the descriptor could name a different key than
      the wire sent, which nothing would detect until a device filed a row
      under a key the server never sends. `row_identity` was computed
      separately from `sync`, so the document claimed identity for `distinct`
      results and many-side joins that `sync` refused by name; it now derives
      from `sync`. And a wrapped callee that never projected its own id
      carries it out as `$tb0`, without which "any anchored, ungrouped read"
      was not actually delivered.

      **Ruled 2026-09-16 (`feed-id-uigen`), after the review the owner asked
      for.** The synced id follows `ui-gen.md` §1: any anchored, ungrouped
      read can be synced and the anchor's id rides every row. This supersedes
      the `feed-id` ruling of 2026-09-15, whose premise was false in code —
      it held that the id could be withheld because the page cursor carries
      it "inside an authenticated token the client cannot read", and the
      cursor is authenticated, not encrypted (`base64url(JSON) .
      base64url(HMAC)`), so every caller already reads the last row's id on
      every page. `id` is reserved and implicit, never sealable, and the
      tiebreak of every row plan, so no caller is entitled to a row but not
      its id. Filed as **C4e**, and it is not the one clause the switch was
      recommended as: the id has no channel today, because `canonical()`
      builds rows from `plan.outputs` alone, so the attach point is in the
      executor and the change crosses `ordo-plan`, `ordo-exec` and
      `ordo-api`, re-versions every read op, and moves 23 of the market
      golden's ops out of `id_not_projected`. Two amendments: the predicate
      names `distinct` and many-side source joins as refusals instead of
      catching them by accident, and the id is attached to every paged read
      row rather than only to feed frames, or a feed would send what an
      ordinary call does not.

      **Defect found by that review, and not caused by the switch.**
      `syncability` never inspects `source_joins`, so an op that fans out
      across a many-side join is syncable **today** whenever it projects its
      id — the anchor stays `q.from` while the rows are the far entity's, so
      several rows share one anchor id, and the subscription keys held rows
      by that id, so siblings overwrite each other. Owed as a fix in its own
      right, ahead of C4e.

- [ ] **5g-rev · Revocable local caching. Owner-ruled 2026-09-12:**
      "lets build that in", with "windows are safer use of sharing".

      **The problem.** A guest's device keeps what it already fetched.
      `definition-language.md` states it honestly: revocation "stops
      access, not memory (a guest's mirror may hold what it already
      fetched)". A session token does not help, because it gates
      *fetching* and reaches nothing already on disk.

      **The shape, in three layers, and only the third binds.** Push and
      purge for an honest online client, which G16 already delivers free
      since an acl write invalidates every guarded live query touching it.
      A lease with an expiry for an honest offline client, whose window
      **is** the revocation latency. And local sealing with a
      server-released key for a client nobody controls: the device holds
      ciphertext, unwrapping needs a release the platform grants only
      while the window is open, and revocation is simply no longer
      releasing.

      **The server half is buildable now and is worth building alone**:
      a content-key release for a shared slice, guard-evaluated at
      **release** time rather than at fetch time, with a bounded lease,
      plus the grace window as deployment policy rather than a client's
      choice. Key release is already per space, per purpose, per
      correlation, and already refuses a lease with no live request behind
      it.

      **Build order, ruled 2026-09-12:** the client half waits until local
      caching is settled and built (**5g-pre**), and then this follows. It
      carries a property worth stating before anyone promises it: **this
      is only as true as the client.** A third-party app can keep
      plaintext in `localStorage` and no key release reaches it. The
      platform makes the guarantee possible; the client Ordo ships makes
      it real, which is one more reason the cache is a platform
      component rather than an integration convenience.

      **Windows only.** A window leaves nothing in the guest's space, so
      the only residue is on their device, which is what this addresses. A
      feed puts the whole slice durably in another tenant's database,
      where no key release, no grace window and no client ever reaches it.

- [x] **5h · Authentication providers — the step-up gate has a key.**
      Was `audit-board-coverage.md` finding #5, which had no board row:
      the trait existed, `LocalPasswordProvider` hashed with
      `PlainTestHasher`, and **there was no passkey provider at all**.
      Since passkey is the sole step-up source, `StepUpRequirement` and
      `LinkCeremony` were gates with no key — every step-up op and every
      second-factor link denied for every caller in any real deployment.
      The hole was one level deeper than "no provider": `AuthenticatedSubject`
      had **no field for presence**, so even a passkey provider could not
      have carried it. Now:
      **`PasskeyProvider` + in-house WebAuthn verification.** CTAP2 CBOR,
      COSE keys, `authenticatorData`, `clientDataJSON`, ES256 and EdDSA
      (`ordo-identity/src/webauthn.rs`, `passkey.rs`). Challenge binding,
      RP-id binding, origin binding, the UP flag, the signature, and the
      sign-count clone signal. `webauthn-rs` was **not** taken and the
      reason is recorded in `security-logging-design.md` §2.1: 223
      transitive crates and native OpenSSL for attestation formats this
      deployment deliberately does not verify (`fmt: none` only).
      **Presence is a type.** `UserPresence` has private fields and no
      constructor outside `passkey.rs`, so the design's "presence comes only
      from here" and "presence is not delegable" are enforced by the module
      boundary rather than by a check somebody has to remember. It walks
      `complete → AuthenticatedSubject::presence → SignIn::token_spec →
      TokenSpec → TokenClaims → StepUpRequirement`; every link is a type.
      **`session.rs` is the join that was missing.** `sign_in` resolves a
      ceremony to a principal and carries its stamp; `refresh_presence` is
      the step-up prompt, and it carries the obligation nothing upstream
      can: *the asserting authenticator must be linked to this session's
      principal.* An attacker with a stolen bearer token can always run a
      valid ceremony with their own passkey — the assertion verifies, the
      presence is real, and it is somebody else's. Tested as an attack.
      **argon2id, pinned.** `m=19456, t=2, p=1`, OWASP's server profile, in
      `provider.rs` where a reader finds it, asserted by a test that reads
      the PHC string. `PlainTestHasher` is now `#[cfg(test)]` — it was a
      public export, so a deployment could have wired the password path to a
      function that stores the password.
      **Two things found that nobody was looking for.** (1) Provider
      ceremony handles had **no expiry at all** — the pending challenge and
      OIDC `state` maps grew for the life of the process and a `state`
      observed once stayed redeemable forever. One-shot was enforced,
      one-*time* was not. `AuthProvider` now takes the server's clock and
      every handle expires at 300s, expired reading as unknown. (2) The COSE
      parser's first form decided what label `-1` meant before reading
      `alg`, which is `crv` for EC2/OKP and the RSA *modulus* for RSA — an
      RS256 key came back "malformed CBOR" instead of "algorithm not
      offered". Two-pass now.
      93 tests in `ordo-identity`; the end-to-end one drives ceremony →
      subject → directory → token → `verify` → `AccessEngine::check` with
      nothing stubbed between the signature and the door, and its control
      runs the identical plumbing with a password and gets
      `StepUpRequired`. **Left to the deployment, deliberately:** the
      browser ceremony itself (no test fakes a user gesture), the HTTP
      login/enrollment endpoints, reading `ORDO_APEX_DOMAIN` at boot, and
      the Postgres-backed credential store. Two of those four are closed:
      the endpoints by **20b** on 2026-09-13, and the credential store by
      **20b** on 2026-09-20 (`$credential` behind
      `ordo_identity::credentials::CredentialStore`). `ORDO_APEX_DOMAIN`
      and the browser ceremony stay open and belong together, since a
      relying-party id is only load bearing once a passkey endpoint
      exists.

---

## 6 · P5's unbuilt write rules

The write path has modes, guards, atomicity, and the log. It does **not**
have the rest of the settled rule set from the definition language's §6
write rules — found by auditing the build plan rather than the tracking
list, which had drifted from it.

- [x] **6a · `rev` as compare-and-set.** — `a01bf0a` `rev` in a write's
      braces is a belief about the row, split out before anything reads
      them as assignments: `rev: $r` converges if the row is still that
      version and answers `cas_miss` naming BOTH versions otherwise, so a
      retry knows how far behind it is. Checked after the guards and under
      the same row lock — authorization precedes version disclosure. `id`
      with `rev: null` is a compile error (asserting the absence of a row
      you hold the id of), and `rev` in a ranged write is refused because
      there is no single version to believe in. `rev: null` as create-once
      on a KEYED write belongs with 6b, which owns unique-key addressing.
- [x] **6b · Converge-or-create.** — `49cee6b` The one blessed upsert,
      compiled to the pinned `INSERT … ON CONFLICT` so the branch is the
      database's decision against the unique index, not ours against a
      stale `SELECT`. The INSERT half carries the whole row; the
      `DO UPDATE` half only the described fields (rule 10). A sealed key
      addresses through its blind index. The two branches ask different
      authorization questions, so the branch is decided first — under
      `FOR UPDATE` on the row the key names — and exactly one guard runs:
      the create gate against the would-be row, or the write guard against
      the row as stored; the window `ON CONFLICT` leaves is covered by a
      post-image obligation located by the KEY. Rule 9's keyed half rides
      here: `rev: null` is create-once (`expected_absent` on a match, and
      the printed mode becomes `creates`), `rev: $r` extends `check_cas`
      rather than duplicating it, and a null key is `null_key_address`.
      Two or more covered unique keys is `ambiguous_identity`.
- [x] **6c · Identity precedence (rule 8).** — `49cee6b` `id` in braces
      addresses, always; other unique fields ride as values, and one that
      already identifies another row is `conflicting_identity` rather than
      a leaked unique violation. No resurrection is unchanged (absent id →
      `not_found`), `id` with `rev: null` is refused in the braces
      spelling too, and `id == null` no longer demotes an addressed write
      to a ranged one that answers `updated: 0`.
- [x] **6d · Insert-completeness (rule 2).** A creation-capable statement
      must supply every required field, checked at compile time rather
      than discovered as a NOT NULL violation. Both this and 6e live in
      `ordo-plan`'s `writes::check_writes`, run from `check_registration`
      beside the read gates: a required field with no value and no default
      is a fact about the statement, so it is answered where statements
      are read, class `insert_incomplete`, naming every missing field.
      Defaulted, optional, and implicit fields are covered without being
      written; `id` in the braces addresses rather than creates, so that
      statement is exempt (rule 8's precedence, honoured early).
- [x] **6e · Bulk-brace restriction (rule 3).** Identity and unique-key
      fields may not appear in bulk braces — addressed and bulk must not
      blur. Class `bulk_identity_field`; a `sealed indexed unique` field
      counts, since the claim rides the blind index. The compile-time mode
      discriminator mirrors the executor's: a bare `id == <literal or
      $param>` is addressed, every other condition ranges.
- [x] **6f · `del entity(true)` and `del … subtree` (rules 5, 6).** —
      `fc84e8a` Bare `del entity` no longer parses: a delete states a
      condition, an addressed `{ id: $id }`, or the explicit `(true)`.
      `del <tree> { id: $id } subtree` is the cascade, and it emits one
      command per deleted row, deepest first (the `depth` column IS the
      foreign-key-safe order), each with its before image, so a revert can
      put the subtree back. `subtree` is a flag on `Del` rather than a
      variant, so consumers that ask "is this a delete?" keep answering
      yes and a forgotten flag under-deletes instead of over-deleting.
      Guards run over every row and the cascade is **all or nothing** —
      one descendant the caller may not delete refuses the whole
      statement, since a partial cascade orphans the rest; bulk's G11
      narrowing is deliberately not applied. Referrers from outside the
      subtree still block it.
- [x] **6g · The computed right-hand side (17f).** DL §6, "RHS field
      references": *"`{ n: n + 1 }` — computed server-side inside the row's
      transaction; atomic increments without read-modify-write races"*, and
      DL §3's list of where expressions are legal, which names *"write RHS
      (per-row values in bulk writes and rules)"*. Before this, a write's
      value was a literal, a param or `$caller`, and DL's own flagship
      `set teams { id: $t, used: used + 1 }` did not compile — which killed
      the quota, counter and append-to-string patterns the language
      documents.

      **One expression language, one renderer.** The write path gets
      expressions by going through the READ path's binder
      (`ordo_sema::write_value_expr` → `Binder::expr`, so `check_arg_types`,
      `binary_ty`/`fn_ty`, the division rule, `allocate`'s one legal home
      and the sealed refusal all arrive unchanged) and the READ path's
      renderer (`ordo_plan::render_write_value` → `Planner::expr`). Nothing
      about what `+` means over text, or what `/` does to avoid truncating
      two `bigint`s, is decided twice. `computed_rhs.rs` asserts that as an
      output rather than an intention: the write's fragment is exactly
      `("teams"."used" + 1)`, and the plan for the same expression read as a
      projection contains it verbatim modulo the alias.

      **The rule is WHEN the value exists**, not literal-versus-expression,
      because the write protocol does a great deal with a value before the
      row lands — proves a create's guard against the would-be row, seals
      it, digests it for a blind index, addresses a unique key with it,
      checks it against a closed ladder, images it. So:

      - **row-free** (no field reference) — legal in every mode. The
        **database** folds it, in one `SELECT` on the write's own connection
        inside the transaction, and the statement is rewritten to carry the
        answer, exactly as `substitute_bound` rewrites a `let` descriptor.
        From there it is an ordinary value and every one of those checks
        works. Folding host-side would have been a second implementation of
        `+`-over-money, of `/`'s scale and of `round`'s tie rule, agreeing
        until the day it did not.
      - **row-reading** (`used + 1`) — rendered into the statement's own
        `SET`, so the read of the current value and the write of the new one
        are one `UPDATE` under the row lock the path already took. Legal
        only where there IS a row: refused by name in every statement whose
        label can create, **converge-or-create included** — its branch is
        the database's decision against a unique index, so a field reference
        would mean one thing on one branch and nothing on the other. Also
        refused in the five field kinds whose value the platform must hold
        first (sealed, unique-keyed, ref, closed-set, content), each named
        with its reason.

      Refused twice — at registration
      (`ordo_plan::writes::check_computed_rhs`, so the op does not deploy)
      and at plan time (the harness can plan an unregistered statement) —
      both reading the same `stmt_label`, so they cannot disagree about
      which statements have a row. Dot-paths stay refused (DL §11 declines
      inter-row computation, and an `UPDATE` has nowhere to put the join); a
      misspelled column is `projection_unknown_field` by GC088's positional
      rule, never the binder's internal `unknown_field`.

      Fifteen executed cases in `ordo-conformance/tests/computed_rhs.rs`
      (`ordo_rhs_*`) plus the concurrency half in
      `ordo-exec/tests/computed_rhs.rs`: **six concurrent `claimSeat` calls
      in six transactions land as exactly `+6`, and the seventh is refused
      by the CHECK** — a read-modify-write implementation loses at least one
      and leaves room for the oversell. `/ 0` from a value in the row is
      `arithmetic_error` with nothing applied; goldens unmoved.

      **HAZARD found while writing that test, and not this feature's.**
      Concurrent writes to ONE row that share an `Arc<Mutex<WriteLog>>`
      **deadlock**. `Executor::write_attempt` takes that `std::sync::Mutex`
      inside its `spawn_blocking` and holds it across the whole protocol —
      the `SELECT … FOR UPDATE` included — while also taking it on the async
      side, where the continuation that ends a transaction and releases its
      row locks lives. Lock order is log-mutex → row-lock in one place and
      the reverse in the other; six writers never return. Proved
      independent of 17f by a control: the identical hang reproduces with
      six plain `set teams { id: $t, used: $n }` converges, no expression
      anywhere. **`ordo_host::FixtureExecutor` shares exactly one such log
      across requests** ("Shared across requests: the command chain is per
      space"), so this is reachable at the served door under ordinary
      contention. Left as an executable `#[ignore]`d reproduction
      (`a_shared_write_log_deadlocks_concurrent_writes_to_one_row`) rather
      than a rushed fix, because the fix is a decision about where the
      in-memory chain sits relative to the transaction, not a lock tweak.
      **Fixed 2026-09-18 as 17f-dl** (see §17): each attempt runs against its
      own log, and the reproduction is now an ordinary passing test.

## 7 · F3 · The security-event stream

- [x] **7a · The stream itself.** — `2d0a544` `ordo-secevent`: the same
      machinery (chain hash, checkpoints, the `Signer` trait, §7's actor
      taxonomy) pointed at a different table, plus
      `ordo_exec::secevent::PgSecurityStore` for the durable half — in
      `ordo-exec` for the layering reason 8e settled. **Separately keyed**:
      a second `Ed25519Signer` from `ORDO_SECURITY_CHECKPOINT_KEY`, argued
      with `sign.rs`'s own three reasons applied a second time — opposite
      lifecycles, opposite blast radius (one key means compromising the
      routine attestation key is also the ability to forge the record of
      the compromise), opposite audiences — and the signed bytes carry
      their own domain tag, so even a deployment that misconfigures both
      variables to one seed cannot have a chain checkpoint verify as a
      stream one. **Separately retained**: its own number (90 days by
      default, longer than a service log's and shorter than a space's
      command history), and pruning drops a whole prefix ending at a
      **verified** checkpoint, refusing a cut whose stored head disagrees
      with the marker — verification then resumes from that head.
      **Per-space or deployment-wide is routed, not configured**, because
      the events do not all have a space: §6.1's members-only rule sends an
      event to a space's stream only for a members row or a live grant
      scoped to *that* space (a suspended member still counts — that denial
      is exactly the one the owner must see), and everything else —
      strangers, unverified callers, the platform's own custody events — to
      the deployment stream. **An unverifiable presentation names no
      principal**: `Subject::Unverified` carries no claimed subject and no
      token fingerprint, since one is a way for a stranger to make the log
      say a name and the other is a verifier for a secret; the test checks
      it in bytes. Collapse is `(scope, subject, op)` with the count inside
      the hashed body and a per-window cap that overflows to one
      suppression marker; the encoding is domain-tagged and is its own
      decoding, so a `$commandlog` body grafted into `$secevents` fails to
      decode rather than verifying as something. Proved against Postgres: a
      domain revert leaves every stored byte of the stream identical, the
      two artifacts verify independently and under different keys, and a
      denial appends **while the command chain's per-space lock is held**
      — with a control showing a command append in the same conditions
      does queue, so §6.1's "no interaction with the append lock" is
      asserted rather than reviewed.
- [x] **7b · The mandatory alert.** — `2d0a544` `DenyVolumeMonitor`
      evaluates a sliding-window *increase* over `ordo_denials_total`,
      aggregated per `(space, principal)` — summing over op, because a
      sweep is one principal spent across many ops and per-op alerting
      would see sixty small numbers instead of one big one. Threshold and
      window are `ORDO_DENY_ALERT_THRESHOLD` / `_WINDOW_SECS`; unset is
      **not** off but the built-in default plus a `MISCONFIGURED` line,
      since a mandatory control a missing variable silences is not a
      control. Operator surface: `/security/alerts`, gated on its own
      reserved op name, because naming a suspect is a different disclosure
      from publishing a counter. The **correlation id** is minted at the
      door as `ordo_commandlog::CorrelationId` — the same type and value a
      command group carries, not two strings a convention says should match
      — and a firing resolves to the requests behind it through the bounded
      correlation sample every collapsed entry keeps. Today it joins the
      security stream to the command chain and to key custody (a metered
      release records the lease's own correlation); it joins no trace and
      no structured request log because neither exists, which is the same
      gap that keeps key-release breadth per-correlation instead of
      per-principal. Honest about the rest: one process's counters, so no
      cross-replica aggregation, no exporter, no routing, no persistence
      across a restart — listed in `ordo_secevent::alert`'s module doc as
      what a deployment must still supply.
      `IdentityAccess`'s `audit: Mutex<Vec<String>>` is gone; it was a
      test-observation buffer, not a record, and `take_audit` now renders
      the same prose out of the routed events that go to the stream.
- [x] **7c · The drainer, the bound, and retention.** — 7a and 7b built the
      record and the alert; nothing moved an entry from the recorder to the
      database. `PgSecurityStore` was constructed only in tests, so a
      long-running process accumulated events in an unbounded in-process
      buffer and **lost every unwritten denial on restart** — the events
      lost being exactly the ones a review would want, since a process that
      restarts under a denial storm restarts because of it.
      `ordo_host::secdrain::SecurityDrain` is the missing half, on the
      shape `PgInvalidationBus` already uses: a constructor that starts a
      task, a handle whose lifetime is the task's, no broker. It is wired
      into the deployment assembly beside the invalidation listener and
      drains the door's **own** recorder, so `deployment.rs`'s
      `health_deep_is_access_controlled` now asserts that the denials it
      makes reach `$secevents` — and that the unverifiable one lands in the
      **deployment** stream, which is §6.1's members-only rule surviving
      the whole assembly rather than only a unit test.

      **The unreachable store: drop, bounded, and declare it in the
      record.** Argued in `ordo_secevent::recorder` against the two
      alternatives. *Refuse the request* is rejected on §6.1's own pin
      ("the stream is evidence, not a precondition") and on mechanics —
      `observe` runs after the door has decided, so there is nothing left
      to refuse, and making it refusable would hand an attacker a denial of
      service costing them one unreachable database. *Spill to disk* is
      rejected as a second durable store with its own retention, its own
      confidentiality, its own ordering problem folding back into a hash
      chain, and an ephemeral filesystem under it on most deployments. What
      ships: a hard outbox cap; **arrivals** are refused so the *onset* of
      a burst survives (the same rule the per-window cap already applies);
      and the loss becomes `EventKind::EventsDropped`, **a hashed link in
      the stream**, appended as soon as the store is back. A counter in a
      process that is about to restart is not visibility; a gap declared
      inside the artifact a reviewer is auditing is. It is a *different*
      marker from `Suppressed` on purpose — one says the caller had too
      much variety, the other says the database was gone, and folding them
      together would hide the second inside exactly the burst that causes
      it. A drop marker is also the one entry the bound may never discard;
      it is regenerated from a tally rather than queued. Nothing is
      sampled: every observation is recorded, collapsed with its count, or
      counted into a declared loss. Visible in four places — the stream,
      four `/metrics` counters, a `security_stream` block on
      `/health/deep`, and that surface's top-level `degraded` after ten
      minutes of failure — and deliberately **not** on `/health/ready`,
      because pulling a node that is serving correct answers out of
      rotation over its audit writer converts a logging fault into an
      outage, fleet-wide when the cause is the shared database.
      `SecurityDrain::shutdown` closes every open window and writes it, so
      a *planned* restart loses nothing.

      **Retention is age, anchored, with a tail** — and the doc says why it
      is smaller than the command log's rather than leaving it to look like
      an omission. Three of that floor's four inputs have no counterpart:
      no registered consumers (nothing reads this stream from a cursor; it
      has no wire surface), no undo horizon (a security entry cannot be
      undone), and no client pin — so no `x-ordo-verifiable-from`, because
      no client holds a pinned security head to be invalidated. The value
      is computed anyway (`verifiable_from`) so the disclosure is one field
      the day a reader exists. What remains: an entry older than
      `Retention::keep_secs` is eligible, the cut lands on a **verified**
      checkpoint at or below it, and **the last entry always survives**.
      That last bound was missing and is load-bearing twice over: an empty
      stream reads as "never written" from genesis *and* as "entries
      missing" from its checkpoint, and — the sharper reason — the append
      path reads the head out of storage, so an emptied table makes the
      next denial **silently fork a second chain from genesis**, reusing
      the freed indices and orphaning every checkpoint. Retention would
      have manufactured the exact shape tampering produces. Driven by the
      drain on `secevent.retention`, its **own** schedule row claimed with
      `claim_schedule` — never `claim_due`, which is how a second cadence
      inherits the first's period. With no `ORDO_SECURITY_CHECKPOINT_KEY`
      there is no lawful cut point, so retention is inert and
      `/health/deep` says `INACTIVE` in those words rather than looking
      healthy while the disk fills.

      **The audit, measured** (`the_cost_of_one_recorded_decision_is_what_the_audit_says`,
      which prints its numbers and asserts loosely so they cannot drift
      silently):

      - **Per request: ~1.0 µs, no I/O, one mutex.** `observe` folds into
        an open window — no allocation of an entry, no round trip, no
        lock the request path shares with anything else. Measured at
        1.004 µs/denial release, 3.3 µs debug. It is **off** the request
        path in the sense that matters: the database is never touched by a
        door thread. One defect was found and fixed while measuring — the
        per-window cap check was a `keys().filter(scope)` scan of the whole
        open-window map on every observation that opened a window, i.e.
        O(open windows) *per request, holding the recorder's mutex,
        cheapest when idle and dearest under a storm*. Now an O(log
        scopes) counter lookup.
      - **Per event: 286 bytes hashed, ~704 bytes stored** (measured over
        1,000 rows with the subject index, on a fresh table). The gap is
        two 32-byte hashes, the denormalized filter columns, and
        Postgres's per-tuple overhead.
      - **Growth is per *collapsed window*, not per request**, which is the
        number that matters. A quiet deployment — 1M requests/day, 0.1%
        denied, ~20 distinct `(scope, principal, op)` keys busy in any
        minute — writes on the order of 20 entries/minute ≈ 29k/day ≈
        **20 MB/month, 240 MB/year**, and 90-day retention holds it at
        ~60 MB steady state. A denial-heavy deployment — 10% denied,
        200 keys busy — is ~288k entries/day ≈ **200 MB/month, 2.4 GB/year**,
        ~600 MB at steady state. Both are small; the point of the numbers
        is that neither scales with request volume.
      - **Under a denial storm the cost goes *down* per request.** A
        scanner firing 100k times at one op is **one** entry per window
        (asserted in the cost test), because collapse is `(scope, subject,
        op)` with the count inside the hashed body. Breadth is what costs,
        and it is capped: 256 windows per scope per minute, overflow to one
        `Suppressed` marker. The two remaining storm costs are named
        rather than hidden — an unauthenticated sweep all routes to
        `Scope::Deployment`, so it contends on **one** advisory lock in the
        `ORDS` namespace (never the command chain's, which is the whole
        point of the separate namespace); and `ordo_denials_total` is
        per-principal by design, so a token-guessing flood inflates
        *metric* cardinality, bounded by the registry's existing 10,000
        series cap and `ordo_metrics_series_dropped_total`. The stream is
        the cheap artifact under load; the counters are the expensive one.
- [ ] **7d · Nothing verifies the security stream, and no owner can read
      it.** Found by the 2026-09-07 coverage audit;
      `audit-board-coverage.md` finding #21 named two of these as
      remainders and §7 closed without them.
      **(a) `ordo verify` does not cover the security-event stream.**
      `ordo-secevent::chain` has `verify_stream` and `verify_stream_from`,
      the stream is hash-chained, checkpointed, and signed under its *own*
      key with its own domain tag — and `grep -rn secevent crates/ordo-cli`
      returns **nothing**. `operator::verify` walks the command chain only.
      So the artifact whose entire purpose is to be tamper-evident has a
      verifier that no command, route or job ever calls. 7a's argument for
      a second key is that "compromising the routine attestation key is
      also the ability to forge the record of the compromise" — that
      argument is only cashed by something that *checks*.
      **(b) There is no owner-facing read surface.** §6.1's members-only
      routing exists precisely so a space owner sees the denials in their
      own space; the only route is `/security/alerts`, which serves the
      `DenyVolumeMonitor`'s firings, not the stream. An owner cannot read
      their own space's security events at all.
      **(c) The dashboard explorer** (`dashboard-plan.md`) consumes (b) and
      is downstream of it.
      Done-when: `ordo verify` walks both chains and reports them
      separately (one command, two artifacts, two answers — not a merged
      verdict that hides which one moved); a space-scoped read route gated
      on its own reserved op name, the way `/security/alerts` already is;
      and a test that tampers with a stored `$secevents` row and requires
      the CLI to name it.

      **(a) is built, 2026-09-26.** `ordo verify --space <spc>` now walks
      both artifacts and prints the stream as its own paragraph with its
      own `VERIFIED`/`FAILED` line, and `ordo verify --deployment` walks the
      deployment scope, which belongs to no space and has no command chain
      beside it. `ordo_exec::secevent::audit_stream` is the policy, beside
      the store it reads, and it returns **what was checked** rather than a
      bare verdict: a stream that recomputes from genesis and one whose
      retained tail recomputes from a signed marker are different claims,
      and only one of them covers last month.

      **Neither half short-circuits the other**, and the direction that
      matters is the unobvious one: a broken *command chain* must not stop
      the stream being checked, because the stream exists to be the artifact
      you can still believe after the domain record is in doubt. So "the
      chain is broken" is the moment its verdict is worth the most.

      **Four checks, and the fourth is the one a chain walk cannot do.**
      Nothing was pruned to nothing (a base above zero with no retained
      entries is not an empty stream — `prune_to_checkpoint` refuses to
      leave one); the anchor is genesis or a checkpoint at exactly the base
      whose signature verifies; the stored suffix recomputes; and **nothing
      was removed from the end**. That last one is structural: deleting the
      last N entries leaves a perfectly self-consistent chain, and only a
      signed marker at a higher position knows they were ever there.

      **A keyless run is a narrower claim and says so.** With no key,
      checks 2 and 4 do not run, so the entries recompute against each other
      and nothing proves none was removed from the end. The report prints
      that sentence and names the checkpoint it declined to use, rather than
      a bare green line. `ORDO_SECURITY_CHECKPOINT_PUBKEY` had to be added:
      the command chain has published its verifying key separately from its
      seed since the beginning, and this stream had only the seed — so there
      was no way to name the key without also handing over the ability to
      forge, which is a small part of why nothing ever verified it.

      **Two defects found building it, neither in the stream itself.**
      `SecurityStore::verify_from` bridges through `Handle::block_on`, so
      calling it from an `async fn` — the obvious way to write this — would
      have **panicked** on the first stream with an entry in it. The walk is
      now `PgSecurityStore::try_verify_from`, async, and the blocking method
      delegates to it: one implementation, with the shape that cannot
      deadlock as the primitive. And `try_entries` fails a whole read on the
      first undecodable row with no position in the error, so the first
      report named **index 0** for a corrupt entry at index 2 — the start of
      a 512-entry window. The command chain deliberately does better (its
      own test says the entry "names itself, not the 512-entry window it sat
      in"), and a second artifact with a worse diagnostic is a second
      artifact nobody can act on. The window is now re-read one row at a
      time on the failure path only.

      **I wrote here, and in `ecddae6`'s message, that nothing in production
      signed a stream checkpoint. That was wrong, and the truth is worse.**
      Step 1 of the retention pass signed one, and it is reachable:
      `secevent.retain` is registered whenever the security key is set. So
      the claim that check 4 needed a hand-built marker was false — what it
      needed was a marker signed by something that had *verified*, and the
      retention pass did not.

      Three defects followed from one signer in the wrong place, and the
      third is serious.

      **Its cadence was retention's.** One schedule row, one period, six
      hours — so the window in which a deleted tail is undetectable was set
      by a storage sweep. This is the exact shape `cadence.rs` spends a
      module doc making inexpressible for the command chain (*"attest must
      not inherit checkpoint's cadence"*), reintroduced on the other
      artifact.

      **Its enabling condition was retention's.** `DrainConfig::retain =
      false` is an ordinary storage choice — keep every denial forever — and
      it silently switched truncation detection off. The more consequential
      setting was decided by the less consequential one.

      **It signed without verifying, and a checkpoint is the only lawful cut
      point.** `check_prune` is handed the head *at the cut point* and never
      walks the prefix it destroys, so a marker over an unverified head
      licenses the next retention pass to delete the entries that prove the
      stream was edited. Doctor an entry, wait for the pass: the prefix
      goes, the survivors chain cleanly from a marker carrying the
      deployment's own signature, and `ordo verify` reports **VERIFIED**.
      Retention laundering a tamper, on schedule, as designed. The
      refusal therefore has to live in the signer, which is where
      `undo::sign_checkpoint` has had it for the command chain since it was
      written.

      Falsified six ways: dropping the security block, short-circuiting on
      a chain failure, dropping the end-truncation check, dropping the
      keyless caveat, naming the window instead of the entry, and letting
      `--deployment` silently ignore `--space`. Each break fails exactly the
      test that claims the property and no others.

      **(b) is built, 2026-09-26.** `GET /security/events?space=&from=&limit=`
      on the platform router, gated on the reserved name
      `platform.security.events`.

      **The gate is the whole of it, and it is the one thing here that
      could have shipped wrong while looking right.** Every other route on
      that router authorizes against the **deployment's own** space, because
      every other route is an operator surface — `/metrics` hands over every
      series, `/usage?space=` answers about every tenant. Reusing that gate
      would have produced a route that returns data, passes a naive test,
      and still leaves §6.1's members-only routing pointing at readers who
      cannot read. So `authorize_in` binds **the space the caller named**,
      and since `engine_from_members` installs one profile in every space, a
      tenant owner holds the op in their own space and holds nothing in
      anybody else's. `the_events_route_answers_a_space_owner_for_their_own_space_only`
      seeds a different number of denials in two spaces so a test that read
      the wrong stream cannot pass by coincidence.

      **A deployment operator does not thereby gain read of every tenant's
      stream**, which follows §6.1's split: the deployment's own stream is
      the operator's artifact, a space's is its members'. An operator who
      needs a tenant's record reads it with `ordo verify --space` at the
      database, which is an act on the host rather than a route.

      **`$deployment` is refused by name.** It is not a valid space id so it
      cannot arrive through `well_formed_space` — it is checked anyway,
      because if it ever did arrive the caller would be handed the one
      stream §6.1 keeps from tenants on purpose: the cross-space sweep,
      where a principal probing a hundred spaces is visible. A disclosure
      that size should not rest on another function's alphabet. The test
      writes a real deployment-scope entry first, so a route that served it
      would return a body rather than an empty one that looks refused.

      **The route routes, and that is not a convenience.** S4 sends each
      space's entries to that space's own database. Reading a routed
      tenant's stream from the control pool finds **nothing**, and for a
      security surface "nothing" renders as *your space has recorded no
      denials* — a clean bill of health for a stream nobody looked at, which
      is the worst answer it could give. So `PlatformState` carries the
      door's own resolver and an unwired process answers
      `space_unavailable` rather than an empty page. Same rule the stream's
      verifier follows: "I could not check" and "there is nothing" must not
      be one value.

      Two smaller decisions, stated because they are choices. The reply
      carries `verifiableFrom` on every page, so "no denials" and "the
      denials there were have aged out" are distinguishable, for the reason
      `x-ordo-verifiable-from` exists on the command log. And `count` is in
      each row, because a collapsed window is one entry standing for many
      occurrences and a reader who assumed one row per event would
      under-report a scanner by orders of magnitude.

      **Owner-only rather than manager-and-owner**, matching
      `/security/alerts` and this item's own done-when. The contrary
      argument is real and is the one the platform ops make — an audit
      surface its own administrator cannot read pushes deployments to hand
      out owner — so this is the conservative direction on a disclosure that
      names suspects, and is worth revisiting rather than assumed settled.

      **One break failed to fail, and the fix is the useful part.** Deleting
      the `$deployment` refusal left the probe loop passing, because the
      access check refuses it anyway — nobody holds a membership in a space
      by that name. So the loop was proving the refusal and not the guard.
      The guard earns its place only where that assumption does not hold, and
      that case is reachable: a membership row is a row, and a hand-written
      INSERT, an import, or a bug in something deriving `$space` could put
      one there, at which point the access check **allows** and the explicit
      refusal is the only thing between a tenant and the cross-space sweep.
      The test now grants a membership in `$deployment` and requires the
      route to refuse anyway, and the break fails against it.

      Also falsified: gating on the deployment's own space instead of the
      caller's (the inversion this whole item is about), falling back to the
      control pool when no resolver is wired, and never deploying the
      reserved op — which is D0's own defect for `ALERTS_OP`, a mounted
      route with no op behind it denying everyone in every deployment.

      **The signer is built too, 2026-09-26, and it is (a)'s other half.**
      `secevent.checkpoint`: its own `$schedules` row, its own **one-hour**
      period, leader-only so a fleet signs once per period, and gated on
      `ORDO_SECURITY_CHECKPOINT_KEY` alone rather than on whether retention
      is on. One hour is the *attestation* cadence's number and argument
      rather than the checkpoint month's, because this artifact has no
      second series beside it: the command chain can anchor monthly because
      hourly attestations close the gap, and here one marker does both jobs.

      `ordo_exec::secevent::checkpoint_scope` **verifies, then signs the
      `(upto, head)` pair the walk itself arrived at.** That is a stronger
      promise than `undo::sign_checkpoint` makes for the command chain and it
      costs less: that function holds the per-space append lock across its
      whole verification precisely so the head it reads afterwards is the
      head it verified, paying an append stall proportional to chain length.
      Here the signed pair *is* the walk's last observation, so a concurrent
      append cannot land between verification and signature — it can only
      land after the position signed, which is always lawful. The obvious
      alternative, verify then `try_head`, looks right and is not: an append
      committing between those two reads yields a signature over an entry
      nothing verified.

      Five named refusals — `WouldAnchorTamperedHistory`, `Unanchored`,
      `TailRemoved`, `ForeignMarker`, `TwoHistories` — each a state where a
      signature makes matters worse rather than better. `TailRemoved` is the
      sharpest: a marker at the shorter position would be a *true* statement
      about the survivors, and that is exactly why writing it is wrong,
      because it puts the deployment's signature beside the marker that
      disproves the shortening. Retention's step 1 delegates to the same
      function, so a refusal stops the cut as well as the signature.

      `ordo checkpoint --security (--space <spc> | --deployment)` is the
      operator form. `--security` *selects* the artifact rather than adding
      one, because signing is an act under a specific key and an operator
      holding one of the two deliberately separate keys must be able to sign
      only its artifact. Refusals exit **3**, the verification-failed code,
      not 1: you asked to sign and were handed a verification finding.

      **Two decisions worth naming.** A pass that cannot reach its key
      **skips and says why**, on `/health/deep` and in the startup census,
      because a loop failing every tick forever is an alert nobody can
      clear; the operator command **errors**, because signing is the only
      thing it was asked to do. A head that has not moved is `AlreadyAt`
      with no row written — `put_security_checkpoint` is
      `ON CONFLICT DO NOTHING`, so an unconditional re-sign would not corrupt
      anything, it would *report a write it did not perform*, every period,
      forever.

      24 tests, each broken deliberately and confirmed to fail. One break
      failed for a different reason than its test named: without the
      `Unanchored` check the pruned tail anchors at genesis and the walk then
      refuses with `WouldAnchorTamperedHistory` — so the property survives
      through defence in depth, but the **diagnosis** would send an operator
      hunting a tamper that did not happen. Asserting the refusal *variant*
      rather than merely "refused" is what caught it.

      **The rotation gap this surfaced is closed, 2026-09-26**
      (`secevent-key-rotation`, ruled *"take reccomendaton and document"*).
      Check 4 verifies the **newest** marker under the supplied key, so after
      a key rotation every marker signed before it failed under the new one
      and the stream reported `FAILED` — the word this artifact uses for a
      rewritten record. A deployment rotates its security key **because it
      suspects a compromise**, so the moment the record most needed to be
      readable was the moment it cried wolf, and an operator who had just
      rotated could not tell that finding from the real thing.

      `--security-key-id` and `--security-public-key` now repeat and pair up
      in order, and the verifier **selects by the marker's own `key_id`**
      rather than trying each in turn — which keeps
      `StreamCheckpoint::verify`'s binding intact, so *"some key we hold
      verifies this"* is never mistaken for *"the key it names verifies
      this"*. A marker signed by a key that was not supplied is reported
      **unreadable, not failed**: a narrower answer rather than a wrong one.

      **Old markers are not re-signed**, deliberately. A rotation that could
      restate the past would make the new key retroactive authority over the
      old record, which is the property the separate key exists to deny. A
      mismatched flag pair is a **usage error at exit 2**, not a verification
      failure — passing the flags wrong and the record being wrong are
      opposite findings, and scripts watch for the second. Falsified both
      ways: treating an unreadable marker as a failure, and accepting any key
      that happens to work.

      Still open from that row and **not** built: a published key-discovery
      document, which is the right end state and is a surface with its own
      hosting and caching questions. The `$seccheckpoints` suppression vector
      is left as it was, on the recommendation's own argument — widening the
      key would let a real marker sit beside a bogus one at one position, and
      `try_latest_checkpoint`'s `ORDER BY upto DESC LIMIT 1` would then choose
      between them arbitrarily, which is worse than an insert that refuses
      loudly.

      **(c) the dashboard explorer** is still open and consumes (b).

---

## 8 · Track F audit findings (2026-08-23)

An independent audit of F0–F7 against the code, told not to trust the
docs. 23 findings. The three clusters below would hurt most on day one.

### 8A · Key management — **BUILT** (was: there is none)

The trait now exposes **operations, never keys**. Sealing, opening and
blind-index digesting all happen inside the provider, and a caller must
first take a `KeyLease` naming the space, the purpose and the correlation
it works under. `ordo-sema` no longer depends on `ordo-crypto` at all,
which is the structural proof that no key material reaches the compiler.

- [x] **8a · A production `KeyProvider`. — `f9939e9`** The only implementor is
      `TestProvider`, whose SMK is a constant published in this repo, and
      it is *constructed at the call site* in non-test code — there is no
      injection seam at all. F0's own done-when says a provider returning
      plaintext must not be selectable outside unit tests; the inverse is
      what shipped.
- [x] **8b · Per-space SMK. — `f9939e9`** `smk()` takes no space argument, so keys are
      deployment-global: blind-index digests are comparable across
      tenants (a cross-space equality oracle), and per-space
      crypto-erasure is structurally impossible.
- [x] **8c · DEK rotation. — `f9939e9`** Zero code. Worse, `dek(epoch)` *derives* from
      the SMK rather than unwrapping a stored wrapped key, so "destroy a
      retired epoch" is not expressible — the key is a pure function of
      the master. The spec calls rotation mandatory on every membership
      revocation.
- [x] **8d · Key release stage. — `f9939e9`** No `release(space, epoch, purpose,
      correlation)`, so no purpose scoping, no breadth cap, and the
      mandatory key-release-breadth alert has no substrate.

### 8B · The command log is not durable, and one bug is mine

- [x] **8e · A Postgres `Store`. DONE** — `86599b0` `ordo_exec::commandlog::PgStore`,
      backed by `$commandlog` (`space`, `idx`, `correlation`, `actor`,
      `prev`, `hash`, `body`) and `$checkpoints`. It lives in **`ordo-exec`,
      not `ordo-commandlog`**, and the layering is the reason: the log crate
      is the leaf — units of history, the frozen canonical encoding, the
      chain arithmetic, revert planning, checkpoint signing, no I/O — and
      two crates already depend on it with no driver between them
      (`ordo-conformance`, `ordo-playground`), as would the reference
      verifier the design owes third parties. Putting `tokio-postgres` in
      the leaf would make every consumer of a hash function pull a driver
      and a runtime. `ordo-exec` already owns the pool and the transaction
      seam, so the dependency runs that way instead.
      Two changes to the trait fell out of having a second implementor:
      the accessors return **owned** entries (a durable store has nothing
      to borrow from), and verification **streams** through a new
      `entries(space, from, limit)` in `VERIFY_BATCH` windows rather than
      materialising a million-group chain. The stored `body` is the exact
      canonical encoding the chain hashed, so `chain::decode_group` is now
      the inverse of `encode_group` (with `Actor::parse` for the five
      attribution forms) and a doctored row is caught by the chain rather
      than quietly re-read as a different group. A read that *fails* is a
      verification failure — `TamperKind::Unreadable`, because "I could not
      check" and "it checks out" must not be the same value.
- [x] **8f · Phantom chain appends — FIXED.** — `68453bd` The protocol now
      *builds* the group and holds it pending; only a committed
      transaction appends it, and every failure path drops it. A refused
      or fenced write leaves no link asserting an effect that was rolled
      back. This is as close to "the append rides the transaction" as an
      in-memory store reaches — a durable Postgres store (8e) would put
      the append inside the transaction and get the property for real, so
      until then the ordering IS the guarantee and is stated as such.
- [x] **8g · Per-space append serialization. DONE** — `86599b0` `TxConn::append_group`
      takes `pg_advisory_xact_lock(ns, space)` **inside the appending
      transaction**, which is the form `implementation.md` names and not the
      one `jobs.rs` uses: `pg_try_advisory_lock` is session-scoped and
      non-blocking, right for clock leadership (a replica that loses should
      carry on, and the claim should die with the process) and wrong here on
      both counts. A second writer must *wait*, not fail; and the lock must
      be released by the commit that ends the append and nothing else,
      because a session lock on a pooled connection outlives the work it
      protected. The lock key is `SHA-256(space)[0..4]`, computed in Rust —
      not `hashtext`, whose value is not stable across server versions, so a
      rolling upgrade would put two replicas on different keys for the same
      space and the chain would fork under a lock that looked healthy.
      Belt and braces: `(space, idx)` is the primary key and
      `(space, correlation)` is unique, so an interleave that somehow got
      past the lock collides instead of forking. Proved concurrent, not
      sequential: two transactions in flight, the second observably blocked
      (its future pending, and `pg_locks` showing one ungranted advisory
      lock) until the first commits; plus eight racing writers producing
      indices `0..7` and a chain `verify_chain` accepts. Removing the lock
      fails both tests.
- [x] **8h · A real `Signer`. DONE** — `86599b0` `Ed25519Signer` +
      `CheckpointVerifier` in `ordo-commandlog`. Asymmetric because the
      design requires it: attestation publishes signed heads to receivers
      outside the deployment and client pinning has every client checking
      head continuity, so **a receiver that needs the signing key to check a
      signature is a receiver that can forge one**. Verification takes a key
      id and 32 public bytes and nothing else; the key id is checked against
      the checkpoint's, so key discovery is part of verification.
      `StubSigner` is now `#[cfg(test)]` — the same rule F0 states for a
      `KeyProvider` returning plaintext: a thing that secures nothing must
      not be *selectable*.
      On "`Signer` implementations ride `KeyProvider`" (F0): the signer holds
      its own keypair, injected at the door (`Ed25519Signer::from_env`,
      mirroring `LocalKeyProvider::from_env` — unset is a refusal, not a
      default). Adding `sign`/`verify` to `KeyProvider` was the other
      defensible reading and is rejected for three reasons that are about
      custody: the lifecycles are opposite (`destroy_epoch` is the erasure
      primitive, and destroying a checkpoint key destroys the tamper
      evidence it exists to provide); the scopes are opposite (confidentiality
      keys are per space and must not be comparable across tenants, a
      checkpoint key is one published deployment identity); and the
      disclosure rules are opposite (nothing leaves `KeyProvider`; the
      verifying key's whole job is to leave). A KMS signer is a second
      implementation of the same trait — `sign` is the only operation
      needed, and it is exactly what a KMS exposes.
- [x] **8i · Reachable revert/checkpoint/verify. DONE** Three new `ordo`
      verbs over a new `ordo_exec::undo`, against the **durable** store —
      `ordo verify --space`, `ordo checkpoint --space`, and
      `ordo revert <program> --space --group --as [--dry-run]`. The CLI
      because an operator surface must work when the HTTP door is the thing
      under investigation, and because cron consumes an **exit code**:
      0 ok · 1 failed or refused · 2 usage · **3 verification failed**. 3 is
      separate from 1 on purpose — "index 41 does not recompute" and "I
      could not reach the database" page different people, and a cron that
      only knows `!= 0` cries wolf once and gets turned off.
      **verify** streams keyset windows and names the first bad index *by
      its stored `idx`*, so a deleted row is reported at the position that
      went missing rather than at a shifted offset; a body that will not
      decode names itself instead of its 512-entry window. It anchors at the
      newest checkpoint that verifies **under the published public key**
      (never one it cannot check — that would verify a suffix against a head
      an attacker chose) and always prints which anchor it used, because
      "the whole chain verifies" and "the suffix since a marker verifies"
      are different claims. It also catches **truncation**, which is the one
      rewrite a hash chain cannot see on its own: every surviving link is
      perfect, and only the checkpoint's `upto` says the chain used to be
      longer.
      **checkpoint** signs the head under `Ed25519Signer::from_env` inside
      the space's own `pg_advisory_xact_lock` — the same lock appends take —
      so `upto` and `head` cannot name different positions, and `upto` is
      `max(idx) + 1` rather than a row count, which is the difference that
      matters the first time a prefix is pruned. It **refuses to sign over
      history that does not verify**: a signature makes damage look vouched
      for. The critical section is one checkpoint interval, not all of
      history.
      **revert** applies the inverse **through the write path's own guards**
      as a new attributed group, in one SERIALIZABLE transaction whose chain
      append is inside it. `revert_group` split into a pure `invert_group`
      so the async applier and the `Store` path share one set of inversion
      rules. Per row: `FOR UPDATE`, the **write guard** from
      `ordo_sema::guard_filter_query` (the write protocol's own primitive —
      a second caller, not a second copy) against the row as it stands *and*
      against the row the revert leaves, `rev` as a stated belief (a row
      written since is `cas_miss`, never a silent clobber), and
      refuse-with-referrers in the platform's vocabulary. `rev` moves
      **forward**: a revert is a write. Recorded images are re-read from the
      database, not copied from the plan. Reverting twice is refused by
      `(space, correlation)` uniqueness itself — the inverse's id is derived
      — so "already reverted" needs no second bookkeeping.
      **Who may revert: whoever the space's write guards would let write
      those rows.** A revert of someone else's write is a write on their
      rows and there is no separate undo permission to appeal to. What the
      tool does **not** check is **authentication**: `--as` is asserted, not
      proven — no token, no session, no suspension check, no capability
      allowlist, and no op-level access check (a revert is not an op). It is
      an operator tool holding database credentials, so the guard evaluation
      is not a boundary against whoever holds them; it is what stops a
      revert run *by mistake* from escalating, and what keeps the audit
      record naming a principal whose authority actually covered the rows.
      Two whole classes were **refused rather than approximated**, both
      because the row could not be reproduced: entities with **sealed**
      fields (images carried handles into a payload store that did not
      exist, so restoring the readable half would have invented a state the
      row was never in) and **trees** (the materialised path is physical, so
      no image records it). **8j narrowed the first of those**: sealed
      images resolve through the payload store now, and the refusal applies
      to a payload that is genuinely gone — pruned, or its epoch key
      destroyed — which is what the undo horizon's "refuse with a typed
      rejection, never a revert computed against a missing before-image"
      actually says. Trees are unchanged.
      Proved against real Postgres, own databases, durable store, 15 tests:
      a group doctored in the table is caught at its index with its kind; a
      deleted entry at the position that went missing; a truncated tail by
      the checkpoint that covered it; verification from a checkpoint with
      absolute indices (and the honest converse — an anchored run does not
      look below its anchor, `--from-genesis` does); a rename reverted with
      the row back to `draft spec`, `rev` at 3, the original group still at
      index 0 and the inverse at index 1, and the whole chain still
      verifying; create and delete inverted against real rows; Bram refused
      on Ana's row and Ana allowed on the same group.
- [x] **8j · A sealed payload store — DONE.** `SealedRef` was minted from
      a counter (`h[8..] = log.next()`) and nothing persisted the payload it
      named, so every ref dangled: `std.E.history` could not resolve a
      sealed image and `revert_group` could not converge one back.
      **The handle** is now `KeyProvider::mint_sealed_ref` — a fresh
      32-byte CSPRNG seed run through the provider's own per-space keyed
      PRF under a label naming the epoch, truncated to 128 bits. The
      plaintext, the ciphertext, the nonce and the row are all *not* inputs;
      the only input that varies is the seed and the seed is discarded.
      **The salt is in the derivation, never in the stored value**, and that
      is what it buys: (1) the RNG's failure domain stops at the space
      boundary — a repeated CSPRNG state (a restored snapshot, two replicas
      from one image) would give two tenants the *same* bare random handle,
      which is exactly the cross-tenant equality signal the random handle
      exists to prevent, and a keyed stream per (space, epoch) makes an
      identical seed yield unrelated handles; (2) resolution is scoped by
      construction as well as by the store's `("$space","ref")` primary key,
      two mechanisms agreeing; (3) the 16 bytes that land in the log's
      **plaintext** envelope carry no space tag, no epoch tag and no
      ordering — had the epoch been salted in by concatenation, reading the
      epoch byte down the chain would give anyone holding the log and no
      keys a **revocation timeline**, since rotation is mandatory on
      revocation.
      **The store** is `$sealedpayload`, per space, `PRIMARY KEY
      ("$space","ref")` in the schema's own composite shape, holding the
      envelope verbatim plus its epoch, entity, field and the correlation of
      the group whose image minted it. **It never deduplicates** — no unique
      index on the payload, no content column, no upsert, no lookup before
      writing — because a deduplicating store reintroduces §7's equality
      oracle through the back door: two images naming one row would prove
      two writes equal whatever the handles looked like. The honest cost is
      stated: a converge stores the payload twice, once per image.
      **Garbage.** A payload is reachable only through the image that names
      it, so it is garbage exactly when its group is. Pruning a group
      orphans its payloads, and `ordo_exec::sealed::collect_pruned` is the
      anti-join that takes exactly those, one index scan; nothing schedules
      it, because nothing prunes yet. A **destroyed epoch** deletes nothing:
      the row stays so the platform can say *why* the value is gone rather
      than pretend there never was one, and the stored `epoch` is what lets
      an operator count what destroying a key would strand.
      **The write path** stages a payload per sealed image write and flushes
      them once the group has a correlation, inside the same transaction as
      the rows — so a payload and the row it describes commit together, and
      a refused write leaves neither. Sealed fields are no longer dropped
      from `row_image`: before-images, after-images and delete-images all
      carry handles now, which is what made the revert possible at all.
      **The revert refusal narrowed.** `invert_group` no longer refuses
      every `SealedRef` — whether a handle resolves is a question about a
      deployment, and the pure crate would have to guess. The applier
      resolves each handle against the store under a `Purpose::Write`
      release, re-seals the plaintext under the **current** epoch with a
      fresh nonce (a revert is a write, not a replay of an old envelope) and
      recomputes the blind index from the plaintext — never copied from
      history, because a stored digest would be the equality index over
      historical values the design refuses. `RevertError::UnresolvableImage`
      gained a `reason` and fires only on a pruned payload or a destroyed
      epoch. `ordo revert` itself still refuses a sealed entity, for the
      reason that is now true: the binary has no `KeyProvider` (there is no
      `WrappedKeyStore` over Postgres for it to boot one against), stated
      rather than faked.
      Proved against real Postgres, own databases, 6 tests plus units: one
      value written twice yields two handles over two rows holding two
      different envelopes, and neither handle is any of eight digests a test
      holding the plaintext and both ciphertexts can compute (nor a counter
      — a counter-derived handle has a zero half); a sealed write's image
      resolves to the plaintext through a key release, with the plaintext
      absent from the chain's own hashed bytes and the handle refusing to
      cross spaces; a converge whose before-image is sealed reverts, the
      value comes back, `rev` moves forward to 4, the restored envelope is
      not the stored one, both of the revert's own images resolve, and the
      chain still verifies; a sealed *indexed* create round-trips through
      delete and restore with its `$bidx` recomputed and the row findable by
      equality again; a pruned payload and a destroyed epoch refuse
      differently and neither applies anything; and collection takes exactly
      the orphans, per space.

### 8C · Live functional bugs and unwired machinery

- [x] **8k · Blind-index equality by PARAMETER — FIXED. — `f9939e9`** The literal
      path digests; the parameter path emits `email$bidx = $param` and
      compares raw plaintext against a digest column — silent zero rows.
      The corpus only ever tested the literal path. **This is a live bug,
      not a gap.**
- [x] **8l · Blind-index null sentinel — FIXED.** The digest is now TOTAL.
      `ordo_crypto::NULL_SENTINEL` is `\xff\xffordo.blind.null.v1\xff\xff`
      — deliberately **not valid UTF-8**, because the text path digests
      `str::as_bytes`, so no value a caller can spell (the empty string,
      the word `null`) can claim the null slot. It is HMAC'd under the
      same per-entity/per-field BIK as every value, so it is
      domain-separated and per-space like everything else, and it survives
      a DEK rotation for the same reason every other digest does.
      **The API can now say null:** `KeyProvider::blind_index` takes
      `BlindValue::{Text, Null}` instead of `&str`. The `&str`-only
      signature was the bug's structural cause — with no way to *say*
      null, every caller invented one, and the one they invented was to
      skip the digest and write SQL NULL.
      **Both consequences the spec calls out are closed**, against real
      Postgres (`corpus_stage5.rs::the_null_sentinel_makes_sealed_nulls_
      unique_and_findable`): a second null email now collides on
      `UNIQUE ("email$bidx")` (DL: a nullable unique field admits at most
      ONE null row) — creating one, and clearing one — and
      `customers(email == null)` finds the null row through the sentinel
      instead of comparing a digest column against SQL NULL and matching
      nothing forever. The generated DDL declares `$bidx` **NOT NULL even
      when the base field is optional**, so a writer that forgets the
      sentinel is refused by the schema rather than silently widening the
      constraint. `ordo-crypto`'s module doc no longer asserts the
      opposite of the settled rule; it states the sentinel AND the
      disclosure it buys — the sentinel is the modal digest in the column,
      so **whoever holds the database can tell which rows are null without
      any key**, which `implementation.md` puts on the shape summary.
      *Found on the way:* the wide fixture seeded SIX null emails into a
      `sealed indexed unique` field. That was only ever legal because SQL
      NULL made the constraint vacuous; the seed now carries five values
      and one null. Still owed: the shape summary has nowhere to say this
      yet (8n/9l), and the DDL still emits plain `UNIQUE` rather than
      `UNIQUE NULLS NOT DISTINCT` for *unsealed* nullable unique fields —
      a separate half of the same DL rule, untouched here.
- [x] **8n · Shape verification gates nothing — GATED.** — `2840a31`
      `ordo_host::shape::verify_registry` runs the mechanism over the whole
      registry against an empty shadow and refuses the boot on any
      mismatch, naming the op, the compiler's type and the database's. The
      refusal is a **type, not a convention**: `PlatformState::new` takes a
      `ShapeReport`, the only source of one is a verification that passed,
      so a deployment that skipped the gate has no router to mount.
      "The whole registry" is stated rather than implied — parameterized
      reads are verified by binding one canonical placeholder per declared
      type (a value cannot change a shape its type already fixed), taking
      coverage from the 7 argument-free ops to 26 of the fixture's 27
      reads; writes are bucketed as *no result shape*; the one op that
      cannot be posed is named with its reason; and a run that verifies
      **nothing** refuses, because zero coverage is indistinguishable from
      no gate. Verification runs on its own short-lived pool: `SET
      search_path` is connection state, and a pooled connection carrying
      it would aim the next ordinary read at an empty schema.
- [x] **8o · Nothing is served — MOUNTED.** — `2840a31` A second router
      (`ordo_host::serve::platform_router`) merged alongside `ordo-api`'s.
      `ordo-api`'s stays generated-from-registry with one path and no
      sibling dependencies; the catalog renders compiler artifacts,
      readiness answers for *this* pool and *this* KeyProvider, and metrics
      are per-process — none is dispatch, and a `Catalog` trait with one
      implementor would be a boundary invented to keep a route in the wrong
      crate. `/health/live`; `/health/ready` (registry, shape gate,
      database, KeyProvider — failures named by *check*, never by
      dependency); `/health/deep` and `/metrics` **access-controlled** on
      reserved op names granted to owners only; the scoped catalog in both
      renderings at `/.well-known/ordo-catalog`, empty for an
      unauthenticated asker rather than an enumeration.
      The **metrics substrate** is an in-process counter registry with a
      Prometheus text rendering, a hard series cap, and its limits written
      down: no traces, no histograms (three of the eight families absent
      rather than approximated), no structured logs, no exporter, no
      cross-replica aggregation. It exists for the two mandatory security
      alerts and says so. Denials are counted **at the door**, where the
      principal is proven — per principal per space per op (7b); key
      releases by a `KeyProvider` decorator, per space and purpose, with
      the per-principal gap named (`release` never sees a principal).
- [x] **8p · Cross-node invalidation — CROSSES.** — `2840a31`
      `ordo_exec::jobs::listen` opens a dedicated connection and drives it
      with `poll_message`, the only way a notification is handed out rather
      than dropped — the pool's driver awaits its connection and discards
      every async message, and `LISTEN` is session state a pooled
      connection cannot hold. `PgInvalidationBus` republishes into per-op
      channels through an entity→ops index that includes **guard-walk
      entities** (G16), so a membership write invalidates a guarded live
      query. No local shortcut: this node's own writes travel out through
      `pg_notify` and back through its own listener, so the local and
      remote paths are one path. Proved by a write committed through node
      A's pool and executor reaching an SSE subscriber on node B's, with
      the in-process bus shown *not* to cross so the result cannot be an
      accident of a shared channel.
- [x] **8m · Isolation levels. DONE** — `86599b0` `OpClass` encodes
      `implementation.md`'s table (read / addressed write / converge-or-create
      → READ COMMITTED, reads additionally READ ONLY; bulk conditional and
      multi-statement group → SERIALIZABLE) and `Pool::begin_for` states the
      level rather than inheriting `default_transaction_isolation`, which a
      bare `BEGIN` does. The write path runs at `MultiStatementGroup`: the
      protocol is preconditions, then locks, then mutations, which is the row
      the table pins.
      `40001`/`40P01` are typed as `PoolError::Serialization` at the same
      seam that already types `22012`/`22003`, because the retry decision
      cannot be made by matching English error text. The retry boundary is
      the **whole command group**, re-executed from scratch — a group built
      inside the failed transaction images a snapshot that no longer exists,
      so each attempt drops its pending group, re-claims its lease in its own
      fresh transaction, and rebuilds. The budget is finite with jitter and
      exhaustion is a *retryable conflict*: `serialization_conflict` is now a
      real wire class (409), which `api-engine-design.md` §4 owed as
      [proposed] — `ordo-host`'s prefix tripwire caught its absence the same
      hour, which is what that test is for.
      Forced, not simulated: `crates/ordo-exec/tests/isolation.rs` drives the
      canonical SSI write skew into a real `40001`, asserts the same
      interleaving commits both sides at READ COMMITTED (the anomaly,
      recorded), and shows two conflicting units of work both completing
      through the loop with one re-executed.
- [~] **8q · Durable idempotency and layer-1 pinning — BOTH BUILT.**
      `PgReplayCache` (`ordo-host/src/replay.rs`) replaces the in-memory
      cache's silent failure mode: two replicas held two maps, so a retry
      landing on the other one missed, executed, and "exactly once" degraded
      to "at least once" with nothing to observe — and a restart did the same,
      once per deploy.
      **`claim` is one statement**, exactly as the design pinned it: the
      unique constraint decides who reserves, in the statement that acts on
      it, and the stale takeover is a **predicate on the conflicting row**
      rather than a second statement. A `SELECT` then `INSERT` — or a
      `SELECT` then `UPDATE` for the takeover — would be the Z-012 race one
      layer down. Only after losing the insert does it read the row, which
      cannot race because the row exists and its state only moves forward.
      **Responses are sealed at rest** under the space key, AAD-bound to the
      `$idempotency:response` pseudo-entity so a stored response cannot be
      swapped for a column's payload. A replay cache is a plaintext mirror of
      every write response, sealed fields included; keeping it in cleartext
      would undo the sealing for anything a caller wrote and retried. An
      unopenable entry is a **miss**, not an error: the epoch is gone, there
      is nothing to replay, and a re-execution is the honest cost.
      **An unreachable cache reports `InFlight`, never `Reserved`** — a cache
      that cannot be reached must not silently become no cache at all.
      Proved against real Postgres on the properties the in-memory version
      cannot have: replay across a dropped pool, exactly-one-reserves across
      two independent connections, mismatch across connections, stale
      takeover, and the sealed body being illegible in the table while still
      replaying verbatim.
      **The crash window is closed, and the closure is a ruling, not a
      fold-in (2026-09-06, `docs/plan-idempotency.md`).** This entry used to
      say `put` running outside the write's transaction let a later retry
      *re-execute a committed write*. It does not, and had already stopped
      doing so when the plan audited it: the `$replay` journal rides the
      effect's own transaction (`TxConn::claim_lease` /
      `complete_lease`, with AA-005's fencing generation), so commit
      publishes the effect and the record of it or neither, a retry's
      in-transaction read answers `Applied`, and the attempt rolls back
      untouched while the router answers instead of executing. What a crash
      between commit and `put` loses is the **verbatim response body**, and
      only that.
      **`put` stays outside the transaction, deliberately**, for two costs
      the fold-in cannot avoid. Sealing takes a `KeyProvider` release, and
      in the transaction it would have to *succeed before the effect could
      commit* — so a custody outage would stop every write, where today it
      loses only a replay convenience. And the append lock is
      transaction-scoped, so any statement placed after the chain append
      runs with it held: an unbounded `INSERT` there (a `whole` read's
      response is the object graph) is `implementation.md`'s *"anything
      that widens the critical section"*, exactly.
      **The plan's third argument did not survive measurement, and is
      withdrawn.** It also said the journal's own `complete_lease` UPDATE
      was a small cost *inside* that section. It is not inside it: the lock
      is taken as late as possible and the completion record is already
      written when the append acquires it (5f, `lock_wait.rs`, proved from
      `pg_locks` rather than from call order). Nor is it small in the
      counterfactual — ~85us against a ~300us section, about 29% — which
      makes the conclusion stronger rather than weaker: the section has
      room for nothing, and the thing already there is only free because of
      where it sits.
      **The retry is told the truth in the field machines read.**
      `idempotency_applied` (409, terminal) — *the effect committed, its
      body went with the process that ran it, stop retrying* — replaces the
      reuse of `idempotency_in_flight`, whose name promises the caller that
      polling will eventually change the answer. It never will. One
      producer (the router's `AlreadyApplied` arm), an inventory row, and
      both directions asserted in `ordo-api/tests/api.rs`: a crash-window
      retry gets it, a verbatim replay still gets its 200 and never sees it.
      **One thing this still does NOT do, pre-existing shape.**
      `KeyBinding::Permanent` is stored like any other entry rather than as
      columns on the created row, so create-once lasts as long as the entry.
      **Layer-1 client pinning — BUILT.** `bootstrap.md` calls it *"always
      on, ~free, and requires no configuration at all"* on the strength of
      responses carrying a command-log position and the signed head at it.
      They carried neither.
      **It is attestation, not read-your-writes** — worth stating because a
      position on a response looks exactly like a consistency token and is
      not one. Nothing waits for a replica. What it buys is tamper
      detection: a client keeps the last `(upto, head, signature)`, and a
      deployment that later serves a different head for a position it
      already attested rewrote history. Every client keeping one 100-byte
      pin is a witness, so detection latency is one request rather than one
      audit. It **detects and does not prevent**, the same bound
      bootstrap.md states for the whole stack.
      **"~free" is a constraint, not a boast** — a layer everyone disables
      is not a layer. The head only moves on append, so the signature is
      computed when the head moves and cached; what remains per request is
      **one indexed lookup**, stated plainly rather than called free.
      **An empty chain pins GENESIS.** "Always on" that switches on after
      the first write is not always on: a client whose first request is a
      read must hold something, or it cannot tell *no history yet* from
      *this deployment does not attest*. A query FAILURE still pins nothing,
      and the difference is the point — an empty result is knowledge, an
      error is the absence of it, and signing genesis on an error would be
      signing a claim nothing checked.
      **No pin on a door refusal**, for the reason the version headers
      already carry none: a position tells a caller the space exists and how
      much history it has.
      Signed over the log's own `Checkpoint::signed_bytes`, so a client
      verifies a pin with the routine that checks a stored checkpoint —
      publishing a second attestation format for one head would let the two
      disagree. `at` is zero: the claim *"this head is at this position"* is
      timeless, and keeping the bytes a function of the chain alone means two
      replicas sign identically without coordinating.
      Proved that the pin follows the chain and never goes backwards
      (`ordo-host/tests/attest.rs`) — which had to be tested there rather
      than through the door, because the deployment harness's fixture
      executor builds groups in memory and never appends them, so every
      response it makes pins genesis, honestly.
- [x] **8r · `std.space.callersOf` — BUILT, on a platform-op path the rest
      of `std.space.*` can follow.** The reason it could not be built was not
      effort: `security-
      logging-design.md` §6 rule 4 requires the via to include **token**, and
      **capability grants were never recorded** — `grant_for` minted into a
      token and nothing wrote it down. The op would have answered confidently
      while omitting exactly the callers most worth auditing, and an audit
      surface that lies by omission is worse than an absent one.
      **The registry now exists** (`ordo_identity::grants`,
      `ordo_host::grants::PgGrantRegistry`) and `callers_of` is proved
      against real Postgres in both directions. Two things it also fixed on
      the way: **a grant could not be revoked at all** — its only end was
      expiry — and a grant had **no identity**, so `CapabilityGrant` gained a
      `GrantId` that the token carries, because `(principal, client, ops)`
      collides on the ordinary case of consenting twice to the same scope and
      revoking "that grant" would have revoked the replacement too.
      **The boundary is deliberate and stated**: the registry is NOT consulted
      to admit a request. §5 pins the pinning comparison as *"the decidable
      form, needing no registry history"*, so the door still decides from the
      token alone and a registry outage cannot become an authorization
      outage. Revocation is where that bends, and a deployment that wants it
      enforced in the request path must call `is_live` and accept the
      registry into that path — `ordo-identity` names the trade-off rather
      than taking it.
      **The mechanism, and the decision it is.** Every `OpSpec` derives from
      the compiled registry; `std.space.*` reads runtime state instead, and
      9k deliberately did not build a second derivation for it. It still has
      not: a platform op is a second **source** of specs, not a second kind
      of op. One hand-written `OpSpec` — hand-written because there is
      nothing to compile it from — registered into the one registry the door
      dispatches on, executed through the one `OpExecutor` trait. The access
      check, the `$space` bind, the parameter binder, the version headers,
      the uniform refusal envelope, the scoped catalog and the metered
      executor are all the same ones every compiled op gets, and nothing in
      `ordo_host::platform_ops` re-implements any of them. **No golden
      moved**, which is the mechanical proof of the claim: nothing was
      derived, so no compiled artifact changed.
      Two alternatives refused, each of which looked cheaper. *Not a second
      route* beside `/health/deep` — `callersOf` is an op, it takes a
      parameter, it belongs in a caller's catalog, and a capability grant
      must be able to *name* it in order to be refused; a route has none of
      that and needs its own copy of every check. *Not a second registry* —
      the specs live on `Deployment` rather than inside a `Loaded`, which
      says the true thing: a platform op is not part of a program version and
      does not move when one is swapped. A program that compiles a colliding
      name is **refused at load** (`LoadRefusal::ReservedOpName`) rather than
      shadowed, because the shadow would be the audit surface answering to
      the thing it audits.
      The door this opens: the rest of `std.space.*` is a spec, a handler arm
      and a profile grant. The writes additionally need a command group and a
      chain append, which a read did not have to solve, and the item does not
      pretend otherwise.
      **The refusal rule, argued.** It returns principals, so it is an
      enumeration surface and two oracles have already been found here.
      *Manager and owner*, listed explicitly because roles carry no
      hierarchy — not editor, who cannot change who may call anything and so
      would gain only a map of the privilege structure; not owner-only,
      because the manager **administers the answer** and a control too tight
      is one people route around by handing out owner. And *step-up gated*,
      which is where the token question is settled: §5 pins that capability
      tokens can never satisfy step-up, and `require_step_up`'s own doc names
      **full export** as what belongs there — a dump of a space's access
      control state including everyone's connected apps is export-class by
      any reading. So a capability token is refused whatever its allowlist
      says, and cannot be lowered by a client (Z-003) or by a profile edit.
      Reusing step-up rather than minting a "human-only" flag matters: the
      platform has exactly one mechanism for *not delegable*.
      **Zero new refusal classes, on purpose.** Every refusal is the door's
      one `access_denied`. A class meaning *"you are not a manager"* would
      itself be the oracle. The op's own refusal is about its **argument** —
      an op outside the asker's scoped catalog answers as one that does not
      exist, under `not_found`, whose documented meaning is already exactly
      that. Deliberately not an empty result: an empty answer for an op that
      does have callers is the lie-by-omission that kept this item shut.
      **Proved in both directions** (`ordo-host/tests/callers_of.rs`, real
      Postgres). Nine refusals — viewer, editor, roleless member, non-member,
      suspended owner, manager without presence, manager with stale presence,
      a capability token *holding this exact op and claiming fresh presence*,
      and an unverifiable token — asserted **equal to each other and to an op
      nobody registered**, status, code and message, so no refusal names the
      wall it hit. Plus the positive control in the same test, an owner-only
      op that a manager cannot ask about and an owner can, and a revoke round
      trip that removes the token caller and provably nothing else.
      One boundary stated rather than closed: a grant's manifest pin is not
      re-checked, because a per-op record carries the combined hash but not
      the op set behind it. A drifted grant is listed and would be refused at
      the door — a false positive, which is the safe direction for an audit
      surface and is not the failure rule 4 exists to prevent.
- [x] **9x · The compile-class set is closed again — TRIAGED AND
      DOCUMENTED.** A census found **24 of 61 author-facing refusal classes in
      no specification document**. §0 answered half the question itself: it
      already says *"`ParseError`, `SchemaError` and `SemaError` all carry a
      `code` drawn from this inventory"*, so the missing classes were always
      §0's to hold.
      **Twenty documented**, under two new headings — schema build (twelve:
      `duplicate_entity`, `unknown_ref_target`, `tree_not_self_ref`,
      `multiple_owners`, the unique/index family…) and binding/planning
      (eight: `ambiguous_field`, `ambiguous_match`, `no_match_path`,
      `bad_aggregate`, `empty_projection`, `unpageable`,
      `duration_unit_mismatch`, `reserved_param`).
      **One collapsed.** `type_error` was a name too general to predict where
      to look, raised once, for `joined` applied to a non-text field — which
      is exactly what `argument_type_mismatch` was minted for. One mistake,
      one class.
      **Two named as internal vocabulary**, following 10f's ruling on
      `unknown_field`: `unscoped_source` and `unresolved_blind_index` are the
      engine reporting on *itself*, not on a program, and must never be a
      conformance case's answer.
      **C105 was a live doc/code disagreement**, not merely an undocumented
      class: three places said `parse_error` for a bare non-boolean field
      while the binder answered `non_boolean_condition`. The code is right on
      the merits — the grammar read the text perfectly well and the defect is
      its type, which is the parse/bind boundary ruling. Document corrected.
      **And the mechanism that let it hide is fixed.** §0 opens *"Every
      reject case names one class"*, and `synonym_spellings_all_fail`
      asserted only that nothing planned. It now asserts each rejection's
      class is one §0 names, with the inventory read **from the document at
      build time** rather than retyped, so the two cannot drift. The
      extractor works by shape rather than position, because message bodies
      contain brackets of their own and a check that reads the wrong token
      passes for the wrong reason — which is worse than no check.
      The census list is now **empty**: a new undocumented class fails the
      build outright rather than joining a crowd.
- [x] **8y · `members` was writable by domain code — FIXED.** Found by the
      settled-vs-code audit and **executed against a real server before the
      fix**. DL §3 says a platform entity's fields are *"all read-only to
      domain code"* and that *"there is no domain-authored path to minting
      one"*. Nothing enforced it: `platform_members()` built every field as
      an ordinary writable one and `RESERVED` covered only `id`/`rev`/`depth`.
      **This was a privilege escalation, not an untidiness.**
      `ordo_host::deployment_engine` builds the door's access engine from
      `SELECT "id", "role" FROM "members"` — so **the role a domain op wrote
      was the role the door read** — and the reference profile grants
      authored write ops to Editor and up. The audit ran it as an editor:
      `promote` → `updated: 1`, `promoteAll` → **`updated: 6`, every member
      an owner**, `del members` → `deleted: 1`, and a suspension lifted by
      writing `status`. Only the outright mint failed, and by accident —
      `principal` happens to be a lexer keyword.
      Fixed with an `Entity::platform` flag, refused at **registration** (so
      the op never deploys) and again at the write path's **one dispatch
      point** — a check the next verb can forget is not a check. The class is
      `reserved_derived_name`, whose documented meaning was already *"the
      statement writes something the platform owns"*; the inventory row named
      `id` and `rev` because those were the only two such things, and it now
      names the general rule.
- [x] **9y · Two-valued logic was not compiled — FIXED.** DL §2 *"Null
      semantics [settled]"* fixes the truth table (`null == null` **true**,
      `null != 5` **true**, ordering against null **false**, `x != y` ≡
      `not (x == y)` *"with exactly one meaning however spelled"*) and then
      states the obligation in its own headed paragraph: *"every comparison
      compiles to the distinctness family… **everywhere**, without
      exception… **A plain `=` in generated SQL is a bug wherever a nullable
      operand can reach it.**"*
      The planner emitted `=` and `<>`. So every comparison against a
      nullable column got SQL's three-valued answer and **the row was
      silently dropped** — no error, a shorter result set. The dangerous
      shape is `!=`: a filter meant to exclude one value also dropped every
      row where the field was never set.
      Fixed at the single renderer every site funnels through, so guards,
      `whole`, `cond` arms and bulk-write predicates inherit it. `==` →
      `IS NOT DISTINCT FROM`, `!=` → `IS DISTINCT FROM` (total, so plain
      `NOT` around it stays correct and `!=` needs no separate negation
      path); ordering keeps its SQL form **wrapped in `COALESCE(…, false)`**,
      because `not (a < b)` must be *true* for a null operand; `in` likewise,
      being a disjunction of equalities.
      **Recorded once before as X-011 and closed as a document fix — the
      planner was never changed.** Proved now against a real server on an
      unguarded entity, including the partition property (`==` and `!=`
      together return every row, which is exactly what 3VL allows and this
      language forbids) and an assertion on the emitted SQL, so a refactor
      that reintroduces a plain `=` fails loudly rather than in a count
      nobody reads.
      Found on the way: `SpaceAudit`'s `table_mentions` counts `FROM "`, and
      `IS NOT DISTINCT FROM "col"` contains it — so it scored a phantom
      table per comparison and refused good statements. Corrected by
      subtraction rather than by parsing, because the audit is blunt by
      design and **fails closed**: a miscount can only refuse a good
      statement, never admit an unscoped one. And one test in `emit.rs` was
      asserting `<>` — it had been pinning the bug.
- [x] **9z · `since` was frozen at the corpus clock — FIXED.** DL §5:
      *"`since` anchors to execution time — the same op returns different
      rows tomorrow."* The engine folds the window at BIND time into an exact
      date literal, so no `NOW()` reaches SQL and plans stay reproducible —
      **that design was always right**. What was missing is that nothing
      supplied a clock: all **eight** `Binder` construction sites hard-coded
      `(2026, 5, 1)`, the conformance corpus's pin, including `bind_as`,
      which the executor calls **per request**.
      So `since 30d` compiled to a window anchored to 2026-05-01 and widening
      by a day every day — in the one feature whose entire point is not being
      stale. Invisible to the corpus because the corpus pins that date on
      purpose, and the binder's own doc comment already said a real runtime
      *"will supply the request's execution instant"*. Nothing did.
      The clock is now a **required argument**, not a defaulted field: a
      default here is exactly the shape of the bug — the wrong answer,
      available without anyone choosing it. Tests say `Clock::CORPUS` at the
      call site, so "this wants the fixed date" is distinguishable from
      "somebody forgot". `Clock::now()` uses the civil arithmetic already
      present for `since`'s own shifting rather than adding a time
      dependency to the platform's one nondeterminism.
      Three of the eight sites keep the pin with a reason: `since` is a
      **step**, so neither a guard atom nor a write right-hand side can
      contain one and no window is folded on those paths.
      Not done, and named: DL §2 says an op containing `since` carries a
      **re-evaluation tick** derived from the finest granularity in the op,
      and the subscription re-signals on that boundary. The folded literal is
      now correct at bind time; nothing yet re-signals when the day turns.
- [x] **8z · A space owner administers acl rows — BUILT; reading is not
      part of it.** DL §3 bootstrap **rule 6 [settled]**: an owner may grant,
      revoke and change levels on every acl-derived link entity *"without
      holding an access row on anything"*, evaluated **at the door, once per
      call, before planning** — explicitly *"not as an extra disjunct in each
      row's guard"*.
      Before this the owner got `not_found` on grant and revoke, and
      `setBoardsAccessLevel` answered **`updated: 0` inside a success
      envelope** — a silent no-op on an authorization change. **10b's closure
      rested on this**, having ruled a guest- or routine-created ungranted row
      safe *because* the owner was the universal repair path.
      **The mechanism is the spec's.** The door step decides once, then the
      call runs against a schema view whose acl entities carry no guards —
      *"guards are not consulted"* by their not being there, so nothing
      downstream needs a flag and nothing downstream can forget to check one.
      A test asserts the derived guard text is **unchanged**, since the `or
      caller is owner` disjunct is what rule 6 forbids: it makes ownership a
      row-level fact every guard must re-derive, which is the per-row reading
      that reintroduces the deadlock.
      **Scoped by construction**: the exemption applies only when *every*
      statement in the call targets a link entity, so an owner writing
      `tasks` meets the ordinary guard like anyone else, and a non-owner
      editor is refused. Both directions asserted.
      **The root's guards clear too**, and this was found rather than
      designed: `grant` creates a row whose `board` ref is checked against the
      ROOT's write guard, so clearing only the link left it answering
      `not_found` on the board — the exact condition *"without holding an
      access row on anything"* waives. Revoke and the level change passed
      throughout, because neither validates a ref, and that asymmetry is what
      made the cause legible. Not a widening: no statement in such a call can
      write the root.
      **The owner cannot READ the rows they administer, and that is
      correct** — I first recorded this as an owed ruling and it is already
      ruled, the other way. DL §3, `[settled — resolves Z-004]`: *"No acl row
      is private from the space owner, and that is the definition of an
      owner… An owner who can administer acl rows can grant themselves any
      level on any row, so nothing inside their own space is
      cryptographically or structurally hidden from them. This is a
      **disclosed property**, not a hole."*
      So the owner is not blind. The question is only **by what route** they
      look, and the document answers that in the obligation it attaches:
      *"every such reach is an ordinary attributed command in the space's
      chain (**the owner cannot look without leaving a record**)."*
      Reads append nothing to the chain — only writes do. So a read exemption
      would be exactly the reach that leaves no record, which is the one thing
      the ruling forbids. The owner's route is to grant themselves a level —
      an attributed command, in the chain, visible to every member — and then
      read as an ordinary holder. **The friction is the audit trail**, and
      removing it would remove the audit trail with it.
      That is why rule 6 lists grant, revoke and level change and stops there:
      those three ARE the reach. Extending it to reads would not widen an
      owner's power (they already have all of it) — it would make the
      exercise of that power silent.
      Obligation two — *"the consent page for anyone joining a space they do
      not own must say so plainly"* — is **not a platform work item**, and
      filing it as one was a category error. The platform provides the
      mechanism and the derived scope; how a deployment presents consent is
      its product's decision, not ours to make or to build. What *is* ours is
      to suggest the wording where an implementer will meet it, and
      `bootstrap.md` already supplies the sentence — but only inside a setup
      decision table, which nobody building a consent surface reads. Now
      cross-referenced into `deploying.md` §5 beside the existing consent
      guidance, with the corollary that carries the action: members who need
      rows the owner cannot reach need a **different space**, not a different
      rule.
      **The flag exists** — rule 6's other half, *"logged with the owner as
      actor and **flagged as owner administration**"*. It was the last thing
      this item owed: the actor was always right, and the flag had nowhere to
      live. `CommandGroup.owner_administration` is now that place, and it is
      **in the chain** rather than beside it, because a record an operator
      could quietly clear answers the auditor's question with whatever the
      last writer preferred.
      **Adding a field to a frozen layout, without moving a hash.** The chain
      hashes the *rendering*, so a byte added to what an existing group
      renders would void every chain written, every checkpoint signed over
      one, and every golden pinning one. So the flag encodes **by presence**:
      an unflagged group renders exactly the bytes it rendered before the
      field existed, and a flagged one renders one more (`0x01`, a trailer).
      Backward compatibility is not a shim here, it is the identity
      `encode(unflagged) == the old encoding` — proved by a golden chain hash
      computed with the encoder that had never heard of a trailer and pinned
      before the field was added (`PRE_FLAG_GOLDEN`). It still matches.
      Canonicity holds because exactly one byte names a trailer: an explicit
      `0x00` is refused as `TrailingBytes`, so "no flags" keeps its one
      spelling, silence, and no group has two renderings.
      **One producer.** The door's rule-6 match arm — the same expression
      that grants the exemption — passes `true` down to the one place that
      builds the group, as a parameter rather than a mutation afterwards, so
      a rule-6 group is *constructed* flagged and there is no window in which
      one exists unflagged. Nothing reads it to decide anything; the guards
      are still absent rather than overridden.
      **Proved by discrimination, not by assertion** (`ordo_flag_*`): the
      owner's grant, revoke and level change land flagged; Ana — `admin` on
      the same board, and not the owner — lands the *same op on the same row*
      unflagged through the ordinary guard; and the owner's write to an
      unguarded entity lands unflagged. That middle case is why the flag
      cannot be derived at read time from "the actor is the space owner":
      that question has the same answer whichever path the call took, which
      is precisely the distinction rule 6 exists to make.
      **A revert never inherits it.** The flag records how *this* call was
      authorized, not what it undid; a revert is a separate call by a
      separate actor that rule 6 did not authorize, and copying the flag
      across would put a decision in the chain that no door ever made.
      **No DDL moved, and could not have.** `$commandlog.body` is the
      canonical encoding, so a field inside it needs no column — and a column
      would have been unworkable, not merely redundant: the platform tables
      are created with `CREATE TABLE IF NOT EXISTS`, which does nothing to a
      table that already exists, so a new column would be absent on every
      deployment that has ever booted. The durable test asserts the column
      list to say so out loud.
      Found while building it: `plan-export.md` §2.3 promises a reader
      holding nothing but `commandlog.jsonl` can recompute `SHA-256(prev ‖
      body)` **without the platform**. A flagged group whose exported line
      omitted the flag would re-encode a byte short, and an outside verifier
      would read a lawful group as tampering. The export carries it, by
      presence, exactly as the chain does.
- [x] **10i · Groups reach guards — the derivation emits its second
      disjunct.** — `00b38fb` (the writing half and Y-013)
      Found by the settled-versus-code audit. The **door**
      expanded group membership (`ops_by_group`) while **no guard could**, so
      an acl row naming a group granted nothing: the row landed, a UI would
      show it, and every guard asked whether the caller's own members row was
      the one named — which for a group it never is. Silent, and in the
      **safe** direction only by omission.
      **`groupMembers` now exists** as a platform entity —
      `{ group -> members owned, member -> members owned }`, per space,
      undeclarable, `platform: true`, `unique (group, member)`. DL called it
      `[settled]` and said it must be *"queryable and walkable like any
      entity, which is what makes group-aware guards and headcounts writable
      instead of magical"* — that clause is the design: `== $caller`
      deliberately never expands a group, so the escape hatch is an explicit
      walk, and there was nothing to walk.
      **The derived guards carry the second disjunct**, in DL's own printed
      shape — two steps, `X(member == $caller) or
      X(member.groupMembers.group(member == $caller))`, rather than one step
      with an internal `or`. The two are logically identical (`EXISTS(a) OR
      EXISTS(b)` ≡ `EXISTS(a OR b)`); the printed form was chosen because the
      shape summary is read by people diffing it against the spec. It
      inherits down every owned subtree automatically, so `cards` got it
      without being mentioned.
      The scope manifest now discloses `groupMembers.{group, member}`, which
      is the property DL asks for in the same sentence: the derivation
      *"discloses that it does by showing the walk it generated"*.
      **The membership surface now writes it** (`ordo_conformance::
      membership`): three commands, because DL names three — create a group,
      add a member, remove one — each appending one attributed
      `CommandGroup`. It is a *platform* surface, not an op, because
      `members` and `groupMembers` are platform entities refused to authored
      statements at registration and at the write path's dispatch, and
      `security-hardening-review-2.md` §B6 says group edits are *"platform
      commands with their own door, not domain-authored ops over a
      members-shaped entity"*. `memberCount` is maintained there, in the same
      command group as the link row it follows.
      **`group_nesting` has a producer.** Refused at the add, with the class
      DL names, asserted to leave no row and no log entry. It is the premise
      the rest rests on: depth 1 is what makes ONE expansion sufficient, so a
      nested group would silently under-expand the guard walk, `callersOf`
      and `memberCount` alike — the same inaudible-omission shape this item
      was opened to fix.
      **Y-013 is ruled** (`corpus-rulings.md`, 2026-09-05): a direct grant and
      a group grant at different levels on the same row resolve to the **more
      permissive**, and the audit reports both paths. The reasoning is that
      the alternatives — the lower level, or a refusal — make *adding someone
      to a group* able to **reduce** their access, turning a routine
      administrative act into a silent revocation. It cost no SQL: the derived
      guard is a union of existence tests over the two paths, which is the
      maximum by construction. Proven in rows in both orderings, with a
      `read`/`read` control so the two permissive cases mean something.
      `grant_paths`/`effective_level` are the audit half, and are the query
      R-G17's surviving-grant-path report was blocked on (`ordo_schema` said
      it *"would be a list that is empty for the wrong reason"* while nothing
      expanded groups); deriving that report as an **op** on every acl root is
      still owed.
      The glossary's *"avoid granting both"* is gone, replaced by the rule.
- [x] **8s · Grant pinning — DL §8's form, BUILT; the escalation is
      refused and proved.** The ruling and its proof are in
      `corpus-rulings.md` (2026-09-05). `OpPin` carries the field set **and
      the row-scope shape** — per entity, the read/write guard walk, the
      `$caller` restriction, the tree flag, and the op-level `unguarded`
      acknowledgment — because §8's counterexample is real and the tests
      execute it: delete `member == $caller` from an op body and the output
      field set is byte-identical, so the §5 field-set-only comparison would
      apply the widening *silently*. `PinDrift::compare` classifies it
      **Expanded** and the grant dies for the whole space until re-consent;
      narrowing applies silently and is audit-noted, which is the behaviour
      §5 wanted and the combined hash denied. Fail-closed at the edges: an
      incomparable guard walk is "a different set of rows" (fatal), an
      undeployed op is expansion-equivalent, and any fatal drift on any op
      kills the whole grant — consent was one act over a set of ops.
      `GrantRecord` stores the encoded pin so "what did I agree to?" has an
      answer, but **the door does not read the registry** — the token
      carries its own pins, so a registry outage cannot become an
      authorization outage. Proved end to end against real Postgres
      (`ordo-host/tests/pinning.rs`): a deleted `$caller` restriction with
      no column moved kills the grant; `unguarded` added while the field set
      *shrank* kills the grant; a narrowing goes through and the audit says
      so; an unrelated op's narrowing kills nothing.
      One boundary recorded rather than closed: a loosened `take` bound
      reaches more rows of the same shape and neither half of the pinned
      manifest can see it — noted in `PinDrift`'s docs as DL has no clause
      for it, not silently folded in.
      *(The item's original text follows for history.)*
      Three documents, three forms, and the code
      implemented none of them. *(Rewritten — the first version of this item
      quoted only `security-logging-design.md` §5 and prescribed a fix that
      `definition-language.md` §8 explicitly calls **wrong**. Corrected by the
      doc-contradiction audit.)*
      **What the code does:** `AccessEngine::grant_for` computes one
      `combined_manifest_hash` over all ops, and `CapabilityGrant` carries a
      single hash and no field sets. Every change to any op kills the whole
      grant. That is **over-strict, so it fails closed** — the current
      behaviour is safe and merely annoying.
      **§5 [pinned]** and `api-engine-design.md` §10 `[settled]` say: pin
      **per op**, its manifest hash and its **ordered output field set**;
      subset = narrowing, applies silently and audit-noted; superset or
      disjoint = expansion, grant dead until re-consent.
      **DL §8 [settled] says that form is decidable and WRONG**, with a
      counterexample that is "the most ordinary edit in the language": delete
      `member == $caller` from an op body, or add `unguarded` to its header,
      and the **output field set is byte-identical** — same columns, types
      and order. A field-set comparison reads that as *not an expansion*,
      classifies it as narrowing, and **applies it silently**. A grant
      consented to as *"this app may read my expenses"* becomes *"this app
      may read everyone's expenses"* with no re-consent, no audit flag, and
      no visible change anywhere. *"Field sets answer which columns; they
      never answered which rows, and rows are what the consent was about."*
      So a pinned manifest must carry the field set **and the row-scope
      shape**. `security-hardening-review-4.md` corroborates DL.
      **This is the dangerous kind of gap**: the current code is safe, and
      implementing the §5 form as written would introduce a silent privilege
      escalation. The per-op half is right and owed; the field-set-only
      comparison is not. Whoever takes this needs the row-scope shape in the
      manifest first — 9l's field-scope work is the foundation, since it
      already derives guard-reached entities and fields from the bound IR.
      Needs a ruling to reconcile §5 and §10 with §8; DL is the language
      authority and states the counterexample, so it should win.
- [x] **9a · `whole` inlines NO read guard — total bypass. FIXED** — `f6b0249` `plan_whole`
      takes no caller and no guard. A guarded entity's whole-document read
      returns the entire object graph to anyone, and the `members`
      signpost dumps `principal`/`role`/`status`. This is the one op that
      returns a whole graph, and it is the one op with no authorization.
- [x] **9b · `whole` on `tree owned` aborts the process. FIXED** — `f6b0249` `owned_children`
      returns the tree self-edge and `row_object` recurses on it →
      `fatal runtime error: stack overflow`. The code comment claiming
      one-owner-max makes it terminate is false for self-loops. A legal,
      spec-blessed shape kills the server.
- [x] **9c · Integer division where the spec pins decimal scale 12. FIXED**
      `BinOp::Div` emitted raw `/`, so `bigint / bigint` truncated in
      Postgres: `money / int` silently produced a wrong number. `avg` was
      bare too. Now: `ScalarTy::Decimal` (expression-only — it has no
      declaration form and `decimal` stays an unknown *type* name);
      `a / b` lowers to `round((a)::numeric / (b)::numeric, 12)` and
      `avg x` to `round(avg((x)::numeric), 12)`; `floor`/`ceil`/`round`
      are the conversions and cast to `bigint` when they land on an
      integral value; `implicit_rounding` refuses an unconverted decimal
      at a write RHS (registration) and at a parameter slot. The binder's
      constant folder was the same bug host-side — it folded `3000 / 7`
      with Rust integer division to 428 while SQL answered
      428.571428571429 — and now refuses. `/ 0` stays loud (no `nullif`)
      and is typed at the pool as `arithmetic_error` rather than reaching
      a caller as `internal`; that class name and its 400 are
      **engine-pinned pending a spec ruling** (DL §2 settles the
      behaviour, `api-engine-design.md`'s closed inventory names no class,
      and `int`/`money` overflow has the same gap). Values, not SQL text,
      are asserted against real Postgres in
      `ordo-conformance/tests/decimal.rs`.
      **Not** blocked on G12 — G12 is `hash(x)`, and division's ruling
      (DL §2, "Division: determinism across venues") is settled.
      `allocate()` is deliberately **not** implemented: see 9f.

### 9B · The docs do not compile

- [x] **9d · Newline-separated `group`/`reduce` braces do not parse. FIXED** —
      `agg_block` now takes a comma OR a line break as the entry separator,
      either of them trailing, and the two mixable. The lexer emits no
      `Newline` inside a brace (G6), so the break is read off the token spans
      (`Parser::at_line_break`). A MISSING separator is still an error, and so
      is an argument-taking aggregate whose argument went missing — otherwise
      `{ rev: sum \n n: count }` would silently read `n` as `sum`'s argument.
      Correction to this item as written: newline braces were **one** of the
      failing blocks in these two docs, not most of them (the corpora use the
      spelling more heavily). The other failures are 9e's, plus the four named
      under 9e′ below. `crates/ordo-syntax/tests/docs_parse.rs` now extracts
      every fenced block from both docs and parses it: 49 of 81 parse, and the
      32 that do not are each named on an allow-list with the construct that
      is owed. The test fails both ways — an unlisted failure, and a list entry
      whose block has started parsing. *(9e′ took it to 50 of 81, and split the
      allow-list's one "owed" category into owed and not-owed.)*
- [x] **9e′ · Doc constructs 9e's list does not name — RULED, all three.**
      Reasoning in [`corpus-rulings.md`](corpus-rulings.md); one construct
      transfers to 9e, which stays open.
      - **`list of { … }` parameter types — neither side wrong; the
        ALLOW-LIST was.** DL §1 marks list params + `each` **[lean]** and
        both doc blocks mark themselves `# [lean]`, so refusing them is the
        parser being right and showing them is the document being right.
        The block harness had filed them under "settled features with no
        parser yet", which promotes an unsettled sketch into a work item —
        the mistake 9c declined to make with `allocate`, whose only legal
        home this is. `docs_parse.rs`'s allow-list now has three categories
        (settled-and-unbuilt · [lean]-and-not-owed · not-Ordo) because they
        are owed differently. **Nothing is owed here.**
      - **`let p = set projects {…}` — the DOCUMENT wins.** Settled in DL §3
        ("Naming a freshly created row in the same op"), R-G19, `traps.md`,
        `modeling-guidance.md`, `tutorial.md`, `sql-translation-corpus-2`.
        The parser has no write-valued `let` production at all, which is why
        `unbindable_write` is inventoried as unreachable. A debt, not a
        divergence: **moved onto 9e's list below.** Deliberately not built
        here — a production without the write-mode derivation behind it
        would make `unbindable_write` raisable in name only.
      - **The wrapped projection — G6 wins; `syntax-examples.md`
        §expiringSoon is fixed.** No step begins with `[`, so a parser
        *could* take the continuation with one token of lookahead; the
        design decides it instead. Delete the wrapped line and what remains
        is a COMPLETE step with a DIFFERENT meaning (`leases(…)` reads the
        whole row), so the continuation would let the next line silently
        narrow what the line above it disclosed — and projection is exactly
        how a caller opts into cost and disclosure. The example now breaks
        *inside* the parens, which G6 explicitly permits, so the block
        demonstrates the rule instead of violating it. 50 of 81 doc blocks
        parse now; the harness's floor moved with it.
- [x] **9e · Unparseable constructs the spec settles — CLOSED; `check`
      row invariants BUILT (row-local), 2026-09-05.** The last owed piece
      landed: `check used <= seats` parses (`CheckDecl`), validates
      (`ordo_schema::check` — row-local, immutable, non-sealed), lowers to
      a real CHECK constraint the database enforces inside every write's
      transaction, and a violation surfaces as **`check_violation`** (409,
      X-016's fix taken at last) — typed at the pool by SQLSTATE 23514
      beside the arithmetic family, never `internal`, with the constraint
      name carrying the condition (`teams$chk0$used<=seats`) so the caller
      learns WHICH check failed. The ref-walking and time-dependent forms
      refuse **by name** (`check_beyond_row`, `check_time_dependent`) —
      the ruling with its reasoning is in `corpus-rulings.md`, along with
      three more classes that fell out (`check_sealed_field`,
      `check_unknown_field`, `check_unsupported`) and two findings:
      DL's own `used: used + 1` spelling did not compile (**since built —
      §6g**, and this suite now executes the constraint against the
      increment as well as the param), and `unique`/`index` never validate
      their column names.
      Two-valued logic holds inside the constraint (COALESCE-wrapped
      ordering, distinctness equality) and the difference is *executed*:
      a null in a checked column refuses where naive rendering would land
      silently. Eight cases in `check_invariants.rs`, real Postgres,
      `ordo_chk_*`. Everything else on the list was already built
      or ruled not-owed — see the sub-entries. The query- and
      parameter-level four are done; `del entity { id: $id }` was never
      owed.

      *The list itself was the problem.* It named twelve constructs as one
      undifferentiated debt, and they are not one thing: three were owed and
      buildable, one is owed and blocked, seven are **[lean] and therefore
      not owed**, and one is settled but has no operand it can name. A
      checklist item that reads as "build these twelve" is how a spec's own
      "we have not decided this yet" gets built anyway — the mistake 9e′
      caught the allow-list making, one document further out.
  - [x] **`unique (a, b)` and `index (…)`. DONE.** Both are `[settled]` in
        DL §3 ("Uniqueness [settled]": *"`unique` on a field, `unique (a, b)`
        composite"*; "Indexes [settled]": *"Composite/extra indexes declared
        with `index (a, b)`"*). The unusual part is that `unique (a, b)`'s
        **pin arrived before its syntax**: the `acl` derivation mints
        `unique (<root>, member)` on its link entity, so `Entity::unique_keys`
        and every consumer of it — the DDL, `SetMode`'s converge-or-create
        classifier, the write path's `duplicate` refusal, `ordo check`'s
        shape summary — already existed with exactly one producer that no
        author could reach. This is the author's spelling arriving at the
        same four places, proved end to end in
        `crates/ordo-conformance/tests/entity_clauses.rs`.
        Rulings taken: **two or more columns, always** — the one-column case
        already has a spelling (the field modifier), and one construct per
        concept means `unique (a)` is refused at the spelling rather than
        becoming a second way to say it; a repeated column in one clause is
        refused for the same reason. **A plain `index` leads with the space
        column too** — DL fixes that for *unique* indexes, where it is
        correctness; for a plain index it is the same shape as a usefulness
        rule, because every statement this compiler emits filters on
        `$space` and an index that did not lead with it would answer no
        query. Engine-pinned, and stated so it is chosen rather than
        discovered from an unused index in production. `index` over a
        `sealed` field is `unsupported_index`: ciphertext orders and groups
        nothing.
        **Found on the way, and it is the real find:
        `NULLS NOT DISTINCT` was missing from every generated unique
        index.** DL: the language has *one* equality, `null == null` is true
        in it, so *"a nullable unique field admits at most one null row"*
        and *"Generated DDL is `UNIQUE NULLS NOT DISTINCT`"* — which the
        document itself calls "the deliberate departure from every SQL
        system". Postgres defaults to the opposite, so a nullable `unique`
        field was **not a unique claim at all** for the rows that left it
        out, and no test could see it because no test had ever written two
        nulls. Two goldens moved for it (`market.ddl.sql`,
        `boardsAccess.ddl.sql`) and the behaviour is now asserted against a
        real server, not only the SQL text. The blind index keeps plain
        `UNIQUE (space, hmac)`: DL spells that one exactly, and its digest
        column is `NOT NULL`, so there is no null for the modifier to rule
        on.
  - [x] **`let x = set E {…}` — the write-valued `let`. DONE, and
        `unbindable_write` has a producer for the first time.** DL §3,
        *"Naming a freshly created row in the same op"* **[settled]**;
        R-G19; five documents spell it. 9e′ deliberately did not build it
        because a production without the write-mode derivation behind it
        would make the class raisable in name only — that derivation
        (`ordo_plan::writes::set_mode` / `stmt_label`) now exists, so this
        is the production landing on top of it.
        - **The grammar needs four tokens of lookahead, and only there.**
          `let` also opens a read pipeline's CTE binding; `let`, a name,
          `=`, then `set` or `del` is what separates them, exactly and with
          no backtracking. `let x = del …` is refused **at the spelling** —
          a delete produces no row in any mode, so the answer does not
          depend on a mode.
        - **The binding is `Option<Ident>` on `WriteStmt::Set`, not a
          variant.** Same call `subtree` took: everything that asks "is this
          a `set`?" — the op kind on the wire, the mode label, the
          entities-touched analysis, rules 2 and 3 — must keep answering
          yes, and a separate variant is what those `matches!` arms would
          silently stop seeing.
        - **What may be bound is read off `stmt_label`**, the single
          authority §6 rule 1 already routes every renderer through — not a
          second reading of the braces, which is exactly the drift that
          function exists to prevent.
        - *Ruled, on a case the spec does not spell:* **a `rev: null`
          create-once IS bindable.** Its derived label is already `creates`
          (this engine ruled that under rule 1 — "the statement asserts the
          row's absence, so converging is not one of the things it can
          do"), and DL's exclusion list is given with its reasons, none of
          which it matches: it addresses no row by an id the caller holds,
          and it affects exactly one. Refusing it would refuse a statement
          for a property it does not have.
        - **Scope is a registration check, and self- and forward-reference
          are the same check.** The environment grows one statement at a
          time, so "never in the same statement" falls out as "not bound
          yet". A member outside the descriptor (`id`, `rev`, and the
          fields that statement wrote) and a name bound twice are
          `invalid_binding_expr` — the existing class for a right-hand side
          whose shape the statement cannot mean. **No class was minted.**
        - **Execution is an AST rewrite before planning**, so the whole
          write protocol downstream — ref visibility, the create gate,
          assignments, images, the command log — never learns about
          descriptors. After the rewrite a statement that named a bound row
          is indistinguishable from one the caller wrote the id into, which
          is what R-G19 says it means. Non-`id` members carry the create's
          *own expression*, so `x.name` and the `name:` it came from cannot
          disagree.
        - *Found on the way, and fixed:* the parent-visibility check (G12)
          refused a ref to a row an **earlier statement of the same group**
          created, because the harness plans every statement before
          applying any. DL's ordering rule already answers it — statements
          in a command group execute in order — so a group-created row is
          admitted, and nothing from outside the group is. The create that
          made it proved its own authorization at its own statement.
        - **`GC095` now runs** (`guards_corpus.rs`).
          **`GC094` still does not, and 9e narrowed the reason to something
          that is not about `let`:** its dependent write is authorized by
          the *auto-grant* the create puts in the same group, and that row
          is not in the database when the second statement's create gate
          probes for it. That is the harness's plan-all-then-apply staging
          (AA-004), and closing it would mean applying each statement before
          planning the next — which trades this harness's "both or neither"
          for it. Recorded rather than traded.
          `crates/ordo-conformance/tests/write_let.rs` proves everything
          `let` itself owes, against real Postgres.
  - [x] **`check` row invariants — CLOSED 2026-09-05 by 9e.** *(This box
        was still unchecked on 2026-09-07 and the work had landed two days
        earlier; corrected by the coverage audit. The blocked-state reasoning
        below is kept verbatim, because it is the record of why the class had
        to be minted before the lowering — and both "things the next attempt
        should know" turned out to be exactly right: the ref-walking and
        time-dependent forms refuse by name, `check_beyond_row` and
        `check_time_dependent`.)* Was: **OWED, and blocked on a missing error
        class rather than on grammar.** DL §3 is unambiguous: *"### Row
        invariants: `check` [settled]"*, plus two further settled bullets
        (**`check` may walk refs [settled]**, **Both sides are re-checked
        [settled]**). Nothing here is lean.
        **What blocks it:** *"violation = typed rejection aborting the whole
        atomic group"*, and **the closed inventory has no class for it**.
        `api-engine-design.md` §4 is headed `[settled]` and says "Codes are
        closed"; `critique-consolidated.md` **X-016** already found this —
        *"Neither found any statement of how a `check` violation surfaces to
        a caller — there is no error class for it in `api-engine-design.md`
        §4"* — and names the fix, "add a `check_violation` code to the
        taxonomy". It has not been taken. Building the lowering first would
        put a **producer behind no class**, so a settled rule would arrive
        as `internal` 500: indistinguishable from a server bug, which is the
        exact failure the write path's typed refusals were added to end.
        `arithmetic_error` shows the one precedent for engine-pinning a
        class, and it was taken by the item that *owned* the ruling; this
        item does not.
        **Two more things the next attempt should know before it starts.**
        (1) A ref-walking `check` **cannot be a Postgres CHECK constraint** —
        CHECK cannot subquery — so the second and third settled bullets need
        triggers on every entity the walk passes through, plus a parent-side
        error that names the child rows. That is a second enforcement
        mechanism, not more DDL. (2) Even the row-local half must refuse
        `today`/`now` in the condition: a CHECK constraint must be
        IMMUTABLE, and DL's closed default set makes those the obvious
        things an author will write.
  - [x] **The nine declaration forms — RULED. Seven are `[lean]` and NOT
        OWED; one is settled with nothing to name; one is `client`, which
        is the same lean pin.** The deciding sentence is a single line, and
        it governs a whole section: `definition-language.md` §8 opens
        **"All illustrative syntax [lean] — concepts settled, file format
        not final."** A *declaration form* IS the file format. So `routine`,
        `principal`, `client`, `policy` and `channel` — every one of which
        is separately headed `[settled]` for its **semantics** — have
        spellings the spec explicitly declines to fix, and 9e′'s ruling
        applies unchanged: a `[lean]` construct is not owed.
        This is DL §3's `allocate` seam read the other way round. There, a
        `[settled]` entry marked a sliver of itself lean and the *semantics*
        were the owed half; here a section preamble marks *all spelling*
        lean while each subsection pins its semantics, so the **spelling is
        the not-owed half** — and the spelling is precisely what "a
        declaration form" means. Same seam, opposite side, opposite answer.
        Case by case:
        - **`routine`** — DL §8 `### Routines — two tiers [settled]`, under
          the lean pin. Not owed. *And the premise that a runtime was
          waiting for it is wrong*: 5d built job-queue **primitives**
          (`enqueue`, `claim_jobs`, `finish_job`, `claim_due`, `schedule`,
          `try_lead` in `ordo-exec/src/jobs.rs`) and **only `enqueue` has a
          production caller** — cache invalidation. Nothing drains the
          queue, nothing writes `$schedules`, there is no worker loop and no
          clock loop, and no `routine:` principal ever runs anything. A
          `routine` declaration would need the mechanism built under it,
          not just a surface over it.
        - **`principal`** — DL §8 `### Principals [settled]`, under the lean
          pin. Not owed. The *mechanism* is the most complete of the nine
          (`ordo-identity`: `Principal`, `PrincipalKind`, the access engine,
          the directory); it is the declaration the spec declines to fix.
          Noted: `adoption-dx-plan.md` line 257 calls §8 principals
          **`[open]`**, the only place in the corpus that does — either way
          it is not owed, but the two marks should be reconciled.
        - **`client`** — DL §8 `### Identity clients (OIDC relying parties)
          [settled]`, under the lean pin. Not owed. It also carries a seam
          of its own: the code fence shows a `stepUp` key, and the prose
          immediately under it **corrects** that reading — *"Step-up is a
          property of the op, not of the client [settled — corrects the
          earlier client-config reading]"* — so the block's own example
          outlives a rule that was withdrawn. Worth knowing before anyone
          transcribes the fence.
        - **`policy`** — DL §8 `### Policies (operator decisions — never
          user consent)`, **with no stability mark on the heading at all**,
          under the lean pin, and heterogeneous below it: `policy spaces` is
          `[settled]`, `policy compliance` is `[lean — draft, needs legal
          review]`, and four more blocks carry nothing. Not owed, and the
          least-specified of the nine.
        - **`channel`** — DL §8 `### Notification channels [settled]`, under
          the lean pin. Not owed. `route` shares the block and is not even a
          reserved word in this lexer (it is contextual), which is the
          grammar agreeing the form is not fixed. The only `channel` in the
          workspace is the LISTEN/NOTIFY invalidation bus — a different
          sense of the word, with nothing behind the notification one.
        - **`app`** (and `page`) — **not owed, and the owning document says
          so in as many words.** DL has no `app` declaration at all; the
          grammar lives in `ui-gen.md` §5, headed *"[mixed: binding grammar
          **normative**, everything else illustrative]"*, which lists block
          set, page composition, layout hints and page-name/op-name
          namespacing as open and then states: *"a manifest cannot be
          compiled against this section until they land."* That is the
          clearest instance of the seam in the corpus — the document splits
          its own section and says which half is buildable.
        - **`trigger`** — **two things share the word, with two statuses.**
          The trigger *layer* is §7, `## 7. Triggers [settled]`, whole and
          above the lean pin. The `trigger …` *clause inside a routine
          block* appears only in §8 code fences and is therefore lean. The
          two are **never reconciled in prose**: §7 spells the fact as
          top-level `when … -> run x`, §8 spells the same fact as a nested
          `trigger when …`, and nothing says which is the declaration.
        - **`when` / `every`** — **settled, not under the lean pin, and
          still not buildable — because their one operand is.** §7 is
          `[settled]` end to end and its spelling pins are real (`every day
          08:00Z` — "UTC is explicit at the keyboard, never silently
          implied"). But `-> run <name>` names a **routine**, and routines
          are the lean half; and §7's own spelling and §8's nested one are
          the unreconciled pair above, so building either picks one. A
          settled construct whose only argument is not-owed is a real
          category, and this is the corpus's example of it.
        The doc-block harness's allow-list is re-filed accordingly: seven
        entries moved out of "settled features with no parser yet" and into
        `[lean]`-and-not-owed, each with its deciding sentence. That
        mis-filing is the same class of bug 9e′ found, one document further
        out — and it had been re-made because the checklist entry above
        said "build these twelve".
  - [x] **Parenthesized guard groups. DONE — `b77011e`** R-G3 admits parens
        when mixing `and`/`or`, and principle 2 requires them, so refusing
        them made the canonical multi-level ACL idiom
        (`syntax-examples.md` §6) unspellable. A group is **distributed**,
        keeping `GuardExpr` in disjunctive normal form. That is a ruling,
        not a convenience: "a disjunct" is the unit three separate rules
        are stated over — at least one must reference `$caller` (G2), a
        write guard is create-exempt only when EVERY disjunct contains a
        reverse step rooted at the new row, and the lowering unions one
        semi-join per disjunct (R-G6) — and a nested node would have made
        all three read past the parens and answer about a walk the author
        did not write. Sound because the guard grammar has no `not`.
        Same call the addressed delete already took for `{ id: $id }`.
        Found on the way: **the guard parser had a precedence table**.
        `a and b or c` parsed silently as `(a and b) or c`, which
        principle 2 forbids and the condition parser has always refused;
        it now refuses it too. Nothing in the corpus or the docs was
        written that way. Engine-pinned: distribution is a product, so the
        normalized form is capped at 64 paths (`parse_error`, by the
        ruling `corpus-rulings.md` took for `take count out of range` — a
        limit on what this compiler can represent, not on what a guard
        may mean). R-G3 says nothing about how many paths a guard may
        have and no inventory names a class for "too many".
        `ordo-conformance/tests/guard_groups.rs` proves it against real
        Postgres in **both** directions, because three plausible wrong
        lowerings all admit every caller a correct one admits; the
        separating negative is a caller holding a `read`-level access row
        in an estate where somebody *else* is admin.
  - [x] **Parameter bounds and parameterized `take`. DONE — `a6f5c93`**
        One change, because they are one proof: `take` bounds RETURNED
        rows (DL ~1816), so it is the row-bound prover's main evidence,
        and `take $n` with an unbounded parameter proves nothing — "as
        many rows as the caller asked for" is the definition of unbounded.
        The bound restores the proof only because the door enforces it, so
        the gate has two halves: registration refuses an unbounded take
        parameter (`unbounded_take`, DL's own name) and reads the bound as
        the op's row bound; the door refuses a value above it
        (`bound_violation`). Found on the way: **`ParamSpec::bounds` had
        no producer at all** — `build_specs` hard-coded `None` — so the
        enforcement half existed with nothing to enforce, and every
        `$n: int <= 5` in the docs was a promise the compiler could not
        make. Rulings taken where the spec is silent: `<=` only (`>=` and
        ranges are X-077's *proposed* fix, unbuilt, so they must not
        parse); bounds on `int`/`money`/`date` only, since a bound the
        door cannot compare is decoration — `datetime` and `duration` are
        **owed, not declined**, and each refusal says which; an implicit
        `min: 0` on a take count, because `take -1` does not parse and a
        parameter count must not say what a literal count cannot (this is
        also `Bounds::min`'s first producer); `unbounded` does not buy an
        unbounded take (it acknowledges space-sized work, not
        caller-controlled work); `take[keys] $n` obeys the same rule. The
        artifact now carries each parameter's `max`, which moved
        `market.artifact.json`: 66 param objects gain `"max": null` and
        the 39 ops that take parameters get a new `version`, because the
        version is the digest of the op's artifact object and its contract
        genuinely changed. Proved against real Postgres in
        `ordo-host/tests/take_param.rs`, including a server PREPAREing and
        EXECUTEing the unfolded `LIMIT $1` form — which the executing path
        never reaches, since it folds the parameter to a literal.
        One more ruling, on a case the spec has not considered: **a bound
        does not travel through composition.** `outer = capped($n: $m)`
        puts `$m` in the `LIMIT`, and the door sees `outer`'s parameters,
        not `capped`'s — so `capped`'s declared `<= 10` is not in force.
        `unbounded_take` cannot see this (it reads an op's own body, and
        `outer` has no `take`). A *viral* bound is not in the spec and was
        not invented; what closes it is the row gate, which already fails
        closed: the composed limit's ceiling is `None`, so the plan is
        Unbounded and `outer` does not register without `unbounded`. A
        bounded `$m` carries its own bound through, which is the one the
        door checks. Asserted in all three directions.
  - [x] **`del entity { id: $id }` — NOT OWED; 9e's list was stale.** It
        landed with **6f** (`fc84e8a`), which is the item that made bare
        `del entity` a parse error. It parses (`declparse.rs`
        `subtree_is_an_explicit_word_on_an_addressed_delete`), the braces
        lower to the `id == $id` condition they mean — one representation,
        so the two spellings cannot drift — and it executes against real
        Postgres on a tree entity (`corpus_stage5.rs`, `dropFolder`) and a
        non-tree one (`guards_corpus.rs`, `revokeAccess` over
        `boardsAccess`). Nothing was built for it here.
  - Beyond the four, and **not** started: `duration` parameter bounds are
    owed three things this engine does not have — a unit conversion it
    refuses everywhere else by design (`duration_unit_mismatch`), a
    `ParamTy` that carries its unit, and `duration_unit_class` (DL §2, a
    door-level class with **no producer anywhere**). ~~`datetime` bounds are
    owed a `ParamTy::Datetime` — a `datetime` parameter validates as text
    today, so it accepts any string, bound or no bound.~~ **Half done
    (17g).** `ParamTy::Datetime` exists and validates
    `YYYY-MM-DDTHH:MM:SSZ` at the door — the "accepts any string" half is
    closed, and it was worse than this row implied: `ordo_host::param_ty`
    had a wildcard arm, so the value reached a plan as whatever the caller
    sent. What is still owed is only the **bounds**: `param_bound` admits
    an int, decimal or date literal and no datetime one, so a datetime
    bound is unspellable rather than unenforced. `text(64)` — the
    spec's own parameter example — does not parse either: `ScalarTy::Text`
    carries no length, so `ParamTy::Text { max_len }` is a second field
    with no producer.
- [x] **9f · Missing aggregates: `percentile`, `distinct <field>`, `joined`.
      DONE** — all three parse and lower.
      `percentile(field, p)` → `percentile_cont(p) WITHIN GROUP (ORDER BY
      field)`, the general form `median` is the p = 0.5 case of; `p` is
      checked at parse time to be a literal strictly between 0 and 1, on the
      source digits rather than through a float, so `percentile(salary, 95)`
      is refused at the spelling instead of being reinterpreted.
      `distinct field` → `count(DISTINCT field)`; it is a different construct
      from the bare `distinct` STEP that W039 composes with `reduce`, and DL
      settles both, so both exist. `joined(field, ", ")` → `string_agg` with
      the ORDER BY *inside* the aggregate — the pipeline's sort at the group
      step, then its tiebreak — because an unordered concatenation is not
      reproducible. The execution tier for the three asserts identities the DL
      states (median ≡ p50, `distinct x` ≡ W039's composition, reversing the
      sort reverses the parts) against a real Postgres, so no expected value
      here was authored: `crates/ordo-conformance/tests/aggregates.rs`.
      Found on the way: a `sort` **before** a `group` was carried into the
      outer ORDER BY and Postgres rejected the statement outright ("must
      appear in the GROUP BY clause"). A pre-group sort orders rows, and after
      the group there are no rows to order; its one surviving meaning is the
      one DL gives it — the order `joined` concatenates in — so the group step
      now captures it for that and clears it.
- [x] **9f′ · `allocate()` — the RULE is built; the SITE is not owed. SPLIT,
      and the split is the finding.** The item asked one question where there
      were two, and 9c and 9e′ had each answered a different one. `allocate`
      is a **semantics** and a **spelling**, and DL §3 separates them in as
      many words: *"The allocation semantics above are settled; the spelling
      lands with that construct."* So:
      - **Settled, and now executable.** The distribution rule is pinned by
        three documents in the same words (DL §3, `glossary.md`, `traps.md`)
        — floor each exact share, hand the leftover minor units out one
        apiece by largest remainder, ties by weight order, parts sum to the
        total — and by a worked value all three print,
        `allocate(100.00, (1,1,1))` = 33.34 / 33.33 / 33.33.
        `ordo_plan::allocate` implements it, beside `hash64` and for the same
        reason: a reference implementation of a rule that must not drift.
        `crates/ordo-conformance/tests/allocate.rs` pins the printed example,
        the sum-to-total guarantee over a hostile sweep, largest-remainder
        *as such* (a sum-only test cannot tell it from round-robin), and
        **host/SQL agreement against a real server** — DL's determinism
        clause is "so both venues agree", and nothing had ever checked it,
        because there was no second venue to check against.
      - **Not owed, and closed as such.** `allocate` is legal **only** inside
        an `each` emission over a `list of { … }` parameter — DL §3 is
        exhaustive by construction, "anywhere else it is `invalid_allocate`"
        — and DL §1 marks both **`[lean]`**. 9e′ ruled a `[lean]` construct is
        not owed. The refusal's claim that no other legal site exists was
        **verified rather than trusted**: all eight expression contexts were
        mapped and no path lets a function call reach generated SQL.
      - **The ambiguity the example resolves.** "Ties broken by weight order"
        could have meant a sort by weight *value*; in the printed example all
        three weights are equal *and* all three remainders tie, so only the
        **positional** reading gives it one answer. Pinned with that argument
        on the page, because the next reader will have the same doubt.
      - **What is still unruled, and named rather than invented:** negative
        weights (no document types the vector beyond the example's
        `weight: int`, and "in proportion to" has no meaning for one) — the
        kernel computes the stated formula and the invariant survives, but no
        refusal was minted. `Σw == 0`, the empty list included, is DL §2's own
        `/ 0` and carries 9w's `arithmetic_error`; no class was minted there
        either.
      - *Structural finding.* **`allocate` can never lower through
        `Planner::function`, even once `each` lands.** It reads the whole
        weight vector to answer for one element, so it is a windowed
        computation over the list relation, not a scalar expression over the
        current row. The arm in `function` is a correct backstop and is
        **not** the slot the lowering will occupy — worth knowing before
        someone tries to finish this by filling that arm in.
      - *Also found, and it is not about `allocate`.* See 9f‴.
- [x] **9f⁗ · `allocate` gets modes — the odd penny is a business rule.**
      Owner's ruling: *"A dev decides in which way to allocate and when that
      happens, not the spec."* DL §3 pinned one distribution and, without
      meaning to, also pinned a **business decision** — which party ends up a
      penny up. Contracts name a residual party; accounting policies name
      largest-remainder; settlement agreements name the largest counterparty.
      Three right answers to the same arithmetic, and hard-coding one makes the
      platform unusable for the other two. Full reasoning in
      [`corpus-rulings.md`](corpus-rulings.md), 2026-09-05.
      - **`allocate(total, weights [, mode])`, four modes, default unchanged.**
        `largestRemainder` (default) · `first` · `last` · `largestShare`. Named
        modes rather than a family of functions: one name to learn, and an
        unknown mode is a **compile error** listing the four rather than a
        silent fall back to the default — a misspelling that quietly allocated
        by largest-remainder would be a money bug wearing a typo's clothes.
      - **A mode is a QUEUE, not a second algorithm** — and that is the design
        move that matters. Every mode floors every exact share and hands the
        leftover minor units out **one apiece**; the mode chooses only the
        order. So both invariants are structural: the parts sum to the total
        exactly, **and** no part is more than one minor unit from its exact
        share. Without the second, "the odd unit to the first party" would
        generalise to *all* the leftover to the first party, which still sums
        and is a redistribution wearing a mode's name.
      - **Determinism is the boundary, stated as the admission test:** a mode
        may depend only on `(total, weights)`. **Rotation is excluded by it** —
        spreading the penny across successive invoices has to remember what it
        did last time, which is application state; an expression function that
        answers differently on a retry breaks both venue agreement and the
        write protocol's idempotency. The refusal message says so, because
        "unknown mode" would send the reader hunting for a typo. The blessed
        shape is a counter the application keeps and passes in as the weights'
        order — a rotation an auditor can see.
      - **Proved per mode, not for the default and then assumed.** Both
        invariants over the whole hostile sweep × four modes; each mode checked
        to be the queue it claims against an independently derived one; the
        four asserted **pairwise distinct as functions** (per-mode tests all
        passing is consistent with two modes sharing a comparator, since the
        expectation and the code would make the same mistake); and host/SQL
        agreement extended to every mode against a real server, plus an
        assertion that the SQL oracle can itself tell the modes apart — without
        which agreeing about them proves nothing.
      - *Found on the way, and nobody was looking for it.* **`allocate`
        panicked on input no document forbids.** The kernel closed with
        `i64::try_from(part).expect("a part is bounded by the total")`; that
        bound holds **only when the weights share a sign**. A mixed-sign vector
        that nearly cancels makes parts far larger than the total they still
        sum to — `allocate(i64::MAX, (1_000_000, -999_999))` panicked. 9f′ had
        recorded negative weights as unruled *and* claimed the kernel "computes
        the stated formula and the invariant survives": the invariant does, the
        function did not. Now `arithmetic_error`, the class an `int`/`money`
        overflow already carries — a reuse, not a mint. Negative weights stay
        unruled; "unruled" cannot mean "panics".
      - *Also found.* **The literature's mode names do not lex.** Every
        accounting text spells it `largest_remainder`, and DL §1 *Identifiers*
        is `[settled]`: one rule for every name, `[a-z][A-Za-z0-9]*`, no
        underscores. The first draft shipped four modes nobody could type. The
        mode-set test now also asserts every name matches §1 and actually
        lexes, because the next person will reach for the same spelling.
      - **The site is still not owed**, unchanged from 9f′: `each` over a
        `list of { … }` parameter is `[lean]`, every call is still refused
        `invalid_allocate` at its site, and a legal mode does not make an
        illegal home legal.
- [x] **9f‴ · Guard-walk literals are type-checked — FIXED.** Found by the
      `allocate()` agent while verifying something else. `guard_atom_cond`
      was the one place in the binder that built a comparison **without ever
      consulting what it compares against**: the literal was wrapped raw as
      `lit_ty(…).unwrap_or(Text)` while the column's type sat unused on the
      line above.
      Not cosmetic. `qty == "seven"` on an `int` field **registered, planned,
      and emitted `"t0"."qty" = 'seven'`**, so a real server answered
      `invalid input syntax for type bigint` on every read — a registered op
      that looks healthy and fails at request time, **on the authorization
      path**, which is the worst place for a guard that cannot be evaluated.
      The fix reuses `check_param_ty` rather than offering a second opinion:
      a guard has no reason to be more permissive than a parameter slot, and
      two type tables would eventually disagree about `money`. The class is
      `param_type_mismatch` — a reuse, not a stretch, since the inventory
      defines it as a value fitted to a declared type by exactly these rules,
      and minting a guard-specific class would name where the mistake was
      *noticed* rather than what is wrong (the 9r boundary ruling).
      Why it survived: the realistic shape is a **well-formed guard with one
      ill-typed conjunct** — G2 refuses a guard that never consults
      `$caller`, so the comparison always sits beside a caller term and the
      guard looks like authorization, because it is. And schema construction
      alone never touches `guard_atom_cond`; the entity looks fine until
      somebody queries it, which is why the test binds a read rather than
      building a schema.
- [x] **9f″ · `hash()` parsed with no lowering. REMOVED from the function
      list** — it now fails at parse time as an unknown function, which is the
      honest answer. DL §3 names `hash(x)` and promises **bit-stability across
      platform versions**, but DL ~130 and engine-plan G12 both leave the
      ALGORITHM unruled ("engine-pinned pending confirmation", flagged ⚠).
      Every hash Postgres offers in-core (`hashtext`, `hashint8`) is
      explicitly not stable across server versions, so shipping a lowering
      would make the compatibility promise and break it in the same commit.
      When G12 is ruled — SipHash-2-4 with a published key needs a real
      implementation, not a built-in — `hash` goes back in the list with its
      lowering, in one change. Until then a caller learns `hash` is not in the
      language at parse time, not as `[unsupported]` after their op looked
      accepted.
      - *Closed by 9t (2026-08-30).* G12 is ruled and `hash` is back, in
        one change as this entry said it would be. The prediction was
        wrong about the algorithm in a useful way: the ruling **withdraws**
        the SipHash pin rather than implementing it, because SipHash is not
        in core Postgres (and the `hash(x)` of a condition is computed by
        the *database*), and because a *fixed published* key removes the
        only property a key provides while adding the key to the
        compatibility surface. SHA-256 truncated to 63 bits needs no real
        implementation at all on the SQL side — `sha256(bytea)` is in core
        — which is why the restoration was cheap.
      - *(9c's finding on `allocate`.)* The **distribution rule is pinned
        precisely enough to build**: floor each share, hand out the
        remaining minor units by largest-remainder, ties by weight order
        (DL §3; `glossary.md`; `traps.md`). What is missing is its only
        legal home. `allocate` is legal **only** inside an `each` emission
        over a **list parameter** — anywhere else is `invalid_allocate` —
        and DL §3 says in as many words that `each` over a list parameter
        is **[lean], not settled**, that the allocation spelling "lands
        with that construct", and that "if `each` changes shape
        `allocate` moves with it". `each` lexes as a keyword and has no
        grammar; `list of { … }` parameters do not exist at all. Building
        the algorithm now would mean inventing the unsettled construct it
        is defined against, so 9c implemented the settled half instead:
        `allocate` parses (closed-arity 2), and every site the language
        can currently express is refused as `invalid_allocate` — which is
        exactly what the spec says those sites are, and which gives the
        class its first construction site. The remainder algorithm lands
        with `each`.

### 9C · Structural gaps

- [x] **9g · Typed write rejections reach the wire as HTTP 500.** Was:
      `classify()` matched seven prefixes and the other ten typed refusals
      fell through to `internal`, so a tree cycle, a referrer refusal and a
      crash were one answer. The cause was two closed inventories saying
      the same thing — `ExecError`'s eight variants restating `ErrorCode` —
      and the smaller one could not express what the write path raises.
      `ExecError` is now `{ code: ErrorCode, message }`, so an executor
      refusal whose class has no wire code cannot be constructed, and
      `ErrorCode::executor_raisable` keeps `access_denied` (the door's word)
      out of the executor's mouth at that boundary. Sixteen classes joined
      the inventory with statuses chosen on one question — *did the stored
      data refuse this, or the statement?* — data → 409 (`has_referrers`,
      `tree_cycle`, `tree_has_children`, `tree_depth_exceeded`,
      `lease_fenced`), statement → 400 (`tree_bulk_reparent`,
      `reserved_derived_name`, `ambiguous_identity`,
      `unbound_required_param`, `projection_unknown_field`,
      `invalid_binding_expr`, `param_type_mismatch`), neither → 5xx but
      still named (`unknown_source` 500, `sealed_open_failed` 500,
      `sealed_unsupported` 501). `classify()` no longer enumerates: it asks
      the inventory. **The seam is still a string** — the write protocol
      returns `Result<_, String>` and its class is a prefix by convention —
      and it is named as a hazard in `classify`'s own doc comment, with a
      static tripwire (`http.rs::every_raised_class_prefix_is_a_wire_class`)
      that reads `write.rs` and `ordo-exec` and fails on a prefix the
      inventory does not know. That catches the misspelling and the
      forgotten entry; only typing the write path's error catches a refusal
      that never had a prefix, which is `write.rs`'s change to make.
- [x] **9h · The `space` column — DONE.** — `232410d` Every table now carries
      `"$space"`, and the isolation layer has something to isolate.
      **Physical only, never language-visible**: `id`/`rev`/`depth` are
      implicit *fields* (undeclarable but addressable); `$space` is the
      same kind of thing as `$path`, and the `$` makes it unpronounceable —
      no author can declare, project, filter on, or assign it, because the
      lexer cannot produce the token. **The primary key is `(space, id)`**
      and **every foreign key is composite**, `("$space", ref) REFERENCES
      t("$space", "id")`, so a cross-space ref is a constraint violation
      rather than a convention nobody happens to break. **Every generated
      unique index leads with the space** (DL, uniqueness — settled), so
      `email$bidx` is now `UNIQUE ("$space", "email$bidx")`, the
      `UNIQUE (space, hmac)` form P2 names.
      **Forgetting the predicate is the failure this was designed against.**
      `Planner::table` is the only function that renders a base table name
      and it returns the source and its predicate *as one value*; every
      emitted statement is then re-verified (`unscoped_source`) before it
      leaves the crate, so a source rendered any other way is a planner
      refusal. The write path, which builds SQL by hand in seventeen
      places, wraps its database handle once at the entry point and audits
      every statement on the way out (`unscoped_write`). The space itself
      is a **planner argument**, never IR and never re-derived — `$1` in
      every artifact, folded to a literal where the door bound one — and
      `$space` is refused as an op parameter (`reserved_param`), which is
      SH2-1 one layer earlier than the envelope.
      **Proved against two tenants**: `tests/spaces.rs` seeds the fixture
      twice with the SAME ids and shows reads, dot-hop joins, guards, tree
      walks, `whole`, paging, addressed/create/bulk/subtree writes,
      uniqueness and the command chain each seeing only their own — and a
      mutation of the predicate to a tautology is caught by ten of the
      twelve checks (the other two are caught by the `whole` and write-path
      mutations). `tests/scoping.rs` poses the negatives directly.
- [x] **9j · No op is live-capable — FIXED.** — `2840a31` `live_capable`
      is now `kind == Read`, per DL §5: *named ⇒ callable ⇒ sourceable ⇒
      subscribable; there is no definition flag*, and liveness is a
      call-time choice. A `set` is an effect, not a result to hold a
      subscription over, and the router had already pinned `?live` to
      `GET`. Landed **after** 8p on purpose: a subscription that cannot see
      other nodes' writes is worse than none, so what makes `?live` honest
      is the bus a deployment wires, not this flag. Still owed: the
      **re-evaluation tick** for ops containing `since` (DL §5) — an op
      whose result changes with the passage of time and no commit gets no
      signal, which is a staleness false negative in the one feature whose
      point is not being stale.
- [x] **9i · Text collation unpinned (G11) — PINNED, pending the ruling.**
      `ordo_plan::TEXT_COLLATION` is `C` — byte order over UTF-8, which for
      valid UTF-8 **is** codepoint order, so it is the same answer G11
      already pinned on paper, spelled in the one collation every Postgres
      installation has (no ICU build, no locale on the host, no agreement
      about which `en_US` a libc version means — glibc has changed that
      ordering across releases). Emitted everywhere ordering happens:
      `ORDER BY`, the keyset `seek` predicate **and** the outer page
      ordering (same rendered key on both sides, which is what makes
      "strictly after this row" answer the question the order asked), a
      `SELECT DISTINCT`'s projection (Postgres demands the ORDER BY
      expression appear in the select list verbatim), `joined`'s ordered
      `string_agg`, `ROW_NUMBER`'s window order — `take[keys] n` picks
      WHICH rows survive by it — `min`/`max` over text, and `whole`'s
      `json_agg` over the owned closure.
      **Proved, not asserted.** `corpus.rs::text_ordering_is_pinned_and_
      deterministic` runs against a database whose own collation is
      `en_US.UTF-8`, over values the two collations disagree about
      (`a`, `a b`, `a-b`, `ab`, `B`, `Z`), and asserts the exact `C` order;
      it then strips the pin from the same plan and asserts the deployment
      really would have answered differently; then it walks every keyset
      page and asserts the walk visits each row exactly once in that order.
      Ids are assigned so the tiebreak could not rescue a dropped pin.
      **Not** collated, deliberately: text equality (`=`, `DISTINCT`,
      `GROUP BY`, `UNIQUE`) is byte equality under every deterministic
      collation and Postgres refuses a nondeterministic one as a database
      default; and nothing in the DDL, because pinning a *column's*
      collation also repoints `lower()`/`upper()` at `C`'s ctype and stops
      folding non-ASCII case — a deterministic order bought with a wrong
      `normalize(lower)`.
      **Still owed.** A real G11 ruling, and this does not substitute for
      one — see engine-plan G11 for the list. The engine chose the order it
      could defend, not the order the language promises: `C` sorts `Z`
      before `a`, and whether that is the language's order, whether NFC
      normalization happens on write/read/never (nothing normalizes today,
      so `é` and `e´` sort apart), and whether an author may ask for a
      locale-aware order per key are all language decisions. Also owed:
      a `COLLATE "C"` expression index per sort key, because a
      default-collation btree cannot serve `ORDER BY x COLLATE "C"` —
      today every pinned text sort is a sort node.
- [~] **9k · The std library — the derivable half is derived.** — `f2ca202`
      `ordo_schema::std_ops` derives seven of §2's twelve verbs for every
      entity — `get`, `count`, `create`, `put`, `update`, `delete`,
      `deleteSubtree` — the same way `acl_admin_ops` derives its three:
      `OpDecl` nodes built from the declaration, injected by
      `Registry::extend_from` at the same two call sites, and pushed
      through the **same** `check_registration`. No second mechanism, and
      no exemption: guards inline because binding inlines them, and the
      artifact for `std.tasks.get` carries the `projects` →
      `projectMembers` semi-join and `bypassed_guards: []` exactly as a
      hand-written op over `tasks` does. 85 ops on the market fixture; the
      artifact golden grew by 6,676 lines and **zero deletions**, which is
      the proof the derivation perturbed no authored op's manifest or
      version hash.
      Proved in `ordo-conformance/tests/std_library.rs` (databases
      `ordo_std_*`): the derived write refuses Ana/Bram/Cleo/Dov with the
      **same class the authored `renameTask` gives each of them** —
      applied, `access_guard`, `not_found`, `not_found` — and the derived
      read filters identically to `taskOne`, both directions asserted in
      one test so "refuses everything" cannot pass. `std.boardsAccess.*`
      cannot raise Bram's own acl level by any of three verbs, which is
      the escalation that matters because that row decides who may write
      rows. Sealed fields are absent from every derived read (§2.4's open
      skeleton is the default, and the only half built — see the ruling).
      **The five verbs NOT derived, each for a different reason** — the
      reasons are the finding, and they are recorded in
      `corpus-rulings.md`: `list` is refused *by the row-bound gate*
      (§5.7 claims keyset paging is a bound; `row_bound` does not accept
      it, and a `take` cannot substitute because `plan_paged` puts the
      take INSIDE the paged subquery, so `take n` caps the listing at n
      rather than windowing it); `whole` has no `OpBody` to live in and an
      `[open]` cap; `under` and `rollup` are `[lean]`, so not owed; and
      `history` needs a log source the language does not have.
      **Also excluded, and not by the spec:** the platform `members`
      entity derives nothing. It is unguarded, so a derived read is a
      whole-space principal directory and a derived write is an
      unattributed path to minting and re-roling principals.
      **`std.space.*` does not derive here and never will** — its ops read
      runtime state, not a table. `callersOf` is built on the platform-op
      path instead (8r), which is a second *source* of specs rather than the
      second derivation this function exists to avoid; the rest of the family
      follows that path, not this one.
- [x] **9k′ · `std.E.list` — RULED and BUILT.** Keyset paging counts as a
      row bound: `ordo_plan::cost::keyset_paged` accepts a plan iff it is a
      keyset page over the language's total order (non-empty tiebreak,
      nothing beneath the order that can multiply a row; group keys and
      `distinct` restore totality by construction). The ruling with its
      reasoning is in `corpus-rulings.md` (2026-09-05) — including why a
      large `take` could not substitute: `plan_paged` puts the take inside
      the paged subquery, so `take n` caps the listing *in total* and row
      n+1 is unreachable by any cursor, a silently-complete-looking lie now
      pinned by `a_take_cannot_stand_in_for_paging`. `std.E.list` derives
      (149 → 165 ops in the golden, audited as pure addition — zero
      existing manifests moved); the suite proves the guard binds in both
      directions **on every page, not just the first**, and that deriving
      `list` did not loosen `count`'s pre-reduce gate (§2.1 rule 4's
      pairing, which the effective-cost class work leans on).
      *(Original question, for history:)* Whether keyset
      paging is a row bound is a language decision about a shared,
      security-bearing gate: relaxing `ordo_plan::cost::row_bound` to
      accept it relaxes it for every authored op at once. Until it is
      ruled, `list` cannot derive without either `unbounded` (which §5.7
      rule 7 forbids std in terms) or a `take` (which would break §1's
      "iterate to exhaustion" promise and put `list` and `count`
      permanently out of the agreement §2.1 requires). Note this also
      makes `std.E.count` derivable while `list` is not, which is odd but
      is what the gate says; and see AA-018, which argues `count` should
      not pass either.
- [x] **9l · Field-scope manifest and release set.** The manifest now says
      WHICH FIELDS: `ordo_plan::artifact::op_scope` emits one entry per
      entity — read fields, written fields, tree structure, and how the op
      reached it (`body` / `guard` / `traversal`) — and `entities` is a
      projection of that list rather than a second walk. For a **read** the
      derivation is the **bound IR** and nothing else, because binding is
      what flattens composition and inlines guards: every base column the
      plan touches (projection, filter, sort, group, join key, guard walk,
      blind-index comparison) is a `Col` in one query tree, so post-pruning
      (C116's `vendor`) and guard reads (GC080's `projectMembers.{project,
      member}`) fall out with no special case. For a **write** it is the
      statements plus `guard_filter_query` for BOTH verbs — the write path
      evaluates the read guard too, to tell `not_found` from
      `access_guard`. `id`/`rev`/`$space` are excluded (they are the
      platform's, and every semi-join and tiebreak reads `id`);
      `$path`/`depth` set `tree_structure`, which is C117's
      `categories.(tree structure)`. **The release set** is the same idea
      for keys: `op_release` names every key a run needs — the `dek` opens
      of `Query::host_open` (stage `host`, carrying the output column), the
      **blind-index** digest sites `host_open` does not carry (stage `sql`,
      a different key on a different rotation), and the write side's seals
      and key digests (stage `write`) — plus the `purpose` the lease is
      taken under. A test proves the release set is a subset of the scope,
      which is DL §8's rule ("release is derived from op access and nothing
      else") made checkable. **Grants pinned to the old manifest are dead**
      — `grant_hash` digests the manifest, so a re-shaped manifest fails
      `ManifestChanged` and a human re-consents. That is the property, not
      a side effect: consent was to a document, and the document changed.
      `ordo_host::manifest` is now a **projection of `op_artifact`** (9m's
      owed reduction, made), withholding the planned SQL, rendering the row
      bound as prose and flattening `flags`; `catalog_prose`, `ordo check`
      and the playground render the same scope through one renderer.
      Two findings fell out of the field grain and are recorded as
      divergences in the guards corpus rather than papered over: a dot-path
      target's read guard does **not** inline (GC007's R-G6 stacking —
      `taskSheet` LEFT JOINs `projects` with no guard on the join), and
      GC048's `boardsAccess.{…, level}` contradicts GC080's own
      post-pruning rule for a read op.

      **GC048 is retired, 2026-09-26, by fixing the document.** The engine
      had always followed GC080 — the conformance assertion has never
      included `level` — so `guards-conformance.md`'s GC048 prose was the
      wrong half and now drops it. `corpus.md`'s table is down to two.

      **GC007 is located and is a semantics question, not a patch.**
      `ordo-sema`'s `entity_source` pushes the base entity's guard into
      `q.filters`; `walk_refs`, which handles each dot-path hop, pushes a
      bare `Join` and the `Join` IR carries **no guard field at all**. So
      the target entity is read unguarded, exactly as R-G6 forbids. The fix
      is not simply "push the guard like the base entity does": a dot-path
      is a LEFT JOIN, so a guard in `WHERE` turns it into an inner join and
      the **row vanishes**, where the same condition in the join's `ON`
      leaves the row with a **null** field. Those are different products.

      **Ruled 2026-09-26: the `ON` clause. The field nulls and the row
      stays.** *"take reccomendaton on dotpath."* The rule it keeps is that
      a guard decides what you may read, not whether a row you already may
      read exists — and it is the answer that fails visibly, because a blank
      field is a support question where a silently missing row is a bug
      report about lost data. The cost is recorded rather than glossed: a
      null is an existence disclosure, which is the same one the platform
      already accepts in treating a space id as leakable. Full ruling and
      the five things implementing it owes are in `corpus-rulings.md`.

      The build is still open, and **checking it turned up why it has
      stood.** The mechanism is small and the placement is now settled: the
      `Join` IR gains a guard field (it cannot express one today), the
      renderer appends it to the `ON` clause — which is where that renderer
      *already* puts the space predicate, for the same stated reason, *"a
      WHERE predicate on the right-hand alias would discard exactly the rows
      the outer join preserved"* — and `taskSheet`'s scope manifest gains
      `projects.owner` as a real guard read.

      **What blocks it is the fixture, verified entity by entity.** Every
      dot-path in `market.ordo` reaches a target whose read guard is either
      *implied* by the source's or *absent*: `tasks.project` is implied
      (task-readable means you are a project member, which satisfies
      `projects`' own disjunct), `taskNotes.task` and `cards.board` are
      `owned` so the guard is inherited rather than stacked, and every other
      ref points at `members`, which carries `guards: None`. So **no caller
      exists who may read a row and may not read what it dots into**, and the
      falsifying case cannot be written against this fixture at all. That is
      the honest reason GC007 is a recorded divergence rather than a fix: not
      that nobody wrote the test, but that the fixture cannot hold it. The
      table is in `corpus.md`.

      Building it therefore starts with **an entity whose read guard is
      independent of its referrer's, reached by a non-`owned` ref**. Adding
      one to the shipped fixture is not free: `gc_support`'s module doc
      records that the guards corpus registers its ops separately precisely
      to avoid moving numbers other suites assert about the fixture, and
      `ordo-host`'s catalog and load gate both count `f.registry.names()`. An
      entity is not an op so op counts should hold, but the schema, the DDL
      and the seeded rows move, and that wants checking rather than assuming. Still owed: `bind_op_generic` leaves
      `$caller` unbound, so an op with `$caller` in its BODY (not its
      guard) records a bind error and gets an empty field list — the op
      still serves, so its manifest under-reports.
- [x] **9m · `ordo plan` emits artifact JSON; `ordo check` takes a
      directory.** The producer is `ordo_plan::artifact` — **not**
      `ordo-host`, whose `manifest()` assembles most of the same facts.
      Every field of the artifact is compiler output, so the contract's
      definition belongs at or below the crate that produces it; putting
      it in the wiring crate would have dragged `tokio-postgres`, `tokio`
      and an HTTP stack into a CLI that reads text and prints JSON, and
      would have made a Postgres-seeded `Fixture` a prerequisite for
      describing a program that has never met a database. `set_mode` (§6
      rule 8's label) moved down with it, from `ordo-conformance` to
      `ordo-plan`, re-exported so the write protocol and `ordo-host` keep
      their one import: one classifier, or the label and the behaviour
      drift. `ordo_host::manifest` is left as it stands — its exact shape
      is hashed into capability grants and parsed by `catalog_prose`, so
      reducing it to a projection of `op_artifact` is its own change with
      its own tests. **Every op object carries the same key set**, a
      non-applicable key being `null`, because an appearing key and a
      changed value look the same in a diff. The SQL of a **blind-index**
      op is withheld (`plan.sql: null`, `cost.class: blind-index`): the
      digest is derived under a per-space key, so it is neither stable
      across custody nor publishable — a digest of a known plaintext is a
      membership oracle against the sealed column. An op that does not
      compile is **recorded** (`error: {code, message}`) rather than
      fatal; that is the gate `ordo apply` reads. The version hash is the
      digest of the op's own artifact object, so it moves when the
      contract moves and not when the declaration moves down the file.
      `ordo check <dir>` walks recursively in sorted relative-path order
      and keeps a line map, so a diagnostic reads `orders.ordo:12:5`
      rather than an offset into a buffer nobody wrote; a single file
      still works. Two fixes fell out: `ordo check` on the market fixture
      **did not run at all** (it planned every op and `?`-ed out on the
      blind-index one), and its op report is now a rendering of the
      artifact, so it cannot disagree with `ordo plan` about a cost class,
      a mode label or a guard.
- [x] **9n · Golden files.** `ordo_conformance::golden::assert_golden` —
      a string comparison and a way to rewrite one side, with **no
      snapshot-testing dependency**: `insta`'s value is a review UI and
      inline snapshots, and these goldens are large separate artifacts a
      reviewer opens and a tool consumes. Regenerate with
      **`ORDO_UPDATE_GOLDEN=1 cargo test --workspace`**, which every
      failure message repeats verbatim — a golden nobody can update
      becomes a golden everybody deletes. Covered: the market fixture's
      generated **DDL** and the `acl`-derived link entity's DDL as its own
      file (P2), **per-case SQL** for every case the slice executes, from
      the same table `corpus.rs` runs so the two sets cannot drift (P4),
      and the whole **`ordo plan` artifact** (P7). Byte-stability is also
      proven *within* a run — two independent compilations, two
      `Registry` hash states, compared byte for byte — which is the half a
      recorded golden cannot prove, since a golden recorded from a wrong
      run agrees with itself.
- [x] **9p · Creates through multi-hop inherited guards refuse — FIXED.**
      A grandchild under a guarded root was **uncreatable** — not forbidden
      to some callers, uncreatable by everyone including the root's own
      administrator, because `create_write_query` refused any `rebase`
      longer than one hop. A refusal nobody can satisfy is not a guard.
      The generalisation turns on one asymmetry: `rebase[0]` is a ref on
      the row being created, and that row does not exist, so hop zero
      cannot be READ — it can only be the value the caller **supplied**.
      Every hop above it is an ordinary ref chase off the ancestor the
      previous hop pinned. So the gate pins hop zero by the supplied
      literal, nests one `EXISTS` per hop after it, and evaluates the
      ancestor's walk at the far end, all under the caller's own `members`
      row. A supplied parent that does not exist (or lives in another
      space) empties the outermost `EXISTS`, so the guard fails closed
      rather than opening. Proven against real Postgres on a two-hop
      chain (`noteTags → taskNotes → tasks`, the fixture's guarded cluster
      plus one owned level) in **both** directions, because a
      generalisation that permits everything is worse than the refusal it
      replaced: `canWrite` lands the row, a member without it is refused
      `access_guard` *at the guard* (he can read the parent, so G12 passes
      first), a non-member gets `not_found`, and the same caller is
      permitted through one note and refused through another — which is
      the second hop deciding.
- [x] **9q · Eight more error classes with no producer.** Audited one by
      one; none is left without a producer or an explanation.
      **Built:** `caller_in_default` — the worst of them, since the
      compiler ACCEPTED `owner -> members = $caller`; `ordo-schema`'s entity
      lowering now refuses `$caller` anywhere inside a shape default
      (nested in a function too) and it fails at `ordo check` with a span.
      `caller_supplied` — the router refuses `$caller` in the envelope on
      both transports before binding or planning, 403.
      `null_key_address` — already had producers (three sites in the write
      protocol) and a wire class; verified, nothing owed.
      **Unreachable because the feature is unbuilt**, each named in its own
      suite doc. *(Update, 9e: `unbindable_write` is no longer among them —
      the write-valued `let` was built and it has a producer. The rest of
      this list stands.)* `guest_exposure_unguarded` (share/exposure
      registration and guest principals — neither the declaration nor the
      principal kind exists); `dependency_failed` and `breaking_dependents`
      (incremental registration and the deploy diff, 9m — the registry is
      built from one source text in one shot, so nothing is ever a
      previously-registered dependent); `sealed_keyset_pagination` (the plan
      artifact publishing its ordering to the transport — `OpSpec` carries
      no sort key, and the host marks every output field open).
      **Dead vocabulary:** none. Every class earns its slot.
- [x] **9r · `ParseError` carries no error class — FIXED.** It now carries
      `code: &'static str`, the same shape `SchemaError` and `SemaError`
      already had, so one `class_of` covers all three and the harness
      asserts the class everywhere it used to stamp the class it expected.
      (`gc_support::schema_with` hard-coded the literal `parse_error` on
      every parse failure, which is precisely how GC077's divergence
      stayed invisible.) The closed set is drawn from the two inventories
      and nothing is invented: **`parse_error`** (text outside the closed
      grammar — every lexer failure and all but three parser sites),
      **`invalid_guard_walk`** (a guard atom the grammar READ that R-G3
      does not admit: an operator other than `==`, an `==` against a
      `$param` that is not `$caller`), and **`bare_field_in_group`**,
      which was already the first WORD of its message and matched there —
      the exact failure mode this item names. The ruling that decides
      which is which is in [`corpus-rulings.md`](corpus-rulings.md): a
      class names what is wrong, never which pass noticed; the parser owns
      an `access` clause's SHAPE and R-G3 owns what a well-formed shape may
      MEAN. Two parse failures have no inventory class and are reported
      rather than named: **parenthesised guard groups** (legal under R-G3,
      unbuilt — no class means "legal but unimplemented"), and **`take`
      count out of range** (a literal outside its range is not a grammar
      violation, and `bound_violation` is a runtime wire class).
- [x] **9s · Mode labels come from the classifier, per statement.**
      `ordo_plan::writes::stmt_label` is the one authority — it reads rule
      8's `set_mode` and the compile-time mirror of the executor's
      `addressed_id`, so the label cannot drift from the behaviour it
      names. All four of §6 rule 1's labels now exist: `creates`,
      `converges`, `creates-or-converges` and **`bulk`**, which nothing
      could say before — every conditioned `set` printed `converges`, so a
      ranged write over a thousand rows and an addressed write over one
      were the same word. `del` is the second verb and rule 1's list is
      written for `set`, so its labels extend that vocabulary along the
      same distinction: `deletes`, `bulk deletes`, and `… subtree` where
      the cascade reaches descendants the caller never addressed. The
      artifact and the catalog carry the per-statement list
      (`statements: [{entity, verb, label}]`, in statement order) and the
      wire reports the union, exactly as rule 1 pins it — so a transfer
      that creates a ledger row and converges a task is
      `creates-or-converges + converges` with each statement named, instead
      of one word that lies about the other.
- [x] **9t · P0's two named gaps are RULED. G11 and G12 no longer carry
      ⚠.** Both rulings are in
      [`corpus-rulings.md`](corpus-rulings.md), "P0's last two gaps"; the
      engine-plan sections carry the cross-reference and DL ~130 no longer
      says "engine-pinned pending confirmation".
      - **G11 — text ordering** is **Unicode codepoint order over the
        stored NFC form**, and it is the *language's* order rather than
        the engine's: it is the only order that is a function of the value
        alone, where every alternative is a function of the value **and** a
        collation table on somebody else's release cadence (glibc has
        changed `en_US.UTF-8` across releases; ICU changes weights with
        CLDR), and keyset pagination turns that into skipped or repeated
        rows rather than a cosmetic difference. So 9i's `COLLATE "C"` is
        confirmed — but by a different argument than the one 9i used. 9i
        picked `C` because every Postgres has it; the ruling says that is
        a happy accident and the real reason is definability. The
        consequence is stated as a **commitment**, not a detail: `Z` sorts
        before `a`, Ordo does not sort text the way a phone book does, and
        a per-sort-key locale order is **refused** (it would re-import the
        versioned table, differ per caller so one caller's cursor would
        not seek in another's, and make an op's cost class depend on which
        locale was passed). The in-language spelling for a legible order
        is a stored `text` sort-key field.
      - **G12 — `hash(x)`** is the **SHA-256** digest of the value's UTF-8
        bytes, first eight bytes big-endian, **top bit cleared**; `null`
        hashes to `null`; the domain is `text` and `id`; frozen at first
        release and only ever added to. The engine's SipHash-2-4 pin is
        **withdrawn** — SipHash is not in core Postgres (and the database
        is what computes `hash(x)` in a condition), and a *fixed
        published* key removes the only property a key provides while
        adding the key to the compatibility surface. `hash()` is
        **restored** with the ruling: parser, `int` result type, lowering,
        `ordo_plan::hash64` as the reference, and a conformance test that
        runs both venues over the same inputs against a real Postgres.
      - **What P0 has left, stated honestly**, because a half-done P0
        stated accurately beats a whole one stated loosely: G16's two
        tracks still need owners (scheduling, not a ruling); the
        `arithmetic_error` wire class below is a ruling still owed; and
        the two *engine* debts the rulings surfaced (9u, and the owed
        expression indexes) are implementation with an answer rather than
        open questions.
      - *Correction (9c), carried forward.* The earlier note here attached
        G12 to 9c. It does not: G12 is the `hash(x)` algorithm, and
        division's ruling — DL §2, "Division: determinism across venues
        **[settled]**" — is settled in full. See 9w.
- [x] **9u · NFC-on-write — DONE.** DL §2's *"stored text is
      Unicode-normalized (NFC) platform-wide"* is `[settled]` and had no
      implementation, so `é` (U+00E9) and `e` + U+0301 sorted apart, compared
      unequal, and could both occupy a `unique` column. **Postgres does not
      do it for us** — asserted first, so the rest is not theatre.
      **Three doors, because there is no single funnel below them.** The
      **lexer** normalizes every string token as it is built, so a literal is
      NFC from the moment it exists and two editors disagreeing about how to
      spell `é` produce the same program. **`ordo-api`'s door** normalizes
      caller values — needed because a *read* filtering on a decomposed
      argument must match the composed row, and normalizing on write alone
      would store one form and compare another, fixing nothing. **The write
      path's SQL funnel** (`sql_str`) is defence in depth at the boundary the
      spec's word "on write" names: the conformance harness does not go
      through `ordo-api` at all, so a guarantee living only at the door would
      be untested by the suite that proves the platform's behaviour — which is
      how a settled rule came to have no implementation for this long.
      **Order is load-bearing at the door**: NFC changes the character COUNT,
      so `text(n)` is checked *after* normalizing, or a value that fits is
      refused depending on how the caller's editor encoded it.
      **NFC, not NFKC** — NFKC folds `ﬁ`→`fi` and `①`→`1`, which is a
      different text, not another spelling of the same one. Normalization is
      total and cannot fail, so no error class was minted.
      **The migration hazard is real and is now probed rather than hit**
      (`ordo_exec::nfc`): normalizing existing rows can merge two rows a
      `unique` column keeps apart, and a migration that converts in place
      discovers that by hitting the constraint with half the table done.
      `unnormalized` counts the work; `collisions` names the values that
      would merge, **before anything is written**. It reports and never
      repairs — which row survives is the deployment's decision, and a probe
      that took it would be making it for them. Proved against real Postgres
      including the safe case, so the probe is useful rather than merely
      alarming.
      Found on the way: `ordo-commandlog` documented its strings as
      "NFC-normalized upstream" with **no upstream**, so one chain hash could
      differ between two deployments handed the same text in two encodings.
      The upstream now exists and is named; the log still does not normalize,
      because a log that rewrote what it was handed could not be an audit
      spine.
- [x] **9v · Expression-function argument typing — DONE, and the closed
      inventory gains `argument_type_mismatch`.** `lower(price)`,
      `length(created)`, `trim(count)` and `hash(price)` bound, planned and
      emitted SQL Postgres would refuse. Ruling in
      [`corpus-rulings.md`](corpus-rulings.md); class recorded in
      composition-conformance.md §0.
      **Not an existing class**: `param_type_mismatch` is a caller binding
      into a callee's declared parameter (composition), `invalid_binding_expr`
      is a write's right-hand side. Applying a function to a value is neither,
      and stretching either name would make it stop predicting where to look.
      **The load gate is not weakened** — F6 caught all four, which is why
      `hash()` was restorable without inventing anything. But it caught them
      as *"the database refused to describe this op's plan"*, about a
      statement rather than an author's mistake, and at boot, in an op nobody
      may have touched. Binding is where the author's line is still in hand;
      a test asserts the refusal is **not** the load gate answering.
      **`mod`'s domain was wrong in the first draft and the error is
      recorded rather than quietly fixed**: reading DL §3's "integer" as a
      type excluded `money` and broke `decimal.rs`. That word describes the
      *division's semantics*; money **is** an integer count of minor units,
      so `mod(price, 100)` is the cents part. `decimal` stays out.
      The discipline throughout is that **a false positive is worse than the
      gap it closes** — a missing check fails an op at load with a poor
      message, a wrong check refuses a correct program. So
      `least`/`greatest`/`coalesce` constrain only **agreement**, and the
      numeric tower counts as agreeing.
- [x] **9w · The arithmetic class — RULED, and P0's rulings are complete.**
      One class, `arithmetic_error`, **400 for `/ 0`, `mod(a, 0)` and
      `int`/`money` overflow alike**, now in `api-engine-design.md`'s closed
      inventory. Reasoning in [`corpus-rulings.md`](corpus-rulings.md). One
      class because there is no repair for an overflow that is not also the
      repair for a division by zero. 400 **not as an attribution of fault**:
      the tempting split — caller-supplied divisor is 400, overflow out of
      stored rows is 500 — has real precedent in the same table
      (`null_key_address` is *"request-shaped, so 400: no stored data decided
      it"*), and is refused because the platform **cannot tell the two
      apart**: 22012 says nothing about whether the zero came from a literal,
      a parameter or a column, and re-deriving provenance from a plan that
      already failed would be wrong in exactly the interesting cases. A code
      sometimes wrong about fault is worse than one consistently modest, so
      400 asserts *this request has no answer, pose a different one*.
      The mechanism needed no change — it was whole, with both modes reached
      against a real Postgres — but the class was **absent from the closed
      inventory**, and an inventory that does not name a class the engine
      emits is not closed.
      **Found by making that self-enforcing** (`ordo-api/tests/
      inventory_is_closed.rs`, two-directional, statuses compared too):
      `log_append_failed` was a **second instance of the identical defect** —
      a 500 a caller can receive, with a producer and a wire code, in no row
      of the document — and `serialization_conflict` was the mirror image,
      still marked *[proposed] … not yet in `ordo-api`'s `ErrorCode`* long
      after it was built. Both fixed. The specified-not-built allow-list may
      only shrink, and a name that gets built must leave it.
- [x] **10a · R-G14's level ordering — CLOSED.** Every derived guard now
      tests **"at or above"** a rung of the declared ladder rather than
      "equal to the top". The rungs are **positional**, because DL pins the
      order and leaves the names to the app: bottom = read, second =
      write, top = admin (clamped, so `acl (read, write)` still means
      "read reads, write writes"). The test compiles to the **closed
      disjunction of `==` over the qualifying levels** DL §acl prescribes,
      which is how an ordering fits inside G1's closed grammar —
      `boardsAccess(member == $caller and level == "write" or member ==
      $caller and level == "admin")`, one indexed semi-join, rendered in
      full in the shape summary. The link entity's own write guard stays
      **top-level only**: that is the meta-permission, and it is the one
      place the ordering deliberately stops short. **Ruling recorded** in
      `definition-language.md` §acl for ladders longer than three, which
      the spec left open: the second rung is the write threshold, so a
      four-level `acl (read, comment, write, admin)` lets `comment` write.
      An author who wants a rung that reads and does not write puts it
      first.
- [x] **10b · The guest auto-grant — CLOSED.** The bootstrap now asks the
      one question rule 4 asks: does the caller hold a `members` row **in
      this space** with a non-null `role`? A guest (null role) and a
      routine/system/source principal (no row at all) both answer no from
      the same query, so the two rules cannot drift apart. **The create
      still applies, and nothing is granted** — GC092's own words are
      "applied … and zero access rows are minted". Refusing the create
      would be a second, undeclared authorization check that the door
      already owns, and it would refuse rule 5's routine-created rows too,
      which the spec expects to land ungranted. The row is not an orphan:
      bootstrap rule 6 makes the space owner the repair path.
- [x] **10c · The derived link entity's constraints — CLOSED.**
      `unique (<root>, member)` and `level: enum(<the ladder>)` are both
      derived now. The unique index follows the `$space` shape that landed
      this week — `UNIQUE ("$space", "board", "member")` — because two
      tenants must be able to hold the same pair. The enum is a `CHECK` in
      the DDL **and** a write-path refusal (`param_type_mismatch`, the
      existing wire class for "a binding expression's value does not have
      its declared type" — no new class minted). The ladder being closed
      is what makes 10a's ordering **total**: a guard that enumerates the
      qualifying levels would silently *exclude* `level: "root"` rather
      than refuse it, which is the quietest possible authorization bug.
- [x] **10d · An anonymous addressed write now answers `not_found` —
      CLOSED.** The refusal itself stays, and for a reason that has
      nothing to do with guards: a committed write appends a
      `CommandGroup`, a group carries an `Actor`, and there is no
      anonymous `Actor` — an unattributable entry in the audit spine is
      worse than a refusal. What moved is the **class**, which is now
      derived from the statement's own mode and the entity's guard rather
      than decided before the op is looked at: an **addressed** write on a
      **guarded** entity is `not_found` (R-G12 makes every guard false, so
      every row is outside an anonymous caller's read view, and R-G10 then
      requires an answer indistinguishable from absence); everything else
      — create, bulk, unguarded — stays `access_guard`, in R-G12's own
      words because "there is no addressed row to be coy about, and the
      guard simply evaluates false".
      **Decided from the schema, with no query, and that is the point.**
      R-G12 makes the guard false for *every* row by rule, so no row's
      existence can change the answer — which is the strongest form of the
      non-oracle property available, because there is no query whose result
      could differ between an id that exists and one that does not. The
      database is never consulted, so it cannot leak by result, by row
      count, or by time. Proved in both directions (`task_1` and
      `task_404`, byte-identical) in GC072's write half, now promoted out
      of `DIVERGENT` into real assertions, and in `corpus_stage5`.
      - *Left as it stands, and stated.* An anonymous **bulk** write is
        refused `access_guard` rather than being the vacuous no-op R-G9
        describes for an empty write view. R-G9's no-op is the right
        answer on a guarded entity, but a bulk write on an **unguarded**
        one would then actually apply and need an `Actor` — so the
        refusal is the attribution rule, not a guard mistake. Making it a
        no-op needs an anonymous `Actor` (or a rule that an empty group
        needs none), which is a command-log decision, not this item's.
- [x] **10e · `not_found` is one message, literally — CLOSED.**
      `ordo_conformance::write::NOT_FOUND` is the single producer of the
      class; every call site returns that exact string. The old message
      echoed the addressed id, which is the caller's own input and
      discloses nothing — but GC089 says "same message", and "same
      message after you mask the part that differs" is not that.
      - **The near-miss the sweep was told to look for, and it was real.**
        The message also named the **entity**, and that is not always the
        caller's own input. A statement that can fail `not_found` in two
        places — the addressed row and a supplied parent — told the caller
        *which* of the two was unreachable, and therefore that the other
        one was fine. That is an existence answer about a row, reached
        without ever seeing it. Length is now asserted equal too, since a
        length is an oracle in its own right.
      - **Where the id should live instead**, stated rather than deleted
        silently: the record. `ordo_secevent::RecordedDecision` already
        keeps `GuardRejected` and `RowAbsent` apart precisely because the
        wire erases the difference (R-G10's "the wire is uniform; the
        record is not") — but `StreamEntry` is documented as payload-free
        and carries scope, subject, op, decision and correlation ids, and
        **not** the row. So an operator can see that a caller was refused
        and why, and cannot see which row. That is owed work on
        `security-logging-design.md`'s surface, not a reason to keep the
        id on the wire.
      - **Deliberately not unified:** `ordo-exec`'s undo path
        (``not_found: `x` (e) is gone, so there is nothing here to
        revert``). Different audience and different contract — the caller
        holds the command group, which already proves the row existed, and
        the message is an operator diagnostic reached through `ordo-cli`.
        Flattening it would cost a real diagnostic for no disclosure
        gain. Noted here so the next reader knows it was considered.
      - **Found next door, and fixed with it:** `plan_addressed_del` ran
        `refuse_tree_parent` and `refuse_with_referrers` **before** the
        two visibility checks, so a row the caller could not read but
        which had children or referrers answered
        `tree_has_children`/`has_referrers` while an id that never existed
        answered `not_found`. Two ids, two classes, and the difference is
        a fact about a row the caller may not see — the enumeration oracle
        R-G10 exists to close, through the one verb GC089 does not
        exercise. `plan_subtree_del` already had the order right.
        Regression: four callers, four answers
        (`an_addressed_delete_decides_authorization_before_structure`).
      - **GC089's timing assertion landed too**, and deterministically:
        the document asks that forbidden and absent be cost-close because
        the guard semi-join runs whatever the outcome. A wall-clock band
        on a shared cluster measures the cluster, so the suite records
        every statement the protocol issues and demands the two runs match
        statement for statement. A short-circuit ahead of the guard shows
        up as a missing statement rather than as a flaky millisecond.
- [x] **10f · Three error classes disagree with the corpus — CLOSED, the
      document winning all three.** All three are promoted out of
      `DIVERGENT` into real assertions. GC077 and GC100 looked like they
      pulled in opposite directions; under the boundary ruling (9r,
      [`corpus-rulings.md`](corpus-rulings.md)) they are one rule applied
      twice. **GC077** — `length(title) > 3` is a readable atom saying
      what R-G3 excludes, so the walk can be named:
      `invalid_guard_walk`. **GC100** — choosing between `open` and a walk
      is a SHAPE decision taken before any walk is read, so `open` is a
      keyword inside an `access` clause and cannot be an operand:
      `parse_error`, exactly as the withdrawn `access write true` already
      was. The old answer was the schema builder's "`announcements3` has
      no field `open`" — a message about a field the author never meant,
      and one that would have gone on being right the day somebody
      declared a `bool` called `open`. **GC088** — the class of a miss is
      decided by the POSITION that named the field, not by the lookup that
      failed, so a projection (dotted or not) is
      `projection_unknown_field`; `unknown_field` is internal vocabulary
      that appears in **no** closed inventory and must never be what a
      conformance case sees.
- [x] **10g · Two corpus texts did not compile as written — CLOSED, the
      COMPILER winning both, and the document corrected in place.** Both
      share one shape: the expected class was right and the source carried a
      **second, unrelated defect** an earlier gate answered first, so the
      case never reached the gate it was written to test. No expected
      outcome moved. Reasoning in [`corpus-rulings.md`](corpus-rulings.md),
      corrections recorded in guards-conformance.md §6.2.
      **GC033** — `openTasks` is declared `unbounded` in §3.4 and
      composition-R9 puts the acknowledgment on the CALLER, so the op is
      `unacknowledged_unbounded` before the bypass gate is reached. An op
      with two registration defects gets one class and no rule ranks a
      superfluous flag above an unacknowledged cost; the text predates R9.
      *The finding worth more than the ruling:* **GC034 and GC035 carry the
      identical defect and only GC033 was ever recorded** — the suite had
      quietly supplied the missing marker for the other two and said
      nothing. All three corrected, and the gate ordering is now held by an
      assertion rather than by prose.
      **GC074–GC078** — R-G21 does prescribe the repair, with one
      correction to the attribution: the RULE the text violates is spec
      **G9** (an `access` declaration is both verbs or neither, so `access
      read` alone is `incomplete_access_clause` at the declaration), and the
      sentence supplying the repair — "the symmetric case … is the same
      error and the same fix (`access read open`)" — lives in G9's own
      paragraph in DL and is restated in R-G21's territory at GC100. All
      five now spell `access write open`, which adds nothing to what they
      assert. This included GC077, promoted out of `DIVERGENT` days earlier
      and until now asserted against a text that could not reach it. GC079
      needs no repair (`access admin` fails at the verb). A sweep found no
      other half-guarded entity in any doc except two labelled *does not
      compile* on purpose.
- [x] **10h · The derived admin ops — CLOSED.** `grantBoardsAccess`,
      `revokeBoardsAccess` and `setBoardsAccessLevel` are **derived from
      the declaration** (`ordo_schema::acl_admin_ops`) and registered
      alongside authored ops, through the same registration gate; the
      fixture declares none of them and the test asserts it does not.
      Shapes: grant is `set <link> { <root>: $, member: $, level: $ }` —
      **converge-or-create on `unique (<root>, member)`**, mode label
      `creates-or-converges` (R-G17), so regranting an existing holder
      converges their level instead of minting a second row. Revoke and
      the level change address by that same key. Depends on 10a and 10c
      exactly as the checklist said: without the ordering a level change
      is meaningless, and without the key there is nothing to converge on
      and no way to revoke a grant that is really one row.
      An op name `acl` derives is now `reserved_derived_name`, the same
      rule the derived entity and field names already had.
      **Two remainders, named rather than left to discovery.** (1) R-G17's
      *surviving-grant-path report* on revoke is not derived: it needs the
      group expansion DL §3 describes, and nothing expands groups yet, so
      the report would be empty for the wrong reason. (2) `del` gained an
      **addressed-by-unique-key** form so revoke can answer `access_guard`
      / `not_found` instead of a vacuous ranged no-op (GC044's second
      half). A conditioned `set` over a unique key still ranges: DL's
      identity precedence is about the braces of an unconditioned `set`,
      and R-G17 says "addressed delete" and nothing about the level
      change.

---

## 11 · Audit-board findings (`audit-board-coverage.md`)

Settled specification with no board row and, in each case, no code. The
audit's numbering is kept so a row can be read against its finding.

- [x] **11-8 · Effective cost class per request — CLOSED.** Finding 8:
      `api-engine-design.md` §7.1 and `std-library.md` §2.1 rule 4 are both
      `[settled]`, `ordo-api/src/refine.rs` deferred the computation to a
      planner that had none, and `refinement_unbounded` was documented in
      the wire inventory with **no producer** — the class-with-no-producer
      shape this repo keeps finding. The exploit was real: refine a
      reduce-shaped op on a sealed field and the request opens every sealed
      payload in the space, past the `unacknowledged_unbounded` gate the op
      never carried.

      **One cost model, extended, not a second one.** `ordo_plan::cost`
      gained the request-time half beside the registration gates it already
      owns: `CostClass::join` (§7.1's `⊔`, and the ordering `classify_with`
      already implied), `refine_field_class` (open → `sql`; `sealed indexed`
      equality → `blind-index`; sealed with no index → host-side), and
      `refine_candidate_bound`. That last one is the whole finding in one
      function. **`row_bound` answers what a plan RETURNS, and a refinement
      is applied to what it EXAMINES** — the same distinction `check_venue`
      makes about `take`. For a row-shaped op the two agree, because §7's
      post-take refinement filters the op's own result. For a **keyless
      reduce** they do not: `count` returns one row, registers with no
      acknowledgment, and refines *before* the reduce over the entire space.
      That gap is what the rejection protects, and it is the one §7.1 names.

      **Both halves, because §7.1 says either alone leaves the hole open in
      one direction.** The declaration-time half existed in
      `refine::validate` and was **inert**: `ordo-host` built every output
      field as `FieldSpec::new` — open, refinable, `sql` — so nothing the
      platform produced could tell a sealed field from an open one, and
      `ordo_schema::Field` dropped the parser's `refinable` marker on the
      floor. Both are fixed: the marker reaches the schema (with
      `refinable_without_sealed` for the decorative case), and `build_specs`
      derives `sealed` from the plan's own `host_open` list, `refinable`
      from the declaration, and every cost fact from `ordo_plan::cost`.
      That also disarmed a landmine: with the old manifest, the day an op
      declares `refinable: basic`, every sealed field in it would have been
      refinable with the full operator set.

      `ErrorCode::RefinementUnbounded` (400, door-only, naming the field)
      came off `inventory_is_closed.rs`'s `SPECIFIED_NOT_BUILT` list, which
      is now **empty** — the test asserts a built class may not stay on it.

      Proved in both directions, because a refusal-only test is half a test:
      `ordo-host/tests/cost_class.rs` (real Postgres, `ordo_cost_*`) refuses
      the sealed refinement on the count pairing, serves the identical
      request on the op that carried the `unbounded` acknowledgment, serves
      an open refinement on the same op with the count coming back from
      Postgres, and serves the same sealed condition on the row-shaped op
      §7.1 carves out. `ordo-api/tests/api.rs` adds the door's answers and
      the two-gates-are-different assertion; `ordo-plan/tests/refine_cost.rs`
      the arithmetic.

      **Remainders, named rather than left to discovery.**
      (1) *No op-level `refinable` marker exists in the language*, so
      `build_specs` still registers every op `RefineTier::None` and no
      refinement reaches the door in a real deployment — "nothing is
      refinable that the op author didn't ship" is honoured by shipping
      nothing. The gate is in place for when the marker lands; the host
      suite switches Tier 0 on explicitly and says so.
      (2) *The effective class is computed, enforced, and not traced.*
      §7.1 puts it on the `ordo.cost_class` span attribute beside the
      registered one; there is no tracing substrate (`ordo-host::metrics`:
      "No traces"), so `refine::effective_class` is public and unread rather
      than half-wired into a counter that answers a different question.
      (3) *The executor still ignores `$refine` entirely* — the envelope is
      validated, priced, and passed to an executor that does not apply it.
      That is a separate unbuilt half (host-side filtering, 5c's own
      remainder), and this gate prices what the design says the executor
      will do, not what it currently does.
- [x] **11-2a · Share links for people who are not members. Built 2026-09-29:** `std.space.shareLink`, `POST /auth/link`, revoke and list, files, SDK `openLink`; lifetime layered with no platform maximum. Settled in
      `definition-language.md` (Share links): minting is an op, logged,
      expiring, revocable, and the link is a bearer credential so expiry is
      mandatory. **Checked 2026-09-29: not built.** No op mints a link and
      nothing serves one; the blob side waits on `private` and presigning,
      which `crates/ordo-blob/src/s3.rs` says is not built. Filed at the
      owner's question the same day; it had no item. Board: `sharing`.

- [ ] **11-2 / 11-3 · Inter-space sharing. RULED 2026-09-08, registration owed.** Findings 2 and 3 (guest windows, consented feeds): `docs/sharing-options.html` itemizes the 22-point consent surface on both sides of both doors and offers three architectures — pull-through window, feed replication, linking space — with a recommendation. **RULED: Option A, grant-shaped, no syntax** (owner, "LEts go with your
      suggestion. Leave the syntax clean."). A guest is an ordinary member
      holding one grant, which is why no `guest` role exists and none is
      needed: `guards-conformance.md` states it as "the corpus's guest is
      an ordinary member holding one grant". The enforcement half already
      passes real-Postgres tests; **registration is what is missing**
      (2026-09-29: `register_share` exists in
      `crates/ordo-identity/src/grants.rs` since 73492f5, and nothing in the
      host or CLI calls it yet), and
      it is the prerequisite for **5g-rev**, since only a window can be
      made revocable on a device.

- [x] **11-4 · Delegation grants — the record and the door, BUILT.**
      Finding 4: `definition-language.md` §8 settles that *delegation is
      itself a grant* — user → agent, expiring, revocable, command-logged,
      **distinct from the token's op allowlist** — and `bootstrap.md` item
      10 requires a **live** one on the agent provisioning path. The code
      column read *"none — no match for `delegat` in `ordo-identity`.
      `PrincipalKind::Agent` exists; the grant that makes it lawful does
      not."*

      Built on the owner's decision, verbatim: *"Yes this product will
      offer that as a feature for the builders for ease of
      use/implementation. MCP by default — though we'll abstract it."* So
      the settled core is built here and the protocol surface is planned
      behind a seam — `docs/plan-delegation-surface.md`, six increments
      with done-whens.

      **Its own record, its own table, its own id.**
      `ordo_identity::delegation` (`DelegationClaim`, `NewDelegation`,
      `DelegationRecord`, `DelegationRegistry`) and
      `ordo_host::delegation::PgDelegationRegistry` over `"$delegation"`,
      mirroring the grant registry. Folding it into `CapabilityGrant`
      would have saved a table and cost the rule: the two records are
      consented and revoked by **different people**, so one record means a
      manager editing an allowlist edits the reach of somebody else's
      consent.

      **The one place the registry boundary moves into the request
      path.** `ordo_identity::grants` states that it is *"not consulted to
      admit a request"*; this one is, on every delegated call, and the
      asymmetry is argued rather than assumed — a capability grant is the
      agent's own authority, a delegation is *someone else's face*, and the
      person withdrawing it does not hold the token and gets no say in how
      long it lives. The cost is named: an unreachable delegation registry
      refuses delegated calls **and nothing else**.

      **The two consents INTERSECT** (`DelegationBeyondDelegator`,
      evaluated with the same `membership_reaches` predicate `callersOf`
      reports with). Without it, whoever administers the agent lists
      `space.delete`, the user consents to "act as me", and the agent
      reaches what the user never could. It also closes what would have
      been suspension's one loophole: suspending the delegator stops the
      delegation at the next call.

      **One new wire class, `delegation_required` (403), and the ordering
      IS the argument.** The door has one answer by rule (C7 / X-038), so
      the carve-out is positional: the refusal is decided by a **token
      gate** that runs before the op registry is consulted and before the
      space is bound, so every name the same token could send gets the
      identical answer and the class correlates with nothing about the
      deployment — asserted directly, over a real op, an op it may not
      call, and a name nobody registered. Had it lived inside
      `AccessChecker::check`, an unregistered name (answered without ever
      reaching the checker) and a registered one would have given a
      dead-delegation caller two different codes: an op-existence oracle.
      Everything else about delegation stays under `access_denied` and is
      four distinct `RecordedDecision`s in the space's own stream.

      **The actor taxonomy was not extended.**
      `Actor::AgentOnBehalfOf` has existed since the command log was
      written and had **no producer** — nothing knew a call was delegated.
      `ordo_host::attribution` is that producer. The delegation id stays
      out of the rendering because DL §8 pins the string verbatim and the
      chain hashes the rendering; it lives in the registry, and a chain
      entry joins to it on `(space, agent, delegator, at)` — exact only
      because a second live delegation for that triple is unrepresentable
      (`AlreadyLive`; re-consent is revoke-then-record). That uniqueness is
      what makes *"which consent authorised this write"* answerable at all.

      **`callersOf` grows a fifth via**, `delegation`, conjunctive with
      `token`: emitted only for an agent that also holds a live grant
      listing the op, and emitted **alongside** the token row, because
      stopping it is two edits by two different people.

      Proved in both directions against real Postgres —
      `ordo-host/tests/delegation.rs`, `ordo_del_*`, eight tests. Revocation
      is why the suite needs a database at all: a bearer string cannot say
      "withdrawn", so the only honest test of DL's word *revocable* is a row
      changing and the very next HTTP call being refused.

      **Three things found on the way, none of them in the brief.**
      (1) `refresh_presence` would have minted a fresh user-presence
      assertion into a **delegated** session — a constructible token shape
      that would have let a delegation satisfy the one gate the platform has
      for *not delegable*. Now refused, with a test. (2) The token payload
      is positional with a fixed field count, so adding the delegation field
      under the `v1` word would have made a truncated token read as *"claims
      no delegation"* — a silent downgrade to acting-as-itself, the
      dangerous direction; it is `v2` and `v1` is refused outright. (3) A
      partial unique index cannot express *at most one **live** delegation
      per pair*, because the predicate needs `expires > now()` and must be
      immutable; enforcing it on `revoked_at IS NULL` alone would lock a
      delegator out forever after one lapsed, unwithdrawn consent.

      **The remainder that decides whether this is useful, named rather
      than left to discovery: `$caller` inside a delegated call is still
      the agent.** The door authorizes against the delegator's reach; the
      executor's row guards do not, so a delegated agent over
      `$caller`-guarded rows currently sees **nothing** — lawful and
      quietly empty. Not changed here because `$caller` is reserved and
      router-inserted (R-G13), `caller_restricted` is pinned per entity,
      and the idempotency cache and page cursor are keyed on the door's
      resolved principal (SH-17) — four arguments, each owed its own
      conformance case. `plan-delegation-surface.md` §7 lays out three
      candidates, recommends `$caller` = the delegator, and makes it
      **increment 2** — ahead of the ceremony, the consent page and MCP —
      because everything else is polish on a feature that otherwise returns
      empty results. The door's bound was built first so the row layer has
      something consistent to move toward.

- [x] **11-4a · `$caller` under delegation — BUILT (2026-10-08).**
      `plan-delegation-surface.md` §7, recommendation (2). Done-when: a
      delegated read over a `$caller`-guarded entity returns the
      delegator's rows and **not one row more**; a guard-conformance case
      covers the "not one row more"; `guards-conformance.md` carries the
      rule; the idempotency and cursor keying decisions are written down.
      **Met.** `$caller` is the delegator; the agent rides as `$agent` for
      the command log (`agent:<a> on behalf of user:<d>`). Replay cache and
      page cursor keyed on delegator plus agent (lead's decision, on the
      board for the owner). GC101 and R-G22. Also fixed: the deployment
      surfaces, `/security/events`, the catalog and the interface now run
      the token gate, so a revoked delegation reaches none of them. Tests:
      `crates/ordo-host/tests/delegation.rs`. Guide: `howto-workers.md`.

- [x] **11-4b · The consent ceremony and its disclosure — BUILT (2026-10-10).**
      `plan-delegation-surface.md` increments 3–4. Owner's go: delegation ask
      5, *"yes"*, before the first deployment.
      **What landed.** The agent asks at `POST /auth/delegation/request`
      (no token; answers a request id and a binder, only the binder's hash
      stored) and redeems at `POST /auth/delegation/redeem` with the binder,
      one-shot. The person uses five platform ops: `std.delegation.review`,
      `.approve` (step-up, so presence; `ORDO_STEP_UP_EXEMPT_OPS` may drop
      it), `.decline`, `.mine`, `.revoke`. Only the named person may review
      or answer. All five are closed to delegated tokens, so an agent cannot
      grant, onward or to itself. `DelegationSurface` is the seam, with one
      in-process implementation (`crates/ordo-host/src/consent.rs`).
      **The disclosure is computed**: each op's reads, writes and
      guard-examined fields from its field-scope manifest, deletes from its
      statements, the term as a date, what is not granted and why, the
      closed doors, the owner-visibility line. The agent's name is the app
      developer's (`ORDO_DELEGATION_AGENTS`); op names that are not ops are
      counted, never shown. Settings: `ORDO_DELEGATION_AGENTS` (unset: no
      request accepted), `_MAX_HOURS` 8, `_REQUEST_SECS` 600, `_MAX_OPS`
      10; refusals name the setting and value. Delegation and grant are
      recorded in one act; a re-approval revokes the old one.
      **Departures.** (1) `consent_correlation` is the approval's
      security-event correlation, not a chain command group: a consent writes
      no tenant row (R4). (2) `NewDelegation` keeps public fields; the
      ceremony is the only production builder. (3) Presence is the door's
      step-up gate ahead of `accept`, not a parameter of it.
      Commits 27be5047, da698621, 4e7d8b7e. Tests:
      `crates/ordo-host/tests/delegation_consent.rs` (12),
      `assistant_app.rs` `the_manager_lends_through_the_consent_ceremony`,
      SDK `clients/ordo-sdk/test/delegation.test.ts`. Guide:
      `howto-delegation.md`. The assistant example runs the real ceremony;
      the dev grant tool is gone.

- [ ] **11-4c · The MCP binding.**
      `plan-delegation-surface.md` increment 5 — the owner's default, and
      the test of whether the seam is in the right place. Done-when: an
      agent completes the ceremony over MCP; the tool list is byte-equal to
      the scoped catalog's projection for the same token; a test asserts no
      op is reachable over MCP that is unreachable over HTTP with the same
      token. The day that last one fails, MCP has become the design.

- [ ] **11-4d · SH-20's budgets, the buildable half.**
      Destructive ops non-delegable by default (a set on the engine plus
      one check, in the shape `require_step_up` already has), displayed on
      the consent page. Rate and volume caps stay **deferred** until the
      metrics substrate can answer "rows affected per hour per delegation",
      which it cannot: a budget that is displayed and not enforced is worse
      than no budget. `delegation_budget` is minted **only** with its
      producer — the wire inventory is closed in both directions.
      **Built (2026-10-08; the display 2026-10-10 with 11-4b, where a delete
      the app developer did not open is listed as not granted, naming
      `ORDO_DELEGABLE_OPS`).** Deletes are closed to delegated tokens unless
      `ORDO_DELEGABLE_OPS` names them; platform writes are closed outright.
      Refused as `OpNotDelegable` (tag 31), answered `access_denied`.
      `crates/ordo-host/src/delegable.rs`.

- [x] **11-10 · Exit ops: full space export and space deletion — CLOSED.**
      Finding 10: `migration.md` §6 and `definition-language.md` §8 both
      `[settled]`, `licensing.md` printing export as **leg 1 of the
      three-leg trust argument**, and **no export path, `space.delete` with
      no producer**. Leg 1 was a paragraph. Design and build plan:
      `docs/plan-export.md`; rulings in `corpus-rulings.md`.

      **The requirement was dynamism, and it is enforced in four places,
      not asserted once.** The export derives everything from the schema
      and the artifact: `ordo_schema::ddl::physical_columns` is now the
      single answer to *which columns does this table have* and `ddl_for`
      iterates it, so the printer and the reader cannot disagree (two tests
      in that file check both directions — omission and invention).
      `schema.sql` and `artifact.json` are the stored artifact's own bytes,
      not a re-rendering, so there is nothing to keep in step. Value
      encoding is an exhaustive `match` on `ScalarTy` with **no wildcard
      arm**, which draws the line exactly where it belongs: a new entity or
      field flows through with no code change, and a new *scalar type in
      the language* fails the build until somebody decides what its JSON
      is. And the requirement is a test — compile, export, compile the same
      program plus a new entity and a new field, export again — asserted
      **two-sidedly**, because an export that regenerated its whole shape
      every deploy would pass a one-sided check while being useless to
      anyone diffing two bundles.

      That test cannot fail today. It is written down anyway: it is a
      *specification of the requirement*, and the day somebody hand-rolls a
      column list to fix an edge case, it is what stops them.

      **A bundle is a pure function of state and artifact.** Rows by `id`
      under the `C` collation so the database's locale cannot reorder a
      bundle between deployments, keys sorted, entities in name order, **no
      clock anywhere**. Two exports of one unchanged space are
      byte-identical. Provenance belongs to the security-event record the
      op writes, which is the artifact that is supposed to carry a clock.

      **The export refuses on a drifted database.** It builds a shadow from
      the artifact's own DDL and asks `live_drift` before the first row is
      read — the same comparison the load gate makes, asked by the other
      reader that has to trust it, and it inherits the constraint half of
      that gate for free. The alternative is a bundle whose `schema.sql`
      does not describe the rows beside it, which **still parses**. Its own
      shadow namespace, so a concurrent boot cannot drop it mid-comparison.

      **Sealed fields follow `migration.md` §6 as written** — opened under
      the owner's authority through the normal key-release path, which is
      *why* the op is step-up gated. Ciphertext mode is §6's second clause,
      for a deployment holding no key. Three consequences ruled: an
      unopenable value **refuses** rather than exporting a null (a bundle
      that under-delivers while looking complete is worse than one that
      says what it cannot do); `$bidx` columns are omitted and declared,
      because a keyed digest is not data and shipping it hands the reader
      an equality oracle over the plaintext beside it; and **export is a
      disclosure format, not a backup format** — re-sealing under a
      destination key is `ordo import`'s job, which is unbuilt.

      **Deletion destroys the SMK, not only the DEKs**, and the argument is
      the whole ruling: blind-index keys are SMK-derived and
      epoch-independent by design, so a surviving SMK leaves a backup's
      `$bidx` columns answerable by anyone who can recompute
      `HMAC(k_field, guess)`. Erasing only the DEKs answers *what was the
      value* and leaves *was the value X* answerable. `destroy_space` /
      `drop_space` are **required** trait methods, not defaulted: a default
      gives every wrapping provider the wrong behaviour, and a wrapper that
      forgets to forward is a deletion that reports success and erases
      nothing. Keys go **before** rows, because a partial failure should
      land on the stronger guarantee.

      **What refuses and what merely discloses.** A non-evictable consumer
      behind the head refuses (`space_pinned`, 409) — archive is the copy
      of record, a client-first device is the authoritative store, a
      listener has at-least-once deliveries outstanding — which is
      `prune`'s own `evictable` split read once more. The **undo horizon is
      disclosed, not enforced**: it is the owner's guarantee and deletion is
      the owner spending it, so refusing would make the platform the owner
      of their data's end. The confirmation is the space's own id typed
      back (`not_confirmed`, 400) — not a boolean a retry supplies for
      free, not a token the platform issues.

      The sweep discovers space-scoped platform tables from
      `information_schema` rather than listing them (the test creates a
      table this codebase has never heard of and asserts it is swept), and
      runs as **one statement** of data-modifying CTEs so the FK ordering
      problem stops existing rather than being solved. `$secevents` /
      `$seccheckpoints` are the one exception, with both reasons on the
      constant and the residue they keep stated rather than hidden.

      **"Command-logged" turns out to mean the audit spine, and the spine
      has two halves.** §8's word cannot be literal here: the command log
      records writes to entity rows, and export mutates nothing while
      deletion destroys the chain. Forging a `Converge` with equal images
      to record a read would put a lie in a chain whose whole value is
      literalness. Both ops record `CustodyAction` in the security-event
      stream instead — beside epoch rotation and epoch destruction, which
      are the acts they are actually siblings of — and that is why the
      erasure retains that stream.

      Owner-only, and the registration loop was **split** to make it so:
      `platform_ops::owner_only` owns the policy beside the ops, because a
      blanket `Manager | Owner` loop would have made `std.space.delete`
      manager-reachable the moment it was added, in silence. Step-up is
      also what makes both structurally undelegable — a capability token can
      never satisfy it — and the door suite asserts that with the strongest
      capability token that can exist.

      Three wire classes with producers and rows in both inventories:
      `schema_drift` (500 — the class `LoadRefusal::SchemaDrift` never had,
      because it refuses a load and never met a caller), `space_pinned`
      (409), `not_confirmed` (400). The migration work's report classes were
      checked first and are owned by neither inventory, so none was
      reusable. `sealed_open_failed` is reused for both custody failures.

      23 tests over real Postgres (`ordo_exp_*`): 10 export, 7 erasure, 6 at
      the door. The erasure suite's load-bearing one holds a sealed envelope
      **outside** the database — which is what a backup is — shows it opens
      before and not after, then re-creates custody for the same space id
      and shows it still does not open. A `DELETE` proves none of that.

      **Two hazards found that nobody was looking for.** (1)
      `Pool::build_shadow` set `search_path` on a **pooled** connection and
      never reset it; the connection went straight back into the pool, so
      the next unrelated query would have resolved `orders` to the shadow's
      empty copy — a read returning nothing, correctly typed, with no error
      anywhere. The only existing caller dials a throwaway pool, which hid
      it rather than removing it. (2) A bundle **was not loadable from
      itself**: members are in name order so a bundle is diffable, but every
      ref is an FK on `($space, id)`, so an importer following bundle order
      hits `orders` before `vendors`. The manifest now carries a
      topological `load.order` plus a `cyclic` list. Found by the round-trip
      test, which follows the manifest and nothing else.

      **Remainders, named rather than left to discovery.** (1) ~~*Blob
      bytes*~~ — **closed by 17g.** The bundle carries them at
      `blobs/<entity>/<field>/<id>`, which is `migration.md` §6's layout,
      and the manifest's `unbuilt` list is empty. The list itself stays: it
      is the machine-readable place a future gap is declared. The manifest
      also gained per-member `digests`, which `plan-production-ops.md` R1
      found missing — a restore marker cannot attest a bundle it cannot
      name. On 2026-09-26 it also gained `blobs.files`: one entry per object with
      its entity, field, id and its **mime where the member holds
      plaintext**. That last field was owed to import rather than chosen
      here: the mime is a sealed envelope on `$blobref`, so it was nowhere
      in the bundle, and an importer could only have defaulted every file to
      `application/octet-stream`. 11-10a's blob leg is the other half of it.
      (2) *Delivering the bundle* — the op writes to a
      configured directory and returns the path; a presigned URL, a stream,
      or `ordo export --into` is unbuilt, so a self-hoster has the bytes and
      a hosted tenant does not yet. (3) *BYO-DB* — the ops run against
      `Pool`, so a BYO-DB space exports and erases through the identical
      path with no branch, which is why there is no BYO code here; what is
      **not** covered is that a customer's own backups of their own
      database are outside any op's reach, and crypto-erasure is what makes
      that survivable for sealed columns and nothing makes it survivable
      for plaintext ones. That belongs on the BYO-DB consent page and is not
      there. (4) *The confirmation window* §8 names (a delay with a cancel)
      needs pending-deletion state and a job. (5) *Legal hold* has no
      producer anywhere in `crates/`, so deletion cannot respect one and
      does not pretend to. (6) *`PlatformExecutor` drops the durable
      `ReplayLease`* — a pre-existing gap for any non-read platform op,
      tolerable here only because both exit ops are idempotent by
      construction, and stated in the module doc rather than hidden.

- [ ] **11-10a · Bulk import: the other direction, v1 built, and since
      2026-09-26 reachable.**

      **The door (`plan-import.md` §11).** `std.space.import` plus
      `ordo import`. Until it, every caller of `import_space` in the
      workspace was a test: 17y finding 1 had the reason right, that an
      import which cannot seal cannot write a sealed field and the CLI holds
      no custody, so the answer was the 2026-09-20 ruling rather than a CLI
      feature. The deployment holds custody, so the deployment imports.

      Owner-only and step-up gated off the same `owner_only` list as the two
      exit ops, registered exactly when `ORDO_IMPORT_BUNDLE_DIR` is set, and
      recorded in the security stream as a `CustodyAction`, because the chain
      records which rows arrived and cannot record that an owner authorised a
      bulk load out of a foreign bundle.

      **A bundle is named, never pathed**, and resolves under the **bound
      space's own directory**. That component is an authorization boundary:
      one staging root serves every tenant, and every tenant's owner passes
      every gate this op has, so a flat root would have made one tenant's
      staged bundle loadable by another into a space whose keys they control.
      `bundle_unknown` is one answer for an illegal name and an absent one, on
      `bucket_refused`'s anti-oracle argument.

      **The defect the tests exist for**, and it is silent: custody must be
      assigned to the *fixture*, not only to the `Executor`. Threading it into
      the executor alone covers every read, the rows arrive, the chain is
      intact, and the sealed values are unopenable by the space that owns
      them. Break the assignment and `tests/import_op.rs` says
      `sealed_open_failed: notes.memo did not open`. The test needs **two**
      key providers to say that; the one-provider version passes either way
      and was written first.

      New wire class `import_refused` (409), one code for fourteen classes on
      `apply_refused`'s reasoning, with the class as the first word of the
      message. `sealed_unsupported` and the blob door's own class are
      deliberately kept for the two refusals that already had a name.

      **The people map (`plan-import.md` §12), owner-ruled and built the
      same day.** An entity declaring `owner -> members` could not be
      imported at all, which is most real applications: `members` is a
      platform entity, the importer will not create one, and nothing mapped
      their ids, so the first row refused. Ruled: **a supplied map, with a
      refusal for any name it does not cover.**

      Applied by pre-seeding `("members", source) -> destination` rows into
      `$importmap` before the load order, so the ordinary rewrite resolves a
      person like anything else and `unresolved_ref` stays the refusal for
      anyone uncovered. The ruling's requirement and the existing behaviour
      are the same sentence, so no second resolution path was added.

      Three refusals enforce the two rejected candidates rather than merely
      declining them. `people_unknown`: the map may not name anybody who is
      not already a member of the destination, because a file that could
      admit a principal would be a file that grants access. `people_ambiguous`:
      a merge of two source people onto one member is refused, since two
      source ids resolving to one row make every reference to either resolve
      to the same row with nothing to show it. `people_changed`: a re-run
      under a used name with a different map would split one import's
      authorship between two answers.

      **The map is stored, not just applied**, so which people an import
      attributed rows to is queryable in the destination afterwards. Staged
      at `<root>/<space>/<bundle>.people`, beside the bundle because the
      manifest's digests make a file added inside it incomplete, and because
      the map is the operator's statement rather than part of the export.

      **Still open here:** delivering a bundle to a hosted deployment (an
      operator stages it on disk today, the same asymmetry export has);
      nothing maps `groupMembers` and nothing should, so a bundle's group
      structure does not survive and is rebuilt in the destination; and the
      map is per import rather than a standing directory of people the
      deployment knows, which would need its own ruling.

      **The engine, v1 (2026-09-20).** 11-10 closed
      leg 1 of the trust argument and left its own sentence standing:
      *"export is a disclosure format, not a backup format; re-sealing under
      a destination key is `ordo import`'s job, which is unbuilt"* (this
      file, line 3830). A 2026-09-19 review found it the single biggest
      obstacle to migrating a real application: no import, bulk-load or
      `COPY` path anywhere in the tree, and rows able to enter only through
      the write protocol one op call at a time with no tool driving that.
      Design and build: [`plan-import.md`](plan-import.md).

      **What v1 is.** `ordo_exec::import::import_space` reads an
      `ordo.space-export.v1` bundle and loads it through the write protocol.
      The input is the **bundle** rather than CSV, and the reason is that
      the manifest already carries the load order, the per-entity column
      list with its encodings, the row counts and the per-member digests,
      while the foreign-system path is blocked on the **mapping file**
      (`migration.md` §2) rather than on a loader. Building a CSV loader
      first would mean inventing the mapping format inside it.

      **The batch group needed no ruling and it was checked before
      anything was invented.** `CommandGroup.commands` is a `Vec` and
      `validate` asks only for non-empty and single-space, which is
      `migration.md` §3's *"Batch = one command group … inherited, not
      re-invented"*. `WriteLog::open_batch` pins one correlation and
      `stage_group` extends the pending group, so N calls are one group and
      one chain entry. **The shared correlation is load-bearing**:
      `$sealedpayload` rows are stamped with their group's correlation and
      collected by the anti-join over correlations no longer in the chain,
      so merging groups after the fact would have made every sealed value in
      a batch collectable.

      **Images are full per-row, which is the 2026-09-08 backfill ruling's
      option 1 rather than its default.** Summarised groups grow the frozen
      chain encoding, and the precedent for growing it (the restore marker,
      the owner-administration trailer) is a ruling, a by-presence trailer
      and a pre-change golden. Deciding that inside a first import slice
      would decide it by accident. The cost is stated: a million rows are a
      million commands and, at batch 1000, a thousand chain entries.

      **The 2026-09-20 ruling lands on the same side and makes the deferred
      work cheaper, which `plan-import.md` §3.2a records.** *"Copying every
      row to make a backfill revertible is refused as the default"*, and a
      summarised backfill gives up the prior values. Its scope is a
      backfill, which transforms rows that exist; import creates rows that
      did not, and **a create has no prior value to lose**. So a summarised
      import group gives up strictly less than a summarised backfill, and
      summarised is where import should end up. Option 1 is the first
      slice, not a preference.

      **Measured, not promised.** 2000 rows, debug build, local socket:
      199 rows/s at batch 1, 3220 at 100, **3705 at 1000**. The batch group
      is worth 18x, and a million rows is about four and a half minutes.
      `tests/import.rs::how_fast_one_import_is`, `#[ignore]`d because a
      threshold would fail on a loaded laptop and prove nothing.

      Ids stay **server-minted, always** `[settled]`; `$importmap` carries
      the source key and the ref rewrite runs through it in the manifest's
      load order, which is what makes that order load-bearing rather than
      advisory. A tree's rows are ordered parents-first inside their own
      file, which `plan-export.md` §2.2 hands to import by name. Resume
      rides the **existing** `$replay` lease, one deterministic key per
      batch fingerprinted by its source ids. **This sentence went on to
      say "so a changed bundle under a used name refuses instead of
      skipping", and that is false when the ids are unchanged and the
      values are not** (`security-review-6.md` S6-006, 2026-09-28): the
      fingerprint covers ids only. Refusals: `format_unknown`,
      `bundle_incomplete`, `artifact_mismatch`, `field_mismatch`,
      `sealed_ciphertext_unsupported`, `space_not_empty`, `cyclic_entities`,
      `tree_cycle`, `unresolved_ref`, `row_refused`, `unencodable_value`,
      and, since the blob leg below, `blob_door_missing`, `blob_unplaced`,
      `blob_tier_unsupported` and `blob_refused` in place of the retired
      `blob_unsupported`.

      **One rule the build had to restate.** "The destination must be
      empty" refused the exact case resume exists for, because an
      interrupted run leaves rows. It is now stated against `$importmap`
      under this import's name: this import's own residue is resumable,
      anything else is the merge nobody has ruled.

      **Two defects found, both unexecuted before this, both now fixed
      (2026-09-20).** (1) **`std.<entity>.create` refused every row of a
      keyless entity.** `std_ops` put `rev: null` in the braces
      unconditionally and DL §6 rule 9 refuses a belief about a row the
      statement does not address. Fixed in `ordo_schema::std_ops`: the
      belief now rides only when the entity has a unique key
      (`unique_keys == 1`); a keyless entity's derived `create` carries no
      `rev` at all and is an ordinary insert every call
      (`ordo-conformance/tests/std_ops_keyless_create.rs`). `import.rs`'s
      own `import.<entity>.create` is **not** retired by this: it omits
      `rev` for every entity, keyed or not, which now differs from
      `std.<entity>.create` on a keyed entity's within-bundle duplicate key
      (converge silently vs. refuse `expected_absent`) — a duplicate-key
      policy question, not a mechanical consequence of the `std_ops` fix,
      and left as its own derivation with the reasoning recorded in
      `import.rs`'s module doc. (2) **A negative `int`, `money` or
      `decimal` could not be bound as a call argument.** The grammar makes
      `-2500` an `Expr::Neg` and `lit_ty` answered only for literal
      productions, so `ordo_host::call_source` could not send a negative
      money either. `fold_negation` closed it on the **write** path first;
      fixed on the **read** path too, in the binder both paths share
      (`ordo_sema::fold_neg_lit`, folding a negative `int`/`decimal`
      literal wherever a literal binds a parameter — call arguments,
      routine params, harness top-level params, and a write's own
      environment). `ordo_conformance::write`'s private copy is gone; it
      now calls the shared function (still needed there, ahead of
      `is_held_value`'s literal-shape match, which does not understand
      `Neg`). See `corpus-rulings.md` for the reasoning and
      `ordo-conformance/tests/negative_call_args.rs` for the read-path
      proof.

      **The blob leg is built (2026-09-26), and a space with an attachment
      now round-trips.** Export has carried the bytes since 17g and import
      refused them in three places, so a space holding one file could leave
      and could not come back, which is the exact half-promise `plan-import.md` §0
      and `licensing.md` are written against. Design and reasoning:
      `plan-import.md` §10.

      **A file is not a column, so it does not ride the create.**
      `Sema::check_content_params` refuses a blob-typed op parameter, so
      `import.<entity>.create` cannot carry one, and the bundle's blob
      column is the **source** space's handle (an object in the source's
      bucket under the source's key), which is dropped. The rows go in
      first, then each object is bound to the row this import minted for it
      **through the deployment's own blob door**, which is the same upload a
      client's `PUT` takes. That is not a preference: `plan-blob-chain.md`
      C1–C8 ruled there is one upload path, and
      `ordo-host/tests/upload_seam.rs` asserts it over the workspace's
      source (one mint site, one `PlatformValue` implementor, one caller of
      `attach_write`). `ordo_exec::import` is three crates below the door,
      so it declares a trait (`import::BlobDoor`) and `ordo-host` implements
      it in four lines of adaptation: the tier check, the intent row, the
      seal under the **destination's** key, the row lock, the write guard
      before and after, the digest in the image and the command group are
      all inherited rather than restated.

      **Where the attach sits**: after every row of every entity, one pass,
      never inside a batch. A batch's closure is re-executed on a
      serialization conflict so it is `Fn + Clone`, and an upload token is
      minted from bytes that were actually stored and consumed by value, so
      it is neither; the bytes also have to reach the store before the
      transaction that names them opens. **The shared correlation is not
      weakened by that**: what it is load-bearing for is that every
      `$sealedpayload` row's correlation is in the chain, and each attach is
      its own group in this space's chain. The tier whose bytes ARE a
      payload row is `blob inline`, refused by name.

      **Resume reads the effect, not a record of it.** `$importmap` is still
      what turns an object's source row id into the minted one, and without
      it an object has nowhere to go. It is not the attach marker: a mapping
      row rides its batch's transaction and has no window, while an attach
      commits in the **door's** transaction, which import cannot join, so a
      mapping row for an object could only be written after that commit and
      a crash in the gap would upload a second object over the first. The
      row's own blob column has no gap, because it is what the attach
      produced in the transaction that produced it, which is the same sentence
      `owning_row` already rests on, *"the column IS the reference"*. Safe
      because `space_not_empty` has already refused unless every row is one
      this import minted.

      **Digests both ways.** Every object is read through the check against
      the manifest's `sha256`, on every run including a resumed one, and an
      object with **no** recorded digest is refused: an object has no row
      count and no second description, so any byte string would be a valid
      file. The round trip also asserts the platform's own digest, the
      sealed `SHA-256` over the plaintext on `$blobref`, opens to the same
      value in both spaces under different keys, and that the destination's
      chain carries it as `scan$digest`.

      **The export gained one field, owed to import rather than chosen**:
      `blobs.files`, one entry per object with its member, entity, field, id
      and its **mime when the member holds plaintext**. The mime is a sealed
      envelope on `$blobref`, so it was nowhere in the bundle and an import
      could only have defaulted every file to `application/octet-stream`. It
      is absent in ciphertext mode and for `blob private`, because a bundle
      the platform did not decrypt must not disclose through the manifest
      what it refused to disclose through the member.

      **`blob_unsupported` is gone and four named refusals replace it**,
      each with a test: `blob_door_missing` (objects and no door wired),
      `blob_unplaced` (no such blob field, or a source row this import never
      mapped), `blob_tier_unsupported` (`private`, `inline`, `sealed`: the
      three the door refuses, refused before a byte is sealed), and
      `blob_refused` (the door's own class, as `row_refused` carries the
      protocol's). `bundle_incomplete` covers the manifest and the member
      list disagreeing in either direction, a miscount, a missing digest and
      a missing mime. One of the three removed refusals was **dead**: it
      keyed on `encoding == "blob"` and the export writes `blob_ref`.

      **Found outside the blob leg and not fixed here: an entity that
      references a platform entity cannot be imported at all.** `members` is
      a platform entity, import skips platform entities on purpose (a bundle
      that could create access rows would be a bundle that grants itself
      membership), and nothing maps their ids, so `owner -> members`, which
      almost every real application declares, refuses `unresolved_ref` on
      its first row. Filed on the decisions board as `platform-ref-import`
      with a recommendation, because the answer is a ruling about identity
      across spaces rather than a mechanism.

      **Deferred, all of it named in `plan-import.md` §7.** Summarised batch
      groups and the encoding ruling they need · `source:import-<name>`
      attribution, which waits on the write-only source principal · the
      **genesis checkpoint** closing an import · the **verify pass** and the
      didn't-migrate manifest, and with it continue-on-reject · **CSV/JSONL
      plus the mapping file**, behind `ordo introspect` · `blob private`,
      `blob inline` and `blob sealed` objects, each refused by name · a
      **changed** object under a used import name, which no fingerprint
      covers · an entity referencing a platform entity
      (`platform-ref-import`) · ciphertext-mode import into a space holding
      the same keys · import
      into a non-empty space · quiet triggers during a large import ·
      deleting `$importmap` after cutover · the bound-plan cache the
      throughput table points at. **`ordo import` itself is deferred for the
      reason the CLI already gives**: the operator tool has no custody
      selection, which is why `ordo revert` passes `keys: None` and refuses
      sealed entities (`ordo-cli/src/operator.rs`), and an import that
      cannot seal cannot write a sealed field. `import_space` is a library
      entry point the host can call today.

      **A bundle restores only into the version it came from, and the owner
      has now asked for the other thing (2026-09-21).** On D-B2: *"It may
      make sense to be able to create a snapshot the user can offload and
      keep somewhere. This will also require migration instructions, which
      should function cross-versions."*

      Most of that exists. `std.space.export` produces the bundle,
      `import_space` puts it back through the write protocol, and the
      loader is already the dynamic thing the owner asked for on the
      `export` row: *"Nothing here knows an entity name, a field name or a
      type"*, built from `Schema` and the manifest by exhaustive match,
      precisely so a new field does not require editing the loader.

      **What blocks it is one deliberate refusal.** `import_space` compares
      the bundle's `artifact_version` against the deployed one and answers
      `ImportError::ArtifactMismatch` — *"same domain, different artifact
      version"*. So a snapshot is restorable only into the exact program it
      was taken under, which is the version most likely to be gone by the
      time anyone reaches for the snapshot. A backup you can only restore
      into a deployment you no longer run is not a backup.

      **The shape of the fix, and it is a real build rather than wiring.**
      The schema-diff engine (13a–13e) already computes what changed
      between two program versions and classifies each change. Carrying a
      bundle forward is that computation applied to rows on the way in
      rather than to a live database: added optional fields default, added
      required fields refuse, dropped fields drop, renamed fields follow
      the same diff codes the migration path already names. The refusal
      then narrows from *any* version difference to *the changes this diff
      cannot carry*, which is the honest boundary.

      **The owner named the cost and is right about it:** *"the concern is
      data destruction but that is no different than any migration where a
      schema change could invalidate or destroy data."* It is the same
      exposure, with one asymmetry in the snapshot's favour — the bundle
      still exists afterwards, so a cross-version restore that drops a
      field loses it from the restored space and not from the record, while
      a live migration that drops a column loses it outright.

      **Filed as 11-10b**, below.

- [ ] **11-10b · A snapshot that restores into a later version.** Owner,
      2026-09-21, on D-B2 (above and in `corpus-rulings.md`). Lift
      `ImportError::ArtifactMismatch` from *any* version difference to
      *the changes the schema diff cannot carry*, so a bundle taken under
      an older program loads into a newer one. Prerequisite for treating
      an export as a snapshot a customer keeps rather than a disclosure
      they read once, which is what the owner asked for.

      **Why it matters to D-B2:** it is the third answer to *"there could
      be broken situations where you may want to revert down the chain."*
      The safety net becomes a bundle the customer holds, rather than a
      deployment that keeps N programs serving at once. That is a better
      trade in both directions: it is cheaper for the platform and it is
      worth more to the customer, because a bundle they hold survives the
      deployment entirely.

      Needs: the diff applied per row at load, a stated refusal for each
      change class it cannot carry, and a round-trip test across a real
      version change rather than a synthetic one.

      **Shaped by the D-B2 ruling, 2026-09-21**, which decided the *how*
      as well as the *whether*: **step by step**, one version at a time
      down the chain, never a single diff between the ends. The diff
      engine is per-step by construction (13a–13e classify adjacent
      versions), a step that cannot be carried has to name itself so an
      operator learns *where* to stop rather than only that the ends
      disagree, and downgrade loses at a specific step — a field added at
      version 5 has no values before it, so coming down through that step
      drops it, and naming the step is what lets an application decide
      whether that is acceptable before it runs.

      **The part this obliges that does not exist.** The diff engine plans
      **forward**. Step-by-step downgrading needs each step to state its
      own down path, or to state that it has none, and no artifact carries
      that property today. This is the real work of 11-10b and it is
      larger than lifting the version check. Scope it before starting.

      **And the snapshot is requested, not universal.** The owner: *"for
      requesting customers on the app dev side."* It follows from D-B's
      correction that the contract is app-developer to customer, so the
      app developer asks for a customer's snapshot and the platform makes
      one available. Generating one for every tenant by default would be a
      storage cost and a disclosure surface imposed on people who never
      asked for either.

*The eight findings below had no row at all when the 2026-09-07 coverage
re-audit ran — not in this file, not on the board. The audit's own
numbering is kept, so each reads against its finding in
`audit-board-coverage.md` §2. Nothing here is re-opened: these are first
entries.*

- [x] **11-11 · Key rotation and the epoch lifecycle — the half 8c did not
      close.** Finding 11. `ordo-crypto/src/custody.rs` has `rotate`,
      retire, `destroy_epoch` and `Purpose::Reseal`; 8b/8c/8d closed the
      per-space SMK, DEK rotation and the release stage. What
      `implementation.md` (Key custody, `[settled]`) and `deploying.md` §4
      additionally require, and what nothing in `crates/` does: **the lazy
      resumable re-seal job** (no `reseal`/`re_seal` worker exists — the
      only hits are a `Purpose` variant and one secevent test), **SMK
      rotation as a blind-index re-index** (offline build, atomic swap,
      under a write barrier), and **retire-then-destroy driven by the
      undo-horizon gate** rather than by a direct call. Sharpest of the
      three: `f7-vault-plan.md` §3.4 rests a departed member's
      equality-oracle closure on an SMK rotation that has no path, and
      `plan-export.md` rests crypto-erasure on `destroy_space` — which
      exists — so the *deletion* half is built and the *rotation* half is
      not. The write barrier this needs is 13c's; sequence it after.

      **Built 2026-09-19 (K1–K3, `plan-production-ops.md` §1.6), on
      branch `k-rotation`.** The rotation half now fires and re-seals:
      - **K1a**, `ordo_exec::reseal` (`crates/ordo-exec/src/reseal.rs`):
        the chunked, checkpointed, resumable pass over the four
        populations, in place under a compare-and-set on the old envelope
        bytes for payloads, live columns and `$blobref` envelopes, and to
        a new handle for objects (K1d, old pair retired by `blobgc`).
        Per-row verification in memory, a `(ref ‖ new envelope)` digest
        per chunk, completion by re-count. `$reseal` is the state table.
        Tests `crates/ordo-exec/tests/reseal.rs`: a pass moves every
        population and no digest; a killed pass resumes from its cursor;
        a row written during the pass is skipped not clobbered (a real
        concurrent commit, not a mock); a verification mismatch aborts
        the chunk; an external object is re-sealed to a new handle.
      - **K1b**, `keys.reseal` (`crates/ordo-host/src/workers.rs`) fills
        the R3 slot: leader-only, the tick is the throttle, gauges set
        before the step, completion recorded as a custody-action in the
        space's stream. Test `crates/ordo-host/tests/reseal.rs::a_rotation_reseals_under_the_supervisor`.
      - **K2**, `reseal::destroy_gate` and `ordo keys destroy`: all four
        populations counted, refused by name with the count, the
        consequence printed as a date, `--confirm <space>:<epoch>`. The
        gate test was observed failing first against a gate that never
        refused. `destroy_epoch` has one production caller.
      - **K3**, `ordo keys status | rotate | reseal`
        (`crates/ordo-cli/src/keys.rs`), `Metrics::gauge` and the two
        series `ordo_reseal_rows_remaining{space,population}` and
        `ordo_reseal_stuck_seconds{space}`, proved against a deliberately
        wedged worker (`a_wedged_pass_raises_the_stuck_gauge`).
      - **Corrections to the 2026-09-08 K text**, recorded in §1.6 and in
        `corpus-rulings.md`: there is no suspension op to wire (the
        function it will call exists, the CLI calls it now); there is no
        `history` op (the K2 test reads `Resolved::Destroyed` and the
        revert refusal); an object cannot keep its handle across a
        re-seal without breaking every retained image's revert, so the
        object population clears by the blobgc floor; `get_byte` on an
        empty sealed column is an error, so the predicate guards length.
      - **Not in K, by the K ruling:** SMK rotation as a blind-index
        re-index under a write barrier stays ruled out of this track and
        remains owed under 13c; the departed-member equality-oracle
        closure `f7-vault-plan.md` §3.4 rests on it still has no path.

- [~] **11-12 · Attestation layers 2–4, and `chainProof` — built, with one
      witness format shipped and four named as owed.** Finding 12.
      `corpus-rulings.md` (2026-09-08) carries the placements and the
      arguments; this is the state.

      **Built.** The **egress kernel** (`ordo-egress`) — §7b's six pins as
      types rather than as checks, shipped as the first increment of its
      first consumer per the standing sequencing rule, since the attestation
      webhook *is* an outbound POST. **Layer 2** — `$attestsinks` with an
      owner-signed destination re-verified before every send,
      `std.space.attestTo` (owner-only, step-up gated), the at-least-once
      outbox with the cursor discipline `triggers.rs` established, backoff,
      dead letters on the ops surface, and the published payload schema
      `ordo.attestation/1`. **Layer 3's generic mechanism** — the
      signed-checkpoint POST, which is what §7's layer 3 *is* ("witnesses =
      wherever the owner points that webhook"). **Layer 4** —
      `std.space.chainProof($from, $to)` (manager-and-owner, step-up gated)
      emitting `ordo.chainproof/1`, and `ordo-verify-proof`, a binary that
      links `ordo-commandlog` and `serde_json` and nothing else.
      `ordo verify --proof` calls the same routine.

      Four rulings became mechanisms rather than prose:

      *Pin 5 is a dependency edge or it is nothing.* "Egress runs under an
      identity that holds nothing" is a **process** property, and this
      platform ships as one process — so the full sentence is a deployment
      obligation (`deploying.md`) and the static half is enforced by the
      graph: `ordo-egress` depends on no `ordo-*` crate, so the code that
      opens a socket to a customer-named host cannot *name* `KeyProvider` or
      `Pool`. `tests/holds_nothing.rs` asserts it by parsing the manifest,
      because the failure mode is somebody adding one path dependency to
      make one thing convenient, and a test reading the manifest fails on
      exactly that line where a test looking for a `use` would pass until
      the second commit.

      *Pin 2 is a move, not a check.* `PinnedDestination` is minted only by
      `resolve_and_pin` and **consumed by value** by `deliver`; it is not
      `Clone`, has no public constructor and no serialisation. "Re-checked
      at connect time" is therefore not a discipline the worker observes but
      the only shape the API has — a second delivery needs a second
      resolution because the first one ate the value. Pin 3 is an
      *absence*: the HTTP/1.1 request is written out by hand so there is no
      redirect-following code to disable, and a reviewer greps for
      `Location` rather than checking a builder chain.

      *The delivery signature is never the checkpoint key.* Two signatures,
      two claims, two keys: the payload's Ed25519 is minted on the cadence
      and copied verbatim, the transport MAC is a per-sink shared secret.
      Sharing them would put the key that authors this deployment's claims
      about its own history inside the egress identity — the one built to
      hold nothing.

      *A chain proof carries digests, not bodies.* Which is what
      `implementation.md` specifies, and it is the design: the third party's
      question is about links and signatures, the bodies are already
      exportable through `std.space.export` under the owner's authority, and
      a second full-content path with a different gate would be the weaker
      of two doors to the same room. The consequence is that `chainProof`
      discloses **no space content**, which is what lets it be
      manager-reachable at all.

      **What the build found.** (i) **Three SSRF bypasses a correct IPv4
      range check does not see**, because none of them is an IPv4 address at
      the moment it runs: `::ffff:127.0.0.1` (for which
      `Ipv6Addr::is_loopback` answers false and the socket reaches localhost
      anyway — the same form of `169.254.169.254` walks past a deny list
      built from §7b's five categories), the NAT64 prefix `64:ff9b::/96`,
      and `0.0.0.0/8`. Every address is now canonicalised before any
      predicate runs. (ii) **§7b's `fd00::/8` is the wrong half of
      `fc00::/7`** — widened in the direction the pin argues for, recorded
      rather than done quietly. (iii) **A health field only some callers
      update lies to the others**: `DeliveryHealth::sent` was incremented by
      the loop and not by the pass, so a caller driving `run_pass` directly
      — a test today, R3's supervisor tomorrow — read zero after a delivery
      that demonstrably landed.

      **Owed, and named rather than implied.** *Four witness formats*: the
      WORM/object-lock bucket is nearest (a request builder plus in-house
      SigV4 on the same pinned client) and is not shipped because "the
      request has the right headers" is not the property — "S3 accepts it
      and applies object lock" is, and that needs a real bucket in CI;
      Rekor is the best second, because the Merkle inclusion-proof
      verification it needs generalises the offline verifier rather than
      special-casing it; OpenTimestamps has the weakest *verification*
      story of the set (honest checking wants a Bitcoin node, and anything
      less trusts a calendar server); git-over-HTTPS is the most code for
      the least incremental property. *The public transparency log* stays
      **declined**, which is what the `attestation` decisions row
      recommended. *A `chainProof` covering more than `MAX_LINKS`* — a
      caller asks for two ranges and verifies each against its own anchor,
      because a proof is one artifact verified as a unit and paging it would
      hand somebody two files and the stitching job the verifier exists to
      do correctly. *Setup guides per target* (§7's third documentation
      obligation) — `howto-audit.md` carries the own-endpoint one; the other
      five wait on their formats. *A multi-sink allowlist table* — one sink
      per space today, which is what makes "the allowlist is exactly the
      owner-signed destination" true by construction; the general family is
      17k's.

- [ ] **11-15 · Space provisioning lifecycle — the reconciler, the
      **2026-09-29: P0 and P1 built:** `ORDO_DEPLOYMENT_OWNERS` names the
      deployment owner at boot, and `provision_own` gives one person one
      tenant, safe under races. P2 and P3 wait for recovery.
      **2026-09-29: built-in provisioning on sign-up planned** in
      `plan-provisioning-on-signup.md` (P0 to P5), owner-ruled as how
      provisioning works; P0 names the deployment owner at boot.
      compensation table, and `std.space.reap`.** Finding 15.
      `bootstrap.md` §1–§4 are `[settled]`: the seven-phase model, the
      genesis command group at chain position 0, the
      `provisioning → active | failed → reaped` state machine, the
      `bootstrapTimeout` reconciler, the per-resource compensation table.
      In `crates/`, `reap`, `reconciler` and `bootstrap_timeout` matched
      **nothing**. What existed was `std.space.create` as an op name.

      **Started 2026-09-25: the state machine, the row-first ordering,
      `std.space.reap` and the `bootstrapTimeout` reconciler.** The 46-item
      input inventory, the seven-phase model and the three entry paths are
      not built and stay in this item.

      **The reconciler marks and does not release.** It moves a stale
      `provisioning` row to `failed` and stops there, because releasing a
      tenant's database and keys is `std.space.reap`, which is
      operator-only and step-up gated for §4.5's reason: a process that
      destroys databases with no human asking will eventually destroy the
      wrong one. `failed` is what puts a stuck provision in
      `tenants::troubled`, which is §4.1's one detection query. So the loop
      is: creation dies, the reconciler notices within the timeout and says
      so, and a person decides whether to reap.

      The sweep lives on `tenants::time_out_stale` rather than in the
      worker, so the policy sits beside the table it is about and can be
      tested without standing up a supervisor. `ORDO_BOOTSTRAP_TIMEOUT_SECS`,
      default the plan's 15 minutes, which is long on purpose: a provision
      creates a database and applies a schema, and a reconciler that fired
      early would race a call that is still running.

      **A test of it was vacuous and the reason is worth keeping.**
      `the_reconciler_ignores_tenants_that_are_active_however_old` swept
      "a year later" by adding to the test's fixed `NOW` constant — but
      `TenantOps::create` stamps `state_at` from the **real wall clock**,
      so the sweep time landed *before* the tenant existed and every row
      was skipped for the wrong reason. It passed with both of the
      reconciler's guards deliberately removed. Found by falsifying, fixed
      by anchoring the arithmetic to the row's own timestamp, and it now
      fails when both guards go. Each guard alone still holds the property,
      which is defence in depth rather than redundancy, and that is now
      shown rather than assumed.

      **The leak was real.** The registry row was written **last**, after
      the custody keys and the database, so a crash in between left both
      with nothing naming them — §4.4's *"every orphan is reachable from
      one table"* was false in the one direction that matters. The row is
      now reserved first, the **target is recorded before the database is
      created** so a leaked database is findable, and activation is the
      last write. `$spaces` carries `state`, `phase`, `target`, `state_at`
      and `note`, which is the owner's *"a value within the space"* rather
      than a cache.

      **A failure marks the row, it does not delete it.** Deleting it
      would be the same leak arriving through the tidy-up path. What is
      deleted is nothing; what is recorded is how far creation got, which
      is what `reap` reads to know how much to undo.

      **`std.space.reap` refuses anything `active`** (§4.5): cleaning up a
      failed provision and destroying a customer's world are different
      buttons with different callers. It releases the attachment, drops
      the database, forgets the fleet row and destroys the custody keys —
      **keys last**, because keys destroyed before the rows they seal
      leave unreadable data standing, which is a worse residue than a
      database with no keys. A tenant on its **own perimeter** has its
      database reported and left standing: the platform never drops one it
      did not create.

      **Three things this turned up.**

      `$spaces` had to join `erase::ATTACHMENTS`. Reserving the row first
      made `rows_held` count it, so a create refused itself with "this
      space already holds rows on the default database". The list already
      existed for exactly this distinction — tables that *name* a space
      rather than hold its data — and the registry is one.

      `ensure_table` now backfills the new columns, because
      `CREATE TABLE IF NOT EXISTS` does nothing to a table that already
      exists and every developer's conformance cluster holds dozens of
      pre-11-15 `$spaces` tables. Not owed as a migration (nothing has
      deployed), built because the alternative is each developer meeting
      `column "state" does not exist` and being told to destroy their
      cluster.

      **That backfill introduced a bug and the fix is the interesting
      part.** It tells an old row from a new one by `state_at = 0`, and
      `reserve` did not stamp that field — so a genuinely half-built
      tenant also had zero there and the next boot would have declared it
      `active`. A tenant announced as ready with no database, produced by
      the code written to smooth an upgrade. Nothing caught it; the test
      `a_boot_does_not_activate_a_tenant_that_is_still_being_built` was
      written afterwards and falsified against it. A
      half-created space with no reconciler is a space that holds
      resources nobody will ever reclaim, and the compensation table is
      the document that says which ones.

      **Phase 0 is a phase now, and that closed a second leak of a
      different kind.** §1.2 says *"preflight is the whole product"* and
      that every decidable failure is refused **before the registry row
      exists**. It was not. Every one of them — a malformed id, a host off
      the allowlist, the control database, a busy routing lock, a space
      already holding rows or attached elsewhere, a database another
      tenant sits on — ran inside `prepare_target`, which runs *after*
      `reserve`. So an operator who mistyped a host was told so **and**
      left a `failed` tenant in `tenants::troubled`, which §4.1 makes the
      deployment's one detection query. A typo is not a stuck provision,
      and a pane that reports typos is a pane people scroll past.

      `prepare_target` is therefore split: `provision::preflight` decides
      and writes nothing that names the space, `provision::commit_target`
      writes. **The `Preflight` value carries the `ApplyLock`**, which is
      the reason this is a value and not two calls: R-S5 wants one lock
      held across the occupancy read and the attach, so taking it in
      phase 0 and dropping it before phase 1 would reopen exactly the
      window it exists to close. The lock rides through `reserve` and is
      released when the value drops, which also makes *dropping a
      `Preflight`* the whole of abandoning a plan. `commit_target` takes
      it by value and takes no `Target`, so there is no path that runs the
      checks twice and provisions against the older answer.

      **Preflight dials, which §2 item 26 asks for in those words**
      (*"connects and runs a probe transaction; it does not merely
      parse"*). `ordo_exec::probe` dials exactly the connection
      `create_database` will — the cluster's maintenance database, with
      the target's own credentials — and answers the catalogue question
      on the same round trip, so the plan can say whether a database
      would be created or adopted. It does **not** prove the role may
      `CREATE DATABASE`; Postgres will not say without trying and trying
      is the write this avoids. What it removes is the commonest mistake
      in the op — a wrong host, port or password — which used to cost a
      registry row, a custody key and a reap.

      **`dryRun` (item 45)** is built on that seam: phase 0 runs, the
      plan comes back (resolved target, minted database name, schema
      version, whether the database is standing, occupancy), and nothing
      is reserved, so *"two dry runs of the same slug both succeed."* The
      answer carries **no space id**, because none was minted for keeps
      and an id would read as a handle to something.

      **Two smaller things fell out and are worth the line.** A custody
      refusal was a 500; it is now `space_unavailable`, because an
      unhealthy KMS is a dependency being down and §4.3 lists it first
      among the steps that cannot be transactional. And a refusal that
      happens *after* the row now says so: every message from
      `refusal_to_error` ends "Nothing was created", which is finally
      true on the preflight path and was never true on the other, so the
      commit path appends the tenant id and names `std.space.reap`.

      **One test had to change its mechanism, and the reason is the
      increment.** `a_failed_provision_leaves_a_row_that_names_what_it_was_building`
      used to fail a provision by naming an unreachable port, which
      worked only because the dial happened in phase 3. That call is now
      refused with no row at all — which is the fix, and is what
      `an_unreachable_cluster_says_so_without_naming_the_topology` now
      asserts. The failure that remains after the row is an external step
      that cannot be transactional, so the test double is a `KeyProvider`
      that refuses `create_space`: a realer fault, and the first row of
      the compensation table.

      Falsified five ways: removing the probe, ignoring `dryRun`,
      deleting the row on failure instead of marking it, dropping the
      sentence that names the leftover tenant, and — the one that matters
      — restoring the pre-11-15 order by reserving before preflight,
      which fails all four of the new assertions at once.

      **What is left of 11-15 was put on the board as a scheduling
      question (`bootstrap-inventory-split`) and answered 2026-09-26:
      *"They'll close after their blockers complete."*** So **this item
      does not close at the lifecycle.** It stays open until the 46-item
      inventory and the three entry paths are genuinely built, and each
      blocked item closes as the subsystem it waits on lands. The roadmap
      keeps one item that tells the truth about what bootstrap still owes
      rather than a closed item and a scattering of pointers.

      Most of the remaining inventory is blocked on subsystems that do not
      exist yet — package `bootstrap` blocks, access-profile templates, the
      compliance pack, the client registry, notification channels, genesis
      import — and each waits on the item that builds its subsystem rather
      than being re-filed here.

      **Item 44 is built, 2026-09-26.** `Idempotency-Key` is required on
      `std.space.create` and on no other op. The header is optional on every
      other write because the cost of omitting it there is bounded — a retry
      repeats an insert, and the row's own key, a `rev` CAS or a `converge`
      mode catches it. That bound does not hold for an op whose effect is
      **outside the database it writes to**: a retry mints a second KMS key
      and a second database, answers with the second tenant's id, and leaves
      the first for somebody to find by reading the cluster. Both rows are
      `active` and correct, and nothing reconciles them.

      **Declared on the spec, not listed in the router.**
      `OpSpec::requires_idempotency_key`, refused before anything is
      reserved or executed, because the point is that this effect cannot be
      made twice safely — so the call that would make it twice must not
      reach the executor even once. The router is generated from the
      registry and has no per-op code by design, and whoever declares an op
      is who knows whether its effect is repeatable. Compiled ops set it
      false with the reason recorded beside them, so nobody turns it on
      catalogue-wide and breaks every client for a risk already covered.

      **`ordo space create` mints a key per invocation and prints it when
      the call dies in transport**, with `--idempotency-key` to pin one.
      That is the case the requirement is actually for: a transport failure
      leaves an operator unable to tell *"the door never saw it"* from
      *"a tenant was built and the answer was lost"*, and those need
      opposite next steps. Re-running with the same key settles it instead
      of risking a second tenant.

      Falsified three ways — never enforcing it, enforcing it on every
      write, and turning the flag off for the op — each caught by the test
      that claims the property. The second test reads the spec directly and
      asserts no *other* registered op carries the flag, because a
      mechanism this easy to widen wants a guard against widening.

      **Item 22 is built, 2026-09-26.** Preflight asks whether custody can
      be opened at all, and refuses before the registry row exists. Phase 2
      mints the tenant's SMK and is the first step that cannot be rolled
      back — a KMS is an external system with no part in a Postgres
      transaction, so its failure leaves a `failed` row whose compensation
      is a *scheduled destroy* of a key that may or may not exist. Item 22
      calls this *"the check that keeps phase 2's non-transactionality from
      mattering in the common case"*, and the common cases are a provider
      that is down and a process holding the wrong master.

      **It is the same question `/health/deep` already asked, moved into
      one function.** `ordo_crypto::probe` is `blind_index` under a reserved
      pseudo-entity, and the choice of question is the substance: the cheap
      alternatives do not answer it. `current_epoch` takes the maximum epoch
      of the store's DEK rows and **unwraps nothing**; so does `release`. A
      process booted with a mistyped `ORDO_MASTER_KEY` passes both, reports
      healthy, and then fails every read of every sealed field.
      `blind_index` derives the per-field index key from the space's SMK, so
      a wrong master is an error here rather than at the first customer
      read, and it takes no lease. Falsifying by swapping it for
      `current_epoch` fails 12 tests, which is that distinction showing up
      rather than an opinion about it.

      **Asked about the deployment's own space, not the tenant being
      created** — the tenant's keys are what the call is deciding whether to
      mint, so there is nothing there to ask about. The system space is
      created unconditionally at boot and is the one always present, which
      is why readiness asks about it too.

      **What it does not claim, pinned by its own test.** A provider can be
      readable and still refuse to *create* — a key ring with no write
      grant, a quota, a policy — and that failure still lands in phase 2 and
      still marks the row `failed`. A probe that appeared to catch it would
      be worse than none, because it would turn *"the residue is possible"*
      into *"the residue is impossible"* while leaving the residue possible.
      `a_readable_provider_that_will_not_create_still_fails_in_phase_two`
      requires that path to stay reachable.

- [x] **11-16 · Deployment-scoped ops, and the `[open]` ruling they wait
      on. RULED 2026-09-07.** Finding 16. `bootstrap.md` §1.1 settles the
      `scope deployment` op class — it binds no space, `$space` becomes
      `unknown_param`, and it authorizes against a **deployment** capability
      rather than a per-space profile. §8 marked the `CapabilityGrant`
      addition and its wire rendering `[open]`, awaiting an owner ruling
      that had never been asked. Ruled: the space-free set is exactly four
      kinds — operator/system tenant management, genesis
      (`std.space.create`), the cross-space personal surface ("list my
      spaces"), and deployment usage export (M3); anything new must argue
      its way in. See `corpus-rulings.md`, "Deployment-scoped ops: the
      space-free set is four, closed." The `CapabilityGrant` shape and wire
      rendering can now be built against a fixed list; that build is
      separate follow-up work, not tracked by this row.

- [ ] **11-17 · Log-positioned reads and snapshot consistency.** Finding
      17. `implementation.md` (Client delivery, `[settled]`) and
      `ui-gen.md` §2 require every read to carry the per-space
      command-log position so a multi-widget page holds **one** position
      and two surfaces can never straddle a command group. §14's preamble
      names it as part of the *Clients & surfaces* group and then gives it
      no item. `log_position` matches nothing in `crates/`; the position
      that 8q's layer-1 pinning attaches is an *attestation* pin, not a
      read snapshot, and the checklist says so at 8q — the two must not be
      conflated. 14b is specified on top of this and cannot be built
      without it.

- [ ] **11-22 · The deployment operational security log.** Finding 22.
      The third artifact of `security-logging-design.md` §6.1's
      three-artifact split, where non-member, anonymous and
      unrelated-space denials land — and the only place a **cross-space
      sweep** is visible, since a principal probing a hundred spaces
      appears in none of their per-space streams *by design*.
      `bootstrap.md` §8 calls it "a whole missing subsystem" and it is
      `[proposed]`, so this is a design item before it is a build item.
      Filed because §7's shipped security-event stream reads as full
      coverage and is members-only.

      **Ruling (2026-09-07), on the "own artifact or dumped in the app
      log" half of this item's design question:** the owner: it "should
      be a toggle, but its own artifact makes sense as a default." So the
      deployment operational log ships as its own hash-chained artifact by
      default, with a deployment-config toggle to turn it off — not a
      choice between the app log and a dedicated stream, a default with an
      off switch. Who may read it (the item's second, harder question)
      remains open.

      **Stale row, corrected 2026-09-10 by `plan-opslog.md` §0.** This is
      not "a whole missing subsystem". `Scope::Deployment` exists, the
      door routes to it on all three denial paths, and it is stored,
      signed into its own chain from its own genesis, and retained
      (`ordo-secevent/src/event.rs:36-51`, `recorder.rs:192-200`,
      `ordo-exec/src/secevent.rs:83-108`). Three holes remain, and one is
      serious: **the collapse key degenerates for deployment scope**.
      §6.1 pins it at `(space, principal, op)` precisely so a sweep
      cannot merge, and `Scope::Deployment` is a unit variant carrying no
      space, so the key is `(principal, op)`, the version the design
      rejected. A ten-thousand-space sweep collapses to one entry with
      `count: 10000`. The artifact that exists because a sweep is
      invisible in any one space's stream cannot presently see a sweep
      either. The cause is that entries never record which space was
      probed: `Observation` carries it, `route` uses it, `observe` drops
      it. Also found: `/security/alerts` is mounted and its op is
      deployed nowhere, so it answers 403 to everyone; and platform
      surfaces authorize against a caller-named space. Plan, increments
      and five owner questions in `plan-opslog.md`.

- [x] **11-27 · The isolation suite stops at the write protocol. Filed
      2026-09-10, BUILT 2026-09-12 (`28ae9be`), and its second finding
      closed 2026-09-13 (`34abb91`).** `crates/ordo-conformance/tests/spaces.rs` (item 9h) is
      the two-space isolation suite and it is well built: the fixture
      seeded twice with **identical row ids**, differing in reads, guards,
      tree walks, writes and uniqueness, on the stated ground that "a
      query missing its space predicate still finds the right row by id
      and looks correct forever" and "isolation must not rest on ids being
      lucky". It reaches the write protocol and nothing above it: the file
      names neither `ordo_exec` nor `ordo_host`.

      **Three bugs in two days lived in exactly that gap, and all three
      are the same shape.** C1 of `plan-blob-chain.md` found **seven**
      statements in the revert applier addressing rows by `id` with no
      space predicate, while ids are minted from a process counter and so
      collide across spaces by construction. C2 found the write protocol
      NFC-normalising space literals and the revert applier not doing so.
      D0 of `plan-opslog.md` found the platform surfaces authorizing
      against a caller-supplied `x-ordo-space` header, so the owner of any
      space could read a deployment-wide disclosure meant for the
      operator, confirmed by a test that returned 200 where it owed 403.

      **What is owed: the same discipline one layer up.** A two-space suite
      over `ordo-exec` and `ordo-host` with the same identical-ids
      construction, covering the revert applier, the blob door and its
      collector, the platform surfaces, and security-event routing. Run by
      default, so the next bug of this shape fails a test rather than a
      tenant.

      **Second, larger fact behind it, CLOSED 2026-09-13** (`34abb91`).
      It read: `deployment_engine` hardcodes `SpaceId("conformance")`, so
      nothing in the tree provisions a second space, and three separate
      test justifications lean on "no deployment provisions two spaces",
      which is a fact about today rather than a property to rely on.

      Both halves are now built. `SPACE` is gone as a constant, replaced
      by `DEFAULT_SPACE` plus `deployment_space()` reading
      `ORDO_DEPLOYMENT_SPACE`, with a malformed value **refused rather
      than defaulted**. `crate::spaces` resolves an executor and a write
      log per request from the door's binding, keeping the space a field
      rather than an argument, because a space that arrives as an argument
      is a space some future call site can pass wrongly.
      `crate::provision` brings a space into existence in one place: name,
      then custody, then the founding member row last, so the single
      reachable interruption leaves keys nobody can reach and no rows to
      be half-visible. The host suite's claim that a two-tenant test was
      not constructible is deleted, and five tests replace it.

- [x] **11-28 · The host's audit chain lived in memory. Found 2026-09-13
      during S1 of `plan-space-routing.md`, and built the same day.**
      The host's op path called `Executor::write_leased` and its blob door
      `attach_write`, both with no chain to append to, so every command
      group went to the in-memory `WriteLog` and never to `$commandlog`.
      `LoggedExecutor` appends durably, and nothing in `ordo-host`
      constructed one. The chain proof and export read `$commandlog`, a
      table the serving path never wrote, and a restart lost every space's
      history. 8q recorded the symptom (every response pinned genesis) as a
      property of a test harness; the host has no other executor, so it was
      the serving path.
      **A second bug sat under it and had to be fixed first.** The
      `WriteLog` minted row ids, command ids, correlations and command times
      from one per-process counter, deterministic so that suites replay. In
      a host that restarts, the next create mints `x_w1` again and collides
      with the row already holding that id, and once appends are durable the
      next group mints `cg_1` again and the chain refuses it as a duplicate.
      A counter is also not a time: prune's undo horizon compares command
      times in seconds, and counter values read as fifty years old.
      **The fix.** `WriteLog::serving(prefix, clock)`: every minted id
      carries a random prefix drawn once per log (one space in one process),
      and commands are stamped with the wall clock in seconds.
      `WriteLog::new` is unchanged, so every suite still replays exactly.
      The host's writes go through `Executor::write_chained` and its uploads
      through `attach_write`, both appending to the executor's own space's
      chain inside the write's transaction, with the space taken from the
      executor's field rather than an argument.
      Proved by `a_restarted_host_continues_the_chain`
      (`ordo-host/tests/spaces.rs`): two resolvers over one database, which
      is a restart, both write, both groups reach `$commandlog` in order,
      and the created rows have distinct ids. The blob-door tests that
      copied upload groups into the chain by hand now assert the door put
      them there.
      **And a unit bug it exposed.** `Timestamp` is seconds by its own doc,
      and the prune compares in seconds, but the blob collector's
      `cutoff_of` took the minimum of a group's time and a horizon in
      milliseconds. With a consumer holding the floor, the cutoff landed
      in January 1970 and the sweep stopped. Invisible while command times
      were counters, and untested because no blobgc test held the floor.
      Converted to milliseconds, and pinned by
      `a_consumer_holding_the_floor_pins_the_cutoff_in_milliseconds`.
      The same sweep also handed `prune::floor` its millisecond `now`, so
      every group looked older than the horizon and the horizon never
      bound, and every mirror looked idle for decades, so every evictable
      mirror was left out of the sweep's floor. Two consequences. The
      report named the chain end as the reason storage was held. And a
      lagging but active mirror did not hold back blob collection the way
      it holds back the prune. The cutoff stayed capped at the horizon, so
      nothing inside the undo horizon was ever collected; client-first
      mirrors and routine triggers are not evictable and were unaffected;
      and the sweep only reports evictions, so no consumer was
      deregistered. Now given seconds, and pinned by
      `with_no_consumer_the_undo_horizon_holds_the_sweeps_floor`.
- [x] **11-25 · `plan-tiers.md`'s two cheap-now-expensive-later items.** **Built
      2026-10-23:** a `tier` field in every op artifact (T0) and observed cost per
      op (`observed_cost.rs`, `std.deployment.observedCost`); CI green on 3f014eaa.
      Finding 25. The document is `[proposed]` and explicitly "not v1", so
      the tier machinery is correctly unbuilt — but it names two things
      that cost almost nothing today and are expensive retrofits: reserve
      a `tier` field in the artifact document, and record observed cost
      from the first executor. Neither exists (`grep tier`
      `ordo-plan/src/artifact.rs`, `grep observed_cost crates/` — both
      empty). The artifact half is the one that matters: every added
      artifact key moves 165 op versions and kills every capability grant
      pinned to them, as 14a just demonstrated, so a reserved field costs
      one re-consent campaign now and one later if it is skipped.

- [ ] **11-26 · Break-glass operator mode. RULED 2026-09-07.** Finding 26.
      `dashboard-plan.md` settles it *as shape*: an explicit time-boxed
      grant, step-up gated so a capability token can never satisfy it,
      command-logged, never a default. It is the **only sanctioned path
      into a space's data by a non-member**, which is why it is an
      authorization capability rather than a dashboard feature. Nothing
      implements it — and four modules already name it as an existing act
      in prose (`ordo-identity/src/access.rs:29`, `:552`,
      `ordo-exec/src/erase.rs:73`, `ordo-secevent/src/event.rs:401`,
      `ordo-host/src/exit_ops.rs:26`, all listing break-glass beside epoch
      rotation as a `CustodyAction`), so the code reads as though the
      capability existed. That is the shape this repo keeps finding: a
      class with no producer. Either build it or strike it from those
      comments.

      **Ruled:** over sealed data there is no break-glass — the platform
      offers the operation only against an explicit, user-provided grant
      key, non-negotiable at the platform level (policy for managed-custody
      spaces, which could physically comply and refuse anyway; physics for
      client-first ones, which never held a key capable of the act); an app
      author may build their own implicit form in their own domain. See
      `corpus-rulings.md`, "Break-glass: no default, and the grant key is
      the only door." The five lying comments are fixed to describe the
      grant-gated form (this item's remainder, since the ruling does not
      itself build the grant-gated operation — that stays open, unclaimed
      by any tracked row).
---

## 12 · Automation & events — the trigger layer

`audit-board-coverage.md` finding #1: `definition-language.md` §7 is headed
`## 7. Triggers [settled]`, it ends before §8's `[lean]` pin begins, and the
readiness board has **no row for any of it** — while one of its rules is an
authorization rule. This is that group.

- [x] **12a · §7 is two halves, and the deciding sentences are different
      sentences. RULED.** The nine-declaration-forms ruling (9C) settled
      that `routine` is `[lean]` and not owed, and that `when`/`every` are
      "settled, not under the lean pin, and still not buildable — because
      their one operand is". Both stand. What that ruling did not separate,
      and this one does, is that §7 pins **operand spellings** as well as a
      declaration it never fixes.

      **Not owed — the declaration form.** §7 spells the fact as top-level
      `when finance.transactions.ingested -> run spendWatch`; §8 spells the
      same fact as a nested `trigger when …` inside a `routine` block;
      nothing in the corpus says which is the declaration, and `-> run
      <name>` names a routine, which is the lean half. Building either would
      pick. `routine`, `each ->`, and the `run` header's *spelling* go with
      it. `docs_parse.rs`'s allow-list is unchanged and still records those
      blocks as legitimately unparsed.

      **Owed — everything §7 states without reference to where the clause is
      written.** Four of them carry their own `[settled]` mark inside §7
      (`Event kinds are a closed set`, `Key release`, `Blind but
      addressable`, `Operational semantics`), and none is a file format:

      - the **closed event-kind vocabulary**, whose own sentence names the
        class that enforces it — *"an unrecognized kind is a compile error
        (`unknown_event_kind`), **not a pattern that silently never
        fires**"*;
      - **wildcards at whole positions only, no prefix globbing**;
      - the **`Z` suffix**, pinned in the imperative — *"schedule times
        **must** be written with the `Z` suffix … UTC is explicit at the
        keyboard, never silently implied"*;
      - `debounce` (trailing-edge, with a declared max-wait) and `coalesce`
        (at most one pending run);
      - the **`$trigger` context** — group id, affected row ids, batched
        event ids, timestamp, and an empty id set for a clock run;
      - **at-least-once**, keyed by batch id × routine version, with retry,
        backoff and a **dead-letter** path;
      - the **listener cursor / archive floor** identity — *"At-least-once
        is not a separate promise from the floor rule — it *is* the floor
        rule, seen from the routine's side."*

      And R-G20 is owed by a second document: `guards-conformance.md` is a
      conformance specification, not a `[lean]` section of DL, and it states
      the provenance bound as a numbered rule with three cases.

      This is DL §3's `allocate` seam read a third way. There, a `[settled]`
      entry marked a sliver of its own *spelling* lean and the semantics were
      owed; in §8 a preamble marks all spelling lean and the semantics are
      owed; here a `[settled]` section pins some spellings **and** a
      declaration it never fixes, so the split runs *inside* the surface
      syntax rather than between syntax and semantics.
- [x] **12b · The operand grammars. DONE — `87a681d`**
      `ordo-syntax::trigger`: `EventKind` (seven words, and an eighth is
      `unknown_event_kind`), `ListenerPattern`, `Schedule`, `Modifiers`.
      Nothing in it is reachable from `parse_program` — it parses strings —
      so the declaration form stays unbuilt and unchosen.
      **`unknown_event_kind` had no producer anywhere**
      (`audit-doc-contradictions.md` F7). It has one now, compile-tier, so
      `api-engine-design.md` §4's closed status table is untouched and
      `class_inventory.rs` is satisfied by §7's own sentence. Its two F7
      siblings (`group_nesting`, `check_violation`) are untouched and still
      owed.
      Engine-pinned, with the reason in the code: §7 requires `debounce` to
      carry "a declared max-wait" and gives no spelling for declaring one,
      and without it a burst arriving faster than the window defers a run
      forever — a listener that silently never fires, arriving through the
      door `unknown_event_kind` guards the other side of. Window and
      max-wait are therefore set together and no path sets one alone.
      Also refused, and nowhere else in the language: `mo` and `y` as
      schedule periods. The engine declines unit conversion everywhere on
      purpose, and the clock is the worst place to start deciding a month is
      thirty days.
- [x] **12c · The runtime. DONE — `464a181`** `ordo-exec::triggers`, seven
      cases against real Postgres (`ordo_trig_*`).
      **Event derivation needed no new bookkeeping.** Every fact a §7 pattern
      may name is already in the command envelope: `CommandKind` is
      created/converged/deleted verbatim, `Actor::Source` narrows to
      `ingested` and `source:feed-*` to `fed`, and `reverted` is decided by
      the group's `rev-` correlation — so "a group revert touching the
      entity" is answerable with no flag column anyone could forget to set.
      Nothing reads a value, which is what makes the layer blind by
      construction rather than by discipline.
      **At-least-once is the floor rule, so it is not implemented twice.**
      `prune::ConsumerKind::Listener` already existed and was already
      non-evictable; a listener registers one and advances it only after its
      batch commits. The suite asserts the asymmetry directly — two
      consumers stalled ten times past the policy window, and only the
      mirror leaves.
      Three rulings became mechanisms rather than prose. (1) **`coalesce`
      keeps the batch it cannot fire**: "at most one pending run" must not
      mean "drop the second burst", because at-least-once is not negotiable
      against a modifier. (2) **The clock uses `claim_schedule`, not
      `claim_due`** — `claim_due` advances every due schedule by the one
      period it was handed, so a nightly sweep and a five-minute heartbeat
      would have the loser adopt the winner's cadence, which `jobs.rs`
      already recorded as the harder kind of bug to notice. (3) **A dead
      letter is a row an operator can enumerate**, not a log line: §7 calls
      it an operator alert, and an alert nobody can list is not one.
      `MAX_ATTEMPTS` is engine-pinned because §7 requires a number and no
      document in the corpus gives one.
- [x] **12d · R-G20 — the provenance bound. DONE** Nine cases in
      `ordo-exec/tests/routine_provenance.rs`, against real Postgres.
      `ordo_sema::Provenance` sits at the seam where guards are applied, so
      the bound is **built with the mechanism rather than after it** — there
      is no code path that runs a routine body and reaches the entity-source
      filter without answering which arm it is on:

      | Woken by | Arm | What proves it |
      |---|---|---|
      | clock | guards pass | the read returns a row set **no principal in the fixture can obtain** — task_5 included, which GC029 says nobody may write |
      | listener | `id in` the reachable set | GC096's probe: an unfiltered `tasks[id, title]` with `unbounded` on the header still reaches task_6 alone |
      | listener + `unguarded` | lifted | one flag's difference, five rows against one |

      **`unbounded` never enters the bound** — it is not read at that seam at
      all. Two markers, two decisions, and the code says so where it would
      otherwise be a comment.
      **The write side follows from `each ->` rather than needing its own
      bound**, and that is why §8 gave routines `each ->` instead of a write
      language: a tier-1 routine's emission walks the rows its pipeline
      produced, so bounding the read bounds the emission by construction.
      **What "reachable from `$trigger.ids`" resolves to, and what it does
      not.** §8 never defines the relation. It is built as **one declared ref
      hop in either direction** — the trigger rows, a child pointing at one,
      a parent one points at — and everything else is `false`. Two hops are
      not built and their rows are *invisible* rather than visible: a bound
      tighter than the specification can never be a confused deputy, while a
      looser one is exactly the escalation R-G20 closes, and §8's own answer
      for a routine that must range wider is already `unguarded`.
      The rationale the corpus forbids is refused *executably*: a case
      asserts that a routine and R-G12's anonymous caller are different code
      paths, in rows.
      A regression case pins the ordinary path byte-identically — the seam
      replaced the `if !self.unguarded` guards were applied behind, so
      `Provenance::Caller` has to plan the same SQL the previous entry point
      did, for five sources including a composed op.
- [ ] **12e · The remainder, named rather than left to discovery.**
      **Built 2026-10-08** (75fd401b, 741132aa, 6854615e, 60bdec5a): the
      declaration forms (§8's nested `routine` block, §7's top-level `when`
      refused with a pointer), `routine:<name>@<version>` attribution, the
      worker and clock wiring in the binary, and paused tenants skipped.
      Guide `docs/howto-routines.md`; tests `crates/ordo-host/tests/routines.rs`.
      Still owed below: `ingested`/`fed` producers, the key-release log, rate
      caps, the manifest's trigger class, `each ->` bodies (asked,
      `triggers` ask 1). Board: `triggers`.
      - ~~**The declaration forms** — 12a.~~ Built 2026-10-08.
      - **`routine:<name>@<version>` attribution, and the two event kinds
        that depend on it.** `Actor::Routine` and `Actor::Source` are in the
        command log's closed taxonomy, round-trip through the chain encoding,
        and have **no producer**: the write protocol builds
        `Actor::User(caller)` at its one construction site
        (`ordo-conformance/src/write.rs`). So a routine's writes cannot yet
        be attributed as §8 requires — **and `ingested`/`fed` can never fire
        on the real write path**, because both are narrowings by an actor
        kind nothing produces. Two of §7's seven kinds are reachable only
        from a synthesised group until that site is widened.
      - **Key release derived from op access**, and its release log
        (principal, op, fields, space, instant). §7 `[settled]`; it belongs
        to the sealed/grant surface rather than to the trigger layer, and
        there is no release log to write to.
      - **Rate caps.** §7 lists them among the modifiers and gives no
        spelling anywhere — the least-specified thing in the section.
      - **The manifest's trigger class.** §8 says a pinned manifest's
        row-scope shape carries clock-vs-listener. The manifest has **no
        row-scope shape at all** yet (`op_object` publishes `scope` as field
        sets, plus `guards`, `flags`, `bypassed_guards`), so adding a trigger
        class alone would be a field with nothing to compare it against. The
        row-scope shape is the prerequisite, and it is a grant-pinning item
        rather than a trigger one.
      - **A worker process and a clock process.** Every loop *step* is built
        and tested — `claim_runs`, `fail_run`, `fire_due`, `fire_clock`,
        `drain_listener` — and none of them is called from a long-running
        task in `ordo-host`. That is deployment wiring, and it is the one
        remaining sense in which "nothing drains the queue" is still true.
      - **`$logconsumers` is not a platform table.** It is created by
        `prune::ensure_tables`, which only the operator CLI calls, so a
        deployment that starts a listener before `ordo prune` has ever run
        finds no cursor table. Recorded rather than traded: the trigger
        tables are in `ensure_platform_tables` because accumulation rides the
        write transaction and must exist before the first write, while a
        cursor is registered by whoever starts the listener.

---

## 13 · Schema evolution (`plan-schema-evolution.md`)

The largest gap in the system: `ddl_for` emits a whole schema from
scratch, there is no diff and no `ALTER` beyond creation-time
`ADD CONSTRAINT`, and the shape gate — which shipped and works — refuses
to boot a deployment whose database disagrees with its compiler. So you
cannot change a deployed schema, and if you change it by hand the
deployment stops serving. The plan is five increments; increment 1 is the
read-only one and it has landed.

- [x] **13a · Increment 1 — the plan, the probes, and the two defects.**
      — `0750dc6` `acc9821` `57e7849` `1f19e9f`
      `ordo plan --against <db>`: a classified, probed, read-only report
      over `ordo_schema::diff` (the §5 taxonomy as a closed enum with a
      `class()`), `ordo_exec::migrate` (the `nfc.rs` probe pattern
      generalised — duplicates, nulls, out-of-enum, dangling refs,
      collisions under a mapping, the sealed epoch histogram, extent and
      a measured chunk rate), and `ordo_exec::migrate::live_shape`. A
      non-empty probe is a **refusal, not a warning**, and the refusal
      reaches the shell: a cron that reads `!= 0` must not be able to
      gate a deploy on a report it did not read.

      *Defect (a) — a schema-only change was a silent no-op deploy.* The
      artifact document emitted no field `default`, no `normalize`, no
      `enum_values`, no composite `unique_keys` and no declared
      `indexes`, and `program_version` was the digest of that document
      alone — so `ordo apply` printed "already present, byte-identical"
      over a change that was never deployed. Closed twice: the document
      now carries all five, which makes the change **visible** in a
      deploy diff, and `program_version` hashes the document AND the
      generated DDL (NUL-separated), which makes identity **total**
      against the next omission. `registry::store` compares both halves.
      The golden moved by exactly those five keys and nothing else —
      audited key-by-key, entity-by-entity: 18 entities, 59 fields, no op
      version moved, so **this deploy kills no capability grant**, which
      is correct because nothing an op discloses or touches changed.

      *And a correction the plan owed.* §1.1(a) named `unique (a, b)` as
      the worse example on the grounds that the bytes were identical
      across it. They were not: a composite unique key is what
      `writes::set_mode` reads to decide an unconditioned `set` addresses
      a row, so declaring one derives `std.<entity>.put` and the document
      moved already. The genuinely silent cases were `index (a, b)`, a
      changed literal default, and `normalize(lower)` — the last emits no
      SQL at all, so the document was its only possible home.
      `crates/ordo-cli/tests/schema_identity.rs` asserts the correction
      rather than restating it.

      *Defect (b) — the drift check could not see constraints.*
      `live_drift` read `information_schema.columns` only, so a dropped
      `UNIQUE`, `CHECK`, `FOREIGN KEY` or `INDEX` — and the
      `NULLS NOT DISTINCT` modifier, which is the whole difference
      between DL's `null == null` and SQL's — was invisible to the gate.
      It now compares `pg_get_constraintdef` / `pg_get_indexdef` text by
      the deterministic names `ddl_for` mints, one-way as before.
      `crates/ordo-exec/tests/migrate.rs` drops a unique, an index and a
      primary key behind the schema's back and requires each to be named;
      recreates a unique that is byte-identical except for the modifier
      and requires the modifier to be named; and asserts the consequence,
      that a second null row is refused where every other SQL system
      would take it.

      *The classifier is tested against the database its names have to
      match*, not against a hand-built shape: every §5 row that a
      schema-to-schema diff can reach is a two-deploy story — apply
      program P's DDL, read the shape out of the catalog, ask what
      deploying Q would be. That found a real defect:
      `UnindexSealedField` fired only when the whole field was dropped,
      so a `sealed indexed` field that merely lost `indexed` left its
      `$bidx` column live with **no artifact asserting it** — and extra
      live columns are not drift, by the one-way rule, so nothing in the
      system would ever have mentioned it again.

      *Three defects in the report itself*, found by reading code the
      agent that wrote it never ran: a deploy with no schema change
      returned before the GRANTS section, printing "nothing to apply"
      over a re-consent campaign; the C1/C2 probe over a column arriving
      *with* the deploy asked Postgres to `GROUP BY` a column it did not
      have and reported the SQL error as `probe_unavailable`, the code
      for "could not reach the database", when the answer is knowable and
      is usually a refusal (every new row gets the same value, and
      `NULLS NOT DISTINCT` admits one of them per space); and §5 D3 —
      dropping a field an op in the **currently active** artifact reads —
      was ruled *Refuse* in the plan and implemented nowhere. It is now
      `drop_read_by_active_op`, read from the serving artifact's
      field-grain scope rather than from a re-derivation of it.

      *What defect (b) changed about §4.3, which the plan did not
      anticipate.* §4.3 names `DROP NOT NULL` as the one relaxation that
      breaks the currently-serving artifact on its next load. Now that
      the gate sees constraints, **every constraint or index drop has the
      same shape**: it makes the serving artifact drift the instant the
      DDL lands, so it must be sequenced artifact-first. Before this
      increment those drops were invisible and therefore harmless. The
      fix that makes them visible is what makes them sequenced, and
      `plan-schema-evolution.md` §8.1 records it.

      Six taxonomy codes are minted for the reverse direction (`A7`,
      `C7`, `C8`, `D8`, `D9`, `D10`) and six report refusal classes
      (`unseal_refused`, `key_change_refused`, `drop_read_by_active_op`,
      `probe_not_empty`, `probe_unavailable`,
      `sealed_epochs_unverified`); neither the wire nor the compile
      inventory owns either set, so both are tabulated in §8.1 rather
      than left to be spelled twice.
- [x] **13b · Increment 2 — additive apply. BUILT 2026-09-20.**
      `ordo migrate --apply` executes **class A** and refuses everything
      else by name, with `CREATE INDEX CONCURRENTLY` outside the
      transaction and `pg_index.indisvalid` read back rather than
      inferred. `$migrations` holds one row per apply (kind, from and to
      version, actor, the changes as the plan printed them, every
      statement sent, phase `applied`, `failed` or `index_invalid`) and
      each apply lands on the deployment's security stream as
      `EventKind::SchemaMigration` (wire tag 14).

      **The logic belongs to the service** (owner constraint, 2026-09-20:
      "the apply belongs to the service as opposed to a run from the
      client to the database"). It lives in
      `crates/ordo-host/src/schema_apply.rs`: `plan`, `assess`, `probe`,
      `schema_state`, `create_schema`, `triage`, `execute`, `record` and
      the composed `apply`, all over a `Pool` and a compiled `Program`,
      returning structured outcomes; no environment, no printing, no
      `psql`. `ordo migrate --apply` and `ordo apply`
      (`crates/ordo-cli/src/{migrate,operator}.rs`) parse, render and set
      an exit code. 18l adds the door for a hosted control plane.

      **The initial-create gap closed with it.** Before this, `ordo
      apply` computed the DDL only to hash it into the version and stored
      it as text; a first deployment meant pasting it into psql. Now
      `schema_state` reads the target: **empty** (none of the program's
      tables, nothing serving) creates the whole schema through
      `spacedb::apply_schema`, the path `provision::prepare_target`
      already ran for a space's own database; **matches** stores and
      activates; **behind** (the gate would refuse the artifact) stores
      and refuses to activate, naming `ordo migrate --apply`; a registry
      naming a serving version over no tables is refused as
      `serving_schema_drifted`, never created over. The version moves with
      the schema because 13a hashes the DDL into it.

      **One rule beyond class A**, recorded in `corpus-rulings.md`: a
      class-C constraint is applied when it constrains no existing data
      (the table is empty, or every column it names arrives in the same
      apply and its probe is clean by construction). A new ref field
      arrives with its foreign key, as `CREATE TABLE` would have carried
      it. A constraint over existing rows is refused
      (`constraint_over_existing_rows`) with the `NOT VALID` then
      `VALIDATE` statement carried for an operator who has read the
      probe. Drops of every class, including the data-free `A7`, are
      refused (`artifact_first`, `not_class_a`): increment 5.

      **The `check` codes 16a asked for** are minted: `C9` (add a row
      invariant, probed by `migrate::check_violations`, which negates the
      rendered total expression so a null counts as the constraint would
      count it) and `C10` (drop one, the same one-way door as `C6`).

      **`ordo dev` migrates now** (17o's second rung): `dev_db::classify`
      returns `Delta::Apply` when the production triage refuses nothing
      and the loop calls `schema_apply::execute`, skipping only the
      registry and the `$migrations` row; anything refused, and an empty
      database, is the reset as before.

      Proved by `crates/ordo-cli/tests/schema_apply.rs` (6 cases through
      `operator::run`, argv and all, written first and observed failing 0
      of 6: empty database, apply, three rows inserted, a field and a
      constant-defaulted field and an index added, `migrate --apply`, the
      rows still present and readable through the new columns, `apply`
      activates and the version moved; a new entity arriving with its FK;
      a B5 refusal that writes nothing; a `check` refused over a violating
      row and applied over an empty table; a hand-dropped column refused
      as `serving_schema_drifted`; a ghost registry refused rather than
      created over) and by `dev_loop.rs`'s D2 case (a row inserted before
      the additive save survives the apply). Done-when met: a self-hoster
      can add a field to a deployed schema and the deployment keeps
      serving throughout, because the DDL lands before the artifact that
      reads it and the serving artifact tolerates the extra column.

      **Named, not built:** after the artifact that stops declaring an
      index or constraint is activated, the diff has no prior that
      declares it, so the object is never proposed for dropping again;
      increment 5's contract phase needs the *previously* active version
      as its prior (`$activations` holds it). A `sealed indexed` field
      added to a populated table is refused
      (`blind_index_backfill_required`): increment 4.
- [ ] **13c · Increment 3 — the barrier and the plaintext backfill.**
      The `$migrations` state machine with a chunk cursor, attempt count
      and dead-letter (`$jobs` has none of these); the write barrier in
      `Executor::write_attempt` with an advisory-lock backstop; chunked
      backfill on `($space, id)`; verify by count and per-entity content
      hash; and `verify_candidate` refusing to activate over a
      `backfilling` migration. **Unblocked 2026-09-08** by the backfill audit
      ruling (a summarised default, full images per creator;
      `corpus-rulings.md`). Not built. (Corrected 2026-09-29 by the board
      audit: this line said "Blocked on §9" after the ruling.)
- [ ] **13d · Increment 4 — sealed and blind-index backfills.** B7, B8,
      B10 through the worker, never SQL; per-space key lease under one
      freeze; re-seal under the current epoch; destroyed/pruned rows
      refused and counted. Increment 1's epoch histogram is the probe
      this consumes, and it is deliberately a histogram rather than an
      "undecryptable" count: liveness is the custody provider's answer,
      not the database's.
- [ ] **13e · Increment 5 — destructive.** D1 and D2 with explicit
      confirmation and the undo-horizon disclosure. The count is already
      built (`migrate::undo_horizon_loss`, printed by the plan); what is
      missing is the apply that consumes it, and the BYO-DB rule that the
      **space owner** confirms drops.

---

## 14 · Clients & surfaces

`audit-board-coverage.md` §5 proposed a new group for the surfaces a client
actually consumes — the capability catalog (already shipped), the interface
descriptor and the generated SDK, surface descriptors, log-positioned reads.
This is that group. Nothing above is re-opened or renumbered; the catalog's
own rows stay where they are (5e, 8r), because this group is about what is
built **on** it.

- [x] **14a · B1 — the Interface Descriptor, built, scoped and verified.**
      The one data contract the generated SDK, the LSP and generated UI all
      hang off (`tooling-and-embedding-plan.md` §B1, §B3; `ui-gen.md`
      §1–§4). `ordo_plan::interface` + `ordo interface <dir> [--out <dir>]`
      + `GET /interface` (alias `/.well-known/ordo-interface`).

      **It is a projection, and that is the whole design.** Ops come from
      the catalog manifest and entities from `entity_artifacts`, both
      projections of `op_artifact` — the one producer 9m/9l collapsed the
      second walk into. The module takes JSON in and JSON out, so it
      cannot reach a schema or a binder even by accident, and
      `ordo-cli/tests/interface.rs` joins the two documents in **both**
      directions: every op, param, output, statement, entity and field is
      the artifact's, byte-equal; and a written-down list of the keys §B1
      and `ui-gen.md` settle is asserted present on every entry, with a
      non-vacuity guard so a shrunken document cannot pass.

      *The producer grew four facts rather than the descriptor deriving
      them* — the same rule stated from the other side:

      - **`params[].id_entity`.** The type system spells every id `id`, so
        *which* entity an id parameter addresses is carried only by the
        body (`id == $id` on `tasks`). `ui-gen.md` §2's settled action-
        discovery rule needs it, the generated SDK's branded ids need it,
        and a detail link needs it — three readers, one answer. Derived
        conservatively: only a comparison or `in` whose other side is
        exactly a parameter, paths walked through refs, ambiguity (the same
        parameter addressed against two entities) resolved to `null`. A
        parameter it cannot type costs a UI one row action; one it typed
        wrongly would offer an action that does not apply.
      - **`outputs[].refinable`.** `FieldSpec`'s own doc says the
        `sealed indexed refinable` opt-in exists so the capability "lands in
        the manifest and therefore on the consent page". It landed in
        neither: it was computed in `ordo-host`'s spec builder, enforced at
        the door, and absent from both the artifact and the catalog. A
        disclosure that only the enforcement layer knows about is not a
        disclosure.
      - **`outputs[].id_entity`** — `ui-gen.md` §1's detail linkage, so a
        reader can link an id column without parsing a prefix out of a value
        it has not fetched.
      - **`result` — `{anchor, grouped, single_row}`.** Row identity (the
        anchor's typed id rides every list row) and §5's *structural* metric
        test. Only the settled case is implemented: `reduce` is a keyless
        `group` and a keyless group is provably one row. §5's other two
        cases are marked `[proposed]` there, and a descriptor that
        implements a proposal has to change when the proposal is ruled on.

      **Catalog scoping is one filter, not two rules.** The route filters
      manifests with the same check the door runs and *then* describes them,
      so the descriptor a caller receives contains exactly the ops their
      catalog shows — asserted as an equality in both directions, for a
      manager and a viewer. Everything derived across ops follows the
      filter: `tests/interface_surface.rs` asserts that a manager-only op's
      name appears **nowhere** in a viewer's serialised document (not in
      `ops`, not in the typed-id index, not in a guard's rendered text), that
      the entity half equals exactly what the viewer's own catalog reaches,
      and that a refusal list is identical for both callers on a shared op —
      a refusal that varied by asker would be a channel for comparing
      catalogs. Anonymous gets a well-formed **empty** document, never a 403.

      **§B3's terminal verbs are stamped**, so a renderer needs no name
      heuristics: `call` carries the method, the verb a builder must end in
      (`fetch`/`live`/`pages` vs `commit`), CAS-by-default, refinability,
      liveness and paging. `ordo_api::method_for` is now public and the two
      spellings are joined over the whole registry, the pattern
      `tests/cost_class.rs` uses.

      **Shape verification** closes onto the load gate: every op in the
      gate's `verified` bucket must have a descriptor whose columns — names,
      order, sealed marks, refinability — are the ones Postgres described.

      **Refusal affordances.** Each op names the subset of the closed wire
      inventory its own declaration makes reachable (a required param, a
      declared `<=` bound, a unique key, a `check`, a tree edge, an inbound
      unowned ref, a key release). It mints no class — asserted against
      `ErrorCode::ALL` — and every derivation rule has a witness, including
      the two the market fixture cannot provide.

      *Identity and the golden.* `schema_version` + a 16-hex `digest` over
      the document with the digest key absent, recomputable by a reader.
      Golden: `crates/ordo-cli/tests/golden/market.interface.json`.

      *The artifact golden moved, audited key by key.* 165 ops gained
      `result`, 490 `id_entity` values and 243 `refinable` values; 18
      entities gained `checks`. **Every op version moved and nothing else
      did** — no `plan.sql`, no `scope`, no `row_scope`, no `guards`, no
      `cost`. That is correct rather than collateral: what an op discloses
      genuinely grew, so a capability grant pinned to the old manifest must
      be re-consented, which is exactly what the pin is for.

      *Found while building, and left as findings:*

      - **`check` was the sixth silently-omitted declaration**
        (`plan-schema-evolution.md` §1.1(a)'s family). `check used <= seats`
        reaches `ddl_for` as a real CHECK constraint and reached the artifact
        document nowhere, so a deploy that tightened or removed a row
        invariant showed a reviewer no line. Fixed the same way the five
        were: the document now carries `{name, source}` per rule. **Not
        fixed, and owed:** `ordo_schema::diff` has no taxonomy code for a
        check at all, so `ordo plan --against` cannot classify one — the
        constraint is invisible to the migration report in both directions.
      - **`ordo_api::FieldSpec` carries no type.** The wire's own output
        manifest has `name`/`sealed`/`refinable`/`refine_class` and nothing
        that says what a column *is*. Compiled ops are unaffected (their
        types come off the bound query), but a **platform op** is the one
        kind whose result columns nothing in the tree can name a type for,
        so `std.space.*` descriptors carry `"type": null`. Recorded in
        `platform_ops::manifest` beside the null rather than papered over
        with a guessed `text`.
      - **`catalog_view` lived in the wiring crate.** Every field it selects
        is compiler output, which is `artifact.rs`'s own layering argument;
        it was in `ordo-host` because that is where the catalog *route* is.
        Moved down to `ordo_plan::artifact`, which is also what lets a CLI
        with no `Fixture`, pool or HTTP stack describe a program that has
        never met a database. `ordo-host` keeps what it owns: who may see
        which op.

      ***`ordo.interface/2` — the seven gaps 14b reported, closed*** (ruling
      dated 2026-09-07 in `corpus-rulings.md`). Six of the seven had a correct
      workaround in the consumer, and that is why they were worth closing: a
      package whose argument is *"no op-name heuristics anywhere in the
      source"* had six of them, in the one document that exists so renderers
      need none. **The producer grew five more facts; the descriptor derives
      the three joins**, and the split is the same rule as before — the
      producer works one op at a time, so a cross-op or cross-document join
      (`enum_values` against the entity half, the listing index over every
      visible op) has to happen after the catalog filter or it is a leak.

      *Why the number moved once, to `/2`.* Four gaps are additions and would
      have stayed `/1` alone (`id_types[].list`/`.get`, `params[].field_ref` +
      `enum_values`, `params[].min`, `result.shape` +
      `statements[].summary`). Three are **changed meanings**, which is the
      case `SCHEMA_VERSION`'s own doc names: `call.cas` meant *is a mutation*
      and now means *asserts a revision*; `call.refinable` was `true` against
      a `none` tier and is now a projection of it; single-row ops no longer
      advertise a second page. A `/1` reader keeping those meanings gets
      wrong answers rather than missing ones. A document cannot be `/1` for
      some keys and `/2` for others, so the additive four ride along — and
      **no fallback was kept in the consumer**, because `parseInterface`
      refuses `/1` outright and a heuristic that can never run is one nobody
      maintains.

      - **The index keyed on what an op RETURNS.** `id_types` indexed only
        what an op *accepts*, so a picker behind an `id_entity` param was
        reconstructed from `result.row_identity` — ranked `std.E.list` first
        **by name**, and unable to tell *"nothing lists this"* from *"my rule
        missed one"*. `list`/`get` derive from `ui-gen.md` §1's row identity
        (an op lists `E` iff its rows carry `E`'s typed id), split on whether
        a required `E` id addresses it; authored `myProjects` and `openTasks`
        qualify by the same test, and 39 of the golden's listing entries are
        mostly authored. `members` carries an empty `list`, **stated** — it is
        platform-provided, `std_ops` skips it by flag, and nothing returns its
        rows. Whether a listing can stock a *blind* picker stays the
        consumer's policy over `params[].required`, which it already holds.
      - **`get` is not `detail`, and that was a live bug.** `detail` is every
        read accepting the id whatever it returns; the consumer's `cas_miss`
        repair refetched through `detail[0]` and was right in this fixture by
        alphabetical luck. Presenting another entity's row as "the current
        value" after a conflict is the failure the conflict state exists to
        prevent.
      - **`params[].field_ref` + `enum_values`.** The field carried the value
        set and the parameter filling it carried no link, so the consumer
        joined `statements[].entity` to a field of the same *name*. The AST
        knows better, and knows more: `recentOrders($since)` filters
        `orders(placed >= $since)` and a read has no statements at all. It
        came free — `param_id_entities` was already walking these sites and
        discarding the field; it is now `param_sites`, one walk, two facts.
        Declared fields only, and `null` never `[]`.
      - **`statements[].cas_param`.** CAS is a revision assertion, so it is
        read off the braces where DL §6 rule 9 puts it. 18 ops keep
        `call.cas`, 71 lose it — every one a create, put, delete or an
        addressed converge that never named a `rev` — and `cas_miss` leaves
        those 71 refusal lists with it, because a refetch-and-remerge
        affordance for a conflict the op cannot raise is exactly what the
        "derived from a stated declaration" rule forbids. CAS-by-default is
        unweakened; the document stopped claiming it of ops that never had it.
      - **A single-row op is `fetch`/`live`.** `Executor::read_page_as`
        already found a keyless `reduce` unpageable, answered `next: null`
        and refused a presented cursor. 17 ops. `invalid_cursor` stays — the
        door still answers it — but the descriptor stopped inviting one.
      - **`call.refinable` is `refine.tier != "none"`.** Two answers to one
        question became one; the structural half was never a key and is
        `kind == "read"`.
      - **Bounds, both halves, with the truth about each.** `params[].max`
        was never dead — `declparse::param_bound` produces it — it read that
        way because the market fixture declares no bound, which is also why
        the consumer's `typeof max === 'number'` branch was dead code that
        looked live (the emitter canonicalises to **digit strings**, always).
        `min` had a producer too, in the wrong crate: `ordo_host::
        param_bounds` derived the `take`-count floor and handed it to the
        door, and nothing else knew. `take_count_param` moved down to
        `ordo_plan::artifact`; `bound_violation` now derives from either. The
        witness test asserts both values *and* that `>=` still does not parse
        — the day it does, `min` stops being derivation-only.

      ***A write does not return the row, and §3 says it does.*** The one
      place describing the deployment honestly meant contradicting a settled
      doc. `api-engine-design.md` §3 (echoed by `std-library.md` §2 and
      `definition-language.md` §4) promises the written row post-`rev` as
      `rows[0]`; the executor answers every write with one summary per
      statement — `{entity}` plus `created`/`converged` (an id) or
      `updated`/`deleted` (a count) — and the post-image is re-read only into
      the command log's `after`. So `result.shape` says `write_summary` and
      `statements[].summary` says which keys, per statement, because a
      transfer that creates one row and converges another puts two shapes in
      one response. A caller does **not** learn the new `rev` from a commit.
      *Owed, and an engine change rather than a descriptor one:* either the
      executor returns the row as §3 says, or §3 and its two echoes are
      amended. Both cannot stand.

      *The goldens moved, audited key by key.* **Artifact:** every op gained
      `result.shape`; 247 params gained `min`, 157 gained a populated
      `field_ref`; 89 statements gained `cas_param` and 88 a `summary`. All
      165 op versions moved and **nothing else did** — no `plan.sql`, no
      `scope`, no `row_scope`, no `guards`, no `cost`, no entity — which is
      correct rather than collateral: `result.shape` is a new disclosed fact
      on every op, so every grant pinned to an old manifest must be
      re-consented. **Interface:** the same, plus `enum_values` on 242 params
      (populated on 5), 39 `list` and 16 `get` entries, `cas` on 71 ops,
      `refinable` on 76, `paged` and `terminal` on 17, `cas_miss` off 71
      refusal lists, and `schema_version` + `digest`. Nothing outside the
      seven gaps moved.

      *Found on the way, and left as findings:*

      - **A write refused by rule 8 reads as callable.** `claimCoupon` covers
        two unique keys and the fixture comments it *"Refused at compile
        time"*, but its artifact `error` is `null` — the artifact records only
        read-bind failures — so the descriptor tells a renderer the op is fine
        and the door refuses it. `renumberEntryOnce` is the same shape.
        **Owed:** surfacing write-registration refusals into `error`.
      - **`text(n)` does not parse.** DL §2 writes `$payee: text(64)` and
        `param_bound`'s own error message recommends it, but `scalar_ty`
        reads the bare word into a `ScalarTy::Text` that carries no length, so
        the spec's example is a parse error at the following token. **Owed:**
        either the grammar slot or the spec.
      - **A platform op's text length reached no document.** `ParamTy::Text {
        max_len }` is enforced at the door and `platform_ops::manifest`
        emitted `"max": null` regardless, so two of the three claimed
        unbounded about a parameter the door measures. Fixed here, and
        unambiguous because `<=` on `text` is refused — a `max` on a `text`
        parameter can only ever be a character count.
- [~] **14b · The headless runtime** (`@ordo/react`) — fetch, paging,
      forms, action discovery and commit **built**; live updates and the
      device copy **built 2026-09-29** (opt-in on `OrdoProvider`); the
      page-level snapshot coordinator **not**. `clients/ordo-react/`, the first JavaScript in
      this repo (`ui-gen.md` §2). Consumes 14a and nothing else: React is a
      peer dependency and the package has no others.

      **It is a runtime over the descriptor, not a generated client.**
      Nothing is emitted; the package reads `interface.json` at run time and
      derives behaviour from it. The wire method comes from `call.method`,
      the legal call-site verbs from `call.terminal`, the version pin from
      `op.version`, the row actions from `id_types`, the form from
      `params[]`. There is no op-name heuristic anywhere in the source,
      which is the property 14a's `call` stamp was there to buy. It is
      therefore not 14d: the SDK is a *pure function of* 14a giving a caller
      compile-time types, this is what an app holds at run time, and where
      they overlap the SDK becomes the static enforcement of what this
      enforces dynamically.

      **CAS is unavoidable, not merely available.** `ui-gen.md` §1 settles
      that the fetched row's `rev` rides the write and `cas_miss` is a typed
      conflict resolved by refetch. Three rules make that true rather than
      offered: a CAS op committed with no `rev` is refused **before the
      wire** with the door's own `missing_param` (there is no
      last-write-wins path through the package); `cas_miss` becomes a
      conflict state carrying the attempted params, the rev that lost, the
      row's address and a `refetch()` through the catalog's own detail op,
      with **exactly one** request made; and a retry re-asserting the stale
      rev is refused locally, so "retry" cannot quietly mean "retry the
      losing write". Where the write's address is ambiguous the runtime
      answers `null` rather than refetch the wrong row and present it as
      "the current value".

      **`cas_miss` and `version_skew` are kept apart**, and that is a
      judgment call recorded in `corpus-rulings.md` (2026-09-07). Both are
      409 conflicts, but the repairs are opposites: one refetches a row, the
      other drops a cursor and restarts the listing from page one (§8 — the
      token is not migrated, and nothing about the rows disagreed). A
      runtime that folded them would put a refetch-and-remerge dialog in
      front of someone who was scrolling a table.

      **Paging terminates on a missing `next` and on nothing else.** §7's
      consequence, implemented rather than rediscovered: a Tier-0 refinement
      applies after the page window, so short and empty pages are ordinary.
      `hasMore` is `next !== undefined`, never `rows.length > 0`; the empty
      state is a fact about the whole iteration; and `maxPages` leaves
      `hasMore` true when it trips, because the runtime stopped and the
      listing did not end.

      **Refinement is refused locally while every op declares tier `none`.**
      `call.refinable` is `true` for every read and `refine.tier` is `none`
      for every op — the first is structural, the second is the permission,
      and a runtime trusting the first would earn `invalid_refinement` on
      the whole catalog. `refinementSurface(op)` answers "sort-only" with
      the field list, so a skin greys the right columns instead of learning
      them from a rejection.

      *Proof.* 89 vitest tests plus compile-time assertions, all against
      `crates/ordo-cli/tests/golden/market.interface.json` and a mock door —
      no server, no DOM, no fixture of our own. They cover form derivation
      per param type over all 165 ops, action discovery for **every**
      `id_types` entry (including that each discovered op's bound param is
      really an `id` param addressing that entity), cursor threading, the
      empty-page trap, the stale-cursor restart, the whole CAS path, the
      unknown-major refusal, and a mock round trip per terminal verb. The
      core is pure and the hooks are adapters over it, which is what lets
      all of that run without a render.

      *Found while building, and left as findings for the descriptor's next
      version* — every one of them a fact a renderer needs and `ordo.interface/1`
      does not carry. **All seven are closed by `ordo.interface/2` (14a
      above); the list is kept verbatim because it is the evidence the
      version moved for, and because what each workaround was is the reason
      the fact had to be stated.** The package now speaks `/2`, refuses `/1`
      loudly, and carries none of the reconstructions below:

      - **Nothing indexes the list op for an entity.** `id_types` indexes
        ops by the id types they *accept*, never by the entity they *list*,
        so a picker behind an `id_entity` param has to be reconstructed from
        `result.row_identity` plus "no required params". The reconstruction
        is honest but it is a heuristic in a package whose whole argument is
        that it reads facts. Worse, it is *empty for `members`* in the
        market fixture: the catalog grants no unparameterised read anchored
        on the platform entity, so every `assignee`-style reference is a
        blind picker. The package reports `candidates.blind` rather than
        render an empty dropdown.
      - **A param carries no `enum_values`.** The entity field does, and
        `interface.rs` says of it "a dropdown is derivable from this and
        nothing else" — but a write param that fills that field is spelled
        `text`, with no link back. The package joins through
        `statements[].entity` and refuses to guess when two written entities
        both have a field of that name.
      - **A write's `result` is empty.** `api-engine-design.md` §3 says an
        addressed write returns the written row post-`rev` as `rows[0]`, and
        the descriptor gives a renderer no shape for it (`outputs: []`,
        `row_identity: null`). So a form cannot know it will get the new
        `rev` back, which is precisely the value the next CAS write needs.
      - **`call.cas` is true for every write and delete**, including ops
        with no `rev` param at all — it means "is a mutation", not "asserts
        a revision". The package detects CAS from the `rev` param instead
        and treats `call.cas` as corroboration.
      - **`single_row` ops still advertise `terminal: [fetch, live, pages]`
        and `paged: true`.** Only `result.single_row` says otherwise, so the
        call contract and the result contract disagree about whether there
        is a second page.
      - **`params[].max` is present but never populated** anywhere in the
        165-op golden, and there is no `min` at all — so the bound half of
        §5's binding rules is untested against real data, and a `text(n)`
        length limit reaches no form.

      *Owed, and deliberately not done here:* live (`?live=1` SSE) and
      page-level snapshot coordination, which need a log position on the
      read path that 14a does not carry — a hook-level approximation would
      ship the exact failure §2's settled design names. Optimistic apply and
      typed rollback, which belong on top of the conflict machine and not
      underneath it. Hook-level render tests, which need
      `@testing-library/react` and `jsdom`. And **the JS build is not wired
      into cargo or CI**, so a descriptor shape change can break this
      package silently.
- [ ] **14c · Surface descriptors** (`surfaces.json`) — `ui-gen.md` §1.
      14a plus the reserved UI annotations, which the grammar does not lex
      yet (`definition-language.md` §10.3): that lexing is the blocker, and
      it is a language change, not a descriptor one.
- [x] **14d · The generated TS SDK — built, generated, and pinned to a fresh
      run.** `tooling-and-embedding-plan.md` §B2–§B3, `implementation.md`.
      `crates/ordo-gen` + `ordo gen ts <dir> [--out <root>]` +
      `clients/ordo-sdk/`. A pure function of 14a, terminal-verb split
      included.

      **The architecture is generator-first, and that is the ruling this
      row implements** (`corpus-rulings.md`, 2026-09-07). Both TypeScript
      packages are generated from the implementation's own canonical
      tables — never hand-maintained — so language evolution is
      regeneration, not rewriting. The SDK and **14f**'s CDK come out of
      one crate in one command, because a package whose two halves came
      from two commands is a package that ships with one of them stale.

      **Two halves, and the header on every emitted file says which.**
      `src/core/` is hand-written, hand-versioned, and the same bytes for
      every deployment: the `fetch` wrapper, the header set, the keyset
      page loop, the CAS rules, the refusal decoder, the id brand.
      `src/generated/` is one typed function per op, its parameters, its
      row shape, its refusal union, and the op table the core reads. **The
      generated half declares; the static half decides** — code in the
      generated half is code nobody can fix, because the fix is lost on the
      next `ordo gen ts` and the person who lost it finds out in
      production.

      There is **no op-name heuristic anywhere in the package**: the wire
      method comes from `call.method`, the legal verbs from
      `call.terminal`, the version pin from `op.version`, the CAS parameter
      from `statements[].cas_param`. That is the property 14a's `call`
      stamp was there to buy, cashed a second time — 14b cashed it at run
      time, this cashes it at compile time.

      *What the types buy.* The terminal-verb split as three classes, so a
      paged read has `.pages()`, a single-row read does **not have the
      method at all**, and a write has `.commit()` and no read verbs
      (§B3: every mutation is `grep '.commit('`). Branded ids per entity,
      out of `id_entity` — the fact the type system erases and `/2` had to
      restore. Enumerated parameters as the closed union `enum_values`
      resolved. Refusals as the op's own declared set, not the whole table.

      *What it refuses before the wire*, in the door's own bind order and
      wearing the door's own classes: unknown keys first, then each
      declared parameter, then bounds. **CAS is unavoidable**, as in 14b:
      an op that asserts a revision and is committed without one is
      refused locally, so there is no last-write-wins path through this
      package either. Validation runs in the `Call` constructor rather
      than at the terminal verb, so a bad argument is refused at the call
      site instead of arriving as a rejected promise from a line the
      caller has not reached yet.

      *Proof.* 25 vitest tests against `market.interface.json` and a mock
      door, plus five Rust tests that pin the checked-in generated half to
      a fresh generator run byte for byte, assert every emitted file
      announces itself, refuse a descriptor from another version by name,
      and catch a hand-written file hiding in a `generated/` directory —
      which would otherwise survive every regeneration while looking
      generated.

      *Owed, and deliberately not done here.* **`.live()`** — the SSE verb
      is stamped on 76 reads and this generator emits no method for it, for
      the reason 14b deferred live: the descriptor carries no log position
      on the read path, so a subscription's first frame and its
      invalidations cannot be ordered against each other by anything the
      client can see, and a `.live()` that reconnected and refetched would
      ship the exact torn read §2's settled design warns about. One answer
      covers both packages. **Optimistic apply and typed rollback**, which
      belong on top of a conflict machine and not underneath it.
      **Call-site refinement** (§B2), which would earn `invalid_refinement`
      on the entire catalog while every op declares tier `none`. **A
      `cas_miss` conflict machine** — the typed refusal is raised and the
      repair is left to the caller, because it is a UI decision and a
      client library that made it would make it for everybody.

      *Found on the way, and left as findings:*

      - **`@ordo/react` and this package carry the same wire twice.** Both
        hold the closed refusal inventory verbatim and both send the same
        headers to the same routes. They are kept independent on purpose —
        one reads a descriptor at run time and one embeds it at build
        time, and neither should acquire the other — but two copies of a
        closed set is the defect this repository spends its life refusing.
        **Owed:** a third package (`@ordo/wire`) holding the envelope, the
        header names and the classes, which both depend on. Until it
        exists, `test/generated.test.ts` pins this copy against every
        class named in the golden descriptor.
      - **A generated params type must be a type ALIAS, not an
        `interface`.** Only an object alias carries an implicit index
        signature, and the client takes parameters as a record; an
        `interface` would have needed a cast at every generated call site,
        which is a hole in exactly the code nobody reviews.
      - **`money` is an exact decimal string, and §B3 said otherwise.**
        The plan proposed "bigint-minor-units in TS" before the wire was
        settled; `api-engine-design.md` §5 settles it as a decimal string
        and the descriptor's bounds are decimal digit strings.
        **Owed:** a one-line correction to §B3.
      - **The JS build is still not wired into cargo or CI** — the same
        debt 14b and 14e already owe, now owed by three packages. The
        generated half *is* pinned from cargo (above), so a descriptor
        shape change cannot break these packages silently any more; what
        is still unwired is `npm run check` itself.
- [x] **14e · Editor tooling, increment 0 — the TextMate grammar, and the
      test that pins it to the lexer.** `docs/plan-editor-tooling.md`,
      `editors/ordo.tmLanguage.json`,
      `crates/ordo-syntax/tests/editor_grammar.rs`. The plan is the whole
      ladder (VS Code, `ordo-lsp`, JetBrains, distribution); increment 0 is
      what is built.

      **One artifact, four hosts, and that is the design.** A TextMate
      grammar is the only highlighting format VS Code, JetBrains' bundle
      importer, shiki (the docs site) and a Monaco/CodeMirror bridge all
      consume unchanged. Before it there was no highlighting anywhere:
      checked, not assumed — the playground edits Ordo in three bare
      `<textarea>`s and fakes comment colour with hand-written `<i>` tags,
      and every doc fence is untagged. So this replaces nothing; it fills
      four empty slots from one file.

      *Scopes are split by what a word does*, not flattened onto one
      `keyword` colour — declaration heads, clause heads, pipeline verbs,
      write verbs, word-operators, field modifiers, registration flags — and
      `unguarded` has a scope of its own, because §8.5 says bypass is
      "explicit and greppable" and a scope shared with `unbounded` could not
      make it loud. Guards are a begin/end clause that survives a line break
      inside open parens, which is G6 encoded as a grammar rule. Typed ids
      are matched **inside string literals**, because `_` is not an
      identifier character and a bare typed id cannot lex. And the grammar
      refuses what the lexer refuses — snake_case, `!`, single quotes,
      unknown escapes, unterminated strings, unknown duration units, `$3`, a
      schedule time without its `Z`, an event kind outside the closed seven,
      and dates that name no day.

      *The parity test is the point.* A second statement of a closed
      vocabulary that nothing checks is a second answer waiting to differ,
      so `editor_grammar.rs` checks it both ways in the spirit of
      `docs_parse.rs`: the grammar's reserved list **is** `Keyword::ALL`; a
      keyword added to the lexer fails by name; and a word the grammar
      colours as reserved that the lexer treats as an ordinary field name
      fails too — the half that protects the two-tier design `count`,
      `read` and `open` depend on. `dual` names the one word that is
      genuinely both (`distinct`). Five borrowed closed sets are pinned to
      their producers rather than retyped. Five mutations were run; each
      fails the intended test and nothing else.

      *Three vocabularies collapsed onto one table each* to make that
      possible without inventing a fourth list — a net deletion:
      `Keyword` and `DurationUnit` are declared once through `word_enum!`
      (enum, `parse`, `as_str` and the new `ALL` are projections of one word
      list); `AGGREGATE_NAMES` replaces a display string that was a second
      list of the same names; `function_names()` exposes the expression
      functions without leaking arities nothing outside the parser needs.

      *Verified against the real engine*: every fence in
      `syntax-examples.md`, `tutorial.md` and `definition-language.md`
      tokenized with `vscode-textmate` + `vscode-oniguruma`. Zero false
      refusals; twelve true ones.

      *Found on the way, recorded and not fixed here:*

      - **`definition-language.md` §2's reserved list is not the lexer's.**
        It carries `run open route read write std`, which `token.rs`
        deliberately keeps contextual, and omits `compliance space auth`,
        which the lexer reserves. The doc promises the list is
        "mechanically regenerated from the grammar"; it is not, and the
        grammar's manifest is now the thing that is. A doc-truth fix.
      - **Four doc defects nothing had read mechanically.**
        `transfers.from_account`, `at: due_date`, `$GOOGLE_CLIENT`,
        `$DISCORD_CLIENT` — all illegal under §2's one identifier rule, all
        in `definition-language.md`, whose fences `docs_parse.rs` does not
        cover. Either fix the four and add the file, or state why its fences
        are prose.

      *Owed, and deliberately not done here:* the tokenizer harness that
      verified the corpus lives in a scratch directory, not the repo, because
      checking it in puts a node toolchain in a Rust workspace's CI — the
      same unanswered question 14b's "the JS build is not wired into cargo or
      CI" already owes, and one answer should cover both. And the LSP
      (increment 2) is **gated on the trigger/routine declaration form**,
      which `trigger.rs` records as unsettled: `parse_program` accepts
      `domain`, `entity` and `op` and nothing else, so a server today would
      underline a third of the settled language as an error.

- [x] **14f · `@ordo/cdk` — the typed TypeScript builder, built as the second
      consumer of 14d's generator.** `tooling-and-embedding-plan.md` Track C,
      `clients/ordo-cdk/`. Added 2026-09-07 on the owner's interest;
      previously untracked BY RULE (the coverage audit's #24: "a plan is not
      a gap"). The sequencing argument held — the CDK's honesty rule is that
      the builder API is *generated from the same grammar definition the
      parser uses*, and that is 14d's generator with a second backend.

      **Done-when, against the plan's own bar.** `synth` emits canonical
      `.ordo` — with one honest qualification below — and **isomorphism is
      a conformance-tested property**: 31 of 31 corpus programs round-trip,
      with no builder misses, over both conformance fixtures and every
      `syntax-examples.md` block `parse_program` accepts.

      **The skeleton/vocabulary boundary, stated as a rule.** *A shape is
      the skeleton's; a word is the generator's.* That a comparison has a
      left, an operator and a right is a shape (`src/core/`, hand-written,
      reviewed once); that the operators are `== != < <= > >=` and are
      spelled `.eq() .ne() .lt() .le() .gt() .ge()` is vocabulary
      (`src/generated/`, emitted). The line is there because of what each
      side costs when it drifts: a drifted *word* is a builder that can
      express what the language cannot, caught by isomorphism and prevented
      by generation; a drifted *shape* is an API redesign, which is a human
      decision and must not arrive as a diff on a generated file. Two seams
      cross it, both in `core/value.ts` — `Value` is an interface with no
      members that the generated vocabulary merges operators into, and
      `installValue()` is the run-time half of the same seam.

      **Three gates a new keyword passes, or none.** `spell.rs` matches
      `Keyword` (and `ScalarTy`, `CmpOp`, `TextOp`, `BinOp`,
      `DurationUnit`, `EventKind`) exhaustively with no wildcard arm, so
      (1) `cargo build` fails if a word has no decision — the generator
      does not compile, so `ordo gen ts` cannot run, so neither package can
      be regenerated. (2) `vocabulary.ts` publishes every decision, and
      `test/vocabulary.test.ts` fails if an offered spelling resolves
      nowhere. (3) the corpus fails if an implementation says something the
      parser refuses. Three mutations were run against the producers and
      are recorded in `tests/ratchet.rs`; a source scan keeps the arms
      wildcard-free, which is the half `rustc` cannot check.

      **Absence is a decision, and it is published with its reason** —
      split the way `docs_parse.rs` splits its allow-list, because the two
      are owed differently. `does-not-parse`: the *language* refuses it, so
      an absent method is the language's refusal one layer earlier, and
      nothing is owed until the grammar moves (`when`, `every`, `trigger`,
      `routine`, `principal`, `client`, `auth`, `policy`, `compliance`,
      `channel`, `app`, `page`, `each`, `space`, `filled`, and all seven
      event kinds). `out-of-scope`: a real debt — **exactly one word**,
      below, and a test asserts it is the only one.

      *Scope, stated in the README and here.* Covered: entity declarations
      with every field modifier, guards (including the parser's DNF
      distribution), `acl`, composite `unique`, `index`, `check`; ops with
      parameters, bounds, defaults and all three registration flags; the
      whole read pipeline; and **writes** — `set`/`del` are in the op
      grammar, so `b.set(…)`, `b.del(…)`, `.where()`, `.subtree()` and
      `.let_()` are all here. Not covered: **`whole`**, which is a special
      form rather than a pipeline step and does not compose — a builder for
      it is a second skeleton, not vocabulary poured into this one, and the
      corpus excludes `whole` blocks by name rather than silently.

      *"Canonical", honestly.* The plan says `synth` emits canonical
      `.ordo` **via `ordo fmt`**. There is no `ordo fmt` — Track A1 is
      unbuilt — so the property checked is **parse-and-reparse equality**:
      `parse(original)` and `parse(synth(…))` are the same AST, spans
      excepted. Weaker than byte equality, stronger than nothing, and it is
      the claim the test makes. When `ordo fmt` lands it becomes the
      normalizer on both sides and the claim tightens to bytes.

      *The isomorphism harness, and why it is shaped that way.* The
      AST→builder translation is **machine-written** (`ordo_gen::emit`),
      matching every AST enum exhaustively: a hand-written set of builder
      programs would exercise only the nodes whoever wrote it thought of,
      and a node kind with no builder spelling here either fails the build
      or is reported by name in the score. The loop crosses a language
      boundary and `cargo test --workspace` must stay green without node,
      so the synthesized `.ordo` is **checked in**: vitest asserts `synth()`
      still produces it, cargo asserts it parses back to the original tree.
      A Rust test that shelled out and skipped when node was absent would
      leave the property unverified on exactly the machines that skip.

      *Found by the corpus, and fixed here* — both were the builder saying
      something `.ordo` cannot, which is the direction that matters most:

      - **A subtree delete must print back in the BRACE form.** The parser
        lowers `del E { id: $id }` into `id == $id`, so the spelling is
        unrecoverable from the tree — ordinarily fine, since both are
        "addressed" with identical semantics. But rule 6 refuses `subtree`
        after a *ranged* delete, and ranged is decided by the spelling
        written, not by what the condition says.
      - **A distributed guard walk needs its parentheses.** `(a and b) or
        (c and d)` was printed flat and the guard grammar refuses mixed
        connectives without parens. Found on `syntax-examples.md`'s
        property manager.

      *Found on the way, and left as findings:*

      - **The demo page's sketch and the plan's sketch disagree with each
        other and with the grammar.** `site-animations/process-flow.html`
        shows `q.from(…)`, `.join(…)`, `.take('plants.id', 1)`,
        string-valued `.where('at <= $today - everyDays or at is null')`
        and `alerts.upsert(…)`; Track C shows `q.source(…)`, `.bar(…)` and
        `f.x.eq(…)`. The string predicate is a second parser, `join` and
        `upsert` are not in the language, and `.bar` names a character
        rather than a step. **Owed:** the README tabulates all nine
        divergences with reasons; the demo page and Track C's sketch should
        be corrected to match what shipped.
      - **`allocate`'s third argument is not an expression, and no arity
        table can say so.** It is a bare mode word the parser checks at the
        written form, so a generator reading only `(name, min, max)` would
        type it as a value and emit a builder whose output does not parse.
        Special-cased, and the exception is recorded beside the probe that
        found it.
      - **`$caller` is a parameter reference that no op declares**, so a
        translator spelling parameters from the op's own list emits
        `p.caller` and finds nothing. Caught by `tsc`; the next ambient
        parameter will look exactly as surprising.
      - **`noUncheckedIndexedAccess` is off in this package and on
        everywhere else.** `f` is a total proxy and an index signature is
        the only way TypeScript can say so; with the flag on, every field
        reference in every program written with this builder would carry a
        `!`, which is strictly worse. Recorded rather than hidden.

- [~] **14g · The door sends no CORS headers, so no browser page on another
      origin can call a deployment. RULED 2026-09-28: an allowlist.
      Built 2026-09-29 for the environment; the console setting is owed.**
      `ORDO_CORS_ORIGINS` lists exact origins; unset keeps the door
      same-origin. One layer, `ordo_api::cors::apply` in
      `crates/ordo-api/src/cors.rs`, wraps the whole door in
      `ordo_host::runtime::door()`: an allowed preflight answers 204 with the
      Allow headers, any other origin's preflight answers 403, and real
      responses carry the origin and `Vary: Origin`. `*` refuses the boot,
      because every call carries a bearer token and a wildcard would let any
      site's script use one. Credentials are never allowed. Tests:
      `crates/ordo-api/tests/cors.rs`, and the assembled door in
      `ordo-host/tests/supervisor.rs`. **Owed:** setting it from the
      console, which needs a writable deployment setting in the system space
      (no such platform op exists; behind 18o), and changing it without a
      restart. Found 2026-09-26 building the first
      app (`examples/first-app`), which is the reason it was built.

      **Verified against a running door, not inferred.** A cross-origin
      `GET /op/listReadings` carrying `Origin: http://localhost:5173`
      answers `200` with no `Access-Control-Allow-Origin`, so the browser
      receives the body and discards it. The preflight is the harder half:
      `OPTIONS /op/listReadings` answers **405**, because no route method
      admits it. And every Ordo call carries `Authorization` and
      `X-Ordo-Space`, which makes every browser call a preflighted one, so
      the 405 is the common path rather than an edge case. `grep -i cors`
      across `crates/` finds nothing, and there is no static file route
      either, so today a browser page can reach a deployment only from the
      deployment's own origin.

      **Whose problem it is first.** `14b`'s `@ordo/react` is a browser
      runtime that reads `/interface` and calls `/op/*` at run time. Every
      app it is for is a page on some origin. A bundler's dev proxy hides
      this locally by default, which is how it has stayed invisible: the
      first app found it only because it was written without a bundler.

      **Not decided here.** Filed on the decisions board as
      `cors-at-the-door` with a recommendation (an allowlist from an
      environment variable, absent meaning same-origin only, with the
      preflight answered by the same layer). The alternative is to declare
      the door same-origin forever and put a reverse proxy in front of both
      the app and the deployment, which is a deployment-topology answer
      rather than a code one and costs nothing in `crates/`.

      **Ruled: the allowlist**, on the owner's observation that the page and
      the deployment being on different origins is the ordinary case
      (*"cloudflare pages as the front with ordo as the baas"*). Two
      requirements ride with it: **documented so it is not missed**, because
      the failure mode is a browser silently discarding a good answer, and
      **easily configurable in the console**. The console is unbuilt (18o),
      so the first increment is deployment configuration shaped so the
      console can drive the same setting.

## 15 · Build, CI and images (`plan-ci-images.md`)

- [ ] **15a · The GHA pipeline for images and testing, WIRED and GREEN

      **The drill ran, 2026-09-20, and all twelve gates held.** Run
      35528648741 (`gate-drill.yml`, `workflow_dispatch`), each job
      printing its own verdict line rather than a summary a reader has to
      trust: the executed-case assertion failed with Postgres removed,
      with C001 renamed and with C001 deleted; each inventory leg failed
      on its own isolated break, naming its direction; the CLI usage exit
      failed against an image that always exits 0; `/health/deep` failed
      both when it answered 200 unauthenticated and when it answered 404;
      `/health/ready` failed on a status that is neither of the two it
      accepts; and the playground's seeded check failed both on an
      `UNAVAILABLE` boot log and on a log with no seeded line at all.
      **The first `v*` release run, observed 2026-09-20/21, and it found
      two things no drill could have.** The tag `v0.1.0` ran the release
      workflow twice. Both times every Linux and arm64-macOS leg
      finished, and both times `x86_64-apple-darwin` sat **`queued`**,
      for twelve hours and then nine, because its runner label
      `macos-13` is GitHub's last Intel image and does not schedule for
      this account. `release` has `needs: [binaries]`, so **no release
      page was ever published**: a matrix leg that can never start is
      indistinguishable, to a gate, from one that has not started yet,
      and no drill catches it because the drill proves a job *fails*,
      never that it *runs*.

      **Fixed 2026-09-21 by dropping the target**, on the owner's
      reading: *"we are building on GHA to run on Linux."* The deployable
      is Linux and the macOS binary is the operator CLI for a laptop,
      which has been arm64 since 2023. Cross-compiling it on the arm64
      runner was the alternative and was refused, because that binary
      could not run the fixture smoke test and shipping one artifact
      nobody watched compile the fixture breaks the rule the job states
      in its own comment.

      **The second finding is what the release notes claimed.** They said
      every binary carried a build-provenance attestation, while the
      provenance step is `continue-on-error` precisely because this
      organization cannot attach one. A green release would have
      published a signature claim that was false. The notes now say
      plainly what is missing, provenance and the Intel binary both.

      **What this leaves owed:** a successful release run. Nothing has
      published a release page yet.

- [ ] **16a · The schema-diff engine's own three.** All from `§13a`/`§14a`
      and one from reading the code:
      - **`ordo_schema::diff::same_type` canonicalises the typmod away.**
        `t.split('(').next()` — so `numeric(10,2)` and `numeric(20,12)`
        are one type, and `varchar(50)` and `varchar(200)` are one type.
        `ddl_for` emits unparameterised `numeric`/`text`, so **Ordo can
        never produce the disagreement itself**, which is why this has
        never bitten. A **BYO-DB** database, or one altered by hand, can:
        a column narrowed to `numeric(10,2)` would be classified "no
        change" by the diff while silently refusing DL's decimal scale 12.
        Latent, not live — recorded so the day a DL type gains a precision
        the gate is not assumed to cover it.
      - **`ordo_schema::diff` has no taxonomy code for `check`** in either
        direction (`§14a`, "Not fixed, and owed"; `corpus-rulings.md`
        2026-09-07). `ordo plan --against` cannot classify adding,
        tightening or dropping a row invariant. **13b needs this before a
        check can be added to a live deployment** — it is a prerequisite
        of an open item and was recorded inside a closed one.
        **Closed 2026-09-20 by 13b:** `C9` (add) and `C10` (drop), probed
        by `migrate::check_violations`; a changed condition reaches the
        diff as a `C10` and a `C9`, because the constraint name carries
        the condensed condition. The other two bullets stand.
      - **`ordo-cli/src/migrate.rs::tenancy()` runs a `count(*)` per
        declared entity on every report.** `ordo plan --against` is sold
        as the cheap read-only command an operator runs before a deploy;
        on a large deployment it is a full scan per entity. Either bound
        it (sample, or `reltuples`) or say in the command's own help that
        it is not cheap.

- [ ] **16b · `crates/ordo-conformance/fixtures/seed.sql` seeds every
      sealed column with `'\x00'::bytea`.** A one-byte placeholder, not an
      envelope, left from before anything in the tree could encrypt — and
      the comment above those rows still says *"nothing in the tree can
      encrypt (F0)"*, which stopped being true when F0 landed. An opened
      export refuses against it, correctly (`corpus-rulings.md`,
      2026-09-06 exit-ops ruling, "recorded rather than silently
      reseeded"). The reason it was not fixed in place is real — real
      envelopes would move every golden that reads those columns — so it
      is its own change, and this is the item for it. Until then every
      test that reads a sealed column in the market fixture is reading a
      placeholder.

- [x] **16c · The flag `8z` owes, approved and unbuilt — now built.** DL
      §3 rule 6 says an owner-administration command is *"logged with the
      owner as actor and **flagged as owner administration**"*. The actor
      was right; `CommandGroup` (`ordo-commandlog/src/command.rs`) carried
      `correlation`, `actor`, `commands` and no flag. The owner approved
      adding the field on **2026-09-06** (`decisions.json`, `owner-admin`,
      `decided`). `8z` recorded it as its own tail, and `8z` was `[~]`,
      which no open-item sweep sees. Without it an auditor cannot
      distinguish "the owner used their rule-6 capability" from "the owner
      happened to hold a level", which is the one distinction the rule
      exists to make.
      **Built, 2026-09-07.** `CommandGroup.owner_administration`, encoded
      as an optional one-byte trailer so no existing group's rendering —
      and therefore no existing hash, checkpoint or golden — moves; the
      encoding argument and the three-way proof are written up in `8z`,
      which is now `[x]`, and the encoding decision itself is in
      `corpus-rulings.md` (2026-09-07). The `owner-admin` decisions row
      moves from *Partial* to *Done*.

- [ ] **16d · Refinement is shipped and unreachable — `11-8`'s three, and
      `5c`'s one.** Filed as a pointer, not a restatement; the reasoning
      is in those items. (1) **No op-level `refinable` marker exists in
      the language**, so `build_specs` registers every op
      `RefineTier::None` and no refinement reaches the door in any real
      deployment. `14a` now stamps `outputs[].refinable` into the
      interface descriptor, so a generated client can read
      *"this field accepts a condition"* on an op whose tier makes every
      condition a refusal — the descriptor is honest (it carries the
      `none` tier too) and the pair is still a trap for a reader who
      checks one and not the other. (2) The effective cost class is
      computed, enforced and **not traced** — there is no tracing
      substrate to put `ordo.cost_class` on. (3) The executor ignores
      `$refine` entirely; the envelope is validated, priced, and handed to
      an executor that does not apply it. (4) That last one is `5c`'s
      **host-side filtering**, closed as deliberately unbuilt because it
      would examine rows the caller never asked for and needs its own
      disclosure rule — which nobody has written. Both `11-8` and `5c` are
      `[x]`, so all four are invisible.

- [x] **16e · `ordo dev` does not exist, and its bar is settled — CLOSED 2026-09-19 by 17o (D1–D4 built; the budget test passes at about 500 ms).**
      `adoption-dx-plan.md` §3.1 is marked **`[settled as bar]`**:
      save-to-serving **under one second** for a domain of realistic size,
      including additive migration apply — and it says so in the strongest
      available terms, *"anything over a second on save is a bug, not a
      tuning opportunity."* §8 item 5 marks it ★ adoption-critical.
      The binary dispatches `check`, `plan`, `interface` and the seven
      operator commands (`crates/ordo-cli/src/main.rs`). There is no `dev`
      and no watch loop anywhere in the tree, so the bar measures nothing.
      This is not a request to build it now — it is a settled obligation
      with a number attached and no work item, which is the shape this
      audit exists to find. Note the dependency, since it decides the
      order: the sub-second budget is *dominated by DDL apply and swap*,
      so the honest version of this waits on **13b** (additive apply) —
      hot swap and the artifact registry, which are the other half, are
      already built.

- [ ] **16f · §14's group names log-positioned reads and gives them no
      item.** §14's preamble lists the surfaces the *Clients & surfaces*
      group covers — "the capability catalog (already shipped), the
      interface descriptor and the generated SDK, surface descriptors,
      log-positioned reads" — and then opens 14a–14d, which is four of the
      five. The fifth is **11-17**, filed under the audit's own numbering
      because that is where its finding lives. This line exists so §14 can
      be read straight through without the omission looking deliberate:
      **14b cannot be built without it**, since `ui-gen.md` §2's
      page-level snapshot coordination is specified on top of the
      per-space log position.

- [ ] **16g · `std-library.md`'s two `[open]`s, both marked *before first
      ship*.** The document names them and then says, of the pair,
      *"Both need pinning before first ship, since the cap is the reason
      `std.E.whole` has a row bound at all (§5.7)."* That is a deadline on
      an open question, and no item carries it.
      - **`whole`'s per-collection cap.** The value is deliberately not
        guessed in the document — it is a platform constant somebody has
        to choose — and so is *how a renderer shows a `truncated`
        collection*: a marker every skin must handle, not an empty list.
        9k records `whole` as one of the five verbs it does not derive,
        partly for this reason, so the cap currently blocks a derivation
        rather than a shipped surface. With 14a landed the second half is
        no longer hypothetical: the interface descriptor is what a
        renderer reads, and a `truncated` marker it does not carry is a
        collection that silently looks complete.
      - **Rollup promotion criteria.** What usage evidence promotes §4
        from `[lean]` to settled, and whether the flat column-pair naming
        survives contact with generated UI. Lower stakes — `rollup` is
        `[lean]` and therefore not owed — but it is the second half of the
        same sentence and is filed with it rather than lost beside it.

---

## 17 · The BaaS gap (2026-09-08) — the distance from "correct" to "running"

The owner asked what stands between today's repo and a shippable
Backend-as-a-Service, then approved planning all of it. This section is the
index: every item points at the plan that owns its detail, so the entries
stay one paragraph and the arguments live where they can be long. Ordered
inside-out — each ring assumes the ones before it.

### Ring 0 — cannot deploy at all (`plan-runtime.md`, `plan-ci-images.md`)

- [x] **17a · The `ordo-host` server binary — DONE 2026-09-19.** `plan-runtime.md` R1 /
      `plan-ci-images.md` 0a. The library returns a Router; nothing binds a
      port. Assembles from `Verified` only, env config with named
      refusal-on-absence, graceful shutdown. Done-when: the CI smoke boots
      it against a service Postgres and the three-way `/health/deep` check
      passes.
      **Done-when met 2026-09-19:** `images.yml` run 4 booted the image
      against its service Postgres, and the three-way `/health/deep` check
      passed (403 unauthenticated, meaning mounted and gated).
      **The binary landed (2026-09-07):** `crates/ordo-host/src/main.rs` is
      `from_env` → `assemble` → `serve` and nothing else;
      `ordo_host::runtime` is the whole of it, split so `ordo dev` (D3) can
      reach the same assembly and the same gate.
      `tests/runtime.rs` spawns the real process and proves: every missing
      secret is a non-zero exit **before any dial** (asserted against an
      unroutable database, so a binary that dialled first would time out
      instead of answering in milliseconds); `ORDO_KEY_PROVIDER=test`
      refuses without the dev flag and is selectable with it; an
      unprovisioned space refuses with `ordo keys init` in the message; a
      write through the running door seals under the operator's
      `LocalKeyProvider` and opens only there; `SIGTERM` finishes in flight
      work, logs what the security drain flushed, and exits 0; a database
      with no active version boots, answers `/health/live` 200 and
      `/health/ready` 503 naming `registry` and `shape`. `IdentityAccess`
      over `deployment_engine_pooled` is what is mounted — no `psql` fork on
      any request path. `ORDO_PG_PASSWORD`/`_FILE`, `TokenKey::from_env` and
      `PageKey::from_env` are new (three loaders a deployment needed and
      none existed for). **Platform ops are served (2026-09-18):** the
      binary boots with `boot_with_platform_ops` and mounts
      `PlatformExecutor` over `MeteredExecutor`. `std.space.callersOf` is
      always served, reading the live engine through `SwappableAccess`, a
      `PgGrantRegistry` and the door's delegation registry; the exit ops are
      served when the new `ORDO_EXIT_BUNDLE_DIR` is set; `attestTo` and
      `chainProof` when `ORDO_CHECKPOINT_KEY` is. `std.space.attachBucket`
      stays unregistered, because no resolver reads attachments yet, and the
      census says so. A swap keeps the platform ops and refuses a version
      that compiles a reserved name. Proven by `tests/runtime.rs`
      `the_binary_serves_the_platform_ops` (owner answered, editor refused,
      export writes a bundle, on the spawned binary) and
      `without_a_bundle_dir_the_exit_ops_are_not_registered`, and by
      `tests/deploy.rs`
      `platform_ops_survive_a_swap_and_a_colliding_version_is_refused`. The
      ruling is in `corpus-rulings.md`. The worker supervisor is R3.
      **17g's open config decision is closed here** — see `ORDO_BLOB_STORE`
      in `deploying.md` §4b and the ruling in `corpus-rulings.md`: a store is
      required exactly when the loaded program declares a non-`inline` blob
      field, `fs:` needs `ORDO_DEV_UNSAFE_BLOB_STORE`, and the runtime holds
      one `SingleStore` resolver for the door, the sweep and the exit ops.
      **Merged to main 2026-09-18** from agent branch
      `worktree-agent-a7efbdbe603839b95`, resolved toward main's
      multi-space model: the binary assembles a shared `Spaces` resolver,
      not one deployment and one `WriteLog`. 846 tests pass across the seven
      affected crates, `tests/runtime.rs` (7) spawning the real process.
- [ ] **17b · Production `KeyProvider`.** `plan-runtime.md` R2a–R2c. The
      only provider is `TestProvider`, whose master key is a constant in
      this repository — and the plan's survey found it is worse than a
      default: **a loaded program seals under the test constant whatever
      provider it was handed** (`Fixture::keys` is hardcoded at
      `compile_source`), so R2a — custody as an input to loading — is the
      binary's prerequisite, not a follow-on. Sealed-file master first
      (bootable with no cloud secret, CI-testable), Vault Transit as the
      first KMS binding, AWS/GCP behind features. Ceremony: `ordo keys
      seal-master | init | rewrap-master`; no escrow — the property that
      makes erasure real is the property that makes loss unrecoverable.
      **R2a landed (2026-09-07):** `verify_candidate` assigns
      `Fixture::keys` and the fixture's space from its arguments and
      asserts both by `Arc::ptr_eq`, and
      `deployment.rs::a_loaded_program_seals_under_the_operators_custody`
      proves a value written through the door opens under a
      `LocalKeyProvider` the test minted and is `KeyError::Seal` under
      `TestProvider`'s constant. Both halves were observed failing with the
      injection removed. **R2c landed (2026-09-07):**
      `ordo_exec::keystore::PgKeyStore` is the `WrappedKeyStore` over
      Postgres — `$keycustody`, wrapped material only, `drop_space` one
      statement — with `tests/keystore.rs` proving seal-in-one-process /
      open-in-the-next, a wrong master unwrapping nothing, and erasure that
      does not come back on restart. **R2b landed (2026-09-07):**
      `ordo_crypto::master` is the sealed-master file (`ordo-master-v1`,
      argon2id at the pinned OWASP profile, key id as AAD, a wrong
      passphrase an indistinct `KeyError::Seal`);
      `ORDO_KEY_PROVIDER=sealed-master` boots the binary from it with the
      master nowhere in the environment; and `ordo keys seal-master | init
      | rewrap-master` is the ceremony, `rewrap-master` transactional and
      asserted not to move a single blind-index digest. **A defect found
      while proving it, reaching further than R2b:** `release` and
      `current_epoch` are presence checks that unwrap nothing, so
      `/health/ready` reported **ready** under the wrong master key and
      failed every sealed read afterwards — the probe is now `blind_index`,
      which unwraps, and the distinction is pinned by a test. Still owed:
      SMK rotation and the re-seal pass (11-11), any KMS binding, and the
      operator tool's custody selection that `ordo revert` waits on.
      **Merged to main 2026-09-18** with 17a. Until then main's served
      write path sealed under `TestProvider`'s published constant: the
      merge is what closes that on main.
- [x] **17c · CI wired, images real — DONE 2026-09-19.** `plan-runtime.md` R4 /
      `plan-ci-images.md` 1–3. **Wired 2026-09-19:** `Cargo.lock` is
      committed (reproducible digests); `images.yml` triggers on push to
      `main` (`amd64`) and `v*` tags (`amd64`+`arm64`) with the boot-smoke
      publish gate from plan §3.3; `release.yml` adopted on `v*` tags.
      **Not yet checked off:** none of it has run on a GitHub Actions
      runner. See 15a for the specific CI observations still owed: the
      executed-case assertion's two failure modes, the inventory step
      names, and every smoke gate observed failing on a deliberately broken
      image at least once.
      **Observed on runners, 2026-09-19:** the rebuilt `ci.yml` is green with
      the golden switch, the exact-name execution check and the three
      inventory steps. `images.yml` took four runs to go green, and every
      failure was real: Rust 1.83 in the Dockerfiles could not build
      edition-2024 crates (now 1.97, the toolchain the suite runs on); the
      host smoke never passed the settings the binary refuses to guess; and
      the binary died on a database with no registry tables, which no test
      had covered (fixed in `assemble`, pinned in `tests/runtime.rs`). Run
      4 built, smoked and pushed all three `amd64` images to GHCR: the host
      boots on an empty database, `/health/deep` answers 403 unauthenticated
      and `/health/ready` 503. The deliberate-failure observations for the
      remaining gates and the first `v*` release are tracked under 15a.

### Ring 1 — a first tenant hits these in week one

- [x] **17d · The worker supervisor.** `plan-runtime.md` R3. Every loop
      step (triggers, clock, secevent drain, prune/checkpoint cadence) is
      built and tested; nothing calls them from a long-running task. One
      supervised in-process worker set, crash-restart per worker, a
      panicking worker never takes the door down. Carries the two riders
      `plan-production-ops.md` assumes: the re-seal worker (K1) and the
      usage rollup (M1). Closes the operational half of §12e.
      **Started, unreviewed (merged 2026-09-18):** the agent that built R1
      and R2 died mid-R3. Its `supervisor.rs`, `workers.rs` and
      `start_supervisor` in `runtime.rs` are on main with
      `tests/supervisor.rs` (3 passing). Nobody has yet checked them against
      `plan-runtime.md` §3's worker table and done-when; that review is the
      next step on this item.
      **Reviewed and finished 2026-09-18.** A read-only review against §3
      found five gaps; each was confirmed in the code and closed, and every
      R3 done-when clause now has a passing test in
      `crates/ordo-host/tests/supervisor.rs` (7 tests):
      - *Write to handler, `finish_run`ed:*
        `a_write_through_the_door_reaches_a_run_handler`. The write goes
        through the assembled runtime's door in the test process, since a
        `RunHandler` is a Rust value the shipped binary has no way to be
        given until 12a.
      - *Panic restart with the platform backoff, counted as
        `reason="panic"`:* `a_panicking_worker_is_restarted_and_does_not_poison_the_door`.
      - *Quarantine after ten failures with `enabled: false` and a reason on
        `/health/deep`, while `/health/live` answers 200 throughout and an op
        succeeds:* `a_quarantined_worker_is_disabled_on_health_deep_while_the_door_stays_live`,
        over a real socket bound to `runtime::door`, the router `serve`
        binds. The backoff unit is 10ms there; the sequence and threshold
        are production's.
      - *Leadership fails over (§3.5):* the overseer re-contends every 5s. A
        follower that takes the lock starts its leader-only workers; a
        leader probes its own lock session and stops them when it is gone.
        Exposed as `supervisor.leadership` on `/health/deep`,
        `ordo_supervisor_leader` and
        `ordo_supervisor_leadership_changes_total{to}`.
        `leadership_fails_over_between_two_supervisors`: two supervisors on
        one database, one leader, takeover after the leader shuts down, and
        a lock session killed with `pg_terminate_backend` is noticed and
        retaken.
      - *`secevent.retain`:* leader-only worker over the drain's retention
        pass, enabled by `ORDO_SECURITY_CHECKPOINT_KEY` (now carried on
        `Runtime`). `secevent_retain_claims_its_own_schedule_once_per_period`.
      - *`secevent.drain`:* kept standalone and listed in the census with
        `supervised: false`; adopting it would change its start, flush and
        failure guarantees. Reasoning in `corpus-rulings.md`, "The security
        drain stays outside the supervisor" (2026-09-18).
      - *Orphaned listener cursors and dead letters (§3.7, §3.6)* are listed
        under `supervisor` on `/health/deep` and never acted on:
        `an_orphaned_listener_cursor_is_reported_and_left_alone`.
      **Re-audited against §3 on 2026-09-26, and it found two wiring defects
      the review of 2026-09-18 did not.** Both were a supervisor doing
      nothing, silently, with no failing test and no log line, which is the
      defect class R3 exists to close:
      - ***`serve` never shut the supervisor down.*** §1.5 step 3 was
        `if let Some(s) = Arc::into_inner(supervisor)`, and `PlatformState`
        holds a clone of the same `Arc` so `/health/deep` can answer the
        census, so `into_inner` returned `None` every time. The workers were
        never cancelled, no `on_shutdown` ran, and leadership was released
        only when the socket closed. M1's partial usage period went missing on
        every restart. `Supervisor::shutdown` now takes `&self`, and
        `tests/runtime.rs` asserts the final-act count and its position
        relative to the drain flush. Falsified by restoring `into_inner` and
        by reordering the two steps.
      - ***`provisioning.reconciler` ran nowhere.*** Registered inside the
        `listeners.is_empty()` else-branch, so it needed a trigger listener no
        deployment declares until 12a, and the census named it neither running
        nor DISABLED. A half-built provision therefore stayed `provisioning`
        instead of turning `failed` and joining `tenants::troubled`. Hoisted
        out of that branch; the sigterm test now asserts the running census by
        name. `secevent.drain` was missing from the startup log's census for a
        related reason (an `External` was not printed) and is now in it.
      **`ORDO_WORKER_<NAME>_TICK_MS` is now read** (2026-09-26), resolved in
      one place (`supervisor::resolve_tick`), moving the tick and never a
      `Cadence::Schedule` period, and refusing an unreadable or zero value at
      boot rather than defaulting. Two tests, both falsified.
      **Still open:** `triggers.runner` stays disabled in the binary until 12a
      supplies a handler. **`ordo triggers ls` and `ordo triggers deregister`
      built 2026-09-30** (`crates/ordo-cli/src/triggers.rs`, tests
      `crates/ordo-cli/tests/triggers.rs`): the delete and a custody record
      of who retired the listener commit together, and a listener the
      census does not show orphaned is refused without `--force`. Three
      `Metrics` accessors still take their lock with `.expect`
      (`Metrics::bins`, `evaluate_alerts` and `render_alerts`, the last two on
      `/security/alerts`), which is `plan-runtime.md` §7's poisoning hazard:
      recorded there and left to the crate that owns the registry.
- [ ] **17e · Migration apply.** Already homed: **§13b–§13e**
      (`plan-schema-evolution.md`). Listed here because a BaaS whose
      schema changes end in "now run the DDL yourself" is not one.
      **13b built 2026-09-20** (the additive apply, the first create, and
      the `check` codes 16a asked for), as a service-owned library
      (`ordo_host::schema_apply`) with the CLI as a thin caller. What
      remains here is 13c to 13e, and 18l's door.
- [x] **17f · Computed write RHS (P5).** Homed and **built: §6g.** `used:
      used + 1` — DL's own flagship example — compiles, computes in the
      row's own `UPDATE`, and is proved atomic by six concurrent
      transactions landing as `+6`. The whole expression language reaches a
      write's right-hand side through the read path's binder and the read
      path's renderer, so nothing about what an expression MEANS is decided
      twice. Quota and counter patterns are live. Two things §6g records
      that a reader of this line should not miss: creation-capable
      statements refuse a field reference by name (there is no current row),
      and the concurrency test turned up a **deadlock in the executor's
      shared `WriteLog` mutex** that is reachable at the served door and is
      still owed.
      **Merged to main 2026-09-18** from agent branch
      `worktree-agent-a58113d1cbc808a3e` after merging 139 main commits
      into it: one import the merge dropped was restored, and 520 tests
      across `ordo-sema`, `ordo-plan`, `ordo-conformance` and `ordo-exec`
      pass. The deadlock it found is its own item, 17f-dl.
- [x] **17f-dl · The shared write-log deadlock — FIXED 2026-09-18.** Found by 17f's
      concurrency test and independent of it (§6g's HAZARD note has the
      mechanism). `Executor::attempt` holds the per-space
      `std::sync::Mutex<WriteLog>` across the write protocol's database I/O,
      and takes the same mutex on the async side, where the continuation that
      commits and releases row locks runs. Two writers to one row take the
      mutex and the row lock in opposite orders. Re-run on 2026-09-18 against
      current main: the ignored reproduction
      (`a_shared_write_log_deadlocks_concurrent_writes_to_one_row`) still
      hangs, and its own 20-second timeout never fires, because the blocked
      `lock()` on the async side stalls the runtime's worker threads.
      `ordo-host` hands one log per space to every request (`spaces.rs`
      `log_of`), so the served door can freeze under ordinary contention.
      The single `pending` slot is also shared: one writer's `drop_pending`
      can discard another's group. Done-when: the reproduction passes as a
      normal test (six concurrent writes to one row all return), and the
      per-write state (pending group, staged payloads) belongs to the
      attempt, never the shared log.
      **Fixed:** `WriteLog::attempt` cuts a log for one write that mints
      from the shared sequence (now an `Arc<AtomicU64>`) and holds its own
      pending group and payloads; `Executor::attempt` runs the protocol
      against it and locks the shared log only to cut it and, after commit,
      to append through `commit_attempt`. The reproduction became
      `a_shared_write_log_serves_concurrent_writes_to_one_row`: six writers
      sharing one log to one row all return in 0.2s, land as +6, and all six
      groups reach the chain. 813 tests across `ordo-conformance`,
      `ordo-exec`, `ordo-host`, `ordo-playground` and `ordo-cli` pass.
- [x] **17g · Blob & content custody.** `plan-blob.md` B1–B5, **done**.
      References in the database, bytes behind one store abstraction
      (`ordo-blob`: an S3-compatible binding, and a filesystem binding that
      is a test double and refuses to claim production readiness). Handle
      addressing ruled and built: keyed-random via
      `KeyProvider::mint_blob_ref` under a `$blob` domain disjoint from
      `$sealed`, never content-hash — asserted negatively against every
      digest a content-addressed scheme would have used.

      **B1** the store trait, two bindings, the resolver, an eleven-case
      contract suite both must pass. **B2** `blob`/`uri` through parser →
      schema → DDL, with all fourteen of §3's ratchets decided and three
      more found by firing (the F6 shape gate's `accepts`, the refusal-class
      inventory, the derived `std.E.create`/`.update`); `ScalarTy::ALL`
      closed a fourth that pre-dated the work — the editor grammar's type
      list was the one inventory with no parity test. **B3** chunked STREAM
      in `ordo-crypto` (lifting `f7-vault-plan.md:767`'s stated exclusion),
      the two blob routes, and an explicit `DefaultBodyLimit` on the op
      route where axum's implicit 2 MB had been governing untested. **B4**
      the two-phase sweep, floored at `prune::Floor` and reporting its
      `FloorHolder`, so a lagging consumer holds one floor and the operator
      sees one name. **B5** `std.space.attachBucket` — owner-only,
      step-up gated, keyed on the target's identity so the dropped
      `ReplayLease` is safe for a stated reason rather than by inheritance —
      and the export's `unbuilt` list, which carried exactly one entry for
      the life of the module, now empty.

      **Four defects found by driving it through real paths**, each by the
      first test to cross a layer the unit tests below it could not see:
      `attachBucket` raising `invalid_param` from the executor (a class the
      executor may not say, so three caller mistakes were 500s); a deleted
      space leaving every file in its bucket, and the object-deletion
      ordering wrong twice before it landed inside `erase_space`; and
      `std.$blob.get` granted to nobody, which made **every** blob read
      `access_denied` for every principal with the row guard unreachable
      behind it. The pattern is the finding: every first real traversal
      produced a bug.

      **What is honestly not done.** `blob private` is **declarable** —
      it parses, binds, gets a column, and carries its tier onto the shape
      summary — and every runtime path that would have to touch the
      plaintext refuses it **by name**: the door's write (it seals on the
      way in, and skipping the sealer would put whatever the client sent
      into the bucket unchecked), the door's read (the platform holds no
      key), and an open export, which hands the object over **as stored**
      rather than as a file of nulls. F7 Phase 6 owes the passkey-PRF
      derivation, the device wrapping and the presigned upload; the seam is
      real, because `ordo-blob` never inspects a byte. `blob inline` writes
      are refused **by name** at
      the door, because those bytes ride the row's sealed payload and are
      therefore the write protocol's to write inside a command group — the
      seam is built (`ordo_crypto::inline`, the policy cap,
      `sealed::collect_pruned_in`), the write path is owed. Share links
      wait on `private`. Multipart upload is deferred with a stated
      trigger. CI runs the store contract against the filesystem binding
      only: there is no object-storage service in `docker/`, so the S3 run
      is gated on `ORDO_BLOB_S3_*` and prints SKIPPED rather than passing
      vacuously.

      **And the tier is not mounted.** Nothing in the tree constructs a
      `StoreResolver`: `ExitOps.stores` is `None` at every construction
      site, `S3Config::from_env` has no caller, and `FsStore` is not a
      deployment target by its own declaration. So `router_with_blobs`,
      `PlatformBlobs` and `blobgc::sweep` are built and tested and reached
      by no running deployment — the blob door is proven, not served, and
      orphaned objects have no path to reclamation. **This is a config
      decision, not an oversight to code around**: somebody must rule where
      a deployment's default store comes from. `ordo blob-sweep` was
      deliberately not built for exactly this reason, rather than have the
      CLI invent the answer.
- [x] **17g′ · Blob uploads through the write protocol, and the digest in
      the chain. DONE 2026-09-21, C1 to C8.** `plan-blob-chain.md`, C1–C8. `plan-blob.md` §8.1 and
      §8.2 were ruled on **2026-09-08**: an upload is a row change and
      takes the same path, so the blob door hands the write to the write
      protocol and the upload becomes a converge (revertible, `converged`
      fires, `rev` moves), and the image's blob column carries the handle
      and a sealed digest, landed in the same commit because that is the
      only moment the encoding is free. Consolidates the two full-row image
      builders into one, which is what makes "only the write protocol
      writes entity rows" a test rather than a habit. Carries three defects
      found while planning: undo's row SQL is not space-scoped, a rolled-back
      upload leaks an object no sweep can enumerate, and `blob sealed` is
      admitted by the door and modelled by nothing.
      **C1 landed 2026-09-08**: the first of those defects is closed. Undo
      addressed rows by `"id"` alone in seven statements, not the four the
      plan first named, and entity tables are keyed on `("$space", "id")`
      while ids are minted from a process counter with no space salt, so a
      two-space deployment could revert against another space's row. All
      seven now carry the space predicate, pinned by five tests in
      `crates/ordo-exec/tests/undo_space.rs` that were each seen to fail
      against the unscoped code.
      **C2 landed 2026-09-09**: the two full-row image builders are one,
      `ordo_schema::image`, with the I/O left at each caller behind a
      `SealSink` trait. They agreed on the bytes of an image by
      coincidence rather than by construction, and C4 and C5 would each
      have had to change both. Licensed by a temporary test comparing both
      old builders against the new one byte for byte over every seeded row
      of two fixtures, then deleted with them. The space predicate now
      lives inside the select, so C1's fix is structural.
      **All four §7 questions are answered** (2026-09-09), so C3 through
      C8 are unblocked.
      **C3 landed 2026-09-09**: the third defect is closed. `blob sealed`
      is refused at the door by name on both the write and the read path,
      and sealed blob fields leave the collector's live-reference query,
      which used to compare `bytea` against `text` and error the sweep
      before it read a row. The declaration stays legal, so a later build
      serves the tier without a syntax change; serving it needs a keyed
      collector or a deterministic blind index derived from the handle,
      because the collector holds no keys on purpose and a sealed handle
      carries a fresh nonce.
      **C4 landed 2026-09-09, and it is the ruling itself.** The door
      seals the bytes, mints a `PinnedUpload` that carries its own target
      row and its own `$blobref` INSERT, and hands it to the write
      protocol, which runs it as an ordinary converge. So an upload
      appends a group, moves `rev`, fires `converged` and `written`, and
      is revertible. `ordo_exec::blob::attach` is gone, and with it the
      only entity-row write in the tree that appended nothing.
      `only_the_write_protocol_updates_entity_rows` is the invariant the
      owner chose this candidate for, and it was verified to bite.
      Two consequences worth knowing before R1. **The wire changes**: a
      caller who cannot see a row now gets `not_found` on upload where
      they used to get `access_guard`, because the protocol checks the
      read guard first and that order is the disclosure rule. The old
      answer told them the row exists. **An upload is not retried**: the
      token is consumed by the converge, so a serialization conflict
      surfaces to the client, who re-uploads, and the first object is
      leaked by the blind spot below.
      **C5 landed 2026-09-09**: a blob column's image now carries
      `"{field}$digest"` beside the handle, a `SealedRef` into
      `$sealedpayload` holding the verbatim `$blobref.integrity`
      envelope. Never plaintext, because a hex digest in the plaintext
      chain would be an unsalted permanent equality index over file
      content that survives crypto-erasure. So every chain hash for a
      group with a blob column moves from here on; groups already
      appended keep the bytes they were appended with.
      **C6 landed 2026-09-10, and the blob chain is built.** A revert now
      compares the digest the chain recorded against the object that
      exists, which is the owner's own definition of verification. A
      mismatch refuses before any write; an absent or unreachable object
      restores and raises `blob_object_absent`; the undo horizon keeps
      its own refusal; an image with no digest restores and records the
      skip. `BlobPipeline::verify` streams instead of buffering, which it
      had to before any of this was affordable. **One loose end**: the
      skip record has no durable home, because there is no ops log. It
      rides `Reverted.unverified` with a named seam. See
      `plan-opslog.md` §3.3, which asks whether it should move to a
      command-group trailer instead.

      **C7 landed 2026-09-21, and the blob chain is finished.**
      `undo::verify_objects` walks the whole stored chain and compares
      every recorded digest against the object the store holds, reusing
      C6's `verify_restored_objects` rather than adding a second
      comparison engine. It stops at the first error, and because the walk
      is in chain order that error is the first mismatching handle: the
      required test tampers **both** objects and proves the report names
      the first and not the second, so a walk that merely found *a*
      mismatch would fail it. `Custody::release` grew a `Purpose`
      parameter it previously hardcoded. Exit 3 on a mismatch.

      **Three gaps it found in the plan**, each the same shape: §4.3 and
      §4.5 described revert-time behaviour and did not notice an offline
      walk cannot inherit it.
      1. **"A digest is recorded and will not open" is not in §4.3's four
         outcomes**, and at revert time it cannot arise, because
         `resolve_sealed` refuses the whole image first. An audit walk
         that hard-stopped over one unrelated sealed column, or over a
         digest whose epoch was crypto-erased, would stop auditing at the
         first erased tenant. Bucketed under `ObjectAbsent` (nothing was
         compared) rather than minting a fifth outcome, with presence
         tested against the raw image so "never recorded" and "recorded
         and unreadable" stay distinguishable.
      2. **§4.5 never says whether the walk checks `before` images,
         `after` images or both.** A revert checks only what it is about
         to write; an audit has no such restriction, so this checks both,
         and the cost is now written down: a handle that is one command's
         `after` and the next command's `before` is read twice.
      3. **§4.5 does not say the mode needs a program argument and real
         key custody**, unlike the other two verify modes. Both follow
         from §4.1 refusing a plaintext digest and from only the schema
         knowing which columns are blob columns, but the signal was a code
         comment rather than the plan.

      **C8 is this entry plus `plan-blob-chain.md` §6.** 17g′ closes.
- [x] **17h · `$caller` rebinding for delegated calls — BUILT with 11-4a
      (2026-10-08).** `plan-delegation-surface.md` increment 2. Guards now
      see the delegator.

### Ring 2 — production operations (`plan-production-ops.md`)

- [x] **R3 wiring note from C4 (2026-09-09). Closed 2026-09-21, and the
      note was stale in the useful direction.** It said the blob door
      holds a `log: Arc<Mutex<WriteLog>>` that must be the **same** `Arc`
      the executor holds, and that only tests construct the door.

      **Both halves had moved.** `PlatformBlobs` carries no log at all: it
      holds `Arc<crate::spaces::Spaces>`, the resolver the op path already
      holds, and a `BlobPipeline`, and nothing else. So the hazard was
      closed by **removing the field** rather than by wiring it carefully,
      which is the stronger fix, because a second chain for one space is
      no longer something the runtime has to remember not to create. It is
      not expressible. The same sentence covers the pool: the door reaches
      a space's rows through the resolver, so it cannot answer "where are
      this space's rows" differently from the executor, which for a moved
      space would mean reading references out of the database the space
      left. And `runtime.rs` **does** construct the door, with
      `spaces.clone()`.

      **What was genuinely missing is the assertion**, which is now
      `the_blob_door_is_wired_once_and_holds_no_chain_of_its_own`
      (`tests/upload_seam.rs`). Two halves: the declaration admits no
      `WriteLog`, `Pool` or `CommandLog` field, and exactly one shipped
      file builds the door, which is the runtime that owns the
      deployment's one `Spaces`. Observed failing on a planted field
      before being accepted.

- [x] **17i · Key rotation fires, and re-seals. DONE 2026-09-19, see
      11-11 for the build.** K1–K3. Sharper than the
      audit's finding: `rotate()` has **no production caller at all** —
      revocation never opens a new epoch. Wire the trigger, run the
      chunked resumable re-seal worker, gate epoch destruction on the
      four counted populations (17g added the fourth: `$blobref`'s sealed
      `mime`/`size`/`integrity` envelopes, plus the objects themselves,
      whose STREAM header carries the epoch byte so a worker can find it
      without opening one — see `plan-production-ops.md` §1.3), give the
      operator status + the two
      metrics (remaining, stuck-seconds).
- [x] **17j · Restore & DR.** `plan-production-ops.md` R1a–R3, re-verified
      against main 2026-09-19. PITR for the deployment, the export bundle
      for a space: both, with non-overlapping roles. The member digests the
      2026-09-08 plan asked for are **built** (`export::DigestingSink`).

      **R1a–R1c built (2026-09-19), Sonnet:**
      - **R1a**: `ExportReport::manifest_sha256` (`export::bundle_digest`,
        `crates/ordo-exec/src/export.rs`), the bundle's own digest, reported
        by `std.space.export` (`crates/ordo-host/src/exit_ops.rs`). Tests:
        `the_bundle_digest_is_the_manifests_own_and_is_stable`,
        `a_flipped_byte_in_any_member_is_caught_by_the_manifest`
        (`crates/ordo-exec/tests/export.rs`),
        `an_owners_export_reports_the_bundle_digest`
        (`crates/ordo-host/tests/exit_ops.rs`).
      - **R1b**: `PgKeyStore::holds_bytes` (`crates/ordo-exec/src/keystore.rs`)
        and the boot refusal in `build_custody`
        (`crates/ordo-host/src/runtime.rs`), same check in `ordo keys init`
        (`crates/ordo-cli/src/keys.rs`), naming `$keycustody` and the
        `rewrap-master` remedy. Test:
        `a_master_stored_in_the_database_refuses_the_boot`
        (`crates/ordo-host/tests/runtime.rs`), observed failing first with
        the check stubbed out.
      - **R1c**: `docs/howto-operating.md` gains "Backup and Restore",
        between "Metrics" and "Verifying the Audit Chain": the two-backup
        contract, that PITR must cover every `PoolResolver::targets`, the
        two mandatory pre-cut points and K2's ordering rule, and that the
        master lives outside every backed-up database. **Plan-vs-build
        note:** R1c's done-when calls for the section to name the R2
        restore procedure (`ordo restore --record`, the boot assertion) as
        if built; R2 is Opus-tier and out of scope for this pass, so the
        section instead states plainly that Track R2 is not built yet and
        points at checklist 17j, rather than documenting commands that do
        not exist.

      **R2a–R2b built (2026-09-19), Opus:**
      - **R2a**: `CommandGroup::restore: Option<RestoreMarker>`
        (`RestoreMarker`, `RestoreSource` in
        `crates/ordo-commandlog/src/command.rs`), encoded by presence as
        trailer `0x03` plus its payload (`chain.rs`); a marker group must
        be empty (`MarkerWithCommands`) and attributed to `system:restore`
        (`MarkerActor`); `invert_group` refuses one
        (`RevertError::RestoreMarker`). Every existing chain hashes as
        before: `PRE_THIRD_FLAG_GOLDEN` and a both-older-tags golden were
        committed against the old encoder first and pass unchanged, as do
        `PRE_FLAG_GOLDEN` and `PRE_SECOND_FLAG_GOLDEN`. Tests
        (`crates/ordo-commandlog/src/tests.rs`):
        `an_unflagged_group_hashes_where_it_did_before_the_restore_trailer`,
        `a_group_with_both_older_tags_hashes_where_it_did_before_the_restore_trailer`,
        `a_restore_marker_round_trips`,
        `a_restore_marker_with_commands_is_refused`,
        `a_restore_marker_under_another_actor_is_refused`,
        `an_empty_group_without_a_marker_is_still_refused`,
        `a_truncated_marker_payload_refuses`,
        `the_restore_tag_before_a_lower_tag_refuses`,
        `a_marker_group_renders_all_three_tags_in_order`,
        `a_marker_cannot_be_reverted`. `an_unknown_trailer_tag_still_refuses`
        moved its "future tag" from `0x03` to `0x04` and now also asserts a
        bare `0x03` refuses.
      - **R2b**: `undo::{Discontinuity, Continuity, restore_markers,
        latest_restore_marker, continuity, checkpoint_at_or_below}`;
        `Verified` gains `discontinuities` and `orphaned_checkpoints`;
        `choose_anchor` anchors at or below the lowest
        `restored_from + 1` over every marker; `ordo verify` prints the
        discontinuity, one `restore` line per marker and one `orphaned`
        line per orphan, exit 0. Tests (`crates/ordo-cli/tests/operator.rs`):
        `a_recorded_restore_verifies_with_one_discontinuity_at_its_index`
        (observed failing first: `Tamper(Unreadable)` at head + 1),
        `verify_anchors_at_or_below_the_restored_head_never_above`,
        `orphaned_checkpoints_are_listed_and_kept`,
        `continuity_reports_rewound_then_explained`,
        `two_restores_are_two_discontinuities_in_order`,
        `a_row_whose_actor_claims_restore_without_a_marker_is_refused`;
        `a_truncated_tail_is_caught_by_the_checkpoint_that_covered_it`
        still passes. `docs/howto-operating.md` shows the output.
      - **Ruled 2026-09-19 (`orphan-checkpoints`):** a checkpoint counts
        when the stored chain agrees with its head; R2b′ below changes the
        built code to that rule.

      **R2b′ built (2026-09-20), Opus:**
      - `undo::{Agreement, agreement, stored_checkpoints, explained_by}`
        (`crates/ordo-exec/src/undo.rs`): a record of a head at a position
        is `Current`, `Orphaned` or `BelowFloor` against the stored chain,
        in at most two row reads. `choose_anchor` takes the newest stored
        checkpoint that is `Current` and reads no markers; `seam_bound` and
        `undo::checkpoint_at_or_below` are gone.
      - `Verified::orphaned_checkpoints` is `Vec<(u64, ChainHash)>`, and
        every orphan listed is one a marker explains. An orphan nothing
        attests fails verification from any anchor, including genesis:
        `VerifyFailure::UnexplainedOrphan { upto, head }` at or below the
        head, the truncation finding at `head + 1` above it. The judgement
        reads `$checkpoints` as data, so a forged row names itself.
      - `continuity` classifies every stored checkpoint instead of the
        newest one, and `Rewound` carries `checkpoint_head`.
      - `$checkpoints` is keyed `(space, upto, head)` through an additive
        idempotent `ALTER` in `COMMANDLOG_DDL` (`checkpoint-key`,
        2026-09-20); `sign_checkpoint` answers `AlreadyAt` only when the
        stored row carries this head, so a checkpoint at an orphan's
        position is signed and stored beside it.
      - Every reader that picks a checkpoint by position filters on
        agreement: `PgStore::try_latest_checkpoint` (a proof's anchor),
        `prune::{checkpoint_at_or_below, checkpoint_exactly_at}`.
        `prune::floor_in` keeps a marker while an orphan above the cut
        still needs it (`FloorHolder::RestoreMarker { index, orphan_upto }`),
        and the clamp lifts when the orphan falls below the floor.
      - `ordo verify` prints
        `orphaned  checkpoint upto N head H…, explained by restore at index M, kept`.
        `docs/howto-operating.md` "Verifying the Audit Chain" states the
        rule.
      - Tests (`crates/ordo-cli/tests/operator.rs`), the old rule's three
        failures first: `verify_survives_a_prune_across_the_seam`,
        `a_later_unannounced_rewind_is_rewound_not_explained` (observed
        failing first: `Intact` where the new rule reads `Explained`, then
        `Rewound`), `a_checkpoint_at_an_orphans_position_is_signed_and_both_rows_are_kept`
        (observed failing first: `AlreadyAt(6)` on the first call),
        `a_prune_keeps_a_marker_while_an_orphan_above_the_cut_needs_it`
        (observed failing first with the clamp removed: the floor reads
        `RetainedTail`), `an_unexplained_orphan_fails_verification_naming_the_checkpoint`,
        `an_unattested_marker_explains_nothing`. Amended with their reasons:
        `a_truncated_tail_is_caught_by_the_checkpoint_that_covered_it`,
        `verify_anchors_on_the_newest_checkpoint_the_chain_agrees_with`
        (renamed), `orphaned_checkpoints_are_listed_and_kept`,
        `continuity_reports_rewound_then_explained`,
        `two_restores_are_two_discontinuities_in_order`,
        `a_row_whose_actor_claims_restore_without_a_marker_is_refused`.
        Every chain golden passes unchanged.
      - **Plan-vs-build note:** the plan's stated pre-change failure for
        `verify_survives_a_prune_across_the_seam` (`PrefixMissing
        { first_stored: 10 }`) does not reproduce, because the cut that
        removes the prefix also removes the marker, which leaves the old
        rule with no seam bound and the newest checkpoint as its anchor.
        The test is kept: it pins that a pruned marker does not disturb
        verification and that an orphan below the floor is not listed.

      **Still open, in order (plan-production-ops.md R2c to R3, rewritten
      2026-09-19 for the `restore-witness` and `orphan-checkpoints`
      rulings):**
      **R2c built (2026-09-20):**
      - `undo::{RestoreRecord, Recorded, PreHeadSource, RestoreError,
        record_restore}`; `head_of` is `pub`. One transaction under the
        append lock: the marker group (`Actor::System("restore")`, no
        commands) and a `custody-action` security entry under the group's
        own correlation, so the two records of one act join on one id. The
        witness `restore` row is written after the commit; a failed write
        leaves the marker standing, reports `NOT WITNESSED` with exit 1,
        and is completed by a re-run without `--again`.
      - `pre_restore_head` defaults to the newest **witnessed** checkpoint,
        else the newest stored one whatever its agreement, else `None`;
        the output says which of the three it took. `--unattested` forces
        `None`.
      - `ordo restore --record` in `crates/ordo-cli/src/operator.rs`, added
        to the dispatch list and to `usage()` with the line that this tool
        attributes rather than authenticates. `--at` is seconds.
      - **Plan-vs-build note, two places the plan is not self-consistent:**
        (1) `gap` is `idx - (restored_from.0 + 1)` and the build note says
        `head_of` gives `restored_from`, which makes the gap structurally
        zero, while §2.4 defines `restored_from` as where history broke and
        says the two differ when writes land first. §2.4 is followed:
        `--restored-from <idx>` lets the operator name the point, defaults
        to the head, and an index the chain does not hold is refused
        (`RestoreError::NoSuchEntry`). (2) `AlreadyRecorded` as specified
        (the newest marker has the same `restored_from`) cannot fire on an
        ordinary re-run, because the first marker moved the head. A second
        reading is added: the newest marker **is** the head, so nothing has
        happened since it was written. Either is enough.
      - Tests (`crates/ordo-cli/tests/operator.rs`):
        `restore_record_writes_the_marker_at_head_plus_one_and_the_custody_event`,
        `restore_record_refuses_an_unidentifiable_source`,
        `restore_record_with_nothing_to_default_from_prints_unattested`,
        `restore_record_defaults_the_pre_restore_head_to_the_witnessed_checkpoint`,
        `restore_record_twice_on_one_point_needs_again`,
        `restore_record_completes_a_missing_witness_row_on_rerun`,
        `restore_record_reports_the_gap_when_writes_landed_first`.

      **R2d-ii built (2026-09-20), Opus:**
      - `undo::continuity(db, witness, space)` and
        `undo::RecordSource { Stored, Witness }`; `Explained` and `Rewound`
        name the source, and `Rewound` carries `checkpoint_head`.
      - The rule, in order: every witness row and every stored row without
        a witness copy is classified by `agreement`; a witness row is
        explained by a `restore` row with a greater `seq` whose own
        `(upto, head)` the chain agrees with; a stored row with no copy
        falls to R2b′'s bound rule; the highest unexplained orphan is
        `Rewound`, naming its source. A stored row *with* a copy is one
        fact recorded twice and is judged once, as the copy.
      - **One rule the plan leaves implicit, added with its reason.** A
        witnessed checkpoint may still fall back to the bound rule, but
        only while no `restore` row precedes it in `seq`. Without the
        fallback, a marker could no longer explain a head witnessed before
        the restore was recorded, and R2b′'s own continuity tests would
        fail, which R2d-ii's done-when says they must not. With it,
        `a_checkpoint_witnessed_after_the_restore_is_a_fresh_rewind_even_below_the_attested_head`
        still holds, because that checkpoint's `seq` is above the restore
        row's.
      - A `restore` row is itself a record of a head, so a chain that no
        longer holds the marker contradicts it and that is a rewind.
      - Tests (`crates/ordo-cli/tests/operator.rs`):
        `a_witnessed_checkpoint_the_chain_no_longer_reaches_is_rewound_even_when_the_checkpoints_agree`
        (observed failing first, and asserted in place: over the space
        alone the same state reads `Intact`),
        `a_restore_row_after_the_witnessed_checkpoint_explains_it`,
        `a_checkpoint_witnessed_after_the_restore_is_a_fresh_rewind_even_below_the_attested_head`,
        `a_restore_row_whose_marker_is_gone_is_itself_a_rewind`. Every
        R2b′ continuity test passes with a witness over the same pool.

      **R2d-iii built (2026-09-20), boot half superseded by 17aa
      (2026-09-21):**
      - `BootRefusal::Rewound(Continuity)` in
        `crates/ordo-host/src/runtime.rs`; `assemble` ran
        `undo::continuity` over `ORDO_SPACE` after `ensure_platform_tables`
        and before the drain, and the handle is kept as `Runtime.witness`
        for the worker registration and for `Spaces`.
        **The variant is gone.** 17aa moved the refusal from the process to
        the space: one tenant's rewound chain no longer stops the
        deployment, because the process it stopped is the door through which
        the restore is recorded. What `assemble` runs now is
        `reconcile_at_boot`, which answers rather than refuses. The refusal
        itself survives unchanged, one layer down.
      - `SpaceRefusal::Rewound { space, continuity }` in
        `crates/ordo-host/src/spaces.rs`;
        `Spaces::routed(deployment, pools, witness)`, `Spaces::new` builds
        one over the boot pool, and `resolve` checks a space once per
        process on that space's own pool, after routing and before `Bound`.
        `continuous: Mutex<HashSet<String>>` holds **passing verdicts
        only**, so the next request after `ordo restore --record` is served
        without a restart. `Spaces::continuity_checks()` exposes the count.
      - A continuity read that *errors* is `Unroutable`, not a pass: "I
        could not check" is not "it checks out".
      - Tests: `a_rewound_space_refuses_the_boot_until_its_restore_is_recorded`
        (`crates/ordo-host/tests/runtime.rs`; observed failing first with
        the refusal removed, where the binary boots and keeps serving, so
        the test waits on the child with a bound and fails rather than
        hangs). **Replaced by 17aa** with
        `a_rewound_space_is_quarantined_and_the_door_stays_up`, which
        asserts the opposite of its predecessor's first line and the same
        thing as its second: the binary boots, and the space does not
        serve. Also `a_rewound_tenant_is_refused_at_resolve_and_served_after_the_record`
        and `a_space_with_no_record_is_never_refused`
        (`crates/ordo-host/tests/spaces.rs`),
        `a_whole_database_restore_of_an_attached_space_is_caught_at_resolve`
        (`crates/ordo-host/tests/restore_witness.rs`, new: a template copy
        of an attached target, restored under its own name, with the first
        assertion being that over the target alone the same state reads
        `Intact`).

      **R2d-iv built (2026-09-20), Opus:**
      - `undo::{RecordOrigin, Explanation, RecordVerdict, classify_records}`:
        every stored checkpoint, every witness row and every receiver head
        classified by `agreement`, with the explanation each can support.
        A receiver's record is explained by the bound rule **only**, because
        no `seq` exists outside the deployment.
      - `ordo verify --control-db <name>` and `--attestation <file>`
        (repeatable; `Flags` gains `repeated` so a flag can carry more than
        one value). Each document is checked by
        `ordo_verify_proof::verify_attestation`, the routine a third party
        runs, so this command cannot disagree with the verifier a receiver
        was pointed at; without a key it is structure only and the line says
        `unverified`. A document naming another space is exit 2, decided
        before anything is verified.
      - Output: `witness  not read (--control-db)` when the control database
        was not named, a `witness` line per witnessed record the chain no
        longer agrees with, and a `receiver` line per file
        (`current` / `orphaned, explained by …` / `below the floor`). An
        unexplained orphan from any origin prints `FAILED  the chain no
        longer reaches a head that <origin> recorded` and exits 3. Stored
        rows keep their existing `orphaned` line rather than being printed
        twice.
      - Tests (`crates/ordo-cli/tests/operator.rs`):
        `verify_reads_the_witness_and_fails_on_an_unrecorded_whole_database_restore`,
        `verify_compares_a_receivers_attestation_against_the_chain`,
        `a_receivers_attestation_below_the_floor_is_reported_not_judged`,
        `an_attestation_for_another_space_is_a_usage_error`,
        `verify_without_the_new_flags_prints_what_it_printed_before`.

      **R2e built (2026-09-20), Opus:**
      - `$attestations` is keyed `(space, upto, head)` through the same
        additive idempotent `ALTER` R2b′ uses for `$checkpoints`;
        `put_attestation` conflicts on the three. `attest_due` classifies
        the stored record at the head's position with `agreement`, and an
        orphan there does not cover this head, so the cadence is not
        satisfied by a statement the chain contradicts.
      - `$attestdeliveries` and `$attestdeadletters` gain
        `"head" bytea NOT NULL DEFAULT ''` and a unique index over
        `(space, upto, head)`; `Delivery` carries the head and every claim,
        finish, dead-letter and reschedule keys on all three.
        `attestdrain::deliver_one` finds the attestation by position **and**
        head, so the wrong one is never sent.
      - `enqueue_due` keeps the cursor as its window and at-least-once, and
        adds two filters inside it: a head the chain no longer holds is not
        sent, and a dead letter is not resurrected. What reaches a receiver
        at a *reused* position is the cursor going back:
        `undo::record_restore` sets it to `restored_from + 1`
        (`webhook::reset_cursor`), and `Recorded::cursor_reset` reports it,
        which replaces R2c's `cursor not reset (R2e)` placeholder line.
      - Tests (`crates/ordo-host/tests/attest_webhook.rs`):
        `a_post_restore_head_at_a_reused_position_is_delivered` (the outbox
        holds a second delivery for the position, and the receiver's second
        payload verifies) and
        `an_orphaned_attestation_is_kept_and_not_redelivered`. Every
        existing test in that file passes unchanged.

      **R3 built (2026-09-20):**
      - `crates/ordo-host/tests/restore_drill.rs`: all five steps against a
        real Postgres. The backup is a template copy of the space's
        **attached target**, so the control database's `$chainwitness`
        survives the restore and step 2's refusal comes from it
        (`source: Witness`). Step 1 prints its wall time with the line
        saying it is not a gate. Step 2 records the restore and asserts
        exactly one discontinuity at exactly N, from the chosen anchor and
        from genesis. Step 3 is a shadow-schema drift check. Step 4 compares
        the rows against the pre-backup snapshot and asserts the M
        post-backup groups are gone from `$commandlog` and that no
        `$sealedpayload` row outlived them. Step 5 re-hashes every bundle
        member against the manifest, recomputes the exported chain's links
        against the restored chain below the marker, and classifies the head
        a receiver was sent before the restore as orphaned and explained.
      - The three variants each assert their specific failure:
        `the_drill_fails_when_the_marker_is_missing` (the space is not
        served and there is no discontinuity to find),
        `..._is_doubled` (two discontinuities where step 2 asserts one),
        `..._is_misplaced` (one discontinuity, not at N).
      - **Plan-vs-build note:** step 2 cannot call `assemble`, because the
        host binary does not construct an `AttachedPools` and `assemble`
        boots with the control pool, so it cannot route to an attached
        space. The boot site that covers an attached space is
        `Spaces::resolve`, and that is the one the drill exercises;
        `assemble`'s own refusal was proved on a single database by
        `a_rewound_space_refuses_the_boot_until_its_restore_is_recorded`;
        since 17aa `assemble` has no refusal here to prove, and
        `a_rewound_space_is_quarantined_and_the_door_stays_up` proves the
        quarantine that replaced it.
        Step 5's receiver comparison runs through `undo::classify_records`,
        the library half of `ordo verify --attestation`, whose CLI surface
        is covered by R2d-iv's own tests.
      - `docs/howto-operating.md` gains "The restore procedure", which R1c
        owed: the four steps, what each flag means, which database holds the
        record, and the single-database case the receivers catch instead.
      Rulings: `corpus-rulings.md`, 2026-09-19 and 2026-09-20.
- [~] **17k · Egress rail — the kernel shipped; the rail did not.** Already
      homed: decisions row `egress` + §7b's pins. The sequencing rule fired
      as written: the attestation webhook was the first consumer, so the
      **six pins and the one client shipped with it** (`ordo-egress`,
      2026-09-08, item 11-12) rather than ahead of it. Every future
      outbound fetcher uses that client — `implementation.md`'s "one client,
      one policy, one place to audit" is now a crate boundary with a test
      on it.

      What remains under this row is the **rail**, not the kernel: the
      per-space egress allowlist as a **multi-host** table with a consent
      event per host (attestation has one sink per space, which is what
      makes its allowlist exactly the owner-signed destination and needs no
      table); the consent-page rendering of the deny-list disclosure; and
      the notification-sink and feed consumers themselves. None of those is
      a security gap today — the pins are enforced for the one consumer
      that exists — and all of them are the work of pointing a second
      consumer at the same client.
- [ ] **17l · Space provisioning lifecycle.** Already homed: §11 (the
      coverage audit's row). Genesis reconciler and `reap`; a
      half-created space must not leak keys and storage silently.
      The deployment-wide **first create** (a schema on an empty target)
      was found unowned on 2026-09-20 and homed under **13b**, which built
      it through the same `spacedb::apply_schema` that
      `provision::prepare_target` runs for a space's own database; 17l
      keeps the per-space half it already had. Neither 17l nor 11-27
      grows a second create path.
- [ ] **17m · Security stream verification & read surfaces.** Already
      homed: **7d** (nothing verifies the secevent chain; `ordo verify`
      walks the command chain only) plus the owner-read surface.

### Ring 3 — the product ring

- [x] **17n · Metering & quotas.** `plan-production-ops.md` M1a–M3,
      re-verified against main 2026-09-19. Two sources with two
      durabilities: requests by class, rows returned and worker runs are
      counted in process (`MeteredExecutor`, `TriggersRunner`) and flushed
      per replica by `usage.flush` into `$usage_replica` (M1a, M1b); writes
      and rows written are read from `$commandlog` between two positions,
      and bytes stored are sampled per target database, by the leader-only
      `usage.rollup` into `$usage` (M1c), which retires the registered
      slot. The 2026-09-08 `AppendObserver::appended` and `$triggerruns`
      counts are withdrawn (the first counts before commit, the second
      counts rows `finish_run` deletes); blob bytes are exact from
      `$blobref.bytes`. `quota_exceeded` (429) is minted with its producer,
      `QuotaGate` sits beside `AccessChecker` and is checked after
      `check_access`, so a non-member still gets `access_denied` (M2).
      `ordo usage` and `/usage` emit deterministic JSONL; invoicing is a
      boundary (M3). Off unless `ORDO_USAGE_LEDGER=1`; never phones home.
      All Sonnet-tier. Unlocks SH-20. Rulings: `corpus-rulings.md`,
      2026-09-19.
      **M1a–M1c built and tested, 2026-09-19.** M1a is the in-process
      ledger (`ordo_host::metrics::Metrics::usage_take`). M1b is
      `$usage_replica` and the per-replica `usage.flush` worker. M1c is
      `$usage` and the leader-only `usage.rollup` worker
      (`crates/ordo-host/src/workers.rs`), which reads `$commandlog`
      between the last rollup's position and the head, `$sealedpayload`
      and `$blobref` for exact byte sums, and the live schema's entity
      tables for the one estimated column, walking every database
      `PoolResolver::targets` names; `usage.rollup` is retired from
      `Slot::all()`. Both cadences (`ORDO_USAGE_FLUSH_SECS`, default 60;
      `ORDO_USAGE_ROLLUP_SECS`, default 3600) are configurable and refuse
      a malformed or zero value at boot, per the owner's ruling
      (`corpus-rulings.md`, 2026-09-19); documented with their trade-offs
      in `howto-operating.md` and `deploying.md` §8c. 14 tests:
      `crates/ordo-exec/src/usage.rs`'s own 3 unit tests plus
      `crates/ordo-host/tests/usage.rs`'s 11 (six for M1a/M1b, five for
      M1c's done-when: reconstruction across flush and rollup together, a
      second rollup continuing from `chain_to` without double-counting,
      the estimated-vs-exact byte columns, walking an attached target
      beside the default, and `usage.rollup` off `Slot::all()`).
      Fixed on the way: `Supervisor::start` now seeds a fresh
      `Cadence::Schedule` worker's `$schedules` row (`next_due = 0`, `ON
      CONFLICT DO NOTHING`) before its loop starts. `usage.rollup` is the
      first worker through that generic path, and `claim_schedule`'s
      `UPDATE` only ever matches a row that already exists, so a name
      nobody had inserted would have idled forever.
      **M2 built and tested, 2026-09-20.** `QuotaDecision`/`QuotaGate`/
      `NoQuota` (`crates/ordo-api/src/spec.rs`), `ErrorCode::QuotaExceeded`
      (429, in `ALL`, `as_str`, `status`, not `executor_raisable` — the
      door's own, like `AccessDenied`) and its row in
      `api-engine-design.md` §4. `router::gate` checks it in a new
      `quota_gate` call immediately after `check_access` and never before
      — a caller with no membership in an over-quota space is answered
      `access_denied`, not `quota_exceeded`, per the owner's ruling
      (`corpus-rulings.md`, `write-quota`, 2026-09-19). `PgQuotas`
      (`crates/ordo-host/src/quotas.rs`, new `$quotas` table) reads a
      snapshot `usage.flush` refreshes on every tick, after writing this
      replica's own rows: `$quotas` limits plus `SUM`s over
      `$usage_replica` for each configured quantity's own period, plus the
      live counters `Metrics::usage_peek` (new, a non-draining sibling of
      `usage_take`) exposes. v1 gates `requests` and `rows_returned` only
      — `writes` is 18i's. `ordo quota set --space --quantity --limit
      --grace --period-secs` and `ordo quota show --space` in
      `operator.rs`, dispatched in `main.rs`; `set` refuses any quantity
      outside the v1 pair by name. Every `ApiState` literal in the tree (17
      sites, across `ordo-api`, `ordo-cli` and `ordo-host`) carries the new
      `quota` field. 7 `PgQuotas` unit tests plus 4
      `crates/ordo-host/tests/quotas.rs` integration tests through the real
      door: `over_quota_plus_grace_is_refused_at_the_door_and_a_non_member_
      still_gets_access_denied` (the ordering non-negotiable, proved by
      asserting the executor's live ledger never saw the refused call),
      `within_grace_warns_and_serves`, `no_quota_row_never_refuses`,
      `the_gate_reads_a_snapshot_and_never_the_database` (refresh from a
      real pool, drop it, `check` still answers from memory). The three
      wire-inventory tests (`inventory_is_closed.rs`,
      `ordo-cli/tests/interface.rs`, `ordo-host/tests/interface_surface.rs`)
      green with `SPECIFIED_NOT_BUILT` still empty. Documented beside M1 in
      `howto-operating.md` and `deploying.md` §8c·1.
      **M3 built and tested, 2026-09-20.** `ordo_exec::usage::render`
      turns a `between` read into JSONL, one record per `(space,
      period_start)`, keys sorted for free (`serde_json::Map` is a
      `BTreeMap` under this workspace's default features), totals summed
      across replicas; a period `usage.flush` wrote that `usage.rollup`
      has not reached yet still gets a record, with the `$usage`-only
      columns simply absent rather than the period being dropped. `ordo
      usage --space --from --to` (`operator.rs`, `main.rs` dispatch) and
      `GET /usage?space=&from=&to=` (`serve.rs`, `USAGE_OP =
      "platform.usage"`, gated by `authorize` against the deployment's own
      space like `/metrics`) both render through it. 6 `ordo-exec` unit
      tests, `two_usage_runs_over_a_closed_period_are_byte_identical` and
      `usage_output_reconstructs_the_tables_exactly`
      (`ordo-cli/tests/operator.rs`),
      `the_usage_route_answers_the_operator_and_refuses_a_tenant_owner`
      (`ordo-host/tests/usage.rs`). Documented in `howto-operating.md`'s
      new "Usage" section and `deploying.md` §8c·2, both ending in the
      CLI's own words: "Ordo meters. It does not invoice."
- [x] **17o · `ordo dev` — the sub-second loop.** `plan-dev-loop.md`
      D1–D4. The bar is `[settled as bar]` and nothing implements or
      measures it; the plan makes the measurement a test. The single
      biggest adoption lever in this section.
      D1 done 2026-09-19: `ordo dev <dir>` (`crates/ordo-cli/src/dev.rs`)
      watches the `.ordo` set (`notify`), recompiles through
      `load_sources_diag`, and prints either a spanned diagnostic (source
      line + caret over `Sources::at`) or the phase journal — no database,
      no serve, no swap yet, per §6. Proved by
      `crates/ordo-host/tests/dev_loop.rs`: the journal's phases sum to its
      own total (§3.2), a broken save carries a caret frame, and the real
      `notify` watcher reports a real save. Measured: read+compile over the
      market fixture is ~7ms in a local debug build; ratcheted ceiling
      250ms. `rule:`/`fix:` teaching text (adoption-dx-plan.md §3.3) is not
      rendered — no error type in the compiler carries that split today,
      only `code` + `message`; D1 renders what exists rather than
      fabricating the split.
      D2 done 2026-09-19: `ordo dev` owns its own Postgres
      (`crates/ordo-cli/src/dev_db.rs` — `initdb`/`pg_ctl` on an allocated
      port, data directory under `<project>/.ordo/devdb`, socket under a
      short per-project temp path, `ORDO_PG_*` overrides to an existing
      cluster, `--fresh` to destroy it, Ctrl-C to tear it down via `Drop`
      — installing a signal handler was necessary: the default abrupt kill
      runs no destructors and would have orphaned the postmaster). Every
      save classifies its schema delta (`live_shape` + `diff`, over
      `Pool`, never `psql`); §4.4's interim rule was implemented exactly
      while `plan-schema-evolution.md` increment 2 was unbuilt: ANY
      non-empty delta reset and reseeded. **Superseded 2026-09-20 by
      13b:** a delta the production triage (`ordo_host::schema_apply`)
      takes whole is applied through `schema_apply::execute`, the same
      function `ordo migrate --apply` calls, and the rows survive; a
      delta it refuses any part of, and an empty database, reset and
      reseed (from `seed.sql` when present), naming each change by
      `Change::code()` and each refusal by its class. Proved by
      `crates/ordo-host/tests/dev_loop.rs`'s
      `d2_owned_cluster_lifecycle_and_delta` (real `initdb`/`pg_ctl`, a
      row inserted, an added field classified A2 and applied with the row
      read back through the new column, a dropped field classified D1 and
      reset) and by a
      manual run: `ordo dev` on a fresh project brought up its cluster,
      applied+seeded on the first save, applied the next save's added
      field via reset, and Ctrl-C left no orphaned `postgres` process.
      Known gap: `seed.sql`'s `@BIDX(...)`/`@SPACE@` placeholder rendering
      (`Fixture::rendered_seed`'s mechanism) is not implemented for dev's
      seed file — it is executed as literal SQL; D3 settled dev's custody
      (`CustodyChoice::Test`, see below) but this gap is separate — a
      project seeding a `sealed indexed` field needs its own answer still.
      D3 done 2026-09-19: `ordo dev` reaches the SAME assembly and gate a
      server gets. `ordo_host::runtime::RuntimeConfig`/`assemble` split
      into `assemble`/`assemble_embedded` over a shared `ProgramSource`
      (`Registry` | `Embedded`) — `ordo dev` never touches the registry
      (§4.3), so it calls `assemble_embedded` directly; `deploy::verify_embedded`
      is the embedded-fixture twin of `verify_candidate`, sharing the gate
      logic via a new `gate_fixture`. `ordo dev`'s custody is
      `CustodyChoice::Test`, composed on the `RuntimeConfig` struct
      directly (never through `ORDO_DEV_UNSAFE_TEST_KEYS`, which gates the
      same provider for a server). Serving is `ordo_host::runtime::serve`
      itself, spawned as a background task, so SIGINT/SIGTERM get the
      full production shutdown (drain flush, worker cancellation) for
      free; `ordo dev` only tears down its own owned cluster after.
      A real bug surfaced and was fixed while proving this: `Deployment::swap`
      moves the dispatch pointer only — the access engine
      (`SwappableAccess`) is a separate structure built once at boot, and
      a swap that skipped rebuilding it left every newly-added op
      dispatchable but `access_denied` for any caller. Fixed with a new
      `ordo_host::runtime::refresh_access`, called after every
      `Swapped::Installed` (`spawn_deploy_listener` needed the identical
      fix for a second process's `ordo apply`, and now shares the
      function). Proved by `crates/ordo-host/tests/dev_loop.rs`'s
      `d3_hot_swap_serves_a_new_op_through_the_same_door`: a save that adds
      an op is refused before the swap (unknown to the door), answers
      through the SAME `axum::Router` after it with no restart, and a
      pre-existing op keeps answering too. Measured against the market
      fixture (378 lines, 165 ops, 76 reads — §3.3's representative save):
      gate ~330ms + swap-and-access-rebuild ~170ms ≈ 500ms, serial,
      consistent across runs — comfortably under the 1s bar with real but
      not enormous margin. The concurrent-describe optimisation §6 D3
      names first was attempted and reverted: it corrupted results under
      real concurrency (see `docs/corpus-rulings.md`); not needed to hit
      the bar at this scale, left named for later. `ordo migrate --apply`
      (schema-evolution increment 2) landed 2026-09-20 and D2's second
      rung with it: see the D2 paragraph above.
      D4 done 2026-09-19: `ordo dev --out <dir>` writes `interface.json`
      on every save whose content changed (byte-equality against the last
      save's — `interface_document`'s own digest is already baked into
      those bytes, so nothing is recomputed), and `--out` is opt-in: no
      third compile pass, no file, when nobody asked. `--gen <command>`
      runs after a write, `ORDO_INTERFACE_PATH` naming the descriptor; a
      non-zero exit prints the command's output and does not stop the
      loop (§6 D4 — the seam, not the generator). Proved by
      `crates/ordo-host/tests/dev_loop.rs`'s
      `d4_compile_once_computes_the_descriptor_only_when_asked` (the
      `descriptor` flag gates the pass, and its output is byte-identical
      to `ordo interface`'s own — one canonical pipeline) and a manual
      run: `ordo dev --out … --gen "echo …"` wrote `interface.json` and
      ran the configured command on the first save, journaled as
      `descriptor 0 (written) · generator 2 (ok)`.
      **D1–D4 all done; the budget test passes.** ordo dev now reaches
      save-to-serving through the real assembly, gate and door, measured
      under the 1s bar (§3.3) on the market fixture.

      **Where `ordo dev` finds Postgres.** `ORDO_PGBIN`, then the Homebrew
      path, then `PATH`, then the newest `/usr/lib/postgresql/<v>/bin`,
      which is where Debian and Ubuntu keep the server binaries while
      installing only the client tools into `PATH`. A CI runner whose
      Postgres is a service container has no `initdb` at all, so
      `d2_owned_cluster_lifecycle_and_delta` skips its owned half there,
      prints why, and still asserts the adopt half. The lifecycle is proved
      on any machine that can actually run it.

      **The bar is met on developer hardware, not on a CI runner.**
      This machine (M4) measures about 500 ms save-to-serving over the
      market fixture. The GitHub runner on 2026-09-19 measured
      `gate 1.085s · swap 233ms · total 1.317s` for the same fixture, so
      the 1 s bar is a developer-machine bar. The concurrent-describe
      optimisation the plan names (§6) is the lever if that ever needs
      to hold on slower hardware, and it is now safe to take: the
      corruption that made it unusable was the shape probe's own
      (`$shape` looked up by name across sessions), fixed 2026-09-19.

- [ ] **17p · Generated UI, steps 3–4.** Already homed: 14b remainder,
      14c, and the Watermelon skin + shadcn registry emission from the
      approved build order.
- [ ] **17q · Sharing.** Already homed: 11-2/11-3, the owner picked the
      recommended architecture (grant-shaped, no syntax) on 2026-09-08;
      registration is owed.
- [x] **17t · Readiness means ready to accept deployments, not "a
      program is deployed".** The owner, 2026-09-20: *"Being a baas in a
      way, a 503 is not an appropriate status. Readiness is not a measure
      of is a deployment running, it is a measure of are we ready to apply
      and run deployments."* Before this, `/health/ready` answered 503
      until a program was active and its shape gate passed, so a correctly
      configured platform with nothing deployed to it read as broken, and
      a Kubernetes readiness probe on it kept the pod out of service
      forever — confirmed failing first: `a_database_with_no_program_boots_and_is_ready`
      in `crates/ordo-host/tests/runtime.rs` asserted the old 503 until
      this landed.
      **Built and tested, 2026-09-20.** `health_ready`
      (`crates/ordo-host/src/serve.rs`) now answers the conjunction of five
      checks, none of them about whether a program is loaded: `database`
      (a pooled `SELECT 1`), `custody` (the existing `blind_index` probe on
      this deployment's own space, unchanged), `config` (the *live*
      program's required configuration — concretely, a non-inline `blob`
      field with a store attached, the one requirement a hot swap can
      change after boot; everything else `runtime::assemble` already
      refuses to boot without), `workers` (no registered worker
      quarantined — `Supervisor::quarantined`, new in
      `crates/ordo-host/src/supervisor.rs`), and `deploy_path` (this role
      has `CREATE` on the database — `has_database_privilege`, skipped
      when `database` already failed). Whether a program is active moved
      to the body (`program_active`), reported in both the ready and the
      not-ready case, alongside a `checks` object naming which check
      failed and why, one line per check, in `/health/deep`'s style — an
      unset variable's name or the `entity.field` with nowhere to put its
      bytes, never a credential's value, a host, or a port. `/health/deep`
      is unchanged: same detail, same owner gate. `PlatformState` gained a
      `blob: Option<String>` field (`with_blob_label`), wired from
      `runtime::assemble`'s existing `blob_label`, so readiness can tell a
      program that needs a store from one that has one without touching
      storage. 8 tests: `crates/ordo-host/tests/health.rs`'s 4
      (`a_deployment_with_no_program_applied_is_ready` — 200, and pins the
      regression by asserting directly on `Deployment::empty`'s fields
      that the pre-17t predicate would have failed both its checks;
      `a_deployment_whose_database_is_unreachable_is_not_ready` — the
      database is dropped out from under the pool's own connection with
      `DROP DATABASE ... WITH (FORCE)`, not ready, `program_active: true`;
      `a_deployment_missing_a_required_credential_is_not_ready_and_names_it`
      — a program declaring a non-inline blob field with no store
      attached, not ready, body names `docs.scan`;
      `the_same_program_is_ready_once_a_blob_store_is_attached` — the
      mirror, ready) plus `crates/ordo-host/tests/runtime.rs`'s
      `a_database_with_no_program_boots_and_is_ready`, rewritten from its
      old, now-overruled name and assertions, and three more updated
      doc-only for the same reason (`crates/ordo-host/src/deploy.rs`'s
      `Deployment::empty` and its "what is deliberately still `pinned`"
      section). The sibling repo's probe
      (`smg_infrastructure/ordo/ordo.yaml`) still points at `/health/live`
      as a workaround and needs moving back to `/health/ready` by hand —
      out of this repo, left open.

- [x] **17w · `ordo dev` cannot hand a developer a token, so nobody can
      call their own op.** Found while writing the quickstart, 2026-09-20,
      by trying to do it. **Built 2026-09-20**: the print-a-token option,
      judged against the code and taken — the only one of the three that
      answers without a developer first learning what a token, a
      principal or membership is, which is the right amount to know five
      minutes in.

      **The principal.** `dev_db::reset` (`crates/ordo-cli/src/dev_db.rs`)
      inserts one row into `members` on every full reset — `id =
      "mem_dev"` (`dev_db::DEV_PRINCIPAL_ID`), `role = "owner"`, the same
      `'dev'` space `dev_runtime_config` hardcodes — after `seed.sql`, so
      a developer's own seed rows are already there and nothing here
      depends on one existing. `members` is the platform-baseline entity
      every compiled schema carries (`ordo_schema::build_with_keys`), so
      the table is always there to insert into. **Dev-only, and why it
      cannot reach a real deployment:** `reset` has exactly one caller,
      `ordo_cli::dev::settle_delta`, itself reachable only from `ordo
      dev`'s own loop (`ordo_cli::dev::run_async`) — never `ordo apply`,
      `ordo migrate`, or the `ordo-host` server binary a real deployment
      runs.

      **The token.** Printed once, at boot, right after
      `Runtime::print_startup` (`crates/ordo-cli/src/dev.rs`,
      `print_dev_token`): `Bearer <token>`, a `curl` line carrying the URL
      and both headers, and a line saying plainly it is local only —
      signed under a key that dies with the process. Minted against
      `dev_runtime_config`'s own `token_secret` for `DEV_PRINCIPAL_ID`,
      expiring effectively never, because there is no session to refresh
      against (`auth: None`, unchanged — no login route is mounted).

      **`ORDO_TOKEN_KEY`, added — cheap.** `dev_runtime_config` now reads
      it the way a server's own `RuntimeConfig::from_env` does: unset
      falls back to the ephemeral, process-derived key exactly as before;
      set and well-formed overrides it, for a script that wants a token
      good across restarts; set and malformed is a refusal rather than a
      silent fallback, the same posture `TokenKey::from_env` itself takes
      — a developer who set it meant something by it. `ordo-identity` was
      already a dependency for the crate's own tests; promoted to an
      ordinary one.

      **The test — the wall, not the mechanism.** New:
      `dev::tests::the_printed_token_calls_a_real_op_over_real_http`
      (`crates/ordo-cli/src/dev.rs`). Compiles a one-entity, one-op
      program, runs the real `dev_db::reset` against a real Postgres
      database, builds the real `dev_runtime_config`, boots through the
      real `ordo_host::runtime::assemble_embedded`/`serve` on an allocated
      loopback port, mints the token `print_dev_token` would have
      printed, and calls the op over a REAL HTTP/1.1 connection —
      `ordo_blob::http::send`, the same client `ordo apply --host` uses
      (`wire.rs`), never `tower::ServiceExt::oneshot` — asserting the row
      it seeded comes back. Confirmed to fail first: with the seeding
      insert disabled by hand, the same test gets 403 `access_denied`.
      **In-process rather than a backgrounded `ordo dev` binary** — a
      backgrounded child is killed quickly in this sandbox, confirmed
      again while building this — but every function under test is the
      real one; nothing here is a second, parallel implementation.
      `dev::tests::ordo_token_key_overrides_or_refuses` covers the
      `ORDO_TOKEN_KEY` branch (well-formed, malformed, unset) directly, in
      one function because both halves mutate the same process-global
      variable and a parallel test runner would otherwise race them.

      **The quickstart.** §2's "Becoming a member" now shows the printed
      token instead of a hand-written `seed.sql` insert for the wall
      itself; `seed.sql` is documented as how a developer adds a SECOND
      principal, not as the only way in. §3's `curl` example and "Where
      this is thin"'s authentication bullet updated to match.

- [x] **17u · The CLI speaks to a deployment, not to its database.** The
      owner, 2026-09-20: *"CLI should interact with the endpoint, should be
      adjusted with a flag."* **Built 2026-09-20 for `apply`**, over 18l's
      door; `crates/ordo-cli/src/wire.rs` is the client and the module doc
      carries the argument for every choice below.

      **The flag.** `--host <h>`, or `ORDO_HOST`; `--token <t>` or
      `ORDO_TOKEN`; `--space <s>` or `ORDO_SPACE`. **Default host
      `ordo.inf.smg.icu`** (owner, 2026-09-20, confirming that the
      `omni.inf.smg.icu` he first wrote was a slip), which `--host` with an
      empty value selects. **With neither flag nor variable the command
      talks to the database exactly as it did**: the default host is what
      `--host` means when it is given, not what the command does when
      nobody mentioned a host.

      **The scheme.** A bare host gets `https`; `http://` must be spelled,
      which is what addressing a local `ordo dev` looks like (`--host
      http://127.0.0.1:4000`, `ORDO_DEV_ADDR`'s default, plaintext with no
      certificate). That is a deliberate departure from
      `ordo_blob::http::Endpoint::parse`, which refuses a schemeless
      endpoint — a blob endpoint is configuration and the failure that
      matters there is a silent upgrade of a local gateway; a `--host` is
      typed at a shell against a bare public default, and this request
      carries a token that can replace the deployment's program.

      **The HTTP client: none was added.** `ordo_blob::http` is a minimal
      HTTP/1.1 client over `tokio-rustls`, hand-rolled for the S3 surface,
      and this is its second caller — so the `ordo` binary and the three
      Docker images gain no new third-party crate and no native TLS
      (`plan-ci-images.md` §1.5's ask, satisfied because the existing
      client is already rustls). `reqwest` would have brought connection
      pooling, redirect following, cookie jars, proxy discovery and a
      middleware stack for one POST; `hyper` + `hyper-util` +
      `http-body-util` + `hyper-rustls` is four manifest entries for the
      same. `ordo-blob` and `futures` become direct dependencies of
      `ordo-cli`; both were already in its tree.

      **The token.** An ordinary sign-in token (`POST /auth/begin`, then
      `/auth/complete`) for a principal who owns the deployment's own
      space. There is no operator credential and no API key — "use what
      the door already accepts". It must carry presence, because applying
      is step-up gated; it can never be a capability token, because a
      capability token can never satisfy step-up, which is the mechanism
      that makes applying structurally human-only.

      **`--as` is refused with `--host`**, as a usage error before a
      connection is opened. On the database path it is an assertion an
      operator holding credentials makes; over the door the actor recorded
      is the principal the token proved, and a flag that does not reach the
      record it looks like it is writing is worse than no flag.

      **The tests** (`crates/ordo-cli/tests/wire_apply.rs`, 7): the
      headline is
      `the_wire_and_the_database_print_the_same_report`, which applies one
      program to two empty databases by the two paths and compares the
      reports **as text** — not field by field, because a field comparison
      would pass for a build where the two renderers had drifted, and that
      is the only way this can go wrong. The mechanism behind it is that
      both call `operator::render_apply` over the same
      `apply_op::Report`. Then
      `a_refusal_over_the_wire_carries_the_direct_paths_class`
      (`not_activated` with the blocking change's `A2` on both),
      `a_broken_program_is_refused_on_both_paths` (`apply_refused`, and
      nothing reaches either database),
      `a_manager_is_refused_over_the_wire_by_name`,
      `as_is_refused_together_with_host`,
      `a_missing_token_says_where_one_comes_from` (the refusal names
      `/auth/complete` and opens no connection), and
      `without_a_host_the_command_still_talks_to_the_database`. The
      deployment listens on a real loopback socket rather than being handed
      a `Service`, because the point of the file is the client: the request
      it builds, the headers it sends and the body it reads back.

      **Which commands follow, and which must not.** `rollback`, `deploys`
      and `migrate --apply` are the three that should follow, in that
      order: the first two are a re-point and a read of the same registry
      this op already writes, and each needs its own platform op and its
      own gate. `migrate --apply` is the largest and is deliberately not
      done here — it sends transforming DDL and its refusals carry hand
      statements, so its door needs the whole `schema_apply::Refused`
      taxonomy on the wire rather than the one summary class
      `apply_refused` collapses to. **`keys`, `verify`, `checkpoint`,
      `prune` and `revert` stay on the database**, as the ruling says: each
      has to work when the door does not, and `ordo keys init` in
      particular is what a deployment cannot boot without.

      **Left open.** The deployment's own space has no published constant,
      so `--space` is required rather than defaulted; 17v owns that name.
      See 18l for why the op compares it rather than asking
      `is_system_space`.

- [x] **17v · The system space exists at boot.** The owner, 2026-09-20:
      *"System space should exist at boot because nothing can really exist
      without it, it is where ordo configurations live."* Confirms and
      extends the `system-space` ruling (the third home, platform-owned,
      with a universal read window). Today nothing creates it: boot refuses
      when `ORDO_SPACE` has no custody, and `provision_space` has no caller
      outside tests. **The distinction that keeps the existing rule intact:**
      "provisioning is never a boot side effect" exists so a typo in a
      *tenant* name cannot mint a tenant, and that stays true. The system
      space is not a tenant: it is the platform's own home, its name is the
      platform's to choose rather than the operator's, and a deployment
      without it has nowhere to keep what it knows. So boot creates the
      system space if it is absent, and still refuses an unknown tenant.
      Done when a fresh database plus a master key yields a running
      deployment with a system space, with no operator command in between.

      **Closed 2026-09-20, from the first cluster deployment's crash loop:**
      `ordo-host: custody: key store: $keycustody does not exist`. Two
      defects in one line, both fixed.

      **The platform's tables are created at boot, custody included.**
      `ordo_exec::keystore::ensure_tables` now runs in
      `ordo_host::runtime::assemble` immediately before `build_custody`,
      the same place and for the same reason the artifact registry's moved
      there on 2026-09-19: the database a first deployment is pointed at has
      neither table. Creating `$keycustody` is not minting custody, because
      an empty custody table grants nobody anything and
      `LocalKeyProvider::release` still refuses a space with no SMK.

      **The system space is `system`, and boot creates it.** The id is
      `ordo_exec::erase::SYSTEM_SPACE`, re-exported as
      `ordo_host::provision::SYSTEM_SPACE` with the argument for the name
      beside it. `build_custody` mints its keys (idempotently, so restarts
      and replicas converge), and `assemble` calls the existing
      `provision::provision_space` for the founding member row, placed after
      the program loads (a program's DDL owns `members`) and before the
      access engine is built from that table, so the space this boot created
      is a space this boot serves. On a database with no program the boot
      mints custody and reports `custody only`, and the **first `ordo apply`
      completes it without a restart**: the deploy listener calls the same
      `ensure_system_space` on an installed swap, before it rebuilds the
      door from `members`. Both paths are `provision_space`'s own retry
      semantics rather than a second way to create a space, and a failure
      there is a log line, because a node that is serving must not stop
      serving over it.

      **Where it lives, and the `$` it does not carry.**
      `ordo_exec::erase` already reserved `$`-prefixed ids for
      platform-owned spaces and refused to erase them. The boot's space
      cannot use that prefix, because `ordo_api::well_formed_space` binds
      only alphanumerics, `_` and `-`, and a space the door cannot bind can
      have no member row, no engine built from it and no read window over
      it. So `is_system_space` answers for the name as well as the prefix,
      in one function rather than two that could differ.

      **Tenants keep their protection.** The rule `build_custody` existed
      for is that a typo in `ORDO_SPACE` cannot mint a tenant, and the
      distinction that keeps it is *where the name came from*: the system
      space's id is a constant compiled into the binary, read from no input,
      while a tenant's arrives in a variable, a header or a command line.
      `ORDO_SPACE` naming a tenant with no custody is now its own refusal,
      `BootRefusal::UnprovisionedTenant`, which says the platform's own home
      is up and this master key opens it, that what is missing is a tenant,
      and gives `ordo keys init --space <tenant> --as <you>`. The cluster's
      `ORDO_SPACE=smg` therefore still refuses, deliberately, and
      `deploying.md` §4c gives the first-deployment order: come up on
      `system`, create the tenant, then move `ORDO_SPACE` to it.

      **Tests.** `ordo-host/tests/runtime.rs` (13 pass), each against the
      binary as a process:
      `a_fresh_database_and_a_master_key_boot_with_a_system_space` is the
      done-when, and it was written first and watched fail with the
      deployment's exact line: an empty database plus a master key now
      serves, `$keycustody` holds the system space's SMK and epoch 1 DEK,
      and the startup log names the space and what the boot did about it.
      The same test then runs that deployment's first `ordo apply` against
      the live process and waits for the founding row, which is the swap
      path; it is one test rather than two because each binary this suite
      spawns is a pool on a cluster another suite is using, and the two
      halves are one deployment's first minutes.
      `the_system_space_gets_one_founding_row_across_two_boots` proves the
      founding row is written once and that the second boot reports
      `present` rather than writing a second owner.
      `an_unprovisioned_space_is_refused_with_its_remedy` now also asserts
      the refusal says *tenant*, names the space, and that `$keycustody`
      holds nothing for the mistyped name. In `ordo-exec` (201 pass),
      `erase.rs`'s `the_system_space_has_no_exit` now refuses the space
      every deployment has, not only a `$`-prefixed one.

      **Left open:** the universal read window the 2026-09-12 ruling names
      is still unbuilt, so "everybody reads it" is not yet a mechanism, and
      the system space holds nothing but its own custody and its founding
      member until a surface puts configuration in it. Nothing reads
      `SYSTEM_PRINCIPAL` as a credential; it is an attribution, not a login.

- [ ] **17r · Notifications & routes.** Settled spec, zero code; behind
      17k. Listed so the popular demo claim ("via push") has a tracked
      owner.
- [x] **17s · SDK + CDK, generator-first.** Homed in 14d/14f, both
      closed: one generator, two outputs, both ratchets live. Ticked
      2026-09-18 when the board reconciliation found it open over two done items.
- [ ] **18o · The operator and the console: hosting many deployments.** **The
      console is required for the release launch (owner, 2026-10-22): it is how
      less technical people read and manage their instance, and it ships in the
      open-source release. The operator stays later.** Ruled
      2026-09-21 (`corpus-rulings.md`, "The control plane is a service
      above deployments"). A service that registers, names, provisions,
      meters and bills deployments, each of which stays one process, one
      database and one separately managed master key. **Not an overhaul of
      the engine**, which is the point of the shape: nothing in a
      deployment learns about the service, so the keying that would have
      been needed to put many deployments in one process is never needed.

      What it owns: the global namespace for deployments and spaces; a
      registry of deployments and where each lives; provisioning (bringing
      a deployment up with its own master key, which it generates and
      hands over once); the control database each deployment writes its
      checkpoint witness into; billing, which the usage ledger (17n)
      already produces per space.

      What it must never do: reach into a deployment's spaces, hold a
      deployment's master key after provisioning, or become something a
      deployment discovers rather than is configured with. Dependence runs
      one way and stays configuration.

      Prerequisites in the engine, each small and worth having anyway: a
      deployment id on the four tables that have no key (`$artifacts`,
      `$activations`, `$jobs`, `$schedules`); the identity tier keyed so
      one login does not span deployments; and 17aa, so one tenant's state
      cannot take a deployment down, because a hosted service multiplies
      that failure by every customer on it.

      **18o · step 1a, built 2026-10-23** (`plan-console.md` section 12,
      step 1, first session). New crate `crates/ordo-console`:
      - The server: `/health/live`, `/health/ready` (never asks the
        deployment), CSP `default-src 'self'`, `frame-ancestors 'none'`,
        `Referrer-Policy: no-referrer`.
      - Its own Postgres database, `ORDO_CONSOLE_DATABASE_URL`, required.
        Boot refuses it unset, and refuses one naming the deployment's
        database (`ORDO_PG_HOST`, `ORDO_PG_PORT`, `ORDO_PG_DB`). Own
        migrations at boot. Every row keyed by a deployment handle. Opened
        with the new `Pool::connect_own`, so it gets no deployment tables.
      - Sign-in: first-run setup code (log and `setup-code` in
        `ORDO_CONSOLE_DATA_DIR`, one use); passkeys through
        `ordo_identity`'s provider on the console's own relying party;
        session cookie HttpOnly, `SameSite=Strict`; idle and absolute
        limits (`ORDO_CONSOLE_SESSION_IDLE_SECS` 1800,
        `ORDO_CONSOLE_SESSION_MAX_SECS` 43200); step-up within 300 s; 10
        recovery codes, argon2id, a used one forces a new passkey and set;
        a second owner by invite code; removing an owner needs a touch.
      - `ordo console recover [--owner <id>]`: a one-time code, audited
        with the unix user; using it ends every console session.
      - An audit row for every console write. One `ConsoleError`, one edge
        mapper; fixable refusals name the setting.
      - Tests: `crates/ordo-console/tests/console.rs`, 11 cases.

      **18o · step 1b, built 2026-10-24** (`plan-console.md` section 12,
      step 1, second session). Step 1 is built:
      - The page, `clients/ordo-console-web`: plain React and Vite, no
        `ordo-react`, only the console's JSON API, same origin. Screens:
        first-run setup, recovery codes shown once with a "saved" check,
        passkey sign-in, a recovery code then a forced new passkey and
        set, invite and join, remove an owner (with a touch), sign out,
        and an empty Health home with a nav for later screens. No tokens
        in storage; no inline script or style.
      - The server serves it from `ORDO_CONSOLE_PAGE_DIR` (default
        `/usr/share/ordo-console/page`): `index.html` for `/d/<handle>/*`,
        hashed files at `/assets/`. A missing page logs the setting and
        the API still answers. New `GET /d/<handle>/api/status` says only
        whether an owner exists.
      - The one image, `docker/Dockerfile`: `ordo-console`, `ordo-host`,
        `ordo` and the page, distroless, `ORDO_CONSOLE_LISTEN`
        `0.0.0.0:8081`. The console runs alone; step 2 makes it
        supervise `ordo-host`. `images.yml` job `ordo` builds and smokes it.
      - e2e: `clients/ordo-console-web/e2e`, Playwright with a virtual
        passkey, 5 cases (setup, sign-in, invite and remove, recovery
        code, phone). CI: `console-e2e.yml`.
      - Tests: `console.rs` now 13 cases.

      **18o · step 2a, built 2026-10-24** (`plan-console.md` section 12,
      step 2, first session). The supervisor, back end only:
      - `ORDO_CONSOLE_MODE`: `supervise` (default) runs `ordo-host` as the
        console's child, in its own task apart from the web server.
        `door-only` builds no supervisor, so it holds no spawn path.
        `/health/ready` stays ready while the child is down and says where
        it is.
      - The child gets a cleared environment: its settings, `<NAME>_FILE`
        per secret, `ORDO_RUNTIME_REVISION`. Managed secrets are sealed
        with the console key (`ORDO_CONSOLE_KEY_FILE`, made at first boot)
        and written 0600 under `ORDO_CONSOLE_RUN_DIR`. Referenced secrets
        pass their path; the console never reads them.
      - Started through `ordo-console` in launcher mode: switch to
        `ORDO_CONSOLE_CHILD_USER` (Linux), set the parent-death signal,
        exec. No `unsafe` (`rustix`). A refused switch falls back to one
        user, logged with the setting and the consequence.
      - Backoff 1 s doubling to 60 s, reset after 10 minutes ready. 5 exits
        in 10 minutes is crash-looping, retried every 60 s. A crash loop
        within 10 minutes of an apply rolls back once; a second holds
        automatic changes.
      - Revisions: revision 1 from the container's `ORDO_*` variables. Each
        apply is a row: settings, secret references, who, why, outcome
        (`good`, `refused`, `failed`, `crashed`). Apply runs G17, the new
        `ordo-host check-config`, then SIGTERM, 30 s, SIGKILL, then waits
        up to 120 s for ready, else restarts the last good revision.
      - API, signed in, writes need a touch and are audited: `GET runtime`,
        `GET runtime/revisions`, `POST runtime/apply`, `runtime/rollback`,
        `runtime/restart`.
      - Tests: `supervisor.rs`, 9 cases against a fake host that starts
        healthy, crashes, hangs or fails readiness (1 Linux only: the child
        dies with the console). `check-config` in `ordo-host`'s
        `runtime.rs`. `console.rs` still 13.
      - Left for step 2b: the Runtime screen, Health from the supervisor,
        the rehearsal, and the image's unix-user split (it runs as one user
        today).

      **18o · step 2b, built 2026-10-24** (`plan-console.md` section 12,
      step 2, second session). Step 2 is built:
      - Secrets sealed. The console keeps one list of secret settings
        (`spec::SECRET_SETTINGS`, plus names ending `_SECRET`, `_PASSWORD`,
        `_PASSPHRASE`). A value given for one is sealed and reaches the
        child as `<NAME>_FILE`; no revision row holds it. A change that
        sends one as a setting is refused, naming `secrets`. Revision 1
        seals the container's plain values; a step 2a row is sealed at boot.
      - `ordo-host` reads every secret through one reader, the new
        `ordo-env` crate: `ORDO_TOKEN_KEY_FILE`, `ORDO_PAGE_KEY_FILE`,
        `ORDO_MASTER_KEY_FILE` under `local`,
        `ORDO_CHECKPOINT_KEY_FILE`, `ORDO_SECURITY_CHECKPOINT_KEY_FILE`,
        `ORDO_BLOB_S3_SECRET_ACCESS_KEY_FILE` are new. Both forms set, or
        an unreadable file, refuses the boot by the `_FILE` name.
      - The image runs two unix users: the console 65532, the deployment
        65533 (`ORDO_CONSOLE_CHILD_USER`). The console binary carries
        CAP_SETUID, CAP_SETGID and CAP_CHOWN as permitted file
        capabilities; the launcher drops all of them before `ordo-host`.
        The console marks itself not dumpable (its `/proc` closed to the
        child; `hidepid` is the platform's). Volume 0700.
      - Runtime screen and Health from the supervisor: state (starting,
        ready, restarting, crash-looping, held), restarts, last exit,
        revisions, apply with progress up to about 150 s, roll back,
        restart. An owner's restart ends a hold.
      - Tests: `supervisor.rs` 11 (the rehearsal: kill, bad settings, crash
        loop, held, while health and the page answer 200 throughout; no
        secret in a revision row). `runtime.rs` gains the `_FILE` forms.
        Console e2e 13 (8 on the Runtime screen, against the fake host).
        The image smoke proves the user split with only the three
        capabilities granted.

      **18o · step 3a, built 2026-10-24** (`plan-console.md` section 12,
      step 3, first session; the security review comes before 3b). The
      link from the console to the deployment:
      - G14, the console key. `ordo-host` stores an Ed25519 public key per
        console as a credential of the owner who enrolled it
        (`$consolekey`). New ops, owner-only, home space:
        `std.deployment.addConsoleKey` and `revokeConsoleKey` (writes, so a
        touch by default), `consoleKeys` (read). At most 2 live keys an
        owner. `/auth/begin` and `/auth/complete` with `method: "console"`:
        a single-use 60 s challenge, signed with a fixed prefix. The session
        lasts `ORDO_AUTH_CONSOLE_SESSION_SECS` (900, 60 to 3600), carries no
        presence, and `/auth/refresh` refuses it. A revocation ends the
        key's open sessions at once.
      - G15, the relayed touch. A step-up challenge is now bound to the
        session that asked for it, for every session. A console session
        stepped up stays present 300 s and keeps its own end.
      - G16, the boot record. The deployment writes the console's
        `ORDO_RUNTIME_REVISION`, `ORDO_RUNTIME_APPLIED_BY` (new, set by the
        supervisor), `ORDO_IMAGE_DIGEST`, the binary's SHA-256 and a
        settings fingerprint (a secret's value left out) to its security
        record, and shows them on `/health/deep` under `boot`.
      - The console's one client to the door (`door.rs`, one typed
        `DoorError`) and the link (`link.rs`): claim or sign in with the
        owner's deployment passkey, enrol the key, seal its private half in
        `credentials`, sign the owner's session out. Routes under
        `/d/<h>/api/door`: status, link, step-up relay, rotate, unlink.
        `ORDO_CONSOLE_DOOR_URL` defaults to the child in the one image;
        plain http only to loopback unless `ORDO_CONSOLE_DOOR_PLAIN_HTTP=on`.
        `ORDO_CONSOLE_DOOR_SPACE` names the home space.
      - Tests: `ordo-host` `console_key.rs` 10; `ordo-console`
        `door_link.rs` 3 against the real sign-in routes in-process,
        `door_scan.rs` 2, door unit tests; secevent round trip.
      - Left for step 3b: the page's claim and touch screens, rotation UI
        and the 30-day schedule, the cross-site rehearsal on two domains,
        Health from the door, the CLI's `revokeConsoleKey`.

      **18o · step 3b, built 2026-10-24** (`plan-console.md` section 12,
      step 3, second session). Step 3 is built.
      - Security review of 3a, 7 findings fixed, each with a test that
        failed before:
        1. A console session can no longer use `/auth/handoff` to come back
           as a full session that revoking the key would miss.
        2. A console session reaches only `/auth/step-up*` and
           `/auth/logout` among `/auth/*` (one layer over every auth route,
           `auth::console::gate`; a test scans every mounted route), and
           the door's ops only in the home space (new security decision
           `ConsoleSessionOffHome`). Adding a passkey through
           `/auth/authenticators/link` now needs a touch bound to the same
           session.
        3. `/auth/begin {method: "console"}` stores nothing: the challenge
           carries its expiry and a MAC under a key derived from the token
           secret. It is spent only after the key's signature verifies.
           Console sign-in attempts are throttled, 30 a minute a node.
        4. `addConsoleKey` and `revokeConsoleKey` can never be exempted by
           `ORDO_STEP_UP_EXEMPT_OPS`; naming one refuses the boot.
        5. A sign-in writes its session row, then checks the key is still
           live, so a racing revocation ends it.
        6. The console carries the session from a touch's begin to its
           finish, and never renews a session the owner touched for.
        7. One list of secret settings, in `ordo-env`, for the console's
           sealing and the boot record's fingerprint; `_CODE` and
           `_CREDENTIAL` values are never hashed.
      - The page: a Deployment screen. Claim the deployment with a new
        passkey (and the claim code) or link with the owner's passkey
        there; the console keys, each but its own revocable; rotate now;
        unlink; forget a link the deployment revoked. A write at the
        deployment asks for the owner's touch there in a dialog, relayed
        for the console's own session. The screen names the address to put
        in `ORDO_AUTH_ORIGINS` or `ORDO_AUTH_RELATED_ORIGINS`.
      - Rotation schedule: `ORDO_CONSOLE_KEY_ROTATE_DAYS` (30, 1 to 365).
        A rotation needs the owner's touch, so the console shows the key as
        due on Health and Deployment and never rotates alone.
      - Revocable from either side: unlink in the console, or
        `ordo console-key revoke --id <ck-…>` (new) or the op at the
        deployment. The console then says the deployment refused its key
        (`console_key_refused`) and offers to forget the link.
      - Health from the door: ready, deep (with the boot record) and the
        nodes, on the Health screen.
      - Tests: `door_link.rs` 5, with the cross-site rehearsal (console on
        `https://console.ops.test`, deployment relying party `deploy.test`
        reached as `localhost`, the console origin in its related origins:
        link, read, a write with the relayed touch, rotation, revocation
        from both sides); `wire_console_key.rs` 1; console e2e `door`
        project (claim, a relayed-touch write, unlink) against
        `examples/e2e_door.rs`, ordo-host's real sign-in routes.

      **18o · step 4, built 2026-10-24** (`plan-console.md` section 12,
      step 4). Deploys:
      - Three new door ops in `ordo-host` (`deploy_ops.rs`), owner-only,
        home space only:
        - G1 `std.deployment.versions`: stored versions, which serves, the
          activation log. A read, no touch.
        - G2 `std.deployment.activate($version)`: a program rollback. A
          change, so a touch. Refuses an unknown version (`not_found`) and
          a version whose tables the database lacks (`not_activated`).
        - G3 `std.deployment.plan($program)`: what an apply would do
          (create, matches, migrate, held, refused) and each change, with
          nothing stored. A `POST`, so a touch: a program is up to 1 MB and
          the plan probes live rows. Probe values are never returned.
      - One body each in `apply_op.rs` (`history`, `activate`, `preview`).
        `ordo deploys` and `ordo rollback` call them too, so the CLI's
        rollback now takes the apply lock and refuses a version the
        database is behind on.
      - G12: an apply or activation through the door writes a `deploy`
        event (new `EventKind::Deploy`, chain tag 24) beside the registry
        rows that name the caller.
      - Console routes under `/d/<h>/api/deploys` (`server/deploys.rs`):
        history, serving (`/health/deploy`), preview, apply, activate.
        Changes need the console owner's touch and the relayed touch at
        the deployment; each is audited. Door calls for the program allow
        180 s.
      - The Deploys screen: choose a `.ordo` file, preview, apply or store
        only, wait until every node serves it with progress, versions with
        "Roll back the app", the activation log.
      - Tests: `deploy_apply.rs` 23 (8 new: each op, no touch, wrong role,
        tenant owner, unknown version, behind version, nothing stored, the
        event); `door_link.rs` 7 (2 new); console e2e `door` project now
        previews, applies two versions and rolls the app back (14 in all).
      **18o · step 5, built 2026-10-24** (`plan-console.md` section 12,
      step 5). Tenants:
      - G4, `std.deployment.tenants`: a read, owner-only, home space. Each
        tenant's id, name, status, phase, when made, the database it is
        on and its custody, its member count (a number, never a name),
        its last rolled-up `$usage` period, this node's quarantine reason
        and the caller's role there. Reaped tenants only with
        `includeReaped`. The reading moved to `ordo_host::fleet`;
        `ordo fleet` now calls it and prints what it printed before.
      - G12, the tenants part: `std.space.create` now records
        `tenant-created` with who, the owner and the outcome, in the
        deployment's stream and the tenant's. Pause, resume and reap
        already recorded; a test now holds all four.
      - A console session may now leave the home space for one thing:
        `std.space.export`, `delete`, `attachBucket` and `detachBucket`
        in a tenant, within 300 s of the owner's relayed touch
        (`console_keys::TENANT_OPS_OPEN`). The key alone reaches no
        tenant, and the door still asks for the owner role there.
      - The console: routes under `/d/<h>/api/tenants` through the door
        client (list, create, reap, pause, resume, export, delete,
        attach and detach a bucket), each write audited and behind both
        touches. Delete needs the tenant's id typed back. The Tenants
        screen: a table that becomes one card a tenant on a phone, the
        tenants held closed and why, create, pause with a reason from the
        closed list, resume, remove one stuck being set up, and export,
        delete and the bucket where the owner owns the tenant.
      - Tests: `ordo-host` `deployment_tenants.rs` 4 (G4 and the G12
        records against Postgres), `console_key.rs` 1 new;
        `ordo-console` `tenants.rs` 3 against the real door check with
        the tenant ops stood in (`tests/support/tenants_door.rs`, shared
        with `examples/e2e_door.rs`); console e2e `tenants` project (list,
        create, pause, resume, the typed delete, phone width).
      **18o · step 7, built 2026-10-24** (`plan-console.md` section 12,
      step 7). Usage, quotas, slow ops:
      - G9, `std.deployment.quotas` (a read, with what each space used
        this period) and `std.deployment.setQuota` (a write, needs a
        touch): owner-only, home space. Both run on
        `quotas::set_limit`, which `ordo quota set` also calls; it now
        refuses a period of 0 as well as an unknown quantity, and a
        refusal is a `QuotaRefusal`. Through the door a quota on a tenant
        the registry does not hold is refused. With the usage ledger off
        the answer says the quota gates nothing until
        `ORDO_USAGE_LEDGER=true`.
      - G12, the quotas part: every `setQuota` records `quota-set` with
        who, the space, the numbers and the outcome, in the deployment's
        stream and the tenant's.
      - Fixed: `/usage` refused every caller of a real deployment,
        because `platform.usage` was never granted. The deployment owner
        now holds it beside `/metrics` (`engine_from_directory`).
      - `std.deployment.observedCost` rows now name the node
        (`ORDO_REPLICA_ID`) and when it began counting (`since`).
      - The console: `/d/<h>/api/usage` (one tenant, or the whole
        deployment with each tenant's share, summed from `GET /usage`
        through the door client, which now reads JSON lines), `usage/quotas`,
        `usage/quotas/set` (audited, both touches) and `usage/slow-ops`.
        The Usage screen: a range picker, requests per period as plain
        SVG bars with a text summary, usage by tenant, uncounted periods
        named, the quotas with edit and add, and the slow ops table from
        one node, sortable by any column.
      - Tests: `ordo-host` `deployment_quotas.rs` 4 (G9, the G12 record,
        refusals for no touch, a manager, a tenant owner, an unknown
        quantity, a zero period, an unknown tenant, and the `/usage`
        grant); `ordo-console` `usage.rs` 3 against the real door check
        with the answers stood in (`tests/support/usage_door.rs`, shared
        with `examples/e2e_door.rs`); console e2e `usage` project (16 in
        all). The `tenants` spec now unlinks at its end so the next spec
        claims with its own passkey.

      **18o · step 6, built 2026-10-24** (`plan-console.md` section 12,
      step 6). Settings:
      - One registry of settings, `ordo_host::settings::REGISTRY`, beside
        the one config reader: each setting's name, group, default and
        range, families such as `ORDO_AUTH_<NAME>_CLIENT_ID` included.
        A test holds it and `docs/settings.md` to the same names and
        defaults; another fails on any `"ORDO_*"` name a deployment crate
        reads that neither lists. Three rows were missing from the doc
        and are added: `ORDO_DECRYPT_SEEN_SECS`, `ORDO_NODE_BEAT_SECS`,
        `ORDO_AUTH_PROVIDER_CA_FILE`.
      - G7, `std.deployment.settings`: a read, owner-only, home space.
        Each setting's effective value, default, range and source
        (`default`, `environment`, `file`, or `stored` when a platform
        op's value wins: claim code, guest, self-serve). A secret only as
        set or unset (`ordo_env::hides_value`); the claim code's stored
        hash never, only `required` or `off`. The app developer's stored
        layers (session policy, email policy, link password limits) are
        rows too. Per node: it reads that process's environment.
      - `ORDO_AUTH_CLAIM_CODE` is sealed: it joined
        `ordo_env::SECRET_SETTINGS`, the one list the console and the
        boot record read. The host reads it through `ordo_env::secret`
        (file form works), `check-config` refuses a bad one, and no
        refusal repeats its value. A revision filed with it in plain is
        sealed at the console's next boot by the existing reseal.
      - The console: `GET /d/<h>/api/settings` joins G7 with the live
        revision and marks drift per setting (not taken, a stored value
        wins, set outside the console). A change goes through
        `runtime/apply`: a new revision, restart, automatic rollback,
        behind the console owner's touch. Door-only mode answers
        `editable: false` and the screens are read-only. Two screens:
        Sign-in settings (sections 1 to 3 of `settings.md` and the
        stored layers) and Every setting, grouped, searchable, one card
        a setting on a phone.
      - Tests: `ordo-host` `settings` unit 6 (registry and doc guard,
        code scan, sources, secrets, stored values, families) and
        `deployment_settings.rs` 3 against Postgres (values and sources,
        stored values, owner-only); `ordo-console` `server::settings` 6
        and `spec` 1 unit, `tests/settings.rs` 4 (drift through a real
        revision, the claim code sealed, door-only read-only, signed out);
        console e2e `settings` project (view, change ORDO_AUTH_GUEST,
        stored-value drift, phone width).

      **18o · step 8a, built 2026-10-24** (`plan-console.md` section 12,
      step 8, session 1 of 2). People:
      - G5, four reads in `ordo-host` (`people_ops.rs`): `std.space.members`
        (each person's role, status, joined and groups; never a group as a
        person; paged by member id, 500 a page), `grants` (each live grant
        with its ops, whose and which client), `groupMembers($group)` and
        `callableBy($member)`. Manager and owner, decided by the engine's
        platform loop like `callersOf`; a viewer, an editor and a person
        with no role there get `access_denied`. No touch unless
        `ORDO_PRESENCE_ON_READS`; no delegated token.
      - `callableBy` answers only ops the asker may call too. It runs on
        `AccessEngine::callable_by`, the body `callers_of` answers from,
        cut to one person, so the two cannot disagree. An id the tenant
        does not hold is `not_found`.
      - `console_keys::TENANT_OPS_OPEN` (still the one list) now also
        holds the four reads, `groups`, `callersOf`, `invite`, `setRole`,
        `remove` and `addToGroup`. Same rule: within 300 s of the owner's
        relayed touch, and only where the owner's role may call the op.
      - G12, the People part: invite, change a role, add to a group and
        remove already record (`invitation-issued`,
        `member-role-changed`, `group-member-added`, `member-removed`);
        a test now holds all four.
      - The console: `/d/<h>/api/people` (members, groups, grants),
        `people/group`, `people/callable`, `people/callers`, and the
        writes `people/invite`, `set-role`, `remove`, `add-to-group`,
        each audited and behind both touches. The People screen: pick a
        tenant where you hold owner or manager, members as a table that
        becomes cards on a phone, what one person can call, who is in a
        group, grants given to apps, who can call an op, invite (the
        ticket shown once), change a role, add to a group, remove.
      - Tests: `ordo-identity` 1 (`callable_by` agrees with `callers_of`);
        `ordo-host` `people_reads.rs` 5 against Postgres (each read, the
        roles that may not see, a tenant where the asker holds no role,
        `callableBy` cut to the asker, the four G12 records),
        `console_key.rs` 1 new (a fresh touch and a role, past the
        window, the key alone, an op off the list); `ordo-console`
        `tests/people.rs` 2 against the real door check with the answers
        stood in (`tests/support/people_door.rs`, shared with
        `examples/e2e_door.rs`); console e2e `people` project (18 in all).
      - Left for session 2: suspend and reinstate, remove from a group,
        create and delete a group, per-member access and the access
        profile, invitations listed and revoked, admission, recovery,
        succession and delegations (4.5's other rows); the SDK's typed
        wrappers for the four reads.
      **18o · step 8b, built 2026-10-24** (`plan-console.md` section 12,
      step 8, session 2 of 2). The rest of People:
      - `console_keys::TENANT_OPS_OPEN` (35 ops) now also holds `suspend`,
        `reinstate`, `createGroup`, `deleteGroup`, `removeFromGroup`,
        `profile`, `setOpAccess`, `resetOpAccess`, `setMemberAccess`,
        `invitations`, `revokeInvitation`, `admission`, `setAdmission`,
        `recoverMember`, `approveMemberRecovery` and
        `setRecoveryPolicy`. Same rule: the owner's relayed touch in the
        last 300 s, and a role there that may call the op (owner for
        membership, admission and recovery settings).
      - The console: `people/invitations`, `people/profile`,
        `people/admission` (reads), and `people/suspend`, `reinstate`,
        `create-group`, `delete-group`, `remove-from-group`,
        `set-op-access`, `reset-op-access`, `set-member-access`,
        `revoke-invitation`, `set-admission` (the four ways in: open, its
        role, provider rules, first-comer; the second factor and
        connections stay with the app), `recover-member` (a ticket, when
        there is one, answered once like an invitation's and never
        audited), `approve-recovery` and `recovery-policy`. Each write
        audited and behind both touches. The People screen gains suspend
        and reinstate, make and delete a group, take a person out of one,
        access by op and one person's own access, invitations with
        revoke, who may join, a member's recovery and its approval, and
        the tenant's recovery settings.
      - G12: the guard places all 16 new ops. Records asserted afresh:
        suspend, reinstate, the group and profile writes and
        `revokeInvitation` (`people_reads.rs`), `admission-policy-set`
        (`admission.rs`), `policy-set` and `approveMemberRecovery`'s
        `approved` in the tenant's stream (`recovery_ops.rs`; the second
        had no test at all).
      - Not in the console: **owner succession**, held for its own task
        while the owner reconsiders what it exposes (board
        `console-operator`, ask 19; built since, see "owner succession"
        below);
        **delegations**, which are a person's own (`std.delegation.mine`,
        `revoke`), not a tenant administrator's.
      - Console sign-in (ask 18, the app's recovery rule): after a
        recovery code, the owner's old console passkeys are retired in
        the transaction that enrols the new one, and the sessions they
        opened end. Not before: a code used and an enrolment abandoned
        retires nothing, and the code stays spent. Never leaves an owner
        with no passkey. `ordo console recover` retires the same way, in
        its enrolment.
      - SDK: `src/people.ts` wrappers for every new route.
      - Tests: `ordo-host` `people_reads.rs` 1, `admission.rs` and
        `recovery_ops.rs` extended; `ordo-console` `tests/people.rs` 1
        (every 8b route through the real door check, stood in),
        `tests/console.rs` 2 extended (abandoned enrolment keeps the old
        passkey; completed enrolment refuses it), `server::people` unit 1,
        `g12_guard.rs` placing every op; console e2e `people` project
        extended (18 in all).
      **18o · step 9, built 2026-10-24** (`plan-console.md` section 12,
      step 9). Shares, links, security log:
      - G8, `std.deployment.securityEvents`: a read, owner-only, home
        space. It reads the deployment's own stream (`$deployment`, in the
        control database): newest first, `before` an index for the next
        page or `from` an index for oldest first, at most 200 a page;
        filters `kind` (refused naming every kind), `tenant` (the space
        an entry is about), `since` and `until`. A row is the recorded
        act: index, kind, act, detail, decisions, who, op, tenant, times,
        count. No hash, body or correlation, and no producer writes a
        secret. The filter runs in SQL, then again on each decoded body
        (`PgSecurityStore::try_entries_where`). A tenant's own stream is
        not read here: the owner reads it through `GET
        /security/events?space=` where they hold a role.
      - A console session may now reach a tenant's `std.space.links`,
        `shares`, `revokeLink`, `revokeShare` and `/security/events`, with
        the owner's touch in the last 5 minutes, reads included
        (`console_keys::TENANT_OPS_OPEN`). The door still asks for a role.
      - G12, check then fill: `ordo-console` `tests/g12_guard.rs` lists
        every door op the console's source calls and fails on one not
        placed. A read must be a read in `ordo-host`'s spec. A write names
        its act, the `ordo-host` source that records it and the test that
        asserts the record. Every write already recorded. Three records
        had no test asserting them and now do: `addConsoleKey` and
        `revokeConsoleKey` (`console_key.rs`), `revokeShare`
        (`sharing.rs`). People ops are placed by step 8.
      - The console: `GET /d/<h>/api/shares?tenant` (links with state,
        opens, devices, lock; live shares), `shares/revoke-link` and
        `shares/revoke-share` (audited, both touches), and `GET
        security?source=deployment|tenant|console` (G8, a tenant's record,
        the console's own audit log filtered by action prefix). One
        screen, Shares, with two views: Links and shares, and Security
        log. Tables become one card a row on a phone; a revoke asks to
        confirm in a dialog.
      - Tests: `ordo-host` `deployment_security.rs` 4 against Postgres
        (owner-only, paging both ways, filters, the row's fields and no
        secret) and `security_record` unit 2; `ordo-console` `shares.rs`
        3 against the real door check with the answers stood in
        (`tests/support/shares_door.rs`, shared with
        `examples/e2e_door.rs`), `g12_guard.rs` 3, `server::shares` unit
        2; console e2e `shares` project after `usage` (18 in all).

      **18o · step 10, built 2026-10-24** (`plan-console.md` section 12,
      step 10). Separate images and door-only mode:
      - Three images from `docker/Dockerfile`, sharing every build stage:
        `runtime` (the one image `ordo`, still the default), `host`
        (`ordo-host` alone, one user, no file capabilities) and `console`
        (`ordo-console` alone, door-only). `docker/Dockerfile.host` is
        retired; the `host` target replaced it.
      - The console-only binary is built without the new `supervisor`
        cargo feature (on by default). Then `supervisor/child.rs` (spawn,
        launcher, privilege drop) and `supervisor/run.rs` (the loop) are
        not compiled. `Supervisor` is an empty enum (`absent.rs`), so no
        value can exist. `ORDO_CONSOLE_MODE` defaults to `door-only` there,
        and `supervise` is refused at boot, naming the setting.
      - The image has no `ordo-host`, no file capabilities, one unix user
        (65532), no run folder. Its failsafe is `ordo-console recover`,
        the same code as `ordo console recover`.
      - The Runtime screen in door-only mode is read-only. It shows the
        door's `/health/ready` and the boot record from `/health/deep`
        (G16), and says the console does not manage the process. A
        silent door shows "the deployment is not answering". No door
        address names `ORDO_CONSOLE_DOOR_URL`.
      - `docker/compose.split.yml`: deployment-only and console-only, each
        on its own Postgres cluster, secrets as files.
      - `images.yml`: the `ordo` job builds all three, smokes the one
        image, then runs `ordo-host` and `ordo-console` side by side and
        pushes all three. The separate `host` job is gone. `ci.yml` adds
        clippy and the console tests with `--no-default-features`.
      - Tests: `ordo-console` `door_only.rs` 6 in both builds: a byte
        scan of the binary for the spawn path's strings (present with
        the feature, absent without), a source scan that only the gated
        files can start a process, the mode per build, and the read-only
        Runtime against a stub door, a silent door and no door. Console
        suite 82 with the feature, 68 without. Only CI proves the images
        and their smoke.

      **18o · owner succession, built 2026-10-24** (board
      `owner-succession-what-the-app-developer`, all three asks, ruled
      2026-10-24; `plan-passkey-recovery.md` R5 and ask 8):
      - **Managers told, and may object.** A succession of a tenant, and a
        recovery through the door of a person who owns a tenant (the
        last-owner case, which the plan gives the same rule), reach each
        **manager** of that tenant: `/auth/complete` and `/auth/refresh`
        carry `owner_cases` (case, tenant, path, maturity, paused) until
        the case ends. `std.space.objectToCase {case}` in the tenant, for
        an active manager only (an owner, editor or viewer is refused),
        pauses the case: it does not mature, yields no ticket and cannot
        be completed. `std.identity.resolveObjection {case, decision,
        reason}` (deployment owner, home space): `resume`
        (`evidence-rechecked`, `objector-withdrew`, `objector-answered`)
        or `cancel` (`objection-upheld`, `opened-in-error`,
        `not-verified`). `objected` and `objection-resolved` are recorded
        in the deployment's stream and the tenant's. New table
        `$recoveryobjection`.
      - **Grants end.** A succession names who has left when it opens:
        `departed`, required, each an owner of the tenant then, never the
        successor (`$recovery.departed`). Only those named lose anything;
        a living co-owner keeps theirs. Completing it ends
        each one's live delegations in the tenant and the grant to the app
        each consent minted (`consent::Consents`, shared with
        `std.delegation.revoke`). `endGrants: false` on the completing call
        keeps them; `ORDO_SUCCESSION_ENDS_GRANTS=off` changes the default
        (on). `owner-succeeded` names each one ended, and any not ended.
      - **The console.** Tenants screen, Owner succession: the open cases
        (`std.identity.openCases`, a read) with maturity, approvals,
        departed owners and each objection and its resolution; a form
        taking tenant, the successor's typed id, who has left (typed ids),
        evidence and reference,
        with no member list; resolve and complete in dialogs. Routes
        `GET tenants/successions`, `POST tenants/succession/open`,
        `complete`, `resolve`, each write behind both touches and
        audited. Home space only: `TENANT_OPS_OPEN` is unchanged.
      - Tests: `ordo-host` `recovery_ops.rs` 3 new (notice, editor and
        owner refused, pause, resume, cancel, the last-owner recovery
        paused and resumed, delegations and grants ended and kept, both
        streams), `auth_durability.rs` 1 (objections in Postgres),
        `recovery` unit 1; `ordo-console` `tenants.rs` 1, `g12_guard.rs`
        placing three ops; console e2e `tenants` project extended.
      **18o · step 11, built 2026-10-24** (`plan-console.md` section 12,
      step 11). Ready to launch, except the tester's session:
      - `ORDO_CONSOLE_DATABASE_URL_FILE`: the URL holds a password, so it
        has a file form, read through the one secret reader in `ordo-env`
        (`secret_from`). Both forms set, an unreadable file or an empty one
        refuses the boot, naming the `_FILE` setting. Listed in
        `plan-console.md` 9.4, the `docker/Dockerfile` notes and the
        `ordo console recover` usage. `ordo console recover` takes it too.
      - `docker/compose.split.yml` now reads the URL from a secret file
        (`console-database-url`), not from a variable. Compose applies the
        `uid`, `gid` and `mode` of a file secret only on Swarm; on a plain
        host it bind-mounts the file as it is. So the file's owner (65532)
        and mode (0400) are set on the host file, in the setup steps.
      - `docker/compose.one.yml`: the one image, with the three
        capabilities and the tmpfs it needs, and its own console database
        cluster apart from the deployment's.
      - `docs/howto-console.md`: first run, recovery codes, a second owner,
        `ordo console recover`, linking, the touch, each screen, the
        settings table, one image or two, the door-only statement, backups,
        a separate cluster, and what the console does not defend against.
      - `docs/usability-script-console.md`: 6 tasks (set up, link, deploy,
        pause a tenant, change a setting, recover from a lost passkey),
        what to watch, what counts as a failure. The owner runs the session.
      - Tests: `ordo-console` `config.rs` unit 1 new (file form works;
        both, empty and missing refuse naming the file setting). Console
        suite green. Only CI or a Docker host proves the compose files
        (checked here with a YAML parse only).
      - Left: the usability session itself, then the fixes it finds.
      **18o · identity service step 1, built 2026-10-27**
      (`plan-identity-service.md` section 9, step 1). The server side of
      the `ordo-identity-service` crate, not yet in the image or used by
      the door:
      - Signing keys with rotation, JWKS, 10-minute EdDSA tokens built in
        one function (`token::issue`), sessions with idle and max ends, an
        audit chain with checkpoints, people with the last-admin refusal.
      - Passkeys, recovery codes, invites, sign-in, `/touch`, CLI grants
        (PKCE), machine keys, the provisioner key or first invite at first
        start, host commands `invite`, `break-glass-token`, `rotate-keys`,
        `audit verify`. Guessing limits count in the service's database.
      - The console import (6.1) and the RP ID warning (6.2).
      - Tests: `cargo test -p ordo-identity-service`, most against Postgres.
      - Reviews accepted 2026-10-27; guessing limits count per source
        address plus a shared ceiling (`80fe1776`).
      **18o · identity service step 2, built 2026-10-28** (its page).
      Plain HTML and JS in `crates/ordo-identity-service/page/`, served
      by `src/page.rs`, strict CSP, no-store:
      - `/join` (invite, name, passkey, recovery codes shown once),
        `/sign-in` (passkey or recovery code), `/cli` (approve `ordo login`
        with a fresh touch; code only to the loopback callback), `/account`
        (passkeys, never the last; new recovery codes).
      - Tests: `tests/page.rs`; Playwright with a virtual passkey in
        `crates/ordo-identity-service/e2e/` (CI: `identity-e2e.yml`).
      **18o · identity service step 3, built 2026-10-28** (`25642cc2`).
      The door verifies the service's tokens:
      - `identity_keys.rs`: one client for the service's keys, refreshed
        every 10 minutes and on an unknown key (at most once a minute),
        the last good set kept in `$identitykeys` so the door verifies
        while the service is down.
      - `deployment_access.rs`: the realm table of plan 4.3, before the
        HMAC verifier; denials `403 access_denied`, a stale touch
        `presence_required`, `sub` on every recorded decision; `/auth/*`
        refuses a deployment token.
      - Settings `ORDO_IDENTITY_URL`, `ORDO_IDENTITY_ORIGIN`,
        `ORDO_IDENTITY_DEPLOYMENT`, `ORDO_IDENTITY_PLAIN_HTTP`.
      - Tests: `crates/ordo-host/tests/identity_tokens.rs` (11).
      - Follow-ups (`254a3fb8`): `ORDO_SECURITY_CHECKPOINT_KEY` made at
        first boot when unset, sealed in `$securitykey`, its public half in
        the boot log and `/health/deep`; the token's `kind`, `client`, `sid`
        in the security record under subject tag 2 (old chains verify).
      **18o · identity service steps 4 and 5, built 2026-10-28.**
      - Step 4: the authorization matrix gains 8 actors (fresh and stale
        deployment person, machine key, 5 failing tokens) on every op,
        6,075 to 9,043 cells; the tenant list read from
        `console_keys::TENANT_OPS_OPEN`. It found DEP-1: platform routes
        (health, metrics, alerts, security events, usage) answered a
        stale touch `access_denied`; `serve::refusal` now gives the
        realm's answer on all five.
      - Step 5: `ordo login` (loopback, PKCE, state), `logout`, `whoami`,
        `--touch` (run itself at a terminal on `presence_required`),
        `ordo people` (list, invite, end, machine keys), machine key
        sign-in from `ORDO_MACHINE_KEY` or `_FILE`. Session file
        `~/.config/ordo/sessions.json`, 0600. Tests `crates/ordo-cli/tests/login.rs`.
      - Tenant ops ruled 2026-10-29: deployment admins keep all 35,
        each behind a fresh touch.
      - Step 6, session 1: the console verifies the service's tokens
        (`ordo-console/src/identity.rs`, `IdentityClient`), signs in at
        the service, calls the door as the person (`link::DoorAs`), and
        sends a stale touch through the service. Old console sign-in
        routes answer 410 when `ORDO_IDENTITY_URL` is set. Tests
        `crates/ordo-console/tests/identity_signin.rs` (9 cases).
      - Security review of session 1 (Fable): 2 medium, 3 low, all fixed
        in session 2 with tests. Only the touch accepts a client origin;
        the door refuses console key sign-in and the console refuses to
        boot holding one when `ORDO_IDENTITY_URL` is set
        (`ordo-console forget-door-key`); break-glass tokens get
        `person_only`; `ORDO_IDENTITY_TOKEN_SECS` documented.
      - Step 6, session 2: the service's `GET /audit`, console `admins/*`
        relay, the "Deployment people" screen (admins, invites, end
        access, machine keys, audit with a chain check), console e2e
        `identity.spec.ts` on a stack with the real service (20 cases).
      - Step 7, session 1: `crates/ordo-supervisor` runs the identity
        service (65531), console (65532) and deployment (65533) as
        siblings, with a control socket and a sealed live spec; it fills
        the identity defaults from the console's settings and forgets the
        old console key on upgrade. Image and `compose.one.yml` (3
        persistent volumes, incl. `/var/lib/ordo-identity`) build and boot;
        killing each child brings it back.
      - Step 7, session 2, after a Fable review (2 high, 2 medium, 2 low,
        all fixed with tests): `apply` takes only `ORDO_` names and never
        the identity wiring; a bad sealed spec falls back; stops go
        through a per-user relay, so no KILL capability; the upgrade is
        automatic (Runtime revisions carried over, the console's database
        URL handed on, old compose files honoured). The console serves the
        service at `/identity`; its database is explicit or opted in with
        `ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER`, and with neither the old
        sign-in stays. Rehearsals: upgrade from the old compose file (the
        old passkey signs in) and killing each child, both pass.
      - Step 8 (PR #3, 2026-10-30): with the identity service, the door
        ignores `ORDO_DEPLOYMENT_OWNERS` with a log line and refuses new
        console keys; without it, everything works as before under a
        banner naming the 2 settings. The claim code is removed on every
        deployment (owner: "Remove it now"); first-comer sign-up is the
        per-tenant setting, off by default, and the home space refuses it.
        Linked consoles stay linked; an unlinked one with no identity
        database names the settings.
      - Step 9 (2026-10-30): `docs/howto-identity.md`; first-deployment,
        howto-console, deploying, settings and howto-e2e reviewed and
        fixed; the e2e upgrade case (an old-console passkey signs in after
        the upgrade, job `upgrade` in identity-e2e.yml, retired 2026-10-31
        with the 1-commit public repository); the live-copy
        rehearsal script `scripts/rehearse/live-copy.sh`, passed on a
        localhost stand-in. `ordo login` now reaches `<console>/identity`.
      - No live-copy rehearsal is owed (owner, 2026-10-31): nothing has
        truly deployed, so what runs in smg_infra is replaced or updated
        without care. Step 9 is complete.

- [ ] **18n · Can one control plane serve more than one deployment?**
      Evaluated 2026-09-21 at the owner's request, read-only, against the
      tree. **Not today, and the blocker is not wiring.** There is no
      deployment identifier anywhere (`deployment_id`, `DeploymentId`:
      zero hits in `crates/`). A deployment is defined implicitly by one
      process, one database, one `ORDO_SPACE` and one master key
      (`runtime.rs`'s `assemble_program`).

      **Already keyed by space**, so a second deployment would be
      admissible if space names stayed disjoint: `$commandlog`,
      `$checkpoints`, `$keycustody`, `$secevents`, `$quotas`, `$usage`,
      `$spacedb`, `$spacebucket`, `$chainwitness`, and `members`.

      **Assumes exactly one, with no key at all:** the program artifact
      and activation pointer (`$artifacts` keyed by version, `$activations`
      by seq, so one database is one program); identity (`$credential`,
      `$principal`, `$sessionrevocation` have no space column, so one
      login reaches everything); the access engine, built once per swap
      from one unfiltered `members` read with one deployment-wide profile;
      custody (one master key per process, `$keycustody` loaded whole and
      cached); `$jobs` and `$schedules`; and the deployment-scope security
      chain.

      **The security finding.** Every boundary in Ordo is enforced by
      *space*, never by deployment, so tenant isolation already survives a
      shared process. Deployment isolation does not, and the load-bearing
      line is one check in `apply_op.rs`: the bound space must be the
      deployment's own home. Its own doc says why, and it is the sentence
      to keep: *"being the owner of a space you were given is not being
      the owner of the deployment that serves it"*. Remove the single
      home and every tenant owner can replace the program. Two supporting
      facts: `token.rs` already argues that a shared token key is "one
      authentication boundary shared by strangers", and `ORDO_PAGE_KEY`
      binds the space into its MAC while `ORDO_TOKEN_KEY` cannot, because
      principals and sessions carry no space. That asymmetry is where the
      identity tier stops being keyed.

      **Three shapes weighed:** (a) a process per deployment over a shared
      control-plane database, (b) one process serving many deployments,
      (c) a separate control-plane service deployments register with.
      **Recommendation: build none of them now, build 17aa**, whose three
      pieces are prerequisites for all three and have full value at one
      deployment. If a second deployment is ever needed, take (c), because
      it keeps one process equal to one custody perimeter; (b) gives that
      up, and a process compromise becomes every deployment. (a) is
      defensible only where one operator owns every deployment in the
      database, and it makes per-deployment point-in-time restore
      impossible, since a whole-database restore *is* the control plane
      being restored.

      **Ruled 2026-09-21, after this evaluation.** Serving many
      deployments is a product requirement (the platform is to be hosted
      as a paid service), and the shape is (c): a service **above**
      deployments. One deployment keeps one separately managed master key,
      which forecloses shape (b) permanently. Identities are global and
      minted by that service. A deployment has no concept of it: it is
      handed configuration and told where to live, like a container, and
      never discovers or calls home. The service above is called **the
      platform service** so it stops colliding with 17aa's "control
      plane", which means the system space inside a deployment. The work
      is filed as 18o.

      **Cheap now, while nothing depends on it:** declare space names
      globally unique (three tables already key on space alone and would
      collide silently), declare that two deployments never share a master
      key, and add a deployment id to the four unkeyed tables when each is
      next touched. Also corrected by this evaluation: `lib.rs` documents
      `ORDO_DEPLOYMENT_SPACE` as how a deployment states its own space,
      and nothing reads it; the only caller of `deployment_space()` is a
      test.

- [ ] **17ab · The erasure receipt, and a credential that survives
      rotation.** Two requirements from the owner, 2026-09-21
      (`corpus-rulings.md`, "Erasure proof, and a secret provider the
      service does not know about").

      **1. The receipt.** Erasure proof is a transaction between an
      application and its user; the platform's job is to make it provable
      without being trusted. Ordo cannot prove bytes are gone and neither
      can anyone else, because absence is not observable from outside the
      store. It can prove **unopenability**, which is the claim
      cryptographic erasure actually makes. The pieces exist and are not
      pointed at this: the chain records the destroyed epoch as a custody
      action, the checkpoint signer signs the chain, and
      `ordo-verify-proof` checks a proof offline against a published key.
      Build: an erasure receipt that carries the signed chain entry, the
      epoch destroyed, and the count and location of anything left, in the
      form the offline verifier already reads. The negative test costs
      nothing and should be documented beside it: a user holding a sealed
      value attempts to open it afterwards and fails, which they run
      themselves against the running system.
      Done-when: a space is erased, the application receives a receipt, and
      a third party with only the published verifying key confirms the
      epoch was destroyed at that chain position, without asking the
      deployment anything.

      **2. A credential that survives rotation.** The pool reads its
      password once at boot, so a rotated database credential is an outage
      until a restart. Read through the provider, cache, and **invalidate
      on an authentication failure and retry once**, so a rotation costs at
      most one failed request. The rotator's obligation is the overlap
      (leave the old value valid until the new one is in the mount), which
      is the operator's to arrange and belongs in the documentation rather
      than in the code. Applies to the deployment's own credential and to
      every attached tenant's.
      Done-when: a test rotates a credential under a running deployment
      and the next request succeeds with no restart.

- [x] **17aa · The control plane boots without the deployment under it.**
      The owner, 2026-09-21 (ruling in `corpus-rulings.md`): the system
      space is the control plane, a tenant's state is a fact about that
      tenant, and dependence runs one way. Three pieces of work:

      **1. Move the rewind refusal from the process to the space.**
      *Built 2026-09-21.* `BootRefusal::Rewound` is gone.
      `Spaces::continuity_of` is the one place the continuity question is
      answered, and both the boot and the first request to name a space
      call it. A rewound tenant is quarantined with the same force and the
      same class it was refused with before (`SpaceRefusal::Rewound`, which
      the executor renders as an internal refusal), the resolver holds a
      report-only quarantine register, and `/health/ready` reads it:
      `quarantined` is an array of `{space, reason}` beside
      `program_active`, and a new `continuity` check answers for the
      **control plane's own space alone**, so a tenant never decides
      whether the deployment is ready. Only passing verdicts are cached, so
      a recorded restore still lifts the quarantine with no restart.
      Tests: `a_rewound_space_is_quarantined_and_the_door_stays_up`
      (`ordo-host/tests/runtime.rs`, observed failing first as
      `the binary exited before serving` with the old refusal's text),
      `a_rewound_tenant_is_quarantined_while_the_deployment_keeps_serving`
      (`ordo-host/tests/spaces.rs`, observed failing first on a `null`
      `quarantined` field: the rewound tenant is refused at the door, the
      other tenant is served 200 through the same router, readiness is
      `ready` and names the closed space).

      **2. Startup reconciliation.** *Built 2026-09-21.*
      `runtime::reconcile_at_boot` reads the system space and `ORDO_SPACE`
      through the same resolver every request uses, and prints one line per
      space, with the two lawful remedies under a closed one.
      **What it settles on its own:** a difference somebody already wrote
      down. A recorded restore whose marker the chain reaches is
      `Explained` and the space serves. That is what makes recording a
      restore through the door work, since the marker written into a
      running deployment is the marker the next boot reads.
      **What it will not do:** write or infer a marker, or accept a rewound
      chain because it looks plausible, which is the act the whole restore
      mechanism exists to prevent. It repairs nothing in the database
      either. An unexplained rewind holds the space closed and names what
      the operator must do; a continuity read that *errors* holds it closed
      too, because "I could not check" is not "it checks out".

      **3. A tenant comes into being through an apply.** Not by writing
      custody into a database beforehand. Keeps the typo protection,
      because the tenant's name is in a program someone deployed rather
      than in a shell command, and removes the last reason the CLI needs
      the database on day zero. **Still open, and it needs a ruling before
      it needs code** (found 2026-09-21 while landing 1 and 2):

      - **Nothing in a program can name a space, and three layers refuse
        it on purpose.** `space` is a reserved keyword with no parser arm
        and no AST node (`ordo-syntax/src/token.rs:134`,
        `lexer.rs:819`); `ordo-sema` refuses an op that declares a
        `$space` parameter, because "a parameter by that name would be a
        second space the caller controls"
        (`ordo-sema/src/lib.rs:143`); and the artifact plans the space as
        slot `$1` rather than a value, because "a folded space literal
        here would be a compiled-in tenant, and a deploy diff that changed
        when the deployment gained a customer"
        (`ordo-plan/src/artifact.rs:482-489`). That last one is the
        sharpest: a program carrying a tenant's name changes its artifact
        hash per tenant, which is the opposite of one artifact served in
        every space.
      - **And a settled design already answers the same question the other
        way.** `bootstrap.md` §1 makes `std.space.create` the one surface
        that brings a tenant into being: a `scope deployment` op binding no
        space, seven phases, a 46-item inventory in which the `slug`, the
        `displayName` and the owner are **caller-supplied** and the
        `spaceId` is platform-minted. `definition-language.md`'s
        `bootstrap` block declares *root rows only*: "a bootstrap block
        declares rows, never grants, never principals, never memberships —
        the IaC boundary is unmoved". So the ruling and the settled design
        disagree about where a tenant's name lives, and that is the
        owner's to settle before anything is built.
      - **A tenant needs an owner, and a program cannot name one.** The
        genesis group mints exactly one `role: owner` row, and a space with
        no owner is the one state `bootstrap.md` §0 says nothing can repair
        from inside. An apply that minted a tenant would have to take the
        owner from the applying principal, which is a different rule from
        the one `std.space.create` uses and needs saying out loud.
      - **The typo property the ruling wants is already held elsewhere.**
        `LocalKeyProvider::create_space` refuses to mint for an unknown
        space on a read path, ids are server-minted, and preflight rejects
        an unknown slug before any row exists. So "a typo cannot mint a
        tenant" is not by itself an argument for putting the name in a
        program.
      - **A tenant an apply created is not reachable until the next boot.**
        `provision_space`'s own doc says refreshing a *live* access engine
        is deliberately not attempted, because an engine swapped under an
        in-flight request is the half-swap `deploy` exists to describe. The
        engine is built from `members` at boot, so this is real work in its
        own right and it is the same work `std.space.create` needs.
      - **Day zero has one refusal left of the shape 17aa removed.**
        `build_custody` returns `BootRefusal::UnprovisionedTenant` when
        `ORDO_SPACE` names a tenant with no custody
        (`runtime.rs:1868`), which is again a process-wide refusal over a
        tenant's state. Its own text already names the way round it (point
        `ORDO_SPACE` at the system space), so a deployment can boot with no
        tenant today. Whether that refusal should survive 17aa's rule at
        all is worth asking in the same breath as piece 3.

      **What is already in place, so the estimate is honest.**
      `provision::provision_space` is built and tested and has **no
      production caller at all**: `ensure_system_space` is the only thing
      that reaches it, for the platform's own home. `apply_op::sequence`
      (`apply_op.rs:462`) is the natural seam, because it is where the
      first apply creates `members` and it is the structural twin of
      `runtime.rs:2362`, which already calls `ensure_system_space` the
      moment `members` exists. The one missing ingredient on the door path
      is custody: `ApplyOps` is `{ pool, home }` and holds no
      `KeyProvider`, while `PlatformWiring` already carries one at the
      construction site (`runtime.rs:919`, `:998`). `scope deployment` has
      no implementation anywhere, which is what
      `critique-consolidated-4.md` AA-007 says: `OpSpec`, the router,
      `AccessChecker`, `CapabilityGrant` and replay keys all structurally
      require a bound space.

      **Smallest honest next step:** ask the owner whether piece 3
      supersedes `std.space.create` or wraps it. If it wraps it,
      `std.space.create` and `scope deployment` are the prerequisite rather
      than the alternative, and piece 3 is the apply path calling a surface
      that does not exist yet.

      This supersedes 17z's scenarios 1 and 2, which asked how those two
      acts reach a door that is not there; the answer is that the door is
      there, because the control plane no longer falls over with a tenant.

      **Piece 3 unblocked 2026-09-21 by the id ruling** ("A tenant is an
      id, and its name is an attribute", `corpus-rulings.md`). The
      contradiction dissolves: a program cannot name a tenant, and now
      nothing wants to, because there is no name to compile in. A tenant
      is created by an owner-gated call that mints a globally unique
      opaque id and records the caller's chosen name as an attribute
      beside it. `provision_space` is the groundwork (built, tested, no
      production caller); `bootstrap.md` §1's `std.space.create` is the
      shape, with its slug and name demoted from identity to attributes.
      Done-when: a deployment with a program can mint a tenant through the
      door, the tenant serves, and nothing anywhere keys on its name.

      **Piece 3 built 2026-09-21.** `std.space.create`
      (`crates/ordo-host/src/tenant_ops.rs`), on `std.deployment.apply`'s
      gate exactly: owner through `owner_only`, step-up so no capability
      token reaches it, and the bound space must be the deployment's own,
      because the first two are deployment-wide policy installed in every
      space and a *tenant's* owner passes both. The caller supplies a
      `name` and an `owner`; the platform mints the id. `$spaces`
      (`crates/ordo-host/src/tenants.rs`) holds the name beside the id in
      the **control** database, so a quarantined tenant is still an id an
      operator can put a customer to. Seven tests in
      `tests/tenant_create.rs`, four of them the gate.

      **The id is `spc_` plus 26 Crockford base32 characters.** 128 bits,
      because "globally unique" was the ruling's word and a sequence is
      unique only to the database that issued it, which is one restore
      away from two tenants sharing an id. Crockford rather than hex
      because the alphabet drops `i`, `l`, `o` and `u`, and an id gets
      read aloud and typed back from a screenshot.
      `ordo_api::well_formed_space` binds it with no new rule.

      **What building it found, and it would have shipped silently.**
      `engine_from_members` builds the door's memberships with
      `members.id` as the **`PrincipalId`**, ignoring the `principal`
      column beside it. The first version minted a member id the way it
      mints a space id, which would have produced a tenant reachable only
      by an identifier nobody holds a token for, and an owner with no
      membership in the space created for them — `bootstrap.md` §0's one
      unrepairable state. The member id is the owner principal, and
      `a_minted_tenant_answers_to_its_owner` pins it by rebuilding the
      engine from the real rows rather than asserting on the row it wrote.

      **A minted tenant is reachable without a restart.** The access
      engine is built from `members` once at boot and `directory.rs` says
      refreshing a live one is not attempted, so a mint would otherwise
      *schedule* a tenant rather than create one. The create announces
      itself on a third channel, `ordo.members`
      (`ordo_exec::jobs::MEMBERS_CHANNEL`), and every replica runs the
      `refresh_access` it already runs after a deploy. Announced rather
      than rebuilt inline: a rebuild on the answering node alone leaves
      the rest refusing the tenant, which is worse than neither because
      the failure then depends on which replica answered. The op's answer
      carries `reachable`, so a caller is told *now* or *next restart*
      rather than assuming.

      **One new error class, `tenant_refused` (400).** `invalid_param` is
      the door's word and `executor_raisable` refuses it, which the tests
      proved by answering 500 to a blank name. The rules that fire here
      are only discoverable below the door — an `owner` becomes the
      founding member id and must be addressable, a blank `name` is the
      one attribute an unreadable id was meant to be picked out by — which
      is `invalid_cursor`'s own argument for being raisable there. One
      code for the set on `apply_refused`'s reasoning, documented in
      `api-engine-design.md`'s inventory.

      **The CLI verb landed 2026-09-21: `ordo space create`.** The name
      is not invented: `bootstrap.md` §6.1 has spelled this surface that
      way since before the op existed. It mirrors `apply --host` for host
      resolution, bearer token, `--space` binding and refusal rendering,
      and the refusal renderer was factored out so the two commands print
      the door's envelope through one function rather than two copies.
      Eight tests in `ordo-cli/tests/wire_space_create.rs`, including both
      `tenant_refused` cases proved distinguishable by their printed
      message, which is what makes that one wire code honest.

      **One departure from `apply`, and it is the right one.**
      `apply --host` falls back to a direct-database apply when no host is
      given, because it had a database path before 18l gave it a door.
      This command never had one, so its no-host arm is a usage error
      rather than a fallback, and there is no `--db` flag at all. The
      message says why: minting a tenant mints custody, writes a founding
      owner **and** tells every replica to rebuild its access engine, and
      a tool holding a database password could do the first two and not
      the third.

      **A claim in that message had to be corrected before it shipped.**
      The first draft told the operator the act happened *"under the same
      lock every replica rebuilds its access engine under"*. There is no
      such lock: the mint announces on `ordo.members` and each replica
      rebuilds on its own. A user-facing sentence describing a mechanism
      that does not exist is worse than no sentence, because it is the
      thing someone reasons from at three in the morning.

      **Still owed:** the security-event record `provision.rs`' module doc
      says a creation owes now that it finally has an authenticated
      creator. `bootstrap.md` §6.1 also spells `--dry-run`, which this
      verb does not have; filed here rather than silently dropped.

- [x] **17ac · The last process-wide refusal over a tenant's state. DONE 2026-09-21.**
      Raised by 17aa piece 3, 2026-09-21. `build_custody` returns
      `BootRefusal::UnprovisionedTenant` when `ORDO_SPACE` names a tenant
      with no custody (`runtime.rs`), which is the same shape 17aa removed
      one layer up: a deployment refusing to start over one space's state.
      Its own text already names the way round it — point `ORDO_SPACE` at
      the system space — so it protects nothing a quarantine would not
      protect better, and 17v plus piece 3 mean a deployment no longer
      needs a tenant to have been arranged before it boots.
      **Replaced with the quarantine register**, the same way
      `BootRefusal::Rewound` was. `Reconciled::Unprovisioned` is the new
      verdict, `Spaces::reconcile` asks the custody question before the
      continuity one (a space with no custody has no chain to ask), the
      register carries the remedy as its reason, and the boot log gives it
      its own arm because telling an operator to record a restore for a
      tenant that never existed sends them looking for a backup that does
      not exist. `BootRefusal::UnprovisionedTenant` is gone.

      **Two things it found that were not in the recommendation.**

      1. **`/health/ready` had the same defect one layer up.** Its
         `custody` check probed `ORDO_SPACE`, so the first run of the new
         test came up, quarantined the space correctly, and then answered
         **503** — a load balancer would have pulled the deployment out of
         rotation because one tenant was not provisioned. The check now
         probes the **system space**, which the boot creates
         unconditionally and which is the one space whose custody failing
         genuinely means the deployment can serve nobody. That is exactly
         the split `continuity` already had: the check answers for the
         control plane, the register answers for tenants.
      2. **The master-key probe was narrower than it read.** It probed
         `ORDO_SPACE` alone, so a deployment whose tenant was not
         provisioned never verified its master key at all — and that is
         now the ordinary first-boot state rather than an error. It probes
         the system space always, and the configured space too when that
         space has custody of its own.

      The typo property is unchanged and is asserted in the new test:
      `current_epoch` reads and never creates, so `$keycustody` still holds
      zero rows for a mistyped `ORDO_SPACE`.
      `an_unprovisioned_tenant_is_quarantined_and_the_deployment_serves`
      (`tests/runtime.rs`) replaces
      `an_unprovisioned_space_is_refused_with_its_remedy`. `deploying.md`
      corrected in two places, one of which printed the refusal verbatim.

- [x] **17af · The tutorial teaches a declaration the compiler cannot
      parse. FIXED 2026-09-27.** Found 2026-09-27 re-running the consolidated critique across
      the documentation (`critique-consolidated-5.md`, W-001).

      `tutorial.md` §13 introduces a `routine` block as ordinary syntax and
      spends a section on what is right and wrong about it. `ordo check` on
      that block answers:

      ```
      parse error [parse_error]: expected a declaration
          (`domain`, `entity`, `op`), found `routine`
      ```

      **The refusal is the defect, not the absence.** 12a ruled the
      declaration form **not owed** and that ruling stands: §7 and §8 spell
      the same fact two ways, and building either would decide it by
      accident. What the ruling did not carry was a pass over the documents
      that teach the spelling it declined to pick. The error names three
      declarations and stops, so it reads as a typo or a stale binary, and a
      reader who has followed twelve sections correctly concludes they
      mistyped something the document told them to write. `grep -c "not
      yet\|unbuilt\|does not compile" docs/tutorial.md` is **0**.

      Taught as working syntax in `tutorial.md`, `syntax-examples.md`,
      `modeling-guidance.md`, `traps.md`, `definition-language.md` and
      `guards-conformance.md`.

      **Two independent fixes.** (1) Mark the sites, in the form the corpus
      already uses for lean material. (2) Make the refusal name the state: the
      lexer knows `routine` is a keyword, and it is the parser that has
      nowhere to send it, so a word the grammar reserves and does not
      implement can say so instead of listing three alternatives.

      The second is the one that generalises. Every other lean declaration
      (`principal`, `client`, `policy`, `channel`, `route`, `app`, `page`)
      will fail the same way the day somebody writes one.

      **Both built.** `UNBUILT_DECLARATIONS` in `declparse.rs` covers all nine
      words with a reason and a checklist id each, on `FOREIGN_SPELLINGS`'
      stated argument that a wrong spelling must fail *saying so*. The four
      documents carry notices separating the settled semantics from the
      unsettled spelling.

      **The build found why this survived four reviews.** The fix was first
      written into the `Ident` arm, because a check reported these words as
      identifiers. They are all `Keyword`s, and `describe` renders a keyword
      in exactly the format the `Ident` arm uses for a word — so both arms
      emitted **byte-identical text** and no amount of reading the output
      could say which had answered. The first fix compiled, looked right and
      did nothing; `a_reserved_but_unbuilt_declaration_says_so` caught it, and
      was itself falsified by removing the call from the keyword arm.

- [ ] **17ag · Ten security findings with no trace.**
      `audit-security-trace.md`, 2026-09-27.

      The 31 `SH-n` findings from the four hardening reviews were traced to
      the tree: **13 closed with a named file, 8 homed on a checklist item and
      unbuilt, 10 with no trail at all.** The reviews were last touched
      2026-08-10 and only `SH-17` and `SH-20` appeared anywhere in this file,
      so the X-series had a disposition trail and the SH-series had none.

      **Untraced:** SH-2 (feed copies re-index under the receiver's salt),
      SH-7 (the mirror has no storage pins), SH-11 (KeyProvider compromise
      blast radius), SH-12 (the envelope as a behavioural side channel),
      SH-14 (correlation ids across spaces, revert planning unscoped),
      SH-18 (every token is a bearer token, nothing binds to a device),
      SH-22 (profile templates bound nothing), SH-24 (the catalog treats
      entity and field names as public-to-the-token), SH-29 (`AccessProfile`
      has no per-member overrides).

      Several are probably answered by work done since August — SH-18 by
      20b's durable credentials, SH-14 by per-space chains and a scoped
      `revert --group` — and *probably* is the whole problem. Each needs what
      the thirteen got: a named file or an honest "nothing".

      **Not the same as a security review.** This traced findings to
      mechanisms. It did not read the system adversarially, and
      `critique-consolidated.md` §3c, the security section of the merged
      expert reviews, is covered by neither this nor round 5.

- [x] **17ae · What the first deployed program is. RULED 2026-09-28: a notes
      app with passkey sign-in.** Filed 2026-09-27, and
      it is the gate everything else on the path to a first deployment is
      waiting behind rather than one more item beside them.
      **Ran once, about 2026-10-05 (owner, 2026-10-19):** "We ran it once
      successfully a couple weeks ago. This was before using the Ordo UI base
      components." Under the definition ruled 2026-10-19 it is not deployed
      today: the page at notes.inf.smg.icu does not resolve. Next is notes v2,
      deployed and kept (end-to-end-tests-through-the-notes-app).

      **The mechanism is finished and the runbook is written** (`deploying.md`
      §4c): boot on an empty database, custody minted at boot, then
      `ordo apply` over the door and `ordo keys init` per tenant. The
      instance at `ordo.inf.smg.icu` has been up since 2026-09-20 with no
      program. So what is missing is not a capability.

      **Why it decides the rest of this section.** Which of the remaining
      items are *gates* is a function of what the first program contains:
      migrated data makes 11-10a's verify pass a gate; background rules make
      12a's declaration spelling one; a browser UI makes `14g`'s CORS one;
      attachments were a gate until 2026-09-26 and are not now. Picking the
      program is what turns this section from a list into an order.

      **Recommendation, filed on the board as `first-program`:** something
      the owner actually uses, small enough to sit inside the surface that is
      built, and **with guards and more than one member**. A single-user
      unguarded program would deploy today and would leave the authorization
      engine unexercised in production, which is the part the whole argument
      rests on. `examples/first-app` proves the path end to end and proves
      almost nothing else, by construction.

      **Ruled.** The owner: *"a note taking app that stores notes. You type
      a note, it stores it and makes sure theres users that can login with
      a passkey."* Notes readable by their author is a real guard over more
      than one member, which is what the recommendation asked for.

      **What the ruling makes blocking**, by this item's own argument:

      1. **Passkey sign-in is not served.** `ordo-identity` has the passkey
         code and its tests, and `auth_from_env` refuses any method but
         `password`: *"passkey needs the browser ceremony and a relying-party
         id"*. First in the queue.
      2. **`14g` CORS**, ruled the same evening: a configurable allowlist.

      Not gates for this program: the import verify pass, the trigger
      declaration, attachments.

      **Next, agreed 2026-10-06:** after the current batch's audit, one
      go-live batch and no new features until the notes app is live:
      refresh `examples/notes` against main, fix 25e and `created = now`,
      cut a fresh image, and write a one-page checklist for N6 and N7
      (`examples/notes/README.md` §8), which the owner runs.
      **Before the image, ruled 2026-10-06 (first-program asks 7 to 10):**
      the share-link miss limit a setting, default off (built 8ec3796c);
      the SDK retries `server_busy` (built 7824609b) with timing as options
      that grow; `ORDO_PG_ROUTING` removed, since every tenant always gets
      its own database on the default tenant cluster. The proxy mode moved
      after the first deployment (28a), and the limits that read a client's
      address are removed until it lands.

- [ ] **17ad · A test that fails leaks a server, and a leaked server is a
      false pass waiting to happen.** Found 2026-09-23 by sweeping for
      orphaned processes: `ordo-host` PID 3492, parented to init, serving
      `/health/live` on 127.0.0.1:49465 for **32 hours**, from a test run
      on 2026-09-21.

      **It is not a missing `kill`.** `crates/ordo-host/tests/runtime.rs`
      spawns the real binary sixteen times and calls `kill` sixteen times.
      Every one of those kills is on the happy path, so an assertion that
      fails between the spawn and the kill unwinds straight past it and
      the child is reparented to init. Given how often those suites failed
      while the 18d work was landing, 3492 is unlikely to have been the
      only one ever created; it is the one that survived long enough to be
      noticed.

      **Root cause found 2026-09-25, and it was one line.**
      `PoolConfig::from_env()` did not read `ORDO_PG_POOL_SIZE`. It read six
      environment variables and computed the seventh as
      `min(cores, 8)`, unconditionally, while `runtime.rs` read the
      variable separately — so a booting deployment honoured it and **every
      test that built a pool directly did not**, dialling eight apiece
      whatever the environment said. That is why sizing the spawned servers
      down worked and setting the same variable for the whole run did
      nothing: the two paths read different things.

      Measured on a 10-core machine: `runtime.rs` peaked at **99**, the
      whole workspace at **81**; after the fix the whole workspace peaks at
      **25** against Postgres's stock 100. `scripts/conformance-pg.sh` is
      back to stock, CI needs no change, and the `ci-max-connections` row
      is closed. `.cargo/config.toml` sets the variable to 2 for anything
      cargo runs. Explained with diagrams in `docs/pooling-explained.html`.

      **Raising `max_connections` to 300 was the first move and was
      wrong** — it would have worked and left the defect in place with the
      ceiling moved. The question that found it was the owner's: *"Why are
      we testing that many connections? Is it for stress testing? We do not
      need to stress test everytime."* Nothing was testing them.

      **Also done: three stores now share one connection.** The keystore,
      the credential store and the principal directory each dialled a pool
      of their own, and each is serialised through a single job loop, so
      between them they could never use more than one at a time. A
      deployment holds four connections instead of six. They may not share
      the **request** pool, and that is a rule rather than a preference: a
      request holds a `TxConn` and reaches the keystore while holding it
      (`ordo_exec::sealed`), so drawing from the same pool would be a task
      holding one permit and waiting for a second from a pool of N. The
      trade taken is that the three now serialise against each other.

      **Superseded 2026-09-24 note, kept for the record:** *"How often those suites
      failed"* was taken as evidence of a leak feeding itself. The measured
      cause is that **`runtime.rs` peaks at 99 client backends on its own**
      against Postgres's default `max_connections = 100`. Cargo runs test
      binaries one at a time, so this is not suites overlapping: it is one
      suite whose ~22 tests run in parallel and each stand up a real
      deployment with several pools. Sitting exactly on the ceiling is why
      it failed a different subset almost every run and why
      `--test-threads=1` always passed.

      `scripts/conformance-pg.sh` now starts at 300 and the whole workspace
      is clean. Put back to 100 on a freshly restarted cluster, `runtime`
      fails four tests, every one of them `FATAL: sorry, too many clients
      already`.

      **Two corrections to what was said earlier today**, both from
      guessing at a mechanism before measuring it. The first claim was that
      a workspace run starts every test binary at once; it does not. The
      second was that this let 17ad off — it does not. A leaked server
      holds its pool open until something kills it, so on a suite with no
      headroom every leak is one step closer to the ceiling, and the
      restart that came with raising the limit is what cleared the ones
      that had accumulated. 18d R-S5 also added one connection per attach.
      Both are contributors to a ceiling that was already being touched.

      **This item stays open and its substance is unchanged.** A test that
      fails between the spawn and the kill still leaks a server, the `Drop`
      guard is still the fix, and a leaked listener can still turn
      `free_port()` into a false pass. **Reproduced twice on 2026-09-24**,
      both parented to init, both from runs earlier in the same session:
      PID 16652 after 4h56m, still holding its listening port, and PID
      54640 after 55m. Killed by hand, again. Two in one day is the rate
      this happens at while the suites are being exercised, which is the
      argument for the guard that the flakiness was wrongly used to make.

      **Why it is worse than a wasted process.** The leak holds a listening
      port, and `free_port()` hands out whatever the OS says is free. A
      later test handed a port a **stale binary** is already serving would
      talk to that binary instead of the one it thought it started: a
      deployment from before 17aa, 17ac and `std.space.create`, answering
      confidently. That is a false pass, and it is the kind that looks
      like flakiness rather than like a bug. It costs nothing in CPU (0.0%
      and 6.5 MB, holding no database connections), which is exactly why
      it went unnoticed for a day and a half.

      **The fix is a `Drop` guard on the spawn handle** so unwinding kills
      the child, rather than sixteen more explicit kills that the next
      panicking assertion also skips. A sweep at suite start is the weaker
      alternative: it cleans up after the damage rather than preventing
      it, and it cannot tell one run's leak from another's.

      **Until then:** `ps -eo pid,ppid,command | awk '$2==1 && /ordo-host/'`
      finds them, and they are always safe to kill. The
      `cleanup-rust-repo` skill already refuses to run while a build is
      live, which is the neighbouring guard rather than this one.

- [ ] **17z · Door-only: the scenarios it does not cover.** The owner,
      2026-09-20: *"We should only work around the door. System is the
      source of truth and errors. If someone needs to break glass they can
      just work in the database."* So the CLI speaks to the door and
      nothing else, and break-glass is `psql`, not a CLI mode. That is a
      cleaner rule than the break-glass-in-the-CLI version it replaces, and
      it leaves these uncovered. Each needs an answer before the database
      path is removed, not after.

      1. **Day zero has no door.** `ordo keys seal-master` writes a file
         and `ordo keys init` writes custody to the database, and the
         binary refuses to boot without custody for its space. So the first
         tenant cannot be created by a tool that only speaks to a door that
         cannot exist yet. The system space is handled (17v creates it at
         boot); a tenant is not. Likeliest answer: the server provisions a
         tenant on an explicit, audited request rather than a laptop
         writing rows, which also keeps the rule that a typo cannot mint a
         tenant.
      2. **The restore recovery is circular.** *Answered and built,
         2026-09-21 (17aa piece 1).* `assemble` refused to boot on an
         unexplained rewind (`BootRefusal::Rewound`, `runtime.rs`), and the
         command that explains it reaches the deployment. So the
         deployment would not start until the marker existed, and the
         marker could not be written without a deployment.
         The answer was not the one guessed here. Rather than a one-shot
         boot flag, the refusal moved from the process to the space: a
         rewound tenant is quarantined and the deployment keeps serving,
         so the door `ordo restore --record` speaks to is up while the
         space it names is closed. The circle is gone rather than worked
         around, and no new operator-set variable exists to be left set.
      3. **Verifying a server you do not trust.** Asking the door whether
         its own chain is intact is worth nothing when the server is what
         you suspect. `ordo-verify-proof` is the offline answer and needs a
         proof it can read. Keep a path to that proof that does not require
         the suspect to cooperate, and document it as the answer to this
         question rather than as a feature.
      4. **Operator lockout.** One owner credential, lost or compromised,
         and door-only means no way back in. The break-glass grant (11-26)
         covers a caller inside the model; it does not cover nobody being
         able to call at all.
      5. **Long operations through a proxy.** `apply`, `migrate --apply`
         and especially `import` can outlast an idle timeout. This
         deployment sits behind Cloudflare, which cuts long requests. A
         synchronous door call is the wrong shape for a job that takes
         minutes: it wants a job id and a poll, or a stream with progress.
      6. **Large payloads.** An import bundle is not a JSON body. It needs
         chunked upload with resume, which the replay lease can make
         idempotent, rather than one request holding gigabytes.
      7. **Version skew between CLI and server.** The refusal class exists
         and nothing tests it. Door-only makes it routine: a laptop's CLI
         will often be older or newer than the deployment.
      8. **A tenant with its own database.** Which pool a door command
         acts on stops being obvious once a space is promoted, for apply,
         import and usage alike.
      9. **Locking yourself out with your own limits.** Quotas and the
         per-space pool cap must never apply to the operator surface, or
         an over-quota deployment refuses the call that would fix it.

      **The owner's answer to break-glass, same day:** *"System UI can
      provide a url string that can be used with credentials for break
      glass. But manual intervention could break things so should only
      exist to fix things."* So break-glass is a **connection string the
      system hands out**, not a mode of the CLI and not a credential an
      operator keeps. That shape carries its own obligations, which belong
      in whatever builds it: the string is minted deliberately and
      recorded when it is minted, it is scoped and short-lived rather than
      standing, it says plainly that using it can break things, and the
      deployment can tell afterwards that someone did. A direct database
      session bypasses seals, the chain and every guard, so the honest
      framing is a fire axe behind glass: available, logged, and never the
      ordinary way to do anything. Scenarios 2, 3 and 4 above are the ones
      it answers; 1 and 5 to 9 are not break-glass questions and still
      need their own answers.

      **Scenario 2 is closed** by 17aa piece 1 (2026-09-21), which dissolved
      the circularity rather than reaching for break-glass. **Scenario 1
      stands open** and is 17aa piece 3: a tenant comes into being through
      an apply. Day zero still has no door for a tenant, though it has one
      for everything else, because the control plane now boots on its own
      custody and its own tables whatever state a tenant's data is in.

- [ ] **17y · The CLI audit of 2026-09-20: what is promised, what ships,
      and where the docs lie.** A full audit of every command, subcommand
      and flag against the dispatch in `crates/ordo-cli/src/main.rs`. Most
      of the surface is built and correct. What it found, in the order
      that hurts:

      1. ~~**`ordo import` has no CLI verb.**~~ **Closed 2026-09-26**
         (`plan-import.md` §11). The finding's diagnosis was right and its
         suggested home was right: the engine was built, the operator tool
         could not call it because it has no custody and an import that
         cannot seal cannot write a sealed field, and *the wire is the
         likelier home now that `apply` has a door*. Built there:
         `std.space.import` plus `ordo import`, door-only, with no database
         mode at all rather than one that would need a key. Finding 2's
         ordering said `import` first, and it went first.
      2. **Only `apply` reaches a deployment over HTTP.** Every other
         operator act still wants Postgres credentials on a laptop, which
         is what 17u set out to remove. **Ruled 2026-09-20:** the CLI
         triggers actions on the server, so the door is the default for
         every command that acts on a deployment and the database path
         becomes break-glass, named as such, for the acts that must work
         when the door is down (custody init, chain verification, revert).
         `import` first, then `rollback`, `deploys`, `migrate --apply`,
         `quota`, `usage`, `restore --record`. **Not started** (board
         row `cli-door-table`). A per-command table begun in `wire.rs`
         on 2026-09-20 was never compiled and is kept at the tag
         `salvage/cli-door-table`; its branch was deleted 2026-09-29.
      3. **`migration.md` reads as a runbook for two commands that do not
         exist** (`ordo introspect`, `ordo import`). Corrected 2026-09-20
         to say what exists; the steps stay as the shape of the pipeline.
      4. **A trap in the host flag.** `wire::requested` reads `ORDO_HOST`
         unconditionally, so an operator who exports it and then runs a
         documented database invocation (`--db … --as …`) gets a confusing
         usage refusal, because the environment silently selected the
         door. `--space` and `--token` are accepted and ignored on the
         database path.
      5. **A third of the shipped surface is missing from `ordo`'s own
         usage text**: `quota set|show`, `usage`, `keys status|rotate|
         reseal|destroy`, `verify --proof`, `verify --head`. A user who
         runs `ordo` with no arguments cannot discover it.
      6. **`ordo dev --gen` is a silent no-op without `--out`**
         (`dev.rs`), and the usage text does not say so.
      7. **The plan docs understate what ships, by seven commands**, so
         nobody can use them to decide what to build next. Two were
         corrected on 2026-09-20 (`plan-production-ops.md`'s "nothing of
         M1 to M3 is built", `deploying.md`'s stale reset claim); the
         rest are listed in the audit.
      8. **`ordo fmt` is promised under two spellings in two documents**
         (`ordo fmt`, `ordoc fmt`) and exists as neither.

      Corrected already on 2026-09-20: the three stale passages above and
      `plan-schema-evolution.md`'s `revert --correlation`, which is
      `--group`. The rest is open.

- [ ] **17x · Usability testing plan (automated + human).** `plan-usability-testing.md`.
      The quickstart (quickstart.md) describes a happy path; this plan measures
      whether a developer can actually walk it. Two halves: an automated suite that
      walks the path on every commit, asserting at each step that printed output
      shows what to do next and things work; a human protocol where a developer
      new to Ordo walks the same path with a moderator recording where they pause,
      go off-path, or need help. First-five-minutes checkpoint: time from "nothing"
      to a callable op with a printed token, targeting under 90 seconds. Increments
      from scaffolding the tests through running five pilot sessions and acting on
      findings. Intended to catch the wall the quickstart author ran into: `ordo dev`
      was silent on the token, invisible to 1,231 tests.

Deliberately NOT here: packages (deferred by decision), the vault/F7
program (its own plan), LSP rungs beyond the grammar (14e's ladder), the
operator dashboard (unruled), `ordoc fmt` (rides the CDK's synth work).

---

## 18 · Post-deployment roadmap (owner-ruled, 2026-09-08)

Items the owner has explicitly placed AFTER the first deployment. Listed so
"later" has an address and cannot silently mean "never".

- [ ] **18a · The full egress rail.** The owner: "add it for a future
      post-deployment roadmap." One reconciliation, recorded: the same
      batch of answers said "execute" attestation, whose webhook is an
      outbound POST — and 5f's note stands that shipping that POST without
      §7b's pins is the SSRF path §7b closes. So the **egress kernel**
      (one client, six pins) ships NOW inside the attestation work as its
      first consumer — exactly the sequencing already ruled — and what is
      post-deploy is the *rail around it*: the per-space consented
      allowlist surface, notification sinks, feed delivery, import
      bridges, connector integrations.
- [ ] **18c · Per-space encryption at rest. Owner-ruled 2026-09-12:**
      "I'd rather databases be encrypted, whole space encrypted at rest
      and usable / searchable / indexable when in memory", and "one
      tenant, one key", with the topology named as **one database per
      tenant inside one Postgres instance** rather than one instance per
      tenant.

      **The in-memory half is not the problem.** Volume encryption already
      gives a whole encrypted `PGDATA` that decrypts into page cache, with
      every index working normally. What it cannot do is key per database:
      a volume covers the cluster, so one instance is one key. Per-instance
      customer-managed keys on RDS have the same limit for the same reason.

      **The topology picks the tool, and it is `pg_tde`.** Percona's
      extension is the only thing that gives one principal key per
      database inside one instance, KMS-backed (Vault, OpenBao, KMIP) and
      rotatable, and it encrypts indexes as well as heap, which is what
      makes an unsealed sort column safe on disk. Two costs, both real.
      It is an extension hooking the storage manager, so it is
      unavailable on managed Postgres, and Ordo currently requires **zero
      extensions**, which is what lets it run on RDS, Aurora, Neon and
      Supabase. And its WAL encryption is beta and explicitly not
      production ready (v1.0 was 2025-06-30).

      **The structural limit no extension can fix: the WAL is
      cluster-wide.** One instance is one write-ahead stream carrying
      every tenant's writes, and it feeds every standby and every
      point-in-time archive. So in this topology, per-tenant keys cover
      the durable tables and their indexes and can never cover the
      stream. Application-level sealing is the only thing that protects a
      value there, because a sealed value is ciphertext before Postgres
      ever sees it. That is the sharpest argument for sealing the corpus
      has, and this topology is what creates it.

      **Scale correction, 2026-09-12.** The owner then gave the target
      size: **tens of thousands of spaces per deployment**. That rules out
      a database per tenant as the default, and with it `pg_tde` as a
      general answer. Ten thousand databases is roughly 75 GB of system
      catalog before any data, an autovacuum launcher with three workers
      cycling all of them, and migrations measured in hours. Connections
      are **not** the binding constraint: a transaction-mode pooler on the
      cluster host opens backends only for concurrently active tenants.

      **What survives at that scale is what is already built**:
      application-level sealing gives one key per tenant across tens of
      thousands of them with no storage overhead, because the key boundary
      is in the data rather than in the storage. Crypto-erasure stays per
      tenant. Cluster-wide volume encryption then covers the plaintext
      skeleton, and one key for the cluster is no longer a defect because
      the per-tenant boundary is handled above it.

      **A dedicated database becomes an enterprise tier**, reached by the
      routing in **18d**, not a default everyone pays for.

      **What is owed here**: the deployment contract ("a space's storage
      is encrypted at rest under a key scoped no wider than the space"),
      what `production_ready` does when it cannot be established, and the
      disclosure an author gets when unsealing a field for sorting.

- [ ] **18d · Database per space, and the BYO-DB custody choice.**

      **Correction, 2026-09-12: database-per-tenant is not implemented,
      and the phrase "BYO-DB is already the deployment model"
      (`adoption-dx-plan.md`) does not mean it.** That sentence is about
      pointing a deployment at your own Postgres rather than a hosted
      one. What exists is a single `PoolConfig { db: String }`, one pool,
      one database, with every space inside it separated by the `$space`
      column. There is no `$spacedb` table, no per-space pool, and no
      routing from a space to a connection.

      **Owner-ruled 2026-09-12, and this is the target shape:**
      "encryption at database not cluster and one tenant per database,
      that allows the distinction without the overhead of many
      instances." One Postgres process, one database per tenant, a
      `pg_tde` principal key per database. It is the rung where the key
      boundary and the query boundary coincide, which is not a
      coincidence: Postgres makes a database the unit you connect to, and
      `pg_tde` keys the unit you connect to.

      **What it requires in this tree**, none of which exists:
      1. **Connection routing per space.** `Pool` is one pool for one
         `PoolConfig { db }`. This becomes a pool per space, or a manager
         keyed by space, with the per-database ceiling on connections
         that implies.
      2. **A ruling on where each platform table lives.** Space-scoped
         ones move into the tenant's database (`$commandlog`,
         `$blobref`, `$sealedpayload`, `$checkpoints`). Cross-tenant ones
         cannot: `$secevents`' deployment scope exists to hold what
         belongs to no space, and `members`, `$spacebucket` and the
         access directory are read across spaces. Those stay central,
         which means a deployment has one control database beside N
         tenant databases.
      3. **Migrations applied N times**, and a deploy that is no longer
         one transaction.
      4. **Space provisioning**, which now exists for a space
         (`crate::provision`, 2026-09-13) and not for a space on **its own
         database**: the sequence mints custody, writes the founding member
         row and stops there, because every space still shares one pool.
         What routing adds is step 3 of §4, creating the database and
         running the space-scoped DDL against it before the member row
         lands.
      5. **Every cross-space sweep becomes a loop over databases**: the
         retention pass walks `try_scopes`, the blob collector runs per
         space, and both currently assume one connection reaches
         everything.

      **Keep the `$space` column.** It is redundant inside a
      single-tenant database and should stay anyway: it keeps one code
      path working in both topologies, it lets a tenant be migrated
      between the shared tier and their own database without a schema
      change, and it keeps the four software defences (composite primary
      key, composite foreign keys, the planner's table-plus-predicate,
      compiled guards) as depth behind the structural boundary rather
      than replaced by it.

      **Planned 2026-09-12 in [`plan-space-routing.md`](plan-space-routing.md)**,
      which resolves the scale problem: routing is data-driven and sparse,
      so almost every space points at the shared default and a few point
      elsewhere. The catalog, autovacuum and migration costs scale with the
      number of distinct **targets**, not with the number of spaces. The
      seam mirrors `StoreResolver`/`SingleStore` and the attachment table
      mirrors `$spacebucket`, both of which already argue every design
      question a database attachment faces. Eight increments, S1 to S8,
      and three questions for the owner.

      **S1 built 2026-09-13: the resolver seam.** `PoolResolver` and
      `SinglePool` (`crates/ordo-exec/src/route.rs`), held on
      `ordo_host::spaces::Spaces` beside the chain logs. The op path, the
      blob door's write and read, `std.space.export`, `std.space.delete`
      and the chain proof now reach a space's rows through it. A
      deployment that attaches nothing gets `SinglePool` over its own
      pool and behaves as before. Proved with a second real database:
      one space routed there uploads there and is read back from there,
      and the default database holds nothing of it. Still on the default
      pool until their increments: `$idempotency` and the attestation
      sink table (S4 decides their home) and the three drains (S6).

      **S2 built 2026-09-13: the attachment table.** `$spacedb`
      (`crates/ordo-exec/src/spacedb.rs`), on `$spacebucket`'s pattern:
      the credential is a reference, custody is stored with the sentence
      a consent page prints, and a repeat is keyed on the target's
      identity. One deliberate difference: a different target for an
      attached space is **refused**, because repointing a live space
      leaves its rows and chain behind, and moving one is the post-release
      item the owner ruled routing must be able to carry. Nothing reads the
      table yet. Next is S3, the resolver that does.

      **S3 built 2026-09-13: `AttachedPools`**
      (`crates/ordo-exec/src/route.rs`). An attached space resolves to its
      target's pool, every other space to the default; one pool per
      distinct target, dialled once and only on first use. The resolver
      became asynchronous. `PoolConfig` gained a password (it had none,
      so no attached database requiring one could have been reached),
      redacted from `Debug` and supplied through `DbCredentials` from the
      attachment's label; a label the node does not hold is refused
      before any dial. Dials are now bounded by the checkout timeout. An
      unreachable target is refused at resolve and never substituted by
      the default. Two obligations recorded for S5: attach before the
      founding member, and only a space with no rows on the default.

      **S4 built 2026-09-13: the security stream split by scope.** The
      drain sends deployment-scope entries to the control database
      without asking the resolver, so a denial naming a nonexistent space
      is recorded without opening anything, and each space's entries to
      its own database. A failed destination requeues only its own
      suffix. **A correction to the plan's §3:** `$jobs` is owed by every
      target, because a write enqueues its invalidation in its own
      transaction, so the "two DDL sets" were not needed and the split is
      a routing split. Homes named for every remaining table. S6 gains
      three items: claiming each target's jobs, the idempotency cache per
      space, and retention across targets. Q-1 ruled 2026-09-13: the
      default database stays the control database.

      **S5 first half built 2026-09-13: `prepare_target`.** A new space
      gets custody, a database of its own created resumably, the schema
      applied all-or-nothing, and an attachment the system chose (Q-3
      ruled: at provisioning the system decides; a later change needs
      both parties). It refuses before writing anything if the space holds
      rows on the default or is attached elsewhere. The record is one
      custody event on the deployment stream. **The founding member waits
      on `membership-home`** (Q-4): the door reads members from the
      control database, while a space's rows reference its members
      through foreign keys that cannot cross databases.

      **S5 second half built 2026-09-14.** Q-4 ruled: members live in the
      space's own database. `provision_space_on` writes the founding
      member into the target, last, and `directory::routed_engine` builds
      the door's access engine from every space's own database, ignoring a
      stale copy on the default and reporting a target it cannot read
      instead of failing. Two tests that assumed `localhost` answers
      `127.0.0.1` first were fixed on the way. Next is S6.

      **S6 core built 2026-09-14.** `PoolResolver::targets()` lists every
      database; security retention walks all of them; the idempotency
      cache keeps each space's records in the space's database. Three
      loops the plan named have no runner in the host yet (blob
      collection, job claiming, the attestation writer), and the
      supervisor that runs them must iterate `targets()`. The attestation
      delivery drain is S6b. **Gap flagged:** a schema change never reaches
      an attached database, since provisioning applies the schema once and
      the load gate checks only the default; item 3 above ("migrations
      applied N times") has to land before a second schema version meets
      an attached space.

      **S7 built 2026-09-14: a listener per database.** `?live` reaches a
      space on an attached database; every node must `refresh()` its bus
      when a database is attached. **S8, the record, done.** Routing is
      S1 to S7. **Open under 18d:** S6b (the attestation delivery drain's
      chain reads), migrations reaching every attached database (item 3
      above), the supervisor's loops iterating `targets()`, and moving a
      live space (18e).

      **What it costs, and this is the part to decide before starting.**
      The platform tables split unevenly. `$commandlog` and `$blobref`
      are already logically per-space and move cleanly. `$secevents` does
      not: it carries two scopes by design, and the deployment-scope half
      exists precisely to hold what belongs to no single space (denials
      by non-members, anonymous callers, and the cross-space sweep that
      is invisible in any one tenant's stream). That half cannot live in
      a tenant's database. So database-per-space requires a central
      database beside the per-tenant ones, and a ruling on which platform
      tables live where. Add to that per-space connection pooling,
      migrations applied N times, and Postgres's practical ceiling on
      databases per cluster.

      It also inverts the threat model. The database operator becomes the
      tenant, so plaintext in the database means plaintext to the data's
      owner, which is not a breach. Sealing stops being protection from
      the disk and becomes protection from **the platform**, which is the
      same thing the client-first vault (5g) is for one tier further on.

      **What is owed is a disclosure, and the pattern already exists.**
      `BucketCustody` (`ordo-exec/src/bucket.rs`) models exactly this
      choice for buckets, `PlatformSealed` versus `OwnPerimeter`, with a
      `disclosure()` that returns the sentence a consent page prints,
      written beside the mechanism because "a disclosure assembled by a
      renderer is a disclosure that differs per renderer". BYO-DB needs
      the same two-way choice and the same generated sentence, and for
      the same reason the bucket one gives: **it changes what
      crypto-erasure means.** Under platform custody, destroying the
      space's keys ends readability. Under the tenant's own perimeter,
      the platform's erasure does not reach into their database, so the
      plaintext skeleton survives in their own storage, and the consent
      page has to say so.

      **Requirements gathered 2026-09-20, in
      [`plan-space-routing.md`](plan-space-routing.md) §9 to §11.** The owner
      asked for the requirements to finish per-tenant database resolution, a
      default cluster with a configurable cluster per tenant, before any more
      of it is built. **The finding that shapes the rest:** S1 to S7 are built
      as libraries and the shipped binary constructs the single-database
      variant of every one of them. `runtime.rs:1488` builds `Spaces` over
      `SinglePool`, `:893` builds the door with `deployment_engine_pooled`
      rather than `directory::routed_engine`, `:1437` connects the
      single-database bus rather than `connect_routed`, `:1311` and `:1407`
      build two more `SinglePool`s inside the replay cache and the security
      drain, and `AttachedPools` is constructed nowhere outside tests. There
      is no environment variable that selects routing, no production code that
      installs a credential, and no production caller of
      `provision::prepare_target`, so no deployment attaches any tenant to
      anything today.

      **Four drifts the requirements found** (plan §9.3). The sharpest is the
      directory: `provision_space_on` writes an attached tenant's founding
      `members` row into its own database per the `membership-home` ruling,
      while the door reads `members` from the control pool only, so the moment
      routing is switched on every request in an attached tenant is denied by
      a door working exactly as written. The others are the idempotency cache
      and the security drain, both of which have routed variants nobody calls;
      and `$jobs`, which contradicts §3's own correction, because neither
      `$jobs` nor `$schedules` has a `$space` column and every consumer reads
      the control pool, so an attached tenant's enqueued work would be claimed
      by nobody.

      **The credentials answer** (plan §9.1). `$spacedb` holds a label and
      never a secret, which is right and is already asserted by a test that
      reads the DDL. The gap is that the only `DbCredentials` implementation is
      an in-memory map built in tests, so an operator cannot install a
      credential at all. The requirement follows the shape the deployment
      already has: `ORDO_DB_CREDENTIALS` names a provider the way
      `ORDO_KEY_PROVIDER` does, and the one v1 provider reads a mounted
      directory where the label is the file name, on the reason
      `PoolConfig::from_env` already gives for preferring
      `ORDO_PG_PASSWORD_FILE`. Five things the door must never return are named
      with a test each, including that a dial error's text never reaches a
      caller, since it carries the host, the user and often the database.

      **Eleven increments, smallest first** (plan §10), T0 to T10. T0 to T5
      make routing switchable and provably identical when off; T6 to T8 are
      real builds. **Eight owner decisions still stand** (plan §11), the two
      that most need a ruling being whether the custody disclosure must state
      **occupancy** as well as custody, since a pool is per target and two
      tenants on one database have the shared tier's isolation while being told
      their data is in a database their organisation runs; and whether an
      unreachable target during a usage rollup leaves a **gap** in the ledger
      rather than a row of zeros, which is a silent discount. All four of the
      plan's §8 questions are ruled, and the code has gone further than three
      of them.

      **The object-storage half is 18p**, gathered in lockstep on the owner's
      instruction. Four of its increments have to ship **with** a T here rather
      than after it, and they are named in 18p.

      **T0 and T1 built 2026-09-21.** T0 is two tests and no production
      change, pinning properties that held by accident: a statement on an
      attached target is bounded too, and a `PoolConfig` never prints its
      password. Both were falsified against deliberately broken code
      before being accepted, and the statement-bound test waits out the
      process's real configured bound rather than a shortened stand-in.

      **T1 puts the door on `routed_engine`**, so an attached tenant's
      founding member is read from its own database. Two things came out
      of it that the plan did not have.

      1. **A production fix the plan did not name, and it would have
         broken every fresh deployment.** `routed_engine` had no
         equivalent of `deployment_engine_pooled`'s `to_regclass`
         `members` guard, so swapping the door onto it as written would
         have turned `a_database_with_no_program_boots_and_is_ready` into
         a hard boot failure against any database `ordo apply` had not
         touched. That is the same mistake C7 found three times in the
         blob plan: one path assumed to inherit another's behaviour.
      2. **The door's credentials are a placeholder until T3.**
         `Installed::new()` holds no label, so a target `$spacedb`
         actually names fails credential lookup before any dial. That is
         honest rather than a regression, because nothing installs a
         credential in production yet and no attach op exists before T8,
         so a target unreachable before this change is unreachable after
         it. `door_for`'s doc says so in full rather than leaving it to be
         discovered.

      **What T1 makes more urgent, and it is already T5's job.** There are
      now two ways a space ends up unserved with two different
      visibilities: the quarantine register, which `/health/ready` names,
      and `Directory::unread`, which is printed to stderr. 17ac just
      established that a closed space must be named in readiness, so these
      two disagree. T5 assembles one list from `Spaces::quarantined`,
      `Directory::unread`, `PgInvalidationBus::unheard` and `targets()`,
      which is exactly the repair. Nothing here is wrong; the gap is
      scheduled, and T5 should not slip past T3.

      **T5 built 2026-09-21, pulled ahead of T2 and T3 on the owner's
      instruction, because T1 had left two lists disagreeing.**

      **One list.** `/health/ready`'s `quarantined` array is assembled
      from both sources that can close a space: the resolver's register
      and the door's report of tenants whose members it could not read.
      Merged rather than concatenated, because a space can be in both at
      once and naming it twice makes one broken tenant look like two. The
      register wins on conflict as the more recent fact: the door is
      built at boot and on deploy, the register on every request.

      **Every failing exit from `resolve` writes the register**, through
      one function (`Spaces::note`), so a refusal added later cannot
      quietly forget to. Success lifts, which makes the register
      self-healing for every reason it holds rather than the rewind
      alone: a cluster that comes back lifts its own tenants on their
      next request with no restart.

      **An unreachable cluster answers 503 through a new class,
      `space_unavailable`.** The distinction an operator and a client both
      act on: 500 says the deployment is broken, this says a dependency is
      down and retrying is right. `Rewound` and `NoCustody` keep
      `internal` deliberately, because no amount of retrying fixes a chain
      needing a recorded restore. The caller's message is opaque about the
      cause (R-C4.4): a dial error names a host, a port and often a user,
      and for a tenant on their own cluster that is their infrastructure
      described back to them by a third party.

      **A down target costs one dial per backoff interval rather than one
      per request** (R-F3), keyed by target so every tenant on a downed
      cluster waits behind one timer. 250ms doubling to a 30s cap, and it
      expires rather than latches so a recovered cluster is found with no
      restart. Asserted on a dial counter rather than on timing or message
      text, because timing cannot separate the two when a port refuses
      immediately and `target_count` answers zero either way: a target
      that never connects has no pool to count. Falsified at 10 dials
      without the backoff against 1 with it.

      **Two things found in the tests rather than the code.** The
      opacity test was checked for vacuity first: Postgres's dial error
      for a missing database really does embed the database name
      (`FATAL: database "…" does not exist`), while a refused port says
      only `Connection refused`, so the assertion is tested against the
      error that can actually leak. And the merge test originally
      collected the published array into a map before asserting "each
      exactly once", which a duplicate would have passed silently, since
      the map would have deduplicated on its behalf. It now asserts on the
      array as published.

      **T2 built 2026-09-21: one resolver, four consumers.** The
      deployment built four independent `SinglePool`s, for `Spaces`, for
      the exit and attestation ops, inside `PgReplayCache::new` and
      inside `SecurityDrain::start`. The last three each had a routed
      variant nothing called, and `exit_ops.rs`' own doc had said since it
      was written that it should be handed `Spaces::pools()`.

      **Four resolvers over one deployment are four answers to "which
      database are this space's rows in".** They agree for exactly as long
      as every space is on the default pool, and they fail quietly when
      that stops being true: a pool that answers looks identical to a pool
      that answers correctly. The three concrete failures that closes are
      worth naming, because each is a different kind of wrong: an **export**
      reading the control database for a tenant attached elsewhere
      produces an empty bundle and calls it a disclosure; an **idempotency
      record** written to the control database means a replayed key finds
      nothing and re-executes the write; and a **security event** written
      there is one the tenant's own retention pass will never see.

      **The reordering is what made it Opus-tier.** `Spaces` was built at
      the far end of `assemble`, after the replay cache, the drain and the
      door. It now sits directly behind the deployment it resolves for,
      which is forced rather than chosen: the replay cache is constructed
      well before the door, so the resolver has to exist before the cache.
      Everything it needs, the deployment and a pool, is already in hand
      there. `SinglePool` is now constructed once in `runtime.rs`, which
      is T2's done-when in its own words.

      **The test is a source scan and the file says why at length.** The
      property is that every surface reaching a space's rows reaches them
      through one resolver, and the failure it prevents is unobservable
      today: `ORDO_PG_ROUTING` does not exist until T4 and no attach op
      exists until T8, so `$spacedb` is empty in every deployment that can
      currently boot. With one database, four resolvers and one resolver
      are indistinguishable by every test that can be written, and they
      stay that way right up to the first attachment, which is exactly
      when the difference becomes one tenant's data read out of another's
      database. So it is pinned at the construction, falsified by putting
      the replay cache back on its own pool, and `tests/one_resolver.rs`
      records what should replace it: once T4 and T8 land, attach a tenant
      to a second database and assert an op, a replayed key, a security
      event and an export all land in the target. **That file should be
      deleted on that day rather than kept alongside**, because a scan
      that outlives the behaviour it stood in for starts refusing
      legitimate edits.

      **T3 built 2026-09-22 with U4, as one provider.**
      `crates/ordo-exec/src/creds.rs`: one mounted directory, read whole
      at boot, where a **file** named by the label is a Postgres password
      and a **directory** named by the label holding `access_key_id` and
      `secret_access_key` is an S3 key pair. It implements `DbCredentials`
      and a new `BucketCredentials`, which is what makes it one provider
      rather than two behind one name. `door_for`'s placeholder from T1 is
      gone and its doc says what is now true.

      **The two plans conflicted on the selector and the later one wins.**
      R-C3 named it `ORDO_DB_CREDENTIALS`; B-K1 renamed it
      `ORDO_TENANT_CREDENTIALS` and argued that two providers would be two
      rotation procedures, two boot refusals and two label lists on one
      node. Implemented under the second name only, with the conflict
      recorded in the module doc where somebody will meet it.

      **"Empty" means opposite things for the two shapes, on purpose.** An
      empty Postgres credential file is *passwordless*, because peer,
      trust and certificate authentication are legitimate and all look
      like no password from here. An empty S3 key half is **refused**,
      because `S3Config::new` already refuses one rather than attempting
      anonymous access, and the failure being prevented is a tenant's
      bucket silently going public. Making them consistent in the obvious
      direction is the mistake; the asymmetry is written at both sites.

      **Three findings the brief asked for and the build had to settle.**

      1. **Symlinks are followed, and an escape is refused.** A blanket
         refusal was the first design and was wrong: Kubernetes Secret
         volumes and Vault Agent both publish keys as symlinks into a
         versioned `..data` directory as their normal atomic update, so
         refusing every symlink makes the provider unusable on the exact
         deployment shape R-C3 was written for. The rule is canonicalize
         the directory once at open, canonicalize every entry, refuse any
         whose canonical path is not inside it.
      2. **The mount root's own mode is not checked, only each label's.**
         Checking the root refused to boot against a real Kubernetes
         secret volume, which is commonly `0755`. B-K3 asks for the S3
         label's own directory, which is what is checked, along with every
         credential file.
      3. **A `TempDir` helper keyed on pid plus nanoseconds collided**
         under four threads, two landing in one clock tick and corrupting
         each other's fixtures. An atomic counter fixed it. Worth keeping
         because it produced nondeterministic failures that looked like
         the feature.

      **One test was added on review and it is the most valuable in the
      file.** `a_sibling_directory_sharing_a_name_prefix_is_still_outside`
      pins the single line the whole symlink argument rests on:
      `resolve_within` compares with `Path::starts_with`, which is
      component-wise, so `/creds-evil` is not inside `/creds`. Rewritten
      over strings, which is the obvious tidy-up, it compares byte-wise
      and lets a sibling directory through. Falsified by making exactly
      that substitution and watching the test fail. No other test would
      have noticed, because every other one puts its escape somewhere with
      an unrelated name.

      **And a defect I introduced adding it, caught by clippy rather than
      by the suite.** The insertion left a stray `#[test]`, so the new test
      carried two and ran twice. The doubled line in the test output was
      visible and I could not explain it from the source; `duplicated
      attribute` under `-D warnings` named it exactly. The lesson is the
      one worth keeping: a test count that does not add up is a fact about
      the build, not a display artifact, and the lint found it while
      reading the source found nothing.

      **T4 built 2026-09-22: the switch, and the first tenant actually
      served from its own cluster.** `ORDO_PG_ROUTING` takes `single` or
      `attached`; an unknown value refuses at boot naming its legal
      values, and `attached` without a credential provider refuses naming
      the variable that fixes it. That second refusal matters more than it
      looks: routing that cannot authenticate resolves an attachment and
      then fails every dial, so without it a node comes up, reports
      **ready**, and refuses each attached tenant one request at a time.
      The invalidation bus moves to `connect_routed` under `attached`,
      because `NOTIFY` reaches only sessions on the database it was sent
      from, so a bus on the control database alone would deliver nothing
      for an attached tenant and do it silently.

      **`a_deployment_that_attaches_nothing_behaves_identically` is what
      made this safe to land before the attach op.** It boots one database
      twice, once per mode, and asserts the readiness body is identical.
      `AttachedPools` reads `$spacedb`, an empty `$spacedb` routes
      everything to the default, and the default is the pool the
      deployment booted with, so flipping the switch with nothing attached
      must be indistinguishable. If it were not, the switch would carry a
      behaviour change nobody asked for.

      **The done-when is `tests/attached_tenant.rs`**: one tenant
      provisioned onto a second real database, through a real
      `AttachedPools`, with its credential resolved through T3's
      `creds::Dir` rather than an in-memory stub, so the test exercises
      the production path from label to dial. Writes, reads, security
      events, idempotency records and `?live` all proved to land in the
      target, **each with its negative half** against the control
      database, because a resolver that quietly fell back to the default
      would pass every positive assertion. Falsified by making the
      resolver ignore attachments entirely, which the test catches.

      **Two test-design corrections found while writing it, both real
      facts about the system.**

      1. **`access_guard` never reaches the security-event recorder.** It
         is raised by the executor, not the door, so a row-guard denial
         records nothing in either database. The first version of the
         security-event assertion used one and found zero rows everywhere,
         which reads as a routing failure and is not. It uses a
         `Role::Viewer` member attempting a write instead, which the
         access engine itself denies, and that is a door decision and is
         recorded.
      2. **`?live` is not space-scoped at the subscription layer, by
         design.** `InvalidationEvent` carries an op and no space, so
         every subscriber to an op is told regardless of which space
         wrote: the notification is a staleness signal and the refetch is
         what is scoped. A "subscriber on the other space hears nothing"
         assertion is therefore wrong for this system. The correct
         negative is a second bus listening only to the control database,
         which hears nothing from the twin's write.

      **And T2's source scan failed on a legitimate edit, one increment
      after that file predicted it would.** T4 moved the single
      `SinglePool` construction into a `match cfg.routing` arm, and the
      scan's anchor was "is `Spaces::routed` just above it". Repointed at
      the `Routing::Single` arm, with the weakening recorded at the
      assertion rather than quietly absorbed, and a second assertion added
      for the half that did not weaken. **Its retirement condition is now
      sharper**: `attached_tenant.rs` hand-assembles the pieces the way
      `assemble_program` does, so it proves they compose when wired to one
      resolver and cannot prove the runtime wires them that way. Delete
      the scan when a test drives the real boot path against an attached
      tenant, which needs T8.

      **A verification lesson worth keeping.** The first full-workspace
      run after T4 reported 1,149 passed and 0 failed, against 1,780
      before it. Zero failures with a third of the tests missing is what a
      failing binary looks like under `cargo test` without
      `--no-fail-fast`: the run stops and the totals silently
      under-report. Sweeps use `--no-fail-fast` from here. The real number
      is 1,784 across 181 binaries.

      **U2 built 2026-09-22, and it found that the plan's own mechanism
      deletes live bytes.** B-F3 said the door writes a `$blobtomb` intent
      before the `put` and phase 2's re-check cancels it on commit. The
      first half is right; the second loses data. `mark` skips a handle
      that already has a tombstone row, and the sweep only ever examines
      tombstones that are **due**, so a committed upload's intent survives
      stamped at upload time and prices every later deletion against that
      clock. A row deleted ninety-one days after its upload has its bytes
      swept while the deletion is seconds old and revertible for ninety
      more days. Every upload whose row is deleted inside the horizon
      would lose its bytes early. Full argument in `corpus-rulings.md`.

      **Verified independently before the departure was accepted**: both
      sweep properties read in the code, and
      `a_delete_long_after_an_upload_is_floored_from_the_delete` run both
      ways, reporting `deleted: 1` against the plan's design and `0`
      against what was built.

      **What was built**: the door declares the intent before the `put`,
      and the `$blobref` insert cancels it **in the same statement** as a
      leading data-modifying CTE. An intent and a committed reference for
      one handle cannot coexist, so there is no window and nothing to
      crash inside. Phase 2 is unchanged and becomes the revert backstop.
      The crash test drives a real `PUT` whose row guard refuses inside
      the write transaction, so the object is in the store and nothing
      about it is committed: the B-F3 window produced rather than
      described.

      **Three things it found and did not fix, filed rather than
      dropped.**
      1. **A pre-existing bug of the same shape, with no intents
         involved. FIXED 2026-09-23.** Mark at `t0`, revert at `t0+10d`
         (the tombstone stays open and not due), delete again at `t0+20d`
         (mark skips, because a tombstone exists), sweep at `t0+91d`
         deletes bytes the second deletion is still revertible into.
         Measured at `deleted: 1` with the fix disabled. Closed by an
         eager cancel in phase 1: before marking, delete open tombstones
         whose handle a live row names again, so the next deletion gets
         its own clock. `tests/blobgc_stale_clock.rs`, falsified both
         ways. It did change the accounting exactly as predicted, and an
         existing test caught it: the revert is still cancelled, one
         phase earlier, so phase 1's cancels now join phase 2's in one
         `cancelled` number. Full argument in `corpus-rulings.md`.
      2. **`reseal.rs` has the window U2 just closed.**
         `Population::Objects` writes a new object and then points the
         column at it; interrupted between the two, the new handle is
         named by nothing. It should declare an intent the same way.
      3. **A cadence caveat.** The cutoff is `min(chain-tail time,
         horizon)`, so an intent stamped after a space's last command
         group waits until that space writes again. Self-healing for an
         active tenant; worth knowing for one that uploads once and stops.

      **The plan's file list was also incomplete**, and unavoidably:
      `blobgc.rs` needed no code change at all, while the handle is minted
      inside `BlobPipeline::put`, so `ordo-blob` had to grow a seam, and
      the tombstone SQL belongs in `ordo-crypto`'s `blobref.rs`, which
      owns the table's shape and is the only place that can render the
      cancel into `insert_sql`.

      **T7 in progress, 2026-09-22/23. Two slices built so far.**

      **R-S1: a fresh target records the version it received.**
      `apply_schema_at_version` sends the DDL, the `$migrations` create and
      the version row as **one** simple-protocol query, which Postgres runs
      as one implicit transaction, so a target can never hold the tables
      without the record of which version they are. `prepare_target` lost
      its `ddl` field and now takes the `Deployment`, reading
      `current()` once so the occupancy check, the DDL and the recorded
      version are all about one version. A database with `members` and no
      `$migrations` row reads as **version unknown** rather than erroring,
      and nothing retrofits a row for it.

      **The all-or-nothing property was checked rather than assumed.**
      `Pool::rows` really is the simple protocol (`SimpleQueryMessage`), so
      the guarantee holds; an extended-protocol path would run each
      statement in its own implicit transaction and the guarantee would
      evaporate with no doc change. A runtime check refuses DDL containing
      `begin`, `commit`, `rollback`, `savepoint` or `concurrently` as bare
      tokens, skipping quoted identifiers and literals so a tenant entity
      named `commit` does not fail provisioning.

      **The discriminating test is
      `the_tables_and_their_version_row_arrive_together_or_not_at_all`**,
      and it earns that name: sending the row as a second query fails it
      and **only** it, with the other seven passing. Verified by making
      that exact substitution.

      **R-S1 as worded closed half the gap, and the build closed both.**
      §9.2 says `prepare_target` takes the DDL from the caller. It also
      took a separate `schema` for the occupancy check, so there were two
      ways a caller could disagree with the deployment. Replacing the
      `schema` parameter with the `Deployment` closes both; adding a
      parameter would have left a caller able to hand a stale `Schema` to
      the occupancy check while the deployment supplied the DDL.

      **Two facts recorded for the rest of T7.** `prepare_target` and
      `provision_space_on` have **no production caller at all** today, so
      R-S5's advisory lock has no live attach path to serialise against
      until T8 provides one. And `$migrations.at` is **milliseconds** where
      `schema_apply::record` writes it while `prepare_target`'s `now` is
      **seconds**; the call site converts, and a test pins it. R-S2's fleet
      view reads that column.

      **R-S3: `apply_everywhere`, the walk.** Every target takes the
      program, and the **control database is applied last**.
      `PoolResolver::targets` lists the default first, which is right for a
      reader and wrong for an apply: the control database holds
      `$artifacts`, `$activations` and the fleet's own record, so applying
      it first leaves the record ahead of the fleet for the length of the
      walk, and a process dying in that window recovers against a control
      database claiming a version some tenant never received. Falsified by
      removing the reordering.

      An unreachable target is an **outcome, not an absence**, on the same
      rule `targets()` follows one layer down: a target silently missing
      from an apply is a tenant a version behind with nothing saying so.
      The walk continues past a failure, because an operator repairing this
      needs to know whether one target is broken or all of them, and
      `all_succeeded` is what the activation is gated on. Nothing is rolled
      back and nothing can be, since no transaction spans N+1 databases;
      what protects the deployment is the activation not landing, which
      every class-A change tolerates because additive statements leave the
      old program's reads working.

      `ApplyRefusal` gained a greppable `class()` on
      `apply_op::ApplyRefused`'s precedent, so a per-target summary gives
      an operator one searchable word rather than a `Debug` dump.

      **R-S4 and R-S6 built 2026-09-23, as one mechanism.** A space whose
      database is not at the serving version is quarantined with `schema
      behind` as its reason, joining T5's one list, and the same question
      is asked by the boot (`reconcile`) and by the first request
      (`resolve`) so the two cannot disagree. A recovered target lifts its
      own quarantine on the next resolve with no restart.

      **A version comparison rather than a shape diff, and the reason is
      cost and authority.** The shape gate answers "does this database
      match the DDL", which is right at boot for the deployment's own
      database and far too heavy to pose against every target on every
      resolve. R-S4 refuses on something narrower that R-S1 just made
      authoritative: the target's own `$migrations` row. A database
      claiming the serving version while not matching it is schema drift,
      which stays the boot gate's question.

      **Three cases must NOT close a space, and each has a test**, because
      each is a way a naive comparison turns a working deployment into a
      dead one.
      1. **Version unknown is not version behind.** Every database
         provisioned before R-S1 has a schema and no `$migrations` row, so
         treating absence as "behind" would quarantine **every existing
         tenant on the first boot after the upgrade**. Falsified by making
         exactly that substitution and watching the guard test fail.
      2. **A deployment with no active program quarantines nobody.**
         `Deployment::empty` carries `(none)`, which no target can be at
         or behind, and a deployment awaiting its first apply is the
         ordinary first-boot state.
      3. **A database at the serving version serves**, which is the
         inverted-comparison case.

      **The walk is wired, 2026-09-23.** `ApplyOps` carries the
      deployment's resolver, `sequence` walks every target that is not the
      control database after storing the artifact, and **the activation
      does not land unless every one of them took it**. Until this,
      `apply_everywhere` had no production caller at all: it existed for a
      day while a deploy still reached the control database alone.

      **The test that would have been red for that day** is
      `an_unreachable_target_holds_back_the_activation_on_the_apply_path`,
      and it asserts on `$activations` rather than on the refusal. A
      refusal alone proves nothing here: an apply could refuse *after*
      activating, leaving the deployment serving a program one of its
      databases does not have, which is the exact state R-S3 exists to
      prevent. Falsified by disabling the gate. Its partner,
      `a_reachable_fleet_activates`, stops the guard passing for the
      trivial reason that nothing ever activates.

      **Why the control database is applied first here and last in
      `apply_everywhere`.** R-S3 asks for control last so the record is
      never ahead of the fleet. In `sequence` the **record is
      `$activations`**, which is the last thing that happens, and the
      control database's own `$migrations` row describes its own shape
      and says nothing about anybody else. So the invariant holds with
      control applied first, and `apply_everywhere` standalone still
      applies it last, where the walk is the whole operation. Two
      functions rather than a flag, because a flag would make "did this
      include the control database" a question every caller answers and
      every reader checks.

      **`ordo apply --db` deliberately does not walk.** The operator
      named a connection string; reaching whatever that database's
      `$spacedb` lists would touch targets they did not name, from a tool
      whose whole contract is "this database". It passes a `SinglePool`,
      so the direct path is unchanged. The door path carries the real
      resolver, which is right: that caller is the deployment acting on
      itself.

      **R-S5 built 2026-09-24: attach and apply are serialised.** It was
      blocked on T8 giving `prepare_target` a production caller, and T8
      gave it one.

      **The lock already existed, and the first version of this built a
      second one.** `apply_op::ApplyLock` has guarded applies since it was
      written: `pg_advisory_xact_lock` keyed from `$migrations`, taken one
      line before the apply sequence and held across the fleet walk *and*
      the activation. R-S5 asks for **one** lock taken by both sides, so
      `prepare_target` takes that one. What was written first was a new
      session-scoped `RoutingLock` in `spacedb.rs`, on the reasoning that
      `CREATE DATABASE` cannot run inside a transaction so a
      transaction-scoped lock could not span the attach. That reasoning is
      wrong in the way that matters: `ApplyLock`'s transaction is on a
      **dedicated connection** and never runs the `CREATE`. The new lock
      also carried a false claim — that advisory locks ignore
      `lock_timeout` — which the existing code disproves by using exactly
      that to bound its wait. Deleted; the mechanism is one, not two.

      **Transaction scope is the better shape and the reason is in
      `ApplyLock`'s own doc:** a session lock left by a task cancelled
      mid-attach is a lock nothing releases until the process restarts. A
      transaction's locks go when the transaction does, including on panic
      and including when the socket closes underneath.

      **The refusal only comes after waiting.** `LOCK_WAIT_MS` is 30s, and
      a timeout is `ProvisionRefusal::RoutingBusy` → 503, because nothing
      about the call was wrong and the identical one lands. That is the
      plan's requirement: *"the loser waits rather than refusing, because
      an attach that refuses during every deploy is an attach that looks
      broken."*

      **A cost worth knowing before a small pool meets it:** the lock rides
      a pooled connection while the rest of `prepare_target` reads the
      control database on another, so **an attach occupies two control
      connections** for its duration and N queued attaches occupy 2N. Same
      shape `ApplyOps` has always had; a control pool sized 2 serves one
      attach at a time. It is what made the first run of the new test
      return `Saturated` rather than a wrong answer, and chasing that is
      what turned up the conformance cluster's connection ceiling — see
      17ad, where the measurement is recorded.

      **R-S2 built 2026-09-25: the fleet view.** `ordo_exec::fleet`, one row
      per target in the control database (`label`, `version`,
      `confirmed_at`). **T7 is complete.**

      **The cache is a copy, and the code makes it one.** Each target's
      version is read back from that target's own `$migrations` after the
      apply rather than assumed to be the program's version. Asserting the
      program's version would make `$fleet` a second claim about the target
      instead of a copy of it, with nothing to arbitrate a disagreement. The
      tests compare the two rather than either against a literal, and there
      is no accessor that returns a version without its `confirmed_at` —
      that would be a caller treating the cache as an authority, which R-S2
      forbids and C-M1 permits this table only on condition of.

      **`route::target_label` now has one definition.** The attach and the
      apply walk both write this table, so two spellings of one database
      would be two rows describing one target and whichever an operator read
      first would look authoritative.

      **Two gaps the falsification found, not the tests.** Breaking the
      fleet write failed the right tests; breaking the *walk's* observation
      failed nothing, twice. First because the only test covering it had one
      target whose row was read by a different code path, then because the
      two near-duplicate walks meant the break landed in the copy the test
      did not call. `apply_everywhere_except_default` is the one `sequence`
      uses. Both are covered now and both falsify. The lesson is the one
      from the loopback test earlier in this increment: a break that fails
      to fail is a fact about the test, and the answer is to find out which
      code path the test actually reaches.

      **A third gap found the same way.** The walk excludes the default
      target — `sequence` applies the control database through the schema
      step instead — so the first version of this recorded every attached
      target and omitted the one database every deployment has. Caught by
      the test asserting a row for `default`.

      **T8 built 2026-09-24: the attach path, its refusals and its
      record.** `prepare_target` and `provision_space_on` had no
      production caller at all — nothing outside a test had ever attached
      a tenant to anything. They have one now.

      **The caller is `std.space.create`, not a separate attach op.** The
      plan's wording implies one (`std.space.attachDatabase`, symmetric
      with `std.space.attachBucket`), and it was not built, because the
      symmetry is false. A bucket is attached to a space that already has
      an owner who consents; a database cannot be, since `prepare_target`
      refuses any space already holding rows (`erase::rows_held`) — so an
      owner-consented database attach would refuse every space that had
      ever been written to. The 2026-09-13 ruling (`db-attach-gate`) says
      the same thing from the other end: *"during provisioning the system
      decides... you cannot have an agreement for a space that does not
      exist and you cannot have a space without a place for storage."* So
      routing is six optional parameters on the create op, `$db` decides,
      and changing it afterwards is the two-party act 18e describes.

      **Four refusals, all falsified against deliberately broken code**
      (three once D-D collapsed the two perimeter refusals into one).
      R-X2's control database, refused **by identity rather than by
      name**: `localhost`, `127.0.0.1` and `::1` are three spellings of
      one server, and a string comparison catches whichever the operator
      did not type. R-X1's occupancy, which D-D then widened: a database
      holds **one tenant**, whatever either side's custody says. R-X3's
      host allowlist (`ORDO_PG_ATTACH_HOSTS`), **default
      deny**, whole-host match, one reason and no detail.

      **The stated residual on identity.** A DNS name that resolves to the
      control database's address is not equal to it as a string and would
      pass. The complete check is to dial and compare Postgres's
      `system_identifier`, which no spelling can disguise; it is not built
      because this check runs **before anything is written**, and catching
      the three common spellings there is worth more than catching every
      spelling after a database has been created.

      **Two things the tests caught that the code did not.** The loopback
      test is **vacuous on a developer machine**: the conformance cluster
      is on a Unix socket (`ORDO_PG_HOST=/tmp/ordo-pg`), so the door-level
      version of it only ever runs on CI. Breaking the normalisation on
      purpose failed to fail, which is how it was found; the claim now has
      a unit test that runs everywhere. And a socket directory is a
      **path**, not a hostname: folding its case, as one must for a
      hostname, would have matched `/var/run/PG` to `/var/run/pg` on a
      case-sensitive filesystem. `fold` now folds one and not the other.

      **The custody record was an event with no facts in it.** It said
      `space_attached_database` and nothing else, so it recorded that an
      attach happened and not one thing about it. It now carries the
      target's identity, the credential **label**, the custody, the
      occupancy at the moment of attach, and whether this call attached
      fresh or completed a provision interrupted after the database was
      made. All of it in the action string rather than in new fields:
      `EventKind::CustodyAction` is hash-chained under a tag, so widening
      the variant would change the encoding of every event already
      written.

      **R-X4's founding-group half was not built, because it was ruled
      away.** D-F, ruled 2026-09-21: *"theres no provability unless its
      byo db - they can see where there data is. I think this belongs to
      the app not ordo."* The deployment-stream record above stands; the
      tenant's own chain does not carry the attachment fact.

      **Occupancy is exposed as a fact, not written as a sentence**
      (`spacedb::occupants`): the platform records and exposes, the
      developer writes the sentence. `DbCustody::disclosure` is unchanged.
      Superseded in part by D-D on 2026-09-25 — see below, which makes one
      tenant per database the rule and so makes occupancy a one-valued fact
      for an attached tenant.

      **D-D ruled 2026-09-25, and it is broader than the question that was
      asked:** *"1 tenant 1 database. same for buckets. database/schema not
      cluster."*

      What was asked was whether a second tenant inside a declared **own
      perimeter** is refused. What was answered is the general rule, so the
      two refusals built for the narrow reading — one per direction,
      distinguished by which side declared `OwnPerimeter` — collapsed into
      one `DatabaseOccupied`, and custody is not consulted at all.

      **A test was inverted rather than deleted.**
      `two_platform_managed_tenants_share_one_database` asserted the shared
      tier worked, and it was not wrong about the code: R-X1 describes that
      tier in as many words, *"a hundred tenants attached to one database
      share one pool and one credential."* The ruling removes it from the
      attach path, so the assertion now reads
      `two_tenants_do_not_share_one_database_whatever_their_custody` and
      also checks a **second database on the same cluster** is still fine —
      without that half the test would pass if attaching had simply stopped
      working.

      **Occupancy survives as a fact, not a disclosure.** `occupants` stays
      (it enforces the refusal, and R-S2's fleet view reads it) but cannot
      return more than one attached tenant now, so the ambiguity R-X1
      wanted a disclosure clause for — "your own database" printed to a
      tenant sharing one — cannot arise. `DbCustody::disclosure` is
      unchanged.

      **`"same for buckets"` lands on U6**, and this session read it wrong
      twice before getting it right. First it was scoped to the bucket
      *attach* path, leaving the default path sharing one bucket, and the
      part scoped off was put back to the owner as a fresh question
      (`st-default-tier`) — his reply: *"one tenant one bucket. Same
      principal. I dont know how that got lost."* Then, told to build the
      general rule, U6 refused any second tenant from a bucket whatever
      prefix they stated, which was too wide. The clarification settles it:
      *"1 bucket can have folders so 1 bucket defined per - but it could be
      the same and bucketname/tenant would be the option."*

      A **bucket** answers to a **cluster** and a **folder** to a
      **database**. `ordo_blob::object_key` has always keyed every object
      `<space>/<handle>`, so two tenants in one bucket could never collide
      and the widened refusal guarded something already impossible. It is
      reversed; D-S8's own rule survives, in both directions. See
      `corpus-rulings.md`, 2026-09-25.

      **What the ruling did not reach was put on the board as
      `db-default-tier`, and answering it reframed the question**
      (2026-09-25): *"there exist a default cluster which databases are
      created in. 1 tenant one database - in addition a tenant COULD have
      their own BYO DB. But the settings are simply to decide what DB
      cluster to store the database in. Either whatever the deployment
      provides as default, or what the deployment provides for that
      tenant."*

      The question offered two readings and the answer took neither,
      because the question had the wrong shape: it assumed a tenant might
      or might not be **given** a database. Every tenant has one. What the
      setting chooses is the **cluster** it is created in.

      **This turns T8's parameters over.** `$db` was built as *name the
      database this tenant's rows go in*, with host, user and credential
      beside it and the call refused unless all were given. Under the
      ruling the database is not optional and not the caller's to name, so
      the parameter becomes which cluster, absent meaning the deployment's
      default tenant cluster, and the database name is minted the way the
      space id already is. Queued as `db-always-a-database` on the board, and **built 2026-09-25**
      except its migration half.

      **What landed.** `$db` is gone and the database name is minted from
      the space id, which is already 30 bytes of `[a-z0-9_]` starting with
      a letter and globally unique, so it is a legal Postgres identifier
      and two tenants cannot collide on one.
      `ORDO_PG_TENANT_HOST`/`_PORT`/`_USER`/`_CREDENTIAL` are level 3, each
      falling back **per field** to the deployment's own: an operator
      pointing tenants at another server usually changes the host and keeps
      the port, and demanding all four would be three more chances to
      mistype the ones that were not the point. `std.space.create` no
      longer has a branch, so there is no path that puts a tenant in the
      control database.

      **Two things had to be true for the zero-configuration case, and
      neither was obvious until it broke.** A tenant database on the
      deployment's own cluster needs the deployment's own credential, so
      `route::OWN_CREDENTIAL` (`$deployment`) is a reserved label the
      provider answers from the control pool's password; without it level 3
      would require a credential directory to exist. And the host allowlist
      denies by default, so an unset `ORDO_PG_ATTACH_HOSTS` would have
      refused a deployment its own cluster and no tenant could ever have
      been created. `HostAllowlist::allowing` adds the configured hosts, on
      D-S6's *"if i defined it on my platform it is allowed."*

      **The D-2 landmine went off, exactly as §9.3 predicted.** Turning
      this on broke `a_minted_tenant_answers_to_its_owner` at once: the
      founding member row is now in the tenant's own database, and the test
      rebuilt the door's engine from the control database alone, so the
      owner had no membership in the space created for them. The plan calls
      this *"the kind of defect that looks like an authorisation bug for a
      day"*. `runtime.rs` was already correct — it builds through
      `directory::routed_engine` — and the **test** was reading a path
      production does not take. It now rebuilds the way a deployment does.

      **Closed 2026-09-25, and the question was moot.** It asked what
      happens to tenants already sitting in a deployed control database.
      There are none: *"Nothing has deployed in terms of ordo so we make -
      expectation of recoverability for anything at the moment."* So no
      migration is owed here, or anywhere else, until the first deployment
      ships. That reaches far past this increment and has its own entry in
      `corpus-rulings.md`. The rule it produced: before asking what happens
      to existing state, establish that any exists.

      **A mistake worth recording.** While falsifying, `git checkout` was
      used to revert a deliberate break in a file that also held
      uncommitted work, and it took the work with it. Falsification
      restores from a copy taken before the break, never from the index.

      **Four clusters, not two**, clarified the same day: *"It doesnt
      neccesarily need to be the control plane database. The console has
      its own it can define, the deplyment can define its own, it can
      define one as a default for tenants, and it can define individually
      per tenant."*

      1. The **console's** own, which belongs to the console rather than to
         any deployment it manages (18o puts the console and the operator
         outside the deployment).
      2. The **deployment's** own: the control plane.
      3. A **default cluster for tenants**, where a tenant's database is
         created when nothing else is said.
      4. A **per-tenant** cluster, the BYO case.

      **Level 3 is the one the build does not have.** `runtime.rs` hands
      `TenantOps` a `PoolConfig::from_env()` template, which is the
      *control* database's own connection, so a tenant with no cluster
      named currently lands on the control plane's cluster by inheritance
      rather than by configuration.

      **They are configuration levels, not necessarily separate servers:**
      *"someone deploying could use the same cluster for all 3 if they
      chose."* So each level defaults to the one above it rather than being
      required, and a deployment that configures nothing runs console,
      control plane and every tenant database on one cluster. That is the
      ordinary small deployment and it has to stay a no-op to set up.

      This is also what squares the ruling with D-D's *"database/schema not
      cluster"*: the **cluster** may be shared, the **database** may not.
      `same_database` already compares host, port *and* database, so a
      tenant database sitting beside the control database on one server
      passes while the control database itself is refused. That check needs
      no change.

      **What survives untouched:** one tenant per database, the control
      database refused as a target, the host allowlist, the routing lock,
      the fleet view and the custody record. None of them depend on who
      chose the name. **What gets heavier:** the reaper and the usage
      ledger, because a thousand trial tenants is now a thousand databases
      before any of them writes a row.

      **Corrected while writing T9:** this line previously still listed
      T7's R-S2 fleet view and R-S5 advisory lock as open, but the two
      sections directly above it say otherwise (`R-S5 built 2026-09-24`,
      `R-S2 built 2026-09-25: ... T7 is complete`) — left stale when T8's
      entry was written beneath them. **Open under 18d now: T10 alone.**
      T9 is below.

      **T9 built 2026-09-24: the operator surfaces.** §9.6. `ordo restore
      --record --tenant` resolves the tenant's database through `$spacedb`
      instead of trusting `--db` to name it; an unreachable target leaves
      an explicit gap in the usage ledger rather than a row of zeros, and
      no longer stalls the rest of the fleet's metering while it is down;
      and a new `ordo fleet` command joins R-S2's cache with `$spacedb` and
      `ordo_host::tenants` to print each target's version, custody and
      tenants, every tenant with its name beside its id.

      **The fast-forward this increment needed, recorded because it is a
      fact about the worktree and not about the code.** This agent's
      worktree branched at `63ca926`, one commit *behind* `d257e2a` (T8,
      R-S5, R-S2), so `crates/ordo-exec/src/fleet.rs` did not exist on
      disk although the increment brief said it was "built yesterday."
      `git merge --ff-only d257e2a` on a clean tree brought it forward with
      no conflict — worth stating plainly because a worktree silently
      missing a file it is told to read is the kind of thing that looks
      like a defect in the file rather than in the checkout.

      **`--tenant` resolves `--space`'s own value rather than taking a
      second flag.** The plan's R-O1 text speaks of "a tenant id" as if it
      were separate from `--space`, but a tenant's id *is* the space id
      `ordo_host::tenants` mints (`tenants.rs`'s own doc: the id is what
      the door binds unchanged) — a second flag naming the same string
      would be two spellings of one thing, the exact hazard R-O1 exists to
      remove one instance of. So `--tenant` is a bare switch: it changes
      how `--space`'s value is resolved to a database, from "trust `--db`"
      to "ask `$spacedb`." Refused together with `--db` (the two are
      independent namings of where the rows are, and R-O1's own words are
      "two independent chances to name the wrong host" — guessing which
      one the operator meant is worse than refusing).

      **The witness is always the control connection in `--tenant` mode,
      never the resolved target**, matching R-O2: the witness is never
      restored, which is what makes a restore checkable, and a tenant on
      its own perimeter has no witness of its own to be. An unattached
      tenant (no `$spacedb` row) resolves to the control database itself —
      the same database every space with no row is already served from.

      **`$spacedb` is not part of `ensure_platform_tables`.** It has
      exactly one production writer (`prepare_target`) and is read by the
      resolver and, now, by this command; a control database from before
      18d reached it has never had a reason to hold the table. `restore
      --tenant` and `ordo fleet` both call `spacedb::ensure_table` (and,
      for fleet, `tenants::ensure_table` and `fleet::ensure_table`) before
      reading it, the same guard every other reader in this codebase takes
      rather than letting a fresh control database's `relation "$spacedb"
      does not exist` read as a real refusal. Found by the first run of
      `restore_resolves_the_tenants_database_from_its_id`, which is the
      falsification this test earns for free: forgetting the guard is
      indistinguishable, from the failure message alone, from resolving
      through the wrong pool entirely, which is exactly why the test also
      reconnects to the target directly and reads the marker back from it
      rather than trusting the CLI's own exit code and printed line.

      **R-O5's gap needed a new table, `$usage_gaps`, which the plan's file
      list for T9 did not name** (only `operator.rs`, `workers.rs`,
      `runtime.rs`) — added to `ordo-exec/src/usage.rs` instead, folded
      into `USAGE_DDL` the way `bucket.rs`'s `path_style`/`key_prefix`
      widened `$spacedb` after it shipped. A zero in `$usage` and a gap in
      `$usage_gaps` are distinguished by **which table holds the period**,
      not by a flag on one row: a caller reading `$usage` for a gapped
      period finds nothing, which is the honest answer, rather than a
      number that looks measured. The rollup does not need to remember a
      gap happened — `upsert_space`'s window continues from the *previous*
      `$usage` row's `chain_to`, which a gap period never advances, so the
      pass after a target recovers scans the whole gap in one window. The
      table exists for the operator, not for the rollup's own bookkeeping.

      **The unreachable-target defect this closes is bigger than R-O5
      states.** `UsageRollup::run` did not write zeros for an unreachable
      target before this — it aborted the **entire pass** the moment any
      target came back `Err` from `targets()`, so a tenant on a healthy
      target went unmeasured too, for exactly as long as an unrelated
      tenant's cluster was down. That is worse than a zero and worse than
      a gap: it is the metering ledger's own single point of failure.
      Fixed as part of the same change, because a gap mechanism that only
      ever fires after the abort is deleted has nothing to falsify.
      Falsified by reverting to the abort: the reachable side's own
      assertion fails first (`the reachable side must still roll up
      despite the other target being down`), which is the half of the test
      that would have passed against a defect the gap alone does not
      cover.

      **R-O6's six facts are only partly built, and the missing half is
      not a shortcut — it is a category the command cannot honestly
      answer.** `ordo fleet` opens a database connection and prints what
      `$fleet`, `$spacedb` and `$spaces` can say: each target's version and
      when it was last confirmed, its custody, and which tenants are on
      it, every tenant printed with its name beside its id (facts 2, 4 and
      6). Facts 1, 3 and 5 — the quarantine register, the connection
      budget, which credential labels a node has installed — are the state
      of a **running replica**, not of the database: `Spaces::quarantined`
      is a bare `Mutex<BTreeMap<String, String>>` that dies with the
      process that holds it (`spaces.rs`), and a stateless CLI command
      reading durable rows cannot reconstruct what one specific live
      replica currently believes without either dialling every target
      itself (a different, live check, not a report of what is stored) or
      printing a number that is already stale by the time it prints. Those
      three belong on `/health/ready`, which T5 already built the
      quarantine half of; this is recorded as the reason rather than left
      to be rediscovered, and the remaining two facts (the connection
      budget, the credential labels) are `/health/ready`'s own future
      extension, not `ordo fleet`'s.

      **`GET /usage` and `ordo usage` now share one function,
      `usage::render_between`, rather than each calling `between` and
      `render` separately.** Not asked for by T9's file list (it touches
      `ordo-host/src/serve.rs`), but the two surfaces' own doc comments
      already claimed "byte-identical," and adding gaps to one side only
      would have made that claim false without anybody having decided it
      should be. One function is cheaper than two call sites that must be
      kept in sync by memory.

      **Tests: four new, each falsified against deliberately broken
      code.** `restore_resolves_the_tenants_database_from_its_id` — broken
      by short-circuiting the `$spacedb` lookup to always report
      unattached; failed because the marker landed in the control database
      instead of the target, which the test catches by reconnecting to the
      target directly rather than trusting the CLI's own report.
      `restore_refuses_a_tenant_id_and_a_host_together` — broken by
      deleting the refusal; failed with exit 1 and a Postgres "database
      does not exist" error instead of exit 2, since the flag combination
      it was supposed to catch went on to dial a database that was never
      created. `an_unreachable_target_leaves_a_gap_not_a_zero` — broken by
      restoring the old abort-on-unreachable behaviour; failed on the
      reachable side's own assertion, which is the defect described above.
      `the_operator_surface_prints_the_name_beside_the_id` — broken by
      dropping the name from the per-tenant line while keeping the id;
      failed on the substring check, confirming the test reads the actual
      rendered line and not just that a name appears somewhere in the
      output. `cargo test -p ordo-cli -p ordo-host -p ordo-exec --tests`:
      every test binary in the three affected crates passed, 0 failures,
      after these four were added.

      **U3 built 2026-09-23: the pipeline and the export take a
      resolver.** `BlobPipeline` held one `BlobStore` for every space, so
      the write path was the one tier with no per-space seam at all. It
      now holds a `StoreResolver` and resolves per call rather than
      caching, because a cache there would be a second answer to "where
      are this space's bytes" and caching belongs where the row that
      invalidates it lives.

      **Three placements that are arguments rather than conveniences.**
      `put` resolves **after the size cap and before the key release**, so
      an upload to a space whose store cannot be resolved costs no key
      release, no minted handle and no U2 tombstone. `get` resolves
      **once** for the header fetch and the range fetch, because two
      resolutions would permit header-from-one-store and
      body-from-another, which reads as AEAD corruption rather than as a
      misconfiguration. And an export with no `$blobref` rows returns
      **before** resolving, so an unreachable bucket cannot refuse a
      bundle that was never going to read a byte from it.

      **The export test is the one worth keeping.** Its method is a decoy:
      the deployment's store is seeded with a valid, openable object under
      the very same key, sealed for the same entity and field. Reading the
      wrong store therefore does not error, it **succeeds with the wrong
      document**. Falsifying it produced exactly that, which is the silent
      wrong-bundle failure the test exists for, and an assertion that only
      checked for an error would have passed against the defect.

      **The one-store equivalence test asserts on the bucket's view**, not
      on internals: a ledger store records every request behind
      `SingleStore`, and one scripted workload must produce an exact
      request sequence, every entry a fact about the pre-resolver path.
      Falsified by adding a single extra `head`.

      **The done-when could not be literally true and the deviation is
      argued.** "No `Arc<dyn BlobStore>` held anywhere outside a resolver"
      cannot hold while `ORDO_BLOB_STORE` parses to one store, since a
      resolver has to be built from something. It is satisfied for every
      **serving** path; `BlobBinding` is documented as the boot-time
      exception that U5 turns into the default arm.

      **U5 built 2026-09-23: `AttachedStores` and the switch.**
      `ORDO_BLOB_ROUTING` takes `single` or `attached`, refusing an
      unknown value and refusing `attached` with no credential provider,
      mirroring T4's shape and wording on the database side.
      `AttachedStores` reads `$spacebucket`, honours `path_style` and
      `key_prefix`, and resolves the deployment's default store for a
      space with no row. It is where U4's `BucketCredentials::key_pair`
      and U1's two columns finally get a live caller.

      **The end-to-end took a third route, and it is better than the two
      offered.** A second real bucket could not be used: there is no
      local S3, the contract suite skips unless `ORDO_BLOB_S3_*` is set,
      and B-C4 forbids `fs:` as an attachment target. Testing at the
      resolver with a stubbed store would have proved only that a branch
      was taken, not that the columns were **honoured**, because a double
      behind the resolver never sees them, and a resolver that read
      `key_prefix` into a field and never onto a request would pass. So
      the second bucket is a loopback HTTP listener recording the request
      line, reached through the whole production path: row, credential
      label, `S3Config::new`, `S3Store`, SigV4, socket. The assertion is
      the request line itself, `HEAD /acme-files/ordo_objects/tenant-a/<hex>`.

      **B-C4 was verified intact rather than taken on trust.** No
      attachment in any test names a directory; every one travels
      `S3Config::new`, which has no `fs:` form. The listener is reached
      over `http://localhost:<port>`, which `S3Config::new` already
      admitted on loopback and only on loopback, and the diff shows that
      rule untouched. `FsStore` appears solely as the deployment's own
      store, which is what `ORDO_BLOB_STORE=fs:` already is under the dev
      flag.

      **Five things the plan had wrong about the code.**
      1. **`StoreResolver::resolve` had to become async.** An attached
         resolver answers out of `$spacebucket`, which is a query. The
         three alternatives are each worse and are argued out at the
         trait: `block_on` in a sync method is a hang; a periodic
         snapshot serves a newly attached tenant from the wrong bucket
         until it refreshes, which is the exact failure attachment
         exists to prevent; and a lazily-resolving `BlobStore` wrapper
         collapses "this bucket is unreachable" into "that object is not
         there". `PoolResolver::resolve` is already async for the same
         reason, so the two halves are now one shape.
      2. **`key_prefix` cannot be honoured from `bucket.rs`.** The key is
         built in `S3Store::uri` from `object_key`, so the ask lands in
         `s3.rs`. Added to `S3Config` and prepended in `uri`,
         deliberately **not** in `object_key`, which is what `FsStore`
         and every outside caller mean by "the key".
      3. **U5's named test collides word for word with T4's**, in the
         same file. T4 keeps the spelling; this one is
         `a_deployment_that_attaches_no_bucket_behaves_identically`.
      4. **The `attachBucket` census line goes stale one increment early.**
         §11 assigns it to U6, but the moment `AttachedStores` exists the
         unconditional "no resolver reads an attached bucket yet" is
         false on an `attached` node. Now per-mode; the op stays
         unregistered in both, only the stated reason differs.
      5. **`AttachedPools`' cache argument does not carry over.** That
         one caches a hit for the life of the process and earns it from
         `spacedb::attach` refusing to move an attached space.
         `bucket::attach` has no such refusal, since `Replaced` is a
         documented custody conversion, so a hit cached forever would
         leave a node writing into a bucket the owner had detached. Both
         arms are bound by one TTL, with `forget()` for the node that
         attached.

      **One mutation could not be falsified cleanly, and the reason is
      inherited.** Making an unknown `ORDO_BLOB_ROUTING` fall back to
      `Single` makes the host **boot and serve**, so the harness's
      `run_to_completion` never returns and the test hangs rather than
      failing. T4's `an_unknown_routing_value_refuses_at_boot` has the
      identical shape and hazard. The property was reached another way,
      through the message mutation, and the harness wants a timeout,
      which is 17ad's neighbourhood rather than U5's.

      **`std.space.attachBucket` is still unregistered, deliberately.**
      U5's done-when is the resolver; U6's is the op with its six
      refusals, its credential probe and its custody event. A registered
      op with none of those would accept an endpoint off the allowlist
      and record nothing.

      **U7 was specified to ship with T5 and did not**, so the reason is
      recorded rather than left as drift. The plan's argument is that T5
      builds the list from four sources and a fifth added later means
      building the assembly twice. That does not apply to what was built:
      the assembly is a map merge, so a further source is one more insert
      loop rather than a second assembler. U7 remains next on the storage
      side and nothing about it has to be redone.

      **The plan oversold T1's gap, which is worth recording.**
      `routed_engine`'s substantive behaviour already had passing tests
      before T1 (`a_space_on_its_own_database_has_its_members_there`,
      `a_target_that_cannot_be_read_is_reported_and_only_its_spaces_lose_members`).
      Only the `door_for` wiring was missing. The two tests the plan names
      are additive pins of the same properties at the same seam, and the
      earlier ones were left alone rather than renamed.

      **D-B ruled 2026-09-21, and it moves off the recommendation.** The
      question was what happens to a tenant whose database is behind the
      activated artifact. The recommendation was unconditional quarantine
      (R-S4), on the argument that serving that tenant the older program means
      one deployment holding two artifacts. The owner ruled: *"It should be
      configurable per tenant. This would be a deployment or possible
      maintenance window contract."* So quarantine is the **default**, for a
      tenant with no contract, and a tenant whose deployment or maintenance
      window has not opened is served the artifact its database matches until
      it does. R-S4 in the plan now carries this.

      **What that buys, named honestly:** the multi-artifact build the
      recommendation existed to avoid. `Deployment` holds one
      `RwLock<Arc<Loaded>>` swapped whole, and everything under it assumes one
      request resolves the same artifact as every other. Contracted lag means
      the previous `Verified` stays loaded and the resolver picks per tenant.
      That is a real increment and it is not in T0 to T10.

      **D-B2 is the open question this raises**, and it is on the board: how
      far back may a contracted tenant lag, and what ends a contract?
      Recommendation: one version back and never two, an expiry on every
      contract after which quarantine applies regardless, and an apply that
      names at activation time which contracted tenants a destructive change
      would break. Until it is ruled, only the default quarantine is built.

      **Answered in part 2026-09-21, and it corrected D-B's wording.** The
      owner: *"I think one version is fine, I'd rather it be flexible, and
      contract I mean app-dev to customer. But yeah one version makes sense
      ideally but there could be broken situations where you may want to
      revert down the chain."*

      1. **The contract is application-developer to customer**, not
         platform to tenant. D-B was recorded the wrong way round. Ordo
         supplies the knob and enforces it and never defines the policy,
         and the principal who sets it is the deployment owner, who is the
         app developer.
      2. **Reverting is not lagging, and only lagging is expensive.**
         Rolling the deployment back down the chain is
         `registry::activate` naming an older version. `$artifacts` is
         never deleted from and the `$activations` foreign key makes
         deleting an ever-active artifact impossible, so revert depth is
         unbounded today and nothing here changes that. Lag is two
         versions serving *at the same instant*, which is what needs a
         second `Loaded` and a per-tenant resolver. A deep revert moves
         every tenant together and costs the lag bound nothing, so the
         broken situation the owner is weighing does not argue for
         widening the window.
      3. **What actually limits a deep revert is the load gate**, which
         refuses a program whose shape no longer matches a migrated
         database. That is unchanged by every option here and is the
         constraint a business requirement would really meet.

      **Settled 2026-09-21:** *"One version back + ability for snapshots
      (for requesting customers on the app dev side) and ensure step by
      step downgrading in schema."* The lag bound is one version, so a
      deployment holds two artifacts at most and two behind quarantines
      regardless. Going further back is a **snapshot**, offered on request
      through the app developer rather than generated for every tenant,
      and restoring one across versions walks the chain **one step at a
      time**. That clause is 11-10b's shape, and it obliges something that
      does not exist: the diff engine plans forward, so each step must
      state its own down path or state that it has none.

- [ ] **18p · Object storage per space, and the BYO-bucket custody choice.**
      The owner, 2026-09-20, alongside the per-tenant database pass: *"While
      we're doing this we need to make sure the same per tenant / deployment
      S3 configuration exists."* **Requirements gathered 2026-09-20 in
      [`plan-tenant-storage.md`](plan-tenant-storage.md)**, mirroring
      `plan-space-routing.md` §9 to §11 section by section so the two build
      together. A separate document rather than a twelfth section of the
      database plan, because it cross-references `plan-blob.md` as heavily as
      it cross-references the database plan.

      **U0 and U1 built 2026-09-21, in lockstep with T0 and T1.** U0 is
      tests only, pinning three properties that held by accident, each
      falsified first against deliberately broken code: a store error
      never reaches the caller (proved by making the door format the
      underlying error in, which surfaced `blob store transport: the store
      is not answering` to an HTTP client), the object key is the space
      and the handle and nothing else, and two spaces on different stores
      do not share keys (proved by pointing both stores at one directory
      and watching the second answer for the first's object).

      **The second of those moved from a unit test into the shared
      contract suite**, so both the filesystem and S3 bindings run it
      rather than one. That change had a consequence the plan's file list
      missed: `tests/contract.rs` asserts how many contract cases ran, so
      without bumping it the promoted case would have been added and never
      executed, which is the exact failure the promotion existed to
      prevent.

      **U1 closed the credential leak and added the two columns.** The
      attach response no longer returns the credential label, and
      `$spacebucket` grew nullable `path_style` and `key_prefix` on
      `$secevents."about"`'s precedent, since there is no platform-table
      migration machinery. `identity()` includes both, so restating only a
      key prefix is a real `Replaced` rather than a silent no-op, and a
      hostile prefix is refused on `space_prefix`'s charset rather than
      sanitised.

      **The backwards-compatibility test is the one that mattered and it
      was built correctly**: it inserts a row naming only the original
      eight columns, never mentioning the new two even as `NULL`, so it
      exercises a genuinely pre-U1 row rather than a new row with the
      fields unset.

      **U1's first item was already built** on 2026-09-21 morning, when an
      S3 credential leak was found and fixed during the 18p requirements
      pass. The plan's `an_s3_config_never_prints_its_credential` is the
      same property as the existing
      `an_s3_config_redacts_its_credentials_when_formatted`, and no
      duplicate was added.

      **The gap is 18d's gap one tier over.** The seam, the table and the op
      are built by 17g (B1 to B5) and the shipped binary wires the
      single-store variant. `runtime.rs:1524` builds
      `ordo_blob::SingleStore::new(store)` from the one `ORDO_BLOB_STORE`, and
      `runtime.rs:1755` registers `std.space.attachBucket` on the
      unavailability census unconditionally, with the reason *"no resolver
      reads an attached bucket yet (every space resolves to the one
      ORDO_BLOB_STORE)"*. `platform_specs` states the policy: the op is
      registered **never, yet**, because *"registering it would record
      attachments that nothing honours"*. Nothing populates
      `BucketOps.credentials`, so no operator can install a bucket credential.

      **One gap the database side does not have.** `BlobPipeline` holds
      `store: Arc<dyn BlobStore>` rather than a resolver, so the serving path
      has no per-space seam at all, and `export::write_blobs` reads through the
      same field. On the database side `Spaces::pool_of` is already the seam
      every request-path checkout passes. Giving the pipeline and the export a
      resolver is the first real build (plan U3), not a wiring change. What is
      already per-space and correct: `blobgc::sweep`, `reseal::move_objects`
      and `erase_space` all take a `StoreResolver` and resolve per space today.

      **The credentials answer** (plan §3). Object storage takes the **same**
      provider as the database, not its own. The six properties that matter
      (a stable label, material in a mounted file, rotation by changing the
      file, a boot refusal on a group- or world-readable mode, a health
      surface naming labels and never values, and one checkable list per node
      for 18e) are about the operator rather than about Postgres, and two
      providers would be two rotation procedures on one node. The `dir`
      provider grows one shape: a label resolves to a **file** (a password,
      empty meaning passwordless) or to a **directory** named by the label
      holding `access_key_id` and `secret_access_key` separately. Region,
      endpoint and path style stay attachment columns, because they address
      the target and are not secret. A role rather than a key pair is not v1:
      it is an STS call, a third signed header and an expiry, all additions
      inside `s3.rs` that touch neither `$spacebucket` nor the resolver.

      **Where storage genuinely differs from databases, and the plan says so
      rather than assuming.** `BlobStore` has **no `list`**, deliberately, so
      the reference set is `$blobref` and the platform cannot answer any
      question about a bucket without that tenant's database. An unreachable
      bucket refuses the blob routes rather than quarantining the tenant,
      because every request that touches no byte can still be served. A blob
      write is object-first and reference-second, so a half-succeeded upload
      leaves an object nothing names and no listing can find. And bytes stored
      is measured from `$blobref` in the tenant's database, never from the
      provider, so the 2026-09-21 ruling applies to the database being
      unreachable and **not** to the bucket being unreachable.

      **Four findings the read produced.** (1) Nothing records which store an
      object was written to, and the attach op promises a split population
      (`existingObjects`), so `blobgc` deletes against the wrong store,
      succeeds because an absent-key delete is idempotent by design, removes
      the reference and leaves bytes nothing can ever name; `erase_space`
      reports a deletion that did not reach them. (2) `attachBucket` returns
      the credential **label** to the caller, who is a tenant. (3) `S3Config`
      derives `Debug` with `access_key_id` and `secret_access_key`, which is a
      `{:?}` away from a credential in a log line. (4) `S3Config::new`'s
      loopback exception admits `localhost`, `127.0.0.1`, `::1`, `*.localhost`
      and the bare name `minio` over plaintext HTTP, and an
      `https://169.254.169.254` endpoint passes outright, so an attachment
      endpoint chosen by data reaches the pod's own network.

      **Eleven increments, smallest first** (plan §11), U0 to U10, and **four
      must not ship alone**: the credential provider with **T3**, the failure
      modes with **T5**, the usage work with **T9**, and the sweep's runner
      **after T2**. U2, which turns a half-written upload into an ordinary
      tombstone, is independent of the database plan entirely and makes a leak
      that exists today on every deployment collectable at all, so it is the
      one worth landing whatever else happens, and it must precede the sweep's
      runner for that to matter.

      **Eight owner decisions** (plan §12). The two that most need a ruling:
      whether an attach **refuses** a space that already holds objects, on
      `erase::rows_held`'s precedent, rather than letting the population split
      (the recommendation is refuse for v1, with a `store` column on
      `$blobref` as the right thing later, since it is what a move needs); and
      whether `std.space.delete` under `OwnPerimeter` **deletes the tenant's
      objects**, which `erase_space` does unconditionally today while the
      disclosure the tenant was shown says the objects are theirs and removing
      them is theirs to do. D-S3 also closes 17g's open `blob.gc` cadence
      decision, recommending one deployment-wide setting.

      **U8 built 2026-10-04: the storage sweep has its runner** (commit
      44b2b67). `workers::BlobGc` runs leader-only on one deployment-wide
      cadence, `ORDO_BLOB_GC_SECS` (default 3600), walking every database
      the resolver names and sweeping each space through the store it
      resolves to. An unreachable database or store is counted
      (`ordo_blob_gc_uncollected_total{cause}`), not fatal. Registered when
      `ORDO_BLOB_STORE` is set. Tests `crates/ordo-host/tests/blob_gc.rs`.
      Board: `st-gc-cadence`.

      **U6 built 2026-09-24: `std.space.attachBucket` is registered, with
      its refusals, its probe and its record.** The op has existed since
      17g B5 and no deployment could reach it. It is now registered when
      `ORDO_BLOB_ROUTING=attached` **and** a store and a credential
      provider are configured — one condition, `runtime::buckets_wired`,
      read by `platform_specs`, by the census and by the wiring, so the
      `PlatformOps` invariant (no registered op without state, no state
      for an unreachable op) cannot drift across three spellings.

      **The refusals, in the order `prepare_target` refuses in**, because
      it is the same problem one tier over: everything decidable from the
      parameters, then the reads, then the probe, then the write. Two were
      already built (a credential label nobody installed, answered
      `not_found`; a target shape `S3Config::new` rejects, answered
      `param_type_mismatch`). Five are new: the endpoint is on the
      operator's allowlist; a plaintext endpoint needs a dev flag; the
      target is not the deployment's own default bucket; the space holds no
      `$blobref` rows; no other tenant is in that bucket.

      **D-D is the ruling that reshaped this increment, and the reading is
      stated because it is a judgement.** *"1 tenant 1 database. same for
      buckets. database/schema not cluster."* The storage analogue is that
      the **bucket** is the unit: many tenant buckets behind one endpoint,
      one account or one gateway stays the ordinary case, and that is what
      keeps the allowlist a list of hosts and a tenant costing a bucket
      rather than an account. **A shared bucket with a per-tenant
      `keyPrefix` violates the ruling and is refused.** The counter-reading
      is real and is worth writing down: a prefix looks like what a schema
      is to a database, a namespace inside the container. What decides it
      against that reading is who enforces the boundary. A Postgres schema
      is enforced by the server's own privilege system, so "database/schema
      not cluster" names a line the server draws; an S3 key prefix is
      enforced only by an IAM policy on the tenant's side that **D-S7 says
      outright the platform cannot enforce**, and §10 B-X1 already records
      that two tenants in one bucket *share one credential*. A prefix says
      where objects are named; it grants nothing and withholds nothing. So
      `bucket::occupants` keys on the endpoint host and the bucket, and
      `key_prefix` is deliberately not part of the key.

      **The narrower reading was not built first, unlike on the database
      side.** T8 built "refuse only where someone declared an own
      perimeter" and had to collapse it into one `DatabaseOccupied`; U6
      built the general rule from the start, custody not consulted, and the
      test asserts all four custody arrivals plus the prefix case rather
      than the one the plan's B-X3.2 framed.

      **What the ruling does *not* touch, stated so nobody assumes it.**
      The **default** path is untouched: every space with no `$spacebucket`
      row still resolves to the one `ORDO_BLOB_STORE`, so unattached
      tenants still share one bucket separated by the `<space>/` prefix
      alone. That is the exact twin of `db-default-tier` on the board and
      it is not assumed either way here.

      **The endpoint allowlist is tied in with the database one, as D-S6
      asked** (*"should be tied in with the db allowlist setup. implication
      really is if i defined it on my platform it is allowed"*). **One list,
      one type, one constructor:** `provision::HostAllowlist` is what both
      tiers check against, and `bucket_ops` takes it as an argument exactly
      as `provision::Target` does — read from the environment once at the
      edge, never inside the check, so the tests that need both answers at
      once can have them. `from_env` now reads **`ORDO_TENANT_ATTACH_HOSTS`
      and `ORDO_PG_ATTACH_HOSTS` and unions them.**

      **Why a union and not a rename, and not a precedence.** A rename was
      the first instinct and is wrong: `ORDO_PG_ATTACH_HOSTS` is already on
      the decisions board as *"one new setting an operator must know
      about"*, so renaming it would make a board entry the owner has read
      false. A precedence is worse than either: an operator who added the
      new variable would silently turn the old one off, and the hosts that
      vanished would be the ones their database tenants were already
      attached to. A union only ever grows, which is the one direction that
      is safe to do behind an operator's back, and it is literally what the
      ruling says — one set of hosts the operator declared, allowed for
      either tier.

      **The loopback exception moved behind its own dev flag**, D-S6's
      second half. `ORDO_DEV_UNSAFE_BUCKET_ENDPOINT=1` is what lets an
      attachment name a plaintext endpoint; without it, the `http://` form
      `S3Config::new` admits for `localhost`, `127.0.0.1`, `::1`,
      `*.localhost` and the bare name `minio` is refused at the door. A
      **separate** flag from `ORDO_DEV_UNSAFE_BLOB_STORE` on that
      variable's own stated reasoning: they unsafe different things, and a
      self-host laptop whose tenant bucket is a local MinIO is not a
      deployment that agreed to put customer files on a filesystem with no
      durability story. An `https` loopback endpoint is **not** gated —
      it is TLS, the credential is not on the wire, and the allowlist
      (which denies by default) is what stops it reaching the pod's own
      network. Gating it too would be a second control over one fact.

      **The probe is a `head` and never a `list`** (D-S7), so a credential
      scoped the way §10 B-X2 publishes passes it. `NotFound` is the
      passing answer and that *is* the question: S3 answers 404 for an
      absent key when the caller holds `GetObject` and 403 when it does
      not. It runs after every refusal, so a target that was never going to
      be accepted costs no network call, and **before the row is written**,
      which is what a test asserts by name. Its two failures answer with
      two classes because an operator acts on them differently: a bucket
      that refused the credential is 400 and never lands until somebody
      changes a policy; a bucket that could not be reached is 503.
      Neither message carries the store's own text (§10 B-X4.4).

      **`bucket::store_config` was lifted out of `AttachedStores::build`**
      so the probe and the resolver cannot disagree. A probe that assembled
      its own configuration would be proving a claim about a target the
      resolver may address differently — a different `path_style`, a
      different prefix — and would pass while every upload failed. One
      function, two callers, and a test asserts the probe was handed the
      bucket and the prefix the row states.

      **`BucketOps.credentials` changed from `Vec<String>` to
      `Arc<dyn BucketCredentials>`, and that fixed a real defect.** The
      label list and the material were two answers to one question, and the
      list (`Dir::labels()`) holds **Postgres password labels too**: an
      attachment naming one passed the door's check and then failed when a
      store was built, at the first upload. Asking the provider is one
      lookup, it answers "not installed", "not a bucket label" and "not for
      you" identically — which is the non-enumerating refusal §10 B-X3 asks
      for — and what comes back is exactly what the probe needs.

      **A defect this increment introduced and then found: D-S2 broke the
      retry.** `objects_held > 0` refuses, and a client retrying an
      attachment its tenant had since uploaded to would be refused, so the
      identical call would stop landing the moment the attachment started
      being used — the opposite of the idempotency the whole op is keyed on.
      Found by reasoning about the retry path while writing the test, not
      by a failure. An attachment identical to the one standing now
      short-circuits to `unchanged` **before** the three state-dependent
      checks (objects, occupancy, probe) and **after** the four
      parameter-only ones, because a retry should not become a way to learn
      that an endpoint is permitted. The D-S2 test asserts both halves: a
      genuine change over existing objects is refused, and a retry over
      existing objects is not.

      **`objects_held` reads the space's own database, not the control
      one.** `$blobref` lives where the space's rows live, so asking the
      control database about a tenant attached elsewhere would answer
      "no objects" for a space with a full bucket — which would make
      "have a database of your own" a way past D-S2. `BucketOps` therefore
      takes the deployment's one `PoolResolver`, the same handle the exit
      ops take and for the same reason.

      **The record: both security streams, and the detail rides the action
      string.** §10 B-X6 asks for both halves and `ordo_secevent::route`
      sends one observation to exactly one scope, so both halves are two
      observations and cannot be one. The tenant's copy names the member
      who asked; the deployment's names the platform and carries the space
      as the event's *object*, which is how one operator listing says which
      tenant it was. Everything the record must carry goes in the
      `CustodyAction` action string — the target's identity (seven fields,
      in which `credential` is a label), the occupancy, and the verb —
      because that variant is hash-chained under a tag and widening it
      would change the encoding of every event already written. A test
      asserts the material never appears.

      **What D-F cancelled and what it did not.** D-F is about the tenant's
      own **chain**: no founding-group entry, and `plan-space-routing.md`
      already records that `the_attachment_fact_is_in_the_tenants_founding_group`
      should not be written. There is no command group here and never was —
      this op writes no entity row, which is `plan-export.md` §5.1's
      reinterpretation of "command-logged". The tenant's **security stream**
      is a different record and B-X6's argument for it stands. **This is a
      reading and it is flagged for the owner**, because D-F's stated
      rationale (a tenant showing from their own records where their data
      went belongs to the app) is the same rationale B-X6 gives for the
      tenant-stream half. Keeping it preserves behaviour this op has had
      since 17g; removing it is a one-line change if the owner wants the
      narrower reading.

      **A new wire class, `bucket_refused`, 400.** Added to the enum,
      `as_str`, `ALL`, `executor_raisable`, `status` and a row in
      `api-engine-design.md`, which `inventory_is_closed.rs` enforces. Its
      own class rather than `tenant_refused`, although the shape is
      identical: an operator's alerting keys off the class, and
      `tenant_refused` says a tenant was not created, which is not what
      happened. One class for the whole set on `apply_refused`'s reasoning,
      with the rule named in the message **except** the allowlist, whose
      message carries one reason and no detail.

      **Deviations from §11's U6 text, each with its reason.**
      1. **"The six refusals" is seven checks, and one of them is the dev
         flag.** §11 counts B-X3's six; the plaintext gate is D-S6's second
         half and is not one of them, so the code has both.
      2. **"On both streams" is read as both *security* streams.** §11 says
         the custody event lands "on both streams"; D-F cancels the chain
         and not this. See above.
      3. **`the_custody_event_reaches_the_tenants_stream_and_the_deployments`
         keeps the plan's name**; the other named tests are unchanged
         except `a_second_tenant_on_an_own_perimeter_bucket_is_refused`,
         which became
         `a_second_tenant_in_one_bucket_is_refused_whatever_the_custody_says`
         because D-D made the name describe a narrower property than the
         code has.
      4. **The plan's file list is missing three.** `ordo-api/src/error.rs`
         and `docs/api-engine-design.md` for the new class, and
         `ordo-exec/src/bucket.rs` gained `all`, `occupants`,
         `endpoint_host`, `objects_held` and `store_config` — §11 named
         that file but for the resolver rather than for these.
      5. **`AttachedStores::forget` still has no caller.** U5 built it for
         "the node that has just attached", and wiring it means plumbing
         the concrete resolver type into `BucketOps`. It is not in U6's
         done-when and the 30-second TTL already bounds the staleness U5
         argued was acceptable, so it is recorded as owed rather than
         smuggled in.

      **Two things the owner should look at rather than the next
      implementer.** (a) **D-S2's answer has a carve-out that is not
      built**: *"yes unless everything matches i.e. a 1 for 1 migration
      would be indistinguishable."* The refusal is built; the exception is
      not, because proving a 1-for-1 migration means a `head` per
      `$blobref` row against the new bucket (there is no `list`, by design)
      and neither the cost bound nor what happens to a partial match is
      specified. (b) the tenant-stream half of B-X6, above.

      **Tests: 10 new (7 through the real door in
      `tests/attach_bucket.rs`, 3 as units), 2 existing ones amended, and
      every one falsified.** 20 deliberate breaks were applied one at a
      time, each confirmed to fail the specific test naming it and then
      reverted: the own-bucket check, the occupancy check, the allowlist
      check, the allowlist refusal made distinguishable, the dev-flag gate,
      the probe not run, the probe moved after the write, the D-S2 check,
      the retry short-circuit, each half of the two-stream record, the
      identity dropped from the action string, occupancy hardcoded,
      occupancy read before the attach rather than after, the allowlist
      union turned into a precedence, the own-store comparison moved from
      the host to the URL, the two probe classes collapsed into one, the
      probe refusal passing the store's text through, endpoint spellings no
      longer folded, and the op registered unconditionally.

      **One property is proved by a double rather than against S3**, and
      the limit is recorded rather than glossed. The probe is the only part
      of the op that opens a socket, and there is no local S3; U5 reached
      its own end-to-end through a loopback listener, which proves the
      *resolver* signs correctly. What the double proves here is everything
      about the probe that is this op's rather than S3's: that it runs, that
      it runs before the write, that it is handed the resolver's own
      configuration, and that its two failures map to two classes. What is
      **not** proved locally is that a real S3 403-versus-404 lands where
      `HeadProbe` puts it; that is three lines of `match` and it is stated
      here as the gap it is.

- [ ] **18b · Domain packages.** The owner: "add it to post-deploy
      roadmap." The settled core (package-qualified names, artifact-hash
      identity, upgrade-as-deploy, scope-drift-kills-grants) waits intact
      in `domain-packages.md`; nothing rots by waiting because its
      dependencies (deploy identity, grant pinning, schema evolution) are
      the parts that already shipped.

- [ ] **18f · Generated UI, the surface half (14c).** The owner, `uigen`
      (2026-09-16): "Lets build this later on - add it to the post-deploy
      roadmap. At that point all the system bits will be settled." **What
      already shipped:** 14a, the interface descriptor at
      `ordo.interface/2` with catalog scoping; and 14b, `@ordo/react`, the
      headless runtime that derives every rendering decision from the
      descriptor with no op-name heuristic. **What this owes:** surface
      descriptors, which are the interface descriptor plus `label`,
      `desc`, `order`, `choices` and `danger` (`ui-gen.md` §1). All five
      are marked inert in DL §10.3 and the lexer carries none of them, so
      the descriptor emits none; the recommendation on the row was to lex
      the five annotations rather than add a sidecar file, because an
      annotation describes one declaration and must be versioned, diffed
      and refused with it. The keywords are already reserved, so the cost
      of the words has been paid. Waiting is what the owner asked for and
      it costs nothing: a renderer that trusts an always-null `danger`
      would be worse than one that has none.

- [ ] **18g · The policy grammar.** The owner, `policy-grammar`
      (2026-09-16): "Take reccomendation", which was *mark them proposed now,
      build the grammar when policies are actually wanted*. **What was
      found:** `policy` is lexed (`lexer.rs`, `token.rs`) and there the trail
      ends — no parser arm, no AST node, no consumer in `ordo-schema` or
      `ordo-plan`. Every policy block the language documents is prose:
      `spaces`, `commandlog`, `vault`, `llm-egress`, `retention`,
      `compliance`. **What this owes:** lex and parse a policy block, an AST
      node, validation, and the descriptor projection, after which mirror
      scope (C7's open half) and retention's nightly sweep become ordinary
      consumers rather than special cases. **Why it is its own item:**
      building `policy mirror` alone would invent a spelling for one policy
      and leave the grammar shaped around the narrowest case, with five more
      blocks arriving later to live with it. Every keyword is forever, so
      this gets scheduled rather than smuggled in under a cache increment.

---

- [ ] **18e · Moving a live space between databases.** The owner,
      `routing-migration` (2026-09-12): "Make sure the routing can handle
      it, and add the full implementation to the post release roadmap."
      **What routing already carries:** the per-space view is cut again
      when a space's pool changes (S1), and an attachment is cached for a
      process's life only because S2 refuses to change one
      (`AttachRefusal::WouldMove`), which is the one place a move has to
      change. **What the build owes:** the chain must not fork; the undo
      horizon must survive; blob references must be repointed or the
      objects moved; the cutover must look atomic across two databases
      that no transaction spans; every node's cached attachment must learn
      of it; and both parties must agree, since a change after creation is
      a two-party act (Q-3, 2026-09-13).

- [ ] **18h · Multi-device sync for vault spaces.** The owner, `vault-sync`:
      "Okay execute form 2 as per your suggestion." In a client-first vault
      the server holds ciphertext it cannot read, so every change is applied
      by a device that holds the key, and nothing pushes a change to a device
      that is absent. Form 2: devices sync through the F7 oplog (end-to-end
      encrypted, signed records the server orders without reading), plus a
      content-blind broadcast ("space X advanced to sequence N") over the
      invalidation bus for liveness. Device enrollment is the one job a
      direct device-to-device handoff keeps. Recorded in `f7-vault-plan.md`
      §5.4. Go given, queued; no code yet. Filed 2026-09-18 because the board
      carried it and this checklist did not.

- [ ] **18i · Quotas on writes.** The owner, `write-quota`
      (2026-09-19): "Take reccomendation - Add the remainder to the post
      deployment roadmap." v1 quotas (17n M2) gate requests and rows
      returned, which the door counts as they happen. Writes are counted
      from the command log at rollup, so a write quota enforced from
      `$usage` would notice an overrun up to one rollup period late. This
      item is a door-side write counter that lets a write quota refuse on
      time. Writes are measured and reported from v1 regardless.

- [ ] **18j · Rotating the deployment's own master key (SMK).** Split
      out of 11-11 on 2026-09-19: K1 to K3 rotate a space's data keys and
      re-seal every ciphertext onto the new epoch. The **master** key that
      wraps them is rotated by `ordo keys rewrap-master`, which rewraps in
      place and is proved not to move a digest, so there is no pass to run
      and no epoch to retire. What is not built is the rest of a master
      rotation as an operation: a schedule, a record of when it last
      happened, and the custody event that says so. Named here so 11-11's
      text stops implying K covers it.

- [ ] **18k · Inferring entities from an existing database.** The owner,
      2026-09-20: pointing Ordo at a live database and having it infer a
      schema into entities is not wanted now, and goes to the
      post-deployment roadmap. It would need more than introspection: a
      column's type does not say whether it is sealed, what guards it, or
      which entity owns a join table, so the useful version involves an
      ontology the developer confirms rather than a reader that guesses.
      Migration today is: stand an Ordo database up beside the old one,
      create the schema from the `.ordo` definition, import the rows
      through the write protocol, cut over.

- [x] **18l · Applying a schema over the wire.** Filed 2026-09-20 with
      the owner's constraint that the apply belongs to the service; **built
      the same day** as `std.deployment.apply`
      (`crates/ordo-host/src/apply_op.rs`).

      **A wrapper, not a second implementation**, which is what the ruling
      required. The op adds exactly three things `schema_apply` does not
      have: the *sequence* one `ordo apply` is (the target's state, then
      the artifact, then the activation), which had been written out inside
      the CLI command and lived nowhere a server could call it; a
      structured `Report`; and serialization. `ordo apply` is now a caller
      of the same `apply_op::run_program`, so the hosted control plane and
      the self-hoster's CLI are one migrator by construction.

      **The namespace.** `std.deployment.*`, not `std.space.*`, and the
      name is the gate restated: every other platform op acts on the space
      it is bound to, and this one acts on the deployment that holds every
      space.

      **Who may: three checks stack.** (1) `platform_ops::owner_only` lists
      it, so `engine_from_members` grants it to `Role::Owner` alone. (2)
      `require_step_up` covers every platform op, so a capability token can
      never call it however its allowlist reads. (3) The handler refuses
      unless the bound space is the **deployment's own** — the one the
      binary was configured with. The third is the one neither of the
      others can express, because the access profile is deployment-wide
      policy installed in *every* space: a tenant space's owner holds
      `Role::Owner`, passes 1 and 2, and is still a space member. Without
      check 3 every tenant owner on a shared deployment could replace its
      program.

      **Why check 3 compares a name.** `ordo_exec::erase::is_system_space`
      is the one answer to "is this space the platform's own" (17v) and is
      the obvious spelling — and it does not work, because
      `ordo_api::router::well_formed_space`'s alphabet is ASCII
      alphanumerics, `_` and `-`, so a `$`-prefixed space id **cannot be
      bound at the door at all**: `missing_space`, 400, before any op is
      reached. Until that alphabet admits the prefix, an op gated on
      `is_system_space` would be an op nobody could call. Recorded here
      because it is a constraint on 17v as much as on this item.

      **The attested actor.** The actor written into `$migrations` and onto
      the security stream is `$caller`, router-inserted from the access
      decision (a reserved key no op parameter can shadow, R-G13). There is
      no `--as` over this door and the CLI refuses the flag beside
      `--host`. The `$migrations` row carries the principal; the security
      event carries the principal **and** `std.deployment.apply`, so an
      auditor can tell an attested apply from one made by whoever held the
      database password.

      **Serialization.** An advisory lock at transaction scope on a
      dedicated connection, keyed by an FNV-1a of the string
      `"$migrations"` — named by the thing it guards, computed in Rust so
      it needs no round trip for an OID that does not exist on a first
      apply anyway. Taken **before the first read of the schema**, because
      what must not interleave is the *decision*: two applies that both
      read an empty target and both call `create_schema` are two correct
      executions of a sequence that raced at its first read. Transaction
      scope rather than session, so a cancelled apply cannot leave a lock
      nothing releases; an advisory lock rather than an "in progress" row,
      because the row's failure mode is that a crashed apply wedges the
      deployment and the lock's is that it releases. A waiter is bounded at
      thirty seconds (`SET LOCAL lock_timeout`) and then refuses.

      **The wire codes**, in `api-engine-design.md`'s inventory and
      therefore under both directions of
      `ordo-api/tests/inventory_is_closed.rs`: `apply_refused` (409, with
      the class as the first word of the message in
      `schema_apply::Refused::class`'s own spelling — one code for the
      whole taxonomy, because increments 3 through 5 extend it and a code
      per class would make every new refusal an amendment to a closed
      inventory), `apply_in_progress` (409, and deliberately not
      `serialization_conflict`, which is a transaction the engine retries
      for you: an apply must not be retried automatically, because the
      apply holding the lock may be changing the schema this one planned
      against), and `not_activated` (409, its own class because the
      artifact *is* stored and the next step is `ordo migrate --apply`
      rather than a recompile). `schema_drift` is reused for the
      registry-over-no-tables case.

      **Two findings the tests forced.** An executor may not say
      `access_denied` — it is the door's word and
      `ErrorCode::executor_producible` does not list it — so the
      wrong-space refusal would have reached a caller as `internal`, a 500
      for a refusal that is nobody's failure. It is `not_found` now, which
      is also the more honest answer: the op does not exist in a tenant
      space, and no membership change there will make the call succeed. And
      a program that does not parse was landing as `Internal`, a 500 for a
      typo; it has its own class, `program_does_not_compile`, under
      `apply_refused`.

      **The tests** (`crates/ordo-host/tests/deploy_apply.rs`, 11, through
      the real router): `a_deployment_owner_applies_and_the_schema_lands`;
      the three refusals that are the point of the file —
      `a_manager_of_the_deployments_own_space_is_refused`,
      `an_editor_of_the_deployments_own_space_is_refused`, and
      `a_space_owner_cannot_apply_to_the_deployment` (`not_found` bound to
      their own space, `access_denied` bound to the deployment's, nothing
      created either way); `a_capability_token_cannot_apply_however_its_allowlist_reads`
      with a token naming this very op and claiming fresh presence;
      `the_actor_recorded_is_the_principal_the_door_proved`, asserting both
      the `$migrations` actor and the security event's op;
      `two_applies_serialize_rather_than_interleave`, whose assertion is
      that `$migrations` holds exactly **one** `create` row and that one
      call created while the other matched;
      `the_lock_is_held_while_an_apply_is_running`, which also proves the
      wait is bounded by returning rather than reaching a timeout; and
      `a_gate_blocking_program_is_stored_and_refused_by_name`,
      `staging_a_gate_blocking_program_reports_rather_than_refuses`,
      `a_program_that_does_not_compile_is_refused_and_stores_nothing`.

      The engine is built through `engine_from_members`, so `owner_only`,
      `require_step_up` and the per-role profile are the production ones
      rather than a test's imitation. The directory and the apply's target
      are separate databases, which is the shape rather than a workaround:
      an apply's target starts empty and holds no `members` table for a
      directory to be read out of.

      **Left open.** The op is registered on every assembled deployment,
      and its state is the **control** pool — a deployment whose spaces
      live in their own databases applies to the control database, which is
      where `$artifacts`, `$activations` and `$migrations` already are, and
      is correct; a per-space schema is not a thing this op can express and
      should not be.

- [ ] **18m · A graphical data modeling tool.** The owner, 2026-09-20.
      Draw entities, fields, refs and the guards over them, and get the
      `.ordo` definition out; read an existing definition back in so the
      picture and the source stay one artifact rather than two. The
      compiler already holds everything such a tool needs: the schema graph
      (`ordo_schema`), the interface descriptor (`ordo gen ts`), and the
      decompiler that turns SQL back into Ordo (`ordo-decompile`), so the
      hard part is the editing surface and round-tripping without losing
      what the text says, not the model. Post-deployment: the text
      definition and `ordo dev`'s loop are what a first deployment needs.

- [ ] **18q · Bucket credentials by role rather than key pair.** The
      owner, 2026-09-21 (`st-roles`): key pairs are enough for v1, roles
      are a post-deployment enhancement. A role means an STS call, an
      `x-amz-security-token` header inside the canonical request, and a
      credential that expires while requests are in flight. All three live
      inside `crates/ordo-blob/src/s3.rs`, so nothing about
      `$spacebucket`, the resolver or the audit trail changes when it
      arrives.

- [ ] **18r · A built-in link shortener.** The owner, 2026-09-23: *"add
      built in link shortener"*.

      **Why a platform owes one rather than an application writing it.**
      Every feature that mails a person a link needs one, and each of
      those would otherwise mint its own: a table, an id scheme, an
      expiry, a redirect route and a scan surface, five times over with
      five different answers about how long a link lives and who may
      resolve it. It is also the natural home for 18s's request links,
      which need a stable short identifier that is **not** the authority.

      **The questions to settle before building, none of them answered
      yet.**
      1. **Whose namespace.** A short code is global by construction, so
         the id cannot be per space the way a tenant's data is. Either the
         code carries the space, or the table is deployment-wide and
         resolving one discloses that it exists. Under the id ruling a
         code is minted, never chosen, and the shortener is the one place
         a **caller-chosen** identifier would be tempting.
      2. **Resolution is an unauthenticated GET**, which makes it an
         enumeration surface and the only such surface in the platform.
         Length and alphabet are the whole defence, and the rate limit on
         a miss matters more than the one on a hit.
      3. **Expiry and single use**, which are the same two properties
         18s's confirmation step needs and should share one mechanism
         with rather than reimplement.
      4. **Whether a resolve is an audit event.** It is a read of a
         deployment-wide table by an unauthenticated caller, and "who
         clicked" is a different question from "who acted".

      *Tier: Opus*, because the namespace question is a ruling rather than
      an implementation choice.

- [ ] **18t · A VS Code extension.** The owner, 2026-09-27: add it to the
      post-deployment roadmap.

      **Most of what an extension needs already exists**, which is why this is
      a packaging item rather than a language one. 14e shipped the TextMate
      grammar, so syntax highlighting is a manifest away. `ordo check` already
      emits `file:line:col: <class> [<code>]: <message>`, which is a diagnostic
      provider's whole input. `interface.json` (14a) is the descriptor the
      generated SDK, the LSP and the generated UI all read, so completion over
      op names, parameters and entity fields is a read of a file the compiler
      already writes.

      **What it does not have is the server.** 14e's ladder stops at the
      grammar, and the rungs above it — a resident process, incremental parse,
      go-to-definition, hover types — are the actual work. A first version that
      highlighted, reported diagnostics on save through `ordo check`, and
      completed from `interface.json` would need none of that, and is the shape
      to build first.

      **Where the real value is, and it is not completion.** The compile errors
      this language produces are the teaching surface: `incomplete_access_clause`
      naming the verb you left out, `unguarded` refusing an entity that reaches
      a guarded one, `unbounded_take` at registration. An editor that surfaces
      those where they are written, rather than after a save-and-run cycle, is
      worth more than any snippet library. `17x`'s first-five-minutes work
      points at the same surface from the other direction.

- [ ] **18s · An emailed link that is not a bearer credential.** The
      owner, 2026-09-23, describing the flow for an alert API wired to an
      existing login:

      > 1. The browser proves control of the signed-in account through a
      >    short-lived challenge.
      > 2. The server verifies that proof and checks workspace membership.
      > 3. It matches the verified account to the email link's intended
      >    recipient.
      > 4. Confirmation still validates expiration, request version, and
      >    single-use protection before applying the action.

      **The owner's example named an existing product's login** (Jazz).
      Recorded here generically, because what Ordo owes is the verifying
      half rather than any one identity provider, and the shape is the
      same whichever one a deployment already has.

      **The property this buys, stated plainly.** A link in an email is
      normally a bearer credential: whoever holds it acts. That makes a
      forwarded mail, a leaked referrer, a corporate mail scanner
      pre-fetching URLs, and a shared mailbox all into grants of
      authority nobody issued. Here the link only **names the request**;
      the authority comes from a session the server proved separately. A
      forwarded link then does nothing for the person who receives it,
      which is the whole point.

      **What Ordo already has, and what is genuinely missing.** Step-up
      exists and is structurally human-only, so no capability token can
      satisfy it (`security-logging-design.md` §5), which is exactly the
      property step 1 needs. Single use is idempotency's, already built.
      Expiry is the token tier's. What does not exist is **step 3**:
      binding a verified account to *the recipient a specific link was
      minted for*, so that a different signed-in member of the same
      workspace cannot act on somebody else's link. Membership alone is
      too coarse: it answers "may this person act here" and not "was this
      request addressed to them".

      **The trap worth naming before anyone builds it.** Steps 2 and 3
      are easy to collapse into one check, and collapsing them is the
      defect: workspace membership is not recipient identity, and a
      system that verified only membership would let any colleague action
      any colleague's alert while looking correct in every log.

      **Scope clarified by the owner, 2026-09-23**, and it is the framing
      rather than a detail: *"basically we just want to make sure to
      provide as a feature the ability to send emails with action links
      that can be used by the application with some authority on
      authenticated users."*

      So this is **a platform feature an application consumes**, not a
      pattern each application reimplements. The application declares the
      action and names the recipient; Ordo mints the link, holds its
      expiry and its single use, poses the challenge, verifies the proof
      and checks that the account it verified is the one the link was
      addressed to. What reaches the application is an action it may
      apply, already attributed.

      **The consequence worth building toward: the safe shape should be
      the only shape.** The whole value of putting this in the platform
      is that an application cannot accidentally ship the bearer-token
      version, where holding the URL is enough. If the surface offers a
      way to skip the proof, every application under deadline will find
      it, and the feature becomes a documentation problem rather than a
      guarantee. There should be no parameter that means "just trust the
      link".

      **Depends on 18r** for the link itself, and on nothing else.
      *Tier: Opus.* Nothing is open: the owner's note trailed off at
      *"If"* and the clarification above supersedes it.

- [ ] **18u · A tenant chooses between versions of an app.** The owner,
      2026-09-13 and again 2026-09-29: *"apps can be versioned and tenants
      should be able to select between those versions"*, and *"okay file
      it."* 20f had parked per-space activation as not work at all; this
      item is the newer word. Board: `per-tenant-install`.
- [ ] **18v · A connection reserve per database.** The owner, 2026-09-29:
      *"Keep it at one - we can file this for post deployment."* Board:
      `db-reserve`.
- [ ] **18w · Noticing sooner that a company removed somebody.** The owner,
      2026-09-29: a removal is noticed when the session token expires, and
      *"we could add some advanced features to post deployment roadmap."*
      Board: `federated-auth-questions`.

- [ ] **18x · Sign-in tokens tied to a device (SH-18).** The owner,
      2026-09-29: *"FIle."* A token works from any device that holds it
      until it expires or the person logs out. Board: `security-trace`.

- [ ] **18y · An outside key service for the master key (SH-11).** The
      owner, 2026-09-29: *"ideally it is indeed in a key management service
      and we need to implement that... plan it for post-deploy."* Checked
      the same day: not built. `KeyProvider` is the seam and has only the
      local and test implementations; no key service client is a
      dependency. Planned in `plan-key-service.md`, KS1 to KS6. Board:
      `key-service`.

- [ ] **18z · More mail services.** The owner, 2026-09-29: *"Leave just
      resend, sendgrid as the main ones and leave the rest for post
      deploy."* v1 sends by secure SMTP, Resend and SendGrid. Mailgun,
      Postmark, Brevo, Mailjet and SES come after. `plan-auth-federated.md`
      §7.6.

- [ ] **18aa · Backend functions.** The owner, 2026-09-29: *"This will
      allow for running actual functions and custom code in the backend -
      deployed through code/iac and fill in the final BAAS gaps."* Custom
      code the deployment runs, deployed the same way a program is (code and
      infrastructure as code), for what the language does not express.
      Related and separate: triggers deliver events today with no routine
      body (the `triggers` task), and `routine` is still a lean spelling
      (19c). Planned in `plan-backend-functions.md`, all 13 asks answered
      2026-10-11. **V1** (owner, 2026-10-31): function calls run as Wasm in
      wasmtime; long-running services stay v2. Increment 1 is built:
      functions, secrets, the SDK call, the notes v2 button, upgrade tests
      (owner, 2026-10-31: "make sure we're testing upgrade of a function"),
      `ordo fn runs` and `ordo fn log` with 14-day logs, the engine cache,
      `TextEncoder` and `crypto.subtle`. One unsafe call loads the cached
      engine (ask 18, ruled). HMAC keyed by a secret's name is built
      (ask 19), with `secret name` alone declaring a signing key. The
      language reference and the skill describe functions. Increment 2,
      files in, is built (2026-11-01): `$x: blob` arguments read as the
      caller through the blob door's row guard (`ctx.blob`), fetch sends
      bytes, and three caps are settings: `ORDO_FN_BODY_BYTES` (1 MiB),
      `ORDO_FN_BLOB_BYTES` (20 MiB), `ORDO_FN_FETCH_BODY_BYTES` (32 MiB).
      Guard: `a_function_reads_a_file_as_its_caller_and_the_row_guard_holds`
      in `crates/ordo-host/tests/blob_door.rs`. Then increment 4.
      Board: `backend-functions`.

- [ ] **18af · Function logs in the console (v2).** The owner,
      2026-10-31: *"Include tool to view logs in the console as a v2
      item."* A console page that lists a deployment's function runs and
      shows each run's log, reading what `ordo fn runs` reads (14 days).
      v1 has the command line alone. Board: `function-logs-in-console`.

- [ ] **18ab · A guest who can write.** The owner, 2026-09-29: *"Add
      planning for writing for post-deploy."* A sharer may let a guest write,
      only through ops that check guards, never through an open share link.
      Shares are read-only until this lands. To be planned from
      `plan-sharing.md` §7 question 3. Board: `sharing`.

- [ ] **18ac · Every setting in the program, and in the CDK.** The owner,
      2026-09-29: *"add a post-deploy roadmap to codify all settings that are
      possible - including creating the syntax and adding the function to
      the cdk."* Each setting in `settings.md` that the app developer layer
      can own gets a spelling in the language (the unparsed `policy` block,
      19-series lean marks, is the likely home) and a matching function in
      `@ordo/cdk`, with the tenant and person layers declared there as what
      the app developer exposes. Builds on 24a, the mechanism that stores
      and resolves the layers. Board: `settings-in-code`.
      **Spelling ruled 2026-10-03** (`syntax-additions` asks 6, 7): one
      `settings { }` block replaces `policy`, names from Ordo's closed
      catalogue (`plan-syntax-additions.md` §7). **Owed with this build, by
      the owner's word** (ask 13, *"make sure its written down to be done
      later"*): per-op settings such as page size, spelled then as
      `op x settings(door.pageSize = 100) =`, decided with the resolver that
      reads them.

- [ ] **18ad · Offline writes, and counts and grouped results on the
      device.** The owner, 2026-09-29: plan it briefly and file it for
      after deployment. `plan-offline.md`: an outbox for server-managed
      tenants (W1 to W6), the client-first path through F7 (V1 to V5), and
      local counts and groups with a parity test (A1 to A6). Board:
      `offline-plan`.

## 21 · Security review, round 6 (2026-09-28)

`docs/security-review-6.md`. Two cold reviewers with disjoint scope and a cold
consolidator, over the surface built since the last hardening review of
2026-08-10. **3 Critical, 5 Major, 3 Minor, 2 Note**, every one verified
against the code or the running binary. Fifteen contradictions between the
system's documents and its code. Nothing here is fixed; the false statements
in operator documents were corrected the same day and the rest waits on a go
or a ruling.

- [ ] **18ae · Shared rate windows across nodes.** The owner, 2026-10-03
      (`public-path-design` ask 12): *"document in post deploy roadmap."* In
      v1 a per-address rate window counts per node, so a limit set on 3
      nodes admits up to 3 times it. A shared count (through `PgSync`)
      makes the number exact. `design-public-path.md` §5. Board:
      `shared-rate-windows`.
- [x] **21a · A guard is dropped on every join the compiler adds (S6-001,
      Critical). FIXED 2026-09-28, three sites.** Two IR nodes, one defect. `Join` (a dot-path,
      `task.project.name`) is GC007, recorded. `SourceJoin` (an inferred
      **bridge** between two listed entities) is new and was recorded
      nowhere: `ir.rs:221` holds `from`, `on` and `many_side` and no guard,
      and `bind_multi_source` materializes a bridge as a bare table.
      **Verified live**: a member read a place linked only through an
      assignment another member owns, and owner and non-owner received
      byte-identical results. The op manifest claims the bridge is guarded.
      GC007's ruling (the guard goes in the join's `ON`) covers both; the
      fixture still cannot falsify either and needs the entity
      `f0b6ede` describes.

      **Built, and the owner's instruction to look for other places found a
      third.** Every site where a table enters a query was audited
      (`security-review-6.md` §8 has the table). `Join` and `SourceJoin`
      each gained a `guard`, compiled by one `Binder::joined_bound` and
      rendered in the join's `ON`. **The third instance was `whole`'s
      signpost**: the one-hop object for each ref a row holds was read with
      the space predicate and no guard, so a whole-read of an open row
      handed over the scalars of a guarded row it pointed at. Latent rather
      than live, since nothing in the host serves `whole` yet.

      Falsified in `tests/guards_compiler_joins.rs`, which carries its own
      program for the reason GC007 stood: with the dot-hop guard removed the
      test prints `"secret": "the secret"`, with the bridge guard removed
      `"label": "Vault"`. GC007 is closed as a divergence.

      **Still owed from the 2026-09-26 ruling**, and not claimed: the scope
      manifest does not yet list the guard reads a dot-hop adds
      (`taskSheet` still reports `projects.{name}` and not `owner`), and the
      catalog does not name which entities a dot-path pulls a guard from.
      Both are disclosure about the fix and neither affects what it
      enforces.

- [~] **21b · Membership at the door is a snapshot (S6-002 Critical, S6-004,
      S6-005, S6-010). Enforced and refreshed 2026-09-28; the command is owed.** `MEMBERS_SQL` selects `$space, id, role` and never
      `status`, every caller of `AccessEngine::suspend` is a test, and the
      engine is rebuilt on a deploy or a restart. So suspension does not
      exist, a removed member keeps serving until the next rebuild, and no op
      can suspend anybody. `deploying.md` said the opposite in two places and
      was corrected 2026-09-28. **Owner decision:** what suspension and
      removal must mean, and how fresh.

      **Ruled, in two answers.** *"This should be fixed and meet to match
      spec"*, and then, on how: *"We shouldn't be restarting deployments
      because someone got or lost access, that would be nonsensical."*

      **Built.** `MEMBERS_SQL` selects `status` and `engine_from_members`
      suspends on it. A supervised worker, `access.refresh`, re-reads the
      members on `ORDO_ACCESS_REFRESH_SECS` (default 30) in every server
      process and swaps the new engine in; it is not leader-only, because
      each process holds its own engine. The rebuild it calls already
      existed and had one trigger, the creation of a tenant.

      **Verified against a running server, one process throughout:** 200,
      then 403 four seconds after the row was set to `suspended`, 200 after
      it was set back, 403 after the row was deleted. The unit tests were
      falsified by making the status comparison never match.

      **Owed, and it is the larger half.** There is **no command that
      suspends a member**, and looking for one found there is no served way
      to add a second member either: the only production writer of
      `members` is provisioning's founding owner. The specification names
      `std.space.members()` and the group ops and no op for inviting,
      changing a role, suspending, reinstating or removing. Filed on the
      board as `membership-commands`; it gates the first program, which
      needs users.

- [x] **21c · A sealed value can be moved between rows (S6-003, Critical).
      RULED 2026-09-28: build it. Built 2026-09-29, B1 to B4 (merge d892e6b).**
      A column value is sealed at its row id, a file's chunks and sealed
      attributes at its handle, and the idempotency journal at its record;
      `seal` and `open` without an anchor are deleted. Falsified: an envelope
      moved to another row does not open
      (`ordo-conformance/tests/row_bound_seals.rs`), and two swapped objects
      do not open (`ordo-blob/tests/pipeline.rs`). A ranged write to a sealed
      field seals per row, with no bound, since the write already reads and
      locks each row. Limit, recorded: two images of one row and one field can
      still be swapped with each other. The blind index is not bound to a
      row. Details: `plan-row-bound-seals.md` §6.
      The AEAD's associated data is `aad(entity, field)` in both
      `ordo-crypto/src/lib.rs:89` and `stream.rs:153`. It binds the column
      and neither the row nor the object handle. An operator with database
      credentials can transplant a sealed value to another row of the same
      column and it opens; anyone with write access to the object store can
      swap two files in one column and both decrypt. The blob integrity
      digest would catch the second and is read only by revert and explicit
      verify. Not settled: whether chain images or a blind-index
      disagreement would expose a transplant afterwards. **Owner decision:**
      bind identity into the AAD, and whether `get` verifies.

      **Ruled: "yes build it."** Scoped the same evening and **not started**,
      because scoping found the change is 65 call sites, 27 more in tests
      and 10 `KeyProvider` implementations, and that the cost the owner was
      quoted was incomplete.

      **The correction.** The owner was told the cost is about thirty bytes
      of associated data per seal. True for a create and for an addressed
      update. **A ranged write to a sealed field seals once today and writes
      that one envelope to every matching row**; with the row in the label
      it seals once per row. Recorded in the plan with the table. It also
      means rows touched by one ranged write hold byte-identical ciphertext
      today, which discloses that they share a value, and the fix ends that.

      `$sealedpayload` has no row column and gains one. Verify-on-get is
      withdrawn: with the handle bound, a swapped file fails to decrypt.

- [~] **21d · Import hardening (S6-006, S6-007, S6-008 Major; S6-011
      Minor). Duplicate keys ruled and built 2026-09-28; three remain.** The resume fingerprint covers source ids only, so a bundle
      with the same ids and different values resumes and reports success
      over a mixture. `import.<entity>.create` carries no `rev`, so two rows
      under one unique key converge silently. Every member is read whole
      into memory with no cap, blobs included, before the pipeline's object
      cap is consulted. The staging reader follows symlinks. **Owner
      decision on one of the four:** refuse or converge on a duplicate key.

      **Ruled "Refuse" and built.** `import.<entity>.create` carries the
      create-once belief under `std_ops`' own condition, so the second row
      under a taken key answers `expected_absent`.

      **Falsifying it corrected the finding.** S6-007 said the rows
      converged silently. They did not: with the belief removed the protocol
      converges, the importer finds no `created` row in the answer, refuses
      and rolls the batch back. The duplicate was already refused, by
      accident, with a message naming the wrong thing. Minor rather than
      Major, and neither the reviewer nor the code's own comment had run it.

      **Owed, mechanical, no ruling needed:** fingerprint the row contents
      and not only the ids (S6-006), cap what is read (S6-008), refuse
      symlinks in the staging reader (S6-011).

- [x] **21e · `ORDO_KEY_PROVIDER=local` stretches the master with one HMAC
      (S6-009, Minor). Ruled 2026-09-28: refuse outside development. Built
      2026-09-29 (merge 91827b3).** The server refuses `local` at boot, and
      `ordo keys` refuses the env master, unless `ORDO_DEV_UNSAFE_TEST_KEYS`
      is set, the flag that already gates the test provider. `ordo dev`
      needs nothing. The refusal names `sealed-master` and the steps to it.
      Tests: `ordo-host/tests/runtime.rs`
      `the_local_provider_is_refused_outside_development`,
      `ordo-cli/tests/keys.rs`
      `the_env_master_is_refused_by_ordo_keys_outside_development`. Offline-crackable against wrapped keys that live in
      backups. `sealed-master` uses argon2id and is unaffected. **Owner
      decision:** warn, refuse outside dev, or harden.

- [ ] **21f · Two notes, recorded (S6-012, S6-013).** The executor splices
      caller values into `.ordo` source text and re-parses, which was
      attacked and held through quote-stripping, and is still a text-splice
      trust boundary. Supervisor leadership has a split-brain window that is
      safe only because the destructive passes claim transactionally.

## 19 · The lean ledger (2026-09-07)

One line item per `[lean]` mark in the spec corpus, owner-requested. **A
checkbox here never means "build me"** — lean owes nothing by definition
(`docs/marks-explained.html`). It means *a ruling has moved this mark*:
promoted to `[settled]` (work may then be owed elsewhere) or retired
(deleted from the spec). Until then an item being unbuilt is the system
working. Marked ⚑ where reality has already overtaken the mark and a
promotion ruling is due.

### `definition-language.md`

- [ ] **19a · `enum("…")` as a declared field type** — §2 (~388).
      Validation, migration checks, generated-UI dropdowns. The *pin* the
      acl derivation needed already exists internally (`enum_values` on
      `Field`); the authoring spelling is the lean half.
- [ ] **19b · `list of {…}` params + `each`** — §2 (~389, 717). The one
      lean mark blocking a settled, built construct: `allocate` has no
      legal call site until this promotes.
- [ ] **19c · `routine` declaration spelling** — §8 under the lean pin
      (§8 ~2449: "All illustrative syntax [lean] — concepts settled, file
      format not final"). The runtime is built (§12); the spelling is the
      whole remainder, and the §7-vs-§8 spelling conflict resolves in the
      same ruling.
- [ ] **19d · `principal` declaration spelling** — §8. Mechanism most
      complete of the nine (identity crates); spelling lean.
- [ ] **19e · `client` (OIDC relying party) declaration spelling** — §8.
- [ ] **19f · `policy` compliance pack** — §8 §Compliance (~3007),
      "[lean — draft, needs legal review]". Note `policy spaces` is
      settled and separate.
- [ ] **19g · `channel` / `route` spellings** — §8. Runtime is unbuilt
      too (17r/18a); when notifications build, they build behind whatever
      this ruling picks.
- [ ] **19h · `app` / `page` declarations** — §8, owned by ui-gen which
      says so itself.
- [ ] **19i · `trigger …` clause inside a routine block** — §8 code
      fences only. Distinct from §7's top-level `when`/`every`, which are
      SETTLED and tracked at §12e (not here).

### `std-library.md`

- [ ] **19j · `std.E.under`** — §1 (~31, 93). Derivable someday; the
      corpus deliberately excludes it from NOT_RUN reasons.
- [ ] **19k · `std.E.rollup`** — §4 (~379). Blocked twice: lean, and its
      derivation condition names a *declared relationship cap* the
      language has no syntax for. §4's own text names the promotion
      criteria (usage evidence).
- [ ] **19l · Field-list leans** — `history`'s output list (~277),
      `std.space.*` exact field lists (~310, 632), `list` default sort
      `-id` (~617), rollup's flat column naming (~400). Small, but each
      is a wire-visible fact a consumer could freeze prematurely.

### `implementation.md`

- [x] **19m ⚑ · Server stack [lean]** (~75) — **overtaken**:
      `plan-runtime.md` (1,915 lines, owner-approved GO, executing) now
      specifies the binary, keys and supervisor with done-whens. This
      section should be promoted/rewritten to point at the plan; a lean
      mark over a shipping subsystem misleads.
      **Closed 2026-09-18** (`lean-overtaken`): `implementation.md`'s
      "Server stack" section is rewritten and dropped its `[lean]` mark; it
      states the settled mechanism (binary/keys/supervisor) and points at
      `plan-runtime.md` §6, noting the binary is not yet merged to main.
      Recorded in `corpus-rulings.md`.
- [ ] **19n · Space custody conversion [lean]** (~644) — partially
      superseded by `f7-vault-plan.md` §5's settled dial-migration; the
      residue here should be reconciled when F7 phase work starts.

### `ui-gen.md`

- [x] **19o ⚑ · Skins [lean]** (§3 ~107) — **overtaken**: the Watermelon
      skin is owner-approved and building (17p). The ruling should
      promote "a skin exists as a package over the headless runtime"
      while keeping specific block sets illustrative.
      **Closed 2026-09-18** (`lean-overtaken`): `ui-gen.md` §3 is retitled
      "Skins [settled as shape]" and states "a skin is a package over the
      headless runtime"; the default four-component block set is marked
      `[illustrative]` rather than promoted. Recorded in
      `corpus-rulings.md`.
- [x] **19p ⚑ · Form generation from write-op params [lean]** (~397) —
      **overtaken**: `useOpForm` derives forms from `params[]` today,
      shipped in `@ordo/react` with the v2 descriptor's `field_ref`/
      `enum_values`. Promote the mechanism; the layout stays lean.
      **Closed 2026-09-18** (`lean-overtaken`): `ui-gen.md` §10's bullet is
      rewritten to `[settled]` and states the mechanism (`useOpForm`,
      `clients/ordo-react/src/hooks/useOpForm.ts` + `src/form.ts`), citing
      `field_ref`/`enum_values` and `id_types[].list`/`.get`; "Form layout"
      stays `[lean]` as its own line. Recorded in `corpus-rulings.md`.
- [ ] **19q · Sequencing [lean]** (~348) — superseded in practice by the
      owner's approved build order; retire or rewrite.

Everything above cites the mark's own line so the ruling reads the
sentence, not this summary. When a ruling lands: record it in
`corpus-rulings.md`, move the spec's mark, tick here, and — if promotion
creates owed work — open the item where the work belongs, never here.

---

## 20 · The topology audit's buyer-facing gaps (2026-09-12)

From [`audit-topology-gaps.md`](audit-topology-gaps.md), which ran the method in
`plan-topology-audit.md` over sixty-two capabilities phrased as the question a
buyer asks. Findings with verdicts live there; this section is the subset with a
schedule.

**Five items, ordered, and the order is a judgement rather than a hedge.** Each
entry says why *that one* deters, because "a gap" and "a gap that loses the
deal" are different claims and only the second earns a row here. 20a is first
because its failure arrives after the signature, which is the expensive place
for a multi-tenant platform to fail. 20b through 20e are ordered by how early in
an evaluation each one stops the conversation. Everything the audit found and
these five do not cover is collected in **20f**, deliberately at low importance,
so nothing is lost and nothing minor is dressed up as urgent.

Two rows the audit leaned on but did not re-open: the absence of a server binary
is **17a**, and it is the single cause of nine built-and-unreachable mechanisms
rather than nine separate defects; space provisioning is **17l** and **11-27**,
and it gates most of Dimension 1.

- [ ] **20a · Noisy neighbours: one pool for every tenant, no query bound, no
      attribution.** The audit's heaviest finding and the one the topology plan
      did not predict (`audit-topology-gaps.md` §2.7). The pool is a single
      `tokio::sync::Semaphore` sized `available_parallelism().clamp(2, 8)`
      shared by every space (`crates/ordo-exec/src/lib.rs:87-91`, `:255-256`),
      and `Pool::acquire` takes no space argument (`:359-375`), so the queue is
      FIFO across tenants and the loser gets `PoolError::Saturated` after two
      seconds. Nothing bounds a single query: no `statement_timeout` anywhere in
      the tree and `dial` sets no session GUCs (`:270-278`). Nothing attributes
      the damage either, because per-space metric labels are opt-in behind
      `ORDO_METRICS_PER_SPACE` (`crates/ordo-host/src/metrics.rs:275-281`).

      **Why this one deters.** Every other item on this list costs a deal during
      the evaluation, where the answer is "not yet". This one costs a deal after
      the signature, in an incident where one customer's unindexed `contains`
      scan makes every other customer's request time out, and where the operator
      cannot say whose query it was. A platform that cannot divide its own
      concurrency has not yet made the promise its tenancy model exists to make.

      **The verdict is unbuilt rather than structural, which is the good news.**
      The topology permits a per-space permit budget and `PoolResolver` (S1 of
      `plan-space-routing.md`) is the seam one would sit behind. The bad news is
      that routing does not fix it for the tenants who need it most: a promoted
      tenant gets their own target and their own pool, and the tens of thousands
      sharing the default target, which is every tenant who has not paid to
      leave, keep sharing one semaphore.

      **Owed, smallest first.** A `statement_timeout` on `dial` with a policy
      value, which is the cheapest high-value change in the whole audit. A
      per-space permit budget behind the resolver, with a reserved floor so no
      space can be starved to zero. And the attribution question, which is
      whether `ORDO_METRICS_PER_SPACE` should be on by default for the spaces
      currently saturating, since a cardinality bill nobody agreed to is a real
      objection and an unattributable outage is a worse one. Distinct from
      **17n**, which is metering and quotas for billing; this is admission
      control for isolation, and a quota that refuses a call after it has
      already held a connection for ninety seconds is not this.

      **Built 2026-09-20.** The finding above is the tree as it was on
      2026-09-12; three of its premises had already moved by the time this
      was built, so the increments are smaller than it implies.

      *Already closed before this work, and not re-done.* A space resolves
      per request through one seam that knows both the space and the pool
      (`ordo_host::spaces::Spaces::resolve` / `pool_of`). `PoolResolver` has
      two implementations (`ordo_exec::route::SinglePool`, `AttachedPools`),
      so a promoted tenant already gets its own target and its own bound.
      The usage ledger counts requests, rows and worker runs per space (17n
      M1), and the metrics registry takes per-space labels. What none of
      them answered was the pool.

      **M1 · A share of the pool per tenant.** `ordo_exec::PoolLimits`
      carries `space_reserve`; `Shared` holds one `Semaphore` of
      `size - reserve` permits per charged space, and `Pool::for_space`
      tags a handle the way `Pool::observing` does.
      `Spaces::pool_of` is where the tag is applied, so every request-path
      checkout is charged and an uncharged handle (boot migrations,
      `ordo-cli`, the loops that walk targets) keeps the bound it had.
      The space's share is taken **before** the pool's permit and out of
      the same `acquire_timeout`.
      `ordo-exec/tests/noisy_neighbour.rs::one_space_cannot_hold_every_connection`
      proves both halves in one body: with the reserve at zero, which is
      the code path the pool had before this work, one space takes all four
      connections and the other is refused `Saturated`; with one reserved,
      the same load caps the noisy space at three and the quiet space reads
      its row. Three more tests pin that the cap is on concurrency rather
      than a quota, that an uncharged handle is unbounded, and the
      arithmetic of the floor.

      **M2 · A statement time bound.** `PoolLimits::statement_timeout`,
      default 30s, `ORDO_PG_STATEMENT_TIMEOUT_MS`, `0` for none, set in the
      dial options so a connection carries it from the moment it exists and
      the re-dial that replaces a dead one carries it too.
      `PoolConfig::connect_config` stays bare for the `LISTEN` session and
      the leadership lock. SQLSTATE 57014 is read beside the three
      `simple_rows` already reads and becomes `PoolError::StatementTimeout`,
      spelled `statement_timeout: <detail>`, with
      `ordo_api::ErrorCode::StatementTimeout` at **503** and a row in
      `api-engine-design.md`'s table, so both inventory checks pass.
      `a_statement_past_the_bound_is_refused_not_hung` proves a five second
      sleep is refused inside the bound and that the pool is intact after;
      `the_bound_reaches_a_statement_inside_a_transaction` proves it is not
      only the one-shot path; `the_bound_can_be_turned_off` proves the knob;
      `ordo-host/tests/noisy_neighbour.rs::a_statement_timeout_reaches_the_caller_as_itself`
      proves it crosses the string seam as a 503 rather than a 500.

      **M3 · Attribution.** Three no-op-by-default methods on
      `ordo_exec::metrics::AppendObserver`, implemented by
      `ordo_host::metrics::Metrics` into
      `ordo_pool_space_capped_total{space}` and
      `ordo_pool_saturated_total{space}`, both labelled by space **with
      `ORDO_METRICS_PER_SPACE` off**, plus `ordo_pool_wait_seconds` behind
      that opt-in. The exemption's argument is that the refusal families
      have a series per space that was refused rather than per space, so a
      healthy deployment carries none.
      `the_space_consuming_the_pool_is_named_with_the_opt_in_off` reads the
      noisy space's name out of `render_prometheus` with the opt-in off and
      asserts the space that was served appears in neither series;
      `the_opt_in_adds_the_wait_per_space` pins the other half.

      **What remains, and it is why this stays open.** A fair queue across
      tenants: the cap bounds one noisy space, and k spaces at their caps
      still reach the pool's bound, at which point a k+1st takes
      `Saturated`. `deploying.md` §2a says so and names the reserve as the
      dial. The cost gate still classifies where work happens and how many
      rows a plan yields, never whether a predicate has an index, so an
      unindexed `contains` is still a sequential scan that passes every
      gate the platform has; the statement bound now ends it rather than
      the gate refusing it. `AttachedPools` dials target pools without the
      deployment's observer, so a promoted tenant's own pool is bounded and
      unattributed. Per-space quotas remain 17n's.

- [ ] **20b · SSO, and the missing credential path underneath it.**

      **2026-09-29, later: passkey sign-in S0 to S7 built (merge 4a7b123)**,
      with the four sign-in hooks and the presence setting
      (`ORDO_STEP_UP_EXEMPT_OPS`, default every platform op asks). E7, the
      90-day sign-up default and `std.person.tenants` built (12d8acc);
      first-comer is off by default everywhere. **Found:** a deployment
      cannot name its first owner on the server side; planned as P0 of
      `plan-provisioning-on-signup.md`. Recovery is next.

      **2026-09-29: enrolment E1 to E6 built (merge e352209).** The door
      resolves a person and a tenant to a member; tickets; membership
      commands; admission per tenant, default viewer; first-comer with the
      claim code as a setting; the password give and reset, and ending every
      session. E7 waits on the sign-up lifetime. **Owed to the sign-in
      route:** a caller for `Admission::on_arrival`, a route for
      `redeem_password`, enforcing a temporary password's forced change at
      sign-in, and a worker that refreshes ended sessions on other replicas
      (needed before running more than one). Passkey sign-in S0 and S1 are
      built; S2 waits on the owner.

      **2026-09-29: the binary did not serve `std.space.create` or
      `std.space.reap`.** Both were wired and missing from the list the
      binary registers; their tests register every op directly, so they
      passed. Fixed (b08e940), with a test that every defined platform op is
      served.

      **2026-09-29: passkey sign-in has a build plan**, `plan-passkey-signin.md`, S0 to S7. The enrolment, recovery and federated plans are rewritten to the owner's answers of the same day.

      **2026-09-29: the served authentication cannot be used, and two
      operator documents said it could.** Asked by the owner whether the
      auth systems are in place. Read against the code:

      | Method | State |
      |---|---|
      | password | **served**: `/auth/begin`, `/auth/complete`, `/auth/logout`, `/auth/refresh`, durable credentials and revocations, argon2id, a decoy verify so an unknown identifier costs the same |
      | passkey | **built and refused**: `passkey.rs` and `webauthn.rs` verify challenge, RP id, origin, presence flag, signature and clone counter, with tests. `auth_from_env` refuses the method: *"passkey needs the browser ceremony and a relying-party id"*. `ORDO_APEX_DOMAIN` is *"read, recorded, and not acted on"* |
      | OIDC | a stub that does no network I/O |
      | SAML, OAuth2, email OTP | named in `AuthMethodKind`, no implementation |

      **Two gaps make the one served method unusable.**

      1. **Nothing can create a credential.** `CredentialStore::put` is
         reached only through `LocalPasswordProvider::enroll`, and every
         caller of `enroll` is a test. The item's own list below says *"no
         signup endpoint"*; the consequence it does not draw is that a
         deployment with `ORDO_AUTH_METHODS=password` has sign-in routes and
         nobody who can sign in.
      2. **A password session cannot pass step-up, and every platform op
         requires it.** By design (`security-logging-design.md` §2):
         presence comes only from passkeys. So `std.deployment.apply`,
         `std.space.create`, `std.space.import`, `std.space.export` and
         `std.space.delete` are uncallable by any real sign-in. The tests
         that prove them (`deploy_apply.rs`, `tenant_create.rs`) mint a
         token directly with `user_presence_at: Some(..)`.

      `howto-operating.md` showed a password sign-in and said in the next
      paragraph that the token must carry presence. `go-live.html`, written
      2026-09-27, told the owner to turn on `password` and then
      `ordo apply --host`. Both corrected.

      **Passkey is therefore first twice over**: it is what the owner ruled
      for the first program, and it is the only thing that unlocks the
      deployment's own operations. Filed as `auth-bootstrap`.

      **Ruled 2026-09-29, and three plans commissioned.** Six principles,
      in `corpus-rulings.md`: the deployment owns credentials, the tenant
      owns admission, a sign-in is an enrolment by default, no flow is
      compulsory, a tenant has recorded power over credentials it created,
      and one person is one identity across many tenants. The owner
      overruled `auth-bootstrap`'s recommendation of a compulsory one-time
      code.

      - `plan-enrolment.md`, **written**: the give, the reset, admission
        policy per tenant, first-comer, and six increments E1 to E6.
      - `plan-auth-federated.md`, SAML, OAuth2 and email code: **written
        2026-09-29**, thirteen increments F0 to F12 and fourteen questions,
        six filed as `federated-auth-questions`. **OIDC first, SAML last**:
        OIDC needs no new third-party crate and forces the shared machinery
        into existence; SAML carries the largest risk, its XML signature
        dependency, and is held until a buyer is named. Email code needs a
        mail channel, a rate limiter and the caller's address, none of which
        exist.

        **Five things it found that the code does today and must stop
        doing**, the first four confirmed by reading: the egress client
        discards every response body, so it cannot fetch discovery
        documents or keys; ceremony state is per process, so a redirect
        returning through a load balancer lands on a replica that never saw
        it begin; the OIDC stub reads the subject from the caller's own
        request, which is a sign-in-as-anyone hole the moment it is wired
        and is why the boot refuses it; the auth routes record nothing; and
        the display name is whatever the caller posts.

        **The auth routes recording nothing was found twice, by two
        planners who did not see each other's work.** It is F0 there and R2
        in the recovery plan, and is built once.

        **One disagreement between plans, left for the owner.**
        `plan-enrolment.md` has a sign-in always enrol an identity, on the
        ruling that each new sign-in is an enrolment. This plan recommends
        that a sign-in admitted to no tenant mint nothing.
      - `plan-passkey-recovery.md`: **written 2026-09-29**, nine increments
        R1 to R9 and nine questions filed as `passkey-recovery-questions`.
        Its first increment, ending every session of a person, has no
        prerequisite and is the same mechanism `plan-enrolment.md` E6 needs,
        so it is built once.

        **Three findings it made beyond its brief, each confirmed by
        reading.** Enrolled passkey public keys are held in process memory
        (`PasskeyProvider.credentials` is a `Mutex<BTreeMap>`), so a
        registered passkey would not survive a restart. `/auth/complete`
        discards the audit events `sign_in` returns: nothing in
        `ordo-host/src` reads `.audit`, so onboarding and linking have no
        record in the security stream. And no route calls `link`, `unlink`
        or `refresh_presence`, so there is no way to add a second passkey,
        remove one, or step up.

      **Planning found that a person and their membership are never joined
      at the door.** A principal is `usr_…` and deployment-wide; a
      membership is `mem_…` and per tenant; `members` has a `principal`
      column; and `MEMBERS_SQL` does not select it, so the engine keys
      memberships by member id and a signed-in principal is a stranger in
      every tenant. It works in tests and in `ordo dev` because both hold a
      token whose subject is the member id. So one person cannot belong to
      two tenants under one identity today. E1 builds the join and
      everything else stands on it. Five questions filed as
      `enrolment-questions`.
      **Partly closed 2026-09-13** (`50c9fc4`): `/auth/begin` and
      `/auth/complete` exist, `token::mint` has a production caller, and a
      ceremony now produces a token the door accepts, proved by minting
      through the route and verifying under the door's own key. Every
      refusal is one answer, so the route is not an account-existence
      oracle.

      **Durability and session lifetime closed 2026-09-20.** Two of the
      three remaining 5h deferrals go here; `ORDO_APEX_DOMAIN` and the
      browser ceremony stay open, and both belong to the passkey
      increment rather than to this one.

      **A credential survives a restart.** `LocalPasswordProvider` held
      its credential table as a `BTreeMap` field, so an enrolment's life
      was the process's life. It now sits behind
      `ordo_identity::credentials::CredentialStore`, with
      `MemoryCredentialStore` for tests and
      `ordo_host::credentials::PgCredentialStore` over `$credential` for
      a deployment: `PgKeyStore`'s pattern, its own table and its own
      thread with a runtime on it. No cache, unlike the key store,
      because it is read once per login and a password change must take
      effect on the next login rather than the next restart. **What one
      login costs**: one round trip plus one argon2id verification at the
      pinned profile, and the verification dominates.

      **A timing oracle, found while moving it.** Every refusal was
      already one answer, but an unknown identifier returned in
      microseconds while a real one paid a full argon2id verification
      first, measurable over the network, and a reliable
      account-existence answer whatever the body said. Both paths now run
      exactly one verify, the unknown one against a per-process decoy.

      **Logout, refresh, and a revocation check the door consults.** A
      token now names its session (`SessionClaim`: an opaque id and the
      sign-in instant), which is the referent everything else needed;
      before it, two logins by one person were indistinguishable and "end
      this one" meant suspending the principal on every device they own.
      `POST /auth/logout` puts the id on a deny list; `POST /auth/refresh`
      mints a fresh token for the same session, clipped to a ceiling
      counted from the sign-in. The token format moves `v2` → `v3` and an
      older token is refused outright rather than read as "no session, so
      nothing to look up", which is exactly the claim a logged-out holder
      would like to make. There is **no session table**: everything a live
      session asserts is in the token under the MAC, so the only durable
      state is the sessions somebody ended, bounded by logouts rather than
      by logins and swept at each ceiling.

      **What the door pays per request**: one `RwLock` read and one
      `BTreeSet` lookup over the revoked ids, no allocation, no syscall,
      no I/O, immediately after an HMAC-SHA256 it is not measurable
      beside. That is what `PgSessionRevocations` is shaped around: the
      list is loaded at open, written through on logout and re-read on a
      cadence, never queried per request. The staleness that buys is named
      in its module doc, along with the two things that bound it.
      `SwappableAccess` carries the list across a program swap, or a
      deploy would forget every logout.

      **What each test proves.** 136 in `ordo-identity` (14 new): a
      credential outlives the provider that enrolled it; the store is
      never handed a password; an unknown identifier costs the same work
      as a wrong password (this one fails on the old code, 0 verifies
      against 1); a store outage is not a wrong password; refresh
      continues a session and widens nothing; a logged-out session cannot
      be refreshed; logout is idempotent; refresh cannot reach past the
      ceiling and the last token is clipped to it; a capability token is
      not a session; refresh carries presence across without refreshing
      it; a revocation outlives every token that could use it; the
      session claim survives the token round trip; a step-up keeps the
      session it stepped up in. 11 in `ordo-host/tests/auth_door.rs` (8
      new): a logged-out token is refused by the door, recording
      `SessionRevoked` where it recorded `NoMembership` a moment earlier;
      a logout takes away access that worked, for a member with a profile;
      refresh continues a session without a ceremony; a logged-out session
      cannot be refreshed; a session ends at its ceiling; every logout
      answer is the same answer; logging out twice is logging out; a
      deployment with no deny list does not pretend to log out. 7 in
      `ordo-host/tests/auth_durability.rs` (all new, real Postgres, each
      one an actual restart): the credential survives, the logout
      survives, the table holds a PHC string and no plaintext, a
      re-enrolment replaces the row and keeps `enrolled_at`, a missing
      table refuses rather than reporting an empty one, a revocation is
      swept at the ceiling, and a second replica sees a logout after one
      refresh.

      **The binary can serve a login, behind `ORDO_AUTH_METHODS`**, with
      `ORDO_SESSION_SECS` (3600), `ORDO_SESSION_MAX_SECS` (43200) and
      `ORDO_AUTH_PROVISIONING` (`invite-only` | `auto-onboard`). Off by
      default and the census says so. A configured store that cannot be
      opened refuses the boot rather than falling back to memory, and a
      method the binary cannot serve refuses the boot naming what exists.

      **Still not built, and an application must build each one itself:**
      no signup endpoint, no password reset, no email verification, no
      "sign out everywhere", and no operator view of live sessions
      (nothing knows them, by design). OIDC remains a stub with no network
      I/O.

      **The directory's plan, before building it (2026-09-20).** Two
      tables, not one, because a row that only names a link cannot make
      principal-id collision a constraint the database enforces rather
      than a probability: `$principal` (`id` primary key, `kind`,
      `display`, `created_at`) holds each minted identity once;
      `$authenticator` (`authenticator` primary key, `principal`,
      `linked_at`) holds which authenticator names which principal — the
      name the census and the operator docs already use for this gap. A
      principal id is drawn the way `SessionId::mint` already draws a
      session id: 128 bits from the OS CSPRNG, hex, prefixed by the kind
      (`usr_…`, `agt_…`), never a counter, so nothing about a restart or a
      process's history can make a deployment mint the same id twice.
      Collision is not left to the odds: minting is an `INSERT … ON
      CONFLICT ("id") DO NOTHING` against `$principal`, and a colliding
      draw (astronomically unlikely, and a claim this plan does not ask
      anyone to trust untested) is retried with a fresh draw, bounded at 8
      attempts, so a broken id source refuses loudly rather than spinning.
      Auto-onboard's idempotency falls out of `resolve_or_onboard`'s
      existing order once the link survives a restart: a second login from
      an authenticator this deployment has already linked finds the row in
      `$authenticator` and returns the same principal before minting is
      ever reached — no new logic, just a durable read where there used to
      be a durable one only until the process exited. `PgCredentialStore`
      is the shape: its own tables, created at boot beside `$credential`
      and `$sessionrevocation`, no in-memory fallback, and a store that
      cannot be opened refuses the boot rather than starting an
      authentication surface with no accounts behind it.

      **Closed 2026-09-20, as planned above.**
      `ordo_identity::directory::Directory` now holds its state behind a
      `DirectoryStore` trait — `MemoryDirectoryStore` for tests,
      `ordo_host::principal_directory::PgDirectoryStore` for a
      deployment — the same split `CredentialStore` already made for a
      password. `create_principal` no longer mints from a counter; it
      draws 128 bits from the OS CSPRNG and the store commits the draw
      with a uniqueness check. **`ORDO_AUTH_UNSAFE_MEMORY_DIRECTORY` is
      gone**: `runtime.rs` builds the directory over `PgDirectoryStore`
      unconditionally whenever `ORDO_AUTH_METHODS` is set, the same way
      it already built the credential store over `PgCredentialStore`,
      and the boot-time census no longer lists the directory as
      undurable because there is nothing left to warn about.

      **What each test proves.** 2 in `ordo-identity` (a colliding draw
      is retried and never minted; a source that only ever collides
      refuses after 8 attempts rather than spinning, against an injected
      id source, no database needed). 3 in
      `ordo-host/tests/auth_durability.rs` (10 total, +3, real Postgres,
      each an actual restart): an enrolled authenticator still maps to
      its principal after a restart, and so does the principal record
      itself; auto-onboard twice across a restart yields the same
      principal and audits nothing the second time, rather than minting
      a second principal under the first one's identifier; a principal
      id cannot be minted that collides with an existing one, proved
      with a rigged id source that draws the same suffix twice and
      showing the retry lands against Postgres's own `$principal`
      constraint rather than trusting a 128-bit draw never collides in
      practice.

      The passkey path is wired and exercised only in
      `ordo-identity`, because nothing can fake a user gesture at the HTTP
      layer. Real SSO still needs an OIDC provider that does network I/O;
      `StubOidcProvider` says of itself that it does none. Of six
      `AuthMethodKind` variants (`crates/ordo-identity/src/principal.rs:468-475`)
      three have an implementation: password with a real Argon2id hasher,
      passkey with real CTAP2 and COSE verification (both landed in **5h**), and
      OIDC as `StubOidcProvider`, which models the authorization-code shape and
      performs no network I/O, saying so in its own doc: a real provider
      "additionally exchanges the code (PKCE) and validates the ID-token
      signature against the issuer's JWKS; those steps are network I/O"
      (`crates/ordo-identity/src/provider.rs:515-523`). SAML, OAuth2 and EmailOtp
      are enum variants with no implementation. SCIM has no match in the tree at
      all.

      **Found underneath it, and it is the larger half.** There is no
      authentication route anywhere. The whole HTTP surface is `/op/{name}`, two
      blob routes, three health routes, `/metrics`, `/security/alerts` and the
      catalog and interface pairs (`crates/ordo-api/src/router.rs:120-148`,
      `crates/ordo-host/src/serve.rs:153-164`), and
      `ordo_identity::token::mint` (`crates/ordo-identity/src/token.rs:91`) has
      no caller outside tests. `session.rs` states the missing step in the
      conditional: mint "could put a" session into a token (`:5`). So the door
      verifies a bearer credential that nothing the platform ships can issue.

      **Why this one deters.** SSO is a gate rather than a preference: above a
      certain deal size the security questionnaire asks for it and a "no" ends
      the conversation before any of Ordo's actual arguments are heard. And the
      credential gap is worse than a missing integration, because a buyer
      running a pilot discovers it in the first hour rather than in procurement.

      **Owed.** The ceremony-to-token step first, since every provider needs it
      and nothing else can be demonstrated without it. Then the federated
      provider that turns `StubOidcProvider` into a real one, which is code
      exchange and JWKS validation over a trait, a ceremony and an
      authenticator-id discipline that are already built and tested. SAML and
      SCIM are in **20f** until a named buyer asks, because each is a sprint
      and neither is evidence of anything the other does not already prove.

- [ ] **20c · Per-tenant restore, which has no Postgres-shaped answer and a
      better Ordo-shaped one.** There is no backup code of any kind in the tree:
      `backup`, `pg_dump` and `snapshot` match only comments about what must
      survive a backup (`crates/ordo-exec/src/erase.rs:17`, `:28`). Undo is per
      command group (`crates/ordo-exec/src/undo.rs:949`) rather than per point in
      time, and the horizon is a 90-day Rust constant applied identically to
      every space (`crates/ordo-exec/src/cadence.rs:96`, `:116-117`), with the
      CLI printing its own limitation: "`policy commandlog` is not yet parsed
      from source" (`crates/ordo-cli/src/operator.rs:490`).

      **The obvious plan is wrong, and that is why this needs a row rather than
      a ticket.** The Postgres answer to "restore one customer" is a per-database
      point-in-time restore, and with tens of thousands of spaces on a sparse set
      of targets, restoring the target restores every tenant on it. Physical
      restore is the wrong instrument for the same reason database-per-tenant
      was rejected as a default. The instrument that fits is the one already
      built: a per-space hash-chained command log, a revert applier that undoes
      one group, and a checkpoint that signs a head. "Restore this tenant to last
      Tuesday" is a walk back through that chain to a timestamp.

      **Why this one deters.** It is the first question in a B2B multi-tenant
      procurement, it is asked in exactly the words the audit uses, and the
      buyer asking it has lived through the incident that taught them to ask.
      Answering "we have a signed hash chain of every write" and then failing to
      answer "so restore this one customer" is worse than not having the chain,
      because it reads as a feature that stops short of the use.

      **Owed.** The timestamp walk, a horizon configurable per space rather than
      a deployment constant, and the blob-side question of what an object
      restored past its sweep means. **17j** is adjacent and is not this: it
      names PITR for the whole deployment and the export bundle for a space, and
      the audit's point is that neither of those restores one tenant. Note also
      that the bundle is ruled "a disclosure format, not a backup format" and
      `ordo import` was unbuilt when this was written; it is built since
      2026-09-26 (`std.space.import` and `ordo import`, 11-10a), which answers
      part of this item: a tenant's export can be loaded back in.

- [ ] **20d · Nothing reads the chain out: no change feed, no webhook, no audit
      export.** Three buyer questions with one mechanism behind them, which is
      why they are one row. "Call our endpoint when a row changes": the only
      outbound path in the tree is the attestation webhook
      (`crates/ordo-exec/src/webhook.rs:1-30`) and even it never runs, because
      `AttestDelivery` has no caller outside `crates/ordo-host/tests/`. "Get our
      changes into our warehouse": no match in the tree for CDC, logical
      replication or a replication slot. "Stream our audit log to our SIEM":
      `SecurityStore::entries` (`crates/ordo-secevent/src/store.rs:228`) is
      called only by integrity and pruning code, the CLI's verbs are all
      integrity rather than content (`crates/ordo-cli/src/main.rs:105`,
      `:160-163`), and `std.space.chainProof` discloses "positions, 32-byte
      digests and signatures" and explicitly not rows, actors or correlation ids
      (`crates/ordo-host/src/attest_ops.rs:22-24`).

      **The substrate is already the best part of the platform.** A per-space
      hash-chained command log is a stronger change feed than a WAL tail: it is
      per tenant by construction, it carries before and after images, it is
      ordered, and it is tamper-evident. Every one of the three questions above
      is a reader over it plus a delivery shape, and the delivery shape's hard
      half is paid, because the egress kernel ships one client with six pins and
      one place to audit (**17k**).

      **Why this one deters.** A buyer does not replace their stack, they add to
      it, and a backend with no outbound event is a backend they have to poll.
      The compliance half compounds it: the chain is the reason to choose Ordo
      over Supabase, and today its owner cannot get it out, so the differentiator
      is invisible in exactly the evaluation where it would win.

      **Owed, and scoped against what is already tracked.** A reader over
      `$commandlog` per space, with a cursor and the floor rule the archive
      already states, then two consumers over the egress client: an outbound
      delivery and a bulk export. **7d** owns the security-event half (verify the
      stream, and an owner-facing pane) and is not re-opened here; this row is
      the command log, the delivery, and the export format. **18a** places the
      per-space consented allowlist rail post-deploy, which stands.

- [ ] **20e · Search, and a refusal whose stated workaround does not exist.**
      Ranked full-text is refused for v1 with the reason on record
      (`docs/sql-translation-corpus.md:810-812`, I5, with I6 refusing trigram and
      fuzzy on the same ground). The refusal is fine. What the audit found is
      that its escape hatch is missing: the recorded workaround is "search-box UX
      is client-side over synced data", and no client-side store exists anywhere
      under `clients/`, with the item that would build one (**5g-pre**)
      unstarted. So the refusal currently has no workaround at all.

      **And what does exist is unindexed.** The language's string predicates are
      a closed three, `contains`, `starts` and `matches`
      (`crates/ordo-syntax/src/ast.rs:67-71`), emitted as `strpos`,
      `left(...)` and `~` (`crates/ordo-plan/src/lib.rs:1700-1702`). The only
      index DDL the compiler emits is a plain btree over declared columns
      (`crates/ordo-schema/src/ddl.rs:567`), and the cost gate classifies venue
      and row bounds and never index coverage (`crates/ordo-plan/src/cost.rs:1-25`),
      so a substring search over a large tenant's table passes every gate the
      platform has and runs as a sequential scan. The mitigation is real and
      worth stating in the same breath: the primary key leads with `"$space"`
      (`crates/ordo-schema/src/ddl.rs:377-382`), so the scan is confined to one
      tenant and paged. That is also the mechanism that makes 20a's failure
      shape possible.

      **Why this one deters.** Every application has a search box. A buyer told
      "not in v1, do it on the client" will accept that answer and then discover
      the client does not exist, which converts a stated trade-off into a
      surprise, and a surprise found after a pilot costs more trust than a "no"
      found before one.

      **Owed, and the cheap version is the one to do first.** State the
      dependency: the search refusal is conditional on **5g-pre**, and nothing
      records that today. Then decide between the two honest v1 answers, because
      the zero-extension rule forecloses `tsvector` and `pg_trgm` together and
      that rule is worth keeping (`audit-topology-gaps.md` §3.3). Either a
      declared search index the compiler emits and maintains, which keeps the
      no-extension property and buys prefix and substring at index speed, or a
      projection beside the database, which is the same shape §2.1 already
      requires for cross-tenant analytics and for vectors. Choosing once serves
      three capabilities.

- [ ] **20f · Everything else the audit found, collected so nothing is lost.
      Low importance by construction: each line is either a capability a named
      buyer has not yet asked for, or one already homed on another row.** Kept as
      one item because splitting it would give twenty gaps the same visual weight
      as the five above, which is the failure mode `plan-topology-audit.md` §0
      calls feature-list envy.

      **Structural, and therefore not work at all until someone decides to pay
      for a redesign** (`audit-topology-gaps.md` §2): cross-tenant analytics as a
      query, per-tenant app install and schema variation (§2.2, and the decision
      that comes first is what an app is allowed to be), the deployment-wide
      freeze a transforming migration takes (§2.3), sort and range over sealed
      fields and guards that read them (§2.4), and the cluster-wide WAL (§2.5).
      The last is an argument for sealing rather than an item.

      **Compliance, and the ones that graduate the moment a regulated buyer
      names them**: customer-managed keys, where the master key comes from
      `ORDO_MASTER_KEY` (`crates/ordo-crypto/src/custody.rs:651-661`) and no KMS
      binding exists, though the trait is already shaped for one; data residency
      per tenant, which routing makes reachable and nothing implements; legal
      hold, which has no producer anywhere in `crates/`, so a prune cannot
      respect one; and per-principal erasure, since `erase_space` takes a
      `SpaceId` (`crates/ordo-exec/src/erase.rs:357`) and a DSAR against one
      individual inside a shared space has no primitive. Key rotation is
      **17i** and is not re-opened.

      **Data platform**: vector search, foreclosed in Postgres by the
      no-extension rule and reachable as a projection; read replicas, cheaper
      than most because `Read` ops are already classified read-only and
      replica-eligible (`crates/ordo-exec/src/lib.rs:199`) while `PoolConfig`
      holds one host; time-series volume, whose recorded answer is the blob tier
      with batching (`corpus-rulings.md` 2026-09-12); and a query result cache,
      which nothing measured says is the bottleneck.

      **Tenancy and change management**: cloning a tenant, moving a live space
      between targets (out of scope in `plan-space-routing.md` §2 with the
      reason recorded, and the feature customers ask for the day they hear the
      tier exists), and preview or branch environments, which inherit the
      per-space activation decision above.

      **Identity**: SAML and SCIM, held behind **20b** until a buyer asks; the
      issuance surface for capability grants, since `AccessEngine::grant_for`
      (`crates/ordo-identity/src/access.rs:615`) has no caller and no op mints
      one, which is what an API key would be here; delegation creation, enforced
      on every request (`crates/ordo-api/src/router.rs:408-417`) and created
      nowhere; the anonymous door, where the language says a guard decides and
      `check_access` refuses before any guard compiles
      (`crates/ordo-api/src/router.rs:722`, `:730`), costing one construction
      site plus the three rules `access-model-explained.html` §3 proposes; and
      the guest question, which is that `Role` has four variants and none is a
      guest (`crates/ordo-identity/src/principal.rs:105-110`), so the window path
      is unproven.

      **Realtime and clients**: presence, absent entirely; other-language
      clients, since `ordo-gen` emits TypeScript only
      (`crates/ordo-gen/src/lib.rs:1-53`) and each further language is a pure
      consumer of the same descriptor; an OpenAPI projection, which is a second
      generator backend and the one concession worth making to a descriptor
      nobody outside Ordo reads; and the fact that the shipped React client
      consumes neither `?live` nor the head pin the server already sends on
      every response (`crates/ordo-api/src/router.rs:475-504`), which is a
      client-side omission rather than a platform gap and belongs with **5g-pre**.

      **And two documentation corrections the audit turned up**, recorded here
      because they are cheap and because leaving them costs the next reader a
      rediscovery. `platform.security.alerts` is deployed and granted to the
      owner in `deployment_engine` (`crates/ordo-host/src/lib.rs:1423-1447`),
      fixed by D0 on 2026-09-10 with a regression test, so the sentence in
      `corpus-rulings.md:5197-5201` describing it as "mounted with its op
      deployed nowhere, so it answers 403 to everyone" is true of that date and
      not of this tree. And `deployment_engine` deploys and role-gates every
      platform op spec (`:1471-1505`) while attaching none of the `with_exit`,
      `with_attest` or `with_buckets` handlers
      (`crates/ordo-host/src/platform_ops.rs:736`, `:742`, `:759`), so five of
      the nine platform ops are authorized and answer a typed internal error
      saying exactly that (`:772-780`). The second is 17a's shape and is noted
      so the wiring work knows what it owes.

---

## 22 · Builds that rulings called for and nothing tracked (board audit, 2026-09-29)

The audit of 2026-09-29 (`audit-board-2026-09-29.md`) checked every board
task against the code. These are rulings whose build had no checklist item.
None has a go; each is filed so it cannot be lost.

- [ ] **22a · The per-tenant lag contract.** Ruled 2026-09-21 (D-B, D-B2):
      a tenant may stay one version behind under a contract, with a snapshot
      behind it and a step-by-step way down. Only the default quarantine is
      built (`crates/ordo-host/src/spaces.rs`, `SchemaBehind`). 18d says the
      contract "is not in T0 to T10". Board: `db-behind-schema`,
      `db-lag-bound`.
- [x] **22b · Erasing a space leaves the objects in a bucket the tenant
      owns.** Ruled 2026-09-21 (D-S5): keys destroyed, objects left, a
      custody entry recording what remained. **Built 2026-10-06**
      (49eb7b43): `erase_space` decides per file by its recorded store,
      leaves a tenant-owned bucket's files, and writes `objects_left`
      beside `space_erased`. Board: `st-delete-objects`.
- [ ] **22c · Attaching a bucket that already holds this tenant's
      objects.** The refusal is built (U6). **Ruled 2026-09-29: allowed when
      every object matches a record.** Not built. **Ruled 2026-10-06:**
      lift the refusal to change or detach a bucket that holds files, since
      each file records its bucket (25ac); old files stay and are cleaned up
      where they are; document the choice and why in
      `docs/howto-byo-custody.md`. **Built** a28b43e9. Board:
      `st-attach-existing`.
- [ ] **22d · Publish the bucket policy the platform's credential
      expects.** Ruled 2026-09-21 (D-S7). It exists only in
      `plan-tenant-storage.md` B-X2; no operator document carries it.
      Board: `st-bucket-policy`.
- [x] **22e · A connection reserve per target.** **Ruled 2026-09-29: one
      value for the deployment is enough; the per-target setting is 18v.** `corpus-rulings.md` says
      the reserve "is set once per target". One deployment-wide value is
      built (`ORDO_PG_SPACE_RESERVE`). Whether the per-target setting is
      wanted is on the board. Board: `db-reserve`.
- [ ] **22f · A role that reads the ops log and cannot prune it.** Ruled
      with `plan-opslog.md` §2.3. Neither the reader nor the role exists.
      Board: `opslog-role-split`.
- [ ] **22g · The modelling guidance says what routing does to windows.**
      `corpus-rulings.md` says the guidance has to say windows cost more
      than feeds under routing. `modeling-guidance.md` does not mention
      routing. Board: `sharing-under-routing`.
- [x] **22h · Commands to add, suspend and remove a member.** **Built 2026-09-29 as enrolment E3 (merge e352209):** `std.space.admit`, `setRole`, `suspend`, `reinstate`, `remove`; owner-only, the last owner protected, every replica told at once. Enforcement
      is built (21b). No command changes a membership. Planned in
      `plan-enrolment.md`, and waits on the owner's answers there. Board:
      `membership-commands`.
- [ ] **22i · A refinement can reach the door.** The cost gate of 11-8 is
      built and idle: no op can be marked refinable and the executor ignores
      `$refine`. Board: `effective-cost`.
- [ ] **22j · Small things recorded only in prose.** The deployment-id
      column on four tables (18o), a caller for `AccessEngine::grant_for`
      (11-16 follow-up), the break-glass connection string (17z), and a
      local-volume blob store. Board: `many-deployments`,
      `deployment-scope`, `door-only`, `blob-store`.
- [x] **22k · A bucket attach is not written to the tenant's security
      stream.** Ruled 2026-09-29, for the reason D-F gave for databases.
      **Built 2026-10-06** (73fbc0dc): attach, replace and detach are
      recorded on the deployment's stream only. Board: `db-chain-attach`.
- [ ] **22l · SH-2: two spaces never share a digest, and a test says so.**
      Traced 2026-09-29. The key is per space by construction
      (`custody.rs`, `blind_index`); no test compares two spaces, and the
      feed of 11-3 must write through the receiver. Board: `security-trace`.
- [ ] **22m · SH-7: the copy in the browser, the three parts owed.** Checked
      2026-09-29. Sealing, per-space stores, the purge on revocation and the
      rule that permissions are never copied are built. Owed: device keys
      wrapped under a passkey, a lock when idle, and a retention policy.
      Board: `security-trace`.

---

## 23 · Sign-in, added by the owner (2026-09-29)

The owner: *"Backup codes and recovery. Add a backup codes system and 2FA
with hardware token auth as an available option."*

- [ ] **23a · Backup codes.** **Planned 2026-09-29 in `plan-second-factor.md` §6: NIST SP 800-63B look-up secrets, one set with two uses.** Planned as path B and step R4 of
      `plan-passkey-recovery.md` (§3.2): ten codes, shown once, issued only
      to a session with fresh presence, voided on reissue. It was in the
      plan and on no list. Needs the recovery core, R1 to R3, first. Not
      built. Board: `second-factor`.
- [ ] **23b · A second factor, with a hardware token as an option.** **Planned 2026-09-29 in `plan-second-factor.md`, T1 to T7; five questions with the owner.** New.
      A security key already works as a passkey; a second step after a
      password or an outside sign-in does not exist. One settled rule bears
      on it: `security-logging-design.md` §2.1 rules out verifying
      attestation, so the platform cannot prove a key is hardware. Five
      questions are with the owner. Not planned, not built. Board:
      `second-factor`.

---

## 24 · Settings in layers (2026-09-29)

The owner: limits are layered, a tenant or person can change a setting when
the app developer exposes it, never one hardcoded system value.
`settings.md` inventories 153 settings against that rule.

- [ ] **24a · A mechanism for the app developer, tenant and person layers.**
      Today only the deployment layer can set almost anything: no program
      spelling for settings exists (`policy` is lexed and not parsed), and
      the few tenant-set values (admission, claim code) each have their own
      store. `settings.md` lists 31 hardcoded values that should be
      settings and 15 single deployment-wide values that disagree with the
      rule. Not planned. Board: `settings-layers`.
- [x] **24b · The 7-day share link value is a default, not a cap.**
      `crates/ordo-blob/src/policy.rs` carried it as `share_ttl_max`; renamed
      `share_ttl_default` 2026-09-29. The owner: seven days is "a default
      setting - when the feature is used but no settings have been defined
      for it", and the platform sets no maximum. Board: `settings-layers`.
- [ ] **24c · Pull hardcoded values out into settings.** The owner,
      2026-09-29: *"add a task to pull hardcoded values out and turn them
      into settings."* The 31 values `settings.md` lists under "Hardcoded
      today and should be a setting", each with its file and the layer it
      belongs to. Each becomes a setting with today's value as its default,
      so behaviour does not change until somebody sets it. Board:
      `settings-layers`.
- [x] **24d · Audit the wiring of settings a tenant can change.** **Done 2026-09-29: `audit-settings-wiring.md`.** Of 18 settings a tenant or person should be able to change, none can be changed today, and no general store, write op, exposure declaration or resolver exists; 24a is that machinery. The owner,
      the same day: *"an audit on the table to see what wiring is missing for
      tenant exposed settings."* For every setting `settings.md` marks
      exposable or could-be-exposed to a tenant or person: what exists (a
      store, an op, a read at the point of use) and what is missing.
      Board: `settings-layers`.
- [x] **24e · Nothing issued a tenant invitation. Built 2026-09-29:** `std.space.invite`, `std.space.invitations`, `std.space.revokeInvitation` (`invite_ops.rs`), owner and manager, a manager cannot invite above their own role; tests `crates/ordo-host/tests/invitations.rs`. Tickets are still clamped at 30 days by `tickets::MAX_TTL_SECS`, a hardcoded value for 24c. Found by the 24d audit,
      checked 2026-09-29: tickets are issued only for password resets and
      shares (`credential_ops.rs`, `share_ops.rs`). Enrolment E2 built
      redeeming an invitation (`admission.rs`) and no op creates one, so an
      invitation-only tenant cannot admit anybody. Owed: an op for a tenant
      owner or manager to invite a person. Board: `enrolment-questions`.
- [ ] **24f · No production code mints a capability grant or delegation.**
      Found by the same audit: the stores and the door's checks exist; nothing
      a person can call creates one. Board: `delegation`.
- [x] **23c · Recovery follow-ups from the owner's answers (2026-09-29).**
      A setting the app developer holds for whether a tenant manager may
      recover a member (off unless set); and recovery paths skip a sign-in
      that belongs to an outside provider, which recovers its own. **Built
      2026-09-29** (commit 29ad47d; recorded 2026-10-04): `ORDO_RECOVERY_MANAGERS`,
      and `OUTSIDE_PROVIDER_SENTENCE` in `crates/ordo-host/src/recovery.rs`.
      Tests in `crates/ordo-host/tests/recovery_ops.rs`:
      `a_manager_may_not_recover_a_member_unless_the_app_developer_set_it`,
      `a_manager_recovers_a_member_below_manager_when_the_setting_is_on`,
      `no_recovery_is_opened_for_a_person_whose_only_sign_in_is_an_outside_providers`,
      `a_recovery_leaves_a_persons_outside_provider_sign_in_alone`. Board:
      `recovery-details`.

## 25 · What the notes app found on the server (2026-09-29)

Built on branch `worktree-agent-a1bb20a51a704d848`, not yet merged. The app
works around each of these; each is owed on the server.

- [x] **25a · The door refuses ids the platform mints** (`is_typed_id` in **Fixed 2026-09-29 (87bfa78).**
      `crates/ordo-api/src/bind.rs` allows one underscore; minted ids have two).
- [x] **25b · A row with an owned reference cannot be deleted** (`has_referrers` **2026-09-29:** `subtree` on a non-tree entity is now a compile error. Whether a plain delete cascades to owned rows is with the owner (`owned-delete`). **Built 2026-09-29:** a delete cascades to owned rows, including access rows, as ruled.
      from its own admin row; `subtree` on a non-tree fails at run time).
- [x] **25c · A tenant made after boot gets no live updates** **Fixed 2026-09-29 (87bfa78).**
      (`PgInvalidationBus::refresh` has no caller).
- [x] **25d · `ordo dev` has no sign-in and ignores CORS**, so an app with **Fixed 2026-09-29 (87bfa78): `ordo dev` honours CORS and passkey settings.**
      passkeys cannot be developed on it.
- [x] **25e · Smaller:** the credentials directory needs an undocumented
      `$deployment` file; `single` routing loses provisioned tenants' reads; the
      door serves a new tenant up to 120 ms after sign-up answers; minted ids
      do not sort by age; `created: datetime = now` passes check and fails DDL.
      **Closed 2026-10-06:** the file was fixed 2026-09-29 (77caa0fa) and the
      README corrected; `single` refuses tenant creation and the boot names
      `ORDO_PG_ROUTING=attached` (e36e45ce, test
      `on_single_routing_a_sign_up_makes_no_tenant_and_the_boot_names_the_setting`);
      the making replica rebuilds its door before answering (e36e45ce, test
      `the_first_call_after_sign_up_is_served_with_an_empty_credentials_directory`);
      ids sort by age to the millisecond (7eab0b57, test
      `a_deployments_minted_ids_sort_by_age_past_ten_and_across_logs`);
      `= now` is refused by check (`unsupported_default`), and making it
      work is after the first deployment.
- [x] **25f · The server stopped accepting connections and did not exit. Fixed 2026-09-29:** the share peek and accept handlers waited on a single-connection store with a blocking receive on a runtime worker, while the answer needed that runtime; two overlapping peeks stopped it. Every sync store bridge now uses `ordo_exec::bridge`. A stop signal was not involved.
      Seen 2026-09-29 running the notes app locally: port 4000 held, 24
      connections queued and never accepted, every thread idle, the database
      quiet. Either a stop signal arrived and the graceful drain never ended
      (the page holds a live update stream open), or the accept loop stopped
      on its own. Either way a server must not hold its port while serving
      nothing. Board: `first-program`.
- [x] **25g · Each tenant held about seven database connections. Fixed 2026-09-29:** an idle tenant holds none (`ORDO_PG_IDLE_SECS`, default 30); 30 sign-ups pass on a default server. Found
      2026-09-29 debugging 25f: every sign-up under built-in provisioning
      keeps 6 to 7 Postgres connections and file descriptors open, so a
      default `max_connections` of 100 runs out at about 12 tenants.
      Sign-ups then fail with "too many clients already". With one tenant
      per person this is the notes app's ceiling on a real deployment.
      Board: `first-program`.
- [x] **25h · Undoing a create whose row has since gained owned rows
      refused. Fixed 2026-10-04** (commit 2964fad). The revert of a create
      used the old referrer check, which ignored `owned`. It now cascades
      like a delete: the owned rows go with it, each recorded with its
      before-image (ids `<rev-id>-owned-<n>`), the reverter must be able to
      delete every one, and a plain reference still refuses. Tests in
      `crates/ordo-exec/tests/owned_delete_revert.rs`. Board: `owned-delete`.
- [x] **25q · Undoing a board's create failed on its own access row. Fixed
      2026-10-04** (commit 2964fad). A create whose group carries a derived
      access row (any `acl` root) could not be reverted: the inverse deleted
      the creator's admin access row first, and the board was then refused
      as unwritable. Every planned delete is now judged before any runs.
      Test `reverting_a_board_create_takes_its_cards_and_access_rows`. Board:
      `revert-board-create`.
- [x] **25i · A program update reached no tenant's own database. Fixed 2026-09-29:** apply brings every tenant forward, a catch-up handles those that missed it, and a refused change closes that tenant alone. Seen
      2026-09-29: after the notes program changed, `serve.sh` applied it; the
      deployment's database moved and the owner's tenant stayed behind, held
      closed as schema-behind. With one tenant per person every update would
      lock out every person. The caller was told the database "cannot be
      reached", which is wrong. Building. Board: `first-program`.
- [x] **25j · New tenants were unreachable where the database needs a
      password. Fixed 2026-09-29:** a tenant on the deployment's own
      cluster is dialled with the deployment's own password. Found by CI.
      Board: `tenant-password`.
- [ ] **26a · The SDK's missing pieces for the device copy.** Found wiring
      it into `@ordo/react`, 2026-09-29: no public `SyncManager.resync()`, so
      an app must supply a reload after a write; a commit's rows are not
      applied to the device at the returned position (`plan-client-cache.md`
      §6), because a write returns a summary and not the rows; and no lookup
      of a sync handle by op and parameters. Board: `ships-a-client`.


- [x] **25k · The device's null logic differs from the server's.** Found
      2026-09-29. `clients/ordo-sdk/src/replica/refine.ts` uses SQL's
      three-valued rules: `null != 5` is false and `null == null` is false
      on the device, where Ordo's two-valued logic makes both true
      (`howto-values.md`, "Two-valued null logic"). A list filtered with
      `!=` or `==` on a nullable field can differ offline and online.
      **Fixed 2026-09-30.** Board: `device-filter-parity`.
- [x] **25l · The device orders text differently from the server.** Found
      the same day. `refine.ts` compares strings by UTF-16 code unit; the
      server orders by code point (`COLLATE "C"`). They disagree when
      characters outside the basic plane, such as emoji, meet characters
      between U+E000 and U+FFFF. Board: `device-filter-parity`. **Fixed 2026-09-30.**
- [x] **25m · The server compared text by the database's locale in
      filters. Fixed 2026-09-30.** `<`, `<=`, `>`, `>=` on text in a
      condition had no `COLLATE "C"`, so on an `en_US` cluster they answered
      by locale; only sorting was collated. Found by the parity test.
      Board: `server-text-compare`.
- [x] **25n · The server's `in` never matched a null value. Fixed
      2026-09-30.** `x in (…, null)` gave SQL's unknown; the language says
      `in` is a chain of `==`, so a null in the list matches a null value.
      Found by the parity test. Board: `server-text-compare`.
- [x] **25o · Custody records lost the name of the command that made
      them. Fixed 2026-09-30.** `ordo keys init`, `rotate`, `reseal`,
      `destroy` and `ordo restore --record` passed their op with spaces, and
      the recorder stores any op outside `[A-Za-z0-9._-]` as
      `<unprintable>`. Now dotted (`ordo.keys.init` and so on). Found while
      building `ordo triggers deregister`. Test:
      `crates/ordo-cli/tests/custody_ops.rs`. Board: `custody-op-names`.

## 27 · Grief remediation: migrations, environments, other systems (2026-10-01)

From the owner's notes on the board's Grief Remediation tab
(`docs/supabase-comparison.html`, notes in `docs/grief-notes.json`), and his
answers of the same day. Three specs: `plan-evolution-plans.md`,
`plan-environments.md`, `plan-external-entities.md`.

- [ ] **27a · Stored evolution plans.** **The command is `ordo evolve`**
      (ruled 2026-10-04, `syntax-additions` ask 1), which also frees `ordo
      plan` for the compiler's artifact; below, read `ordo plan` as `ordo
      evolve`. `ordo apply` ends by offering to remove `prev` lines that have
      landed everywhere. `ordo plan` renders and stores a
      plan per step (forward SQL, reverse SQL or the reason there is none,
      chosen options, the shape it was rendered against); `ordo apply` runs
      a stored plan. Ruled 2026-10-01: a plan with errors is refused, and
      `ordo plan` writes a committable receipt beside the sources.
      `plan-evolution-plans.md` §3.1 to §3.3. Board: `evolution-plans`.
- [ ] **27b · Keep every artifact and plan; the app developer may prune.**
      Ruled 2026-10-01. A pruning setting, off by default, and step-by-step
      replay for a lagging tenant. §3.4. Board: `evolution-plans`.
- [ ] **27c · Seamless migration: the old version serves until the new one
      works.** Ruled 2026-10-01. Expand, check, then switch, per tenant; a
      failure never switched, so nothing rolls back, and the deployment
      owner is told. Replaces holding a behind tenant closed. §3.6. Board:
      `evolution-plans`.
- [x] **27d · Schema applies set `lock_timeout` and retry, and add
      constraints `NOT VALID`, then validate. Built 2026-10-04.** §3.7.
      The lock half (commit a3ba357): each statement runs under
      `lock_timeout`, retried with backoff, then fails naming the table;
      settings `ORDO_APPLY_LOCK_TIMEOUT_MS` (2000), `ORDO_APPLY_LOCK_RETRIES`
      (5), `ORDO_APPLY_LOCK_BACKOFF_MS` (500). The constraint half (commit
      767cef1): a foreign key, enum pin or check onto an existing table is
      added `NOT VALID` in the transaction, then validated on its own after
      commit; a new table keeps its constraints inline. A failed validation
      drops the constraint again and names it, the table and the rows'
      failure (`$migrations` phase `validate_failed`). Uniques still refuse
      over existing rows (no `NOT VALID` form). Tests
      `crates/ordo-host/tests/apply_lock_timeout.rs`, `apply_not_valid.rs`.
      Board: `apply-lock-timeout`.
- [ ] **27e · Compiling is byte-stable across machines.** A test that the
      same source gives the same version hash anywhere; promotion by hash
      depends on it. `plan-environments.md` §2.1. Board: `environments`.
- [ ] **27f · Apply moves up one environment at a time.** dev by default,
      `--staging` and `--prod` refused until the step before runs the
      version. §2.1. Board: `environments`.
- [ ] **27g · Dev branches as dev tenants, and staging as clones of named
      tenants.** §2.2, §2.3. Board: `environments`.
- [ ] **27h · Per-tenant rollout strategies, and destructive commands that
      name their environment.** Ruled 2026-10-01: rolling by default (a
      batch every 5 minutes over an hour), all at once, random canary,
      blue/green, and by query. §2.4, §4. Needs 18ac. Board: `environments`.
- [ ] **27i · External entities.** Generated, read-only translations of
      foreign tables that Ordo serves as ordinary entities.
      `plan-external-entities.md` §2, §3. Board: `external-entities`.
- [ ] **27j · Each apply lists what is present but not Ordo's.** §4.
      Board: `external-entities`.
- [ ] **27k · Ordo's tables under a prefix plus a random identifier per
      database.** Ruled 2026-10-01. In place of `public`;
      `schema_apply::LIVE` becomes per database. §5. Board:
      `external-entities`.
- [ ] **27l · Audit what the deployment's and each tenant's database hold,
      and who owns what.** §8. Board: `deploy-schema-audit`.
- [ ] **27m · Who may delete a note's owned rows is the app developer's
      setting, default only the note.** Ruled 2026-10-01. Board:
      `owned-delete`.
- [x] **27n · Live refetch pacing is a setting.** Group bursts, at least a
      quarter second apart by default. Ruled 2026-10-01. **Built 2026-10-04**
      (commit 6c10ed4): `DEFAULT_LIVE_PACE_MS = 250` in one place
      (`clients/ordo-react/src/live.ts`), the provider's `live.paceMs` sets
      it, and a hook's `livePaceMs` may lengthen it, never shorten it. The
      `live.refetchPace` line in the settings block is part of 27ah. Tests in
      `clients/ordo-react/test/live.test.ts`. Board: `react-live-device`.
- [x] **27o · The interface states each read's sort order**, so the device
      copy returns rows in it, where the language allows. Ruled 2026-10-01.
      **Built 2026-10-04** (commit 9d1c40c) as `ops[].result.order`:
      `{ keys: [{ path, desc }], stated }`. `stated` is true when every key of
      the plan's order, ending in the id, is an open output; otherwise the
      device uses id order. The SDK's `refineRows` and the React device copy
      use it. Board: `react-live-device`.
- [x] **27p · `ordo dev` wipes tables that are not its own. Fixed
      2026-10-04** (commit b65e205). Found 2026-09-30: `dev_db::reset` ran
      `DROP SCHEMA public CASCADE` on any database it was pointed at. Ruled
      2026-10-01: drop only the program's tables and note the rest. A reset
      now drops only Ordo's tables (a `$` name, declared now or at the
      previous save, or carrying the `$space` column), RESTRICT so a foreign
      object depending on one makes the reset refuse instead of losing it,
      and `ordo dev` prints the tables it left alone. Tests
      `dev_db::tests::classify_tables_sorts_by_ownership`,
      `a_foreign_table_survives_a_reset_and_ordos_own_are_reset`. Board:
      `dev-reset-foreign`.
- [ ] **27q · Apply from a connected GitHub repository.** After first
      deployment. `plan-environments.md` §7. Board: `github-apply`.
- [x] **28i · Publish the client packages.** `@ordo/sdk` and
      `@ordo/react` are not published, so an app outside the repo cannot
      install them. Ruled 2026-10-06: GitHub Packages, private, by CI, as
      `@hominemai/ordo-sdk`, `-react` and `-cdk`. **Built** (1c1ce546,
      998cc6b0, 6b327b05, 863feca9); first published by CI on 1e36f303.
      Board: `client-packages`.
- [x] **28k · Gaps the notes app found in the React building blocks.**
      Found 2026-10-07 moving `examples/notes/web` onto
      `@hominemai/ordo-react` (b601d928, c2769dfe, 086c8ec5): five small
      package fixes (device-copy manager, sync spec type, revocation signal,
      conflict state, syncable opt-out) and two design gaps (no hooks for
      platform ops; guest windows get no hooks client). **Built 2026-10-08**
      (14867755, a3177c77, 7355ca96, b75ad1f5), packages 0.2.0. Owed: a
      network failure in a read hook surfaces as an unhandled rejection.
      Board: `react-gaps`.
- [x] **28a · Proxy mode, and limits by visitor address.** After first
      deployment (owner, 2026-10-06). Ruled the same day: off by default,
      documented; the deployment names the header that carries the
      visitor's address and the proxy addresses trusted to set it. Until
      then no decision uses a client address: the per-address sign-up limit
      and the share-link miss limit are removed, and return as settings,
      off by default, with this. Brought forward 2026-10-11 (public-data
      ask 28): slow-down only, before the first deployment. Board: `proxy-mode`.
- [x] **28b · A JSON field type.** Listed 2026-10-06 among what ellanor
      needs. **Built 2026-10-08** (a8bad5a2 to b5315c2b): `json` / `json?`
      as jsonb, size and depth limits as settings, text to json gated by a
      probe. **Filtering inside a document built 2026-10-08** (70035713):
      `prefs.theme`, `prefs["my key"]`, `tags[0]`, typed by the other side;
      missing and JSON null are both null, a wrong type does not match,
      `contains` reaches into arrays. Sort, group and arithmetic stay
      refused. No index serves a path yet (Board: `json-path-index`), and a
      client's `.where()` cannot reach a path yet (Board: `json-refine`).
      Plan `docs/plan-json-field.md`. Board: `json-field`.
- [ ] **28c · Full-text search.** After first deployment. Listed 2026-10-06
      among what ellanor needs: search text fields by words, ranked (tsvector). Board: `full-text-search`.
- [ ] **28d · Vector search.** After first deployment. Listed 2026-10-06
      among what ellanor needs: embeddings and nearest-neighbour search (pgvector). Board: `vector-search`.
- [ ] **28e · An app reads a field's past values.** After first deployment. Listed 2026-10-06
      among what ellanor needs: earlier values of a field from the command log, under the row's guards. Board: `field-history`.
- [ ] **28f · Ordered lists and drag order.** After first deployment. Listed 2026-10-06
      among what ellanor needs: an order people set by dragging, without rewriting every row. Board: `list-ordering`.
- [ ] **28g · Loading a large tenant in stages.** After first deployment. Listed 2026-10-06
      among what ellanor needs: the first screen fast, the rest filled in. Board: `staged-load`.
- [ ] **28h · What Ordo needs before ellanor can move.** After first deployment. Listed 2026-10-06
      among what ellanor needs: the umbrella: 18aa, 12a, 18ad, 11-4 and 28b to 28g. Board: `ellanor-readiness`.
- [ ] **28j · What Ordo needs before denizens can move.** After first
      deployment. Listed 2026-10-07 from denizens-web (Supabase): sign-in by
      emailed code, OAuth and anonymous (20b), edge functions (18aa), a
      server function acting for the app (11-4), JSON columns (28b), import
      from Supabase (11-10a). Board: `denizens-readiness`.
- [ ] **28l · Importing users and data from Supabase and other systems.**
      Raised 2026-10-08 (federated ask 6): users with their linked logins,
      tables through the existing import, other systems later. To design
      carefully. Design go 2026-10-11 (ellanor-readiness ask 5).
      Board: `supabase-import`.
- [ ] **28m · A token-efficiency benchmark.** Raised 2026-10-29: Ordo is
      built to save AI tokens; measure it before publishing the claim.
      Design `docs/plan-token-benchmark.md`: 6 new builds and 4 changes,
      Ordo against Supabase and plain TypeScript on Postgres, Claude Code
      headless, 5 trials per cell, cost per passing app with retries, and
      permission tests in every suite. Ruled 2026-10-29: all 3
      baselines incl. Convex, 1 small app first on Claude Code at no API
      cost, save all telemetry and publish. First run ruled the same day: a
      trimmed team task tracker, Ordo against Supabase, Fable plans and
      Sonnet builds. Harness built (`bench/token/`):
      26 hidden tests, runner, scoring; Ordo reference passes 26/26.
      Ordo's AI guide written (`docs/ai-guide.md`, 22.7 KB against
      Supabase's 24.8 KB; every example checked by
      `crates/ordo-cli/tests/ai_guide_check.rs`). Trials run isolated from
      the owner's plugins, skills, MCP servers and memory. Two errors in
      `traps.md` fixed. Trials run in Claude Code's sandbox.
      Round 1 (2026-10-30), 3 runs each, all 26/26: Ordo $5.69 vs
      Supabase $4.52 per passing app (1.26x), no win. Ordo's planner
      spends more on an unfamiliar SDK; clearing an optional ref cost one
      run 3 repairs. Next round waits on the owner.
      Board: `a-token-efficiency-benchmark-ordo-against-conven`.
- [x] **28n · An Ordo skill and op lookup.** Ruled 2026-10-30 after
      benchmark round 1: an Ordo agent skill that ships with the product,
      an op index written from the definition, and `ordo ops` to search it.
      Benchmark round 2 starts both stacks with their vendor's skill,
      packages installed and backend running. **Built 2026-10-30**
      (PR #2): `skills/ordo/` is the skill and the source of
      `docs/ai-guide.md`; `ordo ops` (grouped by entity and file),
      `ordo gen ts --app` (writes `generated/OPS.md`) and
      `ordo skill install`. Tests `crates/ordo-cli/tests/{ops,
      skill_install,ai_guide_check}.rs`, `scripts/check-skill-examples.mjs`.
      Board: `ordo-agent-skill-and-op-lookup`.
- [x] **28o · Making the repository public.** Raised 2026-10-30: the repo
      will be public, linked from the hominem page. Audit found no real
      secrets; scratch files and local paths untracked (commit 18576ad7).
      **Public 2026-10-30**, with its history, after main passed all 5
      checks. The CLI has no default host; self-hosted CI jobs skip fork
      pull requests and outside contributors need approval. **Restarted
      2026-10-31** (owner): the old repository is private as
      `HominemAI/ordo-history`, and the public `HominemAI/ordo` begins
      from 1 commit, made public after all 5 workflows passed. `main` has
      the ruleset `Protect` (pull request, linear history, no delete or
      force-push, admin bypass). Board: `making-the-repository-public`.
- [ ] **27r · A data generation module.** After first deployment.
      `plan-environments.md` §5. Board: `data-generation`.
- [x] **27s · One live stream per client, shared by every hook.** The
      owner, 2026-10-01: one per entity *"seems like it would be pricey
      among tons of clients"*. Live stays off by default; when on, a client
      opens one stream and the hooks share it. **Built 2026-10-04** (commits
      d27c9a5, 8379764): `GET /live` opens a stream, `POST /live/{stream}`
      adds a read through the full door, `DELETE /live/{stream}/{key}` drops
      it; each signal names its read. A refused read leaves the stream open;
      a stream belongs to the token that opened it. `?live=1` and sync are
      unchanged. The React runtime opens one stream per client. Design:
      `crates/ordo-api/src/live.rs`, `api-engine-design.md` §6.1. Tests in
      `crates/ordo-api/tests/api.rs` (`shared_stream_*`),
      `crates/ordo-host/tests/deployment.rs`
      (`one_shared_stream_carries_two_reads_over_the_postgres_bus`),
      `clients/ordo-react/test/live.test.ts`. **For the audit:** the idle
      window (60 s) and the cap (256 reads) are constants, not settings;
      opening a stream needs only a valid token. **Audit, 2026-10-04:** both
      fixed. `ORDO_LIVE_MAX_READS` (256) and `ORDO_LIVE_IDLE_CLOSE_SECS` (60)
      are settings (commit f41c8e7); opening a stream needs a token the
      deployment issued, as `?live=1` does (commit 850cf11); the React hub
      reopens its stream after a session refresh (commit 1c8f6b5). Board:
      `react-live-device`.
- [ ] **27t · Custom builds for chosen tenants.** Beta builds, a promoted
      dev branch serving named production tenants. After first deployment;
      depends on serving more than one version (`evolution-plans` ask 3).
      Board: `tenant-builds`.
- [ ] **27u · Grant-based read-only SQL views of Ordo entities.** The
      owner: *"could be possible, maybe."* Not planned. Board:
      `external-entities`.
- [ ] **27v · Staging clones mask personal data by default**, as the app
      developer's setting under their contract with their users. Ruled
      2026-10-01. Board: `environments`.
- [ ] **27w · Pre-checks run every check against every tenant, and give
      remediation.** Ruled 2026-10-01 (`forward-gaps` 12, 13): one failure
      does not stop the rest, so the developer gets the full list first;
      each issue names the Ordo-level fix (a value to map, a query or op to
      find the missing data). Refuse-and-offer is the default for every
      case a generator cannot decide (ask 1). Board: `forward-gaps`.
- [ ] **27x · `prev`: renames and correspondence declared in the
      language.** The owner's syntax: `entity derpderp (prev blabla) {}`,
      `field macaroni (prev c)`. Reaches one version back only. Also the
      base for splits, merges and field splits (asks 8, 18). Where it lives
      (source or plan) is decided with the syntax design (ask 23, deferred
      2026-10-03; receipts matter for reversibility). Board: `forward-gaps`.
- [ ] **27y · Orphan instead of drop.** A field or entity the program stops
      declaring is marked orphaned, reported with a warning, and kept for
      reverts; pruning is an explicit step, for exposure risk or a customer
      requirement. Ruled 2026-10-01 (ask 20). Board: `forward-gaps`.
- [ ] **27z · Audit: an `acl` added to existing rows grants the creator
      from the command log.** Ruled 2026-10-01 (ask 4), with the owner's
      flag that it needs a security audit. Board: `forward-gaps`.
- [ ] **27aa · Turn off the extra security layers (v2).** The owner,
      2026-10-03: on the future roadmap. A deployment setting, default on,
      that switches off the security stream, signed checkpoints and
      attestation, sealing, and per-call checks for an app that does not want
      their cost. The command log cannot go without a second write path, since
      undo, safe retries, restore and the erasure receipt depend on it; the
      setting must say so. Board: `security-layers-off`.
- [ ] **27ab · An access visualizer.** The owner, 2026-10-03: on the future
      roadmap. Highlights every field reachable from outside its tenant and
      draws the chain that reaches it (a tenant grant to the deployment's
      space, then a public op; shares, windows, feeds, share links, API
      keys). Built from the scope manifests and grants the compiler already
      has. Board: `access-visualizer`.
- [ ] **27ac · A build-time credential for static sites.** The owner,
      2026-10-03 (`public-data` ask 20): *"yes as a backup option for future
      roadmap."* A capability token limited to public ops, used while a
      static site builds so its data is baked into the pages. A leaked one
      exposes nothing that was not already public. `plan-public-data.md`
      §5. Board: `build-time-credential`.
- [ ] **27ad · Public data.** Ruled 2026-10-03 (`public-data`, asks 1 to
      23); go to build 2026-10-10, before the first deployment. Public data lives in the deployment's space,
      declared `public` / `public*` and refused on sealed fields; public
      reads are cached, unaudited and served from a read layer of their own;
      a tenant's writing stays in its own space; tenants read the
      deployment's space at three developer-decided levels. First check: a
      guard can read the caller's tenant. `plan-public-data.md`. Phase 1
      built 2026-10-10, phase 2 (public writes, per-tenant CORS, proxy
      slow-down, monitoring) built 2026-10-11. Open: asks 29 to 33.
      Board: `public-data`.
- [ ] **27ae · Research: a datastore built for Ordo.** The owner,
      2026-10-03: research a database, datastore or data structures that
      run Ordo natively, possibly as a Postgres extension, to make Ordo's
      computation as efficient as possible. Research only; nothing is
      decided. Places where Ordo pays today for running on plain Postgres:
      each write's one chain link (one hash over the previous link, but
      appended under a per-tenant lock, so a tenant's writes queue, with 4
      round trips per append); guards translated into SQL;
      the language's own semantics re-imposed on SQL (text order with
      `COLLATE C`, a null in an `in` list: fixes 25m and 25n); one database
      per tenant and the connections that costs (25g); sealed payloads
      bound to their rows; scope manifests and live updates; the command
      log's consumers and pruning; the device engine kept in step with the
      server (refine parity). The output is a written comparison: a
      Postgres extension, a storage engine of Ordo's own, or staying on
      plain Postgres. Board: `native-datastore`.
- [ ] **27af · Sections of the hash chain (v2).** The owner, 2026-10-03:
      section the chain so a stretch can be verified on its own, without a
      long walk. Today a verifier starts at a signed checkpoint and walks
      forward to the head (`Store::verify_from`,
      `crates/ordo-commandlog/src/store.rs`); it cannot check a closed
      stretch in the middle. A section would run between two signed
      checkpoints, carry both ends, and verify alone: in parallel, once,
      and recorded as verified. A root per section (a Merkle tree over its
      entries) would also prove one entry belongs to it without reading the
      rest. The chain's frozen encoding and its one-link-per-write rule do
      not change. Board: `chain-sections`.
- [x] **27ag · The reserved-word list in `definition-language.md` §2 is
      generated from the `Keyword` enum. Built 2026-10-04** (commit ae6094f):
      DL §2 matches `Keyword::ALL` in order, and `reserved_words.rs` checks
      both ways, that every keyword is refused as a field name, and that
      every named contextual word is accepted. Ruled 2026-10-03
      (`syntax-additions` ask 15). Today they disagree: the document lists
      `read write open route std`, which the lexer does not reserve, and
      omits `space auth compliance cond`, which it does
      (`crates/ordo-syntax/src/token.rs`). Board: `syntax-additions`.
- [ ] **27ah · Build the syntax additions.** Ruled in substance 2026-10-03
      (`syntax-additions`); no go to build. `public` / `public*` and public
      ops, `deployment` entities, `$tenant` and caller-side guard atoms,
      `apikey` profiles, `prev`, orphans with `ordo plan --prune` under a
      second confirmation, the `settings { }` block, `external` entities.
      New reserved words in `crates/ordo-syntax/src/token.rs`, then
      `crates/ordo-gen/src/spell.rs` and the CDK vocabulary.
      `plan-syntax-additions.md`. Board: `syntax-additions`.
- [ ] **27ai · Ordo Lang: the language as a library, without the platform.**
      The owner, 2026-10-03: *"just the language as a driver/library for
      other languages that can be used without the whole platform - define
      the ordo and get ops, and equivalent in-language methods."* Already
      planned as `tooling-and-embedding-plan.md` Track B: an app keeps
      `.ordo` files beside its source, the build generates a typed function
      per op in the host language, and the engine is linked in-process,
      talking to Postgres directly with no Ordo server (embedded mode, §B4:
      WASM for TS/JS, PyO3 for Python, cgo for Go, Panama for Java, over a C
      ABI). Generated functions follow §B3: reads end in `.fetch()`,
      `.live()` or `.pages()`, writes in `.commit()`, typed errors. Not
      included in embedded mode: multi-node invalidation, the identity
      service, channels. Moving to a full deployment later is a config
      change, since the artifacts are the same. The TS client and the
      interface descriptor are built (14d, B1); Python, Go and Java clients
      (§B6 item 4) and embedded mode (item 5) are not. Board: `ordo-lang`.
- [x] **27al · The audit's decisions, built** (ruled 2026-10-05,
      `audit-2026-10-04`): the playground spells money as the door does;
      duplicate output names in a join are refused as
      `projection_unknown_field`, and documented; the artifact's cost class
      uses the schema; the storage sweep records deletions in the security
      stream, one entry per pass per tenant; `money(0) * money(0)` is
      refused like other places; DL records 12 places as the ceiling. Asks 7
      and 8 need no build now (8 binds 27m). **Built 2026-10-05:** the
      playground's money (commit 65c6ff7) and the sweep's security entries
      (commit d7dc60f, tests `a_pass_records_one_entry_per_tenant_in_its_own_stream`,
      `a_pass_with_nothing_to_delete_records_nothing`). The compiler items
      (2026-10-05): duplicate output names refused, documented in DL
      (commit 2fd23f1); the artifact's cost class uses the schema, with a
      fold's bound no longer bounding the host work before it (b3d5d06; the
      playground and wasm follow); money times money is a decimal at every
      places value (6afa5ef); DL records the 12-place ceiling (bd9be06).
      Board: `audit-2026-10-04`.
- [ ] **27an · Follow-ups ruled 2026-10-05.** **Built 2026-10-05** except
      the migration hint (it waits for `ordo evolve`): `filled` on another
      type is `filled_type` (8f0c549); a group or reduce over a sealed field
      is plain SQL, `host_work_bound` removed (928f798); a refused sweep pass
      records `deleted=0` (2a517ec). `filled` refused on any type
      but `date` and `datetime`; a group or reduce over a sealed field is
      plain SQL (clear `host_open` at the fold, drop `host_work_bound`'s fold
      branch); a sweep pass that storage refused entirely writes a security
      entry with `deleted=0`; a migration refusal suggests a text field's 3
      most used values (never sealed; dev and staging by default, production
      when the deployment owner turns it on). Boards: `dl-spec-gaps`,
      `audit-2026-10-05`, `forward-gaps`.
- [x] **27ap · One-way gaps, the builds ruled 2026-10-05. Built
      2026-10-05** (merge 492af14): decline (d47fc81, a944c8e;
      `/auth/invitation/peek`, `/auth/invitation/decline`,
      `/auth/share/decline`, declined invitations listed until they would
      have expired); `std.space.detachBucket` (77b6145);
      `docs/howto-byo-custody.md` (12900dc); attestation off, readback and
      dead letters (a038156: `std.space.attestation`,
      `std.space.attestationDeadLetters`, `/health/deep`); `std.space.shares`
      (25a2fb3); self-serve readback on `/health/deep` (06bb1bd). A declined
      share appears only in the tenant's security stream, since it never
      makes a seat. Decline an
      invitation or share ticket; `std.space.detachBucket` (owner only,
      step-up, refused while objects are held) and one guide to every
      bring-your-own custody option; switch off and read back the
      attestation destination; webhook dead letters readable by the tenant
      owner and the deployment owner; `std.space.shares`; the deployment's
      self-serve mode and claim-code presence readable. Boards
      `person-can-leave`, `bucket-detach`, `attest-sink-readback`,
      `small-readbacks`.
- [ ] **27aq · Leaving and account deletion, gated by a tenant's release
      rule** (ruled 2026-10-05, the owner's design): leaving a tenant and
      deleting one's own account exist as mechanisms the app decides; a
      tenant carries an app-written release rule (always true, always false,
      or conditions), a membership ends only when it clears, and an account
      goes only when every membership has. Plan: `docs/plan-member-release.md`.
      **Built 2026-10-05, R1 to R4 and R6:** the `release member` line and
      `$member` (8c8b4b5); `std.space.leave` with remove and reassign
      (0f01907, `crates/ordo-host/src/release_ops.rs`); `POST
      /auth/account/delete` on one shared removal path (e2087cc); guide
      `docs/howto-leave.md` (401197d). The leave is one command group
      through the write protocol, revertible, entity guards not re-run
      against the leaver (8c97b7e). Tests
      `crates/ordo-host/tests/member_release.rs` (17),
      `crates/ordo-syntax/tests/release.rs`,
      `crates/ordo-schema/tests/release.rs`,
      `crates/ordo-cli/tests/release_check.rs`. **Owed:** R5 waits on 24a;
      R7 after the first deployment; account delete does not yet spend
      invitations addressed to the person; no test that it clears recovery.
      **Ruled 2026-10-06 (ask 11):** a leave keeps the seat's name,
      renaming on leave is an app setting off by default; account delete
      marks the account deleted and keeps it, removal is an app setting
      default never. **Built** eb2c0b0f, 3136921e: `ORDO_MEMBER_LEFT_NAME`,
      `ORDO_ACCOUNT_REMOVAL` (`never`, `now`, `<n>d`), `members.status`
      readable in programs.
      Board: `person-can-leave`.
- [x] **27ar · Pausing a tenant, and the layer outside it** (ruled
      2026-10-05): pause and resume a whole tenant, and an in-between layer
      where its people can still sign in and do some things, likely at the
      deployment level. Plan `docs/plan-tenant-pause.md`. **Built
      2026-10-05:** `nopause` (f987ecf); the paused state, the gate,
      `space_paused` (423, asked as ask 8), `std.deployment.pauseTenant` and
      `resumeTenant`, `std.space.pause` and `resume` for roles the app
      developer grants (default none) (4c0ff93); `docs/howto-pause.md`
      (9dea00e). Tests `crates/ordo-host/tests/tenant_pause.rs` (18).
      **Owed:** live streams closed with a terminal frame and the SDK's
      `onPaused` (P5); scheduled work during a pause once tenants run any;
      resume by a deployment routine (P6). Per reason, the status and
      message a paused tenant answers are the app developer's (ruled
      2026-10-06, ask 8; built eaab5633, `ORDO_TENANT_PAUSE_ANSWERS`).
      Board: `tenant-pause`.
- [ ] **27as · Maintenance mode: pause every tenant at once** (asked
      2026-10-05). A deployment-wide pause; what still answers is decided as
      for one tenant; the deployment's own space stays reachable. Planned
      with 27ar. Board: `maintenance-mode`.
- [x] **27am · Refinement on by default.** Ruled 2026-10-05: every ordinary
      read accepts filter and pick on its outputs. Owed: program reads
      register tier `Basic` (today `None`, `crates/ordo-host/src/lib.rs`),
      public reads stay `None`, platform ops unchanged; the interface states
      it; the SDK's generated functions gain `.where(...)` and `.pick(...)`
      that send the envelope (`tooling-and-embedding-plan.md` §B2, §B3), and
      the device copy evaluates the same query; live and sync stay refused
      with a refinement until designed. No off switch (ruled 2026-10-05,
      `dl-spec-gaps` ask 6). **Owed with it:** `docs/howto-cost.md`, a guide
      for app developers to cost classes and the gate that refuses a
      refinement (`refinement_unbounded`), since it can surprise them later.
**Ruled 2026-10-05 (asks 1 to 6):** filters compile into the plan;
      sync takes `where`, refuses `pick`; a sealed unindexed condition is
      refused at the door; count over a filtered list this round; size
      limits (16 conditions, 200 `in` values) as settings, documented.
      `ordo check` prints each read's refinable fields (ask 4).
      **Built 2026-10-05, R1 to R7:** server 4adc91e, 333de34, 5d382ce;
      `$id` 877ce19; SDK ba846e5, 0a9d452; React 93881a3; live and sync
      fd4801d, e929942; `ordo check` line d2042ee; `docs/howto-cost.md`
      0521ae9 (also fixed the venue gate message's stray spaces); documents
      4a90405. The notes app's generated client was regenerated and its
      hand-written platform specs gained the `order` field 27o requires.
      R8, the audit, remains. Plan: `docs/plan-refinement.md`.       Board: `refinement-default`.
- [ ] **27ak · Money declares its places; a storable `decimal(p)`.** Ruled
      2026-10-04 (`dl-spec-gaps` ask 3, the middle option). **Built
      2026-10-04** (commits 3933a89 to a1c0efb): `money(n)` for places 0 to
      12, in the artifact and the interface; source writes `29.99` and the
      compiler converts in conditions, writes, defaults, params, bounds,
      guards and checks (finding 6 stays fixed, proved on Postgres); results
      spell money as a string with the field's places; storable `decimal(p)`
      (`numeric`), reaching money only through a round; the chain's value
      tag `6 Decimal` by presence, one canonical spelling; the device engine
      compares decimals exactly (14 new parity cases). Tests
      `crates/ordo-sema/tests/money_places.rs`,
      `crates/ordo-conformance/tests/money_places.rs`,
      `crates/ordo-exec/tests/money_wire.rs`, the chain's golden. **Still
      open:** `allocate` at places is built (`allocate_at_places`) but not
      wired, because its one legal site, `each`, is unbuilt; and the default
      for bare `money` (built as `money(0)`; ruled 2026-10-05 to keep it,
      with a warning on bare `money`, built in commit 9cdf5b4: `ordo check`
      warns on stderr and still exits 0). Board:
      `money-types`.
- [ ] **27aj · The language reference and the compiler disagree in 18
      places.** Found 2026-10-03 writing `examples/syntax/` (every file
      there passes `ordo check`; the list is its README's "Findings"). Some
      settled syntax does not compile: `text(64)`, bare `duration`,
      `$x: date filled`, `std sealed`, a `check` that walks a ref, a
      hand-written `groupMembers.group` walk (`group` is reserved), blob
      `.mime`/`.size`. Some rules are not enforced: `unbounded` only for
      sealed host work, `reproducible` does not refuse `since`, `allocate`
      outside `each` is accepted, `refinable` still reports tier `none`.
      Some lean syntax is unbuilt: `enum(...)`, list parameters and `each`,
      routines, triggers and the other blocks. Two how-tos contradict the
      reference (money in minor units in `howto-values.md`; filtering an
      unindexed sealed field in `howto-custody.md`), and `howto-operating.md`
      still says only password sign-in is served. **2026-10-03:** the sealed
      filter and sign-in how-tos are corrected. Money is open: the compiler
      agrees with `howto-values.md` (minor units, `crates/ordo-sema/src/lib.rs`
      refuses `29.99`), so the reference is the side that disagrees (ask 3).
      **Settled 2026-10-04** by a new design, 27ak. **Built 2026-10-04**
      (commits 96157e5, 5326bba, 1533961): `text(n)` (a row invariant the
      diff and the apply carry), `filled` (the generated client fills a
      `date` or `datetime`), `std sealed`, and the `groupMembers.group` walk
      compile; `reproducible` refuses `since` and `filled`; `allocate` in a
      write's braces is refused; a paged sealed projection is `bounded-host`
      when every opened row reaches the page; lean unbuilt syntax carries
      "Not built yet" notes in DL. **Owed, bigger than a parser fix:** bare
      `duration` (the two-component model), a `check` that walks a ref
      (triggers on every entity in the walk), blob `.mime`/`.size`.
      **Undecided** (board asks): how an op opts in to refinement, and which
      types `filled` may mark. Tests: `crates/ordo-conformance/tests/dl_spec_gaps.rs`,
      `text_bounds.rs`, `crates/ordo-host/tests/text_param_bound.rs`,
      `gen_ts.rs`. Board: `dl-spec-gaps`.
- [x] **25p · `ordo check` exits 0 when an op does not compile. Fixed
      2026-10-03.** Found writing `examples/syntax/`: the command printed
      `DOES NOT COMPILE` for an op and still exited 0. It now exits 1 and
      names each failing op. `unresolved_blind_index` does not count: that op
      needs key material only at runtime. `--expect-fail <op>` allows a
      known failure and fails once the op compiles; both CI smoke steps pass
      `--expect-fail giftOrders` (F0). Tests:
      `crates/ordo-cli/tests/check_exit.rs`. Board: `check-exit-code`.
- [x] **25r · A sealed field read through a join lost its seal. Fixed
      2026-10-04** (commits 67b8626, 2056893). Seen: `customers[name]
      orders[placed, memo]` with `memo` sealed was classed `sql`, and the
      serving path answered 200 with the stored envelope as hex text (no
      plaintext disclosed, no key released, no security event), while the
      single-source read was refused 501. Cause: `bind_multi_source` never
      set `host_open`, the one list every consumer reads sealed-ness from.
      The same hole was in `let` read as a source and in `append`; both
      fixed. An `append` whose arms disagree on a sealed column is now
      `append_misalignment`, and a group output named like a sealed value is
      `sealed_unsupported`. Regression looks like: such an output classed
      `sql`, printed without `(sealed)`, or a value starting `\x` served as
      text. Tests `crates/ordo-conformance/tests/sealed_through_join.rs`
      (8), `crates/ordo-exec/tests/sealed_through_join.rs`. Board:
      `sealed-through-join`.
- [x] **25s · A join could not project fields from a filtered or guarded
      second source. Fixed 2026-10-05** (commit bf85ad4). Seen:
      `customers[name] orders(status == "done")[placed]` and `projects[name]
      tasks[title]` failed with `projection_unknown_field … available:
      [customer]`. Cause: that term's sub-select returned only its join key.
      It now carries the term's open fields, and a sealed field it names keeps
      its seal, anchored to its row (as 25r). Regression looks like: that
      error again, or a sealed field through a filtered term classed `sql`.
      Tests `crates/ordo-conformance/tests/join_filtered_term.rs` (5). Board:
      `join-filtered-term`.
- [x] **25t · The production door never ran the token gate. Fixed
      2026-10-04** (commit 801c48d), found by the Fable audit. Pre-existing:
      `SwappableAccess`, the door's access checker in production, implemented
      `check` only, so `token_gate` fell to the trait's default `Pass`. A
      withdrawn delegation or a dead share grant was never refused as
      `delegation_required` or by the share gate; only tests that built the
      door from `IdentityAccess` directly saw the gate. It now forwards
      `token_gate` and `token_known`. Regression looks like: a revoked
      delegation answering `access_denied` instead of `delegation_required`,
      or not refused at all. Test `the_swappable_door_forwards_the_token_gate`
      (`crates/ordo-host/tests/delegation.rs`). Board: `door-token-gate`.
- [x] **25u · Shutting down the security drain could lose events in flight.
      Fixed 2026-10-04** (commit fbf9acc), found by the Fable audit as the
      cause of the `attached_tenant.rs` flake. `SecurityDrain::shutdown`
      aborted its task; a pass holds drained entries across awaits, so an
      abort mid-pass dropped them, neither written nor requeued. Shutdown
      now wakes the loop, waits for the pass, then flushes. Regression looks
      like: security events missing after a graceful shutdown. Test
      `crates/ordo-host/tests/secdrain_shutdown.rs`. Board: `secdrain-shutdown`.
- [x] **25v · A filtered join term over-reported what an op reaches.
      Fixed 2026-10-05** (commit be6a10d). Found by the Fable audit of 25s:
      the filtered term's sub-select carried every open field and was never
      pruned, so an op composed over it claimed host work and a sealed
      field it never read. A new pass (`crates/ordo-sema/src/prune.rs`) keeps
      in every sub-select only the columns its outer query reads, with their
      `host_open` entries. Regression looks like: a composed op over a
      filtered join reaching fields it does not read, or classed host work
      with no key release. Tests `join_filtered_term::an_op_over_a_filtered_term_reaches_only_what_it_reads`,
      `a_filtered_term_used_only_in_a_filter_or_sort_keeps_those_columns`.
      Board: `filtered-term-reach`.
- [x] **25w · An open field could not be indexed on its own. Fixed
      2026-10-05** (commit ac22ff6, in the R8 audit). `index (status)` was
      refused pointing to `status: … index`, which did not parse. The field
      modifier `index` now exists on open fields, lowering to a single-column
      index the diff and apply already handle; `index` on a sealed field is
      `index_on_sealed`, pointing at `indexed`. Regression looks like: the
      one-column refusal naming a form that does not parse. Tests
      `graph.rs::a_field_modifier_index_is_a_single_column_index`,
      `schema_apply.rs::a_field_modifier_index_is_added_to_an_existing_table`.
      Board: `single-field-index`.
- [x] **25x · Four refinement values reached Postgres as a 500. Fixed
      2026-10-05** (commit 3e5b4db, found by the R8 audit's hostile sweep):
      an impossible date (`2026-02-31`), a NUL in text, a repeated `pick`
      field, and a number past what a literal can hold. All four are now
      `invalid_refinement` at the door; the date and NUL checks are shared
      with parameters, which had the same 500s. Regression looks like: any
      500 from the sweep in `crates/ordo-host/tests/refine_audit.rs`. Tests
      there and in `crates/ordo-api/tests/api.rs`. Board: `refine-500s`.
- [x] **25y · Ref fields were not indexed, though the reference says they
      are. Fixed 2026-10-05** (commit dafbb56): each ref gets a `(space, ref)`
      index `entity$field$ix`, skipped when another index already starts with
      it; existing tables get it as a concurrent add. Test
      `ref_fields_index_automatically`,
      `a_ref_added_to_an_existing_table_gets_its_index`. Found 2026-10-05 by the R8 audit: DL "### Indexes [settled]"
      says ref fields index automatically, but the DDL emits only the
      foreign key (`crates/ordo-schema/src/ddl.rs`, `foreign_key`), so a
      filter on a ref scans. Settled syntax the compiler does not honour is
      a compiler debt (ruling 2026-10-03, `dl-spec-gaps`): emit one index per
      ref field, with a diff `AddIndex` for existing tables. Board:
      `ref-index`.
- [x] **27ao · Serve reads that return a sealed field. Built 2026-10-05**
      (commits 612cf27, 0657485): `Executor::venue` replaces the two
      refusals; a bounded-host read fetches its page with plain SQL, takes
      one `read` lease at the first envelope that opens, opens the page, and
      returns plaintext. `unbounded-host`, a whole read bounded only by
      paging, and an anonymous caller stay refused, each naming the field.
      Tests `crates/ordo-exec/tests/sealed_reads.rs` (6),
      `crates/ordo-host/tests/sealed_reads.rs` (2). Found: Found 2026-10-05
      by the R8 audit: the executor refuses every read classed as host work
      (`crates/ordo-exec/src/lib.rs` around 1374 and 2155, 501
      `sealed_unsupported`, "host staging is not built"), paged or not,
      refinement or not, and the message names no field. `open_sealed` is
      built; only plain SQL and blind-index plans reach it, and a plan that
      returns a sealed value is never one of those. So the door returns no
      sealed value at all: an indexed sealed field can filter only when it
      is left out of the result (`refine_audit.rs`). Sealed fields are
      write-only to an app today. The door cannot
      warn, because the class comes from the projection. Owed: open the
      payloads for the page's rows (bounded host work), and name the field
      in the meantime. Board: `serve-sealed-reads`.
- [x] **25z · A saturated connection pool answered 500. Fixed 2026-10-05**
      (commit 08ea743): a new class `server_busy`, 503 with `Retry-After: 1`,
      on op reads and writes, live, sync, platform ops, the space resolver
      (which no longer marks the tenant closed) and the blob door. Auth keeps
      its own typed 503 `credential_store_unavailable`. Tests
      `server_busy_is_a_retryable_503_with_retry_after`,
      `a_full_pool_answers_server_busy_and_the_retry_lands`. Found 2026-10-05:
      the refinement audit's sweep, run under full-suite load, saw 4 of
      5,243 requests answer `500 internal: pool saturated: acquire timed
      out`. The pool ruling says a saturated acquire is *"a fast typed
      failure the door can render retryable"*; the door renders it as a
      server fault. Owed: a typed, retryable 503. Board: `pool-saturated-500`.
- [x] **25aa · Changing a stored money or decimal field's places is
      invisible to migrations.** Ruled 2026-10-05: refused unless the change
      says whether to rescale the stored amounts. **Fixed 2026-10-05**
      (5c8200b, a685c73): the diff and the load gate see a change of places
      (B11); the plan refuses it with `places_choice_required` until
      `--places <entity>.<field>=rescale|keep` states it; a rescale runs in
      the migration's transaction and refuses a division that loses digits;
      `$places` keeps each stated choice. Regression looks like: `money(0)`
      to `money(2)` applying with no choice, or 29.99 reading as 0.2999.
      Tests `crates/ordo-host/tests/money_places.rs`:
      `money_0_to_2_unstated_is_refused_naming_the_field`,
      `a_stated_rescale_keeps_29_99_as_29_99`,
      `a_rescale_that_would_cut_digits_is_refused_naming_the_rows`,
      `a_decimal_narrows_only_when_every_value_fits` and 4 more;
      `crates/ordo-cli/tests/plan_places.rs` (2). A rescale moves `rev` on
      every row it rewrites (8711f42f, audit 2026-10-05d finding 3). A
      revert past a rescale is 25ad. Found 2026-10-05 by the one-way gap review
      (OW-13, `docs/audit-one-way-gaps.md`), confirmed by the lead: every
      `money(p)` is `bigint` (`crates/ordo-schema/src/ddl.rs:99`) and
      `same_type` cuts a type at `(` (`diff.rs`), so `money(0)` to
      `money(2)` or `decimal(2)` to `decimal(4)` is no change to the diff or
      the shape gate. Stored minor units would then read 100 times smaller.
      Contrary to the 2026-10-05 ruling that a stored value is never
      rescaled silently. Board: `money-places-change`.
- [x] **25ad · A revert past a places rescale writes old-scale amounts.**
      Found 2026-10-05 by the 25aa agent, confirmed by the lead: a revert
      re-applies a before-image from the history, and one taken before a
      rescale holds the old minor units; `crates/ordo-exec/src/undo.rs` does
      not check for it. Ruled 2026-10-06: money only; the revert converts
      the old minor units to the current places, and refuses a lossy
      conversion unless the app developer allows rounding. **Fixed
      2026-10-06** (c3dbba2a): `$places` records each rescale's chain
      position; the revert converts by the net rescale, half-even when
      rounding is allowed. Regression looks like: 29.99 coming back as
      0.2999, or `cas_miss` on a row nobody wrote. Tests
      `crates/ordo-host/tests/revert_past_rescale.rs`:
      `a_revert_past_one_rescale_up_restores_29_99_as_29_99`,
      `a_revert_past_a_rescale_down_that_loses_digits_is_refused_unless_rounding_is_allowed`,
      `a_revert_past_a_keep_converts_nothing`,
      `a_revert_past_two_rescales_converts_by_both`. Board:
      `revert-past-rescale`.
- [x] **25af · A deleted account could get back in through an invitation
      ticket. Fixed 2026-10-06** (f7c0aae8): the directory refuses a new
      link for a deleted person, and the host refuses before the ticket is
      spent. Regression looks like: such a ticket answering 200 with a
      token. Tests `a_deleted_account_is_given_no_new_authenticator_link`
      (ordo-identity), `a_ticket_that_names_a_deleted_account_registers_nothing`
      (`crates/ordo-host/tests/auth_passkey.rs`). Board: `deleted-via-ticket`.
- [x] **25ag · The registration sweep removed a person who had left every
      tenant. Fixed 2026-10-06** (125ac47f): a left seat now still shows the
      person was admitted. Regression looks like: `registration-expired` for
      someone once admitted. Test
      `a_person_who_was_admitted_and_then_left_every_tenant_is_kept_by_the_sweep`
      (`crates/ordo-host/tests/registrations.rs`). Board:
      `sweep-removes-leavers`.
- [x] **25ah · The deployment's record of a bucket change lost who asked.
      Fixed 2026-10-06** (03887a8c). Regression looks like: subject
      `system:attach-bucket`. Test
      `the_deployments_record_of_a_bucket_change_names_the_member_who_asked`
      (`crates/ordo-host/tests/attach_bucket.rs`). Board:
      `bucket-record-caller`.
- [x] **25ak · A text value lost its quote marks, and a backslash was a
      server error. Fixed 2026-10-08** (cf69d005): `call_source` escapes
      instead of stripping. Regression looks like: a stored value missing
      its quotes. Test `crates/ordo-host/tests/text_params.rs`. Board:
      `text-quotes-lost`.
- [x] **25al · An op that saves one row and deletes another stayed open to
      workers. Fixed 2026-10-08** (e7274550). Test
      `an_op_that_sets_and_deletes_is_closed_to_delegation_like_a_delete`.
      Board: `mixed-delete-delegable`.
- [x] **25am · The package workflow could publish from any branch when run
      by hand. Fixed 2026-10-08** (52e888e6). Board: `packages-any-ref`.
- [x] **25ar · An invitation admits no newcomer under the default
      invite-only. Fixed 2026-10-09** (74e3d5f1, 50ca2962). Found 2026-10-09 (audit
      finding 2). Cause: the directory resolved identity first, and under
      `InviteOnly` an unknown subject got `NotInvited` before admission
      read the `invitation`. Fix: `/auth/complete` checks a newcomer's
      invitation and spends it in one statement before the identity step
      (`Admission::take_newcomer_invitation`), then mints the person with
      `Onboarding::Invited` and admits them at its role. If the mint, the
      passkey store or the admission fails after the spend, the invitation
      is given back (`Tickets::give_back`, undoing only that spend). Every method
      works. An email `begin` with a valid invitation marks the ceremony,
      so the worker mails an unknown address. Owner ruling on federated
      ask 9. Regression looks like: an invited newcomer refused under
      invite-only, one invitation admitting two people, or an invitation
      lost to a failed sign-up. Tests
      (`crates/ordo-host/tests/auth_invitation.rs`):
      `invite_only_a_newcomer_with_an_invitation_signs_up_with_a_password`,
      `..._with_a_passkey`, `..._with_an_emailed_code`,
      `..._through_discord`,
      `invite_only_a_spent_expired_other_tenant_or_named_invitation_is_the_one_refusal`,
      `invite_only_two_newcomers_racing_with_one_invitation_spend_it_once`,
      `invite_only_an_invitation_spent_before_a_failed_mint_is_given_back`;
      `invite_only_mints_an_invited_newcomer_and_nobody_else`
      (ordo-identity). Board: `invite-newcomer`.
- [x] **25bh · A device-copy screen re-read every row on every sync batch. Fixed 2026-10-11** (I0 to I10, plan-client-loading-fixes.md): settles at 1.0x to 1.1x of the load.
      Found 2026-10-10 by the client blocking audit (H1):
      `clients/ordo-react/src/device.ts:489-501` with `hooks/useDevice.ts:83-90`.
      Work grows with the square of the rows: 3,000 rows took 7.2 s to settle
      against 0.6 s to load. Batches for other ops trigger re-reads too. Plan:
      `docs/plan-client-loading-fixes.md`. Board: `device-reread-quadratic`.
- [ ] **25bg · A refused write's class varies on CI.** Found 2026-10-10:
      `shopfront_app::a_customer_cannot_reach_the_staff_ops` expected
      `not_found` for a customer's `addProduct` and got `access_denied` once on
      CI (a14136fb); green locally and on the next run. Either answer refuses,
      but the class should not depend on timing. Board: `refusal-class-timing`.
- [x] **25bf · A field an op returns can be marked as read only by a guard.
      Fixed 2026-10-10** (9a11582c): found by the consent ceremony. The
      field-scope manifest marked `pendingExpenses`'s read of
      `members.displayName` (examples/assistant) as guard-only, though the op
      returns it, so the consent disclosure understated what the op reads.
      Cause: `op_scope` (ordo-plan `artifact.rs`) counted only the sources a
      body names as the body's; a ref hop's target in a projection,
      condition or sort fell to "read by a guard". Fix: a read is also bound
      with no guard inlined (`ordo_sema::bind_op_body_only`, never executed),
      and every entity it touches is the body's; a write statement's dot-path
      past a ref counts the same. An entity both the op and a guard read is
      the op's. Regression: a field reached through a ref hop shows "read by
      a guard, not by the op", or the disclosure lists it under examines.
      Tests: `a_projection_through_a_ref_hop_is_read_by_the_op`,
      `a_condition_through_a_ref_hop_is_read_by_the_op`,
      `a_sort_through_a_ref_hop_is_read_by_the_op`,
      `a_write_condition_through_a_ref_hop_is_read_by_the_op`,
      `a_field_only_a_guard_examines_keeps_the_guard_marker`,
      `a_field_both_the_op_and_a_guard_read_is_read_by_the_op`
      (ordo-plan scope_manifest),
      `ordo_consent_a_field_reached_through_a_ref_hop_is_disclosed_as_read`
      (ordo-host delegation_consent). Board: `manifest-guard-only`.
- [x] **25be · A guest's displayName reads back as their id. Fixed
      2026-10-10** (073b9a2e): found by the assistant example. Cause:
      administrator `std.space.admit` wrote the id when the call named no
      `display`, the same fallback sign-in admission had (25ba). Both now use
      `MemberOps::display_name_of` (directory name, the id only when none).
      Regression: a member admitted with no display reads back as their id
      while the directory holds a name. Tests:
      `a_staff_member_admitted_without_a_display_reads_back_by_the_name_the_directory_holds`
      (assistant_app), `every_way_in_names_the_member_by_the_name_the_directory_holds`
      (admission). Share accept takes the guest's own `display_name`. Board:
      `guest-display-name`.
- [x] **25bd · A bulk `set` that matches no row answered 500. Fixed
      2026-10-10** (073b9a2e): found by the assistant example. Cause: the write
      protocol builds no command for an empty bulk match, and
      `ordo-exec/src/lib.rs` treated "no pending group" as
      `log_append_failed`. Per G11 (`definition-language.md`: "an empty
      selection is the ordinary vacuous no-op") the write now succeeds with
      zero rows affected and appends nothing. Regression: a bulk write whose
      condition matches nothing answers 500 or appends an empty group. Tests:
      `a_bulk_write_that_matches_no_row_succeeds_and_appends_nothing`
      (ordo-exec commandlog),
      `an_approval_that_matches_no_expense_is_an_empty_success_not_a_server_error`
      and `the_assistant_cannot_approve_over_the_limit` (assistant_app).
      Board: `empty-set-500`.
- [x] **25bc · `http://*.localhost` origins were refused for passkeys and
      return addresses. Fixed 2026-10-10** (a1b3a34b): the loopback check
      matched only `localhost`, `127.0.0.1` and `[::1]`
      (`passkey.rs`, `origin.rs`). It now also accepts exactly one label under
      `localhost`, and a passkey origin there must sit under the RP id.
      CORS lists took any exact `http://` origin already. Regression looks
      like: `http://shop.localhost:5181` refused in `ORDO_AUTH_ORIGINS` or
      `ORDO_AUTH_RETURN_URLS`, or `a.b.localhost` accepted. Tests:
      `one_label_under_localhost_is_loopback_and_nothing_broader`
      (ordo-identity), the added asserts in the passkey origin test,
      `return_urls_accept_a_local_subdomain_and_nothing_broader` (ordo-host),
      `local_subdomains_are_listed_and_matched_exactly` (ordo-api). Board:
      `localhost-subdomains`.
- [ ] **25bb · `members[id, principal]` is a parse error.** Found 2026-10-10
      by the teamspace example: `principal` is a reserved word, so a page
      cannot project a seat's principal. Board: `principal-projection`.
- [x] **25ba · A member admitted at sign-in was named by their account id.
      Fixed 2026-10-10** (a1b3a34b): `admission.rs` `admit` passed the
      principal id as the display name. It now reads the person's display name
      from the directory and falls back to the id only when there is none.
      Regression looks like: a seat shown as `usr_...` after an open,
      invitation, provider or first-comer admission. Tests:
      `every_way_in_names_the_member_by_the_name_the_directory_holds`
      (admission), `an_invited_colleague_is_shown_by_the_name_they_gave`
      (teamspace_app), and the shopfront order list in
      `a_customer_sees_only_their_own_orders_and_staff_see_all_and_ship_them`.
      Board: `admission-display-name`.
- [x] **25ay · A create under a write guard through a parent ref was always
      refused. Fixed 2026-10-10** (229001b8): the write path classifies a
      value as an id only under 40 characters, and minted ids are 47 since
      25e, so the create gate compiled to false (`ordo-sema` create gate). The
      gate now reads the field's type from the schema. Regression looks
      like: `access_guard` on a create a guard should allow. Tests
      `guards_multihop::the_create_gate_pins_a_long_minted_parent_id`,
      `teamspace_app::a_member_adds_tasks_only_to_projects_they_are_on`.
      Board: `guarded-create-long-id`.
- [x] **25ax · The shipped server answered 404 on new sign-in routes. Fixed
      2026-10-10** (825733e4): the claim (linking), subdomain hand-off,
      outside-provider callback and `/auth/identities` routes were mounted only
      on the router the in-process tests use, not on the one `ordo-host`
      serves. Regression looks like: a 404 from a real server on a route the
      tests pass. Test `the_served_door_mounts_the_claim_hand_off_callback_and_identity_routes`
      (`crates/ordo-host/tests/notes_app.rs`). Both routers now call one list, `auth::auth_routes`; tests `the_served_router_takes_its_auth_routes_from_the_shared_list` and `every_auth_path_in_the_sources_is_mounted_on_the_shared_list` (`crates/ordo-host/tests/served_routes.rs`) keep it one list. Board: `served-routes-404`.
- [x] **25aw · A missing grouped-`let` row reads 0; the docs said null. Fixed 2026-10-09** (f1294697): the spec says an empty bucket sums and counts 0, so the docs were wrong. Found
      2026-10-09 (25at): modeling-guidance C7 and the vendorsBillingLag
      example say a row with no `let` match reads null; the conformance tests
      show 0. One of them is wrong. Board: `let-missing-row`.
- [x] **25av · A refinement the binder refuses was answered 500. Fixed
      2026-10-09** (2b754923): the door reported any bind failure of a
      `$refine` as `internal`. Cause: the binder's message arrives as
      `bind: [code] ...`, and `classify` has no class for the `bind` prefix,
      so every refusal the caller caused fell to 500. Fix:
      `refine_bind_refusal` (`crates/ordo-host/src/lib.rs`) maps a bind
      failure of a read that carried conditions to a 400
      `invalid_refinement` naming the field, or `sealed_unsupported` (its
      own class status, 422) when the binder said so. A bind failure with
      no condition to blame, and every other message, stays 500. Regression
      looks like: a 500 `internal` with `bind: [...]` for a `$refine` the
      binder cannot place. Test
      `a_refinement_that_fails_to_bind_is_refused_with_its_class_not_500`
      (`crates/ordo-host/tests/refine_door.rs`). Board: `refine-bind-500`.
- [x] **25au · `ordo check` can take about 75 s on one failing path search.
      Fixed 2026-10-09** (78947c3d). Found 2026-10-09: the `no_match_path` search
      over the estate schema in a debug build. Cause: match inference tried
      every subset of the schema's 24 ref edges (2^24) before learning that no
      subset connects. It now checks the whole edge set first and names the
      sources it cannot reach, prunes self-refs and dead-end bridges, and
      refuses a search past 2 million subsets in place of the old 24-edge cap.
      Regression looks like: a `no_match_path` check taking seconds, or a
      refusal that no longer names the source. Tests
      `a_failing_path_search_on_the_estate_schema_finishes_quickly`
      (ordo-cli), `a_let_with_no_key_back_names_the_source_it_cannot_reach`
      (ordo-sema). Board: `check-path-search-slow`.
- [x] **25at · Joining a grouped `let` back to its source does not compile.
      Fixed 2026-10-09** (78947c3d). Found 2026-10-09: `let ... group vendors.id`
      joined back gave `no_match_path` or `fan_out`. Cause: a group key that
      is an entity's `id` kept no provenance, so inference had no edge back,
      and every provenance edge counted as a many-branch. A let grouped by an
      `id` (fellow keys from the same row allowed) or by a lone ref now points
      at that entity and joins back one-to-one. `vendorsBillingLag` is
      un-marked; the other blocks named are planned for sealed aggregates, not
      this. C7's recipe compiles (its `[id]` projections became `[]`).
      Regression looks like: either refusal again, or sums multiplied by a
      fanning join. Tests `two_lets_grouped_by_the_entity_id_join_back_one_to_one`,
      `two_lets_grouped_by_the_child_ref_join_back_one_to_one`,
      `two_lets_keyed_by_more_than_the_ref_still_fan_out`,
      `two_ungrouped_lets_on_one_entity_still_fan_out` (ordo-sema);
      conformance `let_group_join.rs`:
      `two_lets_grouped_by_the_customer_id_count_each_customer_once`,
      `the_compared_lets_keep_only_the_stalled_customer`,
      `two_lets_over_different_children_sum_each_vendor_once`,
      `a_let_grouped_by_the_ref_joins_back_once_per_vendor`,
      `a_join_that_fans_out_is_still_refused`. Board: `let-group-join`.
- [x] **25as · A refinement on a sealed indexed field answers 500. Fixed
      2026-10-09** (3bfef387): an op with `take` returning a `sealed indexed
      refinable` field answered 500 to `$refine` `eq` on that field. Cause:
      a `take` settles the op, so the refinement wraps it in a sub-select,
      and the wrap dropped the field's blind index; the equality was then
      refused at bind as `sealed_unsupported`, which the door reports as
      `internal`. Fix: `wrap` (`crates/ordo-sema/src/lib.rs`) carries the
      digest column out of the sub-select as a hidden output and arms the
      blind index on it. `eq` and `in` ride the index; `ne`, `lt`, `le`,
      `gt`, `ge` and an empty `in` are `invalid_refinement` naming the
      field. Regression looks like: a 500 with `bind: [sealed_unsupported]`
      for `$refine` `eq` on `tokenTop`. Test
      `a_sealed_indexed_condition_on_a_take_that_returns_the_field_is_served`
      (`crates/ordo-host/tests/refine_door.rs`). Board: `sealed-refine-500`.
- [x] **25aq · The human check did not cover passkey sign-up. Fixed
      2026-10-09** (00a7a738): `/auth/register` calls `check_human` first, and the
      SDK's `RegisterOptions` has `human`. Regression looks like: a register
      with the check on and no token minting a ceremony. Tests
      `with_the_check_on_a_register_without_a_token_mints_nothing`,
      `with_the_check_on_a_register_with_an_accepted_token_mints_the_ceremony`,
      SDK "sends the human-check token with the first leg, register".
      Board: `human-check-register`.
- [x] **25ap · A blind-index lookup that returns a sealed field does not
      compile. Fixed 2026-10-09** (ad3aaaa9): found by the audit (finding
      1). The artifact now withholds the statement whenever the query holds
      a digest, not only when the class is `blind-index`. Regression looks
      like: `unresolved_blind_index` from `ordo check`, `plan` or `apply`
      for `customers(email == $e)[name, email] take 1`. Tests
      `a_blind_index_lookup_that_returns_the_sealed_field_compiles`
      (`crates/ordo-cli/tests/check_exit.rs`),
      `a_declared_blind_index_lookup_applies_and_returns_the_opened_value`
      (`crates/ordo-host/tests/sealed_reads.rs`). Board: `sealed-lookup-compile`.
- [x] **25ao · An edit to only a routine was silently dropped on deploy.
      Fixed 2026-10-08** (45ed6647): routines are part of the program's
      version hash. Regression looks like: a routine-only edit applied with
      no new version. Test `a_change_to_only_a_routine_changes_the_programs_version`.
      Board: `routine-edit-dropped`.
- [x] **25an · A tenant unreachable at boot replayed its whole history to
      routines. Fixed 2026-10-08** (2075b038). Also 321fa032: idle tenants
      back off to `ORDO_TRIGGERS_IDLE_MAX_SECS` (10 s), and a routine's stale
      cursor is retired once the new one catches up. Test
      `a_tenant_unreachable_at_boot_does_not_replay_its_history_when_reached`.
      Board: `routine-replay-unreachable`.
- [x] **25aj · Revoking one share wiped other live data from the device.
      Fixed 2026-10-08** (6f6d83b9): found by CI from the React gaps fix.
      The device copy is poisoned only when no synced op is still live.
      Regression looks like: a still-shared note vanishing when another
      share closes. Test `clients/ordo-react/test/device.test.ts` "a row
      another synced op still holds survives the revocation of this one".
      Board: `revoke-wipes-others`.
- [x] **25ai · Twelve refusal classes decoded as internal in both
      clients. Fixed 2026-10-06** (c82154c0): found writing the sign-in
      guide; `space_paused` among them, so a pause read as a fault. Both
      lists name all 61 classes. Test
      `crates/ordo-api/tests/client_classes.rs`
      `every_client_names_every_refusal_class_the_door_sends`. Board:
      `client-classes-missing`.
- [x] **25ae · Deleting an account left the person's delegations and
      recovery approvals. Fixed 2026-10-05** (a406073): found by the audit
      of 27aq (`docs/audit-2026-10-05d.md` finding 1). The removal deleted
      personal tables by a `principal` column and skipped tables without
      one; `$delegation` names a person as delegator or agent and
      `$recoveryapproval` as approver. Each table now lists every column
      that names a person, and a listed column a table lacks is an error.
      The never-signed-in branch of a leave now uses the same path.
      Regression looks like: such a row naming the deleted id after the
      route answers 200. Test `crates/ordo-host/tests/member_release.rs`:
      `deleting_an_account_removes_its_delegations_and_recovery_approvals`.
      Board: `delete-leaves-delegations`.
- [x] **25ab · The production server never recorded a key release. Fixed
      2026-10-05** (commit 12aef13): the runtime wraps its provider once in
      `MeteredKeys` with the security recorder (`meter_custody` in
      `crates/ordo-host/src/runtime.rs`), before any holder takes a copy, so
      the executor, workers, blob, share links, second factor and `ordo dev`
      all count on `ordo_key_release_total` and record `KeyReleased` in the
      tenant's stream. Regression looks like: the count flat while sealed
      reads are served, or doubled. Tests
      `crates/ordo-host/tests/key_release_metered.rs`:
      `a_served_sealed_write_and_read_are_counted_and_recorded`,
      `the_reseal_worker_releases_through_the_metered_custody`,
      `every_holder_shares_one_custody_wrapped_once`. Found
      2026-10-05 by the sealed-reads agent, confirmed by the lead: the
      runtime serves with the bare provider from `build_custody`
      (`crates/ordo-host/src/runtime.rs` around 2412 and 3695); only tests
      construct `MeteredKeys`, the decorator that counts releases and writes
      `KeyReleased` to the security stream. So a deployed server meters no
      release and records none, and release-breadth alerting has nothing to
      read. Owed: wrap the provider in `MeteredKeys` with the recorder, in
      the binary. Board: `key-release-unrecorded`.
- [x] **25ac · An upload during a bucket detach or replace can be stranded.**
      Found 2026-10-05 by the Fable audit of 27ap. `objects_held` is counted
      in the tenant's database and the attachment row is deleted in the
      deployment's, with nothing held between, so an upload committing in
      between lands in the old bucket; and other nodes keep routing uploads
      there for up to 30 s (`AttachedStores::TTL`). The object is then
      unreadable and the sweep cannot reach it. The same window exists for
      `replace`. Ruled 2026-10-05: no check at upload; each stored file
      records its bucket. **Fixed 2026-10-05** (0a36ef7): `$blobref` and
      `$blobtomb` carry `"store"`; reads, the sweep, erasure, export, the
      revert's digest check and the re-seal pass follow it. Regression looks
      like: a file uploaded during a detach answers `not_found` or
      `internal`, or the sweep retires its reference while the bytes stay
      in the old bucket. Tests `crates/ordo-host/tests/blob_door.rs`:
      `an_upload_that_lands_during_a_detach_stays_readable`,
      `the_sweep_collects_an_upload_from_the_bucket_it_landed_in`,
      `an_interrupted_upload_during_a_detach_is_collected_from_the_bucket_it_landed_in`,
      `a_replace_moves_new_uploads_and_the_file_written_during_it_stays_where_it_landed`.
      `sealed_unsupported` now answers 422 (f73052a). Board:
      `detach-upload-race`.

