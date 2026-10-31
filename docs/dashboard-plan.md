# Ordo Dashboard Plan

Companion to `implementation.md` (capability catalog, hot swap, client-first
mode), `security-logging-design.md` (roles, capability tokens, command log),
and `api-engine-design.md` (the op protocol every pane speaks). The
operator/developer dashboard: what it shows, how panes compose, and — the
question this document exists to answer — **who has custody to it**.

Markers as elsewhere: **[settled]** carried from companion docs ·
**[lean]** current recommendation · **[open]** needs a decision ·
**[proposed]** a gap filled here with a concrete design, not yet ruled on.

---

## 1. Why the dashboard is cheap [settled]

The dashboard invents nothing. Every surface is a rendering of an artifact the
platform already maintains for its own correctness:

| Source | Already exists because | Dashboard surface |
|---|---|---|
| **Artifact registry** | compiled ops are data: versions, mode labels, cost classes, scope manifests (`implementation.md`) | registry/deploy view, diffs, rollback pointers |
| **Capability catalog** | runtime-verified shapes served per-token (`implementation.md`) | catalog viewer, op runner forms, data browser columns |
| **Command log** | attributed history, groups, hash chain, checkpoints (`security-logging-design.md` §7) | log explorer, group revert, chain health |
| **Security-event stream** | per-space, hash-chained, members-only denial record with its own head and retention (`security-logging-design.md` §6.1) | security-event explorer, denial bursts, stream verify |
| **Platform health** | job queue, trigger cursors, sync/vault state, `KeyProvider` status | routine/trigger monitor, queue depth, dead letters |

No pane has a private data path. A pane that cannot be fed from a registered
op or an existing platform endpoint is a pane the platform is missing an op
for — fix the platform, not the dashboard. This is the same discipline as
generated UI: schema + op metadata in, surfaces out.

## 2. Custody: the dashboard is a client [settled]

**The dashboard is a client, not a backdoor.** It is declared like any other
identity client (`client ops-web { … }`), authenticates through the same
ceremony, holds the same token shapes, and calls registered ops through the
same two routes as everything else. There is no dashboard service account, no
ambient platform credential, no query path that bypasses the access check.
Every pane sees exactly what the signed-in principal's token can see — the
one-check-at-the-door pipeline (`security-logging-design.md` §6) is the whole
authorization story.

Consequences, each deliberate:

- **Per-space data browsing requires membership.** Browsing a space's rows
  means holding a role in that space whose access profile includes the ops
  the browser invokes. No membership, no rows — the pane renders the spaces
  the token can enumerate and nothing else.
- **Sealed values render sealed.** A sealed field shows a sealed marker
  unless the viewer's session lawfully holds key release for that space. The
  dashboard never requests release the signed-in principal couldn't get from
  any other client.
- **Client-first / E2E spaces show structure, never values.** For a space at
  the far end of the custody dial, the dashboard renders what the server
  holds: envelope metadata, row counts, sync state, chain health — ciphertext
  and skeleton. This is not a degraded mode; it is the product working. The
  platform operator opening the dashboard on an E2E space and seeing nothing
  readable **is the sovereignty proof** — the demonstration, in the
  operator's own tool, that the custody promise is mechanical rather than
  contractual.
- **The op runner invokes registered ops only.** Forms are generated from the
  catalog: params, bounds, mode labels, step-up requirements. The dashboard
  gets no raw SQL. The dev/ops free-form query mode the spec reserves
  (`definition-language.md` §1, principle 7) exists — but as a **separate,
  elevated, individually granted, fully audited capability**, never a
  dashboard default. [lean] v1 scopes free-form mode out of the dashboard
  entirely; it lands later as its own step-up-gated pane if at all.

### Three personas, one mechanism [settled]

The dashboard has no persona switch — personas are what different tokens can
see. Named here so the pane inventory can be checked against them:

| Persona | Token holds | Sees |
|---|---|---|
| **Domain developer** | deploy-scope grants | registry, diffs, versions, catalog, eval/conformance results — no space data |
| **Space operator/owner** | membership in their spaces | those spaces' data (per role), profiles, grants, membership, custody dial |
| **Platform operator** | platform-health grants | queues, trigger cursors, chain verification, key-provider status — **metadata only**, never values |

The platform operator's default view contains no user data by construction:
health endpoints serve envelope-tier facts, and everything value-bearing sits
behind space membership the operator doesn't have.

### Break-glass: operator mode [settled as shape, lean on mechanics]

Sometimes support genuinely needs to see inside a space. That path exists and
is engineered to be loud:

- An **explicit grant** — a human with authority over the space (or a
  deployment policy that pre-authorizes support access, disclosed on the
  space's consent page) mints a time-boxed capability listing exactly the ops
  involved.
- **Step-up gated** — activating operator mode demands fresh WebAuthn
  presence; capability tokens can never satisfy it, so break-glass is
  structurally human-only (`security-logging-design.md` §5).
- **Time-boxed** — mandatory expiry, short by default [lean: hours, not
  days]; no standing grant.
- **Command-logged** — the grant, every op called under it, and its expiry
  are attributed history in the space's own chain. The space owner can read
  exactly what support saw.
- **Never a default.** No role, including platform operator, includes it. An
  operator who has not gone through the ceremony sees what §2 says they see.

For E2E spaces even break-glass yields ciphertext — key release requires
client participation, and no grant changes that. Stated plainly in the
consent language.

## 3. Surface inventory [lean]

The pane list, each named with its data source:

- **Data browser** — per-space, catalog-driven tables and detail views over
  registered read ops; sealed markers per §2; refinement envelope (Tier 0)
  for ad-hoc narrowing within what ops already expose.
- **Op runner** — invoke any op the token allows; generated forms from param
  specs; mode label and cost class displayed before execution; `danger`
  annotation and step-up honored; results paginate via the standard tokens.
- **Registry / deploy view** — deployed versions, per-op artifacts, pending
  `plan` diffs with additive/transforming/destructive classification, mode
  label and cost-class drift highlighted, rollback (re-point to prior hash).
- **Command-log explorer** — per-space, filter by actor/entity/time, group
  detail with before/after images (sealed fields as `SealedRef` markers),
  causal chains (`user → op → intent → effector → outcome`), **group revert**
  as a first-class action (itself a logged group), chain verify + checkpoint
  status. Revert is offered only where it can be computed: past the space's
  `undoHorizon`, an image whose epoch key was destroyed renders as
  **unresolvable** and revert refuses with a typed rejection rather than
  computing against a missing before-image (`security-logging-design.md` §7).
- **Log weight view** (inside the command-log explorer) — heaviest entities and
  rows by **command count** and **image bytes**, plus the current pruning floor
  and what is holding it (which consumer, how far behind, whether it is near
  the `consumerStall` eviction threshold). This answers "what is bloating my
  log" with a pane instead of an investigation.

  **Missing platform support, stated per §1's rule.** `image bytes` is not a
  metric the platform emits today and no op returns it. Computed on demand it
  is an O(log) scan — precisely the cost this pane exists to warn about — so it
  needs **maintained per-entity counters advanced at append time**, and this
  pane is blocked until they exist. Two properties to carry into that work:
  sealed values are `SealedRef` handles into a separate non-deduplicating
  store, so a naive byte count **understates** weight wherever fields are
  sealed and must either follow the ref or be labelled as excluding sealed
  payloads; and the counters are skeleton, not content, so the pane degrades
  correctly in a client-first space (row identities and sizes, no values). It exists because the alternative is
  discovering the answer the hard way: append-only stores that lack this view
  get diagnosed by building an inspector *after* a space has already become
  slow, and by then the growth is history you cannot compact away.
- **Security-event explorer** — per-space, over the denial stream
  (`security-logging-design.md` §6.1): filter by principal / op / decision /
  window, collapsed entries with first-last-count, stream verify and checkpoint
  status. Structurally a sibling of the command-log explorer and deliberately
  not a tab inside it; sealed values never appear because the stream never
  carries arguments.
- **Routine / trigger monitor** — registered routines and triggers, last
  runs, debounce/coalesce state, retry and dead-letter queues, listener
  cursor positions vs. the pruning floor.
- **Space administration** — membership, roles, access profiles (template vs.
  current diff), capability grants and their manifest-hash liveness, the
  custody dial with the computed surviving-capability list before any
  conversion.
- **Plan-variant pane** — per space, which SQL formulation each op is running
  and why (`plan-tiers.md`): current variant, the guard/score that selected it,
  the statistics it was scored against and when they were sampled, predicted
  cost class vs. observed, and drift flags. Actions: re-evaluate, pin, revert
  to the compiler's choice, request a shadow run. See §5d.
- **Catalog viewer** — the token-scoped capability catalog rendered
  human-readable; doubles as "what would an agent with my grants see."

## 4. Modularity: panes as a registry-driven plugin model [lean]

"Easy to add new components" is a contract, not a folder convention. A pane
declares a manifest:

```
pane commandLogExplorer {
  id        ops.commandlog
  requires  [commandlog.listGroups, commandlog.groupDetail, commandlog.revertGroup]
  surface   explorer          # table | detail | form | explorer | monitor
}
```

- **`requires` drives visibility.** The shell derives pane availability from
  the viewer's token: a pane whose required ops aren't callable does not
  render — no dead buttons, no probe-by-clicking. This is UX honesty, not
  enforcement; **enforcement stays server-side** at the door, always. A pane
  cannot exceed its token no matter what it declares.
- **First-party panes use the same contract** as any future third-party
  pane. The shell ships knowing nothing pane-specific; the initial inventory
  in §3 is just the bundled pane set.
- **Panes are TypeScript/React** (`implementation.md`: the web UI is TS;
  Rust ends at the API boundary), composed over the **generated SDK + the
  capability catalog** — typed params, typed results, typed errors, pagination
  and `?live` for free. A pane is ordinary client code; nothing about being
  inside the dashboard grants it anything.
- **Surface types are the reuse seam**: schema-driven table, detail, and form
  renderers are shared primitives (the same ones the generated-UI phase
  needs); most panes are configuration over those primitives plus a custom
  fragment.

Third-party pane distribution, signing, and review are **[open]** — deferred
until domain packages (see `domain-packages.md`) settle the analogous
questions for backend modules; a packaged domain shipping a companion pane is
the expected eventual shape.

## 5. Relationship to generated UI [settled]

Same primitives, different products. Generated UI (`definition-language.md`
§10) renders end-user app surfaces from schema + op metadata; the dashboard
renders operator surfaces from the same metadata plus platform internals
(registry, log, health). They share the rendering layer — catalog-driven
tables, forms from param specs, annotation handling (`label`, `desc`,
`order`, `choices`, `danger`) — and diverge in audience and chrome. Work on
either strengthens the other; neither is a fork of the other.

## 5b. Denials: a second stream, not a second kind of command [settled]

The audit story was half-built: access **denials** landed only in the
operational log, so the tamper-evident record a space owner is told to trust
said who *did* things and never who was **refused**. An attacker's failed
probing was invisible in exactly the record built to survive an attacker.

An earlier version of this section fixed that by appending denials to the
space's **command** chain. **That ruling is superseded** — it let an
unauthenticated caller pick a victim space, fail *because* they were not a
member, and grow that space's chain (while contending for its append lock),
and its two bounds were not expressible in an append-only structure at all.
The replacement is owned by `security-logging-design.md` §6.1: **a separate,
per-space, hash-chained security-event stream**, members-only, with its own
head, checkpoints, retention, and lock. This document does not restate its
rules; it renders them.

What the dashboard surfaces, and what each surface depends on:

- **A security-event explorer, beside the command-log explorer, never inside
  it.** Two streams, two heads, two verify buttons, two retention settings —
  showing them as one timeline would re-import the confusion §6.1 exists to
  end. Filter by principal, op, decision, and window; collapsed entries render
  as first instant / last instant / count, which is what the entry carries.
- **The decision, not the wire code.** The stream records
  `not_found`-because-guard distinctly from `not_found`-because-absent, and a
  forbidden op distinctly from an unregistered name, while the wire stays
  deliberately uniform (`api-engine-design.md` §4.1). The explorer is where
  that distinction is *for*, and the pane says so on its face — an operator
  reading it must not conclude that callers see these labels.
- **Burst view is the point.** A run of denials against one principal or one
  op is the signal an operator could not previously get.
- **Sweeps across spaces are not visible here.** A principal probing a hundred
  spaces they are not a member of appears in none of their streams, by design
  (`security-logging-design.md` §6.1, members-only). That pattern lives in the
  deployment security log and in `ordo_denials_total`, which the platform-health
  panes read — so the two surfaces answer different questions and the pane
  links across rather than pretending to be complete.

## 5c. Groups in the admin surfaces [settled]

The access model now resolves membership through **groups** (named principal
collections owned by the space). Two surfaces change: space administration
gains a group editor (create, rename, add/remove members — each an audited
command), and every "who can call this" answer must render **why** — direct
membership, a role's profile, a group, or a capability grant. A permissions
pane that shows only the resolved yes/no forces an operator back to guessing;
the derivation path is the useful part, and `callersOf` returns it.

The group editor invents no data path: it is configuration over the
`std.space.*` group ops (`std-library.md` §3), which is why those ops have to
exist as registered, access-checked, command-logged ops rather than as
dashboard-only endpoints. A pane that cannot be fed from a registered op is a
missing op, per §1.

## 5d. The plan-variant pane [lean]

The surface for `plan-tiers.md`. It exists because that design makes a promise
the dashboard has to keep: **a planner that can explain itself.** If the
explanation is not renderable, the selection is a black box with extra steps.

Everything here is a projection of data the platform already emits — variants
and their guards/scores/regimes from the artifact, current selection and its
justifying statistics from the catalog, predicted class from registration,
observed cost from execution telemetry. The pane computes nothing.

**Per op, for the space in context:**

| Column | Source | Why it is here |
|---|---|---|
| variant in use | catalog, `(space, op)` selection | the answer to "what SQL am I actually running" |
| why | the variant's **regime** sentence | one line, no algebra |
| the numbers | the **score** inputs, with each statistic's sample time | so a claim can be checked, and a stale justification is visible as stale |
| predicted vs observed | registration cost class vs telemetry | the drift signal, expressed as a gap and not a raw latency |
| other variants | guards, and why each is illegal or lower-scoring | the pane must show what was **rejected and why** — otherwise "explains itself" is decorative |

**Actions, each an ordinary attributed command:**

- **Re-evaluate now** — resample statistics and rescore. The common case after
  a bulk import, when the model is right and its inputs are stale.
- **Pin / unpin** — hold a variant against the formula's advice. Pins are
  visible, dated, and carry the pinner; an unexplained pin found two years
  later is the failure mode this prevents.
- **Revert to the compiler's choice** — the same mechanism backwards, always
  available.
- **Request a shadow run** — measure the alternative on real data rather than
  model it (`plan-tiers.md`). Presented as *measure this*, never *try this*:
  it cannot change what any caller receives, because the corpus already proved
  the variants equivalent.

### Split trials — serving two variants at once [lean]

Shadow runs measure a candidate without serving it. A **split trial** serves
both to real traffic and compares — the blue/green shape, and safe here in a
way it never is for ordinary deployments: **the corpus already proved the arms
return identical rows in identical order**, so a caller cannot tell which arm
answered except by timing it. There is no behavioural risk to manage, only a
performance question to settle.

**What the deployment owner configures**, per trial:

| Setting | Options |
|---|---|
| scope | one op in one space · one op across **all** spaces · a set of ops (the workload bundle from `plan-tiers.md`) |
| arms | which two variants — the incumbent and one candidate; never more than two, or attribution gets murky |
| split | percentage to the candidate; 50/50 default, 10/90 for cautious, anything in between |
| duration | wall-clock, or a request count, whichever lands first; **mandatory** — a trial with no end is a permanent divergence nobody remembers starting |
| ending | what happens at expiry: revert to incumbent (default), hold and wait for a decision, or promote if the candidate wins by a stated margin |

**Assignment is deterministic and stateless.** An arm is chosen by
`hash(principal ‖ op ‖ trialId) mod 100 < split` — the platform's own stable
`hash` (spec §3, already blessed for "bucketing/rollout striping"). That buys
three things at once: the same caller lands in the same arm for the trial's
life, so nobody experiences flapping latency; no assignment table exists to
store, migrate, or leak; and any assignment can be **recomputed later** when
reading the results, which is what makes the analysis auditable rather than
merely reported.

**Pagination crosses arms safely, and that is a requirement rather than a
hope.** Because the arms are proven to return identical rows in identical
order, a keyset cursor minted under one is valid under the other. A variant
that cannot accept its peer's cursor is **ineligible for split traffic** —
stated as an eligibility rule so it is checked at trial creation, not
discovered mid-trial.

**What the pane shows while it runs**: per arm, request count, latency
distribution (not a mean — the tail is the point), rows examined, host-decrypt
count, and error counts; the split's actual realized ratio against its
configured one; and time or requests remaining.

**Guardrails.**

- **One trial per `(op, space)` at a time.** Two overlapping trials make the
  difference unattributable, so the second is refused rather than queued
  silently.
- **Never auto-promote without a stated margin.** "The candidate wins" must be
  a threshold the operator wrote down beforehand, or promotion is a coin flip
  wearing a lab coat.
- **Start, end, and promotion are attributed commands** — who ran this trial,
  over what, for how long, and what it concluded. A performance decision
  nobody can trace back is how a system acquires folklore.
- **A trial never outlives its own configuration.** Expiry reverts by default;
  a trial that ends by *holding* is shown as an open decision, not as a
  finished experiment.
- **Not available in client-first spaces** — the server serves nothing there,
  so it can split nothing. On-device trials would be a client feature and are
  out of scope here; the pane says so rather than offering a dead control.

**Deployment-wide view**, for the operator persona: which ops show drift across
*many* spaces (evidence the formula is wrong, not the tenant), the "promote to
default for new spaces" action, and the workload view that bundles ops sharing
a traversal so one maintained structure is proposed once rather than three
times.

**Boundaries.** The pane never shows SQL text to a space operator — the
formulation is platform internals and showing it invites hand-editing of a
thing that must stay compiled; the *domain developer* persona may see it in the
registry view, where artifacts already live. Selection changes are never
offered where they would alter a contract (cursor stability, pagination order,
liveness) — those candidates are not shown as options at all, because a control
that refuses on click is worse than a control that is absent. And in
**client-first spaces the pane is nearly empty by construction**: the server
executes nothing and measures nothing, so it shows the compile-time variants,
states that selection and telemetry are on-device, and stops. That emptiness is
the custody dial working, and the pane says so rather than looking broken.

## 6. Delivery phasing [lean]

Ordered by dependency and by how much trust machinery each step needs:

1. **Read-only core**: registry/deploy view, catalog viewer, command-log
   explorer (no revert), **security-event explorer** (§5b — read-only by
   nature, and it ships in this phase rather than later because the denial
   record is worth nothing if nobody can read it). Pure rendering of existing
   endpoints; ships first because it exercises the pane contract with zero
   write risk.
2. **Op runner** (+ log-explorer revert): first writes through the dashboard;
   needs step-up plumbing in the shell and the `danger` annotation honored.
3. **Space administration**: profiles, grants, membership, custody dial —
   needs the grant-management ops to exist as registered ops themselves.
4. **Break-glass operator mode**: last, deliberately — it needs step-up,
   time-boxed grants, and consent-page disclosure all proven elsewhere first.

Routine/trigger monitor lands alongside whichever phase the queue endpoints
stabilize in [open: depends on engine-plan sequencing].

The **plan-variant pane** splits across phases rather than landing whole: its
read-only half (variant in use, why, the numbers, rejected alternatives) is
pure rendering and belongs in phase 1 the moment the catalog carries variant
metadata — showing which formulation a space runs is worth having long before
anyone can change it. Its actions (re-evaluate, pin, shadow run) wait for
phase 2, since each is a write. Nothing in it can ship before the executor
emits telemetry and the corpus can be run on demand, which is Track F work in
`engine-plan.md`; until then the pane renders compile-time variants with an
honest "not yet measured" rather than an empty chart.

## 7. Open items

- Free-form dev/ops query pane: in-dashboard eventually, or CLI-only forever.
- Third-party pane packaging/signing (deferred to the domain-package story).
- Platform-health endpoint inventory — which metrics are ops vs. a separate
  metrics surface (Prometheus-shaped) that the monitor pane reads.
- Break-glass pre-authorization policy shape (per-deployment support access)
  needs consent-page language review alongside the compliance pack.
- Plan-variant pane: whether **pins expire**. An unexplained permanent pin is
  the failure mode §5d exists to prevent, and a dated pin that never lapses is
  the same thing arriving slowly — but forced expiry on a deliberate operator
  choice is its own annoyance. Leaning toward: pins do not expire, but a pin
  older than the statistics that justified it is flagged as **unreviewed**.
- Plan-variant pane: who may pin. Plausibly a space operator for their own
  space and a deployment operator for defaults, but pinning trades latency for
  write cost in the T2 cases, so it may belong to whoever pays that bill.
