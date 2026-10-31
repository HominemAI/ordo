# How a tenant is paused, and what still answers

For app developers. Built in checklist 27ar. The design is
`plan-tenant-pause.md`; the rulings are in `corpus-rulings.md`, 2026-10-05.

## What a pause is

A paused tenant is held still. Its data is kept. Its ops are refused.

A resume puts it back. Nothing is lost in between.

Deleting a paused tenant works the same as deleting an active one.

## Who pauses

By default, only the deployment owner. Pausing is a deployment operation.

The deployment owner calls these from the deployment's own space:

```
std.deployment.pauseTenant(tenant: "spc_...", note: "billing")
std.deployment.resumeTenant(tenant: "spc_...")
```

The deployment owner's resume lifts any pause.

## Letting a tenant pause itself

That is your choice. Name the tenant roles that may:

```
ORDO_TENANT_PAUSE_ROLES=owner
ORDO_TENANT_RESUME_ROLES=owner
```

Unset means nobody inside a tenant. The resume roles follow the pause roles
unless you set them.

A granted role calls these in its own tenant:

```
std.space.pause(note: "owner")
std.space.resume
```

A tenant's resume lifts only a pause the tenant made. If the deployment owner
paused it, the tenant is told `paused_by_deployment`.

Both ops ask for a fresh passkey, like every platform op, unless you exempt
them with `ORDO_STEP_UP_EXEMPT_OPS`.

## The reason

Every pause carries one word: `owner`, `billing`, `abuse`, `legal`,
`maintenance` or `other`.

Free text is refused. Nothing personal goes into the record.

A message for the tenant's people is yours to build. Keep it in your own
entities.

## What a paused tenant answers

Every op is refused with `space_paused`, status 423.

A 423 means: do not retry. Somebody has to resume the tenant.

The class is always `space_paused`. Clients can rely on it.

### Setting the status and the message, per reason

The status and the message are yours to set, per reason word. Set
`ORDO_TENANT_PAUSE_ANSWERS` to a JSON object:

```
ORDO_TENANT_PAUSE_ANSWERS='{
  "billing": { "status": 402,
               "message": "This workspace is paused. The plan is unpaid." },
  "abuse":   { "status": 403,
               "message": "This workspace is paused after a security review." }
}'
```

- `status` is optional. Allowed: any 4xx, and 503. Default 423.
- `message` is optional. Default is the message above.
- A reason you leave out answers as today.
- The message is fixed text. Never put a name or anything personal in it.
- The class stays `space_paused`, and `GET /auth/spaces` still carries the
  reason.
- A 503 is still never cached for replay. After a resume, the same call runs.

A reason word that is not on the list refuses the boot. So does a status
outside 4xx and 503, an empty message, and any other key.

A caller who is not a member gets `access_denied` instead. They learn nothing
about the pause.

## Keeping an op open: `nopause`

Mark the ops a paused tenant still needs:

```
op openInvoices nopause =
  invoices(status == "open")[number, total]
  take 50

op markPaid($number: text) nopause =
  set invoices(number == $number) { status: "paid" }
```

The word sits beside `unguarded` and `unbounded`.

A `nopause` write still changes rows. Mark only what a paused tenant truly
needs, such as paying a bill.

`ordo check` shows the mark:

```
  openInvoices  (nopause: answers while the tenant is paused)
```

Derived `std.*` ops never carry it. They are refused while paused.

## Platform ops on a paused tenant

These still answer:

- `std.space.resume`, `std.space.export`, `std.space.delete`
- `std.space.chainProof`, `std.space.callersOf`
- the attestation read-backs
- `std.space.setRole`, `std.space.suspend`, `std.space.reinstate`,
  `std.space.remove`
- the share, link and invitation lists, and their revokes
- `std.space.admission`
- `std.person.tenants`

These are refused:

- admitting a member, sharing, inviting, minting a link
- accepting a share or arriving with an invitation. The ticket is not spent,
  so it still works after a resume
- attaching or detaching a bucket, changing attestation
- `std.space.leave` and `std.space.import`
- the blob door, for reads and uploads

The rule: people can look, get out, and take access away. Nothing new
happens inside.

## Where people see it

Sign-in is deployment-wide. A paused tenant does not stop it.

`GET /auth/spaces` and `std.person.tenants` list each tenant with a `state`.
A paused one also carries `pausedAt`, `pausedBy` and `note`:

```
{ "space": "spc_...", "state": "paused", "pausedAt": 1790000000,
  "pausedBy": "deployment", "note": "billing", ... }
```

`pausedBy` is `tenant` or `deployment`. Never a name.

Build your "this workspace is paused" screen from that. Put billing or contact
pages in the deployment's own space, where they keep answering.

## What runs during a pause

Records keep being secured: checkpoints, attestation, the security stream.

A paused tenant's routine runs are skipped. Each skip is finished, not retried, and recorded in the control database's `$routineskips` table with the reason `paused`, and in the log.

A routine whose `run` header says `nopause` runs in a paused tenant anyway. The default is not to run. See `howto-routines.md`.

## Many replicas

A pause holds on the replica that took it at once. Others hear it within one
announcement, and within `ORDO_ACCESS_REFRESH_SECS` at most (default 30).

## A person's own tenant

A person's own tenant stays theirs while paused. Signing in again gives them
the same tenant, never a second one.
