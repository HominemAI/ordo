# Teamspace

A team project tracker in one shared tenant. A founder signs up with a
passkey and gets a tenant, as its owner. Colleagues join by invitation, at a
role. Admins manage projects and teams and read reports across everything.
Members work on the projects they are on.

This file says what the app shows, how to run it, what the deployment owner
sets, and what Ordo still owes it.

## 1 · What it is and what it shows

It covers three ideas from `examples/IDEAS.md`:

- **A team workspace.** One tenant, many people. Everyone sees the same team.
- **Roles.** Admins and members. The door, the guard and the program each
  decide part of what a role may do.
- **Admin reports across everything.** Totals over every project and every
  person, including ones the admin is not on.

| Part | Where |
|---|---|
| The program, the whole server side | [`teamspace.ordo`](teamspace.ordo) |
| The page: React on `@hominemai/ordo-react`, built with Vite | [`web/`](web) |
| The local deployment, one command | [`dev/serve.sh`](dev/serve.sh) |
| A made-up company to fill it | [`dev/seed.mjs`](dev/seed.mjs) |
| The proofs that need a server | `crates/ordo-host/tests/teamspace_app.rs` |
| The proofs that need none | `web/test/` (`npx vitest run`) |

## 2 · The program

```
ordo check examples/teamspace/teamspace.ordo
```

Three entities: `projects`, `team` (who works on which project) and `tasks`.

### The three layers

Three places decide who may do what.

1. **The door.** It looks at the caller's role before the program runs.
   - Every role may call the authored reads.
   - Editors and above may call the authored writes.
   - The derived `std.*` ops are for managers and owners only.
2. **The guard.** `tasks` has `access read project.team(member == $caller)`. A
   person reads the tasks of the projects whose team they are on.
3. **The reports.** They are `unguarded`, so they total every project. Each
   one checks the caller's role in its own body.

### The roles

| Role | Kind | May do |
|---|---|---|
| `owner` | admin | Everything a manager does, and change roles |
| `manager` | admin | Projects, teams, invitations, reports, tasks |
| `editor` | member | Read and write tasks of their projects |
| `viewer` | member | Read only |

- Invitations are for managers and owners. A manager cannot invite above
  their own role.
- Role changes are for owners only (`std.space.setRole`).
- Projects and teams go through `std.projects.*` and `std.team.*`. The door
  keeps those for managers and owners.
- Tasks go through the program's own ops.

### The ops

| Op | Params | What it does |
|---|---|---|
| `me` | none | The caller's own seat: `id`, `displayName`, `role`. The page reads its role from here |
| `people` | none | Everyone in the tenant, with role, status and join date |
| `myProjects` | none | The projects the caller is on |
| `projectTeam` | `$project` | Who is on one project |
| `board` | `$project` | One project's tasks, oldest first. The guard leaves out a project the caller is not on |
| `createTask` | `$project`, `$title`, `$assignee?`, `$due?`, `$added` | Adds a task. The page stamps `$added` from its own clock (`filled`) |
| `moveTask` | `$id`, `$rev`, `$status` | Changes a task's status |
| `editTask` | `$id`, `$rev`, `$title`, `$assignee?`, `$due?` | Changes title, assignee and due date |
| `reportByProject` | `$today` | One row per project: total, todo, doing, review, done, overdue. Admins only, by the access template |
| `reportByPerson` | `$today` | One row per person: total, open, done, overdue. Admins only, by the access template |
| `std.projects.list`, `create`, `update`, `delete` | derived | Manage projects. Managers and owners |
| `std.team.create`, `delete` | derived | Put a person on a project, or take them off. Managers and owners |
| `std.space.invite` | `role?`, `expires?`, `label?` | Makes an invitation ticket. Managers and owners |
| `std.space.invitations` | none | Lists invitations and their status |
| `std.space.revokeInvitation` | `id` | Withdraws an invitation |
| `std.space.setRole` | `member`, `role` | Changes a person's role. Owners only |

`moveTask` and `editTask` are compare-and-set on `rev`. A task someone else
changed meanwhile answers `cas_miss`, and nothing is overwritten.

### The reports

- Both are `unguarded`. The guard on `tasks` would otherwise cut them down to
  the projects the caller is on.
- Each starts with `let admin = members(id == $caller and (role == "owner" or role == "manager")) reduce { n: count }`.
- Every other step carries `admin.n > 0`. This is the in-body role check. A
  member gets no rows.
- A second `let` groups `tasks` on `project` (or on `assignee`) and counts by
  status.
- That grouping is joined back to `projects` (or `members`). A project with
  no tasks still has its row.
- `$today` is `filled`. The page stamps it, and overdue means `due < $today`
  and not done.

## 3 · Try it on this machine

You need:

- PostgreSQL 15 or later. `ORDO_PGBIN` names its bin directory.
- Node 18 or later.
- A browser with passkeys.

```
cargo build -p ordo-cli -p ordo-host
sh examples/teamspace/dev/serve.sh
```

`serve.sh` starts a throwaway Postgres on port 55492 under `dev/.data`, boots
the door on `127.0.0.1:4001`, applies `teamspace.ordo`, and stays in the
foreground. Ctrl-C stops both. The page belongs at `http://localhost:5174`.
The caller may set `ORDO_AUTH_METHODS` and `ORDO_AUTH_PROVISIONING` before
running it.

In another terminal:

```
cd examples/teamspace/web && npm install && npm run dev
```

Open `http://localhost:5174`.

### A walkthrough

1. Sign up with a passkey. You found a team and you are its owner.
2. Open **Projects**. Create a project and add yourself to its team.
3. Add some tasks. Move them across the board.
4. Open **Members**. Invite a colleague as `editor`. The link shows once. It
   looks like `#/join/<space>/<ticket>`. Making it needs a fresh passkey touch.
5. Open the link in another browser profile. The colleague signs up and joins
   as an editor.
6. The colleague sees no Projects, Members or Report. Only tasks of projects
   they are on.
7. Back as owner, change the colleague's role in **Members**.
8. Open **Report**. The totals cover every project and person.

### Seeding

To fill a team with made-up people, start the door with seeding on:

```
TEAMSPACE_SEEDABLE=1 sh examples/teamspace/dev/serve.sh
node examples/teamspace/dev/seed.mjs [--seed N] [--verify]
```

- The script goes through the door's ops, as the people it makes. It never
  touches SQL.
- A script cannot touch a passkey. So every person it makes signs in as a
  guest.
- It makes Northwind Studio: a founder, 7 colleagues, 3 projects and about
  64 tasks. The same `--seed` gives the same data.
- It ends by printing a link. Open it to join as an owner, and finish by
  passkey.

**Warning.** `TEAMSPACE_SEEDABLE=1` turns guests on and frees
`std.space.invite` from the passkey touch. The guest setting is stored on the
first boot and stays on for that data directory. Delete `dev/.data` to reset.
Never use it on a real deployment.

### Invite-only

```
ORDO_AUTH_PROVISIONING=invite-only sh examples/teamspace/dev/serve.sh
```

- Nobody new can found a team.
- An invitation still works with every sign-in method.
- A colleague who joins also gets an empty tenant of their own, from
  `on-signup`. The page then asks which team to open (`GET /auth/spaces`) and
  remembers the answer.

## 4 · The settings

`serve.sh` sets the local column. The deployment owner sets the same
variables on a live deployment.

| Variable | Locally | Why |
|---|---|---|
| `ORDO_AUTH_METHODS` | `passkey` | sign-in and sign-up |
| `ORDO_APEX_DOMAIN` | `localhost` | the passkey relying party. Changing it strands every passkey |
| `ORDO_AUTH_ORIGINS` | `http://localhost:5174` | which page may run a ceremony |
| `ORDO_CORS_ORIGINS` | `http://localhost:5174` | which page may call the door |
| `ORDO_AUTH_PROVISIONING` | `auto-onboard` or `invite-only` | who may found a team |
| `ORDO_PROVISION_SELF_SERVE` | `on-signup` | a founder gets a tenant |
| `ORDO_ACCESS_TEMPLATE` | `reportByProject=owner,manager; reportByPerson=owner,manager` | the reports are for owners and managers. A tenant owner may open them to a group |
| `ORDO_STEP_UP_EXEMPT_OPS` | not set | ops that skip the passkey touch. The seed sets `std.space.invite` |
| `ORDO_AUTH_GUEST` | off | guest accounts. The seed turns them on |

The page build takes two variables:

| Variable | Meaning |
|---|---|
| `VITE_ORDO_URL` | the door's address. Required |
| `VITE_ORDO_SIGN_IN` | a fallback only. The page asks the door which ways in it serves (`GET /auth/methods`). If the door cannot answer, the page reads this list, such as `passkey`. Unset, it is `passkey` |

## 5 · What Ordo still owes this app

These are known defects. Each is marked "planned" in the app.

Done: the door now keeps the reports for owners and managers. The access
template (`ORDO_ACCESS_TEMPLATE`) says so, and the in-body role checks are
gone. A tenant owner may open them to a group with `std.space.setOpAccess`
(`docs/howto-roles.md`).

1. **`members.principal` cannot be projected.** `principal` is a reserved
   word, so `members[id, principal]` is a parse error. The page reads its own
   seat with the `me` op instead.
2. **`web/src/interface.ts` adds the invitation and role ops by hand.**
   `clients/ordo-react/src/platform.ts` now carries `std.space.invite`,
   `invitations`, `revokeInvitation` and `setRole`. The page moves to them
   with the next package release.
3. **A report row for a project or person with no tasks has no count
   fields.** It has no zeros either. The page shows a missing count as 0.

## 6 · The tests

The live tests need the conformance cluster:

```
bash scripts/conformance-pg.sh start
cargo test -p ordo-host --test teamspace_app
```

The page's own tests need no server:

```
cd examples/teamspace/web && npm ci && npx vitest run
```

Each live test boots the real `ordo-host` binary on real Postgres. The test
acts as the page does, with a software authenticator in place of the browser.

| Test | What it proves |
|---|---|
| `a_founder_gets_a_team_and_an_invited_colleague_joins_it_as_a_member` | A founder signs up and owns a tenant. An invited newcomer joins it as an `editor` |
| `a_member_cannot_change_roles_manage_projects_or_read_the_admin_report` | The door refuses a member the role, invitation, project and team ops. A member reads only their projects' tasks and gets no report rows |
| `the_admin_report_totals_every_project_and_person` | Both reports total every project and person, including a project the admin is not on, with the right numbers |
| `under_invite_only_an_invited_newcomer_joins_and_a_stranger_is_refused` | Under `invite-only`, an invitation gets a newcomer in and a stranger is refused |
| `a_member_adds_tasks_only_to_projects_they_are_on` | A member adds tasks to a project they are on and is refused on one they are not (25ay) |
| `an_invited_colleague_is_shown_by_the_name_they_gave` | A colleague admitted by invitation is shown by the name they gave, not their account id (25ba) |
| `a_tenant_owner_opens_the_report_to_a_group` | A tenant owner makes a group, adds a member and opens `reportByProject` to it with `std.space.setOpAccess`. The member then reads the report and another tenant is unchanged |
