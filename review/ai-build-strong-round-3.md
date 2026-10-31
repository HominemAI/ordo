# Strong-tier AI build attempt — round 3

## BUILD BRIEF

> Define an Ordo application for a multi-tenant incident-response service. Each organization has members; incidents have severity/status, an assignee, timeline entries, tasks, and sealed private responder notes. Organization admins can manage membership and all incidents; responders can read/write incidents for their organization; invited guests can read exactly one shared incident but never private notes. Include an atomic organization/bootstrap flow, idempotent incident creation, a stable paginated incident list, a realtime incident-detail stream, a nightly routine that marks overdue tasks, and enough app/surface metadata for generated list/detail/create UI. Assume two offline devices may edit the same incident before reconnecting. Emit the complete definition you believe a developer should write, plus any necessary manifest/UI declarations.

## Attempted definition (verbatim)

The intended tenancy boundary is **one Ordo space per organization**. The domain-level `organizations` row exists to anchor the responder/admin ACL and the per-incident guest guard; the platform space remains the custody, membership, command-log, export, and deletion boundary.

```ordo
domain incidentResponse @ 0.1.0

entity organizations {
  name: text
  acl (responder, admin)
}

entity incidents {
  organization -> organizations   owned
  requestKey:  text
  title:       text
  summary:     text = ""
  severity:    text
  status:      text = "open"
  assignee  -> members?
  created:     datetime = now

  unique (organization, requestKey)
  index (organization, status)
  check severity in ("critical", "high", "medium", "low")
  check status in ("open", "mitigating", "monitoring", "resolved")

  access read   organization.organizationsAccess(member == $caller) or
                incidentGuests(guest == $caller and guest.kind == "principal")
  access write  organization.organizationsAccess(member == $caller)
}

entity incidentTimeline {
  incident -> incidents   owned
  author   -> members
  at:         datetime = now
  body:       text
}

entity incidentTasks {
  incident -> incidents   owned
  assignee -> members?
  title:      text
  due:        date?
  status:     text = "open"
  overdue:    bool = false
  created:    datetime = now

  index (incident, status)
  check status in ("open", "inProgress", "done", "canceled")
}

entity responderNotes {
  incident -> incidents   owned
  author   -> members
  at:         datetime = now
  body:       text        sealed

  access read   incident.organization.organizationsAccess(member == $caller)
  access write  incident.organization.organizationsAccess(member == $caller)
}

entity incidentGuests {
  incident -> incidents   owned
  guest    -> members     owned
  shared:     datetime = now

  unique guest

  access read   guest == $caller
  access write  incident.organization.organizationsAccess(
                  member == $caller and level == "admin")
}

# The platform's acl bootstrap is expected to mint the creator's admin row in
# this same command group. `unguarded` is deliberately present because the
# current findings show the derived create guard otherwise deadlocks before
# that row exists. Door exposure must be space-owner-only.
op bootstrapOrganization($name: text) unguarded =
  set organizations { name: $name }

op organizationList unbounded =
  organizations[name]
  sort -id

op organizationMembers($organization: id) unbounded =
  organizationsAccess(organization == $organization)[member, level]
  sort member

op incidentList($organization: id) unbounded =
  incidents(organization == $organization)
    [title, severity, status, assignee, created]
  sort -id

op incidentDetail($incident: id) =
  incidents(id == $incident)
    [title, summary, severity, status, assignee, created, rev]

# Semantic ingestion idempotency comes from unique (organization, requestKey).
# Transport retries additionally use the SDK's Idempotency-Key envelope.
op createIncident($organization: id, $requestKey: text(128),
                  $title: text(200), $summary: text, $severity: text) =
  set incidents { organization: $organization,
                  requestKey: $requestKey,
                  title: $title,
                  summary: $summary,
                  severity: $severity,
                  status: "open" }

# This is the offline/same-row conflict boundary. Both devices submit the rev
# they read. The first accepted command bumps rev; the second receives cas_miss.
op reviseIncident($incident: id, $rev: int, $title: text(200),
                  $summary: text, $severity: text, $status: text) =
  set incidents { id: $incident, rev: $rev,
                  title: $title, summary: $summary,
                  severity: $severity, status: $status }

op assignIncident($incident: id, $rev: int, $assignee: id) =
  set incidents { id: $incident, rev: $rev, assignee: $assignee }

op clearIncidentAssignee($incident: id, $rev: int) =
  set incidents { id: $incident, rev: $rev, assignee: null }

op incidentTimelineList($incident: id) unbounded =
  incidentTimeline(incident == $incident)[author, at, body]
  sort -id

op addTimelineEntry($incident: id, $body: text) =
  set incidentTimeline { incident: $incident, author: $caller, body: $body }

op incidentTaskList($incident: id) unbounded =
  incidentTasks(incident == $incident)
    [title, assignee, due, status, overdue, created, rev]
  sort -id

op addIncidentTask($incident: id, $title: text(200)) =
  set incidentTasks { incident: $incident, title: $title }

op scheduleIncidentTask($task: id, $rev: int, $due: date) =
  set incidentTasks { id: $task, rev: $rev, due: $due, overdue: false }

op reviseIncidentTask($task: id, $rev: int, $title: text(200),
                      $status: text) =
  set incidentTasks { id: $task, rev: $rev,
                      title: $title, status: $status, overdue: false }

op assignIncidentTask($task: id, $rev: int, $assignee: id) =
  set incidentTasks { id: $task, rev: $rev, assignee: $assignee }

op clearIncidentTaskAssignee($task: id, $rev: int) =
  set incidentTasks { id: $task, rev: $rev, assignee: null }

op responderNoteList($incident: id) unbounded =
  responderNotes(incident == $incident)[author, at, body]
  sort -id

op addResponderNote($incident: id, $body: text) =
  set responderNotes { incident: $incident, author: $caller, body: $body }

# unique guest means one accepted guest principal can point at at most one
# incident in this organization-space. Re-sharing converges that one row and
# atomically moves the window instead of adding a second window.
op shareIncidentWithGuest($incident: id, $guest: id) =
  set incidentGuests { incident: $incident, guest: $guest }

op revokeIncidentGuest($incident: id, $guest: id) =
  del incidentGuests(incident == $incident and guest == $guest)

routine markOverdueTasks in incidentResponse {
  trigger every day 02:00Z
  run($today: date) unbounded:
    incidentTasks(due is not null and due < $today and
                  status != "done" and status != "canceled" and
                  overdue == false)[id]
    each ->
      set incidentTasks { id: id, overdue: true }
}

# Internal responder/admin application. Result descriptors provide typed
# columns, typed ids, keyset pagination, actions, and generated forms. The
# createIncident id parameter matches an organization row, so action discovery
# can render its create form from the organization list/detail surfaces.
app incidentConsole {
  page organizations {
    table organizationList
  }

  page organization($id) {
    detail std.organizations.get($id: $id)
    table  organizationMembers($organization: $id)
    table  incidentList($organization: $id) live
  }

  page incident($id) {
    detail incidentDetail($incident: $id) live
    table  incidentTimelineList($incident: $id) live
    table  incidentTaskList($incident: $id) live
    table  responderNoteList($incident: $id) live
  }
}

# Guest application intentionally has no private-note surface and no write
# surface. Runtime share exposure must include only the three read ops below.
app incidentGuest {
  page incident($id) {
    detail incidentDetail($incident: $id) live
    table  incidentTimelineList($incident: $id) live
    table  incidentTaskList($incident: $id) live
  }
}
```

## Necessary runtime declarations that are not `.ordo` syntax

The definition alone cannot finish the access model because roles, profiles, invitations, guest exposure, and grants are deliberately runtime state. The space/domain bootstrap must therefore install or require this runtime exposure shape:

| Principal state | Door-level callable surface | Row-level result |
|---|---|---|
| Space owner before organization exists | `bootstrapOrganization` | The unguarded create runs; the creator must receive `organizationsAccess.level = admin` in the same command group. |
| Organization `admin` ACL holder | All incident/timeline/task/note ops; `shareIncidentWithGuest`; `revokeIncidentGuest`; derived `grantOrganizationsAccess`, `revokeOrganizationsAccess`, and level-change ops; platform invitation/suspension ops | Top-level ACL/write guards allow membership and guest-window administration. |
| Organization `responder` ACL holder | Incident/timeline/task/note read and write ops | The organization ACL guard admits all organization incidents; membership-admin guards reject. |
| Accepted invited guest | `incidentDetail`, `incidentTimelineList`, `incidentTaskList` only | `incidentGuests(guest == $caller)` admits the one row selected by the unique guest link. No note op or write op is exposed. |

The platform must create the space owner's `members` row during space creation and the guest's null-role `members` row during invitation acceptance. The domain cannot mint either row. The platform also needs invitation and suspension ops, but the current std inventory does not name them.

The incident list is intentionally sorted by implicit UUIDv7 `id`, not by mutable severity, status, assignee, or a declared timestamp. That is the only ordering in this definition for which the compiler can honestly derive `cursor: stable`; pagination remains a call-envelope concern. The `live` surfaces mean Ordo's specified invalidation stream followed by a scoped refetch, not a delta feed.

For two offline editors, this definition deliberately chooses conflict detection rather than merge: both edit the same `rev`, first reconnecting commit wins, and the other gets typed `cas_miss`, refetches, and presents a user-visible merge/retry. Timeline/task creates on different rows do not conflict. No silent last-writer-wins path is intended or exposed.

# SPEC FRICTION

## Likely compilation and idiomaticity

**Today:** this cannot compile in the repository as implemented. The project is still parser-stage, and README explicitly says guards, `acl`, access binding, apply, generated UI, and the runtime do not exist yet.

**Against the written language rather than the current code:** the entity/guard/op/routine portion is close to the documented idiom, but I would not predict a clean spec compilation without owner rulings. The `app` grammar is explicitly illustrative, `detail` accepting an arbitrary one-row op is still open, and `live` is only demonstrated on tables rather than pinned for detail surfaces. UI annotation names exist but their source spelling does not, so I did not invent `label`/`desc`/`choices` tokens that would almost certainly be parser errors. The generated forms will therefore be typed but plain text for severity/status until the annotation grammar or enum-parameter story is settled.

The strongest idiomatic choices are: one space per organization; an `acl` root for responder/admin membership; owned incident children for guard inheritance; an explicit guard replacement on sealed notes; a one-row-per-guest link for the exact-one window; composite uniqueness for semantic create idempotency; `id` ordering for stable keyset pagination; live invalidation/refetch surfaces; and CAS on every same-row edit.

## Ambiguities, contradictions, and forced choices

1. **Atomic organization/bootstrap is not fully expressible.** Space creation, the owner's platform `members` row, domain enablement, the `organizations` row, and its top-level ACL row cannot be one authored command group. I chose the platform's atomic space-owner creation first, followed by an `unguarded` organization create whose derived creator-admin row is supposed to land in that create's command group. This is only atomic for the domain row + ACL row, not for the whole tenant bootstrap.

2. **The advertised ACL bootstrap fix is reopened by Y-003.** Y-102 says creator bootstrap is closed, but Y-003 says the derived write guard rejects the creating write pre-insert, before the same-group grant is minted. I added `unguarded` to `bootstrapOrganization` to bypass that deadlock and moved safety to the door-level owner-only exposure. That is a forced workaround, and mis-exposing this op would be an author/configuration error with a large blast radius. The underlying ordering defect is a documentation/design fault.

3. **Organization membership is two different things.** Platform `members` admits a principal to a space; `organizationsAccess` says responder/admin for this organization. The brief says admins manage membership, but the domain can only manage ACL rows for already-bound principals. It cannot invite, accept, suspend, or mint a `members` row. I treated those as required platform ops. Their absence from the std inventory is a design/documentation fault, not something an author can repair in this definition.

4. **`acl` level vocabulary is underspecified away from `(read, write, admin)`.** The spec says level names are application vocabulary and ordered left-to-right, so I chose `(responder, admin)`. Incident read/write guards explicitly accept any organization access row, and membership/guest administration explicitly checks `level == "admin"`; this avoids relying on an unstated automatic write threshold. Whether the derived organization entity itself treats `responder` as read-only is still a compiler ruling.

5. **A guest is not type-distinct from a member in domain syntax.** Invitation acceptance produces a `members` row with `role = null`, but the guard grammar has no settled `is null` form for that field and write ops have no cross-row preconditions. I restricted the incident guard to `guest.kind == "principal"` and relied on the platform share flow to supply only accepted null-role guests. An admin can still pass an internal principal id to `shareIncidentWithGuest`; preventing that server-side is currently unexpressible.

6. **Exactly one incident is implemented as a stronger storage invariant.** `unique guest` permits at most one `incidentGuests` row per principal per organization-space. Re-sharing moves the same row to a new incident. This exactly satisfies the brief, but it forbids a future legitimate multi-incident guest. That is my forced product choice, not a language defect.

7. **Uniqueness scope across spaces is open (Y-023).** Both `(organization, requestKey)` and `unique guest` assume uniqueness is scoped inside a space. If generated DDL omits the space key in a shared database, another tenant can collide with either value. That is a platform/design fault that can invalidate semantic idempotency and exact-one sharing.

8. **Idempotent creation can also converge mutable payload.** The composite unique key gives the one blessed creates-or-converges mode. Reusing the same key with changed title/summary/severity will converge those fields rather than reject. The transport fingerprint catches a changed retry only while the replay entry exists and for the same space/principal/op/key. Callers must treat `requestKey` as bound to one immutable request payload. A true immutable create-once key is not expressible without a separate assertion/precondition mechanism.

9. **Stable pagination is conditional and the docs conflict with older claims.** I sorted only on implicit `id`; none of the writes can move it. Y-017 remains open for declared sort fields because derived `std.E.update` makes them writable. This build avoids rather than fixes that problem. Page-token MAC/principal binding is also still open in the security review, so ordering can be semantically stable while the transport cursor remains security-weak.

10. **`unbounded` on pageable lists is a forced interpretation.** The latest language text gates any logically unbounded result even when SQL-pushed and call-time paginated. I marked organization, incident, timeline, task, member, and note lists `unbounded`. If page size itself is intended to prove a bound, these flags are redundant; the docs never reconcile logical result cardinality with per-request page cardinality.

11. **Realtime means invalidation + refetch, not a detail delta stream.** I used live header/timeline/task/note surfaces. A page-level snapshot coordinator can make their refetches atomic at one command-log position. The app-manifest grammar does not yet pin `live` on `detail`, and SH2-12/Y-015 show that the current runtime drops or fails to scope the space dimension in critical places; a multi-tenant live stream is not safe until the invalidation bus is keyed by `(space, op)` and re-checks access after revocation.

12. **Private notes require two independent controls.** `body sealed` protects database custody, while the explicit responder-only read guard replaces the guest-readable inherited incident guard. I intentionally did not add `std sealed`; derived std reads expose only the open skeleton. If `responderNoteList` is exposed to a guest at the door, its row guard should still return no rows. If guard binding is absent, the app silently leaks; that is the fatal implementation gap the corpus itself calls out.

13. **Assignee membership cannot be enforced.** `assignee -> members?` guarantees a space-local member id only if executor/DDL space isolation is correct; it does not prove the assignee holds an organization responder/admin row. The UI should pick from `organizationMembers`, but a crafted request can name another bound member. A cross-row check or op precondition is needed. Leaving this unstated would be an author error; the inability to enforce it is a language design fault.

14. **The nightly routine marks but does not autonomously clear.** I chose a clock-only routine that marks newly overdue open tasks. Scheduling or revising a task explicitly resets `overdue: false`; a later nightly pass reasserts it if still overdue. Exposing generic `std.incidentTasks.update` can violate that lifecycle, so profiles should withhold it. This is an authored lifecycle choice and must be reviewed, not a compiler fault.

15. **Routine row authority remains conceptually inconsistent, but this trigger avoids the worst case.** `markOverdueTasks` is clock-driven, carries `unbounded`, and writes only task rows found by its own narrow manifest. It is not outsider-wakeable through `ingested`/`fed`, so SH2-10/Y-004's confused-deputy route is not present in this app. Y-029 remains open because four documents still describe routine guards differently.

16. **Offline conflict behavior is detection, not collaboration.** CAS gives a loud `cas_miss`; it does not merge two offline edits. That is the documented v1 choice and is appropriate for incident severity/status changes. For client-first custody, Y-031 remains open: arrival-order sequencing and rejection/replay are not coherently specified. This definition is safe only under the interpretation that rejected stale commands never become canonical history and the client emits a fresh command after user resolution.

17. **Generated create UI is inferred rather than explicitly placeable.** Action discovery sees `createIncident`'s organization-id parameter and can render it against an organization row. The app grammar has no settled `form` declaration, and presentation annotations have names but no spelling. If the product requires a dedicated create page, that is presently a UI-spec fault; inventing a `form` keyword would be an author error.

18. **`members` has no display name (Y-026).** `organizationMembers`, assignee columns, timeline authors, and note authors can render typed member ids but not a human name from the platform entity's documented fields. This is visibly non-idiomatic UI and cannot be fixed by a domain ref without creating a second identity/profile table and its own bootstrap/synchronization problem.

19. **Admin/profile coupling is not declarable.** ACL `admin` is row state; door-level profiles are space runtime state. The source file cannot guarantee that an admin can call invitation/grant/share ops or that a guest cannot call bootstrap/note/write ops. I supplied the required exposure table, but there is no final profile-template grammar and runtime managers may loosen it. The guards remain the structural backstop where they are expressible.

20. **Guest exposure and app rendering are separate unsettled layers.** The `incidentGuest` app omits notes and writes. Runtime share registration must expose only its three guarded read ops. Catalog-scoped descriptors should hide everything else, but guest catalog trimming/co-holder disclosure remains open in the security review. The app manifest itself is not an authorization boundary.

21. **Multi-tenant runtime isolation is not established by a correct definition.** Y-002 says capability grants are not space-bound; Y-015 says the executor receives no space after the door binds one; SH2-12 says live invalidation is keyed only by op name. These are platform faults that invalidate any claim that this app is safely multi-tenant today, even if every entity guard is authored correctly.

22. **The current error-contract documents disagree.** The language, API design, guards corpus, and code disagree over `access_guard`, `not_found`, `access_denied`, and `unknown_op`. That does not change this source's intended row sets, but it makes the generated client's conflict/denial UX non-portable until Y-030 is resolved.

## Silent-wrong risks

- A missing or unenforced incident/note guard returns plausible rows and is catastrophic. This is a platform/compiler failure, not a query typo.
- Mis-exposing `bootstrapOrganization` or note/write ops at the door defeats the intended product boundary. Because runtime profiles are outside source, this is partly deployment-author error and partly a missing compile-checked profile-template surface.
- A crafted assignee id can select a non-responder member; the request succeeds plausibly.
- Reusing `requestKey` with a changed payload silently edits the existing incident after replay-cache expiry.
- Exposing `std.incidentTasks.update` can preserve or reintroduce an incorrect `overdue` flag.
- A guest share to a group row, or group-aware `$caller` equality changes, can widen one-person access. The `kind == "principal"` test mitigates the incident read guard but does not type the share op's parameter.
- Cross-space uniqueness, executor-space dropping, capability-space omission, and op-only invalidation channels can make one tenant affect or observe another with no error in this definition.
- In client-first mode, a stale offline proposal could be sequenced before its CAS rejection under the currently contradictory merge prose.

## Author errors versus documentation/design faults

**Author errors in this attempt, if left unreviewed:** exposing the unguarded bootstrap too broadly; granting the guest app any note/write op; enabling generic std updates that bypass authored lifecycle ops; treating request keys as reusable; promising auto-merge instead of typed conflict; assuming a UI picker enforces organization membership; or claiming live transport is delta-push.

**Documentation/design faults:** no atomic space+domain bootstrap surface; ACL create-check ordering (Y-003); no domain path for invitations/suspension; no finalized app/form/annotation grammar; no member display profile; no cross-row assignee invariant; unresolved space-scoped uniqueness; contradictory routine/offline/error semantics; and the open cross-space executor/capability/live-stream faults. These cannot be repaired by a more careful `.ordo` author.

## Prior-fix verification

- **Closed and used:** Y-100 (both access verbs are mandatory; every explicit guard here states both), Y-101 (routine flags are spellable; the nightly run carries `unbounded`), Y-104 (transport replay is scoped by space/principal/op/key plus request fingerprint), and Y-103's findable `let` spelling is not needed by this build.
- **Closed in prose, relocated/open in behavior:** Y-102's creator ACL bootstrap is contradicted by Y-003's pre-insert guard ordering. The `unguarded` bootstrap workaround relocates safety to the door; it does not close the platform rule.
- **Avoided, not fixed:** Y-017 is avoided by sorting on implicit immutable id; Y-025 is avoided by putting no SDK-filled date parameter on an app surface; Y-011's `access true` ambiguity is avoided; group grants are not used.
- **Still open and directly visible:** Y-026 (member display names), Y-029 (routine identity rationale), Y-031 (offline/client-first sequencing), Y-023 (space-scoped uniqueness), Y-002/Y-015 (space isolation), and SH2-12/SH-30 (live cross-tenant invalidation and revocation re-check).

## Bottom line

This is a coherent **spec-level model** for the requested app, and most of its business rules map cleanly onto Ordo's intended idioms. It is not a safe deployable application today. The biggest blockers are not mistakes in the attempted entity/query syntax: they are the unresolved ACL bootstrap ordering, missing platform invitation/profile surfaces, unsettled app/form metadata grammar, inability to enforce organization membership on assignees, and cross-space runtime isolation holes. A developer could reasonably write the definition above; they could not responsibly promise that the current platform compiles or securely runs it.
