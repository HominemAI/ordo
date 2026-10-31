# Notes

The first program deployed on Ordo. A person signs up with a passkey, gets a
tenant of their own, writes notes, tags them with words, and can share one
note with another person to read. The page also offers the other ways in a
deployment can serve: an emailed code, an outside connection such as Discord,
and a guest account. Passkey is the default, and the others appear only when
the app developer turns them on (§6).

The plan is `docs/plan-first-program.md`, and its "Built" section says what is
proved and how. This file says how to run the app, what it does not do, what
the platform still owes it, and what the deployment owner sets to put it live.

| Part | Where |
|---|---|
| The program, the whole server side | [`notes.ordo`](notes.ordo) |
| The page: React on the building blocks of `@hominemai/ordo-react`, over `@hominemai/ordo-sdk`, built with Vite | [`web/`](web) |
| The local deployment, one command | [`dev/serve.sh`](dev/serve.sh) |
| The proofs that need a server | `crates/ordo-host/tests/notes_app.rs` |
| The proofs that need none | `web/test/` (`npm test`): the commit path, the rendered editor, the session, the link route, the ways in |

## 1 · The program

```
ordo check examples/notes/notes.ordo
```

One entity, `notes`, with `acl (read, write, admin)`. That derives the link
entity `notesAccess`, the guards on `notes`, `grantNotesAccess`,
`revokeNotesAccess`, and the creator's `admin` row in the same command group as
the create. The app's ops:

| Op | What it is |
|---|---|
| `createNote($title, $body, $tags, $created)` | write, `creates`. The page stamps `$created`. `$tags` is a `json` array of words |
| `editNote($id, $rev, $title, $body, $tags)` | write, `converges`: compare-and-set on `rev` |
| `deleteNote($id)` | write, `deletes`: a plain `del`, which takes the note's access rows with it (see §7) |
| `unshareNote($note, $member)` | takes one note back from one guest |
| `myNotes` | read, `sql`, unbounded, pageable, syncable (`sync: notes.id`), newest first. It returns each note's tags |
| `notesTagged($tag)` | read, `sql`, unbounded: the notes whose `tags` hold the word `$tag` |

Each note has `tags: json = []`, a JSON array such as `["work", "ideas"]`.
`notesTagged` filters with `tags contains $tag`. On a `json` field, `contains`
asks whether the array has that element, and the database does the matching
inside the document. A text field would only match a substring. The page has a
Tags box on a new note and in the editor, and a chip for each tag in the list.
A click on a chip shows `notesTagged` for that word, and **Show all** goes back.

There is no viewer row and no editor row. A tenant has one member, its person,
with role `owner`. Anyone else who reads a note is a guest: a seat with no role
holding one `read` row in `notesAccess`.

### The five constructs the plan could not confirm

Each was tried on 2026-09-29, with `ordo check` and then against the running
server.

| Construct | Result | What the app uses |
|---|---|---|
| `title: text(200)` as a field bound | parse error, `expected a field name, found (`. The length form is for parameters | `title: text` |
| `title contains $q` | compiles, as a `sql` read. Not run on text | not used for text: the device filters what it holds. On a `json` array `contains` is used, by `notesTagged` |
| `notes(id == $id)[id, rev, title, body]` | compiles, `rows <= 1`. Not run | not used: the list already holds every field |
| `std.notesAccess.list` | derived, compiles, and answers: the owner sees the `admin` row and each guest's `read` row | the page lists who can read a note, and its member ids are the seats |
| `created: datetime = now` as a shape default | passed `ordo check` and was refused when the DDL was lowered. Since 2026-09-29 `ordo check` refuses it (`unsupported_default`): `now` is in the language's default set and is not built | a `created: datetime` field with no default, and `createNote` takes it as a `datetime` parameter. The door binds `YYYY-MM-DDTHH:MM:SSZ`. It answers `2026-09-29 10:00:00+00` |

Ordering needed a stamp: the ids the platform minted did not sort by age past
nine (`note_…_9` sorted after `note_…_10`). Since 2026-10-06 a minted id
starts with the time it was minted, so ids sort by age (25e). The app still
orders by `created`, the moment the person wrote the note.

## 2 · Try it on this machine

You need PostgreSQL 15 or later, Node 18 or later, and a browser with
passkeys: Safari and Chrome on a Mac, Chrome or Edge on Windows or Android. In
Chrome without a hardware authenticator, DevTools, WebAuthn, "Enable virtual
authenticator environment" gives you one.

```
cargo build -p ordo-cli -p ordo-host
sh examples/notes/dev/serve.sh                      # terminal 1: the door
cd examples/notes/web && npm install && npm run dev # terminal 2: the page
```

`serve.sh` starts a throwaway Postgres under `dev/.data`, boots `ordo-host` on
`127.0.0.1:4000` with the settings in §6, applies `notes.ordo` and stays in the
foreground. Set `ORDO_PGBIN` if your Postgres is not at
`/opt/homebrew/opt/postgresql@17/bin`. Ctrl-C stops the door and the database.

Open <http://localhost:5173>. The address matters: the relying party is
`localhost`, and the door accepts the ceremony and the calls from this exact
origin. Another port is another origin.

### Signing up, and signing in (N3)

1. Type a name and press **Sign up with a passkey**. The browser asks you to
   create a passkey. You land on an empty list. That is a tenant of your own,
   made while the request ran.
2. Write a note. It appears in the list.
3. Open a second browser profile (or a private window) at the same address and
   sign up as someone else. That person has a different tenant and an empty
   list. Write a note there. Neither of you sees the other's.
4. Press **Sign out**. The door ends the session, so the token is dead
   wherever it went, and the device forgets your notes.
5. Press **Sign in with a passkey**. You land in the same tenant, and your
   notes are back.

Those five steps use a passkey. The page can offer more ways in (see "The
other ways in" below).

A reload keeps you signed in. The token lives only in the page's memory, never
in `localStorage`, because a token stored where the page can read it back is a
token any script on the page can take. Each sign-in also sets a renewal cookie
the page cannot read. On load the page sends it to `POST /auth/refresh` and
gets a new token (`Session.resume`). Sign out, and a reload shows the sign-in
screen. The cookie is sent only when the page and the door are on one site, so
the page reaches the door as `http://localhost:4000`, never `127.0.0.1`: to a
browser those are two sites. `ORDO_REMEMBER=off` turns this off.
While the page stays open, the token is renewed a minute before it ends
(`ORDO_SESSION_SECS`, an hour by default), and the session can be renewed for
up to `ORDO_SESSION_MAX_SECS` (twelve hours).

**Under the door**: `POST /auth/register`, then `/auth/complete`, which
answers `token` and `space`, the person's own tenant. `GET /auth/spaces` lists
the tenants they belong to. The door that made the tenant serves it before it
answers, so the first call is served (25e, 2026-10-06). Another replica learns
of it some moments later, so with more than one replica the first call can
still be refused. The page waits up to six seconds before it believes a
refusal (`untilServed`).

### The other ways in

The door lists its own sign-in methods at `GET /auth/methods`, and the page
asks once at start. It shows only the ways the deployment serves: passkey,
emailed code, guest, and each outside connection by name and label. Nothing in
the build has to match the deployment's settings. The route carries no secret,
no client id and no address inside a provider. The table in §6 pairs each way
in with its settings.

The build variable `VITE_ORDO_SIGN_IN` is kept as a fallback only. If the door
cannot answer that route (it is down, or an older door without it), the page
reads the variable: a comma list in the words of `ORDO_AUTH_METHODS` (`passkey`,
`email`, `guest` and the name of each outside connection). Unset, the fallback
is the passkey alone. `VITE_TURNSTILE_SITE_KEY` is a fallback in the same way.

- **Passkey** stays first and is the default.
- **Emailed code.** The person types an address, then the code. The page keeps
  the device marker in `localStorage`.
- **Outside connection.** A button reads "Continue with Discord", or the like.
  The page keeps the binder in `sessionStorage` while the person is away. On
  return it redeems `?handoff=` and shows `?error=` if the provider failed.
- **Guest.** "Try it as a guest" makes an account at once. The page keeps the
  guest secret in `localStorage`, and offers "Continue as your guest account"
  while one is kept.
- **Human check.** When the door reports a Turnstile check, the page shows the
  widget with the public site key the door gives (`ORDO_AUTH_HUMAN_CHECK_SITE_KEY`)
  and sends its token as `human` on each first leg.

A person signed in without a passkey sees an **Add a passkey** card. Sharing a
note needs a passkey touch, which is why the card exists. It links the passkey
to the account by claim. A guest uses its secret as the first proof, so there is
nothing to type, and the page forgets the secret once the passkey is added. An
emailed-code account is sent a new code. An account that came in through an
outside connection makes one more trip to the provider. The first press of
**Continue with Discord** (or the like) leaves the page. The provider sends the
person back with a code, and a second press adds the passkey.

The claim needs the session token as a bearer and the provider's code as its
first proof. A code is good once, so the page cannot sign in with it and then
use it again. For this one trip only, the page keeps the session in
`sessionStorage` (this tab, never a URL) next to the binder, and forgets it the
moment the person is back. It is written only when the person presses the
button. Every other way in keeps the token in memory alone. If the provider
sends back an error, the person is still signed in and sees the error.

The page test is `test/signin.test.ts`, "adding a passkey to an account that
came in through an outside connection". The fake Discord runs in the test
process and cannot be reached from the `ordo-host` binary, so `notes_app.rs`
does not run this trip. The door side is proved in `tests/auth_linking.rs`.

A deployment that requires a second step gets a clear message. This page does
not offer one. The page also words these refusals: `guests_off`, `linking_off`,
`identifier_taken`, `space_paused`, `quota_exceeded`, `space_unavailable` and
`bound_violation`.

Proved by `a_person_signs_in_with_an_emailed_code_and_a_later_code_finds_the_same_notes`
and `a_guest_adds_a_passkey_and_the_passkey_signs_in_to_the_same_notes` (§9).

### Editing in two tabs (N2)

Open a note in two tabs. Change the title in both and save in the first, then
in the second. The second says **Changed elsewhere**, shows what is stored now,
and offers **Keep my text** (saves again at the stored revision) or **Use
theirs**. Nothing was overwritten. The write asserts the `rev` the tab opened.

### Trying the page against `ordo dev` (N2)

`ordo dev examples/notes` reads `ORDO_CORS_ORIGINS` and the sign-in settings
(`ORDO_AUTH_METHODS` and its siblings) when they are set, as a server does
(`deploying.md` §1b, `howto-sign-in.md` §2). Unset, it has no sign-in and no
CORS headers, and prints a token. It is one space, `dev`, with no self-serve
provisioning: a person who signs up there gets an identity and a token, and no
tenant of their own, so the sign-up screen cannot finish against it. To try
notes alone, with a development token:

```
ordo dev examples/notes                              # prints a token, serves :4000
cd examples/notes/web
ORDO_DEV_DOOR=http://127.0.0.1:4000 VITE_ORDO_URL=http://localhost:5173 npm run dev
```

Vite then forwards `/op` and `/auth` from its own origin to the loop, which
needs no CORS setting because the page and the door share an origin. In the
page, open **Use a development token**, paste the token and the space `ordo
dev` printed (`dev`), and press the button. That form exists only under `npm
run dev`.

## 3 · Calls from another origin (N4)

The page is on one origin and the door on another, and every call carries
`Authorization` and `X-Ordo-Space`, so the browser preflights every one. The
door answers a preflight only for origins in `ORDO_CORS_ORIGINS`:

```
ORDO_CORS_ORIGINS=http://localhost:5173
```

An exact origin, no path, no trailing slash, and `*` refuses the boot. The
preflight for `/op/*`, `/auth/*` and the sync stream is answered `204` for a
listed origin and `403` for any other. With the variable unset the preflight
reaches the router and gets `405`, and a browser reports the block in its
console and nothing in the door's log. The page turns that into a message that
names `ORDO_CORS_ORIGINS` and `ORDO_AUTH_ORIGINS`. The passkey ceremony has its
own list, `ORDO_AUTH_ORIGINS`, so that a page may call the door without being
allowed to sign anyone in.

To see it: stop `serve.sh`, start it with `NOTES_DEV_PAGE=http://localhost:5199`,
and open the page on `:5173`. The sign-up button now shows that message.

Proved by `a_page_on_another_origin_reaches_the_door_and_an_unlisted_one_does_not`,
which preflights eight routes as the listed origin, refuses an unlisted one,
and boots the door a second time with the variable unset.

## 4 · Notes kept on the device (N5)

`myNotes` is syncable, so the SDK's sync manager keeps its rows in an
IndexedDB replica: one per tenant and per person, sealed with a key the
browser holds. The list on screen is drawn from the replica, so it shows at
once and it shows with the network off. The page hands the device to
`OrdoProvider` as its `device` option (the SDK's `openDevice`), and
`useOpQuery('myNotes')` reads it.

Try it: sign in and let the list load, then DevTools, Network, Offline. The
list stays, every note opens, and **Add note**, **Save**, **Delete** and
**Share** say they need the network. Come back online and they work again.

**The page has to stay open.** A reload while offline shows the sign-in screen,
because the page keeps no token and cannot open a person's replica without
knowing who they are, and resuming the sign-in needs the network. The notes are still on
the device and are read again after the next sign-in. Opening the replica at
start with no sign-in would need the tenant and person kept in the clear, and
keys under a passkey (checklist `22m`). Neither is built.

What is proved (`web/test/live.test.ts`, run by
`the_page_keeps_notes_on_the_device_and_erases_a_closed_share` against the real
binary): the notes reach the device, a reopened replica reads them with the
network off, a raw dump of the database holds no title, body, id, tenant or
person, a write with no network is refused and changes nothing, and signing out
leaves no record and no key.

What it does not do:

- **No writes while offline.** No queue, no optimistic apply, no rollback. The
  text stays in the editor.
- **The seal is not a lock.** The key sits in the same IndexedDB as the rows.
  It keeps a copied database unreadable. It does nothing against a script
  running on the page's origin. Keys under a passkey, a lock when idle and a
  retention policy are owed (checklist `22m`).
- **The server does not push to a tenant made after it booted.** The sync feed
  learns of a change from a Postgres notification, and nothing starts
  listening on a new tenant's database until the server next boots
  (`PgInvalidationBus::refresh` has no caller). Observed: a note written after
  the feed opened does not arrive in ten seconds. So the page opens the feed
  again after each write, when it comes to the front, when the network returns
  and every half minute (`keepFresh`). That is polling with the SDK's own
  manager, and it goes when the server subscribes to a tenant at provisioning.
- **Search is a filter on what the device holds**, a substring on title and
  body, done in the page. The Tier-0 operators have no `contains`.

## 5 · Sharing a note (N8)

1. Sign up two people, Ada and Bo, in two browser profiles.
2. Ada opens a note, presses **Share**, then **Make an invitation for a person**. The page shows a
   link like `http://localhost:5173/#/accept/<ticket>`. Ada sends it to Bo. The
   ticket is in the fragment, so no server ever receives it. It is a secret:
   whoever opens it first gets to read the note.
3. Bo opens the link in his profile. The page says who shared what, what
   agreeing tells Ada (the name Bo types), and that the tenant's owner can
   reach anything in it. Bo signs in or signs up, types the name to show, and
   presses **Agree and read it**.
4. Under **Shared with me**, Bo reads the note. There is no edit and no delete.
   The door refuses every write through the window, and Bo's own session is no
   way in.
5. Ada shares a second note with Bo. Bo agrees again to that note, and it
   appears under the same sharer with the same seat.
6. Ada presses **Stop sharing this note** by Bo's row. The note leaves Bo's
   list at his next load.
7. Ada presses **Remove from all my notes**. The window is closed: Bo's next
   call is refused, no new window can be opened, and what the window held is
   erased from his device when his page next reloads the feed (within half a
   minute).

The link in step 2 and the button in step 7 are platform ops, which ask for a
passkey touch no older than five minutes. The page asks for a touch once when
the door refuses, and tries again. **Stop sharing this note** is an op of the
program and asks for none.

Proved by `a_note_shared_with_another_person_is_read_only_and_can_be_taken_back`
(the server: share, peek, accept, window, refused writes in every shape, the
window refused in any other tenant, the same seat for a second share,
`std.notesAccess.list`, unshare, revoke, the guest list) and by
`the_page_keeps_notes_on_the_device_and_erases_a_closed_share` (the reader's
device: the shared note appears, leaves on unshare, and the whole window is
erased on revoke). Acceptance is recorded in the sharer's log and in the deployment's
security log, as ruled.

### Sharing with a link and no account

The share panel has two parts. **Share with a person** is the invitation
above: the reader signs in and becomes a guest. **Anyone with the link can
read** needs no account.

1. Ada presses **Make a link anyone can read**. The page shows
   `http://localhost:5173/#/link/<secret>` once, with the time it expires (7
   days). The door keeps only a hash, so the page cannot show it again.
2. Under **Links to this note**, Ada sees each link as live, expired or
   revoked, and how often it was opened.
3. Anyone who opens the link, in a private window with no session, reads that
   note and nothing else. There is no sign-in, no edit and no delete. The
   secret is in the fragment, so no server receives it in a request line.
4. Ada presses **Revoke link**. The next read is refused, and the page says the
   link has expired or was revoked. Only a live link has the button.

The page checks for `#/link/<secret>` before it checks for a session, so the
sign-in screen never stands in front of it. Making and revoking a link are
platform ops and ask for a fresh passkey touch, as above.

Proved by `web/test/link.test.tsx` (the panel's calls and the route, against a
stubbed door) and by the last-but-one case of
`the_page_keeps_notes_on_the_device_and_erases_a_closed_share` (the real
binary: mint, read with no session, refused after revoke).

### A password on a link (notes v2)

1. Before **Make a link anyone can read**, Ada may type a password. The
   page says the link asks for it. She sends the password by a different
   way than the link.
2. Under **Links to this note**, each live link says whether it has a
   password, and when a lock after wrong tries ends. **Set a password**,
   **Change the password** and **Remove the password** call
   `std.space.setLinkPassword`.
3. The reader opening the link sees a password form first. A wrong password
   says so. After too many (5 by default) the link is locked: the form says
   for how long and counts down. The right password opens the note.
4. The page keeps the password in memory only. The SDK sends it again each
   time it renews the link's token.

Proved by `web/test/link.test.tsx` and `web/test/share-panel.test.tsx`, and
end to end by `e2e/tests/v2/search-and-links.spec.ts`.

### Search (notes v2)

**Search** finds notes by title, text and tag, among the person's own notes
and the notes shared with them. Every word typed must appear. Case and
accents are ignored. The results change on each key.

It runs on the device copies: the person's own, and one per share
(`notes-v2/web/src/search.ts`). Why there and not in an op:

- A share's copy holds only the rows the sharer's row guard served to that
  window. So a search can reach nothing a share does not grant.
- It works offline, for what the device holds, and needs no call per key.
- Ordo has no op that reads across a person's tenant and their shares. A
  server search would be one call per tenant, and its `contains` is case
  sensitive and unindexed (§7, item 22).

Proved by `web/test/search.test.ts`, and end to end by
`e2e/tests/v2/search-and-links.spec.ts`: Bo finds his own note and the note
Ada shared, never the note Ada kept, online and offline.

## 6 · The settings

The same seven, locally and live. `serve.sh` sets the local column.

| Variable | Locally | Live | Why |
|---|---|---|---|
| `ORDO_AUTH_METHODS` | `passkey` | `passkey` | sign-in and sign-up |
| `ORDO_APEX_DOMAIN` | `localhost` | `inf.smg.icu` | the passkey relying party. Changing it strands every passkey, so it is set once |
| `ORDO_AUTH_ORIGINS` | `http://localhost:5173` | `https://notes.inf.smg.icu` | which page may run a ceremony. `http://` only on loopback |
| `ORDO_CORS_ORIGINS` | `http://localhost:5173` | `https://notes.inf.smg.icu` | which page may call the door |
| `ORDO_AUTH_PROVISIONING` | `auto-onboard` | `auto-onboard` | anyone can sign up |
| `ORDO_PROVISION_SELF_SERVE` | `on-signup` | `on-signup` | one person, one tenant |
| `ORDO_PROVISION_SIGNUPS_PER_HOUR` | not set (60) | `60` | the deployment-wide ceiling on new tenants |

Notes v2 keeps files on notes, so it needs a blob store. Locally `serve.sh`
sets `ORDO_BLOB_STORE=fs:<dir>` with `ORDO_DEV_UNSAFE_BLOB_STORE=1`, a folder
on disk that the door refuses without the unsafe switch. Live it is
`ORDO_BLOB_STORE=s3` with the `ORDO_BLOB_S3_*` settings.

Notes v2's reminders email the note's owner through the mail channel.
Locally `serve.sh` sends mail to a catcher on this machine
(`ORDO_MAIL_KIND=smtp`, `ORDO_MAIL_TLS=none`, port `NOTES_DEV_SMTP_PORT`,
4625 by default). Live it is the relay the emailed codes use, over TLS
(`howto-sign-in.md` §9). With no mail channel, reminders show in the page
only, and the log says who was not told.

A routine that `ordo apply` adds runs from the door's next start. After the
first apply of v2, restart the door once.

### The other ways in: settings

Each row is optional. The deployment settings go on the server. The page reads
them from `GET /auth/methods`, so its build needs nothing, except the return
address of an outside connection when it is not the page's own.

| Way in | Deployment settings | Page build |
|---|---|---|
| Emailed code | `ORDO_AUTH_METHODS=passkey,email`, `ORDO_MAIL_KIND`, `ORDO_MAIL_FROM`, and the mail channel's host and key file (`howto-sign-in.md` §9, §10) | nothing |
| Guest | `ORDO_AUTH_GUEST=on`, with the human check beside it (§13) | nothing |
| Outside connection | `ORDO_AUTH_METHODS=passkey,<name>`, `ORDO_PUBLIC_URL`, `ORDO_AUTH_RETURN_URLS`, and per name `ORDO_AUTH_<NAME>_PRESET`, `_CLIENT_ID`, `_CLIENT_SECRET_FILE` (Microsoft also `_TENANT`). Redirect URI: `<ORDO_PUBLIC_URL>/auth/callback/<name>` (§8) | `VITE_ORDO_RETURN_URL` if the return address is not the page's own; its origin must be in `ORDO_AUTH_RETURN_URLS` |
| Human check | `ORDO_AUTH_HUMAN_CHECK=turnstile`, `ORDO_AUTH_HUMAN_CHECK_SECRET_FILE`, `ORDO_AUTH_HUMAN_CHECK_SITE_KEY=<public site key>` | nothing |

Section numbers in the table are those of `docs/howto-sign-in.md`. Linking a
second sign-in to an account (`ORDO_AUTH_LINKING`) is on by default. To see
what the door tells the page, run `curl <door>/auth/methods`.

**No credentials directory is set.** Each person's tenant is a database of
its own, made on the deployment's own Postgres. The door dials it with the
label `$deployment`, "this deployment's own database password". The server
answers that label with the password it already holds, `ORDO_PG_PASSWORD` or
`ORDO_PG_PASSWORD_FILE` (`deploying.md` §4d). There is no routing setting.
A leftover `ORDO_PG_ROUTING=attached` warns at boot, and any other value
refuses it.

**Left at their defaults, on purpose.** Each is the deployment owner's
choice, and the default is safe for this app:

- `ORDO_ACCOUNT_REMOVAL`, default `never`: a person who deletes their account
  is kept, not removed. Set `<n>d` to remove them after `n` days.
- `ORDO_MEMBER_LEFT_NAME`, unset: a person who leaves keeps their name on the
  seats others see. Set a text such as `Former member` to replace it.
- `ORDO_SHARE_LINKS`, default `on`: §5's links need it on.
- `ORDO_SHARE_MAX_SECS`, unset: a share has no ceiling on its expiry.
- `ORDO_TENANT_PAUSE_ROLES`, unset: only the deployment owner pauses a tenant.
- `ORDO_REFINE_MAX_CONDITIONS` (16) and `ORDO_REFINE_MAX_IN_VALUES` (200): the
  page sends no refinement.
- `ORDO_PG_ACQUIRE_TIMEOUT_MS` and `ORDO_PG_POOL_SIZE`: a busy database
  answers `server_busy` (503, `Retry-After: 1`). The page shows it as an
  error and does not retry. Size Postgres `max_connections` for the tenants
  in use at once (`deploying.md` §4d).

## 7 · What the platform owes this app

Each of these is a workaround in the app, or a limit it lives with. None was
fixed when the app was built. Those marked fixed were closed afterwards
(`plan-first-program.md`, "Server gaps closed").

1. **Fixed (2026-09-29): the door refused the ids the platform mints.**
   `note_wf121e1ce63577793_1` has two underscores, and a key derived from a
   two-word name has a hyphen (`note-acce_…`). `is_typed_id` in
   `ordo-api/src/bind.rs` now takes both. The row-id parameters of
   `editNote`, `deleteNote` and `unshareNote` are `id` again, and the
   derived `std.*` write ops and `grantNotesAccess` can be called from a page.
2. **Fixed (2026-09-29): a row that another row references through `owned`
   could not be deleted.** Every note has its creator's `admin` row, so
   `del notes { id: $id }` answered `has_referrers`, and the app emptied the
   note and marked it `gone` instead. The owner ruled that a plain `del`
   deletes every row that references the target through an `owned` reference,
   the derived access rows included (`owned-delete`). `deleteNote` is now a
   `del`, and the `gone` field and the filter on it are gone. It takes the
   note, its access rows and every guest's share in one command group, so it
   can be reverted. A caller must be allowed to delete each row it takes, and
   the access rows need `admin`, so only a note's `admin` can delete it. The
   other half is fixed too: `subtree` on an entity with no `tree` ref is a
   compile error (`subtree_not_tree`), not an `internal` at run time.
3. **Fixed (2026-09-29): the server did not listen on a tenant made after it
   booted** (§4). The door adds a listener for the tenant's database when the
   tenant is created, and on each access refresh tick. The page still opens
   the feed again after each write, on focus, when the network returns and
   every half minute. That is now redundant and is left as a safety net.
4. **Fixed (2026-09-29): `ordo dev` had no sign-in and no CORS** (§2). It
   reads `ORDO_CORS_ORIGINS` and the passkey settings when they are set. It is
   one space, with no tenant credentials directory and no self-serve
   provisioning, so this app, which gives each person a tenant, still runs on
   `ordo-host` and `dev/serve.sh` is still needed.
5. **Fixed (2026-09-29): `$deployment` in the credentials directory.** The
   label answers with the deployment's own password, so the directory can be
   empty (§6, `deploying.md` §4d).
6. **`PasskeySession` has no `space` or `provisioning` field.** The page reads
   `session.raw.space`, where the door puts it.
7. **Fixed (2026-09-29): a `datetime = now` shape default passed `ordo check`
   and the DDL refused it** (§1). `ordo check` now refuses it
   (`unsupported_default`). `now` itself is not built, so the page still
   stamps `created`.
8. **The door answers `access_denied`, never `not_found`, for a tenant that
   does not exist and for one a person is not in**, and the two answers are
   byte-identical. The plan says `not_found`.
9. **Provisioning is not logged with its duration.** The plan's N3 looks for
   the time each provision took in the log. It is not there.
10. **Fixed (2026-10-06): the door served a new tenant 65 to 120 ms after the
    sign-up answered.** The replica that makes a tenant rebuilds its door
    before it answers (25e). `untilServed` stays for a deployment with more
    than one replica.
11. **Fixed (2026-10-06): minted ids did not sort by age** (§1, 25e).
12. **The share-link miss limit is off.** A link secret is 256 random bits.
    `ORDO_SHARE_LINK_MISS_LIMIT` works only in proxy mode, and there it slows
    an address that keeps missing; it never refuses (2026-10-11).

### What notes v2 met (2026-10-22)

Notes v2 (`examples/notes-v2`) adds pins, an archive, files on a note and a
download of everything. These are what made that hard for an app developer.

13. **The SDK and the hooks had no blob helpers.** Closed 2026-10-22 (owner
    ruling). `uploadFile`, `fileInfo`, `fileUrl` and `downloadFile` are in the
    SDK, and `useFileUpload`, `useFile`, `useFileInfo` and `useFileDownload`
    in `@hominemai/ordo-react` (`docs/howto-files.md`). The page makes no
    door call of its own.
14. **A blob's media type and size are not readable from an op**
    (`photo.mime` answers `not_a_ref`). Partly closed 2026-10-22: the door
    answers both at `GET /blob/{handle}/meta`, behind the row guard, and
    `fileInfo` reads it. An op still cannot, so the page keeps `kind` and
    `size` on the row, which a list of many files needs. Dropping them
    waits on the compiler reading `f.mime` and `f.size`.
15. **A share pins the columns of the ops it lists.** An op that answers a
    new column closes every share that lists it until the sharer shares
    again. So v2 keeps `myNotes` exactly as v1 had it, for the shares made
    under v1, and adds `allNotes` with the pin and archive state.
16. **A window guest could not open a file.** `std.$blob.get`, the file
    door's check, was carried only by a share link that lists a
    `std.<entity>.get`. Built 2026-10-22, held for the owner's review: the
    rule moves into `share_ops::validate`, so a share and its accept carry
    it too (`crates/ordo-host/tests/sharing.rs`). Either way a share must
    list `std.attachments.get`, an op the page never calls, to carry it.
17. **The device copy reads `access_denied` as "this share was closed".** It
    then holds nothing for the op. Two ordinary cases answer the same:
    - a page newer than the program the door serves. `ordo apply` answered
      before the door switched, so v2's page opened at once was refused
      `allNotes`. Built 2026-10-22, held for the owner's review: `ordo
      apply` now waits until every node serves the version and every
      tenant is brought to it, and `GET /health/deploy` says the same to a
      page (`docs/deploying.md` §7).
    - a guest reading an op their share does not list, such as a share made
      under v1. Fixed in the SDK: a guest feed now says which ops it holds
      (`GuestFeed.ops`), and the page reads only those.
18. **`std.space.export` does not fit a person's own download.** It writes a
    directory on the server's disk and answers its path. Nothing serves the
    bundle to a browser, it is a directory and not one file, and it holds
    the whole tenant (command log, schema, access profile). So the page
    built its own zip from `allNotes`, `myFiles` and the blob door. Built
    2026-10-22, held for the owner's review: a person's own download,
    `GET /download`. The app developer marks read ops `download`, and the
    door streams one zip of what this person may read. v2 marks `allNotes`
    and `myFiles`, and the page calls `useOwnDownload`
    (`docs/howto-leave.md`, "A person's own download").
19. **A deleted file's bytes stay up to 90 days.** Deleting a note or a
    file makes its blob collectable at once. The blob worker deletes the
    bytes only after the undo horizon passes, so a revert can bring them
    back.
20. **A create answers its new id as `{ entity, created }`.** `useCommit`
    has no typed way to read it, which a two-step create (row, then bytes)
    needs.
21. **Writes made offline are refused, not queued.** Pinning and archiving
    need the network. Showing pinned notes first and the archive filter
    work offline, from the device copy.
22. **No search on the server across a person's reach.** An op reads one
    tenant. A guest reads a sharer's tenant through one window each. So no
    single call searches a person's own notes and their shares. Text
    matching in an op (`contains`, `starts`, `matches`) is case sensitive
    and has no index, and full-text is refused by design
    (`sql-translation-corpus.md`, I5). The page searches its device copies
    instead, which the design names as the way. Nothing was built in Ordo.
23. **A share's copy cannot be opened offline.** A guest's device opens
    through a fresh window (`POST /auth/window`), and the list of shares
    comes from `listSpaces`. Both need the network. So offline, search
    covers shared notes only while the search view that opened them stays
    open. A page loaded offline searches the person's own notes.
24. **Wrong link passwords can lock the link for everyone, when the lock
    is on.** The lock is off by default (owner, 2026-10-22), so a wrong
    password is only refused. Where a tenant, the app developer or the
    deployment owner turns it on, the count is per link, not per reader.
    Whoever holds the secret and guesses can then lock out the rightful
    reader for the lock time. The sharer clears it by setting the password
    again. A link can also carry a maximum number of opens, counted once per
    device. Someone who holds the secret and the password can use them up
    from many devices. The sharer raises the limit with
    `std.space.setLinkMaxOpens`.

### What reminders met (2026-10-22)

Owner ruling 2026-10-22: a note can have a due time, and the app tells the
person when it comes, in the app and by email. A reminder is a `reminders`
row on its note. The `fireReminders` routine marks it fired every 30
seconds, and the page shows what fired, live.

25. **A program could not send mail.** Notification channels were designed
    and not built (DL §8). Built 2026-10-22, held for the owner's review: a
    routine's `notify <level> "<text>"` line mails the holders of an acl
    level on the rows a run wrote (`howto-routines.md`). The text is a
    literal, so no note reaches the mail. A guest holds `read`, so
    `notify admin` never tells a guest.
26. **A routine added by `ordo apply` runs only after the door restarts.**
    v2 over a live v1 brought reminders that did not fire until then.
    Built 2026-10-22, held for the owner's review: each node switches its
    routines with the version, and `ordo apply` waits for it. The
    end-to-end stack no longer restarts the door
    (`crates/ordo-host/tests/routines.rs`).
27. **Built 2026-10-22, held for the owner's review: a person who signs
    in by passkey or Discord can add an address.** Before, Ordo mailed the
    address of an emailed-code sign-in only. Now the page's "Email for
    reminders" adds one, confirmed by an emailed code, and `notify` mails
    it. A Discord address marked verified counts. The app developer may
    require one (`ORDO_EMAIL_REQUIRED`). See `docs/howto-sign-in.md`
    section 18 (`crates/ordo-host/tests/contact_email.rs`).
28. **A clock routine opens every tenant's database each period.** Every 30
    seconds the routine runs once per tenant, with nothing due in most.
    Many tenants mean a burst of connections each period. Size Postgres
    `max_connections` for it. Owed: a way to wake only the tenants with
    work due.
29. **Fixed (2026-10-22): a routine's writes reached no live page.** The
    write path told live readers which entities changed from what an op
    answers, and a routine's run answers a summary of its own. The page
    showed a fired reminder only on a reload. The executor now names the
    entities of the command group too.
30. **A notice goes at most once.** It is queued after the run commits. A
    crash between the two loses that email. The page still shows the
    reminder.
31. **A reminder is a row, not a field on the note.** A field would bump
    the note's revision when it is set or fires, and an open editor would
    then report "Changed elsewhere" for a change to another field. Owed: a
    save that merges with a change to fields it did not touch.

## 8 · Putting it live (N6 and N7)

These touch the live deployment and DNS, which the owner does himself. Nothing
here was run against `ordo.inf.smg.icu`.

**Before anything:** the live instance runs an image that carries passkey
sign-in, CORS and provisioning on sign-up (checklist `15a`), and has custody
minted (`17b`).

**On the live instance, in this order.** Each is an environment variable and
a restart, and several can go in one restart.

1. `ORDO_AUTH_METHODS=passkey`
2. `ORDO_APEX_DOMAIN=inf.smg.icu`, once. It is the relying party.
3. `ORDO_AUTH_ORIGINS=https://notes.inf.smg.icu`
4. `ORDO_CORS_ORIGINS=https://notes.inf.smg.icu`
5. `ORDO_AUTH_PROVISIONING=auto-onboard`
6. `ORDO_PROVISION_SELF_SERVE=on-signup`
7. `ORDO_PROVISION_SIGNUPS_PER_HOUR` stays 60. Behind Cloudflare's proxy
   the socket address is Cloudflare's edge, so the per-address limits do
   nothing until proxy mode names the proxy (`ORDO_PROXY_HEADER`,
   `ORDO_PROXY_TRUSTED`). Even then they only slow an address. Leave
   `ORDO_PROVISION_SIGNUPS_PER_ADDRESS_PER_HOUR` and
   `ORDO_SHARE_LINK_MISS_LIMIT` unset; Cloudflare's own protection comes
   first.
8. Restart, and read the startup log: `passkey relying party inf.smg.icu`,
   `provisioning self-serve on-signup`, and no `door:` line naming a tenant.

**The program and the first owner.** Applying is step-up gated, so it needs a
deployment owner's passkey token, and the first owner cannot exist before
somebody signs in. So the order that needs no exception is:

1. Apply the program once through the database, the path
   `deploying.md` §4c gives for a first deployment:
   `ordo apply examples/notes --as <you>` with the `ORDO_PG_*` and key
   settings of the instance.
2. Publish the page (below), open it and sign up. The page shows **About this
   account** with your account id, `usr_…`. (If you sign up before step 1, the
   sign-in works, no tenant is made, and the page shows the same id in its
   message.)
3. Set `ORDO_DEPLOYMENT_OWNERS=<that id>` and restart. From now on
   `ordo apply examples/notes --host ordo.inf.smg.icu --space <the deployment's
   own space> --token <your passkey token>` works over the door.

**The page (N7).**

1. `cd examples/notes/web && npm ci`
2. `VITE_ORDO_URL=https://ordo.inf.smg.icu npm run build`. A build without the
   variable refuses to start.
3. Create a Cloudflare Pages project from `web/dist` (a direct upload is
   enough: the page has no server and its routes are in the address fragment).
4. Add the custom domain `notes.inf.smg.icu` to the project. You hold the DNS:
   the record is a CNAME from `notes` to the Pages address Cloudflare gives.
5. Open it. Sign up on your phone, write a note, then sign up a second person
   on another phone. Neither sees the other's note.

Cloudflare cuts long requests. The sync feed is one, and the page opens it
again every half minute anyway. How long it lives is N9's measurement, and
nothing about Cloudflare is changed until N9 has measured.

### How the page is built

The page uses the React building blocks that app developers copy, so it
proves that they carry a real app.

| Part of the page | Runs on |
|---|---|
| The signed-in screens | `OrdoProvider` with `live`, the device copy and `stepUp`, over a hooks client built from the session's token and tenant |
| The note list | `useOpQuery('myNotes')`, answered from the device |
| Add note | `useCommit('createNote')` |
| Save, Changed elsewhere, Keep my text, Use theirs | `useCommit('editNote', { autoResolve: true })`: `state.status === 'conflict'`, `state.currentIs` (`found`, `deleted`, `unavailable`), `retry` |
| Delete, Stop sharing this note | `useRow(note.id)` binds the id, `useCommit('deleteNote')` and `useCommit('unshareNote')` send it |
| Who can read a note | `useOpQuery('std.notesAccess.list')`. The package never mirrors an acl entity's access list on a device, so it needs no option |
| Invitation, Make a link, Revoke link, Remove from all my notes, Links to this note | `useCommit` and `useOpQuery` on `std.space.share`, `shareLink`, `revokeLink`, `revokeShare` and `links`. The page names them in `src/interface.ts` with `withPlatformOps` |
| The fresh passkey touch those want | the provider's `stepUp`: a platform write refused `access_denied` asks once and tries again |
| Shared with me | a guest `OrdoProvider` over `guestClient` and its own device copy; `useOpQuery('myNotes')`, and `revoked` says the sharer closed the window |
| A note shared with a link | `linkInto` (the SDK's `linkFeed`) gives a hooks client, and `useOpQuery('myNotes')` reads it |
| Tags, tag chips, Show all | `createNote` and `editNote` carry `$tags`. A chip click reads `useOpQuery('notesTagged', { args })` |
| The ways in | `src/methods.ts` asks the door (`signInMethods` in the SDK). `src/config.ts` holds the fallback variables `VITE_ORDO_SIGN_IN` and `VITE_TURNSTILE_SITE_KEY`, and reads `VITE_ORDO_RETURN_URL`. `src/signin.ts` runs each way. `SignedOut`, `AddPasskey` and `HumanCheck` in `src/components/` draw them |
| What a person is told | the hook's `refusal.class` (`src/errors.ts`) |

Two things are not hooks. Sign-in, sign-up and token renewal stay in
`src/session.ts`, because the hooks do not sign anyone in. `src/device.ts`
calls the SDK's `openDevice` with the page's old database name
(`'ordo-notes'`), and `OrdoProvider` takes what it answers. The provider wraps
it once; the copy declares the hooks' ops and poisons the replica when the
door closes a window. Shared tenants open through a `DeviceRegistry` keyed by
space and person, and sign-out calls `registry.purgeAll(person)`. Nothing
reloads on focus or on a timer: the server pushes each change. The guest's window and the share
link reach the hooks through `guestClient` (`src/shared.ts`), so the page never
holds their tokens.

The page reads one required and three optional build variables:
`VITE_ORDO_URL` (the door, required), `VITE_ORDO_RETURN_URL`, and the two
fallbacks `VITE_ORDO_SIGN_IN` and `VITE_TURNSTILE_SITE_KEY`, used only when the
door cannot answer `GET /auth/methods` (§2, §6). The page reads
session tokens of version 4 and later. The door now issues version 6.

The hooks read `web/src/interface.json`, written by `ordo interface` and
refreshed with `npm run gen` beside the generated client. Both packages are
aliased to `clients/` in `vite.config.ts` and `tsconfig.json`.

## 9 · The tests

```
cd examples/notes/web && npm ci && npm run check          # the page's tests, no server
bash scripts/conformance-pg.sh start                       # a Postgres for the rest
cargo test -p ordo-host --test notes_app                   # the real binary
```

`notes_app.rs` boots the real `ordo-host` with the settings above and acts as
the page does, with the software authenticator standing in for the browser.
One of its tests runs `web/test/live.test.ts` under Node against the same
binary. It needs `npm ci` in `web/` and is skipped, loudly, without it.
`ORDO_REQUIRE_WEB_TESTS=1` makes the skip a failure.

The newest server tests:

- `notes_tagged_lists_only_the_notes_whose_tags_hold_the_word`
- `a_person_signs_in_with_an_emailed_code_and_a_later_code_finds_the_same_notes`
  (a fake SMTP relay on loopback receives the mail)
- `a_guest_adds_a_passkey_and_the_passkey_signs_in_to_the_same_notes`
- `the_served_door_mounts_the_claim_hand_off_callback_and_identity_routes`
  (until 2026-10-10 the binary answered these routes 404)

The end-to-end tests run the page in a real browser, with a virtual passkey,
against a throwaway stack: the real binary, a mail catcher and a stand-in
Discord. One command, `bash examples/notes/e2e/run.sh`. How to run and read
them, locally and from GitHub: `docs/howto-e2e.md`.

The newest page tests are `web/test/signin.test.ts` (the ways in, the return
from a provider, the emailed code, the guest) and additions to
`web/test/friendly.test.ts` and `web/test/notes.test.ts`.
