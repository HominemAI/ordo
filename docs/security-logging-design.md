# Ordo Security & Logging Design

Companion to `definition-language.md` (§8 platform blocks, §6 compilation &
atomicity) and `implementation.md`. This document resolves the identity
**[open]** item in `implementation.md` and pins the design behind the two
platform crates now building: `ordo-identity` and `ordo-commandlog`.

Markers: **[settled]** restated from the spec, or ruled by the owner and
recorded here · **[pinned]** adopted here to unblock the build, each a small
change if vetoed · **[proposed]** a gap filled here, not yet ruled on.

---

## 1. Ruling: in-house core, every method behind a provider trait [pinned]

The identity [open] item resolves as: **in-house core** (principal directory,
sessions, capability tokens, step-up) with **all authentication methods behind
provider traits**. Keycloak, Ory, Google, Discord, an enterprise SAML IdP:
every one is a *configuration* of a provider implementation. No IdP sidecar exists.

Rationale (per `implementation.md`'s lean): invite-gating, capability tokens,
and the directory are custom regardless; a sidecar dilutes the one-binary
story; and the trait boundary makes "which IdP" a deployment decision with
zero code motion. It is the same resolver pattern as `KeyProvider`, databases, blob
storage, and routine execution. No provider is hardcoded anywhere.

The split is exact:

- **Core (in-house, always present):** who exists (directory), who is signed
  in (sessions), what synthetic callers may do (capability tokens), when a
  human must re-prove presence (step-up), what everything did (command log).
- **Providers (pluggable, per-deployment):** how a human proves who they are.
  A provider's only job is to turn a login ceremony into an
  `AuthenticatedSubject`, a stable authenticator identifier plus the method
  used. Everything after that instant is core and method-blind.

## 2. Provider trait family

One trait, five implementations. `begin` starts a ceremony (a challenge to
answer or a redirect to follow); `complete` finishes it or fails loudly.

| Provider | Covers | Notes |
|---|---|---|
| `LocalPasswordProvider` | username/password, email-OTP | argon2id behind a `PasswordHasher` trait (hash algorithm is swappable; the transparent test hasher is `#[cfg(test)]` and no longer nameable by a deployment); email-OTP reuses the platform email channel (§8 notifications) as a login factor and as password-reset proof |
| `PasskeyProvider` | passkeys (WebAuthn) | RP id = the deployment's registrable apex domain [settled] and is **deployment configuration** (`ORDO_APEX_DOMAIN` [proposed]). Changing it strands every enrolled authenticator. It is **the step-up source**: user-presence assertions come only from here. Verification is **in-house** (no `webauthn-rs`) [pinned; see below] |
| `FederatedProvider` (OIDC) | Keycloak, Google, Discord, any OIDC IdP | authorization-code + PKCE, `state` + `nonce` enforced, one-shot; issuer/client-id/secret are config. **Keycloak is a row in the auth block and is not a dependency** |
| `SamlProvider` | enterprise IdPs (Okta, ADFS, Shibboleth) | SP role only; SP-initiated flow; signed assertions required |
| `OAuth2Provider` | OAuth-only providers without OIDC discovery | generic authorization-code; subject identity from a configured userinfo claim |

The spec's `auth { methods … }` block maps 1:1 onto a **ProviderRegistry**:
method name → provider instance. Default-on methods (passkey, password,
email-otp) and opt-in federated methods (`method google { client: … }`) are the
same mechanism at different defaults [settled].

```
auth {
  methods  passkey, password, email-otp
  method   google   { client: $GOOGLE_CLIENT }            # OIDC config
  method   keycloak { issuer: $KC_ISSUER, client: $KC }    # OIDC config
  method   corp-sso { saml: $IDP_METADATA }                # SAML SP config
}
```

Trait shape (as implemented in `ordo-identity`; async arrives at the HTTP
boundary later; the trait is deliberately synchronous until the platform
binary exists):

```rust
trait AuthProvider {
    fn kind(&self) -> AuthMethodKind;
    fn begin(&self, req: &BeginRequest, now: Timestamp) -> ChallengeOrRedirect;
    fn complete(&self, resp: &CompleteRequest, now: Timestamp)
        -> Result<AuthenticatedSubject, AuthError>;
}
```

**The trait takes the server's clock** [pinned; corrects the earlier shape,
which had none]. Without it a ceremony handle has no notion of time, and the
first build of these providers accordingly held its pending challenges and
OIDC `state` values for the life of the process. One-shot was enforced;
one-*time* was not, so a `state` observed once (in a referrer header or a proxy
log) stayed redeemable indefinitely. Every ceremony handle now expires at
`CEREMONY_TTL_SECS` (300s, the step-up window), and an expired handle is
indistinguishable from one that never existed.

Anti-enumeration [pinned]: `begin` always issues a challenge whether or not
the identifier exists; unknown identifier and wrong credential are the same
`BadCredentials` error. Challenges, OIDC `state`, and SAML request ids are
one-shot, so replay is `UnknownChallenge`/`StateMismatch`, never a second try.

### 2.1 The passkey provider, and where Ordo's responsibility ends [pinned]

The provider verifies; it does not run the ceremony. The browser call, the
user gesture and the authenticator are the deployment's, and no test in this
repository pretends otherwise. What Ordo owns, and now implements and tests in
full, is the server's half:

- challenge binding (server-minted from the OS CSPRNG, one-shot, time-boxed,
  and typed, so a registration challenge cannot answer a sign-in);
- RP-id binding (`SHA-256(rp_id)` compared against `authenticatorData`);
- origin binding against a validated allowlist, the check that makes a
  phishing page's genuine signature worthless;
- the **user-presence flag**, which is the entire physical basis of step-up;
- user verification, `required` by default;
- the signature over `authData ‖ SHA-256(clientDataJSON)`;
- the signature counter, whose regression is the one clone signal WebAuthn
  offers and is named as such in its refusal.

**Why not `webauthn-rs`** [pinned; the library was a parenthetical in the row
above, never a marked ruling]: it brings 223 transitive crates and a native
OpenSSL link into a workspace whose largest crate has eleven, and what it adds
over the list above is attestation-statement verification, which this
deployment deliberately does not perform. `ordo-identity` accepts `fmt: none`
only. Attestation identifies the authenticator *model*; verifying it is how an
RP enforces a hardware allowlist and is also a device-fingerprinting surface.
Ordo has no hardware policy, so it has no use for a statement it would not read. Enterprise attestation is a deployment policy with its own
metadata-service dependency and belongs to whoever wants it.

Advertised algorithms are **ES256 and EdDSA**. An authenticator only ever
picks from `pubKeyCredParams`, so that list is also the exhaustive set of what
can arrive; RS256 and everything else is refused by name.

### 2.2 Presence is a type, not a field [pinned]

§5 says presence is not delegable and this section says it comes only from
passkeys. Both are now enforced by construction:
`UserPresence` has private fields and no constructor outside the passkey
module, so no other provider can put one on an `AuthenticatedSubject`, because it has
nothing to put there. The stamp travels one path and every link in it is a
type:

```
PasskeyProvider::complete  →  AuthenticatedSubject::presence
  →  SignIn::token_spec    →  TokenSpec::user_presence_at
  →  TokenClaims::user_presence_at  →  StepUpRequirement
```

The **step-up refresh** (`session::refresh_presence`) is the same walk inside
an existing session, and it carries the one obligation the provider cannot:
the asserting authenticator must be linked to *this session's* principal. An
attacker holding a stolen bearer token can always run a valid ceremony with
their own passkey; the assertion verifies and the presence is genuine and it
belongs to somebody else. Neither the provider (which knows credentials, not
principals) nor the token layer (which minted long ago) can catch that, so it
is caught there.

### 2.3 Where presence is required is a deployment setting [settled]

Earlier text in this document, and the platform, fixed which operations
demand a fresh passkey assertion: every platform op did, and the reasoning in
§5 and §6 (a delegation carries only the delegator's authority, so step-up ops
are structurally human-only) was written as if that were the platform's rule.
That is the default and not a platform rule.

Owner, 2026-09-29, recorded in `docs/corpus-rulings.md` under "Two sign-in
details, in conversation". About the membership and admission ops: *"THis is
not ours to decide - that is for the developer."* Then, the same day, about all
of the platform ops: *"What is enforced is app design ... not our design."* So
**which platform ops require presence is the app developer's setting**, for
every platform op, including `std.deployment.apply`, `std.space.create`,
`std.space.import`, `std.space.export` and `std.space.delete`.

What the platform still owns is unchanged and is what §2.2 pins: step-up is
the one mechanism for "not delegable", presence is minted only by a verified
passkey assertion (`UserPresence` has no other producer), and the freshness
window is the engine's. The setting says only *where presence is required*.

The setting is `ORDO_STEP_UP_EXEMPT_OPS`, a list of the platform ops that do
**not** require presence (exact names, or a prefix ending `.*`). **Its default
is empty: presence is required on every platform op**, which is what every
"step-up gated" statement in this document describes. An exempt op keeps its
role gate, and it is one a capability token may be granted, since nothing about
it is structurally human-only any longer. An entry that names no platform op
refuses the boot. Code: `crates/ordo-host/src/presence.rs`; deployment
reference: `docs/deploying.md`.

## 3. Account linking & provisioning

**One human principal, many linked authenticators** [settled]. The directory
holds `authenticator id → principal id` links; authenticator ids are stable
per method (`password:<user>`, `passkey:<credential-id>`,
`oidc:<method>:<subject>`, `saml:<method>:<name-id>`).

- **Link/unlink are audited commands.** They emit audit events into the
  command log (actor: the linking user, or `system:identity` for
  provisioning). Permission history and authenticator history read the same
  way: from the log.
- **Unlinking a human's last authenticator is refused** [pinned]. Lockout is
  not an operation. Account closure is a different, deliberate command.
- **First-login provisioning is per-deployment policy** [pinned]:
  `invite-only` (default; an unknown authenticator completing login is an
  error) or `auto-onboard` (a new human principal is minted and linked on
  first completion; the onboarding is itself an audited event). The bootstrap
  admin arrives via IaC [settled]; everything after that is runtime state.

## 4. The platform as downstream issuer [settled]

One deployment, one OIDC issuer. Horizontally scaled pods are one logical
platform. Whatever the upstream ceremony (password, passkey, Keycloak, SAML),
completion converges on the same point: resolve the authenticator to a
principal, mint a platform session, issue platform tokens. Identity clients
(`client app-web { … }`) are relying parties of **this** issuer only; no
client ever sees an upstream token, and upstream tokens are never stored.
Only the authenticator linkage survives the ceremony.

## 5. Token model

- **Human sessions:** short-lived access token + refresh token, lifetimes per
  client config (`tokens access 15m, refresh 30d`) [settled]. Access tokens
  carry principal, client, and step-up freshness; refresh rotates.
- **Capability tokens** (agents, sources, API clients): synthetic callers
  never inherit role defaults [settled]. The token carries a **mandatory
  space**, an explicit **op-name allowlist** enumerated by a human on the
  consent page, **per-op pinned manifest hashes and field sets**, and a
  **mandatory expiry** [pinned: no unexpiring grants].
  **A grant is scoped to exactly one space** [pinned; closes the hole where a
  grant carried no space dimension at all]. `CapabilityGrant` has a required
  `space`; there is no deployment-wide grant and no "the token's space is
  wherever it is used". Op names are deployment-wide strings
  (`std.transactions.list`, `finance.setCategory`) and space ids leak through
  invitations, share links, URLs, and support tickets, so a space-blind grant
  is a deployment-wide read for anyone who learns one id. A grant spanning two
  spaces is **two grants, consented twice**, which is already what
  `definition-language.md` says about the tier-2 bridge ("capability tokens for
  both spaces, consented on each side"). Sources declared in IaC name their
  space in the declaration.
  **Per-op, not combined** [pinned; corrects an earlier self-contradiction]:
  a single *combined* hash cannot distinguish expansion from narrowing, because
  **narrowing changes the hash too**. So the rule "expansion kills the grant,
  narrowing applies silently and audit-noted" was not implementable as written
  because hash equality is a yes/no, and the two outcomes it has to separate both
  produce "no". The decidable form, needing no registry history:
  - Pin **per listed op**: its manifest hash **and** its ordered output field
    set at consent time.
  - On call, compare per op: hash equal ⇒ unchanged. Hash differs ⇒ compare
    field sets: **subset** = narrowing (applies silently, audit-noted);
    **superset or disjoint** = expansion, and the grant is **dead until
    re-consent**. An op removed from the deployment is expansion-equivalent
    (the grant no longer means what was consented to) and kills it too.
  - This is also why generated code pins **per-op** version hashes rather than
    a registry-wide one (`api-engine-design.md` §10): one granularity, used by
    grants, skew headers, and the consent diff alike.
- **Step-up** (proof of fresh presence) is demanded **per op**
  (`stepUp [transfer.approve, space.delete]`) [settled]. A client may add or
  tighten a requirement and never subtract one: requirements combine by
  strongest, and the deployment window is a ceiling. See
  `definition-language.md` §8. A fresh-presence
  claim rides the access token with a short freshness window. Presence is
  **not delegable**: capability tokens can never satisfy step-up, so step-up
  ops are structurally human-only [pinned].
- **Sources are mail slots** [settled]: write-only, authenticated per sender,
  scoped to event kinds, modeled as capability tokens whose allowlist is
  write ops only.

## 6. Access check: one check at the door [settled]

Exactly the spec's pipeline, once, before planning:

```
token → principal → ( membership role + group memberships
                       → per-space access profile )
                    OR ( capability allowlist )        → op included?
```

- Human path: membership role in the space (owner/manager/editor/viewer) **and
  the principal's group memberships in that space** → the space's access
  profile (role → op allowlist, group → op allowlist, plus per-member
  overrides; template-seeded, then space-owned runtime data) → is the op listed
  for that principal?
- **Group resolution [settled].** Groups are part of the access model
  (`definition-language.md` §8), and they touch this pipeline in four exact
  places:
  1. **Resolution is per space and non-transitive by default.** A principal's
     effective grant set is `((role allowlist) ∪ (allowlist of each group they
     belong to in *this* space))` **then** per-member overrides applied on top.
     Overrides both add and subtract, so they are a second stage, never a
     third term of the union (a union cannot subtract; the crate implements the
     two-stage form). Nested
     groups are **out of v1**. A group whose members are groups is where
     "who can call X" stops being one query and starts being a graph walk, and
     that is a deliberate later decision.
  2. **Union, then overrides.** Group grants add; per-member overrides are
     applied last and can subtract. **Where a direct role and a group-conferred
     role differ, the effective role is the higher of the two [settled]**:
     owner > manager > editor > viewer. It is stated here because nothing else in the
     corpus ruled it, and a group that could *lower* someone's role would make
     group membership a demotion vector. Deny-by-override beats allow-by-group, so
     there is exactly one order and it is written here.
  3. **Group membership changes are commands**, like role and profile changes,
     so permission history stays auditable and `?live` invalidation reaches
     guarded reads (a membership write invalidates the queries whose guards
     read it).
     **Built**: per-space profile and group resolution is now built by the
     host (`crates/ordo-host/src/profile_ops.rs`, `access_template.rs`). A
     tenant's profile is the app's `ORDO_ACCESS_TEMPLATE` with the tenant's own
     lines laid over it. Every change is a command group in the tenant's chain.
     See `howto-roles.md`.
  4. **`callersOf` stays exhaustive.** `std.space.callersOf($op)` must enumerate
     principals reached *via group*, with the group named as the *via*
     alongside role, override, and token. A group grant that does not show up
     there silently breaks the model's best property.
     **Built**: `ordo_identity::AccessEngine::callers_of` plus
     `ordo_host::platform_ops` (§8r). Three rulings the build had to take, all
     of them about the standard this rule sets:
     - **Every via, not the first.** A principal reached by role, by group and
       by override is three rows. The question behind the op is usually *"how
       do I stop them calling it"*, and an answer naming one of three grants
       has a manager revoke it and find the caller still there.
     - **The subtractions are part of exhaustiveness.** A per-member deny
       removes a principal entirely (with every via at once), and a suspension
       removes them *including their live capability grants*. The door checks
       status before it looks at a token, and the grant registry cannot know
       that. Listing a caller the door refuses is the same class of wrong as
       omitting one it admits.
     - **A fifth via, `delegation`, added 2026-09-06** when delegation grants
       were built (`corpus-rulings.md`, `plan-delegation-surface.md`). The
       four words are closed on purpose, and the argument for the closure is
       the argument for this one: a delegated agent is removed by the
       **delegator** withdrawing consent (a different act by a different
       person), and editing the profile or revoking the token does not remove it. Under four words it would be access nobody can find, and
       under `token` it would send a manager to revoke a grant and find the
       caller still there. It is **conjunctive** with `token` (emitted only
       for an agent that also holds a live grant listing the op, because the
       door requires both), and it is emitted **alongside** the token row
       rather than instead of it.
     - **It is an enumeration surface, so it needs its own floor.** Manager and
       owner, listed explicitly (roles have no hierarchy), and **step-up
       gated**. This is where §5's *"capability tokens can never satisfy
       step-up"* does the load-bearing work: the answer contains other
       principals' consent records, and a delegation carries only the
       delegator's authority. The `$op` argument is scoped to the asker's own
       catalog, and an op they may not call answers identically to one that
       does not exist. **No new refusal class was minted**, deliberately: a
       class meaning *"you are not a manager"* would itself tell a prober
       which wall they hit.
     One boundary is left open: a grant's **manifest pin** is not
     re-checked, because a per-op grant record carries the combined hash but
     not the op set it was computed over. A pin-drifted grant is therefore
     listed and would be refused at the door. That is a false positive, the
     safe direction here (the manager revokes a token that had already stopped
     working), and it is not the failure this rule exists to prevent.
  - **Row-level guards see groups too**, as one more hop in the `$caller`
    walk; that is the engine's side of the same feature
    (`engine-plan.md` P2/P3, `std-library.md` §5.1). The door decides *whether
    the op runs*; guards decide *which rows it sees*; groups appear in both and
    mean the same thing in both.
- Capability path: **grant's space == the door's bound space** → grant not
  expired → per-op manifest hashes match (§5) → op in the allowlist?
  **The space comparison is first**, before expiry and before the hash
  comparison [pinned]: it is the cheapest check and the isolating one, and
  ordering it first means no cross-space call ever reaches the machinery that
  could accidentally admit it. **Capability tokens never fall through to roles
  or groups**; the two paths are exclusive by token shape.
- **The space the two paths check is the space the door bound**, never one
  re-derived downstream. It is bound from the request before the access
  decision (`api-engine-design.md` §2, `$space` / `X-Ordo-Space`) and is
  carried explicitly into the executor as its own parameter, where the executor
  asserts it matches. An executor that re-derives the space from bound params,
  from the token, or from ambient configuration reintroduces exactly the
  cross-space read this check exists to prevent; see `api-engine-design.md`
  §12.
- Step-up is checked after inclusion, so a denial names the real gap first
  [pinned]: an op both unlisted and step-up-gated denies as unlisted.
- **Every denial carries the attempted op** in the record [settled]; what it
  answers **on the wire** is governed by the non-enumerability rule
  (`api-engine-design.md` §4.1): an op the principal may not call and an op
  that does not exist produce the **identical** door answer, `access_denied`
  (403), with the same status, code, and message. `access_denied` is the
  door's word for *every* reason it refuses; `access_guard` is the executor's
  word for a row-level rejection after the door opened, and the two are never
  interchanged. (`definition-language.md` G18 names the *decision*;
  `api-engine-design.md` §4.1 owns the wire rendering of it.)
- **Denials never enter the command log** [settled]. No state changed, and
  the command log records what happened, not what was refused. The consequence
  that ruling used to leave implicit is now answered:
  **the command chain answers "who did what"; a separate per-space
  security-event stream answers "who tried what"** (§6.1). Two obligations
  follow from the split, and neither is optional:
  - **Deny volume per principal per space is a mandatory alert.** It is not a log
    line someone might grep. Every disclosure oracle in the security review is
    a *high-call-count* pattern. The metric, its retention, and its query
    surface are pinned in `implementation.md` (Observability); the
    operator-facing surface is in `dashboard-plan.md` §3. Metrics are the
    *alerting* substrate; §6.1's stream is the *tamper-evident* record. They
    are different artifacts with different retention and neither replaces the
    other.
  - **The tamper-evident denial record is §6.1**, and it is settled here
    and not proposed elsewhere. `dashboard-plan.md` §5b's earlier ruling
    ("a denial is a chain event", appending to the space's **command** chain)
    is **superseded**; `engine-plan.md` P5's "denials do not enter the chain"
    is correct about the command chain and is now scoped to say so.
- Roles have no implicit hierarchy [pinned]: a profile lists ops per role
  explicitly. "Owner sees everything" is how templates ship, not an engine
  rule: managers tighten and the engine obeys. **Groups have no hierarchy
  either**. No group inherits another's grants, for the same reason and with
  the same benefit: the effective set is a union of listed things, computable
  in one query.

### 6.1 The security-event stream: where denials are recorded [settled]

Three documents used to disagree about where a denial goes: a chain event, an
operational log line, or nothing. One answer, owned here, referenced everywhere
else:

**Every space has a second append-only, hash-chained stream, its
security-event stream, separate from its command log, with its own head, its
own checkpoints, its own retention, and its own append path.** A space owner
reads both; `verify()` verifies both, independently.

**Why not the command chain.** Four reasons, each fatal on its own, and they
are what killed the "denial is a chain event" reading:

1. **An outsider must never write to a space's record.** The space is bound
   *before* the access check (it has to be, because the check needs it), so a caller
   who is not a member fails *because* they are not a member. If that denial
   appended to the space's chain, any unauthenticated stranger could pick a
   victim space id and a registered op name and grow that space's audit spine
   at will.
2. **It is a write-path denial of service.** The command chain's append is a
   per-space serialization point under an advisory lock
   (`implementation.md`, write path). A denial that takes that lock lets a
   caller with **no** authority in a space contend with every legitimate
   writer in it.
3. **"Rate-collapsed with a mutable count" is not expressible in an
   append-only structure.** You cannot increment a counter inside an entry
   that has already been hashed.
4. **"Entries age out" is not expressible in a hash chain either.** Removing a
   link breaks recomputation at the first gap, so denials in the command chain
   would either never age out (permanent growth in someone else's space) or
   break `verify(space)` for every space that ever received one.

**The rules, exactly.**

- **Members only.** A denial appends to a space's stream **only when the
  denied principal has a relationship with that space** (a members row, or a
  live capability grant scoped to it, §5). A denial of a non-member, of an
  anonymous caller, or of a token naming a space it has no relationship with
  is a **deployment** event: it lands in the deployment's operational security
  log, where the operator's abuse alerting already lives, and never touches the
  space. This is the load-bearing rule; everything else is bookkeeping.
- **What an entry carries**: space, principal, principal kind, attempted op,
  the **decision** (not the wire code), the instant, and, for collapsed
  windows, first instant, last instant, and count. **Never the attempted
  arguments**: a denied caller must not be able to write chosen plaintext into
  a record others read, and params may carry the very values they were refused.
- **The decision is recorded, not the wire code** [pinned]. The wire is
  deliberately uniform under non-enumerability, so the record must not be:
  `not_found`-because-guard is a distinct recorded decision from
  `not_found`-because-absent, and `access_denied`-for-an-unregistered-name from
  `access_denied`-for-a-forbidden-op. This is the entire reason the two
  surfaces are separate: the caller learns nothing and the owner learns
  everything.
- **Collapse key is `(space, principal, op)`**, over a short window. **One
  entry is emitted at window close**, with the count inside the hashed body,
  so nothing is ever mutated after hashing. Nothing is appended while the
  window is open: the open window is ordinary mutable operational state (an
  accumulator, not a chain entry), and only its closed summary is ever hashed.
  Both the space and the op are in the key, so a principal firing one op at ten
  thousand spaces produces ten thousand keys and cannot merge them. That was
  the second defect in the `(principal, op)` version.
- **Per-space, per-window entry cap.** Overflow collapses to a single
  "N further denials suppressed" marker. The record degrades before it grows.
- **Independent retention, and pruning that verification survives.** The
  stream carries its own periodic signed checkpoints (the same `Signer` trait
  §7 uses). Retention drops a **whole prefix ending at a checkpoint**, never
  an interior entry, and `verify` over the retained suffix starts from that
  checkpoint. Denial retention is set independently of command-history
  retention, because a denial carries no state and is evidence about behaviour,
  not about data.
- **No interaction with the command chain's append lock** [pinned]. The stream
  has its own per-space lock, taken in its own transaction, and appends run
  **off the request path** through the ordinary job queue. Consequences, both
  deliberate: a denial never delays or blocks a legitimate write, and a stream
  append failure is an **operational alert**, never a failed request. The
  stream is evidence, not a precondition.
- **What happens when the store is unreachable** [pinned]. The rule above
  says an append failure must not fail a request; it did not say what
  happens to the *entry*. The answer is **drop, bounded, and declare the
  drop inside the record**: the in-process outbox has a hard cap, arrivals
  past it are refused so the onset of a burst survives, and the loss is
  appended to the affected scope's stream as an `EventsDropped` marker as
  soon as storage is reachable. The alternatives were weighed and refused:
  failing the request converts a logging fault into an outage and hands out
  a denial of service for the price of one unreachable database, and
  spilling to local disk buys a second durable store with its own
  retention, confidentiality and ordering problems. **Nothing is sampled.**
  Every observation is recorded, collapsed with its count, or counted into
  a declared loss; there is no fourth path where an event quietly does not
  happen. A drop is visible in the stream, in the deployment's counters, on
  `/health/deep`, and in that surface's own status, and deliberately *not*
  in readiness, for the reason above applied one layer up.
- **Retention is age, anchored, with a tail** [pinned]. Independent of
  command-history retention (90 days by default), cutting only at a
  **verified** checkpoint, and **never removing the last entry**. That last
  bound is needed because the append path reads the chain head out of
  storage, so an emptied stream makes the next denial start a second chain
  from genesis over the freed indices, orphaning every checkpoint. A
  lawful prune would then manufacture the shape tampering produces. The command
  log's floor also bounds on registered consumers, the undo horizon and a
  published `x-ordo-verifiable-from`; none of the three has a counterpart
  here, because nothing reads this stream from a cursor, a security entry
  cannot be undone, and no client pins a security-stream head. If a reader
  surface is ever built, the floor gains those bounds and the disclosure
  with them.
- **Attribution taxonomy is §7's**, unchanged: a denial names the same actor
  strings a command does.

**What this does not do.** The stream is per space, so a probe *sweeping*
spaces is invisible in any one of them. That pattern is visible only in the
deployment stream and in `ordo_denials_total` (`implementation.md`,
Observability), which is why the metric obligation above is not optional.
Surfaces: `dashboard-plan.md` §3 (explorer, filters, burst view) and §5b.

### Suspension is an authorization input [settled]

A suspended member is denied **at the door, before role, profile, group or
capability resolution**, with its own reason (`principal_suspended`), so
neither a capability token nor a group-conferred role can route around it. It
was previously treated as a queryable status field, which made it display state
that authorization never consulted.

Suspension is non-destructive: role, groups and overrides survive, and
reactivation is a separate step-up-gated command. Denial at the door is
necessary but not sufficient: suspension must also revoke the session's
refresh-token family, close live subscriptions, refuse pending sync proposals,
and (in client-first spaces) trigger a DEK epoch rotation, since a suspended
device still holds keys.

### Linking an authenticator is a dual-proof ceremony [settled]

Linking a new factor is the highest-value action a stolen session can attempt:
it converts transient access into a permanent credential. So it is **not**
reachable from an ordinary authenticated session. Linking requires a fresh
user-presence assertion from an **already-linked** authenticator (checked
against the step-up freshness window), is refused structurally to capability
tokens, and emits an audit event naming **both** the vouching and the new
authenticator.

One exception, deliberately narrow: `linkFirst` bootstraps an account that
holds **zero** factors (invite or auto-onboard). It refuses any account that
already has one, so it cannot become the takeover path it exists to avoid.

Three obligations sit outside the engine and are stated here so they are not
lost: out-of-band notification to the existing factors, refresh-token family
rotation on link, and an immediate-rollback window. A separate delayed,
notified recovery path handles the genuine case where every factor is lost.
Recovery must never share a code path with linking.

## 7. Command log

Append-only, per-space, the safety net under the whole write family [settled].

- **Attribution taxonomy** [settled]: `user:<id>`, `routine:<name>`,
  `agent:<name> on behalf of user:<id>`, `source:<name>`,
  `system:<component>`. Worker pods attribute as `system:<component>`
  [pinned], because workers are infrastructure executing platform decisions, and the
  decision's name is the informative part. The attribution string is the
  canonical rendering; it is what the chain hashes.
- **Command groups** [settled]: all commands from one invocation share a
  correlation id and commit in a single database transaction; triggers and
  invalidations fire only on commit; revert operates on whole groups. A group
  is **single-space** [pinned]: the chain is per-space and BYO-DB makes the
  space the transaction boundary.
- **Before/after images**: per-row, resolved values (never "default")
  [settled]. Create records after only; delete records before only; converge
  records both. Shape violations are typed errors at append.
- **Redaction rule** [pinned]: images of sealed fields store a `SealedRef`, a
  reference into the sealed payload, **never plaintext**. The log stays
  consistent with the encryption boundary: leaking the log leaks the skeleton,
  exactly like leaking Postgres.
- **A `SealedRef` is an opaque handle, never a content hash** [pinned;
  corrects the earlier "SHA-256 of the ciphertext"]. The ref is a **random
  128-bit handle minted per sealed image write**, salted per space and per
  epoch in its derivation and stored alongside the payload it names; two writes
  of the same plaintext produce two different refs, always.
  - Why the old rule was unsafe: a hash of the ciphertext is only unique if the
    ciphertext is, and that held on an invariant no document stated. If sealing
    were deterministic (or, far more likely, if the sealed payload store ever
    deduplicated), equal plaintexts would produce equal refs, and the command
    log would become an **unsalted, permanent, unbounded equality index over
    every sealed field**, including fields the author deliberately did not mark
    `indexed`. That defeats the whole "the choice to index is priced at
    declaration" argument, because nobody chose it. It is also in the
    *plaintext* envelope, so it **survives crypto-erasure**, since destroying the
    key does not touch the log skeleton, and it is readable at **every** custody
    position including client-first, where the envelope is the one thing the
    server can read.
  - Two supporting invariants, stated so they cannot drift: sealed payloads are
    sealed with a **fresh random nonce per write** (`implementation.md`, crypto),
    and the sealed payload store **does not deduplicate**. If content-addressed
    dedup is ever wanted for blobs, it is a per-field opt-in whose equality
    disclosure is priced at declaration exactly as `indexed` is.
  - Conformance: sealing the same plaintext twice must produce two different
    `SealedRef`s, and the canonical-encoding golden test asserts the ref is not
    derivable from the payload bytes.
- **Epoch keys survive at least the undo horizon** [pinned; the fix for
  rotation orphaning images]. A `SealedRef` resolves only while the epoch key
  that sealed the payload still exists, and the chain hashes the ref, so a
  re-seal pass **cannot** rewrite historical before/after images: rewriting one
  would change the group's canonical encoding and break the chain from that
  position forward. Left alone, rotate-then-retire made every pre-rotation
  image unresolvable. `std.E.history` would lose values and `revert_group`
  could not compute its converge-with-before-image, **inside the window the
  platform guarantees undo for**. And because rotation is mandatory on every
  membership revocation, that would fire in exactly the spaces most likely to
  need an undo.
  - The rule: **an epoch key may not be destroyed while any command image
    within the space's `undoHorizon` is tagged with it.** Retirement (no live
    rows reference the epoch) and destruction (the key is gone) become two
    separate events, and destruction is gated on the horizon rather than on the
    re-seal job.
  - The cost: rotate-then-retire's post-compromise recovery is
    **delayed by the undo horizon**: for that window the old epoch key still
    exists in the `KeyProvider` and still opens the historical images sealed
    under it. The rotation confirmation states the real date, derived from the
    space's `undoHorizon`.
  - What is *not* weakened: **crypto-erasure is unaffected.** Erasure destroys
    the **space master key**, which orphans every epoch at once and needs no
    per-epoch cooperation (`implementation.md`, key custody). The gate above
    constrains only the narrower "destroy one retired epoch key" operation.
  - Beyond the horizon, images whose epoch key is gone render as an explicit
    **unresolvable** marker in `std.E.history` and refuse `revert_group` with a
    typed rejection. It never shows a silent empty value and never computes a revert
    against a missing before-image.
  - The rejected alternative is truncating the
    revert horizon at each rotation boundary. Since revocation forces rotation,
    that would collapse the undo guarantee to "since the last person left",
    which is not a guarantee anybody can plan around.
- **Hash chain** [settled, encoding pinned]: per space,
  `hash = SHA-256(prev ‖ canonical-encoding(group))`, genesis prev = 32 zero
  bytes. Canonical encoding is a deterministic byte layout (tagged values,
  big-endian u64 lengths, image fields in key order) documented in
  `ordo-commandlog`; no JSON, no serde ambiguity. `verify(space)` recomputes
  the chain and names the first tampered index.
- **Checkpoints** [settled]: periodic signed markers (`checkpoint 1mo`):
  space, chain position, head hash, timestamp, signed behind a `Signer` trait
  (implementations ride `KeyProvider`: KMS, Vault, local key). A verified
  checkpoint lets verification and pruning start mid-chain.
- **Independent verification: attestation [settled; closes X-051].** The
  chain is computed by the platform and the checkpoints are signed with a key
  the platform holds, so on its own it is tamper-evident against a *partial*
  compromise (someone editing rows without the signing key) and **not** against
  the platform itself, which could recompute from genesis and re-sign. Asking
  the platform for its own history proves nothing. The property comes from a
  copy existing **outside the platform's reach, at a time it cannot
  retroactively influence**. There are four layers, and only the first is on by default:

  **Attestation must not inherit the checkpoint's cadence
  [settled].** A checkpoint is heavyweight: it is the signed anchor that lets
  verification and pruning start mid-chain, so `checkpoint 1mo` is right for
  it. An attestation is a few hundred bytes and one signature. Wiring the two
  together would leave a month of unattested history, which is a long time for
  a rewrite to sit undetected. They are configured separately:

  ```
  policy commandlog {
    checkpoint  1mo                      # pruning anchor, unchanged
    attest      every 1000 commands or 1h   # whichever lands first
  }
  ```

  Volume **or** time, whichever comes first: a busy space attests often, a
  quiet one costs nothing, because the head only moves when a write commits.
  Spaces may request a tighter cadence than the deployment default.

  1. **Client pinning [settled, always on, ~free, no setup].** Responses
     already carry a command-log position (snapshot consistency,
     `implementation.md`); they now also carry the **signed head** at that
     position. So every request is an attestation to that client, and
     detection latency for anyone actually using the system is **one
     request**. Clients persist the last signed head they saw and check
     continuity on next contact (trust-on-first-use with local detection,
     the SSH host-key and Signal safety-number pattern). No third party, no
     configuration, nothing for a developer to set up.

     *Signature cost is bounded by writes, not reads.* The head only changes
     on commit, so the platform signs at most once per commit (in practice
     once per short batching window), caches that signature, and attaches the
     cached one to every response in between. One signature per write burst,
     never one per read. This matters when the `Signer` rides a cloud KMS,
     where each signature is a billable call with real latency; the batching
     window is the knob (`attest.signWindow`, default a few seconds) and it
     bounds cost without meaningfully widening the window.

  2. **Attestation webhook [settled, opt-in per space, requires setup].** At
     each attestation the platform POSTs the signed head to an endpoint the
     space owner nominates, under a **schema published in advance**. Three
     properties, and only some are guaranteed:
     - *Cannot forge*: the attestation is signed.
     - *Cannot redirect*: the destination config is **owner-signed at setup**,
       and the signature is **re-verified before every send**, so an edit of
       the stored destination is a refused delivery rather than a redirected
       one. The limit: this does not
       stop a platform that rewrites the destination *and* the owner key
       together, because a signature checked against a key the attacker also
       chose proves nothing, and no scheme with the verifying key inside the
       perimeter can close that. The owner key's fingerprint therefore travels
       in every payload (`destinationKeyId`), so a receiver who recorded it
       once sees a swap on the next delivery. This is layer 1's trust-on-first-use
       shape, applied to the destination.
     - *Cannot omit undetectably*: omission is not preventable, but the
       **cadence is fixed and known ahead**, so silence is itself the signal. A
       missing attestation is as loud as a wrong one, which leaves a hostile
       platform only the move of going quiet.
  3. **Witnesses = wherever the owner points that webhook.** No special
     protocol and no cross-deployment federation (nobody wires up mutual
     attestation without a reason). Documented targets, cheapest first: the
     owner's own HTTPS endpoint; an object store with **object-lock/WORM** in
     their account (write-once, unrewritable even by them, which is the property that
     matters); a git repository (commit history is itself a hash chain);
     an archived mailbox; **OpenTimestamps** (free; aggregates many hashes into
     one anchor, so per-attestation cost is effectively zero); **Sigstore
     Rekor** (a free public append-only transparency log).
  4. **Public log [settled as an option].** Appending checkpoint hashes to a
     public transparency log for deployments that want maximal provability.
     A head hash is 32 bytes, so ten
     thousand spaces checkpointing monthly is under 4 MB a year. The only real
     expense in blockchain-anchored variants is the anchoring transaction,
     which is exactly what OpenTimestamps amortizes.

  **What requires setup, and what does not [settled].** Layer 1 is on
  everywhere and asks nothing of anyone. A deployment that configures nothing
  still gets per-request detection for active clients. Layers 2–4 are
  **optional and per space**, because each needs somewhere to send things:
  an endpoint, a bucket, a repository, an account. None of them are prerequisites
  for running Ordo, and a deployment that never enables them is not misconfigured.
  It has chosen the default posture, which is a real one.

  Developer-facing documentation obligations for the optional layers:

  - the **attestation payload schema**, versioned and published before first
    use (a receiver must be able to write a verifier against it, not
    reverse-engineer one from samples);
  - the **verification procedure** end to end: given a stored attestation and
    a later chain, how to check continuity, and what a failure looks like;
  - **worked setup guides** per documented target (own endpoint, WORM bucket,
    git repository, mailbox, OpenTimestamps, Rekor), each stating what it costs
    and what it buys;
  - the **signature format and key discovery**: how a receiver obtains the
    verifying key, and what happens to old attestations when that key rotates;
  - a **reference verifier** shipped with the SDKs, so the common case is a
    library call.

  **Two limits.** This *detects* and never
  prevents. The chain becomes tamper-evident against the platform and not
  tamper-proof, which is the same guarantee Certificate Transparency offers and
  is strong for the same reason: rewriting becomes provable. And **you can only
  verify from the point you started watching**. A member joining today cannot
  verify history from before they arrived, and no scheme fixes that. Layers 2–4
  exist to make "started watching" as early as possible. The consent page says
  both.

- **Archive & pruning** [settled]: bundles to object storage per policy
  (`archive r2://…, bundle 1mo`); `undoHorizon` guarantees fine-grained
  revert; the pruning floor **never passes the slowest registered consumer**
  (listener cursors, mirrors, archives all count).

- **A stalled consumer stops pinning the floor [settled].** The rule above,
  taken literally, means one abandoned device or one dead listener cursor
  holds a space's entire history hostage forever: the log grows without
  bound and nothing ever prunes. That outcome needs closing before it shows
  up in production.

  So: **a stalled mirror cursor**, and only a mirror cursor, that has not
  advanced within `policy commandlog { consumerStall <duration> }` (default
  **30d**) is **evicted from the floor calculation**, loudly. Eviction raises
  an operator alert naming the consumer and is recorded in the **deployment
  operational log**, not the space's security-event stream: that stream is
  members-only and its schema is a denial (principal, attempted op, decision),
  which a dormant device is not. An evicted mirror may re-register at any time;
  because it is a cache, it refetches from current state instead of resuming,
  which is exactly what a mirror is for.

  **Eviction applies to exactly one consumer kind.** A "registered consumer" is any cursor the floor calculation
  respects, and there are three kinds:

  | Kind | Evictable? | Why |
  |---|---|---|
  | **Mirror** (managed / BYO custody) | **yes** | It is a cache. Returning past the floor costs a refetch and nothing else. |
  | **Mirror** (client-first space) | **no** | The device *is* the authoritative store (`implementation.md`, client-first mode). There is nothing to refetch from, so eviction would destroy data and not a cache. The dial changes what a mirror *is*, and this rule follows it. |
  | **Listener cursor** (a routine's trigger position) | **no** | See below. |
  | **Archive sink** | **no** | A failing archive is an operational fault to fix. Pruning past it destroys the copy of record. |

  **Listener cursors are never evicted, because §7 already has a lossless
  answer for a listener that stops making progress** (V-002). Delivery is
  `[settled]` **at-least-once**, and it is at-least-once *because* the floor
  never passes the slowest registered consumer. The two sentences are the same
  rule. Evicting a listener and re-registering it at the current floor turns
  at-least-once into zero-times for precisely the routine that most needed
  the events. A listener that fails retries with backoff and then **dead-letters
  as an operator alert**; that path loses nothing and already exists.
  A second, lossy path beside it buys nothing: an abandoned *device* was
  the growth problem this ruling was written to solve, and a device is a mirror.

  A listener cursor that is genuinely dead (its routine deregistered, its
  space's trigger removed) stops being a registered consumer by that act, not
  by timing out. Deregistration is the supported way to stop pinning the floor,
  and it is explicit, attributable, and already logged.

  **The floor is the greatest checkpoint at or below the live bound:**

  ```
  floor = greatest checkpoint position ≤ min(live consumer positions,
                                             position at now − undoHorizon)
  ```

  A checkpoint is the **only legal cut
  point** and is not a competing lower bound, because verification resumes from a checkpoint and a suffix that
  begins mid-chain is unanchored. `min()` over positions and a duration is
  ill-typed; the duration is resolved to a position first.
- **Revert-by-group**: computing the inverse is the log's job; applying it is
  the platform's. `revert_group` returns inverse commands in reverse order
  (create→delete, converge→converge-with-before-image, delete→create), which
  the platform applies **through the normal gate** as a new attributed group.
  History is never rewritten; a revert is more history.

## 7b. Outbound sinks: egress is an attack surface pointed inward [pinned]

A notification sink is an **outbound HTTP destination named by a customer**
(`definition-language.md`, notification channels) that the platform fetches
**from inside its own perimeter**. Nothing in the corpus validated the URL. That
made every space owner (the lowest bar in the threat model, i.e. anyone who
pays) one grant away from `http://169.254.169.254/…`, the cloud metadata
endpoint, whose response is instance credentials, which are the credentials the
`KeyProvider` trusts. **A single-tenant feature reached deployment-wide key
custody**, which is the one outcome the entire custody design exists to prevent.
Sinks deliver payloads and log deliveries, so the response or its metadata comes
back to the tenant; `http://localhost:5432` and the pod/service network are the
same class of target.

The pins, all mandatory, all disclosed on the sink's consent page (the same
computed-not-prose discipline every other disclosure follows). Identical text
and identical obligations in `implementation.md` (Outbound egress). That
document owns the implementation, this one owns the rule:

- **`https` only.** Certificate validation mandatory, TLS 1.2 floor, no opt-out.
  A self-hosted deployment that wants plaintext sinks forks the platform.
- **Resolve-then-pin, re-checked at connect time.** Resolve the hostname at
  grant time *and* at every delivery; reject any resolution in loopback,
  link-local (169.254.0.0/16, fe80::/10), unique-local (**fc00::/7**; this
  said `fd00::/8`, which is only the half in actual use and leaves `fc00::/8`
  reachable, and it was widened 2026-09-08, `corpus-rulings.md`), RFC1918, CGNAT, or the
  deployment's own pod/service CIDRs; then **connect to the resolved address**,
  not to the hostname. Connecting to the address is what makes a DNS rebind
  between check and connect unable to win, and it is the half most
  implementations omit.

  **Every resolved address must pass, not the one that gets used.** A hostile
  name can answer with a public address and `127.0.0.1`; checking only the
  address the client happens to pick is a coin flip an attacker re-flips by
  retrying, and delivery retries with backoff. One denied answer refuses the
  whole resolution.

  **Canonicalise before the check.** `::ffff:127.0.0.1` is an IPv6 value for
  which `is_loopback` is false and which the socket layer nonetheless connects
  to 127.0.0.1; the NAT64 prefix `64:ff9b::/96` and `0.0.0.0/8` are the same
  class of bypass. IPv4-mapped, IPv4-compatible and NAT64 forms are reduced to
  the address they will actually reach *before* any predicate runs; `2002::/16`
  (6to4) is refused outright rather than unmapped.
- **No redirect following, ever.** A 3xx is a delivery failure and is not followed.
  Otherwise the deny-list is one `Location:` header away from irrelevant.
- **Signed deliveries.** `X-Ordo-Signature` = HMAC over
  `version ‖ timestamp ‖ destination ‖ body`, each part length-prefixed
  (u64 big-endian, `ordo-commandlog`'s convention), under a per-sink secret
  shown **once** at grant, with a replay window and the timestamp inside the
  signed material. A receiver that cannot authenticate the platform can be
  spoofed by anyone who learns the URL, and sink URLs travel.

  *Widened 2026-09-08 from `timestamp ‖ body` (`corpus-rulings.md`), both
  additions strict strengthenings.* The **destination** is inside the MAC
  because without it a delivery captured on the wire is valid at any path on
  any host sharing the secret, and nothing stops a receiver reusing one secret
  across two sinks. The **version** is inside the MAC and not only in the
  header because a scheme whose version lives outside the MAC is a scheme whose
  version an attacker chooses.

  **The delivery signature is never the checkpoint signing key.** The payload's
  own Ed25519 signature is minted on the attestation cadence and copied
  verbatim; the transport MAC is a per-sink shared secret. Sharing them would
  put the key that authors this deployment's claims about its own history
  inside the egress identity, the one built to hold nothing, which is the
  pin below.
- **Egress runs under an identity that holds nothing.** Deliveries are made by
  a dedicated egress worker or outbound proxy with **no instance-credential
  access and no `KeyProvider` reachability**, so a bypass of everything above
  still lands somewhere with nothing to steal. This is the control that
  survives a bug in the other four.
- **Per-space egress allowlist, consented at sink creation.** The set of hosts
  a space may deliver to is space-scoped grant state, named on the consent page
  at creation, and adding a host is a new consent and not a config edit.
  Deliveries carry the space's payload, so the destination is a custody
  decision.
- Payload tiering is unchanged and stacks on top: `minimized` derives from the
  envelope tier, `full` from the op's manifest, compile-checked.

## 8. Out of v1

- **SCIM** provisioning/deprovisioning: directory + invite covers v1.
- **Roles beyond the spec's three layers**: no custom roles, no ABAC, no
  per-field ACLs (sealed fields and scope manifests already gate disclosure).
  **Groups are now in** (§6): flat, per-space, non-nested; what stays out is
  **org hierarchy** (groups containing groups, cross-space inheritance, a
  directory tree). The line is drawn where "who can call X" would stop being
  one exhaustive query.
- **IdP-initiated SAML**: SP-initiated only.
- **Session administration UI**, device management, remembered-device
  heuristics: session revocation exists as an op, and surfaces come later.
- **Token introspection/federation endpoints** for third-party resource
  servers: this issuer serves this deployment's clients only.
- **Delta-push of log entries** to consumers: invalidation + refetch stands
  (per the live-query ruling); the transport swap point is already defined.

## 9. Crate map

- `ordo-identity` (now): principal taxonomy, roles/profiles/grants,
  `AuthProvider` + `PasswordHasher` traits, `LocalPasswordProvider` over
  **argon2id** (`m=19456, t=2, p=1`, pinned in `provider.rs` and asserted by a
  test), `StubOidcProvider` (state+nonce semantics without HTTP),
  **`PasskeyProvider`** with in-house WebAuthn verification (§2.1) and the
  `UserPresence` type it alone can mint (§2.2), `session::sign_in` /
  `session::refresh_presence` joining a ceremony to a token,
  `ProviderRegistry`, `Directory` with audited link/unlink and provisioning
  policy, `AccessEngine` implementing §6, token mint/verify.
  Later: real OIDC/SAML over HTTP, the HTTP login/enrollment endpoints,
  `ORDO_APEX_DOMAIN` read at boot by the host binary, Postgres-backed
  directory and credential store.
- `ordo-commandlog` (now): actor/command/group types, canonical encoding,
  SHA-256 per-space chain, `Store` trait + `InMemoryStore`, signed-shape
  checkpoints behind `Signer`, `revert_group` planning. Later: Postgres
  store, archive bundling, checkpoint scheduling, consumer cursors.
- **The security-event stream (§6.1) is the same machinery, separately
  instantiated**: its own head, checkpoints, retention, and lock, sharing
  `ordo-commandlog`'s canonical encoding, chain, and `Signer` rather than
  forking them. Whether it ships as a second store inside `ordo-commandlog` or
  as its own crate is a packaging choice; what is fixed is that it never shares
  a head, a lock, or a retention policy with the command chain.
