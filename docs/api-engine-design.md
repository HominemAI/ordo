# Ordo API Engine

Companion to `implementation.md`. The HTTP layer that serves registered ops,
generated entirely from the artifact registry. No per-op code, no language on
the wire; requests carry an op name, parameters, and transport envelope keys.
Crate: `ordo-api`. Status markers as elsewhere: [settled] / [lean] /
**[proposed]** (a gap filled here with a concrete design, not yet ruled on).

---

## 1. Route model [settled]

One path, methods enforced from the op's **derived kind** [settled;
supersedes the earlier POST-only ruling]:

| Method + route | Kind served |
|---|---|
| `GET /op/{name}` | read ops, one shot; params in the query string |
| `GET /op/{name}?live=1` | read ops, live subscription (SSE), live-capable only |
| `POST /op/{name}` | write ops containing any `set` (incl. mixed set+del) |
| `DELETE /op/{name}` | pure-`del` ops; params in the query string |
| `GET /live` | one shared live stream (SSE) for many reads (§6.1) |
| `POST /live/{stream}` | add one read to a shared stream (§6.1) |
| `DELETE /live/{stream}/{key}` | drop one read from a shared stream (§6.1) |

The method is the op's derived kind, mirrored
onto HTTP. The client does not choose it. A request with the wrong method fails `method_mismatch` (405,
`Allow` header carries the correct one) **before** access check or binding.
This is an extra structural buffer: a caller cannot even *address* a write as if it
were a read. Reads execute in read-only database transactions (replica-
eligible); `Idempotency-Key` is honored on `POST`/`DELETE` only. Generated
SDKs surface the same split as terminal verbs (`.fetch()`/`.live()`/
`.pages()` vs `.commit()`); see tooling-and-embedding-plan §B3.

Dispatch pipeline, in order, all before execution:
**registry lookup → method enforcement → space bind → access check → param bind
→ refinement validation → cursor decode → execute prepared plan (against the
bound space, §12).** One check at the door: the access decision happens before
binding, planning, or any executor work, and the space is bound immediately
before it because the check takes one. Denials name the attempted op and the
decision **in the record** (`security-logging-design.md` §6.1). What they name
*on the wire* is governed by the non-enumerability rule (§4.1). The `OpSpec` is
**cloned at dispatch**, so everything after lookup is immune to a concurrent
registry swap (§11).

## 2. Request envelope [settled]

Body is a JSON object. Bare keys are op parameters (declared names, no `$`
prefix on the wire). `$`-prefixed keys are transport, closed set:

- `$space`: the space this call is bound to (§12) [settled; it was missing
  from this list while both the router and the access engine required it]
- `$refine`: Tier-0 refinement envelope (§7)
- `$page`: opaque page token (§8)

Any other `$`-key is `unknown_param`. Headers:

| Header | Direction | Meaning |
|---|---|---|
| `Authorization: Bearer <token>` | request | principal token; missing/malformed ⇒ `access_denied` |
| `X-Ordo-Space` | request | the header form of `$space`, preferred on `GET`/`DELETE`, where there is no body |
| `Idempotency-Key` | request | write replay key (§9); ignored on reads |
| `X-Ordo-Version` | request | version hash the caller's generated code was built from (§10) |
| `X-Ordo-Op-Version` | response | the served op's version hash, always |
| `X-Ordo-Version-Skew: stale` | response | present iff `X-Ordo-Version` was sent and mismatches |
| `X-Ordo-Idempotent-Replay: true` | response | this response came from the replay cache |

**One exemption: `scope deployment` ops** [settled]. A small, enumerated set
(today `std.space.create` and `std.space.reap`, see `bootstrap.md`) binds **no**
space, because it runs before one exists or after one has failed to exist.
These are not exempt from authorization: the door consults a **deployment
capability** instead of a per-space profile, and `$space`/`X-Ordo-Space` on
such a call is `unknown_param`, never silently ignored. Every other op keeps
the unconditional binding below, and `missing_space` stays a 400.

**Space binding, in exact order** [settled]: the space is bound from `$space`
or `X-Ordo-Space` (both present and disagreeing is an error, not a
precedence rule) **before** the access check, because the check takes a space.
There is **no default space and no space inferred from the token**. A human
belongs to many spaces, and a token that pinned one would collapse the
multi-space product. A request that binds no space is `missing_space` (400).
Whether the space *exists*, and whether this principal has any relationship
with it, is the access checker's single answer (§4.1). A distinct "no such
space" would enumerate the deployment's spaces. `$space` is **stripped before
parameter binding** and is **rejected as a declared op parameter name at
registration**, so no op parameter can shadow it.

Live `GET` params ride the query string; values coerce by declared type
(`int` parses, `bool` accepts `true/false/1/0`, everything else is the string
as given; the binder then validates identically to `POST`). `live` is the
only reserved query key. `$refine`/`$page` do not apply to live sessions.

## 3. Response envelope [settled]

```
{ "op": "vendorSpend", "version": "<hash>", "mode": "creates"?, 
  "rows": [ … ], "next": "<token>"? }
```

- `rows`: always an array, even for one row.
- Addressed writes return the written row (post-defaults, post-`rev`) as
  `rows[0]`; bulk writes return affected-id descriptors. Per the definition
  language, the executor supplies both; the envelope shape does not vary.
- `mode`: the compiler's static mode label, present on writes.
- `next`: present iff another page exists. Absence terminates iteration.
- Status: `200` for every success, writes included. Created-ness is the mode
  label.

## 4. Error taxonomy [settled]

```
{ "error": { "code": "<code>", "message": "…", "op": "<name>"?, "param": "<name>"? } }
```

`param` names the offending parameter or refinement field when one exists.
Codes are closed; messages are loud and name valid alternatives:

| Code | Status | Meaning |
|---|---|---|
| `unknown_op` | 404 | **never answered on the request path**: the door answers `access_denied` for an unregistered name too (§4.1). Retained for the surfaces legitimately allowed to say it: build tooling, `ordo check`, the deploy diff |
| `unknown_param` | 400 | body key matches no declared param; the message lists valid names |
| `missing_param` | 400 | required param absent (explicit `null` counts as absent) |
| `invalid_param` | 400 | value has the wrong shape for its declared type |
| `bound_violation` | 400 | declared bound exceeded (numeric/date range, `text(n)` length) |
| `invalid_refinement` | 400 | envelope malformed, op not refinable, unknown field (named), a value that does not fit its field's type (named), or past a size limit (the limit and its setting named, §7) |
| `invalid_cursor` | 400 | `$page` token undecodable, or paging/refining a write |
| `not_live` | 400 | `?live=1` on an op that is not live-capable |
| `method_mismatch` | 405 | method ≠ the op's derived kind (`Allow` header names it) |
| `missing_space` | 400 | no space bound: `$space` and `X-Ordo-Space` both absent, ill-formed, or disagreeing (§2). There is no default space |
| `null_key_address` | 400 | a converge-or-create whose addressing unique key's bound value is **null** (`definition-language.md` §3), never a create. Request-shaped, so 400: no stored data decided it |
| `access_denied` | 403 | **the door's single answer, for every reason it refuses that depends on the op or the space**: token missing, malformed, or expired; op unregistered; op registered but not callable by this token in this space; step-up unsatisfied (§4.1). The one carve-out is `delegation_required`, below, which is decided before either is looked at |
| `access_guard` | 403 | executor pass-through: a **row-level** guard precondition failed on a row the caller may read (§4.1) |
| `quota_exceeded` | 429 | the space this request named has used its plan's `requests` or `rows_returned` quota, plus grace, for the period (17n M2, `plan-production-ops.md`). Raised only after the access check, so a caller who is not a member of the space is answered `access_denied` and learns nothing about the quota. A quota has a limit and a grace: over the limit alone warns and serves; this class is raised only past limit **plus** grace. v1 gates `requests` and `rows_returned` only. Both are counted at the door as they happen, so the quota is exact. `writes` is counted from the command log at rollup and a quota on it would bite up to a rollup period late, so it is deferred to 18i rather than minted here as a quota in name only. Its own class rather than `delegation_budget` (unminted): a space over its plan and a delegation over its grant are different limits with different remedies |
| `human_check_failed` | 403 | a public write (`design-public-path.md` §5) without a human check the provider confirmed: no `X-Ordo-Human` header, a token the provider refused, a provider that could not be asked, or no provider set. The door's own, before the executor, so nothing was written. Where a setting fixes it, the message names `ORDO_PUBLIC_HUMAN_SECRET_FILE` or `ORDO_PUBLIC_HUMAN_CHECK=off` |
| `delegation_required` | 403 | the token acts **on behalf of** somebody and that delegation is not live (withdrawn, lapsed, or unknown; `definition-language.md` §8; `bootstrap.md` item 10). The one door refusal that is *not* folded into `access_denied`, and the reason is positional: it is decided from the token before the op name or the space is consulted, so every name the same token could send gets the same answer and the class correlates with nothing about the deployment. It discloses only the state of the caller's own delegation, and its repair is a different act by a different person: the delegator consents again. A token claiming **no** delegation never sees it (§8: without one, an agent "acts only as itself") |
| `not_found` | 404 | executor pass-through: the addressed row does not exist, **or** exists outside the caller's read view (§4.1) |
| `idempotency_in_flight` | 409 | an identical request under the same key is **still executing**; the caller retries. Emitted when the loser of a reservation race outwaits its wait budget. It never causes a second execution |
| `idempotency_applied` | 409 | same key, same payload, and the effect **already committed**, and its response body was lost with the process that ran it, so it cannot be replayed verbatim. **Terminal: do not retry.** The sibling of `idempotency_in_flight` and deliberately not the same class, because the two say opposite things about the future: `in_flight` means *ask again*, this means *this will never change*, and a client that polls on the class name would poll forever under the wrong one. Raised by the router when the executor's in-transaction journal read answers `Applied` (§9, the crash window) and the response store has no body; never cached, since a stalled holder's `put` may still land the real body in that slot |
| `version_skew` | 409 | page token minted under a different op version |
| `expected_absent` | 409 | the write asserted `rev: null` (create-once) and the unique key already matches a row; nothing applied |
| `conflicting_identity` | 409 | executor pass-through (write addressing collision) |
| `duplicate` | 409 | executor pass-through (unique violation) |
| `cas_miss` | 409 | executor pass-through (`rev` mismatch; refetch and retry) |
| `idempotency_mismatch` | 409 | same `Idempotency-Key`, different request fingerprint (§9), never a replay |
| `refinement_unbounded` | 400 | the refinement raises the op's **effective** cost class past what it was registered for (§7.1). `param` names the field that did it. Raised at the door, beside the declaration-time refusals of §7 and never instead of them |
| `serialization_conflict` | 409 | a SERIALIZABLE command group exhausted its retry budget (`implementation.md`, write path). Retryable, and *not* `internal` |
| `statement_timeout` | 503 | one statement ran past the deployment's `statement_timeout` and the server cancelled it (SQLSTATE 57014), so nothing it was doing landed. Producer: `ordo_exec::PoolError::StatementTimeout`; the bound is pinned on every pooled connection at dial (`ORDO_PG_STATEMENT_TIMEOUT_MS`, default 30s). 503, like `space_unavailable` and `server_busy`: the request is well formed, the server did not break, and no stored row decided it. Ask for less, or retry when the database is not being held by someone else |
| `caller_supplied` | 403 | the request envelope carried `$caller` as a client parameter. It is refused at the door, before binding or planning, so no guard is evaluated against the principal the client proposed (`guards-conformance.md` R-G13). 403 rather than 400: it is not a malformed envelope, it is an attempt to assert a principal |
| `lease_fenced` | 409 | the idempotency lease this attempt held was taken over by a newer holder; its effect was rolled back rather than committed under a key someone else now owns. Retryable |
| `has_referrers` | 409 | deleting a row another row still references through a plain (not `owned`) reference, naming the referrer and its field (`writing-data.md` §6b) |
| `tree_cycle` | 409 | a move that would put a node inside its own subtree |
| `tree_has_children` | 409 | deleting a tree node that still has children |
| `tree_depth_exceeded` | 409 | a create or move past the depth cap; the *parent's* current depth decided it |
| `tree_bulk_reparent` | 400 | a ranged move; tree nodes move one at a time. 400 rather than 409 because no dataset would accept it |
| `reserved_derived_name` | 400 | the statement writes something the **platform owns**: `id`, `rev`, or any field of a **platform-provided entity** (`members`). DL §3 makes a platform entity's fields "all read-only to domain code" with "no domain-authored path to minting one"; refused at registration so the op never deploys, and again at the write path's dispatch |
| `ambiguous_identity` | 400 | an unconditioned `set` whose braces cover more than one unique key (DL §6 rule 8) |
| `unbound_required_param` | 400 | a parameter or a required field reached the write with no value and no default |
| `projection_unknown_field` | 400 | the statement assigns a field the entity does not have. On a read: a projection names a field its source does not have, or two sources of one list project the same output name (`a[note] b[note]`, ruled 2026-10-05) |
| `invalid_binding_expr` | 400 | a right-hand side the statement cannot mean: one that reads the row in a statement that can CREATE (there is no current row), one that reads the row into a field whose value the platform must hold first (sealed, unique-keyed, ref, closed-set, content), a dot-path (a write sees the row it writes and no other), an aggregate, `rev` where nothing is addressed, a subtree delete with no anchor |
| `param_type_mismatch` | 400 | a binding expression's value does not have its declared type |
| `arithmetic_error` | 400 | executor pass-through: `/ 0`, `mod(a, 0)` or an `int`/`money` overflow (SQLSTATE 22012/22003). DL §2 makes all three "a typed runtime error"; one class because there is no repair for one that is not the repair for the others. 400 asserts *this request has no answer, pose a different one*. It is not caller fault: the engine cannot tell a caller-supplied operand from a stored one, and a code that is sometimes wrong about fault is worse than one that is consistently modest. Never auto-retried (`is_exec_conflict()` is false: nothing applied, and a retry over unchanged data fails identically) |
| `check_violation` | 409 | executor pass-through: a declared row invariant (`check used <= seats`, DL §3) refused the write (SQLSTATE 23514, read once at the pool the way the arithmetic family is). The data violated the rule; the server did not break, so it never wears `internal`. The message carries the constraint name, and the name carries the condition (`teams$chk0$used<=seats`), the only channel Postgres offers wide enough to say WHICH check failed. 409 with the state-decided family: change the data the write collided with and the identical request lands. In `is_exec_conflict()` for `duplicate`'s reason: the refusal IS what the op did against the data at that moment, so a replayed idempotency key answers the same refusal rather than re-executing |
| `unknown_source` | 500 | a registered op names an entity or callee this deployment does not have. **500 and deliberately so**: it is not about the request; it means an op survived registration referring to something absent. Named rather than plain `internal` so the operator is told which kind of broken. Nothing enumerates: the missing name comes from the op's body, never from the caller |
| `sealed_open_failed` | 500 | a sealed value could not be opened because the key was not available to the read (a custody fault, not a statement about the value). The caller can do nothing about it; an operator must be able to tell it from an unknown panic |
| `sealed_unreadable` | 500 | a sealed value failed to authenticate or decrypt while its key is present: unreadable, possibly tampered (owner, 2026-10-23). Never retryable, and never called "destroyed", which means only a key someone erased on purpose. The message names the entity and field, never the value or key material. The deployment owner is alerted: a `sealed-unreadable` event on the space's security-event stream (rate-limited per entity, field and epoch) and the `ordo_sealed_unreadable_total` counter |
| `log_append_failed` | 500 | the write's command group could not be appended to the space's durable command chain, so the write itself was **rolled back**. 500 for `sealed_open_failed`'s reason: the caller did nothing wrong and can do nothing about it, but an operator must be able to tell an audit-spine fault from an unknown panic. The refusal is deliberate: an effect that commits without a link is a write nobody can attribute or undo, which is what the log exists to prevent |
| `sealed_unsupported` | 422 | the op's plan needs a venue this executor does not serve: an `unbounded-host` read, a sealed read answered whole with no bound the plan proves, or a sealed read by an anonymous caller. The message names the sealed field and the fix. A `bounded-host` read is served, one key release per page (27ao). 422 (ruled 2026-10-05, `audit-2026-10-05c` ask 2): what is left is an op shape the app developer fixes, such as a missing take. The request is well formed and nothing failed. It was 501 while sealed reads were unbuilt. Also raised at the door for a refinement condition on a sealed field with no blind index, with `param` naming the field (§7, ruled 2026-10-05) |
| `not_confirmed` | 400 | a destructive op arrived without its confirmation, or with the wrong one; `std.space.delete` requires the space's own id typed back. Deliberately **not** `missing_param`/`invalid_param`: those are the door's type check, and this is a refusal *after* the value bound correctly, made because what the value says is not what the op requires it to say. A caller told "wrong type" where the truth is "you have not confirmed you meant this" would go looking for the wrong bug |
| `space_pinned` | 409 | an irreversible space-level act was refused because the space still owes something the audit spine will not let it drop: an `archive` consumer that has not received the history it is the copy of record for, a `client_first_mirror` that is the authoritative store, a `listener` with at-least-once deliveries outstanding. The same non-evictable set `prune` respects, read once more. 409 with the state-decided family: the identical request lands once the consumer catches up or is de-registered, which is an act somebody performs on purpose and the log records. An evictable stalled `mirror` never raises it |
| `schema_drift` | 500 | the live database is not the shape the deployed artifact asserts, for example a migration half-applied, a restore onto an older schema, a column changed behind the deployment's back. Raised by `std.space.export`, which ships `schema.sql` and the rows side by side and must not hand over a bundle whose own schema does not describe its own data. Its own code rather than `internal` because there is exactly one operator action that fixes it, and an operator who reads `internal` goes looking for a panic. `ordo_host::deploy::LoadRefusal::SchemaDrift` refuses a *load* on the identical fact; this is the same fact reaching a caller |
| `egress_refused` | 400 | an outbound destination was refused by one of `security-logging-design.md` §7b's egress pins: not `https`, not a usable URL (a userinfo section, a fragment, a control byte), not on the space's consented allowlist, does not resolve, or resolves to an address inside the perimeter. Producer: `ordo_host::attest_ops::egress_refusal` ← `ordo_egress::Refusal`. **One class for all six pins**, and the message carries the *category* (`loopback`, `link-local`, `private`) because §7b requires the deny list to be disclosed on the consent page, never the resolved address, and never which of the two halves (a structural category or this deployment's own pod CIDR) matched. A class per pin would be a reconnaissance primitive: it would let anyone who can name a hostname map the deployment's internal address plan one grant at a time |
| `proof_unavailable` | 409 | `std.space.chainProof` was asked for a range this deployment cannot prove: below the pruning floor, past the head, wider than one artifact, or a segment that does not recompute locally. Producer: `ordo_host::attest_ops::AttestOps::chain_proof`. Deliberately **not** `not_found`. A range whose entries were lawfully pruned is a range the deployment is telling the truth about, and collapsing it into "no such thing" would let a deployment answer a tampering question with a shrug, which is the one answer `security-logging-design.md` forbids: *"I could not check" and "it checks out" must not be the same value*. 409 with the state-decided family, because a narrower range lands |
| `apply_refused` | 409 | `std.deployment.apply` (18l) refused a program before sending anything. Producer: `ordo_host::apply_op::ApplyRefused`. The **class is the first word of the message**, in the spelling `ordo_host::schema_apply::Refused::class` gives it (`ops_do_not_compile`, `plan_refused`, `not_class_a`, `artifact_first`, `blind_index_backfill_required`, `constraint_over_existing_rows`), so the same defect reads identically whether the operator ran `ordo apply` against the database or against the door. One code for the whole set because the taxonomy is the apply path's to extend (increments 3 through 5 add to it), and a wire code per class would make every new refusal an amendment to a closed inventory. 409 with the state-decided family: nothing was sent, and the identical call lands once the op recompiles or the offending rows are fixed |
| `apply_in_progress` | 409 | another apply already holds the deployment (18l). Applies serialize on an advisory lock keyed by the `$migrations` table, so two of them queue rather than interleave; this one waited out its bound (`ordo_host::apply_op::LOCK_WAIT_MS`) and gave its connection back. Producer: `ordo_host::apply_op::ApplyLock`. Its own class rather than `serialization_conflict`, which is a transaction the engine retries for the caller: an apply must **not** be retried automatically, because the apply holding the lock may be changing the very schema this one planned against, so the retry has to re-plan and only the caller can decide to |
| `not_activated` | 409 | the artifact was stored and the deployment was **not** re-pointed at it, because the load gate would refuse it over this database (18l). The message carries each blocking change with its diff code. Producer: `ordo_host::apply_op::ApplyRefused::NotActivated`. Its own class rather than `apply_refused` because the outcomes differ in the one way an operator acts on: here the artifact *is* stored, so the next step is `ordo migrate --apply` and not a recompile |
| `tenant_refused` | 400 | `std.space.create` (17aa) will not create a tenant from the values given, and created nothing. Producer: `ordo_host::tenant_ops::TenantOps::create`. **Not `invalid_param`**, which is the door's word and one an executor may not say: the door validates a parameter against the spec (present, a string, inside the length) and these pass it. What refuses them is a rule the door cannot see, because it belongs to provisioning: an `owner` becomes the tenant's founding **member id**, so it must be an identifier the door could later address, and a blank `name` is the one attribute an id nobody chose was meant to be picked out by. Only discoverable below the door, which is `invalid_cursor`'s argument for being raisable there too. One code for the set, on `apply_refused`'s reasoning (the taxonomy is `tenant_ops`' to extend), with the rule named in the message. It is 400 because the values are wrong, not the moment, so no state in the deployment makes the identical call land |
| `bucket_refused` | 400 | `std.space.attachBucket` (18p U6) will not attach the bucket the caller named, and attached nothing. Producer: `ordo_host::bucket_ops::BucketOps::attach`. The set is five rules (six until 2026-10-06, when the D-S2 rule below was lifted): an endpoint off the deployment's attach allowlist, a plaintext loopback endpoint outside dev mode, the deployment's **own** default bucket, a bucket another tenant is already attached to (D-D: one tenant one bucket), and a credential the bucket itself refused to sign for. **Not `tenant_refused`**, although the shape is the same: an operator's alerting keys off the class, and `tenant_refused` says a tenant was not created, which is not what happened. One code for the whole set, on `apply_refused`'s reasoning, with the rule named in the message, **except the allowlist, which carries one reason and no detail**, because a distinguishable refusal is an oracle that maps the allowlist one attempt at a time. 400 rather than 409: what is wrong is the target, and no state in the deployment makes the identical call land |
| `import_refused` | 409 | `std.space.import` (11-10a) will not load the bundle the caller named, and loaded nothing. Producer: `ordo_host::exit_ops::ExitOps::import`. The **class is the first word of the message**, in the spelling `ordo_exec::import::ImportError` gives it: `format_unknown`, `bundle_incomplete`, `artifact_mismatch`, `field_mismatch`, `space_not_empty`, `cyclic_entities`, `tree_cycle`, `unresolved_ref`, `row_refused`, `unencodable_value`, `blob_door_missing`, `blob_unplaced`, `blob_tier_unsupported`, `blob_refused`. One code for the whole set on `apply_refused`'s reasoning, and for the same reason it will grow: the deferred work in `plan-import.md` §7 adds classes, and a wire code per class would make each increment an amendment to a closed inventory. 409 with the deploy family rather than `tenant_refused`'s 400, because the bundle is **named** rather than supplied inline, so every class including a malformed bundle names state the caller repairs and retries the identical call over. **Two import refusals are deliberately other codes**: a ciphertext bundle is `sealed_unsupported`, and a bundle the destination's blob door refused keeps the door's own class, as `row_refused` keeps the write protocol's |
| `space_unavailable` | 503 | the database or object store this space lives in could not be reached (18d T5, 18p U7). Producer: `ordo_host`'s space resolver, below the door. The request is well formed, nothing was written, and the identical call lands when the dependency returns. **503 rather than the `internal` it used to answer**, because a 500 says the deployment is broken and somebody should look at it while this says a dependency is down and the right response is to retry; `statement_timeout` and `server_busy` are the inventory's other 503s and make the same claim about different resources. **The message is opaque about the cause**: a dial error names a host, a port and often a user, which is deployment topology a space owner has no business reading, and for a tenant on their own cluster it is their own infrastructure described back to them by a third party. The reason an operator needs is in `/health/ready`'s per-tenant list. Deliberately not the class a rewound chain gets, which stays `internal`, because no amount of retrying fixes a chain that needs a recorded restore |
| `server_busy` | 503 | every database connection this deployment holds is in use, and the request waited out the pool's bounded acquire (reported fix 25z). Producer: `ordo_exec::PoolError::Saturated`; the bound is `ORDO_PG_POOL_SIZE` and the wait is `ORDO_PG_ACQUIRE_TIMEOUT_MS` (default 2s). Nothing ran. The response carries `Retry-After: 1`. **Its own class**, because the cause differs from the other two 503s: the statement never started, so it is not `statement_timeout`, and the database is up, so it is not `space_unavailable`. The pool fills and drains as the requests ahead finish. The message is fixed and names no pool size and no space; `/metrics` has `ordo_pool_saturated_total{space}`. The door answers this class for a full pool whichever store-failure class a tier below filed it under. **The sign-in routes answer it too** (`/auth/*`, including the mail outbox enqueue behind an email `begin`): the same envelope, status and `Retry-After`, in place of `credential_store_unavailable`, which now means a store that is broken rather than busy. The SDK's auth calls retry it like ops (`PasskeyEndpoint.busyRetry`). Not cacheable for replay |
| `space_paused` | 423 | the tenant is paused (27ar): the deployment owner, or a tenant role the app developer granted `std.space.pause`, held it still on purpose. Its data is kept and nothing ran. Producer: `ordo_host::pause::gate`, in the platform executor after the door's access check, so a caller with no membership is answered `access_denied` and learns nothing of the pause; the blob door answers the same class. Ops the program marks `nopause` and the platform ops `ordo_host::pause::answers_while_paused` lists keep answering. **423 Locked, and its own class.** Not `space_unavailable`, whose 503 tells a client to retry: waiting never lifts a pause, somebody has to resume the tenant. Not `access_denied`: the caller may call the op, and a 403 would send them looking for a permission they hold. The message names the layer (`the tenant` or `the deployment`) and the reason word from a closed list, never a person. Not cacheable for replay: after a resume the identical call lands |
| `function_error` | 500 | a backend function threw, or answered something that is not JSON or is over its answer limit (`plan-backend-functions.md` §10). The app developer's code failed. The message is the function's own, scrubbed of every live secret value. Producer: `ordo_host::functions`, below the door. The same call fails again until the code changes |
| `function_timeout` | 504 | a backend function ran past its wall time (network waits included) or its CPU limit (counted as fuel). Producer: `ordo_host::functions`. The message says which limit and its value |
| `function_oom` | 500 | a backend function needed more memory than its limit. Producer: `ordo_host::functions` |
| `function_busy` | 429 | too many function calls are running for this tenant or this server (`plan-backend-functions.md` §9: 10 per tenant, 100 per process). Nothing ran. `Retry-After: 1`: the identical call lands once one finishes. Producer: `ordo_host::functions` |
| `tenant_not_ready` | 503 | the person's own tenant is still being set up (owner, 2026-10-21): its registry row is `provisioning`, so nothing in it can run yet. Nothing ran. Producer: the door, after the access check denied, through `AccessChecker::not_ready`; the blob door answers the same class. It is answered only to the person the tenant is being made for, and anyone else gets the uniform `access_denied`. The response carries `Retry-After`, `ORDO_PROVISION_NOT_READY_RETRY_SECS` (default 2). **Its own class**: the request is well formed and lands unchanged once the tenant is ready, as with the other 503s, but the tenant does not exist yet, which is neither a full pool nor an unreachable database. `GET /auth/provisioning` says how far setting up has got. Both SDKs retry it like `server_busy`. Not cacheable for replay |
| `email_required` | 403 | the app developer requires a verified email address for this op, and the signed-in person has none (owner ruling 2026-10-22). Nothing ran. Producer: the door, after the access check allowed the call, through `AccessChecker::email_required`. The message names the setting in force: `ORDO_EMAIL_REQUIRED` at the deployment, or `std.deployment.setEmailPolicy` from the app developer. The person adds an address with `POST /auth/email/begin` and confirms the emailed code. A share link's and a delegated token are never asked. **Its own class**: `access_denied` means ask whoever administers you; this the person fixes alone. Not cacheable for replay |
| `presence_required` | 403 | a deployment person's passkey touch, carried in the identity service's token as `touched_at`, is older than this op allows (`plan-identity-service.md` 4.3). Nothing ran. Producer: the door, after the access check denied, through `AccessChecker::presence_required`, and only for a deployment token that a fresh touch would admit. The client touches at the identity service (`POST /touch`, or `ordo login --touch`) and sends the same call with the new token. **Its own class**: `access_denied` means ask whoever administers you; this the person fixes alone. The same word the sign-in surfaces answer for the same reason. Not cacheable for replay |
| `internal` | 500 | executor internal failure |

Everything from `access_guard` down (excluding the door's own `access_denied`
and the envelope's binding classes) originates below the transport and passes
through untranslated. The platform's typed rejections stay typed on the wire.

**How the status is chosen** for a write-path refusal, which is the only part
of this table that is not obvious. Two questions, in order:

1. *Did stored data refuse it?* Then **409**: the identical request lands once
   that data changes. `has_referrers`, `tree_cycle`, `tree_has_children`,
   `tree_depth_exceeded` join `duplicate`/`cas_miss` here.
2. *Did the statement refuse it?* Then **400**: no state will ever make it
   succeed, which is the 4xx family's actual property.
   `tree_bulk_reparent` is the instructive one: it looks like its three tree
   siblings and is not one of them, because a ranged reparent is refused for
   every dataset there is.

A refusal that is neither is 5xx: the deployment is inconsistent, or the
ciphertext will not open. It keeps its class so the deployment owner can read
the log.

Historically the executor's rejections were a *second* closed inventory
(`ExecError`, eight variants) restating this one, and the write protocol grew a
vocabulary of seventeen that the eight could not express. Everything past the
eight arrived as `internal` (500): a tree cycle, a referrer refusal, and a
crash were one answer. `ExecError` now carries an `ErrorCode` directly, so an
executor refusal whose class is not on this table cannot be constructed, and
`access_denied` (the door's word and the door's alone) is rejected at that
boundary.

### 4.1 Non-enumerability: you learn a thing exists only if you may reach it [settled]

One rule, applied at three layers. It resolves the conflict between the guards
corpus's "an addressed write on an unreadable row is `access_guard`" and the
security review's "it must be `not_found`" **in favour of non-enumerability**,
because the former lets any caller with an id distinguish *exists but
forbidden* from *never existed*, deployment-wide.

**The door's answer is `access_denied` for every reason it refuses**
[settled; this was specified three incompatible ways and is now one]. An
unregistered name, a registered op this token may not call in this space, a
missing or malformed token, and an unsatisfied step-up all produce the
*identical* response: same status, same code, same message, same (absent)
headers. `403` rather than `404` because for a name nobody registered the
platform cannot know whether this caller would have been allowed, and the safe
assumption is that they would not. This matches `definition-language.md` G18
("denial at the door is `access_denied`") and the router as built; the earlier
reading here, forbidden ops answering `unknown_op`, is superseded, because it
achieved the same non-enumerability while disagreeing with both.

| Layer | Situation | Answer |
|---|---|---|
| **Op** | op does not exist | `access_denied` (403) |
| **Op** | op exists, principal may not call it in this space | `access_denied` (403), **identical**, so the wire cannot be used to enumerate the deployment's op surface across domains and spaces |
| **Op** | no token, malformed token, expired token | `access_denied` (403), identical again |
| **Token** | the token claims a delegation that is **not live** | `delegation_required` (403), the one distinguishable refusal, and only because it is decided **before** the op name and the space are read. Every name this token could send answers the same way, so nothing about the deployment is correlated with it; what it discloses is the caller's own delegation, and the repair is the delegator's re-consent rather than anything the caller can retry |
| **Row** | addressed read/write on a row that does not exist | `not_found` (404) |
| **Row** | addressed read/write on a **guarded** row **outside** the caller's read view | `not_found` (404), **identical** to absent |
| **Row** | addressed write on a row the caller **may read** but may not write | `access_guard` (403), the informative case, and the safe one: the caller already knows the row exists |
| **Row** | **create** naming a parent the caller cannot read | `not_found` (404) on the parent, **identical** to naming a parent that does not exist |
| **Row** | **create** whose parent the caller **can** read, failing the would-be row's own guard | `access_guard` (403), same reasoning as the addressed write above |

- **A create is two checks, and only the second one is informative** (V-010).
  A create addresses no existing row, which is why it was once ruled wholly
  `access_guard`, but it *names* one. `set tasks { project: $p, … }` puts an
  id in the caller's hands and asks the platform whether it resolves, which is
  the enumeration question with a different verb in front of it. So the parent
  reference is resolved under the ordinary read view first and answers
  `not_found` when it is not visible; only once the parent is readable does the
  would-be row's own guard run, and *its* failure is `access_guard`, because by
  then the caller already knows the parent exists. Splitting it this way keeps
  one rule for the whole surface: **the answer is informative exactly when the
  caller has already been shown the row.**
- **The exception is unguarded entities.** On an entity with no `access`
  clause and no inherited `acl`, there is no read view to be outside of, so
  `not_found` means exactly what it says: the row is absent. Non-enumerability
  is a property of *guarded* data; claiming it for unguarded data would have
  no effect, and the distinction is visible in the shape, the manifest, and the
  consent page.
- **The rule costs a little diagnosability and buys the oracle back.** The
  confusing case becomes the *less informative* error, never a quieter one:
  nothing is silently skipped, and the operational record still names exactly
  what happened for anyone with the log.
- **Timing must not restore the oracle.** The guard semi-join runs regardless
  of outcome, so absent-vs-forbidden are naturally close in cost; that is
  pinned as a **conformance assertion**, not left to luck.
- **Residual channels, stated rather than assumed** [open]. `method_mismatch`
  (405) and `missing_space` (400) are answered *before* the access check (the
  method is enforced structurally and the check needs a space), so a caller can
  still distinguish a registered name from an unregistered one through either.
  Closing them means checking access before method enforcement and before the
  space is known, which the pipeline order pins the other way. Both are noisy,
  and noisy is what §6.1's stream and the denial-volume alert are for; whether
  the pipeline order should change is a ruling this document does not own.
- **The wire is uniform; the record is not.** Every denial at every layer is
  recorded with its **decision** (unregistered-name distinctly from
  forbidden-op, `not_found`-because-guard distinctly from
  `not_found`-because-absent) in the space's security-event stream
  (`security-logging-design.md` §6.1) when the principal has a relationship with
  the space, and in the deployment security log otherwise. Denials also feed the
  denial-volume signal: non-enumerability makes probing quieter for the attacker
  and *louder* in the record and the metrics, which is the right trade only if
  both exist (`implementation.md`, Observability).

## 5. Parameter binding [settled]

Bind order: unknown keys rejected first (typos surface before "missing"
noise), then each declared param in registry order. Types, closed inventory
mirroring the language:

| `ParamTy` | Wire shape |
|---|---|
| `int` | JSON integer |
| `money` | decimal **string** in major units (`"-12.50"`), exact, never a float. A `money(p)` takes at most `p` places and a finer value is `invalid_param` (27ak). Results spell money the same way, with exactly `p` places: the column's `2999` is `"29.99"` at places 2 |
| `decimal(p)` | decimal **string** (`"0.0825"`), exact, never a float, with at most `p` places; a finer value is `invalid_param` (27ak). Results carry exactly `p` places, as Postgres prints the column |
| `text(n)` | JSON string; > n chars ⇒ `bound_violation` |
| `date` | `"YYYY-MM-DD"`, validated |
| `bool` | JSON boolean |
| `id` | typed id string (`prefix_body`) |
| `duration` | `"<int><unit>"`, units exactly `ms s m h d w mo y` |
| `datetime` | `"YYYY-MM-DDTHH:MM:SSZ"`, UTC, the `Z` mandatory, no offset and no fractional seconds |
| `uri` | JSON string: a scheme, then a body; no whitespace or control characters; at most 2048 bytes |
| `json` | any JSON value, carried as JSON and never as a string holding JSON (28b). Larger than `ORDO_JSON_MAX_BYTES` (default 262144, the compact serialization) or deeper than `ORDO_JSON_MAX_DEPTH` (default 64) ⇒ `bound_violation`; a NUL in a string or key ⇒ `invalid_param`. Results carry the stored document as JSON |
| `blob` | a file handle, 32 hex characters, as an op answers a `blob` field after the upload. A **function** parameter only; an op refuses `$f: blob` (`blob_parameter`). The function reads the file as the caller: the door checks `std.$blob.get` and the row guard runs on the read, so a handle the caller cannot read is `not_found` |

**`datetime` was missing, and it was not a documentation gap.** `$t:
datetime` has always parsed, and `ordo_host::param_ty` had a wildcard arm
(`_ => Text { max_len: None }`) that bound it at the door as **unbounded
text with no ISO-8601 validation**. The table had no row because the
inventory had no member; the wildcard is why nobody noticed. Both are
closed. `param_ty` is an exhaustive match returning `Option`, and its
caller treats `None` as an invariant violation and does not fall
back to text.

The offset and fractional-second exclusions are deliberate:
accepting `+02:00` would make the door decide a timezone conversion,
and `implementation.md` puts every instant in UTC precisely so that
decision has nowhere to happen. Two spellings of one instant is one more
than a canonical form allows. `60` seconds **is** accepted. A leap second
is a real instant, and refusing one would make the door disagree with the
clocks that emit them.

**`uri` is its own member and not a `text`**, for the same reason: binding
an address as unbounded text is a validation the door silently does not do.
The check is syntactic only. The platform *references* a
`uri` and does not fetch it (DL §2: "referenced, not held"), so a door that
validated reachability would be holding it.

**`blob` is deliberately absent.** Bytes are not a JSON wire value; they
move through the blob door (`PUT /blob/{entity}/{field}/{id}`,
`GET /blob/{ref}`), which is why that door exists as a second route rather
than an op. The compiler refuses `$f: blob` at registration with class
`blob_parameter`, so the absence here is enforced upstream rather than
relied upon.

Bounds (`min`/`max` on the spec) are enforced at the door, before planning:
exact-decimal comparison for `int`/`money`/`decimal`, lexicographic (= chronological)
for `date`. Bounds on other types are not supported in v1.

## 6. Live transport [settled]

**SSE for v1.** WebSocket can land later behind the same subscription
interface (`InvalidationBus`). Transport is modular per the definition
language; nothing in the protocol below is SSE-specific except framing.

Stream contract, in order:
1. Subscribe to the op's invalidation channel **before** the initial read,
   so no commit slips between snapshot and subscription.
2. First frame: a default `data:` event carrying the ordinary response
   envelope (§3), the initial result.
3. On each bus event: `event: invalidate` with an empty `{}` payload. The
   client **refetches through the normal op under its own scope**.
   Invalidation is a staleness signal, never a data push
   (refetch-through-scope; inherits trigger blindness: *that* something
   changed, never *what*).
4. Heartbeat comment (`: hb`) every 15s keeps proxies from reaping the
   connection.
5. A lagged subscriber (broadcast buffer overrun) receives `invalidate`. A
   missed signal and a coalesced signal are indistinguishable by design;
   refetch is always the correct response.

### 6.1 One stream, many reads (checklist 27s) [settled]

The owner, 2026-10-01: a stream per entity *"seems like it would be pricey
among tons of clients"*. So a client opens one stream and its hooks share
it. `GET /op/{name}?live=1` stays as above, one read on one stream. Live
stays off in clients until the app turns it on.

```
GET    /live                    first frame:  event: ready       data: {"stream":"<id>"}
                                then:         event: invalidate  data: {"key":"<key>"}
POST   /live/{stream}           {"key":"r1","op":"listTasks","params":{"from":"2026-01-01"}}
DELETE /live/{stream}/{key}     204
```

- **Opening** needs a bearer token and passes the token gate. Nothing else
  is checked, because no op or space is named yet.
- **Adding a read** runs the whole door for that op, with the `POST`'s own
  headers: token gate, registry lookup, method, space bind, access check,
  quota, then the live checks of §6. `params` is the query-string form;
  `$space` may ride there or in `X-Ordo-Space`. The answer is what
  `GET /op/{name}?live=1` would refuse with, or `200` with the read's first
  envelope and its version headers.
- **A refusal answers the `POST` alone.** The stream stays open, and its
  other reads keep their signals.
- **The door subscribes before the first read**, as in §6, so no commit
  falls between the envelope and the first signal.
- **A stream belongs to the token that opened it.** A `POST` or `DELETE`
  with another token, or for a stream that has closed, answers `not_found`.
  The two are one answer.
- **Keys** are the client's labels: 1 to 64 letters, digits, `.`, `_` or
  `-`. Adding under a key the stream carries replaces that read. Dropping a
  key it does not carry answers `204`.
- **Signals are tagged and coalesced.** While a key's signal is queued,
  more signals for it join it. A lagged read gets one too, as in §6.
- **Limits.** 256 reads per stream; the next answers `quota_exceeded`. A
  stream with no reads closes after 60 s. Closing the connection drops
  every read on it.
- **Sync** (`?live=1&sync=1`) is not carried. Its frames hold rows, and a
  shared stream carries signals only.

The React runtime (`clients/ordo-react/src/live.ts`) opens one shared
stream per client, adds a read per distinct (op, params), and drops it when
the last hook on it unmounts. Pacing (27n) stays per hook.

## 7. Refinement envelope: Tier 0 [settled]

Implements `tooling-and-embedding-plan.md` §B2. Structured data, never query
text; narrowing only:

```
"$refine": {
  "where": [ { "field": "billed", "op": "gt", "value": "4000" }, … ],
  "pick":  [ "name", "billed" ]
}
```

- `op` ∈ `eq ne lt le gt ge in is_null` (the Tier-0 set, closed).
- Conditions combine as **AND**. `in` takes an array value; `is_null` takes
  no value; the ordering/equality ops take one non-null scalar.
- Every `field`, in `where` and in `pick`, is validated against the op's
  output-field manifest; an unknown field is `invalid_refinement` naming it
  and listing the manifest. `pick`, when present, must be non-empty.
- **The tier is derived, never declared** (owner, 2026-10-05: refinement is
  a standard feature with no off switch). Every program read is `basic`. A
  write, a delete, a platform op and a read that does not bind are `none`;
  a public read will be `none` when the public path lands, because it keeps
  declared parameters only. Refining a `none` op is `invalid_refinement`.
- **Every value is checked against its field's type at the door**, so a bad
  value is `invalid_refinement` naming the field and never a 500. Text is
  normalized to NFC first. Money and decimal take a decimal string or an
  integer, never a float; a value finer than the field's places compares
  exactly (`m le "2.505"` on a `money(2)`). A blob takes no condition.
- **Size is a setting** (ruled 2026-10-05, `refinement-default` ask 6): at
  most `ORDO_REFINE_MAX_CONDITIONS` conditions (default 16) and
  `ORDO_REFINE_MAX_IN_VALUES` values in one `in` (default 200). The refusal
  is `invalid_refinement` and names the limit and the setting. The
  interface descriptor carries the same numbers as `refine.limits`.
- `std.E.count`'s conditions name `std.E.list`'s fields, and its `pick`
  names its own `n` (`std-library.md` §2.1, built 2026-10-05).

**Where the conditions run [settled, ruled 2026-10-05, `refinement-default`
ask 1].** Conditions on open and blind-index fields **compile into the
plan**, through the same inliner as written composition
(`definition-language.md` §5): the door's refinement becomes the call
`op($params, conditions)[pick]`, built as a tree so no value is ever spelled
as source. When the op's rows are settled by a `take`, a `group` or
`distinct`, the call wraps it, so conditions still apply after the op's own
`take`. `pick` is the projection; the anchor's `$id` rides every row that has
one. This replaces the earlier reading, which applied the refinement after
the page window.

**The consequence: pages come back full.** The page window sits on the
refined set, so:

- **Every page but the last is full**, and iterating a refined read two rows
  at a time yields exactly the rows one large page yields, in the same
  order. Absence of `next` is still the only terminator, because that rule
  costs nothing and stays right under any filter.
- **`std.E.list` and `std.E.count` under the same `$refine` describe the
  same set**, and the pages that deliver `count`'s `n` rows are full.
- **An open-field condition costs what a parameter costs**: a SQL predicate
  under the page's `LIMIT`. On a large entity a condition on an unindexed
  field is a scan, exactly as the same predicate in an op would be. The
  cost guide (`howto-cost.md`) says when to index.
- **A cursor reused with a changed refinement is harmless.** The cursor is
  the last row's sort keys; a refinement has no sort key and cannot change
  the op's order, so the new filter starts after the same row
  (`ordo-api/src/page.rs`).
- **Host-side conditions keep the after-the-window reading in principle.**
  A condition on a sealed field with no blind index can only be answered by
  opening payloads on the host. Until host-side filtering is built the door
  refuses it as `sealed_unsupported` (422), naming the field and the fix
  (ruled 2026-10-05, ask 3). An unbounded one is refused first by §7.1 as
  `refinement_unbounded`.

Proved by `crates/ordo-host/tests/refine_door.rs` (every case of the shared
parity table through the door) and `refine_paging.rs`.

### 7.1 Effective cost class per request [settled]

A registered cost class describes the op **as written**. A refinement changes
the work the op actually does: a `where` on a sealed field turns a pushed-down
`sql` op into a per-row host decrypt. So the registered class is not the served
class, and nothing recomputed it.

**Post-take refinement (§7) changes what this gate is for.** Because the filter runs after
the page window, the *candidate count* for a row-shaped op is the page, not the
entity: refinement raises the **venue** (`sql` → host-side decrypt of a page)
without raising **cardinality**. Where the gate still bites is the shape that
has no page window: an op whose result is statically one row (a `reduce`, and
`std.E.count` under the list-manifest pairing) refines *before* the reduce,
over the op's whole result set. That is the case where a caller-supplied
condition can genuinely change the order of work, and it is the case the
rejection below is protecting.

**The binder computes an effective cost class per request**, as the join of the
registered class and every class implied by the refinement:

```
effective = registered ⊔ class_of(refinement fields, refinement ops)
```

- **Sealed refinement fields raise the class**, to `blind-index` for equality
  that rides the index, to host-side for anything else.
- **If the effective class is unbounded-host and the op was not registered with
  the `unbounded` acknowledgment, the request is rejected**:
  `refinement_unbounded` (400), naming the field. The `unbounded` marker exists
  so a human ships O(space) work on purpose; a refinement envelope cannot
  acknowledge on anyone's behalf, and an op that quietly became unbounded at
  request time is precisely the silent breach the marker was meant to prevent.
- **The effective class is what gets served, logged, and traced.** It rides
  the `ordo.cost_class` span attribute next to the registered one
  (`implementation.md`, Observability), so the gap between the two is
  measurable.
  The span attribute is not built. The effective class **is** recorded:
  the metered executor counts every execution by the class it ran at, per
  op and per tenant, beside its duration and rows returned
  (`crates/ordo-host/src/observed_cost.rs`, ruled 2026-10-23). It is read
  on `/metrics` and through `std.deployment.observedCost`
  (`deploying.md` §2d).
- **This is a request-time gate and it is not sufficient alone.** The
  declaration-time half belongs to the field declaration and to std's derivation;
  see `std-library.md` §2.1. It covers whether a sealed field is refinable at
  all, whether sealed refinement is equality-only, and the rule that
  **aggregate-only ops accept no sealed refinement** (*if you may not read the
  rows, you may not measure them*). Implementing only one half leaves the hole open in
  one direction: request-time gating alone still lets a bounded op serve as a
  per-row oracle, and declaration-time gating alone still lets an opted-in
  field blow the cost class.

## 8. Pagination tokens [settled]

Opaque, stateless keyset cursor:

```
payload = JSON { "v": "<op version hash>", "k": [ keyset values… ] }
mac     = HMAC(page-key, "ordo.page.v1" ‖ space ‖ op ‖ principal ‖ payload)
token   = base64url_nopad(payload) "." base64url_nopad(mac)
```

- `k` is the last row's sort-key values plus the implicit id tiebreak. The
  executor owns its meaning; the transport layer treats it as opaque freight.
- No server-side cursor state: any node with the deployment's page key resumes
  any token.
- **The token is authenticated, and the MAC covers its whole context:**
  space, op, and principal, each length-prefixed. The payload is not parsed,
  or trusted, until the MAC verifies. Four things fall out: a caller cannot
  mint a cursor and seek arbitrarily in sort space (which would turn any paged
  op into a binary-search oracle over the sort key); a token that leaks through
  a proxy log, browser history or `Referer` header is not a bearer credential,
  because another principal presenting it fails; a token minted for op A is not
  accepted by op B even when their version hashes coincide; and **a principal
  who belongs to two spaces cannot carry a cursor from one into the other**
  (C4-012). Without the space in the MAC
  every other field still matches, and the executor seeks to the *other*
  space's keyset boundary. The space bound into the token is the approved
  space, the same one the access check ran against.
- Every authentication failure (forged, tampered, wrong principal, wrong op,
  wrong space) answers the same `invalid_cursor`, so the failure carries no
  information about which check it failed.
- Decode checks `v` against the current op's version hash **after** the MAC.
  A mismatch is `version_skew` (409), because keyset columns may no longer mean
  the same thing. The client restarts from page one; the token is not migrated.
  This check is kept separate on purpose: a stale token from an honest client must not
  be confused with a forgery.
- Rotating the page key invalidates outstanding cursors, costing a client one
  restart from page one.

**Reservation, not lookup [settled].** The cache is claimed *before* the body
runs, never after: `claim` is one atomic step returning reserved / hit /
in-flight / mismatch, and there is no non-reserving lookup. Without this, two
concurrent identical requests both miss and both execute, which defeats the
only thing an idempotency key is for. The loser waits briefly for the winner's
answer and then replays it; past its budget it receives `idempotency_in_flight`
rather than executing a second copy. A crashed winner's reservation ages out
(60s) so a key can never be held forever.

**A creating op's key binding is permanent [settled].** For ops whose mode can
create, the key→payload fingerprint is stored *separately from the response*
and does not lapse with it. So the same key with an edited payload is
`idempotency_mismatch` **forever**, not only inside the response TTL.
Otherwise a replay after expiry would silently converge a changed payload onto
an existing row. Non-creating ops keep the ordinary TTL-bound binding.

**The Postgres contract** (the in-memory implementation is a test double):
reservation is `INSERT … ON CONFLICT DO NOTHING RETURNING id`; stale takeover
is `ON CONFLICT … DO UPDATE … WHERE state='pending' AND reserved_at < now() -
60s`; the loser waits on `LISTEN`/`NOTIFY`; and the key's completion
record commits **in the same transaction as the domain write and the chain
append**, or the cache can promise an outcome the ledger never took.
[Narrowed 2026-09-06: the completion record does ride that transaction (the
`$replay` journal, §9), and the sealed response body deliberately does not.]

## 9. Idempotency [settled]

`Idempotency-Key` header on writes; the SDK retry loop generates and reuses
it (plumbing, not data identity, per the definition language). Behavior:

- **Cache key is `(space, principal, op, header value)`**: all four, always.
  Keying on `(op, header value)` alone made the cache a **cross-principal read
  channel**: a caller who guesses or observes another principal's
  `Idempotency-Key` receives that principal's stored response verbatim, with
  the executor and every row guard bypassed, because a cache hit never runs
  them. The principal and space dimensions are what make a hit *the same
  caller's own* prior result. (The space dimension is the same one the access
  check needs; see §12.)
- **Plus a stored request fingerprint**: a hash of the bound params and the
  validated `$refine`. A hit whose fingerprint differs is
  `idempotency_mismatch` (409), **never** a replay. Without it, a retry loop
  that mutates its payload gets a plausible wrong answer, and a pre-registered
  key silently swallows a victim's real write and returns a stale 200.
- Hit ⇒ the stored response returns verbatim, executor untouched,
  `X-Ordo-Idempotent-Replay: true` set. Same caller, same key, same request,
  same result, no re-execution.
- Stored: successful responses and executor conflict outcomes
  (`duplicate` / `cas_miss` / `conflicting_identity`, because they are the op's
  result). Validation errors and `internal` are never cached, so a retry
  re-attempts those.
- **TTL and size bound are mandatory**: 24h default TTL, bounded capacity with
  eviction. An unbounded cache is a growing plaintext mirror of every write
  response.
- **Entries are sealed at rest under the space key.** A replay cache holds
  written rows post-defaults, including sealed values, so it lives inside the
  encryption boundary like every other copy.
- Absent header ⇒ execute normally (transport retries then have no
  protection; the SDK always sends one).
- **Postgres-backed, not in-memory** [settled; supersedes the earlier "shared
  store later"]. The `ReplayCache` trait stays; the in-memory implementation is
  a **test double**. On more than one node an in-memory cache silently
  **re-executes** a retry routed to a different replica: that is a correctness
  defect, not a missing feature, and the replay table is a precondition of
  multi-node rather than an optimization for it (`implementation.md`,
  deployment topology). The write and its replay record commit in the **same
  transaction**, which is also what makes the SERIALIZABLE group-retry policy
  safe.
- **What rides the transaction is the record, not the body** [settled
  2026-09-06; `docs/plan-idempotency.md` §2, and a narrowing of the two
  sentences above]. The `$replay` journal (key, fingerprint, fencing
  generation, done) is claimed and completed inside the write's own
  transaction, so commit publishes the effect and the record of it or
  neither, and no retry can re-execute a committed write. The **sealed
  response body** is stored after commit, outside it, and stays there by
  ruling, for two costs the fold-in cannot avoid. Sealing takes a
  key-provider release, and in the transaction it would have to *succeed
  before the effect could commit*, so a custody outage would stop every
  write, where today it loses only a replay convenience. And the append
  lock is transaction-scoped: every statement placed after the chain
  append runs with it already held, so an unbounded `INSERT` (a `whole`
  read's response is the object graph) landing there widens the write
  path's serialization point, which `implementation.md` names as the
  failure mode to catch in review. A crash in that window therefore loses
  the verbatim body and nothing else, and the retry is told exactly that:
  `idempotency_applied` (409, terminal), never a second execution.

## 10. Version skew [settled]

Per `tooling-and-embedding-plan.md` §B5, generated code pins the version hash
it was built from and sends it as `X-Ordo-Version`. **The pinned hash is
per-op, not registry-wide** [settled; corrects the earlier reading]: the
server compares per-op, so a registry-wide pin marks **every** call stale after
any unrelated deploy, and a soft signal that fires constantly is a signal
clients learn to ignore. Generated code therefore pins the hash of *the op it
is calling*, the same granularity the server compares at and the same
granularity grants pin at (`security-logging-design.md` §5). The server:

- always answers with `X-Ordo-Op-Version` (the served hash);
- adds `X-Ordo-Version-Skew: stale` when the pinned hash mismatches.
  This is a **soft signal** (the call still executes; bindings surface a typed
  warning). Richer drift classification (compatible / narrowed / breaking)
  needs registry history and lands with the registry service. What *is*
  decidable without history is the grant-pinning comparison, an **ordered
  field-set** comparison per op: subset = narrowing, superset or disjoint =
  dead. See `security-logging-design.md` §5.
- The **hard** rejection is page tokens only (§8), the one place stale
  version assumptions can silently corrupt meaning.

## 11. Hot swap [settled]

Per `implementation.md`: compiled ops are data; nodes swap registries
atomically on LISTEN/NOTIFY. In this crate:

- `RegistryHandle` holds `RwLock<Arc<dyn OpRegistry>>`; `swap()` replaces the
  inner `Arc`, `current()` clones it.
- A request clones its `OpSpec` at dispatch, so in-flight requests finish on
  the version they looked up; the next request sees the new registry. No
  drain, no restart. Rollback is the same swap, backwards.
- Live sessions are **not** torn down on swap: the subscription survives, and
  the client's next refetch executes under the new version (skew surfaces
  through §10 headers if the client pinned a hash).

## 12. Trait boundary [lean]

The crate depends on no sibling crate; it defines narrow traits the platform
wires in later, one implementation each today, in-memory:

| Trait | Duty | Fulfilled later by |
|---|---|---|
| `OpRegistry` | `get(name) -> Option<OpSpec>` | artifact registry (Postgres-backed cache) |
| `OpExecutor` | `execute(op, space, bound, refine, page)`: run a prepared plan **against the door's bound space**, with bound params / refinement / cursor | engine executor |
| `AccessChecker` | `check(token, space, op) -> Allow \| Deny` | identity service |
| `InvalidationBus` | `subscribe(op) -> broadcast::Receiver<InvalidationEvent>` | command-log listener |
| `ReplayCache` | idempotency store (§9), keyed `(space, principal, op, key)` | **Postgres replay table**; the in-memory impl is a test double |

`OpSpec` carries exactly what the transport needs from a compiled artifact:
name, version hash, kind (read/write), param specs (type, required, bounds),
output-field manifest, live capability, refinement tier, mode label, registered
cost class, and the guard set the executor must inline.

**The space dimension is a dependency, not a detail** [settled; the binding is
now pinned in §2]. `AccessChecker::check` takes a space
because `ordo-identity`'s engine does, because the replay key (§9) needs one,
and because a router that cannot bind a space cannot express the multi-space
model at all: a member of space A would otherwise reach a registered op against
space B's data.

**`OpExecutor::execute` takes the space too, as a named parameter of its own**
[settled]. The door used to bind a space, hand it to the access check, use it
for the replay key, and then call an executor that never received it, so the
executor had to obtain a space from somewhere, and all three candidates are
unsafe: from `BoundParams` (caller-controlled, and never cross-checked against
what the door approved, so a caller can send one space to satisfy the check and another to
steer execution), from the token (which collapses the multi-space product), or
from ambient configuration (at which point isolation is decorative). Hence:

- The space is a **named trait parameter**, never a member of `BoundParams`, so
  no op parameter can shadow it.
- The executor **asserts** that the space it plans against is the space the door
  approved. Disagreement is `internal` (500), a bug raised loudly, never a silent
  choice between two spaces.
- The same value is the BYO-DB resolver key, the chain-append lock key, and the
  replay key (`implementation.md`), so a second source of truth for it is a
  second source of truth for all three.
- Coverage obligation: cross-space denial tests are the missing ones. A harness
  `AccessChecker` that denies `space != SPACE` proves nothing until a test
  exercises it, and a space-disagreement test is the only thing that can fail if
  this regresses.
