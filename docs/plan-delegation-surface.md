# The delegation surface — design & build plan

Companion to `definition-language.md` §8 Principals (settled),
`bootstrap.md` §3.3 and item 10, `security-logging-design.md` §5–§6,
`security-hardening-review.md` SH-20, `critique-consolidated.md` X-042, and
`audit-board-coverage.md` row 4.

Markers: **[settled]** restated from a settled document · **[built]** in
`crates/` today, with its file · **[proposed]** adopted here, pending owner
review · **[open]** named and not answered.

The owner's decision this plan sits under, verbatim:

> "Yes this product will offer that as a feature for the builders for ease
> of use/implementation. **MCP by default — though we'll abstract it.**"

So the protocol is a *binding*, not the design. Everything below is written
so that MCP is the first implementation of a seam and never the seam itself.

---

## 0 · Where this starts

### 0.1 What landed with this document

The **record and the door** are built. `audit-board-coverage.md` row 4 read
*"no match for `delegat` in `ordo-identity`"*; there is now:

- `crates/ordo-identity/src/delegation.rs` — `DelegationClaim` (what a token
  asserts), `NewDelegation` / `DelegationRecord` (what is stored),
  `DelegationRegistry` (the durable contract). **[built]**
- `crates/ordo-host/src/delegation.rs` — `PgDelegationRegistry` over a
  `"$delegation"` table, mirroring `PgGrantRegistry`. **[built]**
- The token carries a delegation claim (`v2` payload,
  `crates/ordo-identity/src/token.rs`), and `AccessEngine::check` enforces
  everything decidable from it: the delegation's space, its expiry, and the
  **delegator-authority bound**. **[built]**
- The door's **token gate** (`AccessChecker::token_gate`, answered by
  `IdentityAccess`) asks the registry whether the delegation is still live,
  before any op name or space is consulted, and answers the new
  `delegation_required` wire class when it is not. **[built]**
- `std.space.callersOf` gained a fifth via, `delegation`. **[built]**
- `ordo_host::attribution` produces `Actor::AgentOnBehalfOf` from proved
  claims — the variant existed since the command log was written and had no
  producer. **[built]**
- `crates/ordo-host/tests/delegation.rs`, real Postgres, `ordo_del_*`: eight
  tests, revocation and expiry proved in both directions. **[built]**

### 0.2 What this document plans

Everything between *a builder's agent wants to act for a user* and *a
delegation row exists*. None of it is built:

1. The **consent ceremony** — how a user is asked, what they see, what
   proves it was them.
2. The **abstraction** the owner asked for — one trait, MCP as the default
   binding.
3. **Token minting** bound to a delegation id.
4. **Budgets** (SH-20 / X-042), which are `[proposed]` in the security
   review and have no home yet.
5. The one thing that decides whether delegation is *useful* rather than
   merely *lawful*: **what `$caller` is inside a delegated call** (§7).

### 0.3 What this document is not

Not a plan for capability grants or the consent page that mints them —
those exist (`AccessEngine::grant_for`, `GrantRegistry`), and delegation is
`[settled]` as **distinct** from them. Where the two ceremonies share a
screen they are still two consents, recorded twice, revocable separately.

Not a plan for MCP server implementation details (transport, framing,
capability negotiation). The point of §3 is that those live behind a seam
and can be replaced.

---

## 1 · The settled ground, quoted

`definition-language.md` §8:

> **Agents** are outside callers using what domains expose — registered ops
> (HTTP or MCP) plus trigger requests; no special machinery, identity exists
> so scope and attribution apply. Delegation recorded: `agent:<name> on
> behalf of user:<id>` — and **delegation is itself a grant [settled]**:
> acting on behalf of a user requires that user's own consent (user → agent,
> expiring, revocable, command-logged), distinct from the capability token's
> op allowlist. A manager granting an agent ops never grants it *someone
> else's face*; without a live delegation grant the agent acts only as
> itself.

`bootstrap.md` item 10 and §3.3 (the agent-provisioned path):

> Absent or expired ⇒ `access_denied`. … An agent may create a space owned
> by **its delegator only** — never by a third party. … Acknowledgements
> **cannot be minted by the agent**. Custody and jurisdiction are consent,
> and consent is the delegator's.

`security-logging-design.md` §6 rule 4, in the `callersOf` argument:

> a delegation carries only the delegator's authority.

`security-logging-design.md` §5:

> presence is **not delegable**: capability tokens can never satisfy
> step-up, so step-up ops are structurally human-only.

Four consequences the built code already honours, restated so the plan
below cannot contradict them:

- A delegation is **per space** (SH-20). Two spaces is two consents.
- A delegation and an op allowlist **intersect**; neither adds to the other.
- **Nothing an agent does can satisfy step-up.** A step-up-gated op is
  closed to the agent path *by construction*, and `bootstrap.md` says so of
  space creation in terms: "the agent path is structurally closed — which is
  a legitimate deployment posture, and it should be visible rather than
  discovered."
- The command log names **both** principals.

---

## 2 · The consent ceremony

### 2.1 The shape

Five steps, and the shape is deliberately the OAuth-ish one people already
recognise — the novelty is in *what is shown* (§5), not in the dance.

```
agent                        Ordo                         user (browser)
  │  request delegation ──────►│
  │    space, ops, duration    │
  │  ◄── consent handle ───────│   one-shot, TTL'd
  │  ◄── consent URL ──────────│
  │                            │
  │        (out of band: the agent shows the user the URL)
  │                            │◄── user opens it, signs in ──┐
  │                            │─── the consent page ────────►│
  │                            │◄── passkey assertion ───────┤
  │                            │
  │                            ├─ command group: consent recorded
  │                            ├─ $delegation row
  │                            └─ $capabilitygrant rows (the allowlist)
  │  ◄── delegated token ──────│   (agent redeems the handle)
```

### 2.2 Rulings this plan takes

**R1 — Step-up is required to grant a delegation. [proposed]**
Lending your face is at least as consequential as an export. A delegation
granted from a merely-signed-in session is a delegation a stolen session
cookie can grant, and the whole point of the record is that a *person*
agreed. So the consent completion is registered with
`AccessEngine::require_step_up`, which also means — by the mechanism, not by
a second rule — that **an agent can never grant a delegation**, including
one to itself or to another agent. Re-delegation is closed by the same
sentence that closes everything else.

**R2 — The consent handle is one-shot and TTL'd, on the `CEREMONY_TTL_SECS`
pattern. [proposed]**
`crates/ordo-identity/src/provider.rs` already learned this the hard way:
before the auth ceremonies took a clock, a `state` value observed once
stayed redeemable forever. The delegation handle is the same object and
gets the same treatment — five minutes, consumed on redemption, replay
fails loudly.

**R3 — The agent never sees the user's session, and the user never sees the
agent's token.** The handle is the only thing that crosses, and it is
worthless without the redemption secret the agent held from the start.

**R4 — Consent is one command group, and it is the delegation's
`consent_correlation`. [built, unused]**
`DelegationRecord::consent_correlation` is stored today and nothing writes
it but tests. The ceremony fills it with the correlation id of the command
group that recorded the consent, which is what makes DL's *"command-logged"*
a joinable fact rather than a claim. Done-when: a delegation row's
correlation resolves to a real group in the space's chain.

**R5 — Denial and abandonment are recorded too. [proposed]**
A user who reads the consent page and says no is the most interesting event
on this surface, and an agent that requests fifty delegations and gets none
is a pattern only the refused half reveals. Both go to the security-event
stream, which already has the routing and the collapse window.

---

## 3 · The abstraction: one trait, MCP as the default binding

### 3.1 The shape the codebase already uses

`ordo-identity` has done this once, and the module doc states the rule:

> in-house core (directory, grants, step-up) with **every** authentication
> method behind the `provider::AuthProvider` trait — Keycloak, Google,
> Discord, enterprise SAML are configurations of a provider, never
> architectural forks.

That is precisely the owner's instruction applied to a different problem,
and it is the shape to copy. `AuthProvider` is worth copying in three
specific ways, each of which was a correction somebody had to make:

1. **It takes a clock rather than reading one** — so ceremonies expire, and
   so tests can pin an instant.
2. **Its completion payloads are a closed enum**, and a provider handed the
   wrong variant fails loudly rather than ignoring it.
3. **The one privileged capability lives behind a module boundary, not a
   flag.** `UserPresence` has no constructor outside `passkey`, which makes
   "presence comes only from passkeys" a fact about the code. The delegation
   equivalent: nothing outside the ceremony module may construct a
   `NewDelegation`.

### 3.2 The trait [proposed]

```rust
/// One way of asking a user to let an agent act for them, and of handing
/// the agent back a credential. MCP is the default binding; a plain HTTP
/// binding and a CLI binding are the same trait.
pub trait DelegationSurface: Send + Sync {
    /// The agent asks. Returns a one-shot handle plus the URL the user is
    /// sent to. Does NOT create a delegation.
    fn request(&self, req: &DelegationRequest, now: Timestamp)
        -> Result<PendingConsent, SurfaceError>;

    /// What the consent page must render. Computed, never supplied by the
    /// agent — see §5.
    fn disclosure(&self, handle: &ConsentHandle, now: Timestamp)
        -> Result<ConsentDisclosure, SurfaceError>;

    /// The user said yes, with presence. The ONE place a `NewDelegation`
    /// is constructed.
    fn accept(&self, handle: &ConsentHandle, presence: &UserPresence, now: Timestamp)
        -> Result<Accepted, SurfaceError>;

    /// The user said no, or the handle lapsed.
    fn decline(&self, handle: &ConsentHandle, now: Timestamp) -> Result<(), SurfaceError>;

    /// The agent redeems its handle for the token. Fails until `accept`.
    fn redeem(&self, handle: &ConsentHandle, secret: &RedemptionSecret, now: Timestamp)
        -> Result<DelegatedToken, SurfaceError>;
}
```

**What is deliberately *not* on the trait**, because it must not be a
binding's choice: whether presence is required (R1), what the disclosure
contains (§5), what the expiry ceiling is, whether pins are carried (§6),
and what gets written to the chain. A binding decides how bytes move. It
decides nothing about consent.

### 3.3 The MCP binding [proposed]

MCP's affordances map onto the trait without extending it:

| MCP concept | Ordo |
|---|---|
| tool listing | the **scoped catalog** — `ordo_host::catalog_for`, already the asker's own consent page in machine form (F6) |
| tool call | an op call through the one door, unchanged |
| elicitation / sampling | how the agent surfaces the consent URL to the human |
| server capabilities | which spaces this deployment exposes to agents at all |

Two pins for the binding:

- **The catalog is the tool list, and it is already scoped.** An MCP server
  that built its own tool list would be a second answer to "what may this
  caller do", and the first one is enforced at the door. The binding
  *projects*; it does not decide.
- **No MCP-only op exists.** Everything an agent can reach through MCP is
  reachable through HTTP with the same token and the same refusals. The day
  that stops being true, MCP has become the design.

### 3.4 What the abstraction buys, concretely

A builder shipping an agent on a protocol that does not exist yet needs a
new `impl DelegationSurface` and nothing else: no new consent semantics, no
new record, no new door check, no new wire class, no new audit column. That
is the test of whether the seam is in the right place, and it is the test
to apply to any change to §3.2.

---

## 4 · Token minting, bound to the delegation

`TokenSpec` already carries `delegation: Option<DelegationClaim>`
**[built]**. The ceremony's minting rules:

**M1 — The claim is copied from the record, never composed. [proposed]**
`DelegationRecord::claim()` exists for exactly this **[built]**. A minting
path that assembled a claim from parts could mint one the registry never
recorded.

**M2 — The token's expiry is `min(session policy, delegation expiry)`.
[proposed]** A token that outlives its delegation is not dangerous — the
gate refuses it — but it is a token that appears live and is not, and every
minute of that is a support ticket.

**M3 — The capability grant is minted in the same act, and both must
land.** `GrantRegistry::record`'s doc already says a grant that reaches a
token without reaching the registry is one nobody can revoke; the same is
true here twice over. If either record fails, the ceremony fails and no
token is issued.

**M4 — A delegated token never carries `user_presence_at`. [built]**
`refresh_presence` refuses a token with a delegation claim, and the step-up
gate refuses every capability token regardless. Two independent refusals,
which is right for the one property the platform has no second mechanism
for.

**M5 — Refresh re-checks the delegation. [proposed]** Whatever refresh-token
machinery arrives, a refresh is a mint, and a mint consults the registry.
Otherwise revocation lands at the door and is undone by the next refresh.

---

## 5 · What the consent page must show

This is the part the whole feature is judged on. The audit's framing — *"an
agent's catalog IS its consent page in machine form"* — cuts both ways: the
page is the catalog in **human** form, and it has to be readable by somebody
who does not know what an op is.

### 5.1 Required, all of it computed

| Shown | Source | Why it cannot be omitted |
|---|---|---|
| **Who** is asking — agent display name, and who registered it | directory | "An agent" is not an answer; the principal was declared in IaC by somebody nameable |
| **Which space**, by name | space registry | Delegation is per-space (SH-20). A page that does not name the space is asking for a blank cheque |
| **Which ops**, in prose | `catalog_prose` **[built]** | The op list is the allowlist; `catalog_prose` already renders signatures, costs, guards and key release as English |
| **What each op reads and writes, by field** | the manifest's field-grain scope **[built]** | A field set is the difference between "reads your tasks" and "reads your tasks' sealed notes" |
| **Guard-read fields** — data the platform examines on your behalf and you never receive | manifest | `catalog_prose` already says this in as many words, and it is disclosure |
| **When it expires**, as a date | the ceremony | "Expiring" is settled; a page that does not show the date has not disclosed the term |
| **Your own reach is the ceiling** | computed | The delegation cannot exceed what *you* can do — the strongest reassurance available, and it is true (`DelegationBeyondDelegator` **[built]**) |
| **What it cannot do**: step-up ops, ever | computed | Naming the closed door is what makes the open one legible |
| **How to revoke, and where** | static | DL calls it revocable; a page that does not say where is describing a feature the user cannot find |
| **The owner-visibility disclosure**, if the user is not the space owner | DL §8 | Already required for joining a space — *"the consent page … must say so plainly"* — and it is at least as relevant here |
| **Budgets**, once they exist | §6 | X-042 requires them "derived and displayed on the consent page" |

### 5.2 The argument on pins, and the recommendation

**The question.** A capability grant pins, *per op*, the manifest hash, the
field set and the row-scope shape at consent time; drift that **expands**
scope kills the grant until re-consent, and narrowing applies silently
(`AccessEngine::grant_drift`). Should a delegation carry pins the same way?

**The case for.** Symmetry, and the escalation is real in the same shape: I
consent to an agent acting as me *while it can do these things*; if those
things quietly grow, my consent was to something else. Scope expansion
killing consent is the property the pinning work exists to deliver.

**The case against, and it is the stronger one.** *A delegation has no scope
to pin.* Its content is one person and one space. The ops are in the
capability grant — which pins them already, per op, and is minted in the
same ceremony. Adding pins to the delegation record would mean:

- **Storing the same pins twice.** Two records that must agree and can
  drift; the first bug is one of them being updated on re-consent.
- **Two answers to one question.** When the deployment changes and the pins
  drift, does the *grant* die, the *delegation* die, or both? Three
  behaviours, one user-visible event, and no way to explain the difference
  on a consent page.
- **Killing the wrong thing.** A delegation is a relationship; a grant is a
  scope. Ending the relationship because the scope moved is a category
  error, and it makes re-consent a step-up ceremony rather than a
  re-approval.

**Recommendation [proposed]: the delegation does NOT carry op pins. The
grant minted alongside it does, exactly as today.** Scope expansion kills
the grant, the delegated token stops working (`ManifestChanged`), and the
user re-approves the *scope* — while the *relationship* survives, which is
both correct and much better product.

**But it pins one thing the grant cannot: the delegator's reach at consent
time.** The bound is enforced live (`DelegationBeyondDelegator` **[built]**)
and that is right — a promoted user's agent should not silently inherit the
promotion, and a demoted user's agent must lose the reach immediately. The
live check gets the second half. The first half needs a record: store the
delegator's **role and effective op set at consent time**, display it, and
raise an audit note (never a denial) when the delegator's reach has *grown*
since. That is the narrowest form of "scope expansion is visible" that does
not duplicate the grant's job. **[proposed]**

### 5.3 Two things the page must not do

- **No pre-checked boxes, and no "select all".** The allowlist is
  *enumerated by a human at grant time* (DL §8's own words about capability
  tokens). A page that makes granting everything the cheapest gesture has
  complied with the letter.
- **No agent-supplied prose.** Every string on the page is computed from
  the deployment. An agent that could write its own description could
  describe `orders.deleteAll` as "keeps your inbox tidy" — and this is a
  surface whose whole threat model is a prompt-injected agent.

---

## 6 · Budgets — SH-20 / X-042, still open

The security review's pin, unbuilt and now placeable:

> Delegation grants carry **budgets, derived and displayed on the consent
> page**: rate and volume caps per delegation (calls/hour and rows
> affected/hour), exceeding is a typed `delegation_budget` denial plus a
> security event; destructive ops non-delegable by default, opt-in per op
> listed individually; delegation is per-space, never per-principal-globally.

Status against what is built:

| SH-20 clause | Status |
|---|---|
| per-space, never global | **[built]** — `DelegationClaim.space`, `DelegationSpaceMismatch` |
| expiring, revocable | **[built]** |
| rate/volume caps | **[open]** — nothing counts |
| `delegation_budget` wire class | **[open]** — must NOT be minted before it has a producer; the inventory is closed in both directions (`ordo-api/tests/inventory_is_closed.rs`) |
| destructive ops non-delegable by default | **[open]**, and the cheapest of these to build: it is a set on the engine plus one check, in the shape `require_step_up` already has |
| dry-run for delegated bulk writes | **[open]** |

**Recommendation [proposed]:** build *destructive-ops-non-delegable* with the
consent ceremony (it is a consent-page item — "this agent may not delete"),
and defer rate/volume caps until the metrics substrate can answer "rows
affected per hour per delegation", which it cannot today
(`ordo-host::metrics`). A budget that is displayed and not enforced is worse
than no budget.

---

## 7 · The open question this plan does not close: `$caller`

**Today, a delegated call binds `$caller` to the *agent*.** The door
authorizes against the delegator's reach; the executor's row guards still
see the agent. `AccessDecision::Allow { principal }` is what the router
copies into the reserved `$caller` parameter, and that is the agent's id.

**Why that is a problem.** Most guards in this language are `$caller`-shaped
(`owner == $caller`, `project.projectMembers(member == $caller)`). An agent
delegated by Ada, calling an op Ada can call, over rows guarded by Ada's
membership, sees **nothing** — because it is not Ada at the row layer. The
delegation is lawful and empty.

**Why it was not changed here.** `$caller` is reserved and router-inserted
(R-G13, GC067–068); the pinning machinery records a `caller_restricted`
flag per entity; the idempotency cache and the page cursor are both keyed on
the principal the door resolved (SH-17). Changing what `$caller` means for
one class of call touches every one of those, and each needs its own
argument and its own conformance case. Doing it inside this increment would
have been a large semantic change riding on a smaller one.

**The three candidate answers.**

1. **`$caller` = the agent** (today). Honest, useless for guarded reads, and
   quietly so — which is the worst property, because it fails as an empty
   result rather than a refusal.
2. **`$caller` = the delegator.** Matches §6 rule 4's *"a delegation
   carries only the delegator's authority"* exactly, and makes the feature
   work. Costs: the idempotency namespace becomes the delegator's (arguably
   right — same authority), cursors likewise, and every guard-conformance
   case involving `$caller` needs a delegated twin.
3. **Both, distinguished** — `$caller` is the delegator and a second
   reserved `$actor` is the agent, so a guard can say "not by an agent". The
   most expressive and the most surface; it adds a reserved name to a
   language whose reserved names are load-bearing.

**Recommendation [proposed]: (2), with (3) available later.** `$caller`
becomes the delegator for a delegated call, the command log keeps naming
both (it already does), and `$actor` is added only when a real guard wants
to discriminate. The door already enforces the matching rule, so (2) is a
row-layer change that the authorization layer is already consistent with —
which is the whole reason the bound was built first.

**Done-when for (2):** a delegated agent calling a `$caller`-guarded read
sees exactly the rows its delegator sees, no more; a guard-conformance case
proves the "no more" half; the idempotency key's principal component is
documented as the delegator's, with the sharing between two agents
delegated by one user stated rather than discovered.

---

## 8 · Build order

Each increment is shippable and leaves the tree green. Done-whens are
assertions somebody can run, not descriptions.

### Increment 1 — the record and the door · **landed**

See §0.1. Done-when, met: a revoked delegation refuses on the next call and
an expired one refuses, both against real Postgres; a live one passes and
`ordo_host::attribution` produces DL §8's exact attribution string, round-
tripped through a real chain; `callersOf` grows a `delegation` via that
disappears on revocation.

### Increment 2 — `$caller` under delegation · **landed** (2026-10-08)

Built as option (2). What landed, and the keying decisions:

- `$caller` is the delegator's member id. The agent rides beside it as the
  reserved `$agent`, read only where the command group's actor is built.
- Writes and blob uploads record `agent:<agent> on behalf of user:<delegator>`.
- **Replay cache and page cursor: keyed on the delegator plus the agent**
  (`<delegator>+agent:<agent>`). A person and each agent acting for them have
  separate namespaces, so no agent replays a response stored by the person or
  by another agent. Lead's decision, 2026-10-08, on the board for the owner.
- Increment 6's first clause landed with it: deletes are closed to delegated
  tokens unless `ORDO_DELEGABLE_OPS` names them, and platform writes are
  closed outright (`crates/ordo-host/src/delegable.rs`). Refused as
  `OpNotDelegable` (security tag 31), answered `access_denied`.
- The deployment surfaces, `/security/events`, the catalog and the interface
  descriptor now run the token gate, so a revoked delegation reaches none of
  them (a defect found while building this).
- Tests: `crates/ordo-host/tests/delegation.rs`; GC101 and R-G22 in
  `guards-conformance.md`. Guide: `howto-workers.md`.

§7, recommendation (2). **First**, because everything after it is polish on
a feature that otherwise returns empty result sets.

Done-when: a delegated read over a `$caller`-guarded entity returns the
delegator's rows and not one row more; a guard-conformance case covers the
"not one row more"; `docs/guards-conformance.md` carries the rule; the
idempotency and cursor keying decisions are written down.

### Increment 3 — the ceremony, with no protocol binding · **landed** (2026-10-10)

Built as checklist 11-4b: `crates/ordo-host/src/consent.rs`, the HTTP door
as the first binding, tests in `crates/ordo-host/tests/delegation_consent.rs`,
guide `howto-delegation.md`. Two departures, stated there: the consent is
recorded as a security event whose correlation is `consent_correlation`
(a consent writes no tenant row, so no command group), and presence is the
door's step-up gate ahead of `accept` rather than its parameter.

`DelegationSurface` (§3.2) plus one in-process implementation, driven by
tests. The command group (R4), the security events (R5), step-up on accept
(R1), the one-shot TTL'd handle (R2), and `NewDelegation` constructible
only from `accept` (§3.1.3).

Done-when: a test drives request → disclose → accept-with-presence → redeem
and gets a working delegated token; accept without presence refuses; a
replayed handle refuses; a declined consent leaves no row and one security
event; the delegation's `consent_correlation` resolves to a real group in
the space's chain.

### Increment 4 — the consent disclosure · **landed** (2026-10-10)

`std.delegation.review`, computed by `Ceremony::disclose`. The
delegator's-reach-at-consent-time record (§5.2) is not built; the live
bound stands, and each op says whether the person can call it.

`ConsentDisclosure` computed from the catalog and the manifests (§5), and
the delegator's-reach-at-consent-time record (§5.2). No HTML — the
disclosure is a value; rendering is the client's.

Done-when: the disclosure names every field every listed op reads or writes
including guard-read fields; it names the expiry as an instant; it names the
step-up exclusion; it carries the owner-visibility disclosure when the
delegator is not the owner; a test asserts no field of it is
agent-supplied.

### Increment 5 — the MCP binding

`impl DelegationSurface` over MCP, plus the tool-list projection from
`catalog_for`. The pins in §3.3 are the acceptance criteria.

Done-when: an agent completes the whole ceremony over MCP; the tool list is
byte-equal to the scoped catalog's projection for the same token; a test
asserts no op is reachable over MCP that is unreachable over HTTP with the
same token.

### Increment 6 — destructive-ops-non-delegable, and the budget floor

§6's cheapest clause, plus whatever the metrics substrate can honestly
count by then.

Done-when: an op registered destructive is refused to a delegated token
even when the allowlist names it; the consent page says so; the refusal is
its own recorded decision; `delegation_budget` is minted **only if** a
producer lands with it.

### Deferred, deliberately

- **Re-delegation** (an agent delegating onward). Closed today by R1's
  mechanism rather than by a rule, which is the better kind of closed.
  Opening it needs a depth bound, a chain of consent in the attribution
  string DL pins, and an answer to "who revokes the middle link" — a
  different design.
- **Cross-space delegation.** Two spaces is two consents, and the tier-2
  service-principal bridge already covers the automated case.
- **Delegation to a human.** The record does not care, and the door does not
  check the principal's kind. Nothing needs it yet, and the consent page
  copy would be different enough to be its own design.

---

## 9 · Risks named rather than discovered

- **The registry is in the request path.** Stated in
  `ordo_identity::delegation` and argued there; the failure mode is that a
  registry outage refuses delegated calls and nothing else. It wants a
  bounded per-node cache with a revocation-propagation window that is
  *stated on the consent page* ("revocation takes effect within N seconds")
  rather than a promise of instantaneity nobody measured. **[open]**
- **The prompt-injected agent is the threat model**, not the malicious
  builder. Every disclosure rule in §5 and the intersection bound in §1 are
  aimed at it. The residual: an agent talked into calling an op *its
  delegator legitimately holds* is indistinguishable from one that meant
  to. Budgets (§6) and the `DelegationBeyondDelegator` security event are
  the only signals, and neither is an answer.
- **Two live delegations for one pair are unrepresentable**, which is what
  makes the chain-to-registry join exact — and the read-committed window
  where two simultaneous consents could both insert is stated in
  `ordo_host::delegation`. It wants SERIALIZABLE on the consent path.
  **[open]**
- **A delegated token is a bearer token.** Everything true of the others is
  true of this one, with the aggravation that it wears a person's name.
  Short expiries (M2) are the mitigation available today.
