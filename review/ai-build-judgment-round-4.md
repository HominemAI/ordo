# Round-4 AI-buildability judgment — multi-tenant incident response

## Scope, method, and severity

This is a static review only. I did not execute, parse, compile, or test either attempted definition. I read the two round-4 attempts first, then the prescribed current normative corpus, and used the round-3 build reports, round-3 judgment, `docs/critique-consolidated-3.md` §§1/2/10, and `review/findings.json` only for direct regression/improvement and prior-fix verification.

Line citations below use **C4** for `review/ai-build-cheap-round-4.md` and **S4** for `review/ai-build-strong-round-4.md`. “Written-spec compilation” means the result a conforming implementation of the settled written rules should produce. It is separate from the current repository implementation: README lines 90–120 and finding X-001 say the project is pre-alpha, row guards/`acl` do not bind, and there is no planner/executor/generated UI. Both artifacts therefore fail against the current implementation regardless of their written-spec quality.

Severity is about the attempted application: **fatal** means the promised tenancy/authority model cannot safely be delivered; **critical** means a core brief item is absent or silently wrong; **major** means a likely build rejection or material behavioral/UI defect; **minor** means a contained defect or unresolved edge; **note** is a non-defect qualification.

## Verdict

| Tier | Likely written-spec compilation | Semantic/security result | Brief coverage |
|---|---|---|---|
| Cheap | **Fail.** The first incident create binds a converge-or-create write with `let` (`unbindable_write`), every unbounded list and the nightly scan omit `unbounded`, and the app/form grammar remains non-normative. | **Unsafe and incomplete.** The organization ACL still does not govern incidents; every member-created incident auto-grants its creator incident `admin`; caller-supplied group ids decide authority; the note override does not expand those groups; guest exact-one is unenforced; attribution is spoofable; creation still silently converges changed payloads. | Stable id paging, `displayName`, member refs, complete note guard verbs, and CAS edits improve over round 3. Atomic tenant genesis, admin/responder authority, exact-one guest access, trustworthy authorship, idempotent create, realtime root detail, and complete generated UI still fail. |
| Strong | **Core is close but not confidently compilable; complete artifact fails.** Settled shape/op/routine grammar is mostly sound, but `enum(...)` is only `[lean]` for fields and not specified for parameters, `acl` plus explicit guards on the same root has no coexistence rule, and `filled` on a routine parameter is not clearly owned. The app/profile layer is explicitly non-compilable. | **Substantially correct for direct-principal organization ACL rows under managed custody, with two material residuals.** Explicit organization guards are group-blind if an ACL row names a group, and the security-critical guest/profile exposure intent lives only in external YAML. The bootstrap call also appears to put app names in `clients[]`. | Backend brief coverage is strong: one-space-per-org genesis, create-once, assignee invariant, CAS, exact-one share row, private-note replacement guard, stable id paging, coordinated live reads, and bounded routine behavior. Generated UI/profile/guest invitation and managed-offline mutation delivery remain incomplete. |

Round 4 is a real improvement over round 3, but it is asymmetric. The full-spec tier now has a coherent backend design after a few unsettled grammar choices. The cheap tier has fewer incidental errors, yet its core silent-wrong risk did **not** materially decline: the same tenancy, guest, attribution, and create-convergence failures remain, and group-based note access introduces a new silent denial.

## Findings

### Compilation and language-surface findings

#### BJ4-001 — Cheap binds a converge-or-create write that cannot be bound

- **Severity:** major
- **Tier(s):** cheap
- **Attempted citation:** C4 53–58, especially `let incident = set incidents { ... clientKey: $clientKey ... }`.
- **Violated spec citation:** `docs/definition-language.md` 2069–2083 derives a unique-key no-id write as converge-or-create; 1380–1395 says only a statically CREATE-mode `set` may be bound and all other modes reject as `unbindable_write`. The cheap-tier onboarding corpus itself states the same rule at `docs/tutorial.md` 1216–1241 and `docs/traps.md` 837–855.
- **Responsibility:** mixed — author error plus onboarding-doc fault around the missing create-once recipe.
- **Explanation:** `clientKey` is unique and the statement has no `rev: null`, so its mode is `creates-or-converges`, not CREATE. It therefore cannot produce the single fresh row descriptor required for `incident.id`; compilation should stop before either access-row statement. The author had enough onboarding text to know that only creates bind, but the onboarding set does not teach the new `unique key + rev: null` create-once form that would make this statement bindable. This is loud rather than silent, but the obvious local “fix” of removing `let` would expose BJ4-010.

#### BJ4-002 — Cheap still omits every required unbounded-cardinality acknowledgment

- **Severity:** major
- **Tier(s):** cheap
- **Attempted citation:** C4 60–73 (`incidentList`, `incidentTimeline`, `incidentTasks`), 94–103 (`responderNotesForIncident`, `markOverdueTasks`).
- **Violated spec citation:** `docs/definition-language.md` 1782–1787 defines a bound as a statically provable candidate maximum; 1805–1819 requires `unbounded` for an op with no such bound independent of SQL/host venue; 2420–2433 gives routines the same flag slot. An equality on a non-unique parent ref does not cap child count, and the clock routine scans the space's task set.
- **Responsibility:** mixed — author error plus onboarding-doc fault.
- **Explanation:** all four list ops and the nightly scan grow with space data and should fail registration without the marker. The cheap builder had tutorial/traps, but the tutorial still undermines the rule: `docs/tutorial.md` 612–615 explains `run(...) unbounded:` and then says its apparently whole-space plant routine needs neither flag; 638–680 likewise presents unbounded open-field reads and says plantcare needs no flags. This is the surviving half of Z-023.

#### BJ4-003 — Strong relies on an unsettled field type and an unspecified parameter type

- **Severity:** major
- **Tier(s):** strong
- **Attempted citation:** S4 35–36, 52, and 66 declare enum fields; S4 152 and 186–187 declare enum operation parameters.
- **Violated spec citation:** `docs/definition-language.md` 134–138 calls the field-and-parameter type inventory closed and does not include `enum`; 348–353 describes `enum(...)` only as a `[lean]` **field type**. `docs/ui-gen.md` 376–379 also leaves enum-driven form behavior `[lean]`.
- **Responsibility:** mixed — deliberate author risk plus full-spec fault.
- **Explanation:** a settled-only parser has no basis to accept any enum declaration. Even an implementation choosing the lean field proposal has no written rule for `enum(...)` in a parameter slot. The builder discloses the field risk at S4 405 but understates the parameter problem. This is a new round-4 strong-tier compile regression: round 3 used `text` plus `check` constraints, which was less ergonomic but settled.

#### BJ4-004 — Neither tier can compile its requested app/profile/guest surface from the written spec

- **Severity:** major
- **Tier(s):** both
- **Attempted citation:** C4 106–119 and 133–137; S4 290–312, 341–381, and 389–395.
- **Violated spec citation:** `docs/ui-gen.md` 138–150 makes only `$param: expr` binding normative and leaves block inventory, page composition, arbitrary detail, and namespaces illustrative/open; 361–379 keeps final manifest/form grammar open. `docs/engine-plan.md` 49–55 recognizes `table`/`detail`/`live` contextually but does not include `form`. `docs/definition-language.md` 3014–3030 still lists generated UI as open.
- **Responsibility:** full-spec fault, with an additional cheap author collision.
- **Explanation:** the table/detail bindings are plausible intent, not a spec-compilable app. Both tiers invent `form`; both use `live` as a manifest modifier even though liveness is normatively a call-time `?live` subscription. Cheap additionally declares a page and op with the same name, `createIncident`, exactly where `ui-gen.md` says page/op namespace resolution is open. Strong correctly preserves and labels its guesses; cheap also labels the form guess. Neither fulfills “complete generated list/detail/create UI” as a buildable artifact.

#### BJ4-005 — `acl` plus explicit guards on the same root has no settled combination rule

- **Severity:** major
- **Tier(s):** strong
- **Attempted citation:** S4 21–28 combines `acl (respond, admin)` with root-level `access read` and `access write`; S4 411 expressly assumes replacement.
- **Violated spec citation:** `docs/definition-language.md` 994–1011 defines inheritance and replacement for a **child** taking an ancestor guard; 1189–1220 defines guards derived on the ACL-bearing entity and replacement again only for a child. No rule says whether a root's explicit pair replaces, composes with, or conflicts with its own ACL-derived pair.
- **Responsibility:** mixed — author assumption plus design hole.
- **Explanation:** the intended reason is understandable: a two-level ACL would ordinarily make the low `respond` level read-only, so the author replaces the root write guard to let responders write while retaining top-level admin on the access link. A conforming compiler cannot infer that coexistence rule from the current text. Rejection is safe; silently composing or replacing produces different responder authority. A canonical alternative/ruling is needed—either explicitly legalize root replacement or teach a vocabulary whose derived level-to-verb mapping meets the requirement.

#### BJ4-006 — The strong bootstrap handoff confuses apps and identity clients, and the wire route conflicts across normative docs

- **Severity:** major
- **Tier(s):** strong
- **Attempted citation:** S4 318 says `POST /ops/std.space.create`; S4 333–335 supplies `"clients": ["incidentConsole", "guestIncident"]`, exactly the two app names declared at S4 290 and 306.
- **Violated spec citation:** `docs/glossary.md` 60–79 distinguishes apps from OIDC identity clients; `docs/bootstrap.md` 177–185 has separate `apps[]`, while 246–251 makes `clients[]` required registered identity clients and says bootstrap binds but never creates them. `docs/api-engine-design.md` 11–21 owns the route as singular `/op/{name}`. In direct conflict, `docs/bootstrap.md` 458–466 illustrates plural `/ops/std.space.create`.
- **Responsibility:** mixed — author error plus full-spec fault.
- **Explanation:** unless separately declared identity clients intentionally happen to share the app names, preflight should reject `clients[]` as unknown. No client blocks or external IaC declaration accompany the “complete” handoff. `apps[]` could be omitted because all enabled apps default on, but that does not satisfy the distinct required client binding. The route cannot be judged correct against both owning documents, so this review records rather than resolves that conflict.

#### BJ4-007 — `filled` is settled for operation parameters but not clearly for routine-run parameters

- **Severity:** minor
- **Tier(s):** strong
- **Attempted citation:** S4 282–287, `run($today: date filled) unbounded:`.
- **Violated spec citation:** `docs/definition-language.md` 355–383 defines `filled` around generated SDK/client calls and shows only an `op`; routine examples at 2314–2329 and the run-header grammar at 2420–2433 use ordinary parameters and define only the flag slot. No routine example or scheduler rule assigns `filled` semantics.
- **Responsibility:** design hole.
- **Explanation:** a broad reading of “a parameter may be marked” accepts it; a grammar that has separate op and `run` parameter productions may not. It is also unnecessary: the scheduler already supplies `$today` in every normative clock-routine example. This is not a semantic security defect, but it is a needless core compilation uncertainty.

### Authorization, tenancy, and membership findings

#### BJ4-008 — Cheap's organization authority is still detached from incident authority

- **Severity:** fatal
- **Tier(s):** cheap
- **Attempted citation:** C4 8–21 gives organizations and incidents independent ACLs connected only by unowned `organization -> organizations`; C4 53–58 lets the caller supply the incident's admin/responder groups; C4 124–126 explains the inference.
- **Violated spec citation:** `docs/definition-language.md` 659–697 makes `owned` the only composition/inheritance edge; 994–999 rebases a nearest ancestor guard only down an owned path; 1259–1313 auto-grants a member creator the new ACL root's highest level. `docs/modeling-guidance.md` 692–719 teaches container ACLs over owned subtrees and says separate roots do not compose.
- **Responsibility:** mixed — author model failure plus onboarding-doc fault (the missing organization-ACL-plus-guest recipe).
- **Explanation:** an organization access row confers no incident access. “All admins/responders” works only if every create caller supplies the right two group ids forever; no check ties those groups to the organization, and nothing prevents arbitrary same-space groups. More seriously, every space member who creates an incident receives the incident ACL's top `admin` row automatically, so a responder who may create can administer that incident's grants despite the role split. Create is self-root guard-exempt, so row security cannot repair the choice. This is plausible, successful, wrong authority—not merely missing UI. Strong fixes the model by owning incidents under the organization and guarding them by organization membership rather than giving incidents independent ACL roots.

#### BJ4-009 — Explicit group-backed guard walks do not expand the groups either tier may name

- **Severity:** critical for cheap; major latent defect for strong
- **Tier(s):** both
- **Attempted citation:** cheap grants `$adminGroup`/`$responderGroup` at C4 53–58, then private-note guards test only `member == $caller` at C4 45–46. Strong's organization access rows accept any `$member: id` at S4 259–270 (a group is a `members` row), while every explicit organization guard at S4 26–27, 42–46, 56–60, 74–78, 87–88, and 99–105 likewise tests only direct equality.
- **Violated spec citation:** `docs/definition-language.md` 851–890 says `== $caller` never expands groups and gives the required explicit `member.groupMembers.group(member == $caller)` disjunct; 1233–1257 says only ACL-derived guards add group expansion automatically. `docs/traps.md` 558–574 repeats the equality rule.
- **Responsibility:** mixed — author error plus contradictory onboarding wording.
- **Explanation:** cheap's incident root uses ACL-derived guards, so its group rows open ordinary incident/timeline/task access. The note child replaces that guard with a hand-written direct-row test; members who hold incident access only through the responder/admin group therefore see no private notes and cannot add them. Strong works for direct-person organization rows, but `addOrganizationResponder/Admin` can legally receive a group id and then all explicit organization/incident/note guards silently deny the group's humans. The normative fix is the explicit group-membership disjunct. Onboarding makes this easier to miss: `docs/tutorial.md` 1272–1277 and `docs/glossary.md` 181–183 say “a guard walk”/“guards” resolve groups, while the normative section says the equality spelling does not and requires an explicit hop.

#### BJ4-010 — Cheap does not enforce “one guest, one incident” or provide a guest-safe exposure contract

- **Severity:** critical
- **Tier(s):** cheap (exact-one); both (exposure plumbing)
- **Attempted citation:** C4 127 says a guest “can be granted” one `incidentsAccess` row, but C4 contains no uniqueness-bearing guest link or grant op; C4 106–119 has only a common app including the responder-note table. S4 91–105 and 276–280 model exact-one correctly, but S4 341–381 leaves exposure in non-compilable YAML.
- **Violated spec citation:** the brief requires exactly one shared incident and no private notes. `docs/definition-language.md` 2682–2715 requires guest ops to be guard-bearing and describes invitation/accept/binding, but supplies no package exposure grammar; `docs/std-library.md` 267–340 has introspection/group ops but no complete invite/accept/expose inventory. `docs/ui-gen.md` 303–321 describes guest apps but not authorization registration.
- **Responsibility:** mixed — cheap author omission plus full-spec design hole.
- **Explanation:** cheap's generic ACL permits the same guest to receive `read` on any number of incidents. The note row guard should filter the guest, so the common note table is not itself a row leak, but the artifact never defines which guest-callable ops exist. Strong's dedicated `incidentShares` link, `unique (guest)`, null-role check, admin write guard, and guest app are a sound at-most-one-active-window model. Yet its YAML is integrator intent, not compiler-owned authorization; an end-to-end invitation and exposure still cannot be emitted from the corpus.

#### BJ4-011 — Cheap leaves incident and task assignees unconstrained to the organization

- **Severity:** critical
- **Tier(s):** cheap
- **Attempted citation:** C4 19 and 36 use `-> members?`; C4 78–89 accepts arbitrary ids in create/edit operations.
- **Violated spec citation:** `docs/definition-language.md` 786–849 says `members` includes group rows and null-role guests; 449–480 permits outgoing-ref `check` walks. `docs/modeling-guidance.md` 602–619 gives assignee/customer organization equality as the canonical use. Finding Z-019 records the round-3 hole as closed.
- **Responsibility:** mixed — author omission plus onboarding-doc fault.
- **Explanation:** any same-space member id—including a guest or, under cheap's multi-org-per-space reading, another organization's person—can be assigned. A generated picker is not an invariant. Strong exercises the round-4 fix: assignee refs point to the real `organizationsAccess` entity and checks equate its organization to the incident's. The strong builder's concern that such a ref is categorically unsupported is mostly a false positive: `docs/definition-language.md` 1200–1205 calls the derived link a real entity “queryable, writable, extendable like any entity,” and the schema graph treats it as an ordinary entity. A direct conformance example would still improve confidence/UI behavior.

#### BJ4-012 — Cheap's timeline and private-note authorship remains caller-spoofable

- **Severity:** critical
- **Tier(s):** cheap
- **Attempted citation:** C4 75–76 and 91–92 accept `$by: id` and write it to `by -> members`.
- **Violated spec citation:** `docs/definition-language.md` 385–397 binds `$caller` from the session and 983–986 permits it in write RHS; `docs/guards-conformance.md` 224–227 and GC070 make session attribution the explicit pattern.
- **Responsibility:** mixed — author error plus onboarding-doc fault.
- **Explanation:** replacing round 3's free-text byline with a member ref improves referential quality but not integrity. Any legitimate responder can attribute a timeline entry or sealed note to any member id they know. Strong correctly writes `author: $caller` everywhere. The cheap allowed corpus explains `$caller` visibility but still lacks a direct trusted-author write example, so the onboarding fault from round 3 remains.

#### BJ4-013 — Cheap's “bootstrapOrganization” is not the requested atomic tenant bootstrap

- **Severity:** critical
- **Tier(s):** cheap
- **Attempted citation:** C4 49–51 creates one domain organization row and co-admin access row; C4 124 acknowledges uncertainty about space-per-organization.
- **Violated spec citation:** `docs/definition-language.md` 2508–2538 defines domain bootstrap rows and 2736–2786 makes `std.space.create` one operation/one genesis group containing the space, owner member, domains, profile, bootstrap root, ACL grant, and clients. `docs/bootstrap.md` 42–142 owns the complete flow.
- **Responsibility:** onboarding-doc fault with an incomplete author fallback.
- **Explanation:** C4's op is atomic only within an already-created space and does not create the tenant boundary, owner membership, profile, domain enablement, custody/jurisdiction choice, or client binding. The current cheap allowed set (README/tutorial/glossary/traps) contains no discoverable `std.space.create` or domain `bootstrap` recipe, so this failure is not fairly attributable mainly to the author. Strong uses the new mechanism, subject to BJ4-006 and the bootstrap-document conflicts below.

### Idempotency, lifecycle, realtime, and offline findings

#### BJ4-014 — Cheap still calls changed-payload convergence “idempotent incident creation”

- **Severity:** critical
- **Tier(s):** cheap
- **Attempted citation:** C4 15 and 53–58; C4 131 explicitly says retrying the same key converges the listed incident fields.
- **Violated spec citation:** `docs/definition-language.md` 2129–2148 adds `rev: null` create-once precisely because unique-key convergence silently updates a changed payload; 2167–2184 separates transport retries from semantic ingestion. `docs/api-engine-design.md` 397–402 and 411–443 separately pin permanent creating-op key fingerprints and exact replay. Finding Z-018 is closed by this rule.
- **Responsibility:** mixed — author choice plus onboarding-doc fault.
- **Explanation:** if BJ4-001 were repaired by dropping the binding, a reused `clientKey` could silently change organization, title, severity, and status without CAS. With multiple organization rows in a space, the globally space-scoped single-field key also couples tenants and can converge the `organization` ref itself. The round-4 full spec has a precise fix, and strong uses it: unique request key + `rev: null` + transport `Idempotency-Key`. The cheap allowed docs never mention `rev: null`/`expected_absent`, so Z-018 is closed in the full spec but not in onboarding.

#### BJ4-015 — Cheap's overdue flag becomes stale and the routine repeatedly touches already-overdue rows

- **Severity:** major
- **Tier(s):** cheap
- **Attempted citation:** C4 86–89 edits due/done without resetting `overdue`; C4 101–103 selects every past-due unfinished task, including rows already `overdue == true`; C4 135 acknowledges no clearing lifecycle.
- **Violated spec/brief citation:** the brief asks the nightly routine to mark overdue tasks, implying the stored flag remains a truthful projection of due/completion state. `docs/definition-language.md` 2160–2165 makes each routine proposal an atomic command group and 2169–2171 makes `rev` observable conflict state.
- **Responsibility:** author error.
- **Explanation:** completing a task or moving its due date forward can leave `overdue: true`; each later night can touch already-true rows again, adding command-log noise and potentially bumping revisions that cause unrelated offline edits to receive `cas_miss`. Strong filters `overdue == false` and clears the flag on edit/complete/reopen, a material round-4 improvement.

#### BJ4-016 — Cheap's incident header is not realtime, while strong's realtime spelling remains illustrative

- **Severity:** major
- **Tier(s):** cheap for behavior; both for grammar
- **Attempted citation:** C4 110–115 leaves `detail incidentDetail(...)` without `live` while only child tables subscribe; C4 133 calls that a realtime detail stream. S4 295–310 marks detail and child surfaces live.
- **Violated spec/brief citation:** the brief asks for a realtime incident-detail stream. `docs/api-engine-design.md` 18–20 and 229–249 makes live a subscription; `docs/ui-gen.md` 70–102 supplies page-level coordinated invalidation/refetch but leaves manifest spelling open at 138–150 and 361–374.
- **Responsibility:** mixed — cheap author under-fulfillment plus full-spec UI fault.
- **Explanation:** incident title/severity/status/assignee changes do not invalidate/refetch cheap's root detail surface, so the main incident state can remain stale while its tables update. Strong's four coordinated live reads are semantically appropriate and use the documented page-level snapshot coordinator, but `detail ... live` is still not ratified grammar.

#### BJ4-017 — Managed custody has no specified durable offline mutation queue

- **Severity:** major
- **Tier(s):** both
- **Attempted citation:** C4 81–89 and 134 rely on later CAS submission; S4 169–218 uses CAS throughout and S4 385 discloses the managed-offline assumption.
- **Violated spec/brief citation:** the brief assumes two offline devices edit one incident before reconnecting. `docs/implementation.md` 634–712 and 713–803 fully specifies offline proposals, server arrival order, deterministic CAS rejection, and user-visible resolution only for client-first custody; 704–708 simultaneously removes server routines and server-side value execution there. The brief also requires a guest window and nightly server routine, which need managed custody.
- **Responsibility:** design hole.
- **Explanation:** CAS correctly defines what happens if both commands eventually reach the managed server: first accepted revision wins, second gets `cas_miss`, with no silent merge. The corpus does not define how a managed-custody client durably records, retries, orders, and associates offline writes with the revision it read. Choosing client-first would gain that protocol but lose server-enforced guest isolation and the nightly routine. Strong states the incompatibility accurately; cheap supplies only the CAS endpoint. The conflict semantics are covered, but the requested offline delivery behavior is not a complete platform contract.

## Fulfillment matrix

| Brief item | Cheap round 4 | Strong round 4 |
|---|---|---|
| Multi-tenant organizations | **Fail/unsafe.** Organization rows share a possible space and do not govern incident ACLs. | **Pass by stated model.** One organization per space; organization is the bootstrapped ACL root. |
| Organization members/admins/responders | **Fail.** Runtime groups are assumed and caller-supplied; responder/admin relationship to the organization is not emitted or enforced. | **Mostly pass.** `organizationsAccess` holds direct organization roles; admin writes are top-level guarded. Group-backed rows need explicit expansion. |
| Severity/status/incident assignee | **Partial.** Fields/CAS exist; values and assignee eligibility are unconstrained. | **Semantic pass, compile caveat.** Finite values and organization-access assignee check are right; enum syntax is unsettled. |
| Timeline entries | **Partial/fail integrity.** Owned and member-ref byline, but client-spoofable and day-granularity display. | **Pass.** Owned, datetime-stamped, `$caller`-attributed, live read. |
| Tasks | **Partial.** Owned/CAS/basic fields work; nullable field controls cannot be cleared through these ops and overdue can go stale. | **Pass for brief.** Owned, checked assignee, CAS lifecycle, overdue clearing. |
| Sealed private responder notes | **Partial.** Sealed and guest-excluding in shape, but group-backed responders are silently excluded and author is spoofable. | **Pass for direct principals.** Sealed, explicit organization-only replacement guard, `$caller` author; group caveat remains. |
| Admins manage membership/all incidents | **Fail.** Per-incident caller-supplied ACL grants are convention, not organization authority; creators receive incident admin. | **Mostly pass.** Admin guards protect organization access/share writes; invitation/suspension/platform membership inventory remains absent. |
| Responders read/write org incidents | **Conditional/fail.** Only if every incident received the correct responder group; note access still fails through that group. | **Pass for direct ACL holders.** Explicit organization guard applies to incident and children. |
| Guest reads exactly one, never notes | **Fail exact-one; note rows likely filter.** Generic ACL can grant many incidents and no guest exposure surface exists. | **Pass in row model.** Null-role checked unique guest link, safe guest app, private-note guard; invitation/exposure remains external intent. |
| Atomic organization/bootstrap | **Fail whole-tenant requirement.** Only a domain row and co-admin grant in an existing space. | **Pass at initial managed-genesis design level**, subject to client/app handoff and contradictory bootstrap status/route prose. |
| Idempotent incident creation | **Fail and compile-blocked.** Unique-key convergence changes payload; `let` cannot bind it. | **Pass.** `rev: null` create-once plus permanent/fingerprinted transport replay. |
| Stable paginated incident list | **Pass stability, fail registration.** `sort id` is stable; missing `unbounded`. | **Pass.** `sort -id`, `unbounded`, keyset envelope. |
| Realtime incident detail | **Partial.** Child tables live; root detail stale. | **Semantic pass, grammar open.** Coordinated detail/timeline/task/note invalidation-refetch. |
| Nightly overdue routine | **Partial/fail registration.** Correct basic predicate; missing `unbounded`, no stale-flag repair. | **Pass**, with the minor `filled`-on-run grammar question. |
| Generated list/detail/create UI | **Fail.** Invented form, common private surface, unresolved page/op collision. | **Directional pass only.** Plausible two-app manifest and form, but blocks/form/detail/live/profile are non-normative. |
| Two offline devices edit one incident | **Partial.** Incident/task CAS gives conflict, but create bypasses CAS and managed offline delivery is unspecified. | **Partial/pass for conflict semantics.** All incident mutations CAS; managed durable queue remains unspecified. |

## What both tiers got right

- Both obey the current lowerCamel identifier and line-continuation rules; strong fixed all round-3 detached projections and dangling-`or` formatting.
- Both use implicit `id`/`rev` and CAS for ordinary incident/task same-row edits rather than last-write-wins.
- Both model timeline, tasks, and notes as owned incident children and state both access verbs on the note override; Y-100 is not regressed.
- Both correctly keep `today`/`now` in shape defaults and pass `$today` into the routine.
- Both use `members.displayName`, fixing the round-3 normative/onboarding mismatch.
- Both sort the incident list on immutable implicit `id`, so the requested keyset cursor is genuinely stable by construction.
- Both treat paging and liveness as call/runtime behavior rather than inventing OFFSET or pushed row data.
- Both use a UTC `Z` clock trigger and query → `each -> set` routine shape.
- Both understand that sealed storage and row authorization solve different problems, and both give notes a stricter replacement guard than ordinary incident children.
- Both avoid inventing nullable operation-parameter syntax. Strong preserves full behavior with split assign/clear operations; cheap pays for avoidance by making optional fields impossible to clear through its authored ops.

## Builder-note false positives resolved by the full spec

### Cheap-tier false positives

- **Derived ACL names are mechanical.** C4 128–129 need not guess: `docs/definition-language.md` 1206–1216 fixes `incidents` → `incidentsAccess` and its root ref `incident`; `incident.incidentsAccess(...)` is a valid reverse walk.
- **ACL roots may have ordinary association refs.** C4 128's uncertainty is not a compile issue. With no `owned`, the ref is an association (`docs/definition-language.md` 659–697). The consequence is semantic non-composition, BJ4-008.
- **Custom levels and literal disjunctions are valid.** C4 129's `read/write/admin` ordering and `level == "write" or level == "admin"` are inside the closed guard grammar (`docs/definition-language.md` 950–970, 1229–1240).
- **Owned timeline/tasks really do inherit the incident ACL.** C4 127 is resolved by G6 (`docs/definition-language.md` 994–999).
- **`datetime = now` exists in the full spec.** C4 136 could not find it in the allowed onboarding set, but `docs/definition-language.md` 134–147 and 403–428 make it settled. This is a real onboarding omission, not a language hole.

### Strong-tier false positives or over-cautions

- **Explicit-guard propagation to grandchildren is settled.** S4 429 says it is unclear whether an intermediate explicit guard becomes the inherited rule. G6 says a child takes its **nearest guard-bearing ancestor**, identically for hand-written and ACL-derived guards (`docs/definition-language.md` 994–999). Restating guards is legal defensive duplication but not required for propagation.
- **Uniqueness is space-scoped.** S4 477's risk that `unique (guest)` might become deployment-wide is resolved by `docs/definition-language.md` 503–518. Y-023 is closed.
- **Create-once is no longer an open semantic hole.** S4's risk list correctly asks implementations not to miscompile it, but the written meaning is settled: `rev: null` means CREATE or `expected_absent` (`docs/definition-language.md` 2129–2148). Z-018 is closed and the strong definition uses the fix.
- **The assignee target is more supported than S4 claims.** The derived ACL link is a real entity “like any entity” (`docs/definition-language.md` 1200–1205), and refs target entities in the schema graph. A direct authored-ref conformance case/UI picker rule is still desirable, but the full spec does not support treating the ref as categorically illegal.
- **The clock routine authority model is settled.** Guards pass for clock wakes and listener wakes are `$trigger.ids`-bounded (`docs/definition-language.md` 2394–2413; `docs/guards-conformance.md` 248–257 and GC096–GC098). The attempt does not hit the old Y-004 path.

## Direct round-4 versus round-3 comparison

| Area | Round-3 result | Round-4 result |
|---|---|---|
| Strong lexical formatting | Dangling guard `or`, detached projections, invalid `unique guest`. | **Fixed.** Open delimiters preserve lines; projections stay attached; `unique (guest)` is settled syntax. |
| Member display | Cheap was taught nonexistent `.name`; strong degraded to ids. | **Fixed in docs and both attempts** with `displayName` (Y-026 closed). |
| ACL creator bootstrap | G10/pre-insert ordering contradicted auto-grant. | **Fixed at design/conformance level** by G10a on the ACL root; neither builder needs `unguarded` (Y-003/Y-102 closed). |
| Whole-tenant bootstrap | No one-call space/owner/domain/root/profile surface. | **Strong fixed** via domain `bootstrap` + `std.space.create`; cheap onboarding does not expose it (Z-020 closed in full corpus, not learned by cheap). |
| Create idempotency | Both silently converged changed payloads. | **Strong fixed** with `rev: null`; cheap retains convergence and now also misuses `let` (Z-018 closed in full spec, onboarding gap remains). |
| Assignee eligibility | Inexpressible against plain `members`. | **Strong fixed** with outgoing ref checks through `organizationsAccess`; cheap retains arbitrary members (Z-019 closed). |
| Stable pagination | Cheap sorted a supposedly immutable authored timestamp defeated by std analysis; strong used id. | **Both use id.** Current authored/std stability ruling makes this safe (Y-017 closed). Cheap still misses `unbounded`. |
| Member/group semantics | Guard/body equality had inconsistent group behavior in older docs. | **Normatively unified** as exact row equality, but both round-4 attempts miss explicit group expansion on hand-written guards; onboarding wording is stale (Y-014 closed normatively, not pedagogically). |
| Routine authority | Rationale and listener confused-deputy bound conflicted. | **Fixed** by trigger provenance and conformance cases (Y-004/Y-029 closed). Both attempts are clock-woken. |
| Overdue lifecycle | Cheap wrote status broadly; strong had separate flag. | **Strong improved further** by filtering false and clearing on edits. Cheap uses a flag but leaves it stale/re-touched. |
| UI/app/profile | Form/detail/live/profile grammar open. | **Still open.** Binding grammar improved, but both now invent `form`; strong correctly labels external YAML as non-compilable (X-078 partial, Z-022 open). |
| Nullable parameters | Cheap invented `$x: type?`; strong split operations. | **Both avoid the syntax**, but Z-021 remains open and cheap loses clear/null interactions. |
| Offline two-device story | Client-first sequencing/CAS unclear; managed queue absent. | **Client-first fold/CAS is now settled**, but it is incompatible with this brief's guest/server-routine requirements; managed mutation queue remains a distinct hole. |

### Did cheap-tier silent-wrong risk decline?

Only modestly, and not in the brief's security core.

It did decline in three areas: `displayName` now compiles, nullable op-param invention is gone, and id-sorted pagination is truly stable. Timeline attribution moved from arbitrary text to a real member ref. These are meaningful local improvements.

But the dominant silent failures remain: organization authority is not connected to incidents; responders may become per-incident admins through creator auto-grant; caller-supplied groups decide authority; a guest can be granted multiple incidents; incident/task assignees may be guests/wrong-org members; timeline/note bylines remain spoofable; and unique-key creation still silently changes an existing incident. The new group/equality rule additionally makes responder-note access silently empty for users granted through the very groups the definition creates. The `let` and `unbounded` failures make the artifact stop loudly as written, which prevents deployment but does not demonstrate lower silent-wrong risk in the model an author would obtain after obvious compile repairs.

## Prior-fix and finding-status verification

| Finding / area | Round-4 judgment |
|---|---|
| **X-001 — guarded execution absent** | **Open.** README and `review/findings.json` still say `acl`/`access` are not parsed/bound/executed. This is why neither artifact runs today. |
| **X-005 / Y-003 / Y-102 — ACL bootstrap ordering** | **Closed at design and conformance level.** G10a is on the ACL root, not the derived link; `guards-conformance.md` GC091–GC094 covers exemption, same-group auto-grant, guest non-grant, and dependent writes. |
| **Y-100 — both access verbs** | **Closed and effective.** Both attempts provide read and write on every explicit guard. |
| **Y-101 — routine flag slot** | **Closed.** Strong uses it; cheap's problem is missing `unbounded`, not inability to spell it. |
| **X-074 / Y-103 — `let` discoverability** | **Closed, but cheap applies it incorrectly.** Current tutorial/traps say only CREATE binds. |
| **Y-104 / Z-018 — transport replay and create-once** | **Closed in full spec.** Strong uses permanent/fingerprinted replay plus `rev: null`; cheap onboarding omits the semantic create-once half. |
| **Z-019 — assignee eligibility** | **Closed/relocated to a workable pattern.** Ref-walking checks plus a ref to the real ACL-derived membership entity express the invariant; strong uses it. A direct conformance/UI example remains desirable. |
| **Z-020 — whole-tenant atomic bootstrap** | **Closed for initial managed genesis at design level, with stale status prose.** Strong uses the one-op flow. `bootstrap.md` 666–671 still labels the domain bootstrap block `[proposed]` and leaves enable-on-existing-space compiler questions unresolved. |
| **Y-017 — stable cursor versus std writes** | **Closed by the authored-write ruling and disclosure.** Both id sorts are stable even under the strongest interpretation because id cannot move. |
| **Y-023 — uniqueness space scope** | **Closed.** All unique indexes lead with space; strong's `unique (guest)` is per space. |
| **X-079 / Y-025 — `filled`** | **Closed for op parameters, stale prose remains.** Definition language ratifies it; `ui-gen.md` 201–218 still says unratified before 264–269 corrects itself. Routine applicability remains BJ4-007. |
| **Y-026 — member display** | **Closed and effective.** Both use `displayName`. |
| **Y-004 / Y-029 — routine authority/rationale** | **Closed at design/conformance level.** Trigger provenance now owns the row bound. |
| **Y-013 / Y-014 — group combination/equality** | **Closed normatively, stale onboarding remains.** Definition language chooses maximum effective ACL level and exact equality; `glossary.md` 191–194 still calls direct/group overlap unruled, while broad “guards expand groups” wording obscures the required explicit hop. |
| **Y-002 / Y-015 / Y-106 — space-bound door/executor/capability path** | **Closed in the current finding ledger at design level.** That does not close X-001's missing actual executor. |
| **Y-031 — client-first sequencing/CAS** | **Closed as a client-first protocol.** Server arrival order and deterministic device folding are specified. It does not answer managed-custody offline mutation queuing (BJ4-017). |
| **X-078 / Z-022 — app/form/detail/live grammar** | **Partial/open.** Bindings are normative; complete app grammar is not. Both attempts hit it. |
| **Z-021 — nullable op parameters** | **Open.** Both avoid the syntax; the generated-form interaction remains distorted. |
| **Z-023 — invalid display/unbounded onboarding** | **Partial.** Display is fixed. The tutorial's “plantcare needs none” examples still contradict the settled unbounded-cardinality gate and cheap repeats the failure. |

## Unresolved document conflicts relevant to this build

These are recorded without choosing a winner:

1. **Bootstrap route:** `docs/api-engine-design.md` 11–21 says `/op/{name}`; `docs/bootstrap.md` 458–466 says `/ops/std.space.create`. Strong follows the latter.
2. **Custody/jurisdiction defaults:** `docs/definition-language.md` 2776–2780 and `docs/bootstrap.md` 427–440 say neither may default; `docs/definition-language.md` 2803–2808 still illustrates `defaults custody: managed, jurisdiction: US`. Strong supplies both, so its result is robust to the conflict.
3. **Domain bootstrap status:** `docs/definition-language.md` 2517–2538 marks the bootstrap row mechanism settled; `docs/bootstrap.md` 666–671 calls the domain `bootstrap` block proposed and leaves compiler behavior unresolved.
4. **Deployment-scoped wire status:** `docs/api-engine-design.md` 65–71 already defines the `scope deployment` exemption; `docs/bootstrap.md` 704–706 still says the API document lacks it.
5. **`filled` status:** `docs/ui-gen.md` 201–218 says the spelling is unratified and absent from the language; the same document at 264–269 and `docs/definition-language.md` 366–383 say it is settled.
6. **Group-aware guard wording:** `docs/definition-language.md` 868–890 requires an explicit membership disjunct because `member == $caller` never expands groups; tutorial/glossary/modeling prose often says “guards expand groups” without preserving that syntactic distinction. Both attempts demonstrate the ambiguity.
7. **Enum inventory:** `docs/definition-language.md` 134–138 calls the field/parameter type inventory closed without enum; 348–353 and `ui-gen.md` treat enum as a lean field/UI direction. Strong depends on both field and parameter enum forms.
8. **Cursor wording:** `docs/ui-gen.md` 42–45 says total order means cursors are always stable; `docs/definition-language.md` 1709–1745 correctly makes stability conditional on sort-key writability. Both attempts happen to use id, so this conflict does not change their result.
9. **Group conflict status:** `docs/glossary.md` 191–194 says direct/group level conflicts are not ruled, but `docs/definition-language.md` 1233–1250 settles maximum effective level and surviving-path revoke reporting; finding Y-013 is closed.

## Ranked documentation actions

1. **Finalize one compile-owned app/profile/guest-exposure grammar.** Specify `app/page/table/detail/form/live`, page/op namespaces, UI annotations, package profile templates, guest op exposure, and invite/accept/revoke operations in one end-to-end buildable example. This is now the largest strong-tier blocker and the highest-risk integrator seam.
2. **Move create-once into the cheap onboarding path.** Put `unique key + rev: null`, `expected_absent`, transport `Idempotency-Key`, and “only CREATE may bind with `let`” in one tutorial example. The full-spec fix exists but the cheap tier reproduced both the old silent convergence and a new loud binding error.
3. **Publish the exact organization-ACL + per-incident guest-window recipe.** Include direct and group-backed organization members, explicit group-disjunct guards, private-child replacement, exact-one guest link, assignee-via-access-row check, and the door/profile split. This single recipe would have prevented most cheap-tier security findings and the strong latent group failure.
4. **Rule `acl` plus explicit guards on the same root.** Either define root replacement/coexistence precisely or reject it and show the canonical ACL vocabulary/hand-rolled membership pattern that gives responder write plus admin-only membership management.
5. **Make bootstrap one canonical surface across documents.** Reconcile `/op` versus `/ops`, `apps[]` versus `clients[]`, domain-bootstrap status, managed/client-first genesis status, and the custody/jurisdiction default contradiction. Include required client IaC beside the call so app names are not mistaken for OIDC clients.
6. **Either settle enum completely or teach the settled fallback.** Define field and parameter enum grammar/migration/UI behavior, or make `text + check` plus `choices` the canonical buildable pattern. The current half-status caused a new strong-tier compile regression.
7. **Specify managed-custody offline mutations.** Define durable queue ownership, stored op version/rev/idempotency key, replay ordering, conflict presentation, and schema-skew behavior. The client-first protocol is detailed but structurally incompatible with this common guest-plus-server-routine brief.
8. **Close nullable/defaulted operation parameters.** Generated create/edit forms need a settled way to send null and clear refs/dates without multiplying operations. Keep Z-021 visible in §13 open items until closed.
9. **Repair all unbounded onboarding examples.** Every registered whole-space list/reduce/routine should either prove a bound or visibly carry `unbounded`; remove “plantcare needs none” where the settled cardinality rule says otherwise.
10. **Add conformance cases for authored refs to ACL-derived entities and their generated UI.** The language strongly implies legality and the assignee solution depends on it; a single compile case and picker descriptor would turn a plausible inference into a dependable recipe.
11. **Keep the actual-implementation stop adjacent to every multi-user example.** X-001 remains open. Until guard parsing/binding/execution and the behavioral corpus run, no language-correct security model should be presented as deployable behavior.

## Final judgment

The full round-4 corpus materially improves AI buildability. It now provides enough settled backend semantics for the strong tier to solve the failures that dominated round 3: atomic tenant genesis, ACL create exemption, exact create-once behavior, organization-qualified assignees, stable cursor analysis, display identity, and routine provenance. The strong attempt is recognizably Ordo-shaped and, after replacing enums and settling the root-guard choice, its data/operation/routine model is close to a defensible written-spec backend.

It is not a complete compilable application. UI composition, form annotations, profile templates, guest invitation/exposure, identity-client binding, nullable controls, and managed-offline delivery remain prose seams. The strong artifact has to preserve security intent in YAML beside the definition, which is evidence that the platform's most important remaining buildability failure has moved from row semantics to packaging and product surface.

The cheap tier remains unusable. Its confidence rose from round 3, but its central model still grants authority incident-by-incident under caller control rather than deriving it from the organization, and it retains exact-one, attribution, assignment, and semantic-create failures. Several new full-spec fixes are simply absent from the allowed onboarding set. Round 4 therefore demonstrates a wider knowledge-tier gap and a better full spec, but not a meaningful decline in cheap-tier silent-wrong security risk.
