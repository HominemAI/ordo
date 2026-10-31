# Plan: durable idempotency's last window (8q)

**The instruction this plan answers:** *"Plan the changes and, before
implementing, audit to make sure it will not interfere with any other
system adversely."* This is that plan and that audit. Nothing here is
implemented beyond what already exists; the audit came first, as asked —
and it changed the recommendation.

## 0 · The finding that reframes the task

The decisions file described the defect as: *"`put` runs after the executor
returns, so a crash between the write committing and the response being
stored lets a later retry **re-execute a committed write**."*

**That is no longer the state of the code.** Reading the write path end to
end (`ordo-exec/src/lib.rs`, `write_retrying`; `ordo-api/src/router.rs`;
`ordo-host/src/replay.rs`) finds AA-004's third leg already built:

- A **`$replay` journal rides the write's own transaction.** The executor
  claims the key *inside* the transaction that will write the effect
  (`TxConn::claim_lease`), with a **fencing generation** (AA-005), and
  marks it done in the same transaction (`complete_lease`) — *"commit
  publishes both or neither. A crash before commit loses both; a crash
  after loses neither."*
- A retry after the crash re-claims at the door, hands its lease to the
  executor, and the executor's in-transaction read answers
  `LeaseState::Applied` → the transaction rolls back untouched and the
  router **answers instead of executing** (`ExecOutcome::AlreadyApplied`).
- A fenced holder — a stalled node waking up after its lease was taken
  over — **cannot commit**: `complete_lease` is conditional on the
  generation, and failure rolls the whole effect back.
- Key misuse and fencing surface as real wire classes
  (`idempotency_mismatch`, `lease_fenced`), through the same
  class-prefix seam `classify` reads once — not as `internal`.

So **the re-execution hazard — the correctness hazard — is closed.** What
survives the crash window is strictly smaller: the **verbatim response
body** is stored by `put` after commit, outside the transaction. A crash in
that window loses the body; the retry gets an honest, named refusal —
*"this key's effect already committed; its response was lost with the
process that ran it, so it cannot be replayed verbatim."* Nothing
re-executes. Nothing lies.

The question this plan actually has to answer is therefore not *"how do we
close the re-execution hole"* — it is closed — but *"should the sealed
response move inside the write transaction too?"*

## 1 · The audit, subsystem by subsystem

What was checked, and the verdict for folding the response store into the
write transaction. An audit that concludes "no interference" without naming
what it checked is worth nothing, so: each row names the code read.

| Subsystem | What was read | Verdict on in-tx response storage |
|---|---|---|
| **Command log + append lock** | `commandlog.rs` (`append_group`, `pg_advisory_xact_lock` §"the lock is transaction scoped"), `write_retrying`'s ordering | **UNSAFE as a fold-in.** The advisory lock is held from acquisition to commit; every statement between is critical section. `claim_lease` runs *before* the body (outside the lock's reach) and costs nothing under it; `complete_lease` adds one PK UPDATE inside it — small, bounded, and now **measurable** by the `ordo_chain_append_seconds` / lock-wait histogram pair that just landed. Response storage would add a **seal plus an INSERT of a sealed body** inside that same section, and the body is unbounded (a `whole` read's response is the object graph). The critical section's width is the single-space write throughput ceiling; `implementation.md` names "anything that widens the critical section" as the thing to catch. |
| **Custody / `KeyProvider`** | `replay.rs` §"Responses are sealed at rest", `seal_body` (release per put) | **UNSAFE as a fold-in, and this is the decisive row.** Sealing takes a `KeyProvider` release. Today that happens after commit, off every lock. Inside the transaction it would couple the provider's latency — and its outages — to the global append lock: a slow or unreachable KMS becomes **every space's write stall**, transitively. The security-event stream made exactly the opposite choice for exactly this reason (nothing on the request path touches its store), and that reasoning was accepted. |
| **`SpaceAudit`** | The statement wrapper; `$replay` DDL and its statements in `TxConn` | **Safe / consistent.** The journal's statements are platform bookkeeping on a platform table, the same standing as the trigger queue's enqueue — which also rides the write transaction. No change either way. |
| **Trigger accumulation** | `write_retrying`: `tx.enqueue("invalidate", …)` inside the tx; `ordo-exec::triggers` | **Safe.** Triggers key on commit; they neither read nor race the replay stores. An in-tx response INSERT would not change when they fire. |
| **Invalidation bus** | `tx.notify("ordo.invalidate", …)` in the same tx; listener refetch discipline | **Safe.** A listener that observes the commit before the response store holds the body refetches through its own guarded read; the response store is consulted only by same-key retries, which the journal answers honestly. No listener assumes the response exists. |
| **Security event stream** | `secdrain` design (deliberately off the write tx) | **Safe / unchanged.** It is not on the transaction and this plan does not put it there. Named because proposing the opposite for the cache invites the question of why the two differ: they don't — this plan keeps the response store off the transaction too. |
| **Wire inventory** | `error.rs` (`idempotency_mismatch`, `lease_fenced`, `idempotency_in_flight`), `inventory_is_closed.rs` | **One honest wrinkle, below.** No new class is required by the recommendation; one is *recommended* for a different reason. |
| **The `#[must_use]` history** | `idempotency.rs` trait docs (`put` dropped silently when the trait went async; `abandon` caught by the lint) | **Held.** The recommendation changes no signatures, so the class of mistake cannot recur here. Any future signature change on this trait must keep the `#[must_use]` and its message. |

## 2 · The recommendation

**Keep response storage outside the transaction. Close nothing, because
the thing worth closing is already closed.** Concretely:

1. **Do not** move `put` (or the sealing it requires) into the write
   transaction. The audit's two UNSAFE rows are the argument: it would put
   an unbounded INSERT and a key-provider round trip inside the global
   append critical section, converting a custody outage into a total write
   outage — to recover a *convenience* (verbatim replay) in a window that
   already answers honestly.
2. **Measure instead of assume.** The journal already costs one UPDATE
   inside the critical section. The histogram pair that landed today can
   put a number on it; the number belongs beside the release measurements
   (5f), not in this plan as an adjective.
3. **One wire honesty improvement, recommended but severable:** the
   "effect committed, response lost" answer currently reuses
   `idempotency_in_flight` (with a message that says the real thing). The
   class name suggests *retry later*; the truth is *this will never
   change*. A caller polling on the class name polls forever. Minting
   `idempotency_applied` (409, terminal: the effect is committed, the body
   is gone, stop retrying) states the truth in the field machines read.
   This is a deliberate widening of a closed inventory, so it goes through
   the decisions file, not through this plan unilaterally.

### What was ruled out, and why

- **Two-phase (reserve → execute → commit both):** the reserve already
  exists (door claim + in-tx journal claim); a third phase would only be
  buying verbatim-replay durability, at the cost above. Rejected.
- **Sealing before the transaction opens:** the response body does not
  exist until the effect runs; there is nothing to seal early. Rejected on
  mechanics.
- **Storing the response unsealed inside the tx, sealing later:** a
  plaintext mirror of every write response in a table, even briefly, is
  the exact thing `replay.rs` §"Responses are sealed at rest" exists to
  prevent. Rejected.

## 3 · Build order (if the recommendation is accepted)

Small, because the recommendation is mostly *stop and document*:

1. **Docs truth pass** — `replay.rs`'s "What this does NOT do" block and
   the checklist's 8q entry still describe the pre-journal state in
   places; align them with §0's findings. Done-when: a reader of either
   cannot conclude a retry can re-execute a committed write.
2. **The measurement** — extend `lock_wait.rs`'s harness to report the
   journal UPDATE's share of the critical section. Done-when: the number
   prints beside the append/wait pair.
3. **(If approved via decisions)** mint `idempotency_applied` with its
   producer at the router's `AlreadyApplied` arm, inventory row, and a
   test asserting a crash-window retry gets it and a verbatim replay does
   not. Done-when: `inventory_is_closed.rs` passes with the new row and
   the class has exactly one producer.

## 4 · What this plan does not cover

- **`KeyBinding::Permanent` is still not a column on the created row** —
  the create-once guarantee lasts as long as the cache entry, not the row.
  Real, pre-existing, and a different item than the crash window.
- **Multi-node takeover policy** (when a reservation may be seized) is the
  fencing generation's job and is built; *tuning* the reservation timeout
  is an operations question for the deployment guide.
