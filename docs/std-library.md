# Ordo Standard Library (`std.`)

Companion to `definition-language.md` (shapes, queries, writes, access model),
`implementation.md` (capability catalog, shape verification),
`api-engine-design.md` (route model, refinement envelope),
`tooling-and-embedding-plan.md` (§B2 refinement, §B3 terminal verbs), and
`domain-packages.md` (namespacing). Specifies the **std library**: the
platform-derived operation set compiled by default at `ordo apply` under the
reserved `std.` namespace. It is a **pure function of the schema**, so every
deployment carries a predictable baseline surface before any authored op
exists.

Status markers as elsewhere: **[settled]** / **[lean]** / **[open]** /
**[proposed]** (a gap filled here, not yet ruled on).

---

## 0. What is built, as of 2026-09-08

This document specifies twelve verbs per entity plus `std.space.*`. **Seven
verbs derive today**, and **four `std.space.*` ops are built**:
`callersOf`, `export`, `delete`, `attachBucket`. The rest do not, and the
reasons differ enough that "unbuilt" is the wrong single word for them. Checklist 9k/9k′/8r and the
`corpus-rulings.md` entry of the same date carry the argument; this is the
index.

| Surface | State | Why |
|---|---|---|
| `get` `count` `create` `put` `update` `delete` `deleteSubtree` | **derived** | `ordo_schema::std_ops`, injected by `Registry::extend_from` and gated by the same `check_registration` as every authored op |
| `list` | **blocked on a language ruling** | §5.7 calls keyset paging a row bound; the implemented prover does not, and neither `unbounded` (rule 7 forbids it) nor a `take` (the engine pages *inside* the take, so it caps rather than windows) is a legal substitute. §9k′ |
| `whole` | **not derivable** | no `OpBody` holds it, and its cap is `[open]` with two incompatible result envelopes (§2 vs §5.7) |
| `under`, `rollup` | **not owed** | `[lean]`; and `rollup`'s derivation condition names a *declared relationship cap* the language has no syntax for |
| `history` | **not derivable** | platform-derived by §2.3's own account, and the language has no log source |
| `std.space.callersOf` | **built, on the platform-op path** | `ordo_host::platform_ops`, a hand-written `OpSpec` registered into the one registry the door dispatches on, executed through the one `OpExecutor` trait, so the access check, `$space` bind, param binder, refusal envelope and scoped catalog are the same ones every compiled op gets. Manager-and-owner, step-up gated (which is what makes it undelegable), argument scoped to the asker's own catalog. §8r |
| `std.space.export`, `std.space.delete` | **built** | `ordo_host::exit_ops`, owner-only and step-up gated. Both are **writes** and neither appends a command group: an export mutates no row, and a deletion destroys the chain it would append to. They record a `CustodyAction` on the security-event stream instead. That is the half of the audit spine that records *decisions about a space* (the other half records *changes to its rows*), and it is the half that can outlive the space. `plan-export.md` §5.1 |
| `std.space.attachBucket` | **built** | `ordo_host::bucket_ops`, `plan-blob.md` B5. Owner-only (a manager administers who may do what; deciding where a space's files live decides what leaving the platform means), step-up gated, and keyed on the target's identity so a retry is a no-op that writes no custody event. Its credential is a **label**, never a key, because a secret in an op parameter is a secret in the plaintext command log. `std.space.detachBucket` (2026-10-05) runs it back: owner-only, step-up gated, files already written stay in the bucket they were written to (2026-10-06). Every bring-your-own option is in `howto-byo-custody.md` |
| `std.space.pause`, `std.space.resume`, `std.deployment.pauseTenant`, `std.deployment.resumeTenant` | **built** (27ar) | `ordo_host::pause`. Pausing is a deployment operation: the two `std.deployment.*` ops are the deployment owner's, in its own space. The two `std.space.*` ops reach no tenant role until the app developer names roles (`ORDO_TENANT_PAUSE_ROLES`, `ORDO_TENANT_RESUME_ROLES`). A tenant's resume never lifts the deployment's pause. See `howto-pause.md` |
| `std.deployment.tenants` | **built** (G4, console step 5) | `ordo_host::fleet`. A read for the deployment owner, in its own space: each tenant's id, name, status, when made, the database it is on, its member count, its last rolled-up usage period, this node's quarantine reason and the caller's role there. Never its members or shares (ask 5, 2026-10-23). `ordo fleet` prints the same reading by target. `includeReaped: true` adds reaped tenants |
| `std.deployment.settings` | **built** (G7, console step 6) | `ordo_host::settings`. A read for the deployment owner, in its own space: every setting in the registry and every `ORDO_*` variable the process has, each with its effective value, default, range and source (`default`, `environment`, `file`, `stored`). A secret only as set or unset. A value a platform op stored (guest, self-serve) wins and names its op; the app developer's stored layers are rows of kind `layer`. Answers for the node that served it |
| `std.deployment.quotas`, `std.deployment.setQuota` | **built** (G9, console step 7) | `ordo_host::quotas`. The deployment owner's, in its own space. `quotas($space?)` reads every quota with what the space used this period; `setQuota($space, $quantity, $limit, $grace, $periodSecs)` is `ordo quota set` through the door, on the same `set_limit`, and needs a touch. Each change records `quota-set` (G12). A quota gates nothing until `ORDO_USAGE_LEDGER=true`, and the answer says so |
| `std.deployment.securityEvents` | **built** (G8, console step 9) | `ordo_host::security_record`. A read for the deployment owner, in its own space: the deployment's own security stream, newest first (`$before`) or oldest first (`$from`), at most 200 a page, filtered by `$kind`, `$tenant` (the space an entry is about), `$since` and `$until`. Each row is the recorded act, never a secret. A tenant's own record stays `GET /security/events?space=` |
| the rest of `std.space.*` | **not started; the mechanism is no longer the blocker** | the path above is the door 9k declined to build. Adding one is a spec, a handler arm and a profile grant. ~~the writes additionally need a command group and a chain append, which `callersOf` (a read) did not have to solve~~. **Three writes have now shipped without one**, because none of them writes an entity row; a write that *does* is still owed that machinery, and `plan-blob.md` §8.1 is the open ruling on what it should look like. `engine-plan.md` still schedules the remainder in no phase. §8r |

Two rulings taken while deriving, both recorded in `corpus-rulings.md`
because this document said both things: `update` drops sealed fields from
its **params** as well as its braces (§2.4's table contradicts itself one
row apart), and `create` is **skipped** on an entity with two unique keys
(§2 says "always", DL §6 rule 8 refuses the statement).

One exclusion this document does not mention and should: **the platform
`members` entity derives nothing.** It is unguarded, so a derived read is a
whole-space principal directory and a derived write is an unattributed path
to minting and re-roling principals. That is `std.space.*`'s job, done as
attributed commands.

---

## 1. Rationale: value lives at the call boundary [settled]

Inside `.ordo`, std adds nothing: the entity source is already the accessor
(`transactions(date since 30d)[…]` is shorter than any std call could be).
std has no role as an in-language convenience layer, and its ops are rarely worth
composing over when the entity is available.

The value is at the **call boundary**. Deploy a shape, get an API: typed SDK
functions, agent-callable catalog entries, generated-UI table/detail/form
surfaces all exist the moment the entity does, with zero authored ops.

Ad-hoc reads are served by **one artifact plus structured conditions**:
`std.E.list` plus the Tier-0 refinement
envelope (§B2: a condition tree over output fields, narrowing-only, never
query text). One registered op, one
manifest, one cost story. The variation rides in validated data, so no
combinatorial method namespace grows.

**One caveat applies, because refinement is post-take** (`api-engine-design.md`
§7): a refinement narrows *results*, not *work*. Iterating a refined
`std.E.list` to exhaustion yields exactly the matching set, but pages arrive
short or empty and a selective filter still walks the base set. So std plus
refinement covers **browsing and shaping**. Indexed
search over a large entity remains an authored op whose
parameter compiles into the plan. Saying so here is cheaper than letting an
author discover it at scale.

---

## 2. Derivation table [settled]

For every entity `E` in every deployed domain, `ordo apply` derives and
registers:

| Op | Derived when | Kind → method | Semantics |
|---|---|---|---|
| `std.E.get($id)` | always | read → GET | the declared row, **the open skeleton by default when `E` has sealed fields**, §2.4, plus implicit `id`, `rev`, and `depth` on trees. Absent id ⇒ typed `not_found` (§2.5) |
| `std.E.list` | always | read → GET | keyset-paged listing of the same row shape as `get`; default sort `-id` (§10); Tier-0 refinable; live-capable |
| `std.E.count` | always | read → GET | `reduce { n: count }`; refinable (conditions validate against the **list** manifest, §2.1) |
| `std.E.whole($id)` | always | read → GET | the anchored composition per the language's `whole`: owned closure down, signpost up. **Every collection inside the result is paged, not truncated.** Each carries `{ items, count, next }`: the first *n* rows (default **100**, policy-tunable), the **total count** so a caller always knows what it did not receive, and a keyset token to continue that one collection. Depth is bounded by the ownership structure itself; this bounds breadth, which is what makes the op registrable (§5.7). Absent id ⇒ `not_found` |
| `std.E.under($id)` | tree entities | read → GET | subtree rows (`E(under $id)`), full row + `depth`; paged; default sort `depth, id` [lean] |
| `std.E.create(…)` | always | write → POST | mode `creates`; params = every required field (required params) + every defaulted/nullable field (defaulted params mirroring the field default). Insert-complete by construction |
| `std.E.put(…)` | exactly one unique key declared | write → POST | mode `creates-or-converges` on that key; params cover the key and every declared field. Insert-complete |
| `std.E.update(…)` | always | write → POST | full-row CAS converge (the wrinkle, §2.2) |
| `std.E.delete($id)` | always | write → DELETE | mode addressed delete; owned rows go with it (owner ruling 2026-09-29, `owned-delete`), plain refs restrict (refuse-with-referrers), and a tree node with children refuses |
| `std.E.deleteSubtree($id)` | tree entities | write → DELETE | the explicit `del … subtree` form as its own op, separately named and separately grantable, never a flag on delete |
| `std.E.history($id)` | always | read → GET | per-row command-log slice, platform-derived (§2.3) |
| `std.E.rollup($id)` | owning entities **with a provably bounded closure**, §4 | read → GET | owned-subtree numeric aggregation [lean] |

Notes:

- **Methods follow derived kind** exactly as for authored ops
  (api-engine-design §1): std reads are `GET`, `create`/`put`/`update` are
  `POST`, `delete`/`deleteSubtree` are `DELETE`. Generated SDKs surface the
  same terminal-verb split (§B3): std reads end in `.fetch()`/`.live()`/
  `.pages()`, std writes in `.commit()`, so they look the same as authored
  ops at the call site.
- **`put` derives only when exactly one unique key is declared** (composite
  counts as one). With two or more unique keys, identity-precedence rule 8
  makes any covering statement ambiguous, so std refuses to pick a key;
  converge-or-create over multi-key entities is an authored op that chooses.
- **Write results are protocol-standard**: addressed writes return the
  written row (post-defaults, post-`rev`); deletes return affected-id
  descriptors. `duplicate` / `cas_miss` / `conflicting_identity` pass
  through typed, as everywhere.
- **`not_found`** joins the executor pass-through set (404): `get`, `whole`,
  `under`, `history`, `rollup` on an absent id reject typed, consistent
  with no-resurrection (ids are server-minted; absence means deleted). It
  carries a second meaning on guarded entities, and the two are deliberately
  indistinguishable (§2.5).
- **Derived entities are entities**: `acl`-derived link entities (e.g.
  `projectsAccess`) receive the same std derivation. No privilege gap:
  the acl derivation's inherited guards gate writes on those rows at the
  **highest declared level**, the same pin that gates the acl admin ops,
  so `std.projectsAccess.create` cannot undercut `grantProjectsAccess`.
- All std reads are **live-capable** (named ⇒ callable ⇒ subscribable;
  liveness is a call-time choice, as for every op).

### 2.1 Refinement pairing [settled]

`std.E.list` is refinable like every read (Tier 0, ruled 2026-10-05, with
no op-level word and no off switch). Its output manifest is the full row, so
`$refine.where`/`pick` over any declared field is the findBy replacement of
§1. `std.E.get`, `whole`, and `under` are refinable the same way (`pick`
trims payloads; conditions are legal, rarely useful).

`std.E.count` gets one std-specific pairing: its own output is `{ n }`,
which would make refinement useless under the output-fields-only rule. So
**count's refinement conditions validate against `std.E.list`'s manifest**
and apply before the reduce. `list` and `count` under the same `$refine`
always describe the same set; bulk-confirm UIs are exactly this pair.
**Built** (2026-10-05, `refinement-default` ask 5; the door's
`count_fields` in `crates/ordo-host/src/lib.rs`, proved by
`crates/ordo-conformance/tests/refine_paging.rs`). `ordo check` prints a
count's refinable fields as its list's.

Two exactness notes. The *sets* agree, and so do the pages: conditions on
open and blind-index fields compile into the plan (ruled 2026-10-05,
`refinement-default` ask 1), so `list` pages come back full under a
refinement. A bulk-confirm UI still reads `n` from `count` and takes the
absence of `next` as the only end of a listing. And `count` is the one std
shape with **no page window**, so its refinement is also the one that can
genuinely raise the effective cost class, which is why rule 4 below is not
theoretical for this op. `docs/howto-cost.md` is the guide for app
developers.

Refinement conditions on **sealed** fields follow their own rules. The
earlier phrasing, "they cost what they always cost and are refusable per
policy", was wrong twice: it priced cost while ignoring **leakage**, and it
deferred to a policy whose default was stated nowhere. The rules, in the order
they apply:

1. **Not refinable by default.** A sealed field accepts no refinement
   condition unless its declaration says so (the language marks it at the
   field, alongside `sealed indexed`; see `definition-language.md`). Opt-in
   means the capability lands in the **manifest** and therefore on the consent
   page, like every other disclosure in this design.
2. **Where enabled, equality only, and the operator set is closed [settled].**
   Sealed refinement rides the blind index, which answers one question: does
   this value hash to that? So exactly two operators are legal, `eq` and `in`
   (a set of equalities the index serves directly). Everything else is
   `invalid_refinement`: `gt`/`lt`/`ge`/`le` because caller-controlled ordering
   over decrypted values is the oracle; **`ne`** because it is an ordering-free
   inequality that still partitions the space one probe at a time; and
   **`is_null`** because presence is itself a disclosure (see the null-sentinel
   note in `implementation.md`: the sentinel is the modal blind-index value,
   so nullness is already the cheapest thing to learn about a sealed field).
   Enumerated because "equality only" was previously read as ruling on four
   operators and silent on three.
3. **Aggregate-only ops accept no sealed refinement at all.**
   `std.E.count` inherits `list`'s manifest for field *names* (above), but a
   **sealed** condition under `count` is `invalid_refinement`. The rule in one
   line: **if you may not read the rows, you may not measure them.** A viewer
   granted only `std.E.count` (one number, apparently harmless) otherwise
   binary-searches every sealed numeric field in the space
   (`count(salary gt 100000)`, `count(salary gt 150000)`, …) and recovers a
   value to the dollar in twenty calls, with no read grant on it. Guards filter
   rows; they do not blunt an aggregate over rows the caller *may* see, and
   `count` is designed to pair with `list`.
4. **The effective cost class is recomputed per request**, and a sealed
   refinement that pushes an op past its registered class is
   `refinement_unbounded` (`api-engine-design.md` §7.1). Rules 1–3 are a
   *declaration-time* gate; rule 4 is a *request-time* gate. Both are required:
   without 4 the cost lie survives for opt-in fields, and without 1–3 the
   oracle survives for bounded ops.
5. **A sealed condition the host would answer is refused at the door**,
   naming the field, as `sealed_unsupported` (ruled 2026-10-05,
   `refinement-default` ask 3). That is a sealed refinable field with no blind
   index, on a bounded op or one that says `unbounded`. It lands once
   host-side filtering is built.

### 2.4 Sealed fields and std derivation [settled]

**Entities containing sealed fields derive their std read surface over the
OPEN SKELETON ONLY.** Sealed-inclusive std exists **only where the shape says
so**, via an explicit marker on the entity (the language owns the spelling;
see `definition-language.md`'s shape syntax). This document depends on the
marker's *behaviour*, not its exact word).

Why the default flipped: every other disclosure in this design is
**structural**: guards inline, manifests derive, scope expansion kills grants,
and consent pages compute. Full-row std was the one exception: the platform
derived, registered, shape-verified, and catalogued a **full-disclosure op for
every entity in every domain**, then relied on runtime discipline (a template
convention, a manager's attention) to keep it ungranted. One package template
granting `std.E.*` to `viewer`, one owner clicking enable, and every viewer
reads every sealed field of every entity the package defines. The consent page
did say so, buried in a list of forty ops. Sealed disclosure is now an
**authored act**, greppable in the schema and visible in the deploy diff.

Exactly what changes:

| Surface | Default (sealed present, no marker) | With the marker |
|---|---|---|
| `std.E.get` / `list` / `whole` / `under` output manifest | open skeleton; sealed fields **absent from the manifest**, rendered as sealed markers by generated UI (`ui-gen.md` §8) exactly as an unheld key release renders today | full row |
| `std.E.update` **returned row** | open skeleton | full row |
| `std.E.create` / `put` / `update` **write params** | **unchanged: sealed fields stay writable.** Writing a value is not disclosing it, and dropping them would break insert-completeness on entities with required sealed fields | unchanged |
| `std.E.update` converge braces | the **open skeleton only**; sealed fields are *not touched* and not nulled. Still a static brace list (the skeleton is known at compile time), so §2.2's "no dynamic write shape" rule is intact | full row |
| `$refine` over sealed fields | governed by §2.1 independently: the marker does not imply refinability, and refinability does not imply the marker | same |
| `std.E.history` images | already `SealedRef`, unchanged (`security-logging-design.md` §7) | unchanged |

- **The cost is real and worth naming**: generated UI and the data browser show
  sealed markers until someone opts in, and the two-pages-to-an-app demo gets
  one line longer for entities with sealed fields.
- **`std.E.rollup` is unaffected.** It was already open-numeric-only (§4), for
  the cost reason and not the disclosure reason. The two rules now agree.
- **Cross-reference discipline**: if the language's marker is renamed, this
  section's *behaviour* is the contract and the spelling follows the spec.

### 2.5 `not_found` on a guarded entity [settled]

`not_found` means **either** "no such row" **or** "a row you may not read",
and on a guarded entity the two are **deliberately indistinguishable**.
Addressed std ops (`get`, `whole`, `under`, `history`, `rollup`, `update`,
`delete`, `deleteSubtree`) answer `not_found` for both. A caller who may read
the row but may not write it gets the informative `access_guard` instead,
because they already know the row exists.

- **The exception is unguarded entities**, where there is no read view to be
  outside of and `not_found` means exactly "absent".
- Full rule, wire codes, and the timing conformance assertion:
  `api-engine-design.md` §4.1. std states it here because std is where most
  addressed-by-id traffic lands, and because `std.E.get($id)` is the single
  op most likely to be used as an existence oracle.

### 2.2 `std.E.update`: the wrinkle [settled]

Stated honestly: **a generic field-subset update is unexpressible.** "Update
whichever fields the caller sent" needs a brace list decided at runtime,
and dynamic brace lists violate static statements: mode derivation, scope
manifests, and insert-completeness all read the braces at compile time.
There is no std op with a dynamic write shape because there is no *op* with
a dynamic write shape.

So `std.E.update` is a **full-row converge**: params = `$id`, `$rev`
(**mandatory**, because it is CAS and no unversioned form exists), plus every declared
field; the body lists every field, every time. Fat but correct:

- Lost updates are structurally impossible: a stale writer gets `cas_miss`
  and refetches; there is no silent last-writer partial merge.
- The SDK makes it ergonomic: the generated flow is **read → mutate →
  commit**. `get` returns a typed record carrying `rev`, the caller
  mutates fields in their own language, `.commit()` ships the whole row
  back. The wire is fat; the call site is three lines.
- **Field-subset updates are authored ops by design.** An op that touches
  two fields says so in its name, its scope manifest and its consent entry.
  That precision is deliberate.

### 2.3 `std.E.history($id)` [settled mechanism, lean field list]

The per-row command-log slice: every command that ever touched row `$id`,
newest-first, keyset-paged on log position. **Platform-derived, not
language-compiled**: the language has no log-source construct; std is
where the platform's own data becomes callable.

- Output [lean]: `command` (id), `group` (command-group id), `at`, `actor`
  (attributed principal, `on behalf of` preserved), `kind`
  (create/converge/delete), and nullable `before`/`after` row-shaped image
  records (static nesting, like `whole`).
- Cost: envelope columns are plaintext skeleton ⇒ `sql`; when `E` has
  sealed fields the image decryption is per-page host work, classified
  `bounded-host` with the page window as its proven bound. Platform-derived ops
  have this latitude because the platform owns the executor.
- **Images outlive their keys only inside the undo horizon.** Sealed images are
  named by `SealedRef` and open only while the epoch key that sealed them
  exists; that key is retained for at least the space's `undoHorizon`
  (`security-logging-design.md` §7). Beyond it, an image whose epoch key was
  destroyed renders as an explicit **unresolvable** marker, never a silent
  null, and never an empty value a reader could mistake for "the field was
  blank then".
- Live-capable: any commit touching `E` invalidates.
- Composable like any read, and worth composing over (§7): audit panes are one authored op sourcing
  `std.E.history`.

---

## 3. `std.space.*`: introspection ops [settled]

The access model promises that "who can call X" is one exhaustive query.
std ships that query. Access is *answered by* registered ops and those ops
are *access-checked themselves*: a viewer cannot enumerate grants, but a
manager can (§5 exposure defaults):

| Op | Answers |
|---|---|
| `std.space.create(…)` | **`scope deployment`**. Binds no space; creates one atomically (see `bootstrap.md`). The only op that may run before a space exists |
| `std.space.reap(space)` | **`scope deployment`**. Removes a space stuck `provisioning`/`failed`; distinct from `space.delete`, which ends a live space and is owner-only |
| `std.space.members()` | membership rows: member, role, joined [lean fields] |
| `std.space.profile()` | the live role→ops mapping plus per-member overrides (runtime state, rendered) |
| `std.space.grants()` | capability tokens: principal, op allowlist, pinned scope hashes, granted-by/when |
| `std.space.groups()` | the space's groups: group id, name, member count, created-by/when |
| `std.space.groupMembers($group: id)` | the group's membership rows: principal, added-by, added-at; paged |
| `std.space.callersOf($op: text)` | every principal currently able to call `$op`, with the *via* (role / group / override / token). Group-reached principals appear individually, naming the group as the via. **Built** (§8r): every via is reported, not the first, because the question behind it is *"how do I stop them"* and an answer naming one of three grants has a manager revoke it and find the caller still there |
| `std.space.callableBy($principal: id)` | every op the principal may call, with the via (including which group supplied it) |
| `std.catalog()` | the caller-scoped capability catalog as an op. It is the same runtime-verified artifact as the well-known endpoint (implementation.md), now callable and composable |

- All are live-capable: admin panes subscribe and update on membership,
  profile, and grant changes. Permission history is already commands, so
  invalidation is the ordinary machinery.
- `std.catalog()` is **self-scoped**: it shows the caller only what the
  caller may call, and it republishes atomically with every hot swap (a
  subscribed agent sees the surface change). It is safe to grant to any
  principal *that already holds the ops it lists*, which is the honest form of
  the claim: it discloses no op a caller could not already call. It is **not**
  content-free: descriptors name entities, fields, and types, which is
  vocabulary, and vocabulary is metadata worth guarding at a guest boundary.
  Granting it universally is safe, but that does not make catalog metadata
  safe in every case (see the guest-window
  discussion in `definition-language.md` §8 inter-space sharing).

### 3.1 Groups: the ops, and what a group-member row means [proposed]

Groups are part of the access model (`security-logging-design.md` §6) and are
rendered by a deployment owner surface (`dashboard-plan.md` §5c), but nothing had
specified the operations that create or change one. That left the group editor
with no data path and the dashboard's "no pane has a private data path" rule
unsatisfiable. std ships them, because a group is space-owned runtime state and
`std.space.*` is where space runtime state becomes callable:

| Op | Kind | Semantics |
|---|---|---|
| `std.space.createGroup($name: text)` | write → POST | mints a group in the bound space; `$name` is unique per space (`duplicate` otherwise) |
| `std.space.renameGroup($group: id, $name: text)` | write → POST | CAS on the group's `rev`, like any converge |
| `std.space.deleteGroup($group: id)` | write → DELETE | refuses while the group holds any grant in the space's profile, because a delete that silently revoked access would be the quietest possible authorization change. Refuses with `group_in_use` while a profile line names it (remove it with `setOpAccess` or `resetOpAccess`) or an access grant on a record does. Otherwise one command group removes the group's links and its row |
| `std.space.addToGroup($group: id, $principal: id)` | write → POST | idempotent: adding an existing member converges, never `duplicate` |
| `std.space.removeFromGroup($group: id, $principal: id)` | write → DELETE | addressed by the pair; absent membership answers `not_found` |

Row semantics, since "a group" is really two row shapes:

- **A group is a row owned by the space**; a **group-member row** is
  `(group, principal, addedBy, addedAt)` and is likewise space-owned. Both are
  ordinary entities: they get ids, `rev`, images, and command-log history like
  anything else.
- **Membership is per space and non-transitive.** A group holds principals, not
  groups (`security-logging-design.md` §6, nested groups are out of v1), so
  "who is in this group" is one query and stays one query.
- **Every one of these ops is a command**: attributed, chained, revertible,
  and `?live`-invalidating, so permission history and the admin pane's live
  updates follow from that.
- **They are manager-gated by default**, exactly as `std.space.profile()` and
  `std.space.grants()` are (§5.3). Group administration is access
  administration.
- **Effect timing**: a removal takes effect at the **next access check** and at
  the next guard evaluation; there is no cached group grant with a life of its
  own. A guard's group hop is resolved inside the semi-join at query time
  (§5.1), so a removed principal's next read is already scoped without it.
- **Never mirrored on-device**, like the rest of `std.space.*`
  (`implementation.md`, client storage rules), because a stolen laptop must not yield
  the permission map.
- The **grammar** for declaring groups in `.ordo` (as distinct from administering
  them at runtime) belongs to `definition-language.md` §8; this section owns
  only the callable surface and the row semantics, and it depends on the
  spelling settled there rather than fixing one.

---

## 4. `std.E.rollup($id)` [lean]

Grouped-by-root numeric aggregation: for anchor `$id`, the **sum and
count** of every `money`/`int` field across the entity's **owned closure**
(anchor included). This is the same closure `whole` walks, and only that closure.

- **Never associations.** Aggregating over incoming associations (an
  account's transactions) is a query, not the account's body; same boundary
  as `whole`.
- **The no-double-count proof this section used to carry is withdrawn.**
  It read: *"the one-owner rule guarantees each row has at most one owning
  root, so every row is counted exactly once … structurally impossible."*
  `definition-language.md` §Ownership withdrew the one-owner rule as
  **false**. A link entity has two `owned` refs by construction, so a link
  row can legitimately be counted once per end. It instructed that the
  claim must not survive here either. The claim is removed and not repaired.
  What replaces it is a *condition on derivation* and is not a theorem. The
  condition is the bounded-closure test below. An aggregate that may
  double-count across anchors is a fact about the shape, and the shape
  summary is where it belongs.
- Result: one row; one column pair per numeric field in the closure,
  flat-named `<entity>_<field>_sum` / `<entity>_<field>_count` [lean:
  static shape, SDK-record-friendly].
- Derived only when the entity **owns at least one entity**, the closure
  contains at least one `money`/`int` field, **and the closure is provably
  bounded**: every owned collection in it carries a declared relationship cap
  and every tree in it a depth cap (§5.7). An aggregate cannot be capped the way
  `whole` is, because a sum over a truncated set is a wrong number, so
  the unbounded case is **not derived** and the plan output names the uncapped
  collection.
- **Open numeric fields only.** A sealed aggregate over an unbounded
  closure is `unbounded-host`, and std never registers an unbounded-host
  op (§5). Sealed fields are omitted from the derivation and the shape
  summary says so. Sealed rollups are authored ops, where an author can
  acknowledge `unbounded` deliberately.
- Anything past sum/count (averages, windows, conditions, time buckets)
  is an authored op. rollup is the receipt-total / project-budget baseline.

---

## 5. Rules that keep std honest [settled]

1. **Guards inline into std like into any op.** The compiler inlines
   `access` guards into every op touching the entity, std included, with
   no exception path. On a guarded entity, `std.E.list` is caller-scoped
   *by construction*; std write guards are loud `access_guard`
   preconditions, and addressed ops on rows outside the read view answer
   `not_found` (§2.5). **std never uses `unguarded`**, and there is no derived
   bypass; bypass is always authored, greppable, manifest-flagged.
   **Groups resolve inside the guard, not around it**: where a guard's
   `$caller`-anchored walk reaches membership through a **group** (the access
   model's group layer, in `definition-language.md` §8 and
   `security-logging-design.md` §6 for the check pipeline), std inherits the
   resolution unchanged. std adds no group-specific behaviour and gets no
   group-specific exemption; a group is one more hop in the same semi-join,
   and it appears in std's manifest and consent entry exactly as a direct
   membership walk does.
2. **Registry-present, exposure opt-in.** std ops sit in **no profile by
   default**. Deriving an op and exposing it are different acts: who may
   call what is runtime state, never a compile side effect. A freshly
   enabled domain has its std surface *available*, not *granted*.
3. **Profile templates SHOULD grant std to owner/manager tiers only**
   (plus `std.catalog()` to every role, self-scoped, §3). Viewer- and
   editor-tier access to raw rows is a per-space loosening a manager
   performs deliberately, as an audited profile command. This remains a
   SHOULD, and it is now a *second* line of defence. §2.4 moved the sealed half
   of the problem into the schema, where it is structural.

   **Built**: until packages exist, the template is the deployment setting
   `ORDO_ACCESS_TEMPLATE`, shipped beside the program. A tenant's owner may
   change it for their tenant with `std.space.setOpAccess` (`howto-roles.md`).
4. **Disclosure is total and stated, over the shape std actually derives.**
   `std.E.get`/`list`/`update` manifests are the **whole derived row**, and
   the consent page says exactly that, because scope manifests derive from
   bodies. On an entity with no sealed fields that is the entire entity:
   granting `std.E.*` grants it, and narrower access is an authored op with a
   narrower manifest. On an entity **with** sealed fields the derived row is
   the open skeleton unless the shape opted in (§2.4), so the consent page
   reads "grants every open field of E" or "grants every field of E,
   including sealed", and which one it says is a fact about the schema that
   a reviewer can grep for.
5. **Unguarded entity + `std.E.list` = a whole-table op.** Stated loudly:
   the plan output and the consent page flag it. Nothing is wrong. It is
   the truth of the schema, disclosed at deploy time.
6. **Sealed fields are opt-in, and priced.** Where a shape opts into
   sealed-inclusive std (§2.4), std reads decrypt through the normal
   key-release path (logged) and std writes seal on write. Conditions on
   sealed fields arrive only via refinement and are gated by §2.1: not
   refinable by default, equality-only when enabled, never under an
   aggregate.
7. **std never *registers* an `unbounded-host` op, and no std op ever needs
   the `unbounded` acknowledgment.** The acknowledgment exists so a human ships
   O(space) work on purpose; a derivation cannot acknowledge on anyone's
   behalf, so derivations that would cross the line are trimmed or bounded
   instead. The gate covers **row cardinality as well as venue**
   (`definition-language.md`, cost classes), and an op with no provable row bound
   fails registration unless acknowledged. So every derivation has to carry a
   bound the compiler can prove. Most do trivially: `get`, `update`, `delete`,
   `history` are addressed; `list`, `under` are keyset-paged; `count` is
   reduce-shaped. **Two did not, and they are fixed here so that they do not
   fail registration at first deploy** [proposed]:
   - **`std.E.whole($id)` gets a structural bound.** The owned closure is a
     static set of *entities* (the ownership graph is a tree, so the depth is
     schema-fixed), but the *rows* under it are not: one invoice may own ten
     thousand line items, and nothing in the shape says otherwise. So the
     derivation emits a **per-collection cap**: each owned collection inside
     the composition is taken to a platform-fixed page under the closure's own
     order, and each capped collection carries a `truncated` marker in the
     result. The row bound is then `(entities in the static closure) × cap`,
     multiplied by the tree depth cap where a tree is in the closure: provable,
     small, and computed the same way for every entity. Truncating a **display**
     closure is honest (the language's own `whole` is described as a display
     closure) provided the truncation is visible, which the marker ensures.
     An author who needs the full closure writes an op that pages it.
   - **`std.E.rollup($id)` is trimmed, because an aggregate cannot
     truncate without lying.** A sum over a capped page is a wrong number, and
     a wrong number with a `truncated` flag beside it is worse than no op. So
     rollup derives **only when the closure is provably bounded**, meaning every owned
     collection in it carries a declared relationship cap and any tree in it a
     depth cap. Otherwise it **is not derived at all**, with the plan output
     and the shape summary naming the collection that has no cap. This extends
     §4's existing "derived only when…" conditions with no new
     mechanism, and it keeps std's promise intact: **std derives nothing it
     cannot bound.**
   - The honest consequence: on schemas with no relationship caps, `rollup`
     mostly does not exist, and receipt-total-shaped features start as an
     authored op that acknowledges `unbounded` deliberately. That puts the
     acknowledgment on a human, where it belongs, and the platform never takes
     it silently on anyone's behalf.

   One caveat keeps the claim true:
   - **The registered class is not the served class.** A refinement can raise
     an op's **effective** class at request time: a host-side condition on
     `std.E.list` raises the *venue* (a page-sized host filter, since refinement
     is post-take), and a condition under `std.E.count` (the one std shape with
     no page window) can raise *cardinality*. std ops are registered without
     `unbounded`, so a refinement that would make one unbounded is **rejected**
     (`refinement_unbounded`) and is never served (see §2.1 rule 4 and
     `api-engine-design.md` §7.1). That rejection keeps this rule honest.
     Without it the rule would describe registration and say nothing about
     behaviour.
   - **Cost classes rank host-side work.** An op that scans every row of an
     entity but pushes every stage into SQL classifies `sql` and needs no
     acknowledgment. From std's side this is bounded by std's derivations
     being addressed, keyset-paged, or reduce-shaped. The general claim
     "nobody ships an O(space) dashboard by accident" is about *host*
     cost, and the language scopes it (`definition-language.md` §5, cost
     classes).

---

## 6. Namespace [settled]

`std.` is **reserved wholesale**: no package, domain, or authored op may
claim the name or register under it. Doing so is a compile error. The
reservation is what makes the baseline predictable: `std.E.get` means one
thing in every Ordo deployment ever.

Packaged entities derive under **the package's namespace inside std**:
entity `finance.transactions` yields `std.finance.transactions.get`. The
qualified form is pinned. std over a package's **exported** shapes is the
stable, composable, catalog-listed surface (and the package's
capability-catalog fragment lists it, so an assistant sees the baseline
without inference [lean]); std over internal shapes exists but follows the
export rule: it is not composable across the package boundary, and free to
re-hash in minor versions because internal shapes restructure freely.

---

## 7. Composability [settled]

std ops are **ordinary registered artifacts**. Every consequence follows
from that sentence:

- **Sources.** Read-kind std ops are legal composition sources anywhere a
  registered query is. Rarely *shorter* than the entity source (inside
  `.ordo` you would write `projects(…)`), but useful where std is
  the only door: history and introspection.

  ```
  op auditTrail($p: id) =
    std.projects.history($id: $p)[at, actor, kind]
    take 50
  ```

- **In-source spelling (grammar note).** Composition uses the dotted name
  **verbatim** (`std.projects.history(…)`). Registered-name references in
  source position accept qualified names, the same rule that makes
  cross-package composition (`finance.spendingByPayee(…)`) parse. std
  names are *always* qualified, because no source file is ever
  "inside" std the way a package's own files omit their qualifier. The
  paren/bracket grammar is unchanged. The dots are part of the name and are never
  a traversal.
- **Refinable** per §2.1; refinement compiles through the same inliner as
  written composition.
- **Versioned with the schema.** std artifacts are a function of (schema,
  platform derivation version): a shape change recompiles the entity's std
  ops in the same deploy; a platform upgrade that changes the derivation
  re-hashes them. Version-hashed, shape-verified against the empty shadow
  schema at load, hot-swapped, rollback-eligible, like everything. Scope
  expansion kills pinned grants until re-consent, and std uses the same
  mechanism that packages do.

---

## 8. Deref-by-prefix [settled]

Typed ids make a universal dereference possible, and std makes it real:
`ordo.deref(id)` in every generated SDK reads the id's prefix, maps it to
the entity (the prefix table rides in the interface descriptor), and calls
`std.E.get`. This is **client-side routing** and is not a server op. It cannot be a
server op, because result shapes are static per entity, so "get anything" has no
one registered shape; the SDK returns the branded per-entity record type.

Consequence: **every id anywhere is a working link.** A ref in a query
result, an id in a command-log entry, an id pasted into a debug session:
each is one call away from its full row, access-checked as always.

---

## 9. Interaction table [settled]

| std × | Ruling |
|---|---|
| acl / guards | Guards inline (§5.1); acl-derived link entities get std ops with the highest-level write gate (§2); guard inheritance disclosed in std manifests like any op's |
| groups | A group is a hop in the guard's semi-join, resolved inside the inlined guard (§5.1). std derives no *per-entity* group ops; group administration and membership are the `std.space.*` group ops (§3.1), which are manager-gated, command-logged, and never mirrored on-device (`implementation.md`, client storage rules) |
| sealed fields | Open-skeleton derivation by default; sealed-inclusive std needs the shape's explicit marker (§2.4). Write params keep sealed fields either way |
| cost classes | Every derivation carries a provable **row bound** as well as a venue class: addressed, paged, reduce-shaped, or capped (`whole`), and trimmed where neither is possible (`rollup`), so no std op ever needs the `unbounded` acknowledgment (§5.7). Registered class ≠ served class: the effective class is recomputed per request from the refinement, and a std op that would become unbounded is rejected, never served (§5.7, §2.1) |
| composition | std ops are ordinary sources, dotted-name verbatim (§7); only read-kind std ops are legal sources, per the general rule |
| refinement | list/count/get/whole/under Tier-0; count validates against the list manifest (§2.1); sealed conditions policy-gated per §B2 |
| live | every std read is live-capable; `std.space.*` and `std.catalog()` included (§3) |
| client-first | std compiles into the on-device engine **identically**, with the same artifacts and a different execution venue (implementation.md); the surviving-capability list under custody conversion computes over std manifests like any other |
| domain packages | derivation per package under `std.<pkg>.<entity>.<verb>` (§6); exports bound the stable surface; enabling a package in a space makes its std ops registry-present there, exposure still opt-in |
| generated UI | table = `std.E.list` (+refinement), detail = `std.E.get`/`whole`, form = `std.E.create`/`update`. The generated-UI north star renders from std before any authored op exists; UI annotations (`label`, `desc`, …) flow into std manifests |
| capability catalog | std ops are catalog entries like any op, access-scoped. An agent's catalog shows only the std it may call, so a narrow token's surface stays small |

---

## 10. Open / lean items

- **`std.E.list` default sort = `-id` [lean].** Newest-first: UUIDv7 is
  time-ordered, so `-id` is reverse-chronological-by-creation with zero
  extra index. It is the feed default nearly every listing screen wants, and
  the keyset stays stable under writes (new rows land before the cursor,
  never inside a visited page). Refinement cannot re-sort in Tier 0; ops
  needing another order are authored. Revisit only if ascending scans
  (export/ETL callers) turn out to dominate.
- **rollup promotion criteria [open]**: what usage evidence promotes §4
  from [lean] to settled, and whether the flat column-pair naming
  survives contact with generated UI.
- **`whole`'s per-collection cap [open]**: the cap's value (a platform
  constant, deliberately not guessed here) and how generated UI renders a
  `truncated` collection (a marker skins must handle, and not an empty list).
  Both need pinning before first ship, since the cap is the reason
  `std.E.whole` has a row bound at all (§5.7).
- **`std.space.*` and `history` exact field lists [lean]**: mechanisms
  settled, column names not final; `history`'s image rendering should pin
  before first ship.
- **Registry size [lean]**: ~12 ops × every entity is real registry
  weight. Catalog scoping already hides unexposed std from callers;
  whether the registry view for the deployment owner needs std folding is a
  tooling question.
