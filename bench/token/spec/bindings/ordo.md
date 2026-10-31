## Stack: Ordo

### Where things go

| Path | What |
|---|---|
| `backend/*.ordo` | the program. `ordo check backend` checks it |
| `backend/deploy.env` | optional. `ORDO_*=value` lines: your deployment's settings |
| `web/` | the page. `@hominemai/ordo-sdk` and `@hominemai/ordo-react` are listed in `web/package.json`; `npm install` puts them in `node_modules` |
| `guidance/` | Ordo's docs |

`ordo` is on your PATH. `ordo gen ts backend --out <dir>` writes the typed
client; `web/gen.sh` shows the way the examples keep it in `web/src`.

### Running it

`bash run-stack.sh` starts Postgres, a mail catcher, the door on
`http://127.0.0.1:4710`, applies `backend/`, and serves the page on
`http://localhost:5310`. `BENCH_NO_WEB=1 bash run-stack.sh` skips the page.
Ctrl-C stops it all.

- The page reaches the door at `import.meta.env.VITE_BACKEND_URL`
  (`http://localhost:4710`).
- Emailed codes: `GET http://127.0.0.1:4726/messages?to=<email>`.
- The start script sets the machine: database, keys, ports, origins, mail,
  and `ORDO_AUTH_METHODS=email`. Those values win over `deploy.env`.
  Everything else about the deployment is yours to set in `deploy.env`.

### How the app maps onto Ordo

- **A team is a tenant.** The owner is the tenant's `owner` role. A member
  holds the `editor` role. The page shows `editor` as `member`.
- **People's emails.** Pass the person's address as `display` when the
  emailed-code sign-in completes. It becomes their `displayName`, which the
  page shows as their email.
- **Invitations and removal** are the platform's own ops: `std.space.invite`
  and `std.space.remove`.

### The API the hidden checks call

The checks call the door directly, in a team's tenant (`X-Ordo-Space`). Your
program must declare these ops, with these names, parameters and columns.
More columns are fine.

| Op | Params | Answers |
|---|---|---|
| `listTasks` | none | rows `id`, `rev`, `title`, `done`, `assignee` (a member id or null) |
| `addTask` | `$title` and nothing else required | creates one task |
| `updateTask` | `$id`, `$rev`, `$title`, `$done`, `$assignee` (null for nobody) | changes one task |
| `deleteTask` | `$id` | removes one task |
| `listMembers` | none | rows `id`, `displayName`, `role` |

The checks also call `std.space.invite`, `std.space.remove`,
`std.space.setRole` and `std.space.admit` as each kind of person, and sign
people in through `/auth/begin` and `/auth/complete` with `display` set to
the email.

To join, the checks sign the invited person in with the invitation:
`space` and `invitation` (the ticket `std.space.invite` answered) on both
`/auth/begin` and `/auth/complete`, as the SDK's `beginEmailSignIn` and
`completeEmailSignIn` take them.
