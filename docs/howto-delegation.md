# How a person lets an agent act for them

For app developers. Built in checklist 11-4b, increments 3 and 4 of
`plan-delegation-surface.md`. What a delegated agent may then do is in
`howto-workers.md`.

## The ceremony in five steps

1. **The agent asks.** It names the tenant, itself, the person and the ops.
2. **The person reads** what the grant allows, in plain words.
3. **The person approves**, with a fresh passkey touch by default.
4. Or **declines**.
5. **The agent redeems** its request and gets its token.

The agent never sees the person's session. The person never sees the
agent's token.

## Before you start

Name the agents your deployment accepts, with the name people see:

```
ORDO_DELEGATION_AGENTS=agt_assistant=Expense assistant,agt_mail=Mail sorter
```

Unset, no agent may ask. The name on the review is yours, never the agent's.

## 1 · The agent asks

The agent holds no token yet, so this route asks for none.

```
POST /auth/delegation/request
{"space": "<tenant>", "agent": "agt_assistant", "person": "<member id>",
 "ops": ["pendingExpenses", "approveSmallExpense"], "hours": 2}
```

It answers `201`:

```
{"request": "dr_…", "binder": "<64 hex>", "expiresAt": 1791633600,
 "review": "std.delegation.review"}
```

- **Keep the binder secret.** Only its hash is stored. It never goes in a URL.
- Send the person the request id. Your app shows them the review.
- `hours` defaults to 1.
- With `ORDO_AUTH_HUMAN_CHECK` on, the body also carries `"human": "<token>"`,
  as a sign-in does. The person's page solves the check and hands the token
  to the agent. A missing or refused token answers `401
  authentication_failed`, and no request is stored.

### What an open route costs

The route takes no token, so anyone can call it in a listed agent's name.
Each call stores a row for `ORDO_DELEGATION_REQUEST_SECS` plus a day, writes
one event to the deployment's security stream, and shows in the named
person's `mine` list. These bound it:

- `ORDO_DELEGATION_OPEN_PER_AGENT` (1000): requests one agent may have
  waiting, across every tenant.
- `ORDO_DELEGATION_OPEN_PER_PERSON` (10): requests one person may have
  waiting, from every agent. It counts the person id as named, whether or not
  that member exists, so it tells a caller nothing about who is a member.
- The human check, when it is on.

Over a cap, the route answers `429 delegation_limit`. The message names the
setting and the value that would admit the request. Agent credentials of
their own (11-4c) will close the route to strangers.

In the SDK: `requestDelegation`. The example in `examples/assistant` runs
the whole ceremony from a script.

## 2 · The person reads

```
GET /op/std.delegation.review?request=dr_…
```

Only the person the request names gets an answer. Anyone else gets
`not_found`.

The disclosure is computed from your program. Nothing the agent sent is shown
as text.

| Part | What it says | Where it comes from |
|---|---|---|
| `agent.name` | Who is asking | `ORDO_DELEGATION_AGENTS` |
| `tenant` | Which tenant | the request |
| `term` | How long, and the end as a date | the request, capped by `ORDO_DELEGATION_MAX_HOURS` |
| `ops[].reads`, `writes` | Each field each op reads or changes | the op's field-scope manifest |
| `ops[].examines` | Fields a row guard looks at for the op | the same manifest |
| `ops[].deletes` | Entities it deletes rows in | the op's statements |
| `ops[].words` | All of that as sentences | the same |
| `ops[].yours` | False when the person cannot call it either | the person's role |
| `notGranted` | Ops asked for that will not be granted, and why | the deployment's rules |
| `unknownNames` | How many names were not ops at all | counted, never shown |
| `cannot` | What the agent can never do | fixed |
| `ceiling` | It never exceeds the person's own reach | fixed |
| `revoke` | How to end it | fixed |
| `ownerSees` | That the tenant's owner can see it, when the person is not the owner | the person's role |

An op is not granted when it is:

- not in your program,
- a platform op,
- a delete you have not opened with `ORDO_DELEGABLE_OPS`,
- or one that needs a fresh passkey touch.

In the SDK: `reviewDelegation`, `approveDelegation`, `declineDelegation`,
`myDelegations` and `revokeDelegation`. In React, the five ops are in
`PLATFORM_OPS`, so `useOpQuery` and `useCommit` reach them.

## 3 · The person approves

```
POST /op/std.delegation.approve   {"request": "dr_…"}
```

By default this asks for a **fresh passkey touch**, like every platform op.
Call `/auth/step-up` first. A two-step claim your deployment accepts counts
too. To drop the touch, name the op in `ORDO_STEP_UP_EXEMPT_OPS`.

An agent can never approve. A delegated token is refused on every
`std.delegation.*` op, and no agent token carries presence.

The answer names the delegation, the ops granted and the end:

```
{"delegation": "dlg_…", "ops": ["approveSmallExpense", "pendingExpenses"],
 "expiresAt": 1791640800, "replaced": [], "outcome": "approved"}
```

A second approval for the same agent replaces the first. The first is
revoked.

## 4 · Or declines

```
POST /op/std.delegation.decline   {"request": "dr_…"}
```

No delegation is made. The agent is told `delegation_declined`.

## 5 · The agent redeems

```
POST /auth/delegation/redeem   {"request": "dr_…", "binder": "<64 hex>"}
```

| Answer | Means |
|---|---|
| `200 {"token", "expiresAt", "delegation", "space", "ops"}` | Approved. Use the token. |
| `202 {"status": "waiting"}` | Not answered yet. Ask again in a few seconds. |
| `403 delegation_declined` | The person said no. |
| `410 delegation_expired` | The request lapsed, or was revoked first. Ask again. |
| `410 delegation_redeemed` | Already redeemed. A request gives one token. |
| `404 not_found` | No request with that id and that binder. |

A request id seen on a screen is worth nothing without the binder.

The token lasts until the delegation ends. In the SDK: `waitForDelegation`
polls this route until it is approved or refused.

## Listing and revoking

```
GET  /op/std.delegation.mine
POST /op/std.delegation.revoke   {"delegation": "dlg_…"}
```

`mine` lists the person's requests and delegations in this tenant. Each row
has a `state`: `waiting`, `live`, `revoked`, `expired`, `declined` or
`failed`.

Only the person who granted a delegation can revoke it. The agent's next call
is refused with `delegation_required`.

A feed the agent holds open (`?live=1`, a sync, or a read on `/live`) ends
too. It asks the door again before every pass, so the next write sends an
`error` frame naming `delegation_required` and no rows. With no write, it
ends within `ORDO_LIVE_RECHECK_SECS` (30 s by default).

Neither asks for a passkey touch.

## Settings

| Setting | Default | What it does |
|---|---|---|
| `ORDO_DELEGATION_AGENTS` | unset | The agents that may ask, as `id=name`. Unset accepts none. |
| `ORDO_DELEGATION_MAX_HOURS` | `8` | The longest delegation, 1 to 720. |
| `ORDO_DELEGATION_REQUEST_SECS` | `600` | How long a request waits for the person, then for the agent to redeem it. 60 to 3600. |
| `ORDO_DELEGATION_MAX_OPS` | `10` | The most ops one request may name. 1 to 100. |
| `ORDO_DELEGATION_OPEN_PER_AGENT` | `1000` | Requests one agent may have waiting at once. 1 to 1000000. |
| `ORDO_DELEGATION_OPEN_PER_PERSON` | `10` | Requests one person may have waiting at once. 1 to 1000. |
| `ORDO_STEP_UP_EXEMPT_OPS` | unset | Name `std.delegation.approve` here to approve without a passkey touch. |
| `ORDO_DELEGABLE_OPS` | unset | Deletes a delegation may include. |

A request a setting refuses answers `403 delegation_refused`. The message
names the setting, its value, and the value to set, for example "the request
asks for 24 hours, over ORDO_DELEGATION_MAX_HOURS=8; ask for 8 or fewer, or
set ORDO_DELEGATION_MAX_HOURS=24". An agent the deployment does not list is
told the `id=name` entry to add to `ORDO_DELEGATION_AGENTS`. An op that deletes
and is not open to agents names the `ORDO_DELEGABLE_OPS` entry to add. A bad
value at boot names the range and the default.

## What is recorded

Each step is a security event, under `delegation.*`:

- `requested`, in the deployment's stream (the agent proved nothing)
- `approved`, `declined` and `revoked`, in the tenant's stream
- `redeemed`
- `refused`, with the reason: `not-the-person`, `request-expired`,
  `unknown-or-wrong-binder`, `replayed`, `delegation-ended` and others

The approval's correlation id is stored on the delegation as its
`consent_correlation`.

## Not built yet

- The MCP binding (11-4c).
- Credentials of the agent's own. Until then, any caller may ask in a listed
  agent's name, within the caps above. The person still decides, and only the
  asker holds the binder.
- Rate and volume budgets per delegation (SH-20).
