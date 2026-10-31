# How a worker acts for a person

For app developers. Built in checklist 11-4, increment 2 of
`plan-delegation-surface.md`; the design is its §7.

## What a worker is

A worker is a program that calls your ops for somebody: a background job, a
sync, an AI agent.

It holds its own credential. That credential carries a **delegation**: a
person's consent, recorded, expiring and revocable.

## What it can do

A worker acting for Ana is Ana to your guards.

- It sees exactly the rows Ana sees. Not one more.
- Its writes get Ana's answers. Where Ana may only read, it may only read.
- It calls only the ops its credential lists, and only those Ana may call.
  The two lists intersect. They never add.

Without a live delegation the worker is only itself. With no seat in the
space, that is no rows at all.

## What it cannot do

- **Delete**, unless you open that op by name (below). Every op that deletes
  a row counts, including one that also sets rows.
- **Platform writes**: `std.space.*`, `std.person.*` and the rest. These stay
  closed whatever you set.
- **Anything needing a fresh passkey.** A worker never carries one.
- **Name somebody else.** `$caller` and `$agent` are bound by the door. A
  request that sends either is refused.

## Opening a delete

Deletes are closed to workers by default. Open one by naming it:

```
ORDO_DELEGABLE_OPS=purgeDone,std.tasks.delete
```

- Exact op names only. A pattern refuses the boot.
- A platform op refuses the boot.
- A name your program does not have does nothing.

A closed op answers `access_denied`. The security record says
`OpNotDelegable`.

## How it is recorded

Every write a worker makes is in the space's command log as:

```
agent:<worker> on behalf of user:<person>
```

Both are named. The chain hashes that line, so it cannot be edited later.

## Stopping a worker

Revoke the delegation. The worker's **next** call is refused with
`delegation_required`, before it reaches anything:

- every op
- the catalog and the interface descriptor, which then list nothing
- `/metrics`, `/security/events` and the other deployment surfaces

The credential itself does not need to expire first.

## Replays and pages

A worker gets its own `Idempotency-Key` namespace and its own page cursors.
They are keyed on the person **and** the worker.

So a worker cannot replay a response the person stored. Two workers for the
same person cannot replay each other's.

## Making a worker credential

A person grants it through the consent ceremony (11-4b). The agent asks, the
person reads what it allows and approves, and the agent redeems its request
for the token. `howto-delegation.md` walks through it.

Your server can still make one directly, in three steps:

1. Record the delegation: `PgDelegationRegistry::record` with a
   `NewDelegation` (person, worker, space, expiry, the consent's record).
2. Record the worker's grant over its ops: `PgGrantRegistry::record`.
3. Mint the token with the deployment's `ORDO_TOKEN_KEY`. Copy the claim from
   the record with `DelegationRecord::claim()`; never build one by hand.

Keep the token's expiry no later than the delegation's. Prefer the ceremony:
it is the only path where the person sees what they agree to.

## Not built yet

- Rate and volume budgets per delegation (SH-20).
- A worker acting for a worker.
- One delegation across two spaces.
