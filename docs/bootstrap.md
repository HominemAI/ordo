# Ordo Space Bootstrap

Resolves `critique-consolidated-3.md` **Z-020**, *whole-tenant atomic bootstrap
has no single platform surface*.

Companion to `definition-language.md` (§3 `members` / `acl` / G10a, §8 access
model, principals, clients, inter-space sharing, exit ops; §9 deployment model),
`implementation.md` (deployment topology, key custody, custody dial, client-first),
`security-logging-design.md` (providers, provisioning, token model, the door,
command log), `std-library.md` (§3 `std.space.*`), `migration.md` (genesis import),
`domain-packages.md` (what a package ships and what it may never ship).

Markers as elsewhere: **[settled]** owned here or carried from a companion ·
**[proposed]** current recommendation · **[open]** needs an owner ruling.

---

## 0. The problem, stated once

Getting a working space today means calling five or six surfaces in an order
nobody wrote down: mint the space, mint the owner's `members` row, enable the
domain packages, instantiate each package's access-profile template, create the
domain's root rows, seed the first `acl` rows, bind a client, provision keys,
maybe import data. Every one of those is a real, defensible operation. Together
they are a distributed transaction with no coordinator, no failure semantics,
and no document listing its inputs.

The consequences are not hypothetical. A crash between step 2 and step 4 leaves
a space with an owner and no profile, so the owner can call nothing. A crash
between step 1 and step 2 leaves a space with **no owner at all**, which by the
`acl` rules (§3, bootstrap rule 6) is the one state from which nothing can ever
be repaired from inside. Z-020's own counterexample is sharper still. The
authored op can atomically couple the domain row and its `acl` creator grant,
and nothing else. The deadlock G10a closed at the row level is still
open at the tenant level.

**The fix is one surface:** one platform operation with one input set, one
command group and one failure mode. It replaces the documented sequence and the
helper script.

---

## 1. The one surface: `std.space.create` [settled]

```
std.space.create(<the input inventory of §2>) -> { space, chainHead, state }
```

It lives in `std.space.*` because that namespace is already "where space runtime
state becomes callable" (`std-library.md` §3.1), and its siblings
(`createGroup`, `addToGroup`) are already the space's own administrative
commands. It is a `write` op, `POST`, command-logged, revertible, idempotent on
`Idempotency-Key` like every other write.

### 1.1 It is the deployment-scoped op [settled; this document owns it]

`std.space.create` is the one op in the platform that **binds no space**. This
needs stating because `api-engine-design.md` §2 makes the
opposite a hard rule: space binding happens *before* the access check, there is
no default space, and an unbound request is `missing_space` (400). A call that
creates a space cannot name the space it is creating.

The rule applies to a closed set of exactly one op in v1:

> An op may declare **`scope deployment`**. A deployment-scoped op binds no
> space; `$space` / `X-Ordo-Space` on such a call is `unknown_param`, not a
> hint. Its door check consults a **deployment capability** instead of a space
> profile, because there is no space whose profile could list it.

Everything else about the door is unchanged: one check before planning, the
denial answer is `access_denied` for every reason, denials never enter a command
log and land in the deployment's security-event stream instead. The deployment
capability that admits the call is `deployment.spaces.create`, and §3 says who
holds it on each of the three entry paths.

This is deliberately not solved by "create the space, then bind it", which is
the two-phase shape the finding is about. A sentinel space id is also ruled out,
because a sentinel is a space-shaped thing that some code will
eventually treat as a space.

### 1.2 What it does, in order [settled]

Seven phases. Phases 0–1 and 4–5 are transactional; 2–3 are not, and §4 owns
their compensation.

| # | Phase | Transactional? | Where |
|---|---|---|---|
| 0 | **Preflight**: validate every input, resolve every reference, decide every default, produce the full plan. No writes of any kind. | n/a (pure) | platform |
| 1 | **Reserve**: insert the space registry row in state `provisioning`, taking the deployment-unique `slug` lock. This is the durable intent record; nothing after it is orphanable. | yes (platform DB) | platform DB |
| 2 | **Provision keys**: `KeyProvider.createSpaceKey` mints the **SMK**; `DEK_1` is minted and wrapped under it; the blind-index key `BIK = KDF(SMK, "blind-index", …)` becomes derivable. Handles recorded on the registry row. | **no** (external KMS) | KeyProvider |
| 3 | **Provision storage & schema**: resolve the space→database and space→bucket bindings (platform default or BYO), verify connectivity, apply the enabled packages' compiled schema to that database. | **no** (external DB/bucket) | target DB/bucket |
| 4 | **Genesis group**: one command group, chain position 0, `prev` = 32 zero bytes, in the *new space's* chain. Contents in §1.3. | yes (space DB) | space DB |
| 5 | **Activate**: registry row `provisioning` → `active`; deployment control-plane audit event. | yes (platform DB) | platform DB |
| 6 | **Post-commit**: deliver the owner invitation if any; emit `space.created`; enqueue the custody conversion if `custody: client-first`; enqueue the genesis import if `seed` was supplied. | n/a (each owns its own machinery) | jobs |

**Preflight is the whole product.** Every decidable failure (a missing input, an
unknown package, an unresolvable owner, an unreachable BYO database, an
unhealthy KeyProvider, a jurisdiction the deployment does not serve) is
rejected in phase 0, before the registry row exists. A caller who passes
preflight and then fails has hit an infrastructure fault, not a mistake, and §4
is about exactly that residue. `dryRun: true` runs phase 0 and returns the plan.

### 1.3 The genesis group [settled]

**One command group. Both or neither.** It commits in a single transaction in
the space's database, hashes into the space's own chain as its first entry, and
is revertible as a unit by `revert_group` like anything else. Its actor is the
attributed bootstrap actor (§2, item 9).

It contains, in this order:

1. The **space row**: id, slug, displayName, custody position, jurisdiction,
   placement, key handles, DEK epoch 1.
2. The **owner `members` row**: `role: owner`, `status: active`, `kind:
   principal`, `displayName` from the identity service (§3 `members`, minting
   path (i): "space creation, for the creating principal, with role `owner`").
3. The **policy rows**: `commandlog`, `retention`, `compliance`, `mirror`,
   materialized from the inputs of §2 group G. Policies are space state, not
   config re-read at runtime.
4. The **access-profile instantiation**: one profile row set per enabled
   package, copied from that package's template. From this instant the template
   is inert; the space owns its profile (`domain-packages.md` §1).
5. The **domain bootstrap rows**: the singleton root rows each enabled package
   declares in its `bootstrap` block (§2 item 14): the `organization`, the
   `household`, the `practice`. These are ordinary creates by an ordinary actor.
6. The **`acl` creator grants** for those rows: the owner receives the top
   declared level on each, by §3's creator-gets-admin rule, *in this same group*.
   G10a exempts each create from the write guard it is bootstrapping.
7. The **client bindings**: which registered clients this space's humans
   authenticate through.
8. The **notification routes**: channel defaults for the space.
9. The **group shells**, if `groups[]` was supplied: named, empty.

Points 5 and 6 are the direct answer to Z-020's counterexample. The finding
observes that "the authored op can atomically couple only the final domain row
and its grant"; the resolution is to move the domain's root rows *into* the
genesis group, which requires the package to declare them, which is the
`bootstrap` block. Nothing here is a new authorization mechanism: the creator
grant is the settled §3 rule, minted in the same group, with the same actor,
under the same G10a exemption.

**What is NOT in the group**, deliberately: no capability grants, no
memberships beyond the owner, no per-member overrides, no outbound sinks, no
imported data. §5 says why for each.

---

## 2. The complete input inventory: 46 items

The heart of this document. Every item carries: what it is, whether it is
**required** / **defaulted** / **derived** / **not an input**, who supplies it,
and what happens if it is wrong.

"Not an input" entries are load-bearing. Half the confusion Z-020 describes is
callers looking for a knob that must not exist.

### A. Space identity and placement (1–4)

| # | Input | What it is | Status | Supplied by | If wrong |
|---|---|---|---|---|---|
| 1 | `slug` | Deployment-unique URL-safe handle. The thing that appears in links, support tickets, and share invitations. | **required** | the human | Collision ⇒ `duplicate` in preflight, no state. Charset violation ⇒ `invalid_param`. It is **not** secret: §2 of the token model already treats space ids as leakable, which is why grants are space-bound. |
| 2 | `displayName` | Human-readable name, rendered everywhere. | **required** | the human | Empty ⇒ `missing_param`. Never null; a space with no name is a space nobody can pick out of a list. |
| 3 | `spaceId` | `spc_…`. | **not an input: platform-minted** | platform | A caller-supplied id would let a caller name a space that already exists elsewhere in the deployment and probe for it. Ids are server-minted, always (`migration.md` §3, no exception for imports and none here). |
| 4 | `placement` | Which database/bucket class or region the space lands in when not BYO. | **defaulted** (deployment default) | deployment config | An unserved region ⇒ preflight rejection. Changing it later is a custody conversion, not an edit. The confirmation says so at creation. |

### B. The owner principal and attribution (5–11)

| # | Input | What it is | Status | Supplied by | If wrong |
|---|---|---|---|---|---|
| 5 | `owner.principal` | An existing deployment principal id (`usr_…`) to mint as owner. | **required, XOR with 6** | caller | Unknown principal ⇒ preflight rejection, and the rejection is the anti-enumeration answer, not "no such user". A non-human principal (`agt_`, `svc_`, routine, source) ⇒ hard reject: §3's minting paths admit no such row, and a space owned by a robot has no one to consent. |
| 6 | `owner.invite` | Identifier (email / OIDC subject / SAML name-id) plus method, for a human who **does not exist yet**. Provisions a directory principal with a pending authenticator link, then mints them owner. | **required, XOR with 5** | caller | An identifier already linked to a principal resolves to item 5 instead of double-minting. An identifier no configured provider can ever authenticate ⇒ reject: minting an owner who can never log in creates the unrepairable state this whole document exists to prevent. |
| 7 | `owner.displayName` | The name on the owner's `members` row. | **derived** | identity service | Taken from the OIDC `name` claim → `preferred_username` → platform-generated handle (§3 `members`). Never null, never caller-supplied. A caller-supplied name is a caller writing a platform field. For path 6 (invited, never logged in), the invite's identifier local part is the handle until first login renames it. |
| 8 | owner role | `owner`. | **not an input: fixed** | platform | There is no "create a space whose first member is a viewer". The genesis group mints exactly one `role: owner`, `status: active` row. See §8's new rule. |
| 9 | `actor` | The attribution string on the genesis group. | **derived from the entry path** | platform | First-run ⇒ `system:bootstrap`; self-serve ⇒ `user:<caller>`; operator ⇒ `user:<operator>`; agent ⇒ `agent:<name> on behalf of user:<owner>`. Never caller-supplied. The chain records who really did it. |
| 10 | `delegation` | Reference to the live delegation grant when an **agent** is the caller. | **required on the agent path** | runtime state | Absent or expired ⇒ `access_denied`. §8's rule stands unchanged: "acting on behalf of a user requires that user's own consent." An agent may create a space owned by **its delegator only**. It may never create one for a third party. |
| 11 | `stepUpProof` | Fresh WebAuthn user-presence. | **required** on self-serve and operator paths; **impossible** on the agent path | the human | Missing ⇒ `access_denied` naming step-up. Presence is not delegable (`security-logging-design.md` §5), so an agent-created space **inherits its delegator's presence at delegation time** and cannot mint one. If deployment policy demands step-up for space creation, the agent path is structurally closed. That is a legitimate deployment posture, and it should be made visible. |

### C. Domains, schema, and surfaces (12–16)

| # | Input | What it is | Status | Supplied by | If wrong |
|---|---|---|---|---|---|
| 12 | `domains[]` | Package names with exact versions, such as `finance@2.1.0`. What schema this space gets and what ops exist in it. | **required, non-empty** | the human, from the deployment's installed set | Unknown/uninstalled package ⇒ preflight rejection listing what is installed. Unsatisfied `requires` dep ⇒ rejection naming the missing dep. An empty list is rejected: a space with no domain is a space with no ops, which is a state nobody wants and everybody would then have to repair. |
| 13 | `profile` | Which named access-profile template to instantiate per package, when a package ships more than one (`default`, `restricted`). | **defaulted** (`default`) | domain package | Unknown template name ⇒ rejection listing the package's templates. The template is copied, then owned by the space; upgrades never overwrite it. |
| 14 | `bootstrapRows` | Field values for the singleton root rows each package declares in its `bootstrap` block, such as the organization's legal name, the household's currency, the practice's timezone. | **required iff any enabled package declares one** | the human | A missing required field ⇒ `missing_param` naming package, entity, and field. A `check` violation ⇒ typed rejection, in preflight. This is the input class that did not exist before this document and without which Z-020 cannot close. |
| 15 | domain-required policy inputs | Values a package declares it needs for its own retention/compliance rows (`finance` needs a records-retention term). | **required iff declared** | the human, defaulted by jurisdiction | Missing ⇒ rejection. A package may not ship a *default* that is a legal decision; it may ship a per-jurisdiction suggestion the caller confirms. |
| 16 | `apps[]` | Which UI surface manifests are enabled for the space. | **defaulted** (all apps whose domains are enabled) | deployment config | Unknown app ⇒ rejection. An app referencing an op from a package this space did not enable ⇒ rejection naming both. Apps are windows; enabling one grants nothing. |

### D. Access seeding (17–20)

| # | Input | What it is | Status | Supplied by | If wrong |
|---|---|---|---|---|---|
| 17 | `acl` creator grants | Who holds the top level on each bootstrap root row. | **derived: always the owner** | platform | Not an input. §3's rule is "the creator is the principal attributed on the creating command, resolved to their members row"; in the genesis group the attributed principal's members row is the owner's, so the owner holds top level on every bootstrap row. A caller-supplied grant list here would be consent granted by a call parameter. |
| 18 | `groups[]` | Names of groups to create, empty. | **optional** | the human | Duplicate names ⇒ `duplicate`. **Members may not be supplied**: adding a principal to a group is an audited command with its own op (`std.space.addToGroup`), and group membership is a grant path (`security-logging-design.md` §6). |
| 19 | per-member overrides | Profile additions/subtractions for named members. | **not an input: refused** | none | There is exactly one member at creation and they are the owner; an override at creation could only mean "pre-consent someone who is not here yet". Overrides are ordinary post-creation profile commands. |
| 20 | `provisioning` | The space's first-login policy: `invite-only` (default) or `auto-onboard`, plus the role auto-onboarded humans receive. | **defaulted** (deployment default, `invite-only`) | deployment config, overridable by the human | The auto-onboard role may never be `owner` (§3 `members`, path iii). A request for it is a hard reject. `auto-onboard` on a space whose bound clients are public is a self-serve tenant with open registration; the confirmation says so in those words. |

### E. Keys and custody (21–25)

| # | Input | What it is | Status | Supplied by | If wrong |
|---|---|---|---|---|---|
| 21 | `custody` | Dial position at creation: `managed` \| `byo-storage` \| `client-first`. | **required: no default, ever** | the human | Absent ⇒ `missing_param`. There is no silent default here (§5). See the client-first ruling below. It changes *who can validate the bootstrap*, which is why it cannot be inferred. |
| 22 | `keyProvider` | Which configured KeyProvider and which key ring/label the SMK lives in. | **defaulted** (deployment's single provider) | deployment config | Unknown provider ⇒ preflight rejection. Unhealthy provider ⇒ preflight rejection *before* the registry row exists. This is the check that keeps phase 2's non-transactionality from mattering in the common case. |
| 23 | SMK origin | `mint` (default) or `import` of a customer-held master key. | **defaulted** (`mint`) | the human | An imported key the provider cannot wrap ⇒ preflight rejection. Import shifts erasure semantics: destroying an imported SMK is still crypto-erasure, but the customer may hold a copy, and the consent page must not claim otherwise. |
| 24 | `DEK` epoch 1 | The first data-encryption key and its epoch tag. | **derived** | platform | Not an input. Every sealed payload carries an epoch tag **from the first write** (`implementation.md`, key custody). Bootstrap is where "from the first write" becomes true. |
| 25 | custody acknowledgement | The consent token proving the caller was shown the computed capability list and envelope disclosure for the chosen dial position. | **required iff `custody != managed`** | the human | Absent ⇒ rejection. The list is computed from manifests, never prose (`implementation.md`, custody conversion). At `client-first` the acknowledgement must also carry the three sentences of "what blindness costs": members can read everything, the server stops refereeing, bad writes are caught rather than blocked. |

**`client-first` at creation, ruled [proposed].** A blind server cannot validate
its own genesis: phase 4 asserts an owner exists, a profile was instantiated, and
the `acl` rows match the bootstrap rows, and every one of those assertions reads
plaintext. So:

> `std.space.create` **always executes the genesis group server-side under
> managed custody.** `custody: client-first` is accepted as a **target**: the
> space is created managed, and phase 6 enqueues the standard custody conversion
> job with its consent already captured. The space reports
> `state: active, custody: converting` until the job completes.

This costs nothing real, because the genesis group contains no sealed values.
Every row in §1.3 is plaintext skeleton at **every** dial position. Members,
roles, profiles, `acl` rows, policies, and the command envelope are readable to
the operator at client-first too (`implementation.md`, "What the dial does not
cover: envelope disclosure"). Pretending the genesis group is private at
client-first would be the mis-selling that document warns about. One hard
consequence: **`seed` (item 42) is refused with `custody: client-first`**.
A client-first space imports on-device through the embedded engine
(`migration.md` §3), which cannot happen inside a server-side call.

### F. BYO placement targets (26–28)

| # | Input | What it is | Status | Supplied by | If wrong |
|---|---|---|---|---|---|
| 26 | `database` | BYO-DB target: a secret reference resolving to a connection string, plus the schema name. | **required iff `custody: byo-storage`** | the human / deployment secret store | Unreachable, wrong version, insufficient privilege, or non-empty schema ⇒ preflight rejection with the specific fault. Preflight **connects and runs a probe transaction**; it does not merely parse. |
| 27 | `bucket` | BYO-bucket target for blobs, same resolver pattern. | **optional** (platform bucket otherwise; omit entirely if every blob field is `inline`/`uri`) | the human | Unwritable bucket ⇒ preflight rejection. A write-probe object is created and deleted in preflight. |
| 28 | credential handles | Secret references, never literal credentials. | **required with 26/27** | deployment secret store | A literal credential in the call body is rejected at the binder, not stored and warned about. It would land in the request log, the replay cache, and the operator's terminal. |

### G. Jurisdiction, compliance, retention (29–33)

| # | Input | What it is | Status | Supplied by | If wrong |
|---|---|---|---|---|---|
| 29 | `jurisdiction` | `EU` \| `US` \| `US-CA` \| …. The per-space flag the compliance pack keys every default off. | **required: no default, ever** | the human | Absent ⇒ `missing_param`. §5. A jurisdiction the deployment does not serve (no lawful placement) ⇒ rejection. Changing it later is a real migration with data-residency consequences, not a field edit. |
| 30 | `retention` | Per-entity-class keep terms and legal-hold capability, from the compliance pack. | **defaulted** (jurisdiction + package defaults) | jurisdiction, domain package | An override shorter than a statutory minimum for the jurisdiction ⇒ rejection naming the class and the floor. The consent page states retention per class, as the compliance pack requires. |
| 31 | `commandlog` | `undoHorizon`, `checkpoint` interval, `archive` destination, and **`attest` cadence**, which is a **separate knob from `checkpoint`, not derived from it**. | **defaulted** (deployment policy) | deployment config | `undoHorizon` is not cosmetic: it gates when a retired DEK epoch key may be destroyed (`security-logging-design.md` §7), so a 1-year horizon delays post-compromise recovery by a year. The confirmation states the derived date, not the prose. **`attest` must not inherit `checkpoint`'s cadence:** a checkpoint is the heavyweight pruning/verification anchor and `1mo` is right for it; an attestation is a few hundred bytes and one signature. Wiring them together would leave a month of unattested history. A month is a long time for a rewrite to sit undetected. Spelled `attest every 1000 commands or 1h` (volume **or** time, whichever lands first), so a busy space attests often and a quiet one costs nothing, because the head only moves when a write commits. A space may request a **tighter** cadence than the deployment default. |
| 31a | `attest.webhook` | Owner-nominated destination for the signed head at each attestation, under a schema published in advance. | **optional: opt-in per space** | the human (owner) | This is the **only** attestation layer that needs setup, and it is per space, not deployment-wide. Layer 1, **client pinning**, is **always on, ~free, and requires no configuration at all**: responses already carry a command-log position. They now also carry the **signed head** at that position, so every request is an attestation to that client and detection latency for anyone actually using the system is one request. Do not present the webhook as the thing that makes history verifiable; present it as the thing that makes verification survive a client that stops calling. The destination config is **owner-signed at setup**, so the platform stores it but cannot silently redirect it. Omission is not preventable: this layer detects, it does not prevent. |
| 32 | legal-hold flags | Which entity classes may be placed under hold, suspending erasure. | **defaulted** (compliance pack) | jurisdiction, package | A hold-capable class in a space whose owner was promised crypto-erasure is a contradiction the confirmation must surface, because erasure destroys the SMK and a hold cannot survive it. |
| 33 | `mirror` | Client-mirror policy: whether devices may hold a local store, and its retention. | **defaulted** (`on` for every custody position, owner-ruled 2026-09-14; mandatory for `client-first`) | deployment config | A stolen device yields whatever the mirror holds; `std.space.*` and command-log slices are never mirrored regardless (`implementation.md`, client storage rules). |

### H. Authentication and clients (34–37)

| # | Input | What it is | Status | Supplied by | If wrong |
|---|---|---|---|---|---|
| 34 | `clients[]` | Which registered identity clients this space's humans authenticate through. | **required, non-empty** | deployment config (IaC), selected by the human | An unregistered client name ⇒ rejection. **Bootstrap binds clients; it never creates them**. A client is IaC (§8, principals table), and minting one at runtime would let a caller create a relying party with its own redirect URI. A space with no bound client has an owner who cannot log in. |
| 35 | auth methods | passkey / password / email-otp / federated providers. | **not an input: deployment-level** | `auth { … }` block | One deployment, one issuer, one method registry. A space does not get its own login methods; it gets the deployment's. Preflight asserts at least one method can authenticate the owner (items 5/6), and rejects otherwise. |
| 36 | step-up op set | Which ops demand fresh presence. | **not an input: client-level** | `client { stepUp … }` | Per Z-003 the minimum belongs on the op, and clients may only strengthen it. Bootstrap neither reads nor writes this. |
| 37 | issuer / RP apex domain | The passkey RP id and OIDC issuer. | **not an input: deployment-level, chosen once** | deployment config | Changing it strands every authenticator in the deployment (`implementation.md`). It is not per-space and must never look per-space. |

### I. Notifications (38–41)

| # | Input | What it is | Status | Supplied by | If wrong |
|---|---|---|---|---|---|
| 38 | `channels` | Which platform transports the space uses: push, email, sms. | **defaulted** (deployment's configured channels) | deployment config | A channel with no configured provider ⇒ rejection. Domains never touch providers; they emit `notify` proposals. |
| 39 | `routes` | Severity → transport routing defaults (`route alerts.critical -> [push, sms] fallback email`). | **defaulted** (package + deployment defaults) | domain package, deployment | A route naming an unconfigured channel ⇒ rejection. Routes are operator policy and are ordinary space state afterwards. |
| 40 | owner contact | Where the bootstrap receipt and the invitation (path 6) go. | **required iff path 6**; **derived** otherwise | invite identifier / identity service | An undeliverable address on path 6 means an invitation nobody receives and an owner who never arrives. Preflight validates deliverability shape; phase 6 reports the actual send. |
| 41 | outbound HTTP sinks | Customer-named egress destinations. | **not an input: refused** | none | An outbound sink carrying a space's data is a **per-space granted sink** requiring space-owner consent naming the endpoint (§8, notification channels; `security-logging-design.md` §7b). At creation there is no owner who has consented to anything yet. A config file can route; only a grant can point a space's data outward. |

### J. Starting data (42–43)

| # | Input | What it is | Status | Supplied by | If wrong |
|---|---|---|---|---|---|
| 42 | `seed` | Reference to a genesis-import mapping (`migration.md` §3). | **optional** | the human | Refused with `custody: client-first` (§2E). Enqueued in phase 6, **outside** the genesis group: an import is thousands of batch groups with its own actor (`source:import-<name>`), its own checkpoint, and its own verify report, and folding it into one transaction would make a 10M-row import a single lock. |
| 43 | `seed.blocking` | Whether the space is withheld from non-owner members until the genesis checkpoint lands. | **defaulted** (`true`) | the human | With `false`, invitees can arrive mid-import and see partial data. Stated, not discovered. |

### K. Call controls (44–46)

| # | Input | What it is | Status | Supplied by | If wrong |
|---|---|---|---|---|---|
| 44 | `Idempotency-Key` | Replay key. | **required**: the one write where it is not optional | caller | A retry after a network failure must return the *same* space, not a second one. The replay cache is Postgres-backed (`implementation.md`, topology); an in-memory cache in a multi-node deployment makes double-creation reachable, which is a correctness defect. Absent ⇒ rejection. |
| 45 | `dryRun` | Run phase 0 and return the plan: resolved defaults, computed capability list, retention schedule, derived key-destruction date, the exact genesis group. | **defaulted** (`false`) | caller | This is the surface agents and operators should call first. It writes nothing and reserves nothing, so two dry runs of the same slug both succeed. |
| 46 | `confirm` | Bundle of acknowledgement tokens: custody (25), jurisdiction, retention floors, and, when applicable, auto-onboard and hold/erasure tension. | **required iff any acknowledgement is pending** | the human | A missing token ⇒ rejection listing exactly what has not been acknowledged. An agent cannot mint these; see §3. |

**Count: 46 inputs: 13 required, 14 defaulted, 5 derived, 14 not-an-input
(9 platform-owned, 5 explicitly refused).**

---

## 3. The three entry paths [settled]

Same op, same genesis group, three different answers to "who is allowed and who
is attributed".

### 3.1 First-run: empty deployment, no humans exist

The state the spec already names: `principals` are "IaC only for the bootstrap
admin" (§8), and "the bootstrap admin arrives via IaC; everything after that is
runtime state" (`security-logging-design.md` §3).

- **Caller:** the bootstrap admin principal, declared in IaC, holding
  `deployment.spaces.create` implicitly.
- **Attribution:** `system:bootstrap`.
- **Owner:** items 5 or 6. The bootstrap admin is the *caller*, and it is
  **not** minted as owner unless it is also named as owner. An IaC-declared
  principal silently owning every space is the config-grants-consent failure §5
  forbids.
- **Step-up:** waived, because there is no authenticator to prove presence with yet.
  This waiver is the reason for the next bullet.
- **The one-shot rule [proposed]:** first-run is available **only while the
  deployment's directory holds no linked human authenticator**. The moment the
  first human links one, the bootstrap admin's `deployment.spaces.create`
  capability is a normal capability requiring the normal proof, and the waiver
  is gone. Otherwise an IaC credential is a permanent step-up bypass.
- **Practical shape:** `ordo bootstrap --dry-run` prints the plan; the operator
  reads it; `ordo bootstrap` executes it; the owner receives an invitation and
  links their first authenticator.

### 3.2 Self-serve: an authenticated human creates their own space

- **Caller:** any human principal, **iff** deployment policy enables self-serve
  (`policy spaces { selfServe: on, perPrincipalLimit: N }`). Default **off**:
  an unruled default here is an open tenant factory reachable by anyone who can
  register.
- **Attribution:** `user:<caller>`.
- **Owner:** must be the caller (item 5 = `$caller`'s principal). Naming
  somebody else is the invited path, and it needs the operator capability or a
  delegation. Being signed in is not enough.
- **Step-up:** required (item 11).
- **Quota:** `perPrincipalLimit` is checked in preflight. Space creation
  provisions a KMS key and possibly a database; it is the most expensive
  unauthenticated-adjacent operation in the platform.

### 3.3 Invited / provisioned: someone creates a space for someone else

Two sub-cases, and they differ in exactly one thing.

**Operator-provisioned.** A human operator holding the deployment capability
creates a space owned by another human, existing (item 5) or invited (item 6).
Attribution `user:<operator>`; step-up required. The owner's `members` row is
minted **`status: active`** immediately. The space is real and fully formed
before its owner ever logs in, which is the point, and it is why item
6 provisions a directory principal rather than a placeholder. A pending owner
would violate the "always at least one owner" rule at the only moment where the
violation is unrepairable.

**Agent-provisioned.** An agent creates a space under tighter constraints:

- A **live delegation grant** from the intended owner is required (item 10).
  Without it the agent acts only as itself, and an agent cannot own a space
  (item 5's non-human rejection).
- The owner **must be the delegator**. An agent that could name a third party
  would be manufacturing tenants for people who consented to nothing.
- The agent's capability token must list `std.space.create`, enumerated by a
  human on the consent page, space-bound like every grant. For a
  deployment-scoped op that means bound to the *deployment* control scope, and that
  is the one place the space-binding rule needs a companion. **[open]**
- Acknowledgements (item 46) **cannot be minted by the agent**. Custody and
  jurisdiction are consent, and consent is the delegator's. The workable shape
  is: the agent calls `dryRun`, presents the plan, the human acknowledges, the
  agent replays with the tokens. An agent that can create a space *and* accept
  its custody terms in one motion has been handed the human's signature.
- Step-up is structurally unavailable (item 11).

---

## 4. Atomicity and failure [settled]

### 4.1 The registry state machine

Every space has a registry row in the platform database from phase 1 onward:

```
provisioning ──► active ──► (custody conversion / seeding are sub-states of active)
     │            ▲   │
     │            │   ▼
     │            paused        (27ar: held still; resumed back to active)
     │
     └──► failed ──► reaped   (terminal)
```

- **`paused` is held still on purpose** (27ar, `howto-pause.md`). Reached only
  from `active` and left by a resume or a delete. Its ops are refused
  `space_paused` except those the app marks `nopause` and the platform ops
  that let people look, get out or take access away. The reconciler never
  times it out, it is not in the detection query below, and `std.space.reap`
  refuses it as it refuses `active`.

- **`provisioning` is never callable.** The door refuses every op, including
  reads and `std.space.members()`, for any space not `active`. This is
  the property Z-020 asks for in its alternative framing: earlier states are
  "resumable and non-callable".
- **`provisioning` older than `bootstrapTimeout` (default 15 min) is a
  reconciler target.** The reconciler is an ordinary job-queue worker.
- **`failed` is set by the reconciler or by the op's own error path**, and
  carries the phase that failed and the compensation outcome per resource.
- **`reaped` means every resource is released and the slug is free.**

Detection is therefore one query, `registry where state not in ('active',
'paused')`, and it
is on the operator dashboard as a mandatory pane, not a runbook grep.

### 4.2 What is transactional

- **Phase 1** (reserve): one platform-DB transaction, taking the slug lock.
- **Phase 4** (genesis group): one space-DB transaction, one command group, one
  chain entry, per the settled group rule. Every fact in §1.3 lands or none of
  them do.
- **Phase 5** (activate): one platform-DB transaction.

### 4.3 What cannot be, and the compensation for each

| Non-transactional step | Why | Compensation |
|---|---|---|
| **SMK creation (phase 2)** | The KeyProvider is an external KMS with its own durability and no participation in Postgres transactions. Crash after `createSpaceKey` returns but before the handle is recorded ⇒ an orphan key. | The handle is recorded **on the reserved registry row, in its own transaction, before the key is used**, so the row always names the key it may have created. Reconciliation is a **scheduled destroy** of the SMK, gated on the space never having reached `active`. Destroying an SMK is crypto-erasure and must never race a live space. Deployments whose KMS cannot delete (compliance holds) mark the key `abandoned` and alert; an orphan key is a cost item, not a breach. |
| **BYO-DB provisioning (phase 3)** | A second database, a second transaction manager, possibly a second network. | Preflight already connected and probed. On failure, the schema apply is rolled back with the recorded DDL inverse; a BYO schema that cannot be cleaned is **left alone and reported**. The platform never issues an unattended `DROP SCHEMA` against a customer's database. The registry row carries the residue so the operator can act. |
| **Bucket provisioning (phase 3)** | Object storage, no transactions. | Bootstrap creates only the prefix and a probe object. Compensation deletes the prefix; failure leaves an empty prefix, which is inert. |
| **Schema apply (phase 3)** | DDL is per-statement in most engines. | Applied in one transaction where the engine supports transactional DDL (Postgres does); otherwise the migration machinery's existing rollback path (`implementation.md`, migration failure modes). |
| **Invitation delivery (phase 6)** | Email/SMS providers. | Retried by the notification machinery. The space is `active` regardless. The owner exists whether or not the mail arrived, and the invitation is resendable. Delivery failure is never a reason to unwind a tenant. |
| **Genesis import (phase 6)** | Thousands of batch groups by design. | Owned by `migration.md`: batches revert in reverse order before the genesis checkpoint. Import failure leaves an `active`, empty, perfectly usable space. **Caveat this document does not own and will not paper over:** `migration.md` §4 describes genesis batches as create-only and therefore *convergent on re-run*, and also gives the group inverse as delete-by-minted-ids with no per-row before-images. Those two cannot both hold. A converged re-run wrote nothing new for a row that already existed, so a later delete-by-minted-id destroys a **pre-existing** row and reverts nothing. The clean case (a first import into the empty space this bootstrap just made) is unaffected, which is why bootstrap can proceed; a **re-run** is not, and needs `migration.md`'s ruling (finding Z-014). |
| **Custody conversion (phase 6)** | Chunked, resumable re-encryption. | Owned by `implementation.md`: interruption resumes, nothing is deleted until the target verifies. The space stays `active` at `managed`. A failed conversion leaves a space that did not move, never one that is half-encrypted. |

### 4.4 The rule that makes this tractable

> **Nothing external is provisioned before the registry row exists, and nothing
> external is used before its handle is recorded on that row.**

Every orphan is therefore reachable from one table. There is no resource the
platform created and cannot name.

### 4.5 Removal

`std.space.reap($space)` is deployment-scoped, operator-only and step-up gated.
It refuses any space in state `active`. It runs the compensation table in reverse
phase order and lands the space in `reaped`. Reaping an `active` space is not
this op; that is `space.delete`, which is owner-only, step-up gated, and the
crypto-erasure path with a confirmation window (§8, exit ops). **The two are
deliberately different ops with different callers**, because "clean up a failed
provision" and "destroy a customer's world" must never be one button.

---

## 5. What `std.space.create` must not do [settled]

1. **No consent granted by config.** It creates no capability grants, no
   memberships beyond the owner, no per-member overrides, no group memberships,
   and no outbound sinks. `domain-packages.md` already forbids a package from
   shipping principals, grants, spaces, or memberships ("a package carrying a
   grant would be consent granted by a tarball"), and a bootstrap *call*
   carrying them is the same violation with a different container. The
   access-profile **template** is the sole permission-shaped thing that crosses
   the boundary, and it is inert until instantiated (item 13).
2. **No silent default for custody (21) or jurisdiction (29).** Both are
   irreversible-in-practice decisions with legal and cryptographic
   consequences, and a default is an answer nobody gave. `missing_param`, every
   time. Deployment config may *pre-fill a form*; it may not satisfy the input.
3. **No bypass of the audit chain.** The genesis group is an ordinary command
   group with an ordinary actor, ordinary before/after images, and chain
   position 0. It is not "platform setup" written under the chain. There is no
   `unguarded` anywhere in it, and no synthetic actor that means "the platform
   did this on nobody's behalf". Every path in §3 produces a real attribution.
4. **No client creation, no auth-method creation, no issuer changes.** Those are
   IaC (items 34–37). Bootstrap binds what exists.
5. **No principal minting beyond the owner.** Item 6 provisions exactly one
   directory principal, and only when the space would otherwise have no owner.
6. **No writes before preflight passes**, and no partial success reported as
   success. The op returns `{ space, chainHead, state: "active" }` or it returns
   an error; there is no third answer.
7. **It is not a migration tool.** `seed` enqueues genesis import; it does not
   reimplement it.

---

## 6. Worked example, and the minimum viable call

### 6.1 End to end: an operator provisions a clinic

`ordo space create --dry-run` first:

```jsonc
// POST /ops/std.space.create   (no $space, scope deployment)
// Authorization: Bearer <operator token, step-up fresh>
{
  "slug":        "northside-clinic",
  "displayName": "Northside Family Practice",

  "owner":       { "invite": { "identifier": "dr.reyes@northside.example",
                               "method": "email-otp" } },

  "domains":     ["practice@3.2.0", "billing@1.9.4"],
  "profile":     { "practice": "default", "billing": "restricted" },
  "bootstrapRows": {
    "practice.organization": { "legalName": "Northside Family Practice PLLC",
                               "timezone":  "America/Chicago" }
  },
  "policyInputs": { "billing.recordsRetention": "7y" },

  "custody":      "byo-storage",
  "database":     { "secretRef": "sm://northside/pg", "schema": "ordo" },
  "bucket":       { "secretRef": "sm://northside/r2", "prefix": "spaces/northside" },

  "jurisdiction": "US",
  "retention":    { "practice.encounters": "10y legalHold" },
  "commandlog":   { "undoHorizon": "90d", "checkpoint": "1mo",
                    "attest": "every 1000 commands or 1h" },

  "clients":      ["clinicWeb"],
  "provisioning": { "mode": "invite-only" },
  "channels":     ["email", "push"],

  "groups":       ["Front Desk", "Clinicians"],

  "confirm": { "custody": "<tok>", "jurisdiction": "<tok>", "retention": "<tok>" },
  "dryRun": true
}
```

The dry run returns the resolved plan: every default filled in, the computed
op-capability list for `byo-storage`, the retention schedule per entity class,
the derived earliest-epoch-destruction date implied by `undoHorizon: 90d`, and a
rendering of the genesis group it would write. The operator reads it; the
acknowledgement tokens are minted against *that* plan hash, so an edited plan
invalidates them.

Re-sent with `dryRun: false` and an `Idempotency-Key`, it runs:

- **Phase 0**: resolves `practice@3.2.0` and `billing@1.9.4` and their deps;
  connects to the customer's Postgres and probes; write-probes the bucket;
  confirms the KeyProvider is healthy; confirms `email-otp` can authenticate the
  invited identifier; confirms `clinicWeb` is a registered client; validates the
  organization row against `practice`'s `check` invariants.
- **Phase 1**: registry row `spc_7Kq…` in `provisioning`; slug locked.
- **Phase 2**: SMK minted in the deployment's KMS; `DEK_1` wrapped; handles
  recorded on the registry row.
- **Phase 3**: schema for both packages applied to `ordo` in the customer's
  database; bucket prefix created.
- **Phase 4**: one command group, chain position 0, actor
  `user:usr_operator_…`:
  space row · `members` row for the provisioned `usr_reyes_…` (`role: owner`,
  `status: active`, `displayName: "dr.reyes"`) · three policy rows · two
  profile instantiations · `practice.organization` row · `organizationAccess`
  row granting Dr. Reyes `admin` · `clinicWeb` binding · email/push routes ·
  two empty groups.
- **Phase 5**: `active`. Deployment audit event.
- **Phase 6**: invitation emailed to `dr.reyes@northside.example`.

Dr. Reyes clicks the invitation, links a passkey, signs into `clinicWeb`, and
lands in a space where she is owner, the organization exists, she administers
it, `billing`'s restricted profile is in force, and `std.space.callersOf` has a
truthful answer for every op. **She had to do nothing to make the space work.**

### 6.2 The minimum viable call

The smallest legal input set (self-serve, everything else defaulted):

```jsonc
{
  "slug":         "my-budget",
  "displayName":  "My Budget",
  "owner":        { "principal": "$caller" },
  "domains":      ["finance@2.1.0"],
  "custody":      "managed",
  "jurisdiction": "US",
  "clients":      ["appWeb"]
}
```

Seven fields. Everything else in §2 defaults, derives, or does not exist. If
`finance@2.1.0` declares no `bootstrap` rows, item 14 is empty too. Note what is
*still* mandatory and why: `custody` and `jurisdiction` because §5.2 forbids
defaulting them; `clients` because an owner who cannot log in is the one
unrepairable outcome; `domains` because a space with no ops is a space that must
immediately be repaired.

---

## 7. The checklist

A literal list. Follow it top to bottom; nothing here requires reading the rest
of this document.

**Before you call anything**

1. Pick a **slug** (deployment-unique, URL-safe) and a **display name**.
2. Decide **who owns it**. Either an existing principal id, or an
   identifier + login method for a human who does not exist yet. Not a robot;
   not an agent; not "we'll add an owner later".
3. Decide **which entry path** you are on: first-run (nobody exists yet),
   self-serve (you own it), or invited/provisioned (someone else owns it). If an
   agent is calling, confirm the **delegation grant** from the intended owner is
   live.
4. List the **domain packages** and pin exact versions.
5. Ask each package what it needs: which **profile template**, which
   **bootstrap rows** and their field values, which **policy inputs**.
6. Choose **custody**: `managed`, `byo-storage`, or `client-first`. There is no
   default. If `client-first`, know that the space is created managed and
   converts, and that `seed` is unavailable.
7. Choose **jurisdiction**. There is no default.
8. If `byo-storage`, have the **database** and **bucket** secret references
   ready. They are references, never literal credentials.
9. Choose the **clients** the space's humans will sign in through. They must
   already be registered in IaC.
10. Accept or override: **retention**, **command-log policy** (know that
    `undoHorizon` sets the earliest date a retired key may be destroyed),
    **notification channels and routes**, **provisioning mode**, **mirror
    policy**, **placement**.
11. Optionally name empty **groups**. Do not try to put people in them here.
12. Optionally point at a **seed** import mapping.

**Calling it**

13. Call `std.space.create` with `dryRun: true`. No space is bound on this call
    That is correct, not a bug.
14. **Read the returned plan.** Resolved defaults, computed capability list,
    retention schedule, derived key-destruction date, and the exact genesis
    group.
15. Collect the **acknowledgement tokens** the plan demands (custody,
    jurisdiction, retention floors, and any others it names). A human collects
    these. An agent may not mint them.
16. Re-send with `dryRun: false`, the tokens, a fresh **step-up proof** (except
    first-run), and an **`Idempotency-Key`**.
17. On a network failure, **retry with the same `Idempotency-Key`.** Do not
    change the slug and try again.

**Afterwards**

18. Confirm the response says `state: "active"` and note the `space` id and
    `chainHead`.
19. Confirm the invitation was delivered, if you used one. Resend if not; the
    space is fine either way.
20. If anything reported an error, look up the space in the registry. Anything
    not `active` is not callable. Either resume it or run `std.space.reap`. Do
    **not** create a second space with a new slug and leave the first behind.
21. Everything from here is ordinary runtime state: invite members, edit the
    profile, grant capability tokens, add people to groups. None of it belongs
    in the bootstrap call, and none of it belongs in config.

---

## 8. Open items

- **Deployment-scoped grants [open].** A capability grant is pinned to exactly
  one space (`security-logging-design.md` §5). `std.space.create` has no space,
  so an agent's grant for it needs a *deployment* scope value. That is a real addition
  to `CapabilityGrant`, and the one place this design widens a pinned rule. The
  conservative reading (deployment scope is a distinct, separately-consented,
  operator-only scope that never satisfies a space comparison) is what §3.3
  assumes, and it needs an owner ruling before implementation.
- **~~Interaction with Z-004~~ [settled: Z-004 is resolved, and it resolved
  the way that costs this document nothing].** The concern was that Z-004 might
  be closed by *restricting* owner reach (quorum, delayed break-glass), which a
  single-owner space cannot satisfy, forcing bootstrap to mint two owners or to
  degrade the quorum rule explicitly at n=1. It was closed the other way.
  `definition-language.md` §8 now states: **no `acl` row is private from the
  space owner, and that is the definition of an owner**: an owner who can
  administer `acl` rows can grant themselves any level on any row, so nothing
  inside their own space is structurally hidden from them. It is a **disclosed
  property of ownership, not a hole**: a space is the unit of custody, and the
  person holding custody is by construction able to reach what they hold, the
  same way a database owner can read their database.

  So bootstrap mints exactly one owner and that stays correct. Two obligations
  ride with the ruling instead of a change to the minting rule, and both are
  this document's to honour:

  1. **Every such reach is an ordinary attributed command** in the space's
     chain, so the owner cannot look without leaving a record. The capability is
     checked **at the door**, once per call, before planning, where the space is
     already bound; it is not a per-row disjunct in every derived guard, which
     is what keeps it from deadlocking on the very rows it exists to repair.
     Nothing in phases 1–7 needs to change for this: the door check and the
     chain already exist.
  2. **The consent page for anyone joining a space they do not own must say so
     plainly.** This is the item that is *not* free. §5's invitation and consent
     surface must carry the sentence in the invitee's language, not bury it in a
     policy link: the owner of this space can reach anything in it, and every
     time they do it is logged. Members who need rows the owner cannot reach
     need a **different space**, and the bootstrap answer
     to that requirement is space-per-user (`modeling-guidance.md` §A), not a
     second owner here.
- **Self-serve default [proposed as `off`].** Confirm.
- **The domain `bootstrap` block [proposed].** Grammar sketched in
  `definition-language.md` §8; the compiler questions are unresolved: required-value
  validation, what happens when a package is enabled on an *already existing*
  space (the same rows must be created then, in their own group, with the
  enabling actor as creator), and whether a bootstrap row may be non-singleton.
- **Genesis mode is the developer's choice; `managed` is the default
  [settled].** Two shapes are supported and `policy spaces { genesis … }`
  selects which a deployment offers:

  **A: create managed, then convert (default).** The server performs genesis
  in plaintext and the custody conversion follows as a job. Simplest, reuses
  machinery that already exists, and the server can validate everything it
  wrote. The cost: there is a window in which the
  space existed in managed custody, so "your plaintext never touched our
  server" is not literally true for the genesis rows.

  **C: native client-first genesis.** The device generates the space key
  before the space exists, seals whatever is sealed, and uploads; the server
  validates the parts it can read and stores the rest opaque. This is far more
  tractable than it first appears, because **the genesis group is plaintext
  skeleton at every dial position**: space id, owner membership, roles,
  profile links. None of that is sealed, so the Z-002 "a blind server cannot
  validate" objection does not bite here: the server is not blind to the parts
  it must check. The chicken-and-egg (authenticating to a space that does not
  exist) is already solved by the `scope deployment` op class: the caller
  authenticates to the deployment. Constraints: the caller must be a real
  device holding a key (an agent or server principal cannot stand in), and
  `seed` is refused, because the server cannot validate sealed seed values.

  Rejected: having the **server act as the client**, generating the space key
  server-side, sealing genesis, then handing the key to the first device. It is
  worse than A (the server both saw plaintext *and* held the key) with
  more machinery to justify it.
- **A deployment control-plane audit stream [proposed].** Phase 5 writes an
  audit event that is not a space command and not a space security event.
  `security-logging-design.md` defines both streams per space; a deployment-level
  equivalent is assumed here and does not yet exist in the corpus.
- **Wire rendering of a deployment-scoped op [open].** `api-engine-design.md`
  §2 owns the envelope and currently makes `missing_space` unconditional. It
  needs the `scope deployment` exemption written into §2 and §4's table.
