# Ordo Generated UI

Companion to `definition-language.md` (§8 identity clients, §10 north star),
`implementation.md` (capability catalog, shape verification, client delivery),
`tooling-and-embedding-plan.md` (§B1 descriptor, §B2 refinement, §B3 terminal
verbs), `api-engine-design.md` (the op protocol), and `dashboard-plan.md`
(the dashboard product sharing these primitives). `std-library.md` is the
sibling covering the standard op families referenced here:
`std.E.list` / `std.E.get` / `std.E.whole` / `std.E.rollup` and the metric
ops. This doc depends on their existence and says nothing of their text.

Markers as elsewhere: **[settled]** agreed · **[lean]** current
recommendation · **[open]** needs a decision · **[proposed]** a gap filled
here with a concrete design, not yet ruled on.

---

## 1. The decision: compile to data, not components [settled]

Generated UI is **not codegen**. For every registered op, the build emits a
**surface descriptor**: framework-neutral JSON describing how that op's
results present. Component code rots the moment it is written. A descriptor
is derived truth, so it evolves with the registry and the runtime evolves
independently of both.

Mechanics: descriptors are a pure projection of the Interface Descriptor
(`tooling-and-embedding-plan.md` §B1) plus the reserved UI annotations
(`label`, `desc`, `order`, `choices`, `danger`; see `definition-language.md`
§10.3, where they were inert until now and are load-bearing here). `ordoc build` emits them as
`out/surfaces.json` beside `interface.json`, which is cheap once artifacts exist,
and `ordo apply` registers them with the artifact registry. Each descriptor
carries its op's **version hash** and republishes atomically with hot swap.
Descriptors are **shape-verified** with the catalog: the columns a descriptor
claims must match the op's observed shape from empty-set execution
(`implementation.md`), so a descriptor can never silently disagree with the
wire.

Per read op, the descriptor states:

| Field | Derived from |
|---|---|
| **columns** | the output-field manifest: name, type, **format hint**, alignment, sortability, refinability. The hint vocabulary is the closed type inventory itself: `money`, `date`/`datetime`, `id`, `text`, `int`, `bool`, `duration`, `enum`. There is no second metadata channel. Alignment derives (numeric/money right, dates tabular, text left). Refinability mirrors the op's declared tier. |
| **row identity** | the anchor entity's implicit typed id. It rides every list result row **even when unprojected**. It already underwrites cursor stability, so delivering it costs nothing. Grouped/`reduce` results are **synthetic rows**: no identity, no detail link, no actions. |
| **pagination** | keyset, from the op's sort + id tiebreak, using the standard token protocol (`api-engine-design.md` §8). Total order is a language guarantee, so cursors are always stable. |
| **live** | the op's live-capability flag. |
| **detail linkage** | typed-id deref: an id-typed column links to the detail surface for that entity type, which `std.E.get`/`std.E.whole` serves or which app page claims (§5). |
| **actions** | the write ops that accept this row (rule below), surfaced as the SDK's `.commit()` verbs. Each carries its mode label, `danger` mark, and step-up requirement from the catalog. |
| **empty / error states** | empty-state text from the op's `desc` annotation or a typed default. Errors are the closed taxonomy (`api-engine-design.md` §4), and typed rejections render typed, never as strings. |
| **annotations** | `label`/`desc`/`order`/`choices`/`danger` flow through verbatim. |

**Action discovery [settled]:** a write op *applies to a row* iff it declares
an id-typed parameter whose entity prefix matches the row's id type (typed
ids make this statically detectable) **and** the caller's token can call it.
No annotation, no registration flag; the catalog is the source.

**Commit semantics [settled]:** the generated commit path is
**CAS-by-default**: the fetched row's `rev` rides the write, and `cas_miss` is a
typed conflict the runtime resolves by refetch + conflict presentation, never
a silent overwrite. (Extends the definition language's "generated forms CAS
by default" into the runtime contract.)

**Module ≠ access [settled].** A descriptor describes *shape*, never rows.
Pulling one, caching one, rendering one grants nothing. Data arrives only
through the op itself, under the caller's token, past the one check at the
door. An unauthorized viewer renders chrome and no rows. And descriptors are
themselves **catalog-scoped**: served with the capability catalog, so a token
receives descriptors only for ops it may call, so an app's renderable surface
*is* its catalog.

## 2. The headless runtime [settled as shape]

One thin package per framework; **`@hominemai/ordo-react` first** (the web UI is
TS/React, per `implementation.md`). The whole package is one idea:

```
useSurface(op, params) -> { columns, rows, sort, page, refine, actions, live, commit }
```

Everything wired, nothing rendered:

- **Fetch** through the generated SDK: typed params, typed rows, typed
  errors.
- **Live**: `?live` subscription, invalidation → refetch-through-scope
  (a staleness signal, never a data push).
- **Snapshot consistency [settled]**: log-positioned reads
  (`implementation.md`). All surfaces on a page hold **one** command-log
  position; after invalidation the runtime advances the visible snapshot only
  when every affected refetch reaches the new position. The coordinator is
  page-level, so two widgets never show pre- and post-write
  states of one command group.
- **Paging**: keyset tokens as scroll/next-page state; `heavy` mode
  auto-paging where declared. **Only the absence of `next` ends a listing.** A
  refinement's conditions compile into the plan (ruled 2026-10-05,
  `api-engine-design.md` §7), so refined pages come back full; a runtime still
  stops on the absence of `next` and never on a short page. The infinite-scroll
  loop keeps pulling until `next` is gone, and the empty-state renders only
  when the whole iteration produced nothing.
- **Refinement**: sort/filter/column UI state serializing to **exactly the
  Tier-0 `$refine` envelope** and nothing more. Every program read is refinable;
  writes, platform ops and public reads (tier `none`) render sort-only chrome.
  Narrowing-only holds by construction.
- **Commits**: optimistic apply, CAS as above, typed rollback on typed
  rejection; idempotency envelope automatic in the SDK retry loop.

Zero DOM opinions. The hook returns state and verbs; what a table looks like
is left to the skin.

**Status of live and the device copy (built 2026-09-29, checklist 14b).**
`@hominemai/ordo-react` follows the live feed and answers from the device replica. Both
are switched on at `<OrdoProvider>` (`live`, `device`) and are off until an
app does.

- *Live.* `useOpQuery` and `useOpPages` open `?live=1` for an op whose
  `call.live` is true, share one stream per (op, params), wait for the stream
  to open before the first fetch, and refetch when an `invalidate` arrives.
  A reopened stream counts as an invalidation. The document above says
  "`?live` subscription" and does not say whether a hook subscribes by
  default, or how invalidations are paced. Chosen and reported for the owner:
  **opt-in** (provider `live`, or `live: true` on a hook), and **coalesced,
  at least a quarter second apart** by default (`paceMs`, shipped as
  `DEFAULT_LIVE_PACE_MS`; ruled 2026-10-01, 27n: a setting, and a hook's
  `livePaceMs` may lengthen the gap, never shorten it).
- *Device copy.* A read whose descriptor says `result.sync.syncable` is
  answered from the SDK's `LocalReplica`, kept current by its `SyncManager`,
  and keeps answering offline. Everything else goes to the network, and the
  hook says why (`deviceFallback`). It never covers `std.space.*`, aggregates,
  grouped reads, `distinct` or many-side joins, and never a write. Offline
  writes are refused, not queued. This is §8's "the UI layer cannot tell"
  from the reading side: the same hook, a different venue.
- *Commit and revocation.* A committed write tells the device to ask the
  server again (the app's `reload`); a revoking refusal empties the hook and
  releases what the op held.
- *Not built.* The page-level snapshot coordinator above. It needs a log
  position on this package's read path, which the descriptor does not carry.
  Until then two hooks on one page can show a refetch at different moments.
  The SDK's `SyncManager` view holds one common position for synced ops; the
  hooks do not gate on it yet.

## 3. Skins [settled as shape] and static renderers [settled]

**A skin is a package over the headless runtime.** It takes what layer 2
(§2) returns, the descriptor plus the `useSurface` state, and a theme, and
renders a surface; nothing else crosses the boundary. Because layer 2 is
headless, any design system can write one, mapping descriptors onto its own
components, and the runtime cannot tell which skin is drawing it. This
shape is owner-approved and building: the
Watermelon skin and its shadcn registry emission are homed at 17p
(`roadmap-checklist.md`).

**Default skins [illustrative]:** `<OrdoTable>`, `<OrdoDetail>`, `<OrdoForm>`,
`<OrdoMetric>`, themed via **CSS variables only** (`--ordo-*` namespace;
token inventory [open]). This specific block set is a starting point, not a
commitment. What is settled is that a skin exists as a package over the
runtime. Which four components ship as the default one is open.

**Static renderers [settled]:** the same descriptors drive non-interactive
mouths: a markdown table, a CSV export, an HTML email fragment (feeding the
platform notification channel), an LLM-readable summary (feeding the
catalog's agent-facing story). They are pure functions `descriptor + rows → text`, in
a framework-free `@ordo/render`.

## 4. Ejection [settled]

CDK-style codegen exists **only as an off-ramp**:

```
ordo eject surface <op>
```

writes a copy-in component the app owns outright (the shadcn pattern).
Ejection is per-surface, never all-or-nothing (per §10's replace-per-surface
pin). It is **never the delivery mechanism**: generated code rots, while
descriptors and runtime evolve independently. An ejected component pins the
version hash it was built from and rides the standard skew headers
(`api-engine-design.md` §10), so leaving the runtime keeps drift detection.

## 5. The `app` manifest [mixed: binding grammar **normative**, everything else illustrative]

Read the marker precisely, because the two halves of this section have
different status and conflating them is how the wrong thing gets built:

- **Normative and no longer illustrative:** the **binding grammar**, meaning the
  `$param: expr` vs `field == expr` distinction and the legal-where table below.
  It is the definition language's composition grammar verbatim, not a sketch.
- **Illustrative and still open:** **block set, page composition, and layout
  hints**; whether `detail` accepts an arbitrary single-row op; the
  **page-name vs op-name** namespacing rule; and the `app` ↔ `domain`
  namespace collision. Those are §10's open items, and a manifest cannot be
  compiled against this section until they land.

A small declarative layer naming apps and their pages:

```
app portfolio {
  page dashboard {
    metric monthlyBurn
    table  projectCosts live
  }
  page project($id) {
    detail std.projects.whole($id: $id)
    table  std.tasks.list(project == $id)
  }
}
```

- **Compile-time checked against the catalog.** An unknown op, a missing
  required param, a wrong param type: each is a build failure at compile time.
  Everything the manifest names is verifiable because everything registered
  is data.
- **Pages take params** (`page project($id)`), bound into the ops they host
  with **exactly the definition language's composition grammar**. There is no
  manifest-specific dialect. Inside the parens, *dollars bind and fields
  filter*:

  | Form | What it is | Legal where |
  |---|---|---|
  | `$param: expr` | a **parameter binding** on the callee | always; **every required param of the op must be bound this way** |
  | `field == expr` | a **condition over the op's output fields** | only when `field` is in the op's output manifest, and never as a way to pass a param |
  | `opName($id)` | **positional binding** | **never; it does not parse.** An earlier draft of this section used it |
  | `opName()` | empty parens | never; omit the parens entirely when there is nothing to pass |

  So `std.projects.whole($id: $id)` binds the op's `$id` parameter to the
  page's `$id`, and `std.tasks.list(project == $id)` filters `std.tasks.list`'s
  output field `project`. These are two different mechanisms that happen to look alike.
  Both appear in the example above deliberately. The condition form carries the
  ordinary **post-take** composition semantics (`definition-language.md` §5):
  the callee finishes first and the condition applies to its result. For
  a keyset-paged list op this means the filter narrows the page and leaves the scan unchanged.
  Pages come back short or empty while `next` is still present, and the runtime
  pages to exhaustion and does not treat an empty page as the end (§2).
  Where a page wants server-side narrowing on a large entity, the answer
  is an op that takes the parameter. The `$refine` envelope has the *same*
  post-take semantics (`api-engine-design.md` §7), so it buys ergonomics and
  no selectivity. A condition should not be asked to do filtering work it was not designed for.
- **Rows auto-link** to detail pages via typed-id deref: an id column whose
  entity type matches a page's param links there. **At most one page per
  entity id type per app** may be the deref target, and ambiguity is a manifest
  compile error. When no page claims the type, the fallback is a generated detail over
  `std.E.whole`.
- **Every required param must be bound, and `filled` is how a runtime-supplied
  one satisfies that** [settled: the requirement and the semantics. **The
  spelling is NOT ratified**: `filled` appears nowhere in
  `definition-language.md`. §2 Parameters shows only `$min: int <= 100`,
  `$since: date`, `$payee: text(64)` and no such mark, and the spec still says
  only "SDK-fillable". An earlier revision of this bullet claimed the spelling
  was "ratified in `definition-language.md` §2". It was not, and the claim is
  **withdrawn**: this document cannot ratify a token in a grammar it does not
  own, and a reader who took the claim at face value would have written a
  parameter mark the compiler does not lex.] Three facts collide and
  the collision was blocking: the manifest requires every required param bound;
  `today` is legal only in shape defaults, never in a query
  (`definition-language.md`), so execution-relative dates *must* arrive as
  parameters; and the mechanism that supplies them was named "SDK-fillable" in
  the spec with no spelling anywhere. An author facing that defensively names
  every execution-relative parameter `$today` and refuses to rely on any
  auto-fill. A doc gap distorted the API shape, which is exactly what happened
  in review.

  The compiler has to know which params the **caller** supplies and which the
  **SDK/runtime** fills at call time. Three candidate rules exist: by name
  (`$today`), by type (`date`), or an explicit mark. Only the mark scales:
  by-name is a convention the compiler cannot check, and by-type makes every
  date parameter magic and unbindable. So the mark is settled as a
  **requirement**; the word below is this document's **proposed** spelling,
  used throughout for concreteness and nothing more:

  ```
  op thirsty($today: date filled) = …
  ```

  - A `filled` param is **bound by the runtime, not the caller**: the generated
    SDK supplies it (execution date in the caller's declared timezone), and the
    app manifest **satisfies the required-param check without binding it**.
  - It is still an ordinary parameter everywhere else: typed, bounded,
    validated at the door, present in the manifest and the catalog, and
    explicitly bindable by a caller that wants to control it (which is what
    makes reproducible replays and `reproducible` ops possible at all).
  - The **catalog carries the mark**, so the SDK generator, the app-manifest
    checker, the op runner's generated form, and an agent reading the catalog
    all learn it from one place rather than from a naming convention.
  - **A `filled` param is never required-but-unbound at any layer.** The app
    manifest's required-param check treats it as satisfied; the generated form
    omits it (or shows it as an overridable advanced field); the door still
    validates it like any other param when a caller does send one; and an
    unfilled `filled` param at call time is an SDK bug and no wire error. The
    SDK supplies it unconditionally.
  - **Interaction with `reproducible`** [settled, ruled the other way]: this
    document previously argued that `filled` and `reproducible` compose, on the
    grounds that the value is an argument merely defaulted by the runtime.
    `definition-language.md` §2 ruled otherwise: **`filled` + `reproducible` on
    the same op is a compile error**, because in practice the fill *is* a clock
    read and a `reproducible` op that reproduces only when the caller remembers
    to bind explicitly is not reproducible. A generated surface that needs both
    takes the value as an ordinary required parameter and lets the calling app
    decide where it comes from.
  - **Spec-side dependency, stated as a dependency**: `definition-language.md`
    §2 (parameters) owns the final spelling and the grammar slot. Its
    "`$today: date`, SDK-fillable" sentence is the requirement without a
    mechanism, and this is the mechanism. This document owns the *obligation*
    that some checkable mark exist, because §5's manifest cannot compile
    without one; if the spec ratifies a different word, the behaviour above is
    the contract and the spelling follows.
    **Status, stated exactly** (finding X-079, now closed): the spec ratified
    the spelling. `filled` is `[settled]` in `definition-language.md` §2 with
    the grammar slot `$today: date filled`, and is in the reserved-word list.
    §5's required-param check is unblocked. The one residual is implementation:
    the lexer's keyword set must carry it, or the token parses as an
    identifier.
- **Reduce-ended ops are metrics [settled]:** a result shape that is
  statically one row (the body ends in `reduce`) is detectably scalar, and the
  descriptor becomes a metric card with `money`/`int` formatting. Detection is structural
  and never by naming convention.
- **One-row `group` results are metrics too, when the shape proves it**
  [proposed]. `reduce` is not the only way to a single row, and the gap showed
  up immediately in review: an op ending in `group` that yields exactly one row
  rendered as a one-row table instead of a metric card, with no way to say
  otherwise. The rule stays structural: the descriptor asks whether the result
  is **statically single-row**, and answers yes in exactly three cases:
  1. the body ends in `reduce` (today's rule);
  2. the body ends in `group` over a **constant** key set (nothing varies
     per row), so one bucket exists by construction;
  3. the body ends in `take 1` over a totally-ordered pipeline.

  Everything else is a table, including a `group` whose key *happens* to have one
  distinct value in today's data. "One row right now" is
  data and not shape, and a descriptor that flips between card and table as rows
  arrive is worse than a consistent table. An author who wants a card from a
  genuinely multi-bucket op writes an op that reduces. That is a one-line
  change and it makes the intent legible.
- **Navigation derives from pages.** There is no route table; the manifest is the
  information architecture.
- **Deployment:** app manifests are definitions. They ride the same
  `plan`/`apply` engine as every other kind, version-hashed, diffed,
  hot-swapped. They are IaC-owned (code, not runtime state) and referenced from
  identity clients (§8 of the definition language).

This is the top of the two-pages-to-working-app story: shapes + ops + a
manifest, written by hand or by an LLM. The grammar is tiny and closed, and every
name it utters is checkable, so generated manifests either compile or fail
loudly.

## 6. Multi-app [settled: mirrors the §8 pin]

**Apps are plural per deployment.** Apps sit above domains; several apps
share the deployment's one OIDC issuer and may share a single identity
client, so a user signs in once and every app is live. An app references ops from **any
enabled domain**; the manifest compile-check spans domains. Two apps over one
space are two windows onto one world, with the same membership, profiles and
chain. App ↔ domain ↔ space stay deliberately many-to-many.

Cross-space app interfacing rides the sharing doors and adds nothing:

- **Guest windows** for live foreign reads: guard-bearing ops, attributed
  as the guest, revocable at the next call.
- **Feeds are the blessed app-to-app interface.** Copies land as ordinary
  writes on the receiver's chain (`source:feed-<name>`), so the receiver's
  own triggers and rules fire on them: cross-space reactivity becomes local
  reactivity.
- **Tight coupling** = a tier-2 service principal holding capability tokens
  for both spaces, consented on each side.

## 7. Relationship to the dashboard [settled]

Per `dashboard-plan.md` §5: **same primitives, different products.** The
dashboard renders deployment-owner surfaces from this metadata plus platform
internals; generated UI renders end-user app surfaces from schema + op
metadata alone. The seam is the shared surface types (schema-driven table,
detail, and form renderers), and dashboard **panes are consumers of this
layer**: a pane is configuration over these descriptors and the headless
runtime plus a custom fragment. Work on either strengthens the other;
neither is a fork.

## 8. Custody interactions [settled]

- **Descriptors are metadata**: envelope-tier and value-free. Catalog scoping
  (§1) means you only ever hold descriptors for ops your token can call;
  there is nothing custody-sensitive in one.
- **Client-first spaces render identically.** Same descriptors, same runtime,
  same skins. The ops execute on-device against the local store via the
  embedded engine (`implementation.md`); only the execution venue moves. The
  UI layer cannot tell, by design.
- **Sealed fields render as sealed markers** when the session does not
  lawfully hold key release, the same rule the dashboard states in its §2.
  The descriptor carries the field's sealed mark (from the B1 descriptor), so
  skins render the marker and no empty cell.

## 9. Sequencing [lean]

1. **Surface descriptors** out of `ordoc build`, which is cheap once artifacts exist
   (a projection of `interface.json` + annotations). **`interface.json` is
   built** as of 2026-09-07 (`ordo interface`, `GET /interface`;
   `tooling-and-embedding-plan.md` §B1), catalog-scoped and shape-verified as
   §1 requires, with §2's typed-id action discovery and §B3's terminal verbs
   already stamped on it. What remains for *surface* descriptors is the
   second half of §1's sentence: the reserved annotations `label`, `desc`,
   `order`, `choices`, `danger`, which `definition-language.md` §10.3 marks
   inert and the lexer does not carry. **That lexing is the blocker, and it
   is a language change and no descriptor change.** B1 deliberately
   emits none of them, because an always-null `danger` is a field a renderer
   would learn to trust.
2. **`@hominemai/ordo-react`** headless runtime: fetch/live/snapshot/paging/
   refinement/commit wiring. **Partly built** as of 2026-09-07
   (`clients/ordo-react/`, checklist 14b): fetch, keyset paging to
   exhaustion, form derivation from `params[]`, §2's typed-id action
   discovery, and the CAS-by-default commit path with its conflict state
   are in, tested against the interface golden with no server. **Live and
   the device copy are built** (2026-09-29; see §2's status note): opt-in at
   the provider, paced, and answering syncable ops from the SDK's replica.
   **The page-level snapshot coordinator is not**, and the reason is a
   dependency rather than an omission: the coordinator holds one command-log
   position across every surface on a page, and `interface.json` carries no
   log position on the read path, so a hook-level approximation would ship
   exactly the split-snapshot failure §2 names. **Refinement is built**
   (2026-10-05, refinement R5 and R6): typed `refine` and `pick` on the read
   hooks, no local tier refusal, and refined live reads on the shared stream.
3. **Default skins**: the four components, CSS-variable theming.
4. **App manifest**: grammar, compile checks, deref linking, deployment.
5. **Ejection**, last, because it needs stable descriptors to eject from.

Static renderers land opportunistically alongside 3. They are pure
functions with no runtime dependency.

## 10. Open items

- Manifest final grammar (§5 is illustrative for **blocks, page composition,
  and layout hints**; the **binding grammar is no longer illustrative**; it is
  the definition language's composition grammar verbatim, §5). Still open and
  still blocking a settled mark: **page-name vs op-name namespacing**, and
  `app` ↔ `domain` namespace collision. A manifest that names a page
  `projectCosts` alongside an op `projectCosts` currently has no stated
  resolution, and authors have been sidestepping it by hand-keeping the two
  sets disjoint.
- Whether `detail` accepts an **arbitrary single-row op** or only
  `std.E.get`/`std.E.whole`. Every example shows the latter, nothing states
  the former is illegal, and the single-row test above (§5) now gives it a
  usable definition if the answer is yes.
- Theming token inventory: which `--ordo-*` variables are the contract.
- **Form generation from write-op params [settled]**: `useOpForm`
  (`clients/ordo-react/src/hooks/useOpForm.ts`, field derivation in
  `src/form.ts`) derives a field model from any write op's `params[]`
  today, shipped in `@hominemai/ordo-react`. Required/optional and `max`/`min` bounds
  read off the param; reference fields (`id_entity`) resolve to
  candidate-picker sources through the v2 descriptor's `id_types[].list`
  and `.get`; dropdowns come from `field_ref` + `enum_values`, the
  declared field's own enum set resolved by the emitter, not the `choices`
  annotation this item originally named, which still does not lex (§9.1).
  The mechanism runs against any op that has `params[]`, not only
  `std.E.create`; `danger` has no shipped annotation to read yet, so the
  hook proxies it from the op's `kind` (a delete is destructive by
  derivation) until `definition-language.md` §10.3's `danger` lands.
  **Form layout [lean]**: field arrangement and grouping in a rendered
  form are still undecided; a field model is settled, a layout is not.
- Static-renderer inventory beyond the four named (PDF? Slack blocks?),
  demand-gated.
