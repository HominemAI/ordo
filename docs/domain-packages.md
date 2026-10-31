# Ordo Domain Packages

Companion to `definition-language.md` (§8 `domain` block, §9 deployment
model), `implementation.md` (artifact registry, capability catalog), and
`security-logging-design.md` (attribution, grants). This document turns the
spec's one-paragraph `domain` block into a distribution and versioning story:
how a domain becomes a **package** someone else can install.

Markers as elsewhere: **[settled]** carried from companion docs ·
**[lean]** current recommendation · **[open]** needs a decision.

---

## 0. Grain: backend modules, not UI components [settled]

Read this first, because the word "component" is overloaded. A domain package
is at the grain of a **Convex component**: a sandboxed *backend* module with its
own tables, its own functions and its own scheduled jobs, installed into
someone else's deployment. In Ordo terms: **shapes + ops + routines +
triggers + policies-as-templates**, compiled and versioned. It is **not** a
React component, a widget, or anything that renders.

The connection to UI is exactly one-directional: a package **may carry** the
spec's reserved UI annotations (`label`, `desc`, `order`, `choices`,
`danger`; see `definition-language.md` §10) and optional surface hints, so that
generated UI renders the package's data well. Presentation
metadata rides along; presentation *logic* never does. A package contains no
executable client code of any kind.

## 1. What a package contains [lean: contents settled, manifest format not final]

```
package finance @ 2.1.0 {
  shapes    accounts, transactions, categories, payees, receipts
  ops       register, spendingByPayee, monthlySpend, setCategory
  routines  categorizerRules, spendWatch, missedPayments
  triggers  …
  profile   default                 # access-profile TEMPLATE (role → ops)
  requires  ledger-core @ ^1.4      # compile-time dep, semver
  companions [llm-categorizer]      # declared tier-2 externals (egress review)
  docs      …                       # prose + examples
}
```

Ships:

- **Shapes, named queries/ops, tier-1 routines, triggers**: the compiled
  artifact set: planned SQL, mode labels, cost classes, scope manifests,
  version hashes. A package is `.ordo` source plus its `ordoc build` output.
- **Default access-profile template** [settled mechanism]: applied when a
  space enables the package, then owned by the space. Managers tighten or
  loosen it; the template never overwrites runtime state on upgrade.
  **Built**: packages are not built yet. Until they are, the template is the
  deployment setting `ORDO_ACCESS_TEMPLATE`, and the tenant's profile is built
  (`howto-roles.md`).
- **UI annotations + surface hints** per §0.
- **Docs and the package's own capability-catalog fragment**: the
  LLM-ready rendering of its ops (see §5). It is generated and never hand-written.
- **Compile-time dependencies on other packages**, semver-ranged. Resolution
  happens at `ordoc build` / `ordo plan` time; a deployment holds one version
  of each package [lean: side-by-side major versions deferred until a real
  conflict demands them].
- **Declared tier-2 companions** by name and egress, so trust review (§4)
  sees the whole footprint. The companions themselves remain external
  service-principals, provisioned separately, never code inside the package.

Never ships (**the IaC boundary, extended** [settled]):

- **No principals, no grants, no spaces, no memberships.** Consent is runtime
  state a human creates (`definition-language.md` §9); a package carrying a
  grant would be consent granted by a tarball. The profile *template* is the
  only permission-shaped thing in the box, and it is inert until a space
  owner enables the package.

## 2. Namespacing & isolation

- **Package-qualified names** [lean]: entities and ops live under the package
  name, as in `finance.transactions` and `finance.spendingByPayee`. The trigger
  syntax already assumes this (`when finance.transactions.ingested`,
  `definition-language.md` §7); packaging makes the qualifier load-bearing.
  Unqualified names remain legal inside the package's own source; the
  compiler qualifies on registration.
- **Exports** [lean, proposal]: a package declares which shapes and ops are
  **exported**; everything else is internal. Cross-package refs
  (`field -> ledger-core.accounts`) and query composition across the boundary
  are legal **only against the depended-on package's exports**. Internal
  shapes can be restructured in a minor version; exports are the compatibility
  surface semver speaks about. Compile-checked at build; a dependent binding
  to a non-exported name fails loudly.
- **No runtime sandbox, because none is needed** [settled]: the language has
  no I/O primitives, so a package's routines cannot phone home, read files,
  or touch anything their compiled scope manifests don't name. Isolation is
  **namespace + manifest**, enforced by the same machinery as everything
  else. The dangerous grain (LLM calls, external APIs) stays in tier-2
  companions, which are external principals with declared egress and their
  own consent pages. Checking a package means reading its manifest.

## 3. Versioning & upgrade [settled mechanics, lean policy]

- **A package version *is* its compiled artifact hash set.** The semver label
  is for humans and dependency ranges; identity is the content hash, the
  same version-hash discipline the registry, SDK pinning, and page tokens
  already use. Two builds with identical artifacts are the same version
  regardless of label.
- **Installing an upgrade is a deploy.** It uses the standard diff engine
  (`definition-language.md` §9) with nothing package-specific: `plan` diffs the
  incoming artifact set against the deployed one, classifies each change
  additive / transforming / destructive, and demands mappings and
  confirmations exactly as for first-party code. **Classification is
  per-space and custody-aware**: sealed-field transforms route through the
  worker; for BYO-DB and client-first spaces the space owner confirms
  destructive steps; a client-first space applies migrations through the
  embedded engine on sync.
- **Enabling a package in a space** instantiates the profile template as that
  space's runtime profile and runs the package's DDL against the space's
  database (per-space shape versioning replays the diff per target, as with
  BYO-DB connect). Enabling is a logged, attributed command.
- **Scope drift kills grants** [settled, restated]: an upgrade that expands
  any op's scope manifest changes its hash. Capability tokens and routine
  grants pinned to the old hash go dead until re-consent. Package upgrades
  get this from the mechanism itself.
- **Uninstall** [lean]: disabling a package in a space poses one question,
  which is the data. The space owner chooses among **retain** (tables kept, ops gone; the
  package can be re-enabled), **export-then-destroy** (space-export bundle of
  the package's entities, then drop), **destroy** (drop). Destroy paths are
  destructive-classified: explicit confirmation, step-up where the client
  demands it, the apply is a logged command. Refs from surviving entities
  into package entities follow the ordinary delete-restrict rule. An
  uninstall that would orphan referrers is refused with the list.

## 4. Distribution & trust

- **Registry-as-data** [lean]: v1 distribution is an **index of signed
  packages**, a fetchable document mapping package name + version to a git
  URL and a content hash. There is no central npm-clone service, no accounts and no
  upload pipeline. A package is installable from any git remote the deployment owner
  can reach, and the index is curation. A hosted registry is a later
  product decision layered on the same record shape.
- **Signing + provenance**: package artifacts are signed (same `Signer` trait
  family as checkpoint signing); the installed **package hash rides in every
  command its routines produce**. Attribution already carries routine
  identity (`routine:<name>`), and the package hash extends it
  (`routine:finance/categorizerRules@<hash>` [lean: exact rendering open]).
  Every row a package ever touched is traceable to the exact signed version
  that touched it, through the ordinary command log.
- **Trust review = the manifest** [settled in spirit]: reviewing a package is
  not reading its code hoping to spot exfiltration, because the language cannot
  express exfiltration. Review reads the derived facts: which fields each op
  touches (scope manifests), which fields are sealed, what the routines may
  propose, what the profile template grants each role, and which tier-2
  companions with what egress the package declares. The consent page a space
  owner sees at enable-time is generated from exactly this, using the same
  computed-not-prose discipline as custody conversion.

## 5. LLM-reachability [settled as goal]

An assistant reaches for a package **instead of regenerating a
domain**. The north star (`definition-language.md` §10) has an LLM writing
two pages of `.ordo`; the packaged version of that story is better: the
assistant searches the index, reads a package's capability-catalog fragment
(compact signatures plus mode labels, cost classes and annotations), and
composes against it. **A package's catalog fragment plus the two-page grammar
prompt is sufficient context to use the package correctly**: the closed
grammar guarantees composition against exported ops either parses or fails
loudly, and the catalog is runtime-verified so it cannot lie about shapes.
The eval story extends to packages. A package ships with its conformance
cases, and "can a model compose this package from its fragment alone" is a
measurable bar a package should pass before the index lists it [lean].

## 6. Relationship to generated UI [settled]

**The same annotations feed both.** A package with good `label`/`desc`/`order`/
`choices`/`danger` coverage gets good generated surfaces: tables,
detail views and forms render from the catalog the moment a space enables
it, with no UI code shipped or written. Surface hints can nudge layout;
that is the ceiling. Custom UI remains an **app-level, per-surface
override** (`definition-language.md` §10). An application replaces
generated surfaces one at a time with its own. A package can never require a
custom UI, bundle one, or depend on one existing. Packages ship data and
behavior; apps decide how it looks.

## 7. Open items

- Manifest file format and the exact `package` block syntax (the spec's
  `domain` block is the seed; whether `domain` and `package` unify into one
  keyword).
- Export-list syntax and whether shapes export field-by-field or
  whole-shape only.
- Side-by-side major versions of one package in a deployment (deferred; one
  version per package for now).
- Hosted registry product (search, namespacing authority, yanking). The
  index record shape should be designed not to preclude it.
- Attribution rendering for package-qualified routines (exact string is
  chain-hashed, so it must be pinned before first ship).
- Package-shipped conformance cases as an index listing requirement (bar
  and tooling).
