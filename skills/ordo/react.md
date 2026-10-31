# React: `@hominemai/ordo-react`

Headless hooks over the app's `interface.json`. They load, write, build
forms and keep state; they render nothing (except `TenantGate`). Reach for
them before any hand-written loader or `fetch`. Everything is imported from
`@hominemai/ordo-react` itself. Exact signatures: `sdk-signatures.md`.

## Setup: one client, one provider

`ordo gen ts <dir> --app <src>` writes `<src>/interface.json`. Import it
(`resolveJsonModule` on) and hand it to `parseInterface`. Do not read it:
look ops up with `ordo ops` or `<src>/generated/OPS.md`.

```tsx
import { useMemo, type ReactNode } from 'react';
import { DescriptorClient, OrdoProvider, parseInterface, withPlatformOps } from '@hominemai/ordo-react';
import doc from './interface.json';
import { auth } from './auth'; // createOrdoAuth, signin.md

// Name every platform op the app calls (see "Platform ops" below).
const iface = withPlatformOps(parseInterface(doc), 'std.space.invite', 'std.space.invitations', 'std.space.setRole');

// Asked once when a platform write is refused for want of a fresh touch.
const touch = async () => {
  await auth.stepUp();
};

export function Workspace({ space, children }: { space: string; children: ReactNode }) {
  const client = useMemo(() => new DescriptorClient({ ...auth.clientOptions(), space, iface }), [space]);
  return <OrdoProvider client={client} stepUp={touch}>{children}</OrdoProvider>;
}
```

- `auth.clientOptions()` is `{ baseUrl, token, renewToken }`. `token` is a
  function, so every call gets a live, renewed token (`signin.md`).
- Another tenant: `client.withSpace(otherSpace)`.
- `live` on `OrdoProvider` (`live` or `live={{ paceMs }}`) makes every read
  hook refetch when the door says its rows changed. Off by default: each
  live hook holds a connection open.
- Leave `stepUp` out when the app has no passkeys.

## Loading rows: `useOpQuery`

```tsx
import { useOpQuery } from '@hominemai/ordo-react';
import type { ProjectNotesRow } from './generated/ops.js';

export function Notes({ project }: { project: string }) {
  const notes = useOpQuery<ProjectNotesRow>('projectNotes', { params: { project } });
  if (notes.loading) return <p role="status">Loading…</p>;
  if (notes.refusal) return <p role="alert">{notes.refusal.message}</p>;
  return (
    <ul>
      {notes.rows.map((n) => (
        <li key={n.id}>{n.body}</li>
      ))}
    </ul>
  );
}
```

- Answers `{ rows, row, loading, refusal, revoked, refetch, retrying, ... }`.
  `row` is `rows[0]`, for a one-row read.
- Params are compared by content: `{ params: { project } }` inline is fine.
- `enabled: false` skips the call until its params are ready.
- `refine: (f) => [f.posted.gt('2026-01-01')]` and `pick: ['body']` narrow
  the rows at the door, over the op's own output columns.
- The first page only (`useOpPages` pages on). A listing that may pass one
  page uses `useOpPages`.
- The row type `<Op>Row` comes from `./generated/ops.js` (names in OPS.md).
  Without it the rows are `Record<string, unknown>`.

## Writing: `useCommit`

```tsx
import { useCommit, useOpQuery } from '@hominemai/ordo-react';

export function AddNote({ project }: { project: string }) {
  const notes = useOpQuery('projectNotes', { params: { project } });
  const add = useCommit('addNote');

  async function save(body: string) {
    const today = new Date().toLocaleDateString('en-CA'); // YYYY-MM-DD, local
    const s = await add.commit({ project, body, today });
    if (s.status === 'committed') notes.refetch();
  }
  // ...render a form calling save(); show add.refusal?.message; disable on add.committing
}
```

- `commit(values, rev?)` answers the new `CommitState` and never throws for
  a refusal. `state.status` is `idle`, `committing`, `committed`
  (`state.response.rows`), `conflict` or `refused` (`state.refusal`).
- A write does not refresh any `useOpQuery`. Call its `refetch()` after
  `committed`, or turn `live` on.
- **CAS.** An op whose braces hold `id` and `rev` declares `rev`. Pass the
  `rev` you read as the second argument, never inside `values`:

```tsx
import { useCommit } from '@hominemai/ordo-react';

const rename = useCommit('renameProject');
const s = await rename.commit({ id: p.id, name: newName }, p.rev);
if (s.status === 'conflict') {
  const current = await rename.resolve(); // the row as stored now, or null if it was deleted
  // show `current` to the person; to keep their edit:
  if (current) await rename.retry({ id: p.id, name: newName }, current.rev as number);
}
```

- `useCommit(name, { autoResolve: true })` fetches the current row as soon
  as a conflict arrives (`state.current`, `state.currentIs`).
- **Clearing an optional field** (an optional ref, a `date?`): on an op
  that converges a row, send `null` to clear it and the current value to
  keep it. Leaving it out writes the param's default, which clears too.
  `ordo ops <dir> <op>` prints which params do this.
- A CAS op committed without `rev` is refused before the wire
  (`missing_param`). There is no last-write-wins path.

## Forms: `useOpForm`

The form's fields come from the op's params: names, types, required,
enum options, reference pickers. `rev` is hidden and owned by the runtime.

```tsx
import { useOpForm, useCommit } from '@hominemai/ordo-react';
import type { FormEvent } from 'react';

export function NewNote({ project, onDone }: { project: string; onDone: () => void }) {
  const form = useOpForm('addNote', { project });
  const add = useCommit('addNote');

  async function submit(e: FormEvent) {
    e.preventDefault();
    const s = await add.commit(form.params);
    if (s.status === 'committed') {
      form.reset();
      onDone();
    }
  }

  return (
    <form onSubmit={submit}>
      {form.visible
        .filter((f) => f.name !== 'project')
        .map((f) => (
          <p key={f.name}>
            <label htmlFor={f.name}>{f.name}</label>
            <input id={f.name} name={f.name} required={f.required}
              type={f.type === 'date' ? 'date' : 'text'}
              value={String(form.values[f.name] ?? '')}
              onChange={(e) => form.setValue(f.name, e.target.value || undefined)} />
            {form.issueFor(f.name) && <span role="alert">{form.issueFor(f.name)?.message}</span>}
          </p>
        ))}
      <button type="submit" disabled={!form.valid || add.committing}>Add note</button>
      {add.refusal && <p role="alert">{add.refusal.message}</p>}
    </form>
  );
}
```

- Set values in their wire type: `int` a number, `bool` a boolean, `money`
  and `decimal` a string, `date` `"YYYY-MM-DD"`, `id` the typed id string.
  `issues` uses the door's own classes and messages.
- Set an emptied field to `undefined`, not `""`: `form.params` drops only
  `undefined` and `null`.
- `f.kind`: `scalar`, `enum` (`f.options`), `reference`
  (`await form.searchCandidates(f.name)` gives rows for a picker), `json`,
  `rev` (hidden).
- For a CAS form: `add.commit(form.params, row.rev)`.

## Platform ops

`std.space.*` ops are not in `interface.json`. Name the ones the app calls
in `withPlatformOps(iface, ...names)`; then `useOpQuery` and `useCommit`
call them like program ops, typed by name.

```tsx
import { useCommit, useOpQuery } from '@hominemai/ordo-react';

const invite = useCommit('std.space.invite');
const s = await invite.commit({ role: 'editor', expires: '7d' });
if (s.status === 'committed') {
  const { ticket } = s.response.rows[0];
  showLink(`${location.origin}/#/join/${space}/${ticket}`);
}

const invitations = useOpQuery('std.space.invitations'); // rows: id, role, label, status, expiresAt, ...
const setRole = useCommit('std.space.setRole');          // commit({ member, role })
const members = useOpQuery('std.space.members');         // rows: member, principal, name, role, status, joined, groups
const remove = useCommit('std.space.remove');            // commit({ member })
const leave = useCommit('std.space.leave');              // commit({}): the caller's own seat
```

Names `withPlatformOps` takes, all `std.space.`: `share`, `shareLink`,
`setLinkPassword`, `setLinkMaxOpens`, `revokeShare`, `revokeLink`, `links`,
`shares`, `invite`, `invitations`, `revokeInvitation`, `setRole`,
`members`, `admit`, `suspend`, `reinstate`, `remove`, `leave`, `profile`,
`setOpAccess`, `resetOpAccess`, `setMemberAccess`, `createGroup`,
`addToGroup`, `removeFromGroup`, `deleteGroup`, `groups`. And
`std.delegation.review`, `approve`, `decline`, `mine`, `revoke`.

- `std.space.members` answers at most 500 rows in member id order. For
  more, pass the last `member` as `params: { after }`. A program read over
  `members[id, displayName, role]` is often simpler.

## Other hooks

| Hook | For |
|---|---|
| `useOpPages(op, { params })` | a read paged on: `rows`, `hasMore`, `more()`, `exhaust()`, `loading`, `refusal` |
| `useOrdoAuth(auth)` | sign-in state over `createOrdoAuth`: `{ phase, session, retry, auth }` (`signin.md`) |
| `useTenantStatus`, `<TenantGate>` | a new person's tenant (`signin.md`) |
| `useRow(id)` | the ops that act on a typed id, from the id alone |
| `useFileUpload`, `useFileDownload`, `useFile` | file fields |
| `useOwnDownload` | the person's own data as a zip |
| `useAccountEmail` | the person's confirmed email address |
