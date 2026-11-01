# The plain client, the wire, refusals

Use this outside React (a script, a test, a worker), or where a hook does
not fit. In React prefer `react.md`.

## The generated functions

`ordo gen ts <dir> --app <src>` writes `<src>/generated/`: one function
per op, with its types, importing `@hominemai/ordo-sdk` by package name.
Regenerate after every definition change; never edit it, never open
`generated/ops.ts`. Look ops up with `ordo ops <dir> <name>` (a card with
ready-to-paste calls) or `<src>/generated/OPS.md`.

```ts
import { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';
import { OrdoRefusal } from '@hominemai/ordo-sdk/core/refusal.js';
import { addNote, projectNotes, renameProject, setLead } from './generated/ops.js';
import { auth } from './auth'; // createOrdoAuth, signin.md

const client = new OrdoClient({ ...auth.clientOptions(), space });

const page = await projectNotes(client, { project }).fetch();      // page.rows, page.hasMore
const all = await projectNotes(client, { project }).rows();         // every page
const some = await projectNotes(client, { project })
  .where((f) => [f.pinned.eq(true)])
  .pick('id', 'body')
  .fetch();

await addNote(client, { project, body, today: '2026-10-30' }).commit();

try {
  await renameProject(client, { id: p.id, rev: p.rev, name: newName }).commit();
} catch (e) {
  if (e instanceof OrdoRefusal && e.class === 'cas_miss') {
    // someone changed it first: reload, show the new state
  } else throw e;
}

// An op that converges a row and fills an optional field: the key is required.
await setLead(client, { id: p.id, lead: null }).commit();   // clears it
await setLead(client, { id: p.id, lead: member }).commit(); // sets or keeps it
```

- Each function is `opName(client, params)`. A read answers a `ReadCall`
  (`.fetch()`, `.pages()`, `.rows()`, `.where()`, `.pick()`); a one-row
  read a `SingleReadCall` (`.fetch()`, `.row()`); a write a `WriteCall`
  (`.commit()`). An op with no params takes no second argument.
- Types per op, named after it in PascalCase: `<Op>Row`, `<Op>Params`,
  `<Op>Fields`, `<Op>Refusal`. Ids: `<Entity>Id` in `generated/ids.ts`.
- Ids are branded per entity. A row's ref column already has the right
  brand. A plain string needs `asId<'projects'>(s)` from
  `@hominemai/ordo-sdk/core/brand.js`, or a cast.
- `rev` is an ordinary param of a CAS op here. Left out, the call is
  refused before the wire (`missing_param`).
- An optional param: leave it out, or pass `null`, to send its default.
  The door reads `null` as left out.
- **Clearing an optional field.** On an op that converges a row (an edit),
  every field in its braces is written, so a param that fills an optional
  field is required in the types: pass the current value to keep it, or
  `null` to clear it (when its default is `null`, the usual `= null`). On a
  create the same param stays optional. `ordo ops <dir> <op>` says, per
  param, how to clear.
- Money and decimals travel as strings. Dates as `YYYY-MM-DD`. Datetimes
  as ISO strings.
- `server_busy` and `serialization_conflict` are retried by the client
  already. `token` may be a function, so it can renew (`signin.md`).

## Calling a function

```ts
import { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';
import { auth } from './auth';

const client = new OrdoClient({ ...auth.clientOptions(), space });
const { result } = await client.callFunction<{ title: string }>(
  'suggestTitle',
  { text: body },
);
```

`result` is the JSON the function returned. A failure is an `OrdoRefusal`
whose class is `function_error` (the code threw; the message is its own),
`function_timeout`, `function_oom`, `function_busy` (retry later),
`egress_refused` or `space_paused`. Wire: `POST /fn/<name>` with the
parameters as a JSON body. In React use `useFunction(name)`.

A file goes in by handle, never in the body. Upload it to the row's `blob`
field first (`uploadFile`, `useFileUpload`), read the row back for the
handle, then pass it: `client.callFunction('extractDocument', { file:
row.file })`. A file the caller may not read is `not_found`. A body over
`ORDO_FN_BODY_BYTES` (1 MiB) is `bound_violation`.

## The wire

Read: `GET /op/<name>?param=value`. Write: `POST /op/<name>` with a JSON
body. Delete-only op: `DELETE`. Every call sends
`Authorization: Bearer <token>` and `X-Ordo-Space: <tenant id>`. Success is
`{ "rows": [...] }`, with `next` when another page exists; send it back as
the `X-Ordo-Page` header. A refusal is
`{ "error": { "code", "message", "op"?, "param"? } }` with a 4xx or 5xx.
Platform ops (`std.space.*`) use the same shape.

## Raw HTTP

Every sign-in, renewal, sign-out and platform op has an SDK call. Raw
`fetch` is for a tool with no SDK at all (curl, another language); the
wire above is all it needs. Sign-in routes: `POST /auth/begin`,
`/auth/complete` (takes `display`), `/auth/refresh` (bearer, or the
renewal cookie), `/auth/logout`.

## Refusals

Every failure is an `OrdoRefusal` (from `@hominemai/ordo-sdk/core/refusal.js`,
also re-exported by `@hominemai/ordo-react`). Read `e.class`; show
`e.refusal.message` (the door's words; `e.message` adds the class in
front). Hooks give the same detail as `refusal` (`.class`, `.message`,
`.param`). A call that got no answer at all is an
`OrdoNetworkError` (`kind`: `timeout`, `unreachable`, `blocked`).

| Class | Means | Page does |
|---|---|---|
| `access_denied` | the door refused the caller: not a member, removed, op not allowed for the role, or no fresh touch | show the message; drop the tenant if it is gone |
| `not_found` | absent, or guarded from you | reload |
| `cas_miss` | someone changed the row first | reload, show the new state |
| `access_guard` | you may read the row but not write it | show the message |
| `check_violation`, `bound_violation`, `invalid_param`, `missing_param` | bad input | show the message by its `param` |
| `duplicate` | a `unique` value is taken | show the message |
| `server_busy`, `serialization_conflict`, `tenant_not_ready` | transient | the client already retried; show the message |

`e.disposition` sums it up: `conflict`, `restart`, `retry`, `terminal`,
`request`, `denied` or `fault`.
