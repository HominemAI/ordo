# AI Buildability — Cheap Tier, Round 4

## Attempted definition (verbatim)

```ordo
domain incidentresponse @ 0.1.0

entity organizations {
  name: text
  acl (read, write, admin)
}

entity incidents {
  organization -> organizations
  clientKey:    text unique
  title:        text
  severity:     text
  status:       text = "open"
  assignee     -> members?
  acl (read, write, admin)
}

entity timelineEntries {
  incident -> incidents owned
  body:     text
  at:       date = today
  by        -> members
}

entity tasks {
  incident -> incidents owned
  title:    text
  due:      date?
  done:     bool = false
  overdue:  bool = false
  assignee  -> members?
}

entity responderNotes {
  incident -> incidents owned
  body:     text sealed
  at:       date = today
  by        -> members

  access read   incident.incidentsAccess(member == $caller and (level == "write" or level == "admin"))
  access write  incident.incidentsAccess(member == $caller and (level == "write" or level == "admin"))
}

op bootstrapOrganization($name: text, $coAdmin: id) =
  let organization = set organizations { name: $name }
  set organizationsAccess { organization: organization.id, member: $coAdmin, level: "admin" }

op createIncident($organization: id, $clientKey: text, $title: text, $severity: text,
                  $adminGroup: id, $responderGroup: id) =
  let incident = set incidents { organization: $organization, clientKey: $clientKey,
                                 title: $title, severity: $severity, status: "open" }
  set incidentsAccess { incident: incident.id, member: $adminGroup, level: "admin" }
  set incidentsAccess { incident: incident.id, member: $responderGroup, level: "write" }

op incidentList =
  incidents[id, title, severity, status, assignee.displayName]
  sort id

op incidentDetail($incident: id) =
  incidents(id == $incident)[id, rev, title, severity, status, assignee.displayName]

op incidentTimeline($incident: id) =
  timelineEntries(incident == $incident)[id, body, at, by.displayName]
  sort id

op incidentTasks($incident: id) =
  tasks(incident == $incident)[id, rev, title, due, done, overdue, assignee.displayName]
  sort id

op createTimelineEntry($incident: id, $body: text, $by: id) =
  set timelineEntries { incident: $incident, body: $body, by: $by }

op createTask($incident: id, $title: text, $due: date, $assignee: id) =
  set tasks { incident: $incident, title: $title, due: $due, assignee: $assignee }

op editIncident($incident: id, $rev: int, $title: text, $severity: text, $status: text,
                $assignee: id) =
  set incidents { id: $incident, rev: $rev, title: $title, severity: $severity,
                  status: $status, assignee: $assignee }

op editTask($task: id, $rev: int, $title: text, $due: date, $done: bool,
            $assignee: id) =
  set tasks { id: $task, rev: $rev, title: $title, due: $due, done: $done,
              assignee: $assignee }

op addResponderNote($incident: id, $body: text, $by: id) =
  set responderNotes { incident: $incident, body: $body, by: $by }

op responderNotesForIncident($incident: id) =
  responderNotes(incident == $incident)[id, body, at, by.displayName]
  sort id

routine markOverdueTasks in incidentresponse {
  trigger every day 02:00Z
  run($today: date):
    tasks(done == false and due is not null and due < $today)[id]
    each ->
      set tasks { id: id, overdue: true }
}

app incidentresponse {
  page incidents {
    table incidentList live
  }
  page incident($id) {
    detail incidentDetail($incident: $id)
    table incidentTimeline($incident: $id) live
    table incidentTasks($incident: $id) live
    table responderNotesForIncident($incident: $id) live
  }
  page createIncident {
    form createIncident
  }
}
```

## # BUILDER NOTES

- I modeled each organization as a row, while the allowed glossary says a space is the tenancy boundary. It may be more correct to use one space per organization and omit `organizations`; the brief's plural wording made the intended arrangement unclear.
- The four fixed membership roles are owner/manager/editor/viewer, not admin/responder/guest. I inferred that organization administrators and responders should be runtime groups granted to each incident ACL, with role profiles controlling which operations members may call. The permitted docs did not provide syntax for a profile template, group creation, or declaring fixed per-organization groups.
- `createIncident` therefore takes `$adminGroup` and `$responderGroup`. This is an implementation-facing guess: it makes ACL grants atomic, but exposes group identifiers in a generated create form unless a surface feature not shown in the allowed docs supplies them.
- A guest can be granted `read` on exactly one `incidentsAccess` row. Because `responderNotes` requires write-or-admin access, the guest cannot read notes. I assume the ACL derived from `incidents` also protects its owned timeline entries and tasks; the tutorial documents that inheritance.
- It is unclear whether an `acl` entity may also have an ordinary association ref as `incidents` does, and whether the `incidentsAccess` derived link name is always available in the same domain text exactly as written. The tutorial uses analogous names.
- I guessed that `acl (read, write, admin)` permits the level literals used in the explicit responder-note guard and that this guard's relationship walk is accepted. The tutorial shows the same guard form for a link entity but not an ACL-derived child override.
- I deliberately did not mark `timelineEntries` or `tasks` sealed: responders and guests should see incident details, while notes alone are sealed/private.
- Idempotency is represented by per-space `clientKey: text unique` and the creation-shaped `set` that covers it, which the allowed tutorial calls converge-or-create. A retry with the same key will also converge the listed incident fields; that is a behavioral consequence of this attempted definition.
- The incident list sorts only by immutable `id`, intentionally trading a recency/severity ordering for the allowed docs' strongest stable-keyset-pagination claim. Pagination itself is call-time/envelope behavior, not written in the op.
- “Realtime incident-detail stream” is expressed as `live` manifest bindings on the incident page's detail and supporting tables. The tutorial explicitly documents `table ... live`; it shows `detail` but does not explicitly show `detail ... live`, so I left the detail line plain and used live child surfaces.
- Conflict handling for two offline devices is CAS (`rev`) on incident and task edits, which yields `cas_miss` and refetch on conflict. Timeline/note creation remains append-only. The allowed docs do not specify a merge operation, offline sync declaration, or a way to CAS a partial edit without supplying all fields named in the operation.
- The nightly task routine uses `$today` because `today` is legal only in field defaults. It only sets `overdue: true`; I did not define clearing overdue when a due date is moved, completed, or re-opened. That lifecycle needs additional writes/routine logic not dictated by the brief.
- I used `date` for timestamps because that is what the allowed examples expose. A distinct time/timestamp type, actor auto-stamping feature, client/identity declaration syntax, profile template syntax, app `form` syntax, and generated action declaration syntax were searched for in the allowed README/tutorial/glossary/traps material but not found.
- The manifest grammar is explicitly described as illustrative/lean in the tutorial. `form createIncident` is therefore an informed guess rather than a documented concrete form declaration.

Compilation confidence (without consulting the full specification): **low (35%)**. The central shape/op/routine patterns follow the tutorial, but tenant/profile/group provisioning, ACL composition for organization-wide roles, and some manifest details are not concretely specified in the allowed documents.
