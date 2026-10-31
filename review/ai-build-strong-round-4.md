# Ordo round-4 AI buildability review — strong tier

## Scope and method

I treated the build brief as a cold authoring task. I read the current README and the full allowed specification corpus first, then used only `critique-consolidated-3.md` §§1/2/10, `security-hardening-review-3.md`, and `review/findings.json` to check whether earlier holes had actually moved or closed. I did not execute, parse, compile, or test the attempted definition.

The design below chooses one organization per Ordo space. The space is the tenant isolation and command-log boundary; the bootstrapped `organizations` row is the application's tenant root. Organization membership is the ACL-derived `organizationsAccess` entity: its two levels are `respond` and `admin`. Incidents and their ordinary children add a per-incident guest window; private notes replace that inherited guard and retain organization-member-only access.

## Attempted definition — preserved verbatim

The following is the exact definition I would hand to an Ordo developer from the written corpus. I have not silently repaired the open grammar after writing it. In particular, the `form` line and app block composition are preserved because they are the most plausible way to state the requested UI, even though the current UI document does not ratify that grammar.

```ordo
domain incidentResponse @ 1.0.0 {
  shapes    organizations, incidents, incidentTimelineEntries, incidentTasks, privateResponderNotes, incidentShares
  ops       incidentList, incidentDetail, incidentTimeline, incidentTaskList, privateNoteList, organizationMemberList, incidentGuestList, createIncident, editIncident, setIncidentState, assignIncident, clearIncidentAssignee, addTimelineEntry, createIncidentTask, editIncidentTask, completeIncidentTask, reopenIncidentTask, addPrivateResponderNote, addOrganizationResponder, addOrganizationAdmin, removeOrganizationMember, shareIncidentWithGuest, revokeIncidentGuest
  routines  markOverdueTasks
  bootstrap organizations { name, timezone }
}

entity organizations {
  name:     text
  timezone: text
  acl (respond, admin)

  access read  organizationsAccess(member == $caller)
  access write organizationsAccess(member == $caller and level == "admin")
}

entity incidents {
  organization -> organizations owned
  requestKey:    text unique
  title:         text
  description:   text
  severity:      enum("sev1", "sev2", "sev3", "sev4") = "sev3"
  status:        enum("open", "mitigating", "resolved", "closed") = "open"
  assignee      -> organizationsAccess?
  created:       datetime = now

  check assignee is null or assignee.organization == organization

  access read  (
    organization.organizationsAccess(member == $caller) or
    incidentShares(guest == $caller)
  )
  access write organization.organizationsAccess(member == $caller)
}

entity incidentTimelineEntries {
  incident -> incidents owned
  author   -> members
  kind:      enum("created", "details", "state", "assignment", "comment")
  body:      text
  created:   datetime = now

  access read  (
    incident.organization.organizationsAccess(member == $caller) or
    incident.incidentShares(guest == $caller)
  )
  access write incident.organization.organizationsAccess(member == $caller)
}

entity incidentTasks {
  incident -> incidents owned
  title:      text
  status:     enum("open", "done") = "open"
  due:        date
  overdue:    bool = false
  assignee   -> organizationsAccess?
  created:    datetime = now

  check assignee is null or assignee.organization == incident.organization

  access read  (
    incident.organization.organizationsAccess(member == $caller) or
    incident.incidentShares(guest == $caller)
  )
  access write incident.organization.organizationsAccess(member == $caller)
}

entity privateResponderNotes {
  incident -> incidents owned
  author   -> members
  body:      text sealed
  created:   datetime = now

  access read  incident.organization.organizationsAccess(member == $caller)
  access write incident.organization.organizationsAccess(member == $caller)
}

entity incidentShares {
  incident -> incidents owned
  guest    -> members owned
  created:   datetime = now
  unique (incident, guest)
  unique (guest)
  check guest.role is null

  access read  (
    guest == $caller or
    incident.organization.organizationsAccess(
      member == $caller and level == "admin")
  )
  access write incident.organization.organizationsAccess(
                 member == $caller and level == "admin")
}

op incidentList($organization: id) unbounded =
  incidents(organization == $organization)[
    id, title, severity, status, assignee.member.displayName, created
  ]
  sort -id

op incidentDetail($incident: id) =
  incidents(id == $incident)[
    id, organization, title, description, severity, status,
    assignee.member.displayName, created, rev
  ]

op incidentTimeline($incident: id) unbounded =
  incidentTimelineEntries(incident == $incident)[
    id, author.displayName, kind, body, created
  ]
  sort -id

op incidentTaskList($incident: id) unbounded =
  incidentTasks(incident == $incident)[
    id, title, status, due, overdue, assignee.member.displayName, rev
  ]
  sort -id

op privateNoteList($incident: id) unbounded =
  privateResponderNotes(incident == $incident)[
    id, author.displayName, body, created
  ]
  sort -id

op organizationMemberList($organization: id) unbounded =
  organizationsAccess(organization == $organization)[
    id, member, member.displayName, level
  ]
  sort id

op incidentGuestList($incident: id) unbounded =
  incidentShares(incident == $incident)[
    id, guest, guest.displayName, created
  ]
  sort id

op createIncident($organization: id, $requestKey: text, $title: text,
                  $description: text,
                  $severity: enum("sev1", "sev2", "sev3", "sev4")) =
  let incident = set incidents {
    organization: $organization,
    requestKey: $requestKey,
    rev: null,
    title: $title,
    description: $description,
    severity: $severity,
    status: "open"
  }
  set incidentTimelineEntries {
    incident: incident.id,
    author: $caller,
    kind: "created",
    body: "Incident created"
  }

op editIncident($incident: id, $rev: int, $title: text, $description: text) =
  set incidents {
    id: $incident,
    rev: $rev,
    title: $title,
    description: $description
  }
  set incidentTimelineEntries {
    incident: $incident,
    author: $caller,
    kind: "details",
    body: "Incident details edited"
  }

op setIncidentState(
  $incident: id,
  $rev: int,
  $severity: enum("sev1", "sev2", "sev3", "sev4"),
  $status: enum("open", "mitigating", "resolved", "closed")
) =
  set incidents {
    id: $incident,
    rev: $rev,
    severity: $severity,
    status: $status
  }
  set incidentTimelineEntries {
    incident: $incident,
    author: $caller,
    kind: "state",
    body: "Incident state changed to " + $severity + "/" + $status
  }

op assignIncident($incident: id, $rev: int, $assignee: id) =
  set incidents { id: $incident, rev: $rev, assignee: $assignee }
  set incidentTimelineEntries {
    incident: $incident,
    author: $caller,
    kind: "assignment",
    body: "Incident assignee changed"
  }

op clearIncidentAssignee($incident: id, $rev: int) =
  set incidents { id: $incident, rev: $rev, assignee: null }
  set incidentTimelineEntries {
    incident: $incident,
    author: $caller,
    kind: "assignment",
    body: "Incident assignee cleared"
  }

op addTimelineEntry($incident: id, $body: text) =
  set incidentTimelineEntries {
    incident: $incident,
    author: $caller,
    kind: "comment",
    body: $body
  }

op createIncidentTask($incident: id, $title: text, $due: date) =
  set incidentTasks {
    incident: $incident,
    title: $title,
    due: $due,
    status: "open",
    overdue: false
  }

op editIncidentTask($task: id, $rev: int, $title: text, $due: date) =
  set incidentTasks {
    id: $task,
    rev: $rev,
    title: $title,
    due: $due,
    overdue: false
  }

op completeIncidentTask($task: id, $rev: int) =
  set incidentTasks { id: $task, rev: $rev, status: "done", overdue: false }

op reopenIncidentTask($task: id, $rev: int) =
  set incidentTasks { id: $task, rev: $rev, status: "open", overdue: false }

op addPrivateResponderNote($incident: id, $body: text) =
  set privateResponderNotes {
    incident: $incident,
    author: $caller,
    body: $body
  }

op addOrganizationResponder($organization: id, $member: id) =
  set organizationsAccess {
    organization: $organization,
    member: $member,
    level: "respond"
  }

op addOrganizationAdmin($organization: id, $member: id) =
  set organizationsAccess {
    organization: $organization,
    member: $member,
    level: "admin"
  }

op removeOrganizationMember($membership: id) =
  del organizationsAccess { id: $membership }

op shareIncidentWithGuest($incident: id, $guest: id) =
  set incidentShares { incident: $incident, guest: $guest }

op revokeIncidentGuest($share: id) =
  del incidentShares { id: $share }

routine markOverdueTasks in incidentResponse {
  trigger every day 00:05Z
  run($today: date filled) unbounded:
    incidentTasks(status == "open" and due < $today and overdue == false)[id]
    each ->
      set incidentTasks { id: id, overdue: true }
}

app incidentConsole {
  page incidents($organization) {
    table incidentList($organization: $organization) live
    form  createIncident($organization: $organization)
  }
  page incident($id) {
    detail incidentDetail($incident: $id) live
    table  incidentTimeline($incident: $id) live
    table  incidentTaskList($incident: $id) live
    table  privateNoteList($incident: $id) live
  }
  page organization($id) {
    table organizationMemberList($organization: $id) live
  }
}

app guestIncident {
  page incident($id) {
    detail incidentDetail($incident: $id) live
    table  incidentTimeline($incident: $id) live
    table  incidentTaskList($incident: $id) live
  }
}
```

The required bootstrap call is part of the attempted application because the organization row and its first admin grant must not be a second, fallible call:

```jsonc
// POST /ops/std.space.create
// Idempotency-Key: <stable key for this organization bootstrap>
{
  "slug": "<organization slug>",
  "displayName": "<organization display name>",
  "owner": { "invite": { "identifier": "<owner identifier>",
                           "method": "email-otp" } },
  "domains": ["incidentResponse@1.0.0"],
  "profile": { "incidentResponse": "default" },
  "bootstrapRows": {
    "incidentResponse.organizations": {
      "name": "<organization name>",
      "timezone": "<IANA timezone>"
    }
  },
  "custody": "managed",
  "jurisdiction": "US",
  "clients": ["incidentConsole", "guestIncident"],
  "confirm": { "jurisdiction": "<plan-bound acknowledgement>" },
  "dryRun": false
}
```

The access-profile and guest-exposure intent is necessary but has no final package/profile declaration grammar. This is the exact non-`.ordo` declaration I would give the platform integrator rather than hiding it in prose:

```yaml
profileIntent:
  spaceMembers:
    allow:
      - incidentList
      - incidentDetail
      - incidentTimeline
      - incidentTaskList
      - privateNoteList
      - organizationMemberList
      - incidentGuestList
      - createIncident
      - editIncident
      - setIncidentState
      - assignIncident
      - clearIncidentAssignee
      - addTimelineEntry
      - createIncidentTask
      - editIncidentTask
      - completeIncidentTask
      - reopenIncidentTask
      - addPrivateResponderNote
      - addOrganizationResponder
      - addOrganizationAdmin
      - removeOrganizationMember
      - shareIncidentWithGuest
      - revokeIncidentGuest
    note: Organization row guards, not the door profile, distinguish responders from admins.
  invitedGuestWindow:
    expose:
      - incidentDetail
      - incidentTimeline
      - incidentTaskList
    neverExpose:
      - privateNoteList
      - organizationMemberList
      - incidentGuestList
      - every write op
```

# SPEC FRICTION

1. **Offline editing, guest windows, and server routines cannot all select client-first custody.** The only fully described offline multi-device proposal protocol is client-first. That custody stop explicitly forbids guest windows and server-side value routines, while the brief requires both a live invited guest and a nightly task routine. I chose `managed` custody. An offline device may prepare a CAS command and submit it on reconnect; the first stale-revision edit to reach the server wins and the other receives `cas_miss`. The corpus does not specify a durable managed-custody offline queue, so that client behavior is a forced assumption.

2. **One organization per space versus several organizations in one space is not stated.** I chose one organization per space because Ordo's tenant, key, chain, profile, guest, and bootstrap boundaries are spaces. The `organizations` plural is retained because entity names are plural and because the bootstrap grammar names an entity, but only one row is created by this package. Multiple organizations in one space would share custody, profiles, owner power, and audit history and would make “tenant” misleading.

3. **The app block is not spec-compilable.** `ui-gen.md` makes `$param: expr` binding normative, but block inventory, page composition, arbitrary detail sources, `live` as a manifest modifier, form placement, and namespace collisions remain illustrative/open. I chose the tutorial's `app/page/table/detail/live` spellings and invented the obvious `form createIncident(...)` line because the brief explicitly requires create UI. Those lines are honest intent, not settled grammar.

4. **Reserved UI annotations have names but no declaration grammar.** `label`, `desc`, `order`, `choices`, and `danger` are reserved channels, yet no document says where or how to attach their values. I used `enum(...)` to provide severity/status/task choices and relied on derived names/types for labels and formatting. I could not legally mark revoke operations dangerous, hide or auto-fill `requestKey`, order form fields, or write human descriptions.

5. **The package profile template mechanism is settled but its manifest format is not.** The brief needs responder/admin/guest surfaces to be exposed differently. Organization roles are row-level ACL facts, while door profiles are space runtime state. I therefore supplied explicit YAML intent outside `.ordo`; it cannot be compiled from the current package grammar. Row guards still enforce security if an admin-only op is callable by a responder, but generated action discovery may show an action that later rejects.

6. **Guest invitation and op exposure lack a complete callable surface.** The spec settles invite/accept/principal binding and says guest exposure must be guard-bearing, but the std inventory does not name the invite/share/expose calls and no final declaration syntax binds `guestIncident` to the three safe ops. I assumed the platform creates the guest's null-role `members` row on invitation acceptance, after which `shareIncidentWithGuest` writes the domain link.

7. **“Exactly one” could mean at most one active share, exactly one at all times, or one per organization.** I chose at most one incident per guest per space by adding `unique (guest)`. A guest with no share sees none; after sharing, exactly one; after revocation, none. This prevents the same accepted guest identity from being reused for a second incident. A new invitation/principal is required for a different incident.

8. **The guest's incident body is underspecified.** I chose to let a guest see the incident's ordinary detail, timeline, and tasks, but never `privateResponderNotes`. If “read exactly one shared incident” meant only the root row and not its timeline/tasks, the guest app and incident-child inherited guards are too broad. The brief does not say.

9. **Using the derived ACL entity as an assignee target is plausible but not explicitly blessed.** `organizationsAccess` is specified to be a real, queryable, writable entity with mechanically fixed names. I therefore made `assignee -> organizationsAccess?`, which makes organization eligibility enforceable with an outgoing-ref `check` and gives the UI `assignee.member.displayName`. The corpus never explicitly says authored shapes may reference a compiler-derived entity. Pointing directly to `members` would make “assignee must be a responder/admin of this organization” inexpressible because `members` has no outgoing organization ref and `check` cannot walk reverse collections.

10. **Nullable op parameters remain unsettled.** Fields and refs have `?`; operation parameters do not have a ratified nullable spelling. I avoided inventing `$assignee: id?` by splitting assign and clear into two CAS ops, and I made task due dates and incident descriptions required. This is more verbose and gives generated forms a different interaction than a natural optional control.

11. **`enum(...)` is still `[lean]`, not settled language.** I chose it because unvalidated severity and status text is a semantic defect and because generated dropdowns require a finite choice set. If the compiler implements only settled grammar, every enum field is a likely parse/type failure. Replacing enums with text plus `check` disjunctions would improve core compilability but lose the only currently described route to generated choices.

12. **Create-once and transport idempotency solve different problems.** `requestKey unique` plus `rev: null` means a second logical create with that key rejects `expected_absent`; it does not converge changed payloads. The HTTP `Idempotency-Key` provides exact replay of the first response and is permanent for creating operations. I require the client to reuse both keys for retries. No settled annotation hides or auto-generates `requestKey` in the create form.

13. **Organization ACL vocabulary and platform space roles are separate.** I chose ACL levels `(respond, admin)`. The space owner receives `admin` on bootstrap, and organization admins grant rows to responders/admins. Platform owner/manager/editor/viewer roles only decide which ops the caller may reach at the door; they do not imply an organization level. The brief does not define how these two role systems should align.

14. **Organization root writes needed an override.** A bare ACL would derive write semantics from level ordering and could make responder authority on the organization root ambiguous. I kept `acl` for membership/admin machinery but explicitly replaced the root guard: all organization members may read the root and only `admin` may write it. The docs clearly settle replacement on children, but do not show `acl` plus an explicit guard on the same root; I assume the same replacement rule applies.

15. **“Nightly” needs a timezone choice.** Trigger times require UTC. I chose `00:05Z` for the whole space and compare due dates with a runtime-filled date. The bootstrapped organization timezone is otherwise unused. A true organization-local midnight scheduler is not described, and multiple organizations/timezones would make one routine insufficient.

16. **Overdue transition semantics are not specified.** I chose `due < today`, only for open tasks. Completing, reopening, or editing a task clears `overdue`; the next nightly run recomputes true when appropriate. There is no automatic clearing job for a due date moved forward except the authored edit op, so granting `std.incidentTasks.update` could silently violate that convention.

17. **A realtime “incident-detail stream” has no multi-collection op shape.** One pipeline cannot safely return an incident plus two independent many-branches without the fan-out guard. I chose four coordinated live reads—detail, timeline, tasks, private notes—relying on the headless runtime's page-level log-position coordinator. Guests use the same first three. This is a realtime detail page, not one nested wire envelope.

18. **Stable pagination is narrow by design.** I sort `incidentList` by immutable implicit `id`, so neither authored nor std writes can move the key. Sorting by severity, status, updated time, or assignee would be `cursor: drifting`. The UX loses natural urgency ordering to keep the brief's stability promise.

19. **The incident list is acknowledged `unbounded`.** Call-time keyset pagination bounds the returned page but current cost rules still require an author acknowledgment for an op without a statically proven candidate bound. The documentation has historically disagreed about whether `take` bounds examined rows; I did not use `take`, because that would turn a pageable list into top-N semantics.

20. **Admin deletion and destructive UI are deliberately absent.** “Manage all incidents” does not explicitly require hard deletion. Exposing derived `std.incidents.delete` would let responders delete if the door grant were broad and would need danger/step-up metadata whose authoring grammar is missing. I chose status transitions (`resolved`, `closed`) and left std delete unexposed.

21. **Revocation cannot erase a guest's memory.** Removing `incidentShares` closes future guarded reads. It cannot revoke plaintext already fetched into a guest mirror. This is a settled platform limitation and must appear in share consent.

22. **The bootstrap policy examples contradict the no-default rule.** The normative bootstrap text says custody and jurisdiction have no defaults and cannot be satisfied by deployment configuration, while the illustrative `policy spaces` block still contains `defaults custody: managed, jurisdiction: US`. I followed the stronger bootstrap ruling and supplied both explicitly.

23. **Propagation of an intermediate explicit guard down its owned subtree is not stated clearly enough to bet guest access on.** `incidents` replaces its inherited organization ACL to add the guest disjunct. The corpus clearly says an explicit child guard replaces inheritance, but does not give a worked case proving whether that replacement becomes the inherited rule for grandchildren. I therefore restated both verbs on timeline entries and tasks. That duplication is intentional; omitting it could silently make the guest see only the incident root.

## Assessment

### Likely written-spec compilation

**Core definition: plausible but not confidently compilable.** The shapes, guards, owned refs, create-once write, CAS writes, stable list, routine flag slot, and parameter bindings follow settled examples closely. The biggest core-language risks are `[lean]` `enum`, refs to `organizationsAccess`, `acl` plus root-level explicit guards, and `filled` on a routine parameter. The latter is settled for parameters but not demonstrated on a routine.

**App/profile layer: not compilable from the current written spec.** The binding expressions are normative; the block/form/detail/live/profile declarations are not. The YAML is intentionally not represented as Ordo syntax.

I would expect a real current parser to fail much earlier because the repository status says most of the language/runtime is not implemented, but this review scores written-spec buildability, not current code availability.

### Semantic and security correctness

Under managed custody and the stated exposure intent, the row-security design is sound on paper:

- responders and admins are organization ACL holders;
- incidents accept those holders for reads/writes and add one guest read path;
- timeline and task children explicitly restate the same read/write split;
- private notes replace inheritance and omit the guest disjunct;
- share rows require a null-role guest, are admin-written, and are unique per guest;
- assignees are organization-access rows checked against the incident's organization;
- all conflicting incident mutations use `id + rev` CAS;
- incident create is atomic with its first timeline entry and rejects key reuse;
- membership, share, task, note, and incident writes remain single-space attributed commands.

The principal security residual is not a hidden row leak; it is the missing declarative exposure/profile layer. A platform integrator must reproduce the YAML intent correctly. Client-first custody would materially invalidate the row-security promise and is intentionally not selected.

### Idiomaticity

The definition is strongly Ordo-shaped: one space per tenant, a bootstrapped ACL root, derived access rows instead of a shadow identity table, ownership for composition, explicit guest-aware child guards, a replacement guard for the secret child, create-once rather than keyed upsert, CAS rather than last-write-wins, immutable-id keyset ordering, clock-derived routine authority, live reads rather than pushed data, and separate ops instead of nullable-parameter invention.

The least idiomatic parts are all pressure from incomplete surfaces: the external YAML, the invented `form` line, a client-supplied request key visible to UI generation, and the derived ACL entity used as the assignee type.

### Silent-wrong risks

The highest risks if an implementation accepted this without strong diagnostics are:

1. treating `requestKey + rev: null` as ordinary converge-or-create and silently rewriting an existing incident;
2. composing the guest disjunct into private-note guards instead of replacing inherited access;
3. accepting a plain `members` assignee and losing organization eligibility;
4. granting the guest app `privateNoteList` because exposure intent is not compile-owned;
5. rendering `requestKey` as a human field and allowing accidental key reuse;
6. presenting admin actions to responders because the catalog knows door access but not the row on which an ACL guard will pass;
7. claiming stable paging after changing the list sort away from `id`;
8. treating an empty/short refined live page as end-of-list;
9. assuming managed mirrors provide a durable offline mutation queue when only client-first proposals are specified;
10. allowing std full-row task updates to move due/status without maintaining `overdue`;
11. interpreting `unique (guest)` as deployment-wide rather than space-scoped;
12. expanding `whole incidents` without applying the private-note child guard.

### Brief coverage

| Brief requirement | Coverage |
|---|---|
| Multi-tenant incident service | One organization/tenant per space; atomic bootstrap creates the tenant root and admin grant. |
| Organization members/admins/responders | `organizationsAccess` with `respond`/`admin`; explicit admin membership ops. |
| Severity/status/assignee | Incident enum fields; assignee is an organization-access row with an organization check. |
| Timeline/tasks | Owned entities, read/write with the incident. |
| Sealed private notes | Sealed body plus a replacement guard with no guest path. |
| Admins manage membership/all incidents | Admin-only membership/share guards; all incident ops accept admin and responder ACL holders. |
| Responders read/write org incidents | Incident and inherited child guards accept organization access. |
| Guest exactly one incident, no notes | Null-role checked share, `unique (guest)`, guest-only app omitting notes, note guard excludes share. |
| Atomic organization/bootstrap | `bootstrap organizations { ... }` plus one `std.space.create` genesis call. |
| Idempotent incident creation | Transport `Idempotency-Key` plus unique `requestKey` and `rev: null` create-once. |
| Stable paginated list | Pageable `incidentList`, `sort -id`. |
| Realtime incident detail | Coordinated live detail/timeline/task/note reads; guest gets only the safe three. |
| Nightly overdue routine | Clock trigger, filled date, `unbounded`, open-task update. |
| Generated list/detail/create UI | Plausible app manifest and enums, but form/detail/live grammar remains non-normative. |
| Two offline devices edit one incident | Every incident mutation is CAS; first accepted revision wins, stale edit gets `cas_miss`; managed offline queue is unspecified. |

## Prior-fix verification

| Prior area | Round-4 status | Verification in this build |
|---|---|---|
| `bootstrap.md` / atomic tenant bootstrap | **Closed at design level, with residual contradictions** | `std.space.create`, domain bootstrap rows, owner member, and ACL creator grant are one genesis group. Custody/jurisdiction no-default text conflicts with the illustrative policy block, and client-first genesis still has competing directions, neither affecting this managed-custody attempt. |
| App grammar | **Relocated/partially open** | Parameter binding is normative. Page blocks, create forms, arbitrary detail, live modifier, layout, and namespaces remain open; the attempted app is therefore intent, not a compile target. |
| Nullable parameters | **Open** | No settled `$x: type?` or optional-param grammar. This build avoids it with assign/clear ops and required form fields. |
| Create-once/idempotency | **Closed** | `rev: null` on a unique-key write is settled `creates` mode with `expected_absent`; transport replay is separately keyed/fingerprinted. The attempted create uses both layers. |
| ACL creator bootstrap | **Closed at design level** | G10a exempts the ACL root create and the creator's top-level row lands in the same group. The current engine plan now states the root/link distinction correctly. |
| ACLs and guests | **Mostly closed; exposure plumbing open** | Guard grammar, both verbs, replacement, guest null role, and guest-bearing-op requirement are settled. Invitation/exposure op inventory and package-profile syntax remain incomplete. Client-first guest windows are explicitly unavailable. |
| Stable paging | **Closed for this key choice** | Immutable `id` gives a construction-level stable cursor, avoiding authored/std mutability and post-take composition. General cursor analysis remains narrower than many readers assume. |
| Routine authority | **Closed at design level** | Clock-woken routines pass guards; listener routines are trigger-id bounded. This attempt uses a clock and acknowledges its whole-space scan with `unbounded`. |
| Client-first conflict semantics | **Closed as a disclosed weaker model, but unusable for the whole brief** | Server arrival order is canonical; devices fold deterministically; CAS rejection is history with no state change; resolution is a new command; no CRDT/silent merge. Client-first also removes server enforcement, guest windows, and server routines, so this build chooses managed custody. |
| Assignee eligibility | **Relocated/conditionally closed** | Ref-walking checks are settled, but `members` has no organization ref. Using `organizationsAccess` as the assignee makes the invariant expressible; legality and UI treatment of authored refs to derived ACL entities are not explicitly documented. |
| `filled` execution-relative params | **Closed in the language, stale prose remains** | `definition-language.md` ratifies `filled` and forbids `filled + reproducible`; `ui-gen.md` still contains an earlier paragraph saying the spelling is unratified before later correcting itself. |

## Author errors versus documentation/design faults

### Author-owned choices and risks

- I chose one organization per space, managed custody, UTC nightly evaluation, at-most-one share per guest, and guest visibility of timeline/tasks.
- I chose required description/due fields and separate clear-assignee operations to avoid optional parameters.
- I chose an ACL-derived membership row as the assignee type.
- I chose immutable-id ordering over severity-first UX.
- I did not expose hard delete or std full-row updates.
- I made `requestKey` a caller parameter despite the create-form UX cost.

If product intent differs on any of those points, that is an author/product decision, not a compiler defect.

### Documentation/design-owned faults

- No settled nullable-parameter grammar.
- No final app/page/form/detail/live grammar and no syntax for attaching reserved UI annotations.
- No final package/profile template grammar or guest op-exposure declaration.
- No named std invitation/share acceptance surface sufficient to write the end-to-end guest flow.
- The only detailed offline mutation protocol selects a custody mode incompatible with guest windows and server routines.
- No explicit ruling that authored fields may ref ACL-derived entities, despite those entities being described as real and extendable.
- Contradictory bootstrap policy examples about whether custody/jurisdiction may default.
- Stale self-contradictory `ui-gen.md` prose around `filled` despite the language-level closure.

## Overall verdict

The written corpus is now strong enough to produce a substantially correct core incident model without inventing authorization, bootstrap, idempotency, paging, routine, or conflict semantics. The hard security shape—organization ACL plus per-incident guest window plus a private-note override—is derivable and auditable.

It is not yet strong enough to emit the *complete application the brief asks for* as one confidently compilable artifact. The remaining buildability failure has moved upward: core data/authority semantics are mostly closed, while UI composition, form metadata, access-profile packaging, guest exposure plumbing, nullable form parameters, and the managed-offline story are still prose seams. A developer can write the backend definition; they cannot yet write a fully declared list/detail/create application and guest surface without preserving non-normative intent beside it, as this review had to do.
