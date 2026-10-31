# Round-3 AI-buildability judgment — multi-tenant incident response

## Scope and method

This is a static judgment only. I did not execute, parse, compile, or test either attempted definition. I first read the cheap and strong attempts, then the prescribed normative corpus, and used `docs/critique-consolidated.md` §2/§8 plus `review/findings.json` only to verify whether earlier failures were closed, moved, or remained open.

Line citations below refer to `review/ai-build-cheap-round-3.md` as **C** and `review/ai-build-strong-round-3.md` as **S**. “Spec compilation” means compilation against the written language, not the repository's current implementation. README's current-status section independently says neither app is runnable today because guards, ACL binding, planner/executor, apply, and generated UI are not implemented.

Severity is about the attempted app: **fatal** means the promised security/tenancy model cannot safely be delivered; **critical** means a core brief item is absent or silently wrong; **major** means likely rejection or a material behavior/UI defect; **minor** means a contained defect or uncertainty; **note** is a non-defect qualification.

## Verdict

| Tier | Likely written-spec compilation | Semantic/security result | Brief coverage |
|---|---|---|---|
| Cheap | **Fail**. Unsupported nullable parameter spellings, missing `unbounded`, invalid `members.name` projections under the normative member shape, an unbound page parameter, and an invented `form` surface are independent blockers. | **Unsafe/incomplete.** Organization ACL state never governs incident rows; guest sharing is not exact-one; membership administration is comments rather than an emitted/runtime surface; authorship is spoofable; idempotent creation can silently edit. | Shapes and most CRUD intent are recognizable, but tenant administration, exact guest sharing, trustworthy attribution, stable paging, complete generated UI, and safe offline behavior are not fulfilled. |
| Strong | **Fail as written, but close in its entity/op/routine model.** The split `access read ... or` line and three source/projection line breaks violate the line-oriented grammar; `unique guest` has no supported spelling. The app block remains illustrative and `detail ... live` is open. | **Much better model, still not safely deployable.** It correctly makes organization access govern incidents, isolates notes, uses CAS, uses immutable-id paging, and supplies runtime exposure intent. Silent create-key convergence, assignee eligibility, bootstrap ordering, invitation/profile machinery, uniqueness scope, and runtime space propagation remain. | Substantially covers every requested area. Atomic *whole-tenant* bootstrap and compile-settled create-page metadata remain impossible or unsettled; “idempotent create” is only create-or-converge, not create-once. |

The strong tier is decisively better on business modeling and silent-wrong avoidance. Its surprising regression is lexical: full-corpus knowledge did not prevent line-oriented formatting mistakes. The remaining gap after those local edits is dominated by specification and platform holes, not by lack of builder understanding.

## Findings

### Compilation and surface-definition findings

#### BJ3-001 — Strong guard continuation ends a line after `or`

- **Severity:** major
- **Tier(s):** strong
- **Attempted citation:** S34–35, `access read organization.organizationsAccess(...) or` followed on the next line by `incidentGuests(...)`.
- **Violated spec:** `docs/definition-language.md` §2 “Lexical formalization,” line structure; `docs/engine-plan.md` G6. A step ends at end-of-line unless an expression delimiter is open.
- **Responsibility:** author error.
- **Explanation:** the reverse-guest disjunct is not inside an open parenthesis at the line break. The surrounding entity brace cannot make every entity line one continued expression. Put the whole guard on one logical line or open parentheses around the complete expression. This is a loud parse failure, not silent wrong behavior.

#### BJ3-002 — Strong detaches projections from their sources

- **Severity:** major
- **Tier(s):** strong
- **Attempted citation:** S97–99, S102–103, S138–140. Each source line closes its parentheses before the newline; the next line begins with `[...]`.
- **Violated spec:** `docs/definition-language.md` §2 line structure and §5 “Core form”; `docs/traps.md` “Brackets mean what their position means.” Projection brackets belong to the source form and are not a standalone transform.
- **Responsibility:** author error.
- **Explanation:** these likely reject lexically even though the intended queries are obvious. S130–132 and S159–161 correctly keep the projection on the source line and show the viable spelling.

#### BJ3-003 — `unique guest` has no normative grammar

- **Severity:** major
- **Tier(s):** strong
- **Attempted citation:** S69–75, specifically `unique guest`.
- **Violated spec:** `docs/definition-language.md` §3 “Refs” says field uniqueness is a field modifier (for example `key: text unique`) and composite constraint syntax is `unique (a, b)`; the conformance corpora use `unique (refA, refB)` for ref keys.
- **Responsibility:** mixed — author error plus full-spec fault.
- **Explanation:** a ref cannot take the scalar-field postfix spelling shown for `text unique`, and no prefix `unique field` form is specified. The defensible spelling is `unique (guest)`, but single-ref constraint syntax lacks a direct worked example. Since this constraint drives both exact-one sharing and converge-or-create mode, it is not cosmetic.

#### BJ3-004 — Cheap invents nullable operation-parameter types

- **Severity:** major
- **Tier(s):** cheap
- **Attempted citation:** C61–67, C74–80: `$dueOn: date?`, `$assignee: id?`.
- **Violated spec:** `docs/definition-language.md` §2 “Types” and “Parameters.” `?` is specified as shape-field sugar in §3, not as a parameter type constructor; the parameter inventory does not define nullable params or defaulted-param spelling.
- **Responsibility:** mixed — onboarding/full-spec fault and author guess.
- **Explanation:** likely binding/parser rejection. The builder explicitly called out the guess at C150. The strong tier correctly avoids the hole with separate assign/clear and schedule operations. The language still needs a settled way for a form to submit nullable field values.

#### BJ3-005 — Cheap omits required `unbounded` acknowledgments

- **Severity:** major
- **Tier(s):** cheap
- **Attempted citation:** C88–91, C97–107, and routine C109–115.
- **Violated spec:** `docs/definition-language.md` §5 “Cost classes” (“an op with no provable row bound fails registration unless acknowledged with `unbounded`”) and §8 run flags; `docs/traps.md` routine flag example.
- **Responsibility:** mixed — onboarding-doc fault and author error.
- **Explanation:** filtering by a non-unique organization/incident ref proves no maximum. `incidentList`, timeline/task/note lists, and the nightly full-task scan are unbounded. The cheap corpus included traps/tutorial, but the tutorial says the plantcare routine “needs none” despite its own apparently unbounded scan, weakening the lesson. Strong consistently carries the flag.

#### BJ3-006 — Cheap projects a nonexistent normative member field

- **Severity:** major
- **Tier(s):** cheap
- **Attempted citation:** C90, C95, C102, C106: `assignee.name` / `author.name`.
- **Violated spec:** `docs/definition-language.md` §3 “members — the principal entity,” whose closed field table has `id`, `principal`, `kind`, `role`, `status`, `joined`, but no `name`; `review/findings.json` Y-026.
- **Responsibility:** onboarding-doc fault/full-spec fault, not fairly an author error.
- **Explanation:** this should be an unknown-field compile error under the normative member shape. The tutorial itself uses `assignee.name` in its Part 2 `projectBoard` example, directly training the cheap builder into the error. Strong correctly returns typed member refs and explicitly reports the missing display-name design hole.

#### BJ3-007 — Cheap list page cannot bind `incidentList`

- **Severity:** major
- **Tier(s):** cheap
- **Attempted citation:** C118–121: `page incidents { table incidentList live }` while C88 requires `$organization`.
- **Violated spec:** `docs/ui-gen.md` §5 and `docs/tutorial.md` §6: every required op parameter must be bound with `$param: expression`; missing bindings are manifest build failures.
- **Responsibility:** author error, clearly signaled by onboarding docs.
- **Explanation:** there is no page parameter or other surrounding organization value. This fails loudly rather than selecting a tenant implicitly. The strong organization page binds `$organization: $id` correctly.

#### BJ3-008 — Cheap invents a `form` manifest clause

- **Severity:** major
- **Tier(s):** cheap
- **Attempted citation:** C128–130, `form createIncident`.
- **Violated spec:** `docs/ui-gen.md` §3/§5 and §10. The manifest's illustrated clauses are `metric`, `table`, and `detail`; form generation is derived from write parameters and remains `[lean]`. Final block grammar is explicitly open.
- **Responsibility:** mixed — author invention caused by an onboarding/UI-spec fault.
- **Explanation:** `form` is not in either closed reserved list and has no declared manifest grammar. Strong avoids inventing it and relies on action discovery, which is the documented direction, though it does not give the product a dedicated create page.

#### BJ3-009 — Strong's app syntax rests on unresolved grammar

- **Severity:** major
- **Tier(s):** strong
- **Attempted citation:** S189–216, especially arbitrary `detail incidentDetail(...) live` at S201/S212.
- **Violated spec:** `docs/ui-gen.md` §5 and §10: app block/layout grammar is illustrative; whether `detail` accepts an arbitrary single-row op is open; `live` on `detail` is not pinned.
- **Responsibility:** full-spec fault.
- **Explanation:** the strong author used the most plausible grammar and disclosed the uncertainty. This is not a reasonable author-error finding. List/detail/create metadata cannot be promised compile-settled until the UI grammar is finalized. Action discovery plausibly supplies the create form, but the brief's dedicated create UX is not explicitly placeable.

### Authorization, tenancy, and membership findings

#### BJ3-010 — Cheap's organization ACL never governs incidents

- **Severity:** fatal
- **Tier(s):** cheap
- **Attempted citation:** C11–20 and C140. `organization -> organizations` is unowned, and `incidents` has an independent ACL.
- **Violated spec/brief:** brief: admins manage all organization incidents and responders read/write organization incidents; `docs/modeling-guidance.md` §D says container ACLs govern their owned subtree, while explicit/separate roots do not compose; `docs/definition-language.md` §3 G6/G8.
- **Responsibility:** mixed — author model failure and onboarding-doc fault.
- **Explanation:** an organization admin has no incident access unless separately granted on each incident. Conversely, the incident creator gets incident admin even if merely a responder. Supplying an organization id labels the row but establishes no organization-authority check. This is plausible, silent wrong behavior: queries and writes succeed, just under the wrong tenancy rule. Strong fixes it by owning incidents under organizations and explicitly walking the organization ACL.

#### BJ3-011 — Cheap does not emit membership administration or door exposure

- **Severity:** critical
- **Tier(s):** cheap
- **Attempted citation:** C54–56 is only a comment; C141 and C159 admit no emitted names/signatures/profile surface.
- **Violated spec/brief:** brief: organization admins can manage membership and guests; `docs/definition-language.md` §3 `acl` derives named grant/revoke/level-change ops, while §8 says profiles/grants/invitations are runtime state; `docs/std-library.md` §3 exposes introspection but no invitation/suspension inventory.
- **Responsibility:** mixed — onboarding-doc fault and design hole, with an incomplete authored handoff.
- **Explanation:** ACL role rows for already-bound principals are only half of “membership.” Platform invitation, acceptance, suspension, profile exposure, and guest exposure are not emitted or supplied as necessary runtime declarations. Strong's S219–230 exposure table is the right kind of explicit handoff, while also correctly noting the missing platform op inventory.

#### BJ3-012 — Whole-tenant atomic bootstrap is not expressible

- **Severity:** critical
- **Tier(s):** both
- **Attempted citation:** C48–52/C154 and S81–86/S248–250.
- **Violated spec/brief:** brief asks for atomic organization/bootstrap; `docs/definition-language.md` §3 ACL bootstrap only atomically couples an ACL-bearing *domain row* and creator access row. Space creation, owner `members` row, domain enablement, organization row, and organization ACL row have no one authored atomic surface.
- **Responsibility:** design hole.
- **Explanation:** cheap overstates its operation as the bootstrap; strong accurately limits what it achieves. At best, each definition atomically creates `organizations` plus its ACL grant after a separate platform space/member/domain bootstrap. The whole requested tenant bootstrap remains multi-stage.

#### BJ3-013 — ACL creator bootstrap is normatively contradictory

- **Severity:** fatal
- **Tier(s):** both
- **Attempted citation:** C48–52 relies on the automatic grant; S81–86 adds `unguarded` to bypass the conflict.
- **Violated/conflicting spec:** `docs/definition-language.md` §3 G10 says create guards run post-defaults/pre-insert before anything is inserted; the same section's ACL bootstrap says the creator grant is minted in the same group and calls it the sanctioned escape. `review/findings.json` Y-003 remains open while Y-102 is marked closed.
- **Responsibility:** full-spec fault/design hole; judge disagreement recorded.
- **Explanation:** under the intended special-case reading, cheap is correct and strong's `unguarded` is unnecessary exposure. Under literal G10 ordering, cheap deadlocks and strong is the only executable workaround. The documents do not define when the derived ACL create guard is waived or when the grant becomes visible. I do not silently select one. Strong correctly relocates safety to owner-only door exposure, but that exposure is runtime convention rather than source-checked safety.

#### BJ3-014 — Cheap guest access is not “exactly one”

- **Severity:** critical
- **Tier(s):** cheap
- **Attempted citation:** C54–56/C140–146: guest gets a `read` row on an incident ACL, with no per-guest uniqueness constraint.
- **Violated brief:** an invited guest can read exactly one shared incident.
- **Responsibility:** mixed — author omission and onboarding-doc fault.
- **Explanation:** nothing prevents granting the same guest `read` on multiple incident ACL roots. Strong models a dedicated guest link with one row per guest, which is the correct storage approach if BJ3-003's spelling is fixed. The stronger model still cannot type-restrict `$guest` to a null-role external guest; an admin can pass an internal principal, an admitted design limitation (S256, S272).

#### BJ3-015 — Assignee eligibility is unenforceable in both models

- **Severity:** critical
- **Tier(s):** both
- **Attempted citation:** C19/C35 and writes C66–81; S26/S48 and writes S124–157.
- **Violated brief/security intent:** assignees should be organization responders/members, not arbitrary space-bound principals or guests.
- **Violated spec gap:** `docs/definition-language.md` §3 checks are row-local; write conditions have no cross-row existence precondition. `members` includes null-role guests.
- **Responsibility:** mixed — design hole, worsened by cheap's multi-org-per-space model.
- **Explanation:** every assignment accepts any `members` id in the space. In cheap, that can be a member of another organization; in strong, it can be a guest/null-role member. A UI picker over `organizationMembers` is not a server invariant. This is a crafted-request path that succeeds plausibly and silently.

#### BJ3-016 — Runtime space isolation remains outside either definition

- **Severity:** fatal
- **Tier(s):** both
- **Attempted citation:** S9's one-space-per-organization premise and S288–300; cheap implicitly relies on the same platform binding.
- **Violated platform contract:** `docs/api-engine-design.md` §12 calls space propagation a hard prerequisite; `review/findings.json` Y-002 and Y-015 remain open. Y-106 says the door binds space, but Y-015 says `OpExecutor::execute` drops it.
- **Responsibility:** design/implementation hole.
- **Explanation:** a correct row model cannot compensate for an executor or capability that loses the space dimension. Prior work closed the door-binding half and relocated the failure one layer later; it did not close end-to-end tenant isolation. This is fatal today but not a definition-author error.

### Idempotency, concurrency, chronology, and routine findings

#### BJ3-017 — Both “idempotent create” ops can silently edit an existing incident

- **Severity:** critical
- **Tier(s):** both
- **Attempted citation:** C58–64 and S105–114/S262. Both bind a unique key and mutable payload in one no-id `set`.
- **Violated behavioral requirement:** an idempotent *creation* retry should not become an unversioned edit with a changed payload; `docs/definition-language.md` §6 says unique-key mode is converge-or-create and touches described fields.
- **Responsibility:** mixed — design hole plus misleading naming; strong discloses it, cheap does not.
- **Explanation:** reuse of the key with a changed title/severity/etc. updates the existing row without `rev`. Transport fingerprinting (`api-engine-design.md` §9, Y-104 closed) protects only the same replay key during cache life; it does not make a semantic request key immutable. In strong, the statement also resets status to `open`. This bypasses the otherwise careful CAS story and is the most important silent-wrong behavior common to both tiers.

#### BJ3-018 — Cheap key scope is globally over-restrictive and tenant-coupled

- **Severity:** major
- **Tier(s):** cheap
- **Attempted citation:** C13/C61–64/C146, `key: text unique` rather than `(organization, key)`.
- **Violated brief/idiom:** idempotency is organization-local; `docs/definition-language.md` §3 supports `unique (a, b)` and `docs/traps.md` gives a composite example.
- **Responsibility:** onboarding-doc fault with a disclosed author compromise.
- **Explanation:** two organizations cannot use the same incident request key in one space. Strong uses the idiomatic composite key. Separately, Y-023 leaves the database's cross-space uniqueness scope open, so even strong's correctness is conditional on a platform ruling.

#### BJ3-019 — Cheap's “stable” incident list is not provably stable

- **Severity:** critical
- **Tier(s):** cheap
- **Attempted citation:** C86–91/C158, sorted by `createdOn`.
- **Violated spec:** `docs/definition-language.md` §5 “Determinism”; `docs/traps.md` pagination caveat; `docs/std-library.md` §2.2 derives full-row `std.incidents.update`; `review/findings.json` Y-017.
- **Responsibility:** mixed — author claim plus full-spec design fault.
- **Explanation:** the compiler considers registered writes, not only authored writes. Derived `std.incidents.update` can move `createdOn`, making the cursor `drifting`; the claimed no-skip/no-duplicate property does not follow. Strong's `sort -id` is genuinely stable because implicit identity cannot move and therefore successfully avoids, rather than closes, Y-017.

#### BJ3-020 — Cheap timeline lacks trustworthy and adequately ordered chronology

- **Severity:** critical
- **Tier(s):** cheap
- **Attempted citation:** C23–27/C71–72/C97–99/C147–148.
- **Violated brief/security intent:** incident timeline entries should be chronologically useful and attributable.
- **Relevant spec:** `docs/definition-language.md` §2/§3 supports `datetime = now` and `$caller` in write RHS; sort gets an id tiebreak but does not restore sub-day event time.
- **Responsibility:** author error encouraged by onboarding examples.
- **Explanation:** `date` collapses every same-day event to one timestamp bucket, while `by: text` is client-supplied and spoofable. Strong correctly uses `datetime = now`, `author -> members`, and `author: $caller`, then sorts by time-ordered UUIDv7 id.

#### BJ3-021 — Cheap responder-note author is spoofable

- **Severity:** critical
- **Tier(s):** cheap
- **Attempted citation:** C83–84, caller supplies `$author`.
- **Violated security intent/spec idiom:** `docs/definition-language.md` §3 G4 and `docs/guards-conformance.md` R-G2 explicitly permit `author: $caller`.
- **Responsibility:** onboarding-doc fault plus author error.
- **Explanation:** a legitimate responder can create a sealed note attributed to any member id. The row guard protects confidentiality but not integrity of authorship. Strong fixes this exactly.

#### BJ3-022 — Cheap overdue routine is overbroad and may not register

- **Severity:** major
- **Tier(s):** cheap
- **Attempted citation:** C109–115/C156–157.
- **Violated requirement/spec:** brief asks to mark overdue tasks; `docs/definition-language.md` §8 requires `unbounded` for unbounded routine scans; row lifecycle should not mark canceled terminal states merely because they are not `done`.
- **Responsibility:** mixed — author semantics and onboarding unbounded ambiguity.
- **Explanation:** every non-`done` past-due task becomes status `overdue`, including any future terminal text such as `canceled` because status has no check. Strong constrains status, excludes `done` and `canceled`, and uses a separate `overdue` boolean with an explicit false-to-true transition. Strong's routine is a good fulfillment of the brief, though generic std update exposure could still violate that lifecycle.

#### BJ3-023 — Routine authority wording remains inconsistent, but this clock routine avoids the fatal route

- **Severity:** minor
- **Tier(s):** both
- **Attempted citation:** C109–115 and S175–183.
- **Conflicting spec/prior finding:** `docs/definition-language.md` §8 says routines are trusted domain code for which guards pass; `docs/traps.md` says guards “do not apply”; `review/findings.json` Y-029 remains open and Y-004 concerns outsider-wakeable `ingested`/`fed` routines.
- **Responsibility:** full-spec fault.
- **Explanation:** for these clock-only, narrow routines, both readings select the same rows and Y-004's outsider wakeup is absent. This is not a defect in either routine's business logic. Strong correctly uses the now-expressible run flag slot (Y-101); cheap needed `unbounded`, not `unguarded`.

### UI/privacy and runtime-exposure findings

#### BJ3-024 — Cheap does not define a guest-safe callable surface

- **Severity:** critical
- **Tier(s):** cheap
- **Attempted citation:** C118–130 exposes the note table in the only incident app; C153 and C159 acknowledge no guest/profile surface.
- **Violated brief/platform model:** guests must never get private notes; `docs/definition-language.md` §8 separates app metadata from runtime profiles/guest exposure, and guest-exposed ops must be guard-bearing.
- **Responsibility:** mixed — incomplete author handoff plus missing finalized profile/share registration surface.
- **Explanation:** the responder-note row guard should return zero guest rows if it is enforced, so this is not itself a data leak. But the guest sees or may be offered an inappropriate surface, and the attempt never states which ops the guest may call. Strong correctly uses a separate guest app with no note/write surface and an explicit runtime exposure table. The app manifest remains UX, not an authorization boundary.

#### BJ3-025 — Current implementation cannot enforce either app's guards

- **Severity:** fatal
- **Tier(s):** both
- **Attempted citation:** all `acl`/`access` lines; both builders acknowledge pre-alpha status (C161, S240/S270).
- **Violated implementation claim:** README “Current status”; `docs/engine-plan.md` G15. Authorization is specified but not implemented/bound in the current parser/runtime.
- **Responsibility:** implementation gap, not author error.
- **Explanation:** if someone treated either file as deployable today, missing guard enforcement would return plausible cross-principal rows—the archetypal silent-wrong failure. This is a release blocker independent of written-spec quality.

#### BJ3-026 — Strong custom ACL vocabulary is only partially pinned

- **Severity:** minor
- **Tier(s):** strong
- **Attempted citation:** S14–16 and S254, `acl (responder, admin)`.
- **Relevant spec:** `docs/definition-language.md` §3 `acl` says names are application vocabulary, ordered left-to-right, and the highest gates administration.
- **Responsibility:** full-spec fault.
- **Explanation:** the spec clearly permits custom names and makes `admin` top-level here, so the builder's fear that custom vocabulary is categorically invalid is a false positive. What remains unclear is how a two-level vocabulary maps derived read versus write rights on the ACL-bearing `organizations` entity. Strong avoids relying on that for incident writes by spelling explicit organization-access guards.

## Brief-by-brief fulfillment matrix

| Brief item | Cheap | Strong |
|---|---|---|
| Multi-tenant organizations and members | **Fail**: organizations exist, but their ACL does not govern incidents and membership runtime is absent. | **Mostly pass at spec level**: one space per organization plus organization ACL; platform invite/suspend inventory remains missing. |
| Incident severity/status/assignee | **Partial**: fields exist; no checks; assignee can cross org/be guest. | **Pass with caveat**: checks and CAS ops; assignee eligibility still unenforceable. |
| Timeline entries | **Partial/fail integrity**: date-only and spoofable `by`. | **Pass**: datetime and `$caller`-bound member author. |
| Tasks | **Pass shape/basic CRUD**, with unchecked status. | **Pass**, with checked lifecycle and CAS. |
| Sealed private responder notes | **Confidentiality mostly modeled**, authorship spoofable and guest door surface unspecified. | **Pass at written-spec level**: sealed field plus explicit organization-only guard and separate guest app. |
| Admins manage membership/all incidents | **Fail**. | **Pass for ACL rows/incidents**, partial for platform invitation/suspension. |
| Responders read/write org incidents | **Fail** unless separately granted incident by incident. | **Pass** through organization ACL guards. |
| Guest reads exactly one, never notes | **Fail exact-one**, note guard likely filters if enforced. | **Pass in model** after fixing uniqueness syntax; cannot server-enforce “external guest only.” |
| Atomic organization/bootstrap | **Partial and overstated**; domain row+ACL only, with Y-003 conflict. | **Partial and accurately disclosed**; same domain-only atomicity and Y-003 workaround. |
| Idempotent incident creation | **Partial/silent-wrong**: global key and unversioned convergence. | **Partial/silent-wrong**: scoped composite key, but changed payload silently edits/reset status. |
| Stable paginated incident list | **Fail claimed stability** because std update moves `createdOn`. | **Pass** via `sort -id`; pagination is correctly left to the call envelope. |
| Realtime incident-detail stream | **Partial**: live child tables, non-live detail header. | **Semantically pass**, grammar-unsettled: live detail and child surfaces use invalidation/refetch. |
| Nightly overdue routine | **Partial**: correct broad shape, missing `unbounded`, overbroad statuses. | **Pass** for the brief. |
| Generated list/detail/create UI metadata | **Fail**: unbound list and invented `form`. | **Mostly pass directionally**: list/detail pages and action-derived form, but app/detail grammar remains open and no dedicated create page is placeable. |
| Two offline devices editing same incident | **Partial**: explicit update uses CAS, but unique-key create bypasses it. | **Pass for ordinary edits**: CAS and user-visible retry; semantic create convergence remains a bypass; client-first sequencing remains Y-031. |

## What both tiers got right

- Both use implicit `id`/`rev` rather than inventing primary keys, and both choose CAS for ordinary same-row incident edits.
- Both correctly model timeline/tasks/notes as owned incident children, so guard inheritance is available.
- Both state both access verbs on the note override, satisfying the Y-100 fix; neither reintroduces write-open-by-omission.
- Both understand that sealed custody and row authorization are separate controls.
- Both use the documented clock-trigger / query / `each -> set` routine form and UTC `Z` schedules.
- Both correctly leave paging in the API envelope rather than inventing OFFSET/page-number syntax.
- Both recognize runtime profiles/grants as runtime state rather than trying to encode user consent in IaC.
- Both correctly decline to add notifications when the brief did not request them.

## Builder-note false positives and well-founded fears

### Cheap false positives resolved by the full spec

- **Derived ACL name/walk:** C142's guess `incidentsAccess` and `incident.incidentsAccess(...)` is supported by `docs/definition-language.md` §3 `acl` and the tutorial's `projectsAccess` examples. The guard spelling is valid.
- **Guest `$caller` reconciliation:** C144 is resolved by `docs/definition-language.md` §3 “members”: invitation/guest binding creates a null-role `members` row and `$caller` binds its space-local id.
- **Multiple ACL roots:** C145's concern is not a compilation issue. Multiple ACL roots linked by an unowned association are legal. They simply do not compose—which is the real semantic failure BJ3-010.
- **Custom ACL levels/order:** C143's enumeration of `write`/`admin` is valid and conservative; ACL levels are ordered left-to-right. The note guard does not need a mysterious comparison operator.
- **Platform ACL bootstrap concept:** C154 correctly identifies the documented domain-row+creator-grant mechanism. The remaining problem is the G10/Y-003 ordering contradiction, not ignorance of the intended rule.

### Cheap fears that remain justified

- Nullable parameter syntax (C150), explicit form placement (C151), automatic page binding (C152), and invitation/profile operation names (C141/C159) are not settled by the full corpus.
- The absence of a trusted display name and a direct onboarding example for `$caller` authorship caused real defects.

### Strong false positives or over-cautions

- S254's custom ACL vocabulary is expressly permitted; only the precise derived rights of a two-level vocabulary remain unclear.
- S266's `unbounded` use is not merely a forced guess under the latest spec: the row-cardinality gate explicitly requires it for logically unbounded lists, independent of call-time page size.
- S270's no-`std sealed` choice is clearly correct: std defaults to the open skeleton, while the hand-written responder-note op may intentionally project the sealed body.
- S276's clock routine does not trigger Y-004's outsider-wakeable confused-deputy path.

## Documentation gap between tiers, ranked by actionability

1. **Make every example obey line structure and add a one-screen formatting rule.** The strong tier's BJ3-001/BJ3-002 errors are mundane and fully preventable. Show legal multi-line guards and source projections side by side with rejected forms.
2. **Close create-once idempotency or stop calling converge-or-create “idempotent creation.”** Both tiers emitted the same silent edit path (BJ3-017). Add an immutable-key/assertion mechanism or make docs name it “idempotent ingestion convergence” and show changed-payload behavior prominently.
3. **Publish a complete incident/org authorization recipe.** Onboarding teaches one ACL root but not organization-wide authority plus per-incident guest windows. That missing composition recipe directly produced cheap's fatal BJ3-010/BJ3-014.
4. **Finalize UI manifest/form placement grammar.** Both tiers could describe the desired UI but neither could write a compile-settled create surface (BJ3-008/BJ3-009). Settle `form`/action placement, arbitrary detail ops, and `live` on detail.
5. **Fix the member-display contradiction.** Remove `assignee.name` from the tutorial or add a normative display field/profile mechanism (BJ3-006/Y-026). This is an unusually direct example-to-compile-error pipeline.
6. **Settle nullable/defaulted op parameters.** Generated create/edit forms need to clear nullable dates and refs; the language currently teaches field nullability but not parameter nullability (BJ3-004).
7. **Define bootstrap ordering, not only desired outcome.** Fold an explicit ACL-create exception into G10 or reject the model and supply another atomic mechanism (BJ3-012/BJ3-013/Y-003). The current “closed” label is contradicted by the current finding list.
8. **Name platform membership and guest-exposure operations.** ACL grants do not invite or suspend principals. A complete app handoff needs stable callable names/signatures and a compile-checkable exposure/profile template (BJ3-011/BJ3-024).
9. **Add cross-row membership invariants or a blessed guarded assignment primitive.** Both tiers can silently assign a guest/wrong-org principal (BJ3-015); UI filtering is not enforcement.
10. **Reconcile std writes with cursor-stability analysis.** Strong can avoid Y-017 with `id`; ordinary create timestamps cannot honestly be called immutable while `std.E.update` writes them (BJ3-019).
11. **Keep `unbounded` teaching internally consistent.** The settled cardinality gate is clear, but tutorial examples suggesting scans “need none” invite cheap's BJ3-005/BJ3-022.
12. **Finish end-to-end space propagation before making multi-tenant claims.** Y-106 closed only the door half; Y-015 shows relocation to the executor (BJ3-016).

The highest-priority documentation issues are 2, 3, 5, and 10 because they create plausible successful output with wrong semantics. Parser failures are cheaper and safer: they stop the build. Silent convergence, wrong authority inheritance, invented display identity, and drifting cursors survive into apparently working applications.

## Prior-fix verification

| Prior fix | Judgment in round 3 |
|---|---|
| Y-100 — missing `access write` rejected | **Closed and effective.** Both tiers state both verbs on every explicit guard. Cheap no longer emits the old write-open-looking schema. |
| Y-101 — routine flag slot | **Closed.** Strong uses `run(...) unbounded:` correctly. Cheap's failure is discoverability/application of `unbounded`, not inability to spell it. |
| Y-102 — creator ACL bootstrap | **Relocated/reopened.** The desired rule is documented, but Y-003 exposes its conflict with pre-insert guard ordering. Strong moves safety to `unguarded` + door exposure; cheap remains conditional on the intended special case. |
| Y-103 — `let` discoverability | **Closed but not exercised.** Neither build needs `let`. |
| Y-104 — replay key scoping/fingerprint | **Closed for transport retries, not semantic create identity.** It does not fix BJ3-017 after a different/missing transport key or cache expiry. |
| Y-105 — DEK epochs | **Closed but not material to these definitions.** |
| Y-106 — bind space before door check | **Relocated.** Door binding is closed; Y-015 says executor propagation is still absent, so end-to-end isolation remains open. |
| Y-017 — std update defeats stable declared keys | **Open.** Strong avoids it with implicit id; cheap hits it with `createdOn`. |
| Y-023 — uniqueness space scope | **Open.** Strong explicitly depends on a ruling; cheap is also affected if multiple spaces share DDL. |
| Y-025 — SDK-filled params | **Open but avoided.** Neither app surface requires `$today`; routines receive it from the scheduler. |
| Y-026 — member display name | **Open and directly visible.** Cheap compiles against a field the normative shape lacks; strong degrades to ids. |
| Y-029 — routine rationale conflict | **Open, behaviorally harmless here.** Both routines are clock-only and narrow. |
| Y-031 — client-first sequencing/CAS | **Open.** Strong accurately limits its offline promise to conflict detection/replay under the intended interpretation. |

## Final judgment

The cheap tier is not a usable definition: it has several loud compile blockers and, more importantly, fatal silent authority mistakes. Its notes often identify the exact gaps, but identifying them did not produce a complete safe artifact.

The strong tier demonstrates that the full corpus materially improves AI buildability. After small local syntax corrections, its core entity/guard/op/routine model is coherent and idiomatic. It fulfills most of the brief at the written-spec level and is honest about what it cannot express. It still cannot be certified safe or complete because core requested behavior crosses unsettled boundaries: whole-tenant bootstrap, invitation/profile exposure, immutable semantic create keys, assignee eligibility, finalized UI grammar, space-scoped uniqueness, and end-to-end runtime tenant isolation.

Round 3 therefore shows a meaningful tier gap, but not a spec ready for “emit a complete secure app” evaluation. The strongest improvement opportunity is to turn the strong builder's caveats into compile-checkable language/platform constructs and to repair the onboarding examples that directly generated cheap-tier silent-wrong output.
