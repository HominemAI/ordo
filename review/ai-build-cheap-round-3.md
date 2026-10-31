# Cold build attempt — multi-tenant incident response

```ordo
domain incidentresponse @ 0.1.0

entity organizations {
  name: text unique
  acl (read, write, admin)
}

entity incidents {
  organization -> organizations
  key:          text unique
  title:        text
  severity:     text = "medium"
  status:       text = "open"
  createdOn:    date = today
  dueOn:        date?
  assignee      -> members?
  acl (read, write, admin)
}

entity incidentTimeline {
  incident -> incidents owned
  body:     text
  at:       date = today
  by:       text = ""
}

entity incidentTasks {
  incident -> incidents owned
  title:    text
  dueOn:    date?
  status:   text = "open"
  assignee  -> members?
}

entity responderNotes {
  incident -> incidents owned
  body:     text sealed
  author    -> members
  createdOn: date = today

  access read   incident.incidentsAccess(member == $caller and (level == "write" or level == "admin"))
  access write  incident.incidentsAccess(member == $caller and (level == "write" or level == "admin"))
}

# Organization creation is the intended bootstrap: because organizations has
# acl, the creating principal receives its admin access row in the same command
# group by platform derivation.
op bootstrapOrganization($name: text) =
  set organizations { name: $name }

# Intended membership administration: use the acl-derived organization grant,
# revoke, and level-change operations. Admins grant admin; responders grant
# write. A guest is granted read only on one incident's acl-derived access row.

# The incident key is client-supplied and unique, so this is a converges-or-
# creates idempotent creation. The created incident's creator receives incident
# admin in the platform-derived atomic acl bootstrap.
op createIncident($organization: id, $key: text, $title: text,
                  $severity: text, $dueOn: date?) =
  set incidents { organization: $organization, key: $key, title: $title,
                  severity: $severity, dueOn: $dueOn }

op updateIncident($incident: id, $rev: int, $severity: text, $status: text,
                  $assignee: id?) =
  set incidents { id: $incident, rev: $rev, severity: $severity,
                  status: $status, assignee: $assignee }

op addTimelineEntry($incident: id, $body: text, $by: text) =
  set incidentTimeline { incident: $incident, body: $body, by: $by }

op createTask($incident: id, $title: text, $dueOn: date?, $assignee: id?) =
  set incidentTasks { incident: $incident, title: $title, dueOn: $dueOn,
                      assignee: $assignee }

op updateTask($task: id, $rev: int, $title: text, $dueOn: date?,
              $status: text, $assignee: id?) =
  set incidentTasks { id: $task, rev: $rev, title: $title, dueOn: $dueOn,
                      status: $status, assignee: $assignee }

op addResponderNote($incident: id, $body: text, $author: id) =
  set responderNotes { incident: $incident, body: $body, author: $author }

# Stable pagination: createdOn is only defaulted on creation and no authored
# write changes it; the implicit id completes the total sort order.
op incidentList($organization: id) =
  incidents(organization == $organization)[key, title, severity, status,
                                             createdOn, dueOn, assignee.name]
  sort -createdOn

op incidentDetail($incident: id) =
  incidents(id == $incident)[key, title, severity, status, createdOn, dueOn,
                              assignee.name]

op incidentTimelineList($incident: id) =
  incidentTimeline(incident == $incident)[body, at, by]
  sort -at

op incidentTaskList($incident: id) =
  incidentTasks(incident == $incident)[title, dueOn, status, assignee.name]
  sort dueOn

op incidentResponderNotes($incident: id) =
  responderNotes(incident == $incident)[body, author.name, createdOn]
  sort -createdOn

routine markOverdueTasks in incidentresponse {
  trigger every day 00:00Z
  run($today: date):
    incidentTasks[id, dueOn, status]
    | (dueOn < $today) and status != "done"
    each ->
      set incidentTasks { id: incidentTasks.id, status: "overdue" }
}

app incidentresponse {
  page incidents {
    table incidentList live
  }
  page incident($id) {
    detail incidentDetail($incident: $id)
    table  incidentTimelineList($incident: $id) live
    table  incidentTaskList($incident: $id) live
    table  incidentResponderNotes($incident: $id) live
  }
  page createIncident {
    form createIncident
  }
}
```

# BUILDER NOTES

Confidence this compiles: **low (20%)**. I did not consult the full specification, syntax examples, code, tests, or any other documentation, and did not execute anything.

Confusions and guesses:

- I searched the allowed README, glossary, tutorial, and traps for a way to compose an organization-wide ACL (admins manage all incidents) with an incident-specific ACL (a guest reads exactly one incident). I found `acl`, inheritance to owned children, explicit guard replacement, and guest windows, but no documented syntax or semantics for combining two ACL roots. I guessed an unowned `organization -> organizations` ref plus a separate `acl` on `incidents`. That does **not** by itself establish that organization admins can manage every incident; it requires grants to each incident or undocumented ACL composition.
- I searched for an explicit syntax to call or declare acl-derived grant/revoke operations and for a group identifier usable in an operation. The tutorial says those operations are derived, but does not give callable names/signatures. I therefore left membership management as a declaration comment rather than inventing API syntax. This means the requested organization-admin membership flow is not fully emitted.
- I guessed that the derived link entity for `incidents` is named `incidentsAccess`, based on the tutorial's `projects` → `projectsAccess` example. I also guessed it is reachable from `responderNotes` as `incident.incidentsAccess`.
- I searched for a documented way to distinguish a guest access row from a responder access row. I guessed `write` and `admin` represent responders/admins and `read` represents guests, and used an explicit guard over those two literal levels for sealed notes. This correctly expresses the intended privacy only if the guessed derived link name and level semantics are valid.
- The tutorial says guests are not in the space but also describes an ACL row naming their principal, while the illustrative ACL link points `member -> members`. I could not resolve how `$caller` matches a guest for the derived incident ACL, so the guest requirement remains uncertain.
- I used two ACL-bearing entities. The allowed docs demonstrate ACL roots and owned inheritance but do not state whether multiple ACL roots linked by an unowned ref are permitted.
- I chose `key: text unique` as the idempotency key. The key is global to `incidents`, not scoped by organization, because I only saw single-field `unique` in the tutorial and did not confirm composite-unique syntax. This is a functional guess and may be too restrictive.
- I used `date` rather than a timestamp for timeline and task dates because the tutorial examples show `date = today`; I did not find an allowed-doc example of a timestamp default. Consequently same-day timeline ordering relies on the implicit id and may not satisfy a true chronological timeline.
- I passed `$by: text` for timeline attribution rather than using a `-> members` author ref. This was copied from the tutorial pattern, but it permits caller-supplied display attribution; it is not a trustworthy responder identity model.
- I used `$author: id` in `addResponderNote` because the body needs the required `author -> members` reference. I could not find a documented write expression that binds a ref directly to `$caller`, so this can spoof the note author unless an omitted guard or operation convention prevents it.
- I assumed `id?` is permitted as a nullable operation parameter because nullable fields use `?`; the allowed docs did not show nullable parameters.
- I assumed `form createIncident` is valid UI-manifest syntax. The tutorial says write operations generate forms/actions, but only gives `metric`, `table`, and `detail` manifest syntax; it does not show `form`.
- I assumed a page with `table incidentList live` can supply its required `$organization` parameter from surrounding context. The documented table binding example always names bindings. The list page is therefore likely incomplete.
- I exposed `incidentResponderNotes` on the common incident page. The row guard should filter guests, but the allowed docs do not state whether a UI table with no visible rows is the intended way to hide a private surface.
- I searched the allowed docs for an explicit "atomic bootstrap" write pattern that creates an organization and also installs membership. The only documented atomic bootstrap is platform-derived: creation of an `acl` row grants its creator top-level access in the same command group. `bootstrapOrganization` relies on that rule; it does not explicitly create platform `members` rows or organization membership records.
- I relied on the tutorial's stated atomicity of a multi-statement operation but did not use it, because no documented way was found to reference a newly minted incident id when creating ACL grants in the same operation.
- I used `trigger every day 00:00Z`, `run($today: date):`, pipeline filtering, `each ->`, and id-addressed convergence exactly in the tutorial's established forms. The routine intentionally writes only `status`, avoiding its own overdue state changes. Guards do not apply to routines according to the traps; the routine's field/entity scope is therefore its relevant authority boundary.
- The nightly routine could mark tasks already marked overdue again; convergence makes this harmless. It will also mark any non-`done` status, which is a guess because status is text rather than a documented enum type.
- The list sorts by `createdOn`, a creation default never altered by the authored writes, to aim for stable keyset pagination. The UI/API pagination envelope itself is generated at call time, not declared in source, according to the tutorial.
- I did not declare a client or profile/role-to-operation surface: the glossary says profiles/grants are runtime state and clients are IaC, but the allowed docs give no complete syntax for either. This is missing metadata if "complete definition" requires deployment identity configuration.
- I did not declare a route/notification because the brief did not request notifications.
- The README states the engine is pre-alpha and authorization, ACL, generated UI, planner, and executor are not implemented; accordingly even a language-correct definition cannot presently be compiled or run.
