# Audit: the wiring behind tenant and person settings

Written 2026-09-29 against the code at commit `adaf25d`. Checklist 24d, board
task `settings-layers`. The owner asked for "an audit on the table to see what
wiring is missing for tenant exposed settings". The table is `settings.md`.

Paths drop `crates/` and `/src`: `host/` is `crates/ordo-host/src/`, and
likewise `exec/`, `api/`, `identity/`, `blob/`, `secevent/`, `egress/`,
`syntax/`.

## What the audit found

- 18 settings marked exposable or could-be-exposed for a tenant or person
  were checked, plus 3 that serve as references or were added (share link
  default lifetime, admission default role, lifetime of one share).
- None of the 18 can be changed by a tenant or person today.
- None has an exposure gate. No app developer can say "tenants may set this".
- Only 1 of the 18 has a store (grant and delegation expiry), and no
  production op writes it.
- Two settings are fully wired for one layer and show the pattern:
  admission default role (tenant owner, E4) and the lifetime of one share
  (the sharer, G2). Neither has an exposure gate.
- There is no general store, write op, exposure declaration or resolver.
  Each finished setting built its own.
- Three constants in `settings.md` behave differently from its rows: the
  blob inline cap and the share link default are never read, and no op
  issues an invitation ticket. See "Rows that proved wrong".

## Shared machinery (24a)

A general mechanism for the tenant and person layers needs four pieces.

| Piece | What it must do | What exists today |
|---|---|---|
| One store | Hold a value per tenant (and per person) by setting name, with who set it and when | Nothing general. `$admissionpolicy` is one row per tenant with fixed columns (`host/admission.rs:225`, in the control database). `$deploymentsetting` is a key and value table with no tenant column (`host/admission.rs:247`) and only the claim code uses it. `$attestsinks` holds one row per tenant for one purpose (`exec/webhook.rs:100`). Nothing holds a person's setting: `$principal` has identity columns only (`host/principal_directory.rs:63`) |
| One write op | A tenant owner (or a person for themselves) sets a value, checked against the layer above, with presence | `std.space.setAdmission` is the pattern: owner only (`host/platform_ops.rs:1210`), role gate and presence from `host/lib.rs:2023,2034`, checked against a ceiling (`host/admission.rs:571`), recorded (`host/admission.rs:580`). `std.deployment.setClaimCode` is the deployment's own (`host/admission.rs:852`). The only person-level op is a read, `std.person.tenants` (`host/platform_ops.rs:176`) |
| One exposure declaration | The app developer says which settings a tenant or person may change, and within what range | Nothing. `policy` is lexed and not parsed (`syntax/trigger.rs:518-528`, `exec/cadence.rs:69-75`). `LogPolicy` and `BlobPolicy` are Rust structs with a `DEFAULT` and no program spelling. The nearest thing is the deployment's ceiling on admission ways (`host/admission.rs:83`), which is the deployment owner gating tenants, and no app developer layer sits between |
| One resolver | Given a setting and a tenant (and person), return the effective value: deployment default, then app developer, then tenant, then person, each checked against the one above | Nothing. Three separate bound checks exist: `Policy::within(&Ceiling)` (`host/admission.rs:170`), the step-up `min` of op and deployment (`identity/access.rs:842`) and the share ceiling (`host/share_ops.rs:334`). Each is written for its own setting |

Three further seams a general mechanism would use:

- **Recording.** `enrolment::record` writes one act to the deployment stream and
  the tenant's own stream (`host/enrolment.rs:34`). The tenant reads its
  stream at `/security/events` (`host/serve.rs:939`). A settings change can
  reuse both.
- **Propagation to other replicas.** Membership changes announce a rebuild
  (`host/member_ops.rs:544`, `host/deploy.rs` `announce_members`). Admission
  reads its row on each sign-in, so it needs no cache. Values read on every
  request (session life, step-up window, blob cap) need a cache and a signal
  like this one, and no settings signal exists.
- **Where the value lives.** E4 keeps its row in the control database, keyed
  by tenant. A person's setting spans tenants (one person, many tenants,
  ruling P6), so it belongs in the control database too, keyed by principal.

Two structural facts limit what a tenant value can mean:

- A session is minted for a person at deployment sign-in and names no tenant
  (`host/admission.rs` module note). A tenant's session limit has to be
  enforced at the door, against the session's start time, when a call names
  that tenant.
- Credentials belong to the deployment (ruling P1). A password is one secret
  across every tenant of a person. A tenant password rule can only govern
  passwords that tenant types for a person it created.

## The table

Cells are "yes (file:line)", "no" or "n/a". A "no" in Read at use says what
is read instead. Rows 1 to 18 are the checked settings. Rows 19 and 20 are
references that show a wired path.

| # | Setting | Tenant or person | Store | Write | Exposure | Read at use | Bounds | Visible | Recorded | Missing, in one line |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | `ORDO_SESSION_SECS` | tenant | no | no | no | no (deployment: `host/runtime.rs:401`, `identity/session.rs:435`) | no | no | no | All seven. Enforce at the door against session start; tenant value only shortens |
| 2 | `ORDO_SESSION_MAX_SECS` | tenant, person | no | no | no | no (deployment: `host/runtime.rs:405`, `identity/session.rs:428`) | no | no | no | All seven. A person's value needs the person store |
| 3 | `ORDO_AUTH_TEMPORARY_PASSWORDS` | tenant | no | no | no | no (deployment bool: `host/credential_ops.rs:659`, from `host/runtime.rs:2812`) | no | no | no | All seven. Env stays the ceiling, tenant switch beneath it |
| 4 | Step-up freshness (300 s) | tenant | no | no | no | no (one engine value: `identity/access.rs:842`) | no (op level min exists: `identity/access.rs:842`) | no | no | All seven. Not yet a deployment setting: setter `access.rs:473` and `require_step_up_within` `access.rs:664` have no caller in host. Engine is keyed by op, not tenant |
| 5 | `ORDO_STEP_UP_EXEMPT_OPS` | tenant | no | no | no | no (deployment list, applied once at build: `host/lib.rs:2034`) | no | no | no | All seven. Needs a request-time, per-tenant check that can only add presence |
| 6 | Password length or strength rule | tenant | no | no | no | no (no rule anywhere; sites `host/credential_ops.rs:692`, `host/auth.rs:1170,1222`) | no | no | no | All seven, and the deployment rule itself. Governs only passwords a tenant types |
| 7 | Invitation lifetime (3 d, cap 30 d) | tenant, person | no | no (no op issues an invitation ticket; create and reset pass 0: `host/credential_ops.rs:323,626`) | no | no (constants: `host/tickets.rs:312-315`) | no (fixed 30 d cap: `host/tickets.rs:57`) | yes (`expiresAt` in the answer: `host/credential_ops.rs:642`) | no | Issuing op with `$expires`, tenant default, cap taken from layers not a constant |
| 8 | Idle-tenant clean-up | tenant | no | no | no | no (nothing runs) | no | no | no | All seven, and a scheduled listener to run it (12a) |
| 9 | Sealed replica idle lock and retention | tenant, person | no | no | no | no (no code) | no | no | no | All seven. Plan only (`plan-client-cache.md`, 22m). A person value is device state plus the person store |
| 10 | Blob upload cap (100 MiB) | tenant | no | no | no | no (constants: `host/runtime.rs:2690,2742`; checks `blob/pipeline.rs:358`, `api/router.rs:253`) | no | no | no | All seven. The limit is held twice, at the door and in the pipeline; the tenant is known before the door check (`api/router.rs:238`) |
| 11 | `ORDO_DENY_ALERT_THRESHOLD` and `_WINDOW_SECS` | tenant | no | no | no | no (one rule for all: `host/metrics.rs:392`, `secevent/alert.rs:277`) | no | no (alerts served to the deployment space only: `host/serve.rs:914`) | no | All seven, plus a way for a tenant to see its own alert |
| 12 | Tenant security stream retention (90 d) | tenant | no | no | no | no (one value: `secevent/store.rs:490`, `host/secdrain.rs:283`) | no | no | no | All seven. One seam: `Retention::keep_secs_for(scope)` (`secevent/store.rs:514`). Tenant may only lengthen |
| 13 | Command log undo horizon (90 d) | tenant | no | no | no | no (one `LogPolicy::DEFAULT`: `host/runtime.rs:1632,1652,2996`; used `exec/prune.rs:512`, `exec/blobgc.rs:401`, `exec/erase.rs:293`) | no | no | no | All seven. Callers already take a policy argument, so a per-tenant lookup fits. Tenant may only lengthen |
| 14 | Egress timeout and body cap | tenant | no (`$attestsinks` has one row per tenant, no such column: `exec/webhook.rs:100`) | no (`std.space.attestTo`, owner only, takes no limits: `host/platform_ops.rs:1188`) | no | no (`host/attestdrain.rs:174,199,451`, `egress/client.rs:58,61`) | no | no | no | All seven. Columns on the sink row and a bound at the deployment value |
| 15 | `ORDO_SHARE_LINKS` | tenant | no | no | no | no (no share link op exists) | no | no | no | All seven. Waits on the link op (plan-sharing L1) |
| 16 | `ORDO_SHARE_MAX_SECS`, tenant layer | tenant | no | no | no | no (deployment only: `host/runtime.rs:1846`, `host/share_ops.rs:334`) | no | no | no | Tenant layer: all seven. The deployment ceiling and the per-share act are wired (row 20) |
| 17 | Capability grant and delegation lifetime | person | yes (`host/grants.rs:47,56`, `host/delegation.rs:57,63`) | no (no non-test caller of `GrantRegistry::record`) | no | yes (expiry checked: `host/grants.rs:221,264`, `host/delegation.rs:130`) | no (no ceiling, no tenant bound) | yes (`grants_of` returns expired grants too: `identity/grants.rs:160`) | no | No op mints a grant or delegation (consent page unbuilt); no exposure; no bound |
| 18 | Share link default lifetime (7 d) | tenant | no | no | no | no (field never read: `blob/policy.rs:52,59`) | no | no | no | All seven, and the link op that would read it |
| 19 | Admission default role (reference, E4) | tenant | yes (`host/admission.rs:225`) | yes (`host/platform_ops.rs:1082`, owner only `:1210`, presence `host/lib.rs:2034`) | no | yes (`host/admission.rs:385-411`) | no (ceiling covers ways only: `host/admission.rs:571`) | yes (`std.space.admission`: `host/admission.rs:629`) | yes (`host/admission.rs:580`) | Exposure gate, an app developer default, a bound on the role |
| 20 | Lifetime of one share (reference, G2) | person (the sharer) | yes (ticket and grant expiry: `host/share_ops.rs:513-524`) | yes (`std.space.share`, manager and owner: `host/platform_ops.rs:938`) | n/a (the act states it) | yes (`host/share_ops.rs:469`) | yes (deployment ceiling: `host/share_ops.rs:334`) | yes (`windowEndsAt`: `host/share_ops.rs:553`) | yes (`host/share_ops.rs:533`) | Only a manager or owner may share; no tenant layer between the act and the deployment |

### Tally of rows 1 to 18

| Piece | Rows with it | Which |
|---|---|---|
| Store | 1 | 17 |
| Write path | 0 | none |
| Exposure gate | 0 | none |
| Read at point of use | 1 | 17 (per-grant expiry) |
| Bounds against the layer above | 0 | none |
| Visible | 2 | 7, 17 |
| Recorded | 0 | none |

The two visible cells in rows 7 and 17 are the individual act's own
expiry, not a tenant's value.

## Ranked: what a tenant most needs to change first

All ten wait on the shared machinery above. The order is by need, then by how
little more each takes once the machinery exists.

1. **Share lifetime, tenant layer (16).** The owner named it. The act layer
   and deployment ceiling are wired (row 20), so the tenant value is one
   more layer in the resolver.
2. **Session life (1, 2).** The most common tenant security ask, and a
   tenant that handles sensitive data cannot shorten it today.
3. **Temporary passwords (3).** The owner ruled it a recorded option and said
   to "store the tenant's choice like the admission policy". One switch
   under the existing env ceiling.
4. **Step-up freshness (4).** A tenant that wants recent proof for its own
   risky ops has no lever. The tighten-only rule already exists in the engine.
5. **Invitation lifetime (7).** The fixed 30 day cap contradicts the
   owner's ruling of no platform maximum, and no op issues an invitation yet.
6. **Security stream retention (12).** A compliance need that differs by
   tenant, and tenant retention has no setting at all.
7. **Blob upload cap (10).** A tenant lowers it to control cost and abuse.
   Needs the door and the pipeline to agree.
8. **Undo horizon (13).** A tenant that must keep history longer for audit.
9. **Deny alert threshold (11).** Busy and quiet tenants differ. Needs a way
   for the tenant to see the alert first.
10. **Step-up exempt ops (5).** Tenant tightening only. Lowest urgency: the
    default already requires presence for every platform op.

## Rows that proved wrong against the code

1. **Sharing, section 5 intro and rows for `ORDO_SHARE_MAX_SECS`.** The
   section says "Nothing here is built". `std.space.share` and
   `std.space.revokeShare` are built (`host/share_ops.rs:61-62`), and
   `ORDO_SHARE_MAX_SECS` is read at boot (`host/runtime.rs:1846`). The row
   says "365 days in the plan"; the code has no default ceiling and
   `deploying.md:271` calls the variable optional.
2. **Capability grant and delegation lifetime, "built, exposable".** The
   store is built. No production code mints a grant or delegation, so a
   person cannot state a lifetime today.
3. **Invitation (ticket), "built, tenant sets".** Only redemption exists
   (`host/admission.rs:401`). No op issues a ticket with purpose invitation;
   `std.space.createPerson` issues password and passkey tickets.
4. **Blob inline cap, "hardcoded".** The constant is never read.
   `BlobPolicy::admits_inline` has no caller (`blob/policy.rs:67`), so the cap
   is not enforced anywhere.
5. **Share link default lifetime.** `share_ttl_default` (`blob/policy.rs:59`)
   is never read. The doc comment says "unused until the share-link op
   exists".
6. **Blob upload cap location.** `settings.md` cites `host/runtime.rs:2611`.
   The value is held at `host/runtime.rs:2690` (pipeline) and `:2742` (door),
   and the door check is at `api/router.rs:253`.
7. **Step-up rows.** Row 97 cites a setter at `access.rs:464`; it is at
   `identity/access.rs:473`. The engine also has `require_step_up_within`
   (`:664`) and per-client demands (`:672`), which the row does not
   mention. Row 138 cites `host/lib.rs:1965`; the exemption is applied at
   `host/lib.rs:2034`, once at build time.
8. **Line drift in `host/runtime.rs`.** Command log policy is at
   `:1632,1652,2996`, not `:1589,1609,2916`. Page size is at `:2870`, not
   `:2791`.
9. **Deny alert rows.** Correct as written, with one gap: the alert is served
   only to the deployment space (`host/serve.rs:914`), so a tenant cannot see
   what a tenant threshold would produce.
