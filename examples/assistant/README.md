# Assistant

A manager approves expenses. An assistant approves the small ones in the
manager's name. The manager can take that back at any moment.

The assistant is a small Node script. It stands in for an AI agent, a
background job, or a human assistant with a login of their own. The door
treats all of them the same.

| Part | Where |
|---|---|
| The program, the whole server side | [`assistant.ordo`](assistant.ordo) |
| The local deployment | [`dev/serve.sh`](dev/serve.sh) |
| Made-up people and expenses | [`dev/seed.mjs`](dev/seed.mjs) |
| The manager's side of the consent ceremony | [`manager/approve.mjs`](manager/approve.mjs) |
| The assistant | [`agent/assistant.mjs`](agent/assistant.mjs) |
| The proofs | `crates/ordo-host/tests/assistant_app.rs` |

## 1 · What delegation is

Ordo checks every call against who is asking. Normally that is the person
whose token is on the call.

A **delegation grant** lets a person lend part of their rights to someone
else. That someone is the **agent**. It can be a program, a worker, or another
person.

- The assistant asks. The manager reads what it would be allowed to do, and says yes once, with an expiry.
- The agent gets its own token. The token lists the ops it may call.
- The token also names the delegation. The door checks on every call that the
  delegation is still live.

Two sets must agree for a call to pass.

- **The grant's list.** The ops the manager named.
- **The manager's own rights.** The agent never gets more than the manager has.

## 2 · What the assistant can and cannot do

| It tries | Result | Why |
|---|---|---|
| List the manager's pending expenses | Allowed | `pendingExpenses` is in the grant |
| Approve a 42.50 expense | Allowed | `approveSmallExpense` is in the grant |
| Approve a 250.00 expense | Changes nothing | The op's body only matches amounts up to 100.00 |
| Approve any amount with `approveExpense` | Refused, `access_denied` | The op is not in the grant |
| Reject an expense | Refused, `access_denied` | `rejectExpense` is not in the grant |
| Delete an expense | Refused, `access_denied` | Not in the grant. Deletes are also closed to every delegated token by default |
| Change a role | Refused, `access_denied` | Platform writes are closed to every delegated token, whatever the grant lists |
| See another manager's expenses | Sees none | The row guard answers for the manager, and these are not hers |
| Anything, after a revoke | Refused, `delegation_required` | The next call, before it reaches the program |

### Why the limit is a separate op

The program cannot read `$agent`. The agent's name goes to the command log and
nowhere else. So a rule such as "100.00 for the assistant, any amount for the
manager" cannot be written in one op.

There are two ops instead. `approveExpense` has no limit and is for the
manager. `approveSmallExpense` carries the limit in its body, and only it is in
the grant. The limit is a fact about the op. It holds for any caller, the
manager included.

### Settings

`ORDO_DELEGABLE_OPS` lists delete ops that a delegated token may call. The
assistant deletes nothing. `serve.sh` sets it to the empty list, so every
delete stays closed. Approving is not a delete and needs no entry.

## 3 · What the history shows

Two places record a decision.

**The row.** `history` returns each decided expense with who decided it and
through which op. Under a delegation `$caller` is the manager, so the row names
the manager.

```
Taxi from the airport  38.69  approved  decidedBy: Maya Chen  decidedVia: approveSmallExpense
```

**The command log.** It is tamper-evident. Each write is recorded with its
actor. A write the assistant made reads:

```
agent:agt_assistant on behalf of user:<the manager>
```

A write the manager made by hand names the manager alone. The test
`the_assistant_approves_a_small_expense_and_the_record_names_both` reads both
back.

## 4 · How the grant is made and revoked

Through the consent ceremony (checklist 11-4b). `docs/howto-delegation.md`
describes it in full.

1. The assistant asks the door for exactly two ops, `pendingExpenses` and
   `approveSmallExpense`, for 8 hours. It gets a request id and a binder.
2. It prints the request id and waits. Only it holds the binder.
3. The manager runs `manager/approve.mjs`. It shows what each op reads and
   changes, in words computed from the program, and asks to approve.
4. The assistant collects its token with the binder and starts work.

`serve.sh` names the assistant in `ORDO_DELEGATION_AGENTS`, with the name the
manager sees. It also exempts `std.delegation.approve` from the passkey touch,
because the manager is a guest with no passkey. A real deployment keeps the
touch.

To revoke, run `node examples/assistant/manager/approve.mjs --revoke`. The
same token, unexpired and unchanged, is refused on its next call.
`--list` shows the manager's delegations and their state.

## 5 · Run it

You need Postgres 15 or later, Node 18 or later, and a Rust toolchain.

```
cargo build -p ordo-cli -p ordo-host
(cd clients/ordo-sdk && npm ci && npm run build)
(cd examples/assistant/agent && npm install)
(cd examples/assistant/manager && npm install)
```

Then, in one terminal:

```
sh examples/assistant/dev/serve.sh
```

In another:

```
node examples/assistant/dev/seed.mjs
node examples/assistant/agent/assistant.mjs
```

The assistant asks and waits. In a third terminal, as the manager:

```
node examples/assistant/manager/approve.mjs
```

The seed makes one manager, 3 staff and 32 expenses. About 7 in 10 are 100.00
or less. The manager decides 5 by hand. The data is synthetic and the same
`--seed` gives the same data.

The assistant prints what it approved, what it left for the manager, and what
the door refused. Run `node examples/assistant/manager/approve.mjs --revoke` and
run it again to see `delegation_required`. `--ask` asks again.

The throwaway Postgres listens on port 55498 and `serve.sh` keeps its data under
`dev/.data`. Delete that directory to start over.

To prove it without a browser or a server:

```
cargo test -p ordo-host --test assistant_app
```

It needs the conformance cluster (`scripts/conformance-pg.sh start`).

## 6 · What Ordo still owes this app

- **Worker credentials of their own.** The assistant has none, so any caller
  may ask in its name. The manager still decides, and only the asker holds the
  binder.
- **Reading the command log through the door.** `history` shows the manager
  from the row. The agent's name is in the command log, which the app cannot
  query. The test reads it from the store.
