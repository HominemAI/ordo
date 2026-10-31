---
name: ordo
license: Apache-2.0
description: Build or change an app on Ordo. Use when writing or editing `.ordo` definition files (entities, ops, guards, roles), running `ordo check`, `ordo dev`, `ordo ops` or `ordo gen ts`, or writing page code that calls an Ordo door through `@hominemai/ordo-react` or `@hominemai/ordo-sdk` (loading rows, writing, forms, sign-in, staying signed in, tenants, invitations).
---

# Ordo

Ordo turns one folder of `.ordo` files into a multi-tenant backend: the
database, the HTTP door, roles, sign-in and a typed client. You write the
definition. Everything else is derived from it.

This skill is the whole of what an app needs. Do not open the SDK's `.d.ts`
files, its compiled `dist`, `generated/ops.ts` or `interface.json`: every
signature an app touches is in `sdk-signatures.md`, and every op of your app
is one `ordo ops` call away.

## The loop

1. Write or edit the definition (`*.ordo`). Language: `language.md`.
2. `ordo check <dir>`. It prints what it derived and exits 1 on any error.
   Each error names the rule and the fix. Fix, run again.
3. `ordo gen ts <dir> --app <src>`. It writes `<src>/generated/*.ts` (one
   function per op), `<src>/interface.json` (for the React hooks) and
   `<src>/generated/OPS.md`. Run it after every definition change.
4. Look ops up, never guess them:
   - `ordo ops <dir>`: every op, one line each.
   - `ordo ops <dir> <exactName>`: the op's card, with ready-to-paste calls.
   - `ordo ops <dir> <text>`: ops whose name contains the text.
   - Or read `<src>/generated/OPS.md`, which says the same.
5. Call ops from the page (below). `ordo dev <dir>` serves the door on
   `127.0.0.1:4000` with its own Postgres and recompiles on save.

```ordo
domain hello @ 0.1.0

entity notes {
  author -> members
  body:   text
  posted: date
}

op addNote($body: text, $today: date) =
  set notes { author: $caller, body: $body, posted: $today }

op recentNotes unbounded =
  notes[id, rev, body, posted, author.displayName]
  sort -posted
```

## What to reach for first

| Need | Reach for | Read |
|---|---|---|
| show rows | `useOpQuery('op', { params })` | `react.md` |
| write, with conflicts handled | `useCommit('op')` | `react.md` |
| a form for an op | `useOpForm('op')` + `useCommit('op')` | `react.md` |
| sign in, stay signed in, renew, sign out | `createOrdoAuth({ baseUrl })`, in React `useOrdoAuth(auth)` | `signin.md` |
| wait for a new person's tenant | `<TenantGate>` | `signin.md` |
| invite, roles, remove a member | `useCommit('std.space.invite')` after `withPlatformOps` | `react.md`, `guards-and-roles.md` |
| clear an optional field | `null` for its param; the current value keeps it | `client.md` |
| no React (a script, a test, a worker) | the generated functions over `OrdoClient` | `client.md` |
| raw HTTP | only from a tool with no SDK | `client.md` |

Order of preference: `@hominemai/ordo-react` hooks, then the plain client,
then raw `fetch`. Do not hand-write a `useState` + `useEffect` loader, a
sign-in token store or a `fetch('/op/...')` when a hook covers it.

## Rules that hold everywhere

- The grammar is closed. A plausible SQL or TypeScript variant fails to
  parse. If a feature is not in these files, assume it does not exist.
- Isolation is the tenant. Never add an `organisations` entity and a foreign
  key on every row: each customer is a tenant (`language.md`).
- `today` and `now` are not values in ops. The page sends the date.
- Every refusal has a class (`e.class`), and the class says what the page
  does (`client.md`, "Refusals").
- Settings are environment variables named `ORDO_*`. A refused call names
  the setting that would allow it. The app developer is the deployment
  owner and decides them.

## The reference files

Open one only when the task needs it.

| File | Open when |
|---|---|
| `language.md` | writing entities, reads or writes |
| `traps.md` | `ordo check` refused something, or before a tricky op (joins, counts, nulls, upserts) |
| `guards-and-roles.md` | some members must not see some rows; roles; invitations; who gets a tenant |
| `react.md` | page code in React: loading, writing, forms, platform ops |
| `signin.md` | sign-in, staying signed in, renewing, signing out, tenants, invitations |
| `client.md` | calling ops without React, the wire format, refusals, raw HTTP |
| `sdk-signatures.md` | the exact import path, signature or field of any export |

`docs/ai-guide.md` is these files joined into one, for a reader who wants a
single file. It is generated: edit the files here, never the guide.
