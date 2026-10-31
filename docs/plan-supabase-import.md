# Importing users and data from Supabase, and other systems later

Design for board task `supabase-import`, given the go on 2026-10-11
(`ellanor-readiness` ask 5, *"design"*). Design only. Nothing here is built.

Builds on `plan-import.md` (the bulk import, built), `plan-export.md` (the
bundle it reads), `migration.md` §1 to §5 (the pipeline, proposed),
`howto-sign-in.md` and `plan-auth-federated.md` (every sign-in method, built).

Markers: **[built]** exists today · **[plan]** designed here · **[open]** the
owner's to decide, listed at the end.

---

## The approach (owner, 2026-10-16)

> "This isn't straight forward ETL - And we'd need to make a lot of
> assumptions about the system. I think our best bet will be going through
> some of the design work and implementig some of the parts, then attempting
> to migrate some apps and using the results and observations (recording
> each step) To then produce a methedology and create a skill from it"

So this design is a starting point, not the method. The method comes from
trying it.

1. **Build the parts every try needs.** The core first: people (I1), the
   mapping and row source (I3), tenant rules (I4). Then only what the first
   app forces.
2. **Migrate real apps, one at a time, into a staging deployment.** Each try
   records every step in a migration log: what was run, what was assumed,
   what broke, what a person had to decide, and how long it took.
3. **Write the method from the logs.** What held across apps becomes the
   method. What differed becomes a question the method asks.
4. **Make it a skill.** The method becomes a skill an agent follows to
   migrate the next app.

The 11 steps in §7 are the list to build from, not a fixed order. A part is
built when a try needs it. The logs live in `docs/migrations/`, one file per
try.

---

## In short

**A finding first.** Ellanor is not on Supabase. Its data is in Jazz, its
files are in R2 and its people sign in with passkeys only
(`ellanor/docs/ordo-migration-analysis.md` §1.2). The Supabase users are
**denizens** and the **hominem apps** (pambl, adnullius, bias scan), which
share one Supabase project. Ellanor is a reference only: it shows what
ellanor would need if it were on Supabase. No Jazz reader is planned
(ruled 2026-10-12, ask 1).

**The shape.** One importer with a reader per source system. The reader turns
the source into files Ordo already understands, and the built import loads
them. Ordo never holds Supabase's keys: the app developer exports the files.

1. **Users.** Each Supabase user becomes one Ordo person. Each linked login
   becomes a sign-in on that person: Discord, GitHub, Google, a confirmed
   email. Email plus Discord stays one account (federated ask 6). Passwords
   come across as bcrypt hashes, only if the app developer turns that on,
   and each is rehashed on first sign-in. Sessions do not come across:
   everyone signs in once more.
2. **Data.** The reader drafts a `.ordo` program from the schema: tables to
   entities, foreign keys to refs, `auth.users` to people. RLS policies
   become commented-out suggestions. A tenant rule per table says which
   space each row lands in. Rows load through the built import.
3. **Files.** Each Supabase Storage object is attached to the row that names
   it. Objects no row names land in a generated file entity.
4. **Move day.** 2 rehearsals into a staging deployment, then a short write
   freeze, load, verify by counts and checksums, switch. Supabase stays up
   read-only for 30 days as the way back. Keeping it so is the deployment
   owner's own step. Ordo only documents it (ruled 2026-10-12, ask 7).
5. **Later.** Plain Postgres and Firebase are more readers in front of the
   same core.
6. **What is missing:** 11 build steps, about 10 to 15 agent sessions (§7).
   Denizens and pambl cannot move until backend functions exist, because
   their billing runs in Supabase edge functions.

---

## 0 · What the reference apps hold

Read 2026-10-11, read only, from the sibling directories. No `.env`, key or
credential was opened on purpose. One search line matched a project
hostname in `hominem-bias-scan/.env`; nothing else in that file was read,
and the project id is public in `hominem-supabase/README.md` anyway.

| App | Source system | Sign-in today | Files | Server code |
|---|---|---|---|---|
| **denizens** (`denizens-web`) | Supabase, its own project (its tables are not in the hominem migrations) | emailed code, Discord, username and password, guest | buckets `avatars`, `user-avatars` | about 13 RPC functions; backend in another repo |
| **pambl** (`prompt-assemble-ui-hominem`) | Supabase, hominem project | emailed code or link | none seen | edge functions: checkout, Stripe webhook, prompt generation |
| **adnullius** | Supabase, hominem project | through the hominem project | none seen | `adnullius-ai`; realtime on 2 tables |
| **bias scan** (`hominem-bias-scan`) | Supabase, hominem project | admin password | none seen | `check-bias`, `check-bias-2` |
| **ellanor** | **Jazz** (CRDT sync), R2, a worker with a Postgres mirror | passkeys (Jazz) | R2 bucket `ellanor`, client-encrypted | Cloudflare Pages Functions, the omni-ellanor worker |

**The hominem project** (`hominem-supabase/supabase/migrations`, 26 files)
holds 19 tables in 4 groups:

| Group | Tables | Access pattern |
|---|---|---|
| Shared by all hominem apps | `user_subscriptions`, `usage_metrics`, `feedback`, `supporters` | own row read; service role writes; supporters wall is public |
| pambl | `prompt_sync`, `variable_set_sync`, `sync_metadata`, `tags`, `prompt_tags`, `shared_prompts` | one person's rows; shared prompts public until they expire |
| adnullius | `adnullius_documents`, `_comments`, `_doc_versions`, `_citations`, `_user_state`, `_allowed_users` | every allowed person reads all; an email allowlist admits |
| Site | `blog_posts` | public when published; author edits own |

Billing: `user_subscriptions` is written by the `stripe-webhook` edge
function. It was first filled from `auth.users.raw_user_meta_data` by
`backfill_user_subscriptions.sql`, so some old billing state may still sit
in user metadata.

**Denizens** reads about 50 tables from the browser (`denizens`,
`denizen_instances`, `voices`, `platform_config`, `discord_user_links`,
`discord_server_config` and more). Its migrations are not in `denizens-web`.
It also copies each person's Discord id into its own `discord_user_links`
table from `auth.identities`.

**Ellanor links to denizens by Supabase id.** Its integration links name a
denizens person as `(issuer, auth.users.id)`
(`ellanor/docs/server-authority/integration-account-linking.md`). When
denizens moves, those ids must stay findable, or every link breaks (§2.6).

---

## 1 · The shape [plan]

```
reader (per source)          core (shared)                     built today
-------------------          -------------                     -----------
files the app developer  ->  draft program + mapping + worklist
exports                      review by the app developer
                             deploy the program                -> ordo deploy
                             users.jsonl -> people, sign-ins,
                               memberships
                             rows split by tenant rule ->      -> import_space, per space
                               one bundle per space               with a people map
                             files -> objects in each bundle   -> the blob leg (plan-import §10)
                             verify report
```

Three choices shape it.

- **Files in, never a live connection.** The app developer runs
  `pg_dump --schema-only`, `COPY ... TO` per table, and a storage download,
  with their own Supabase credentials on their own machine. Ordo reads the
  output. Ordo never asks for or holds a service role key.
- **The reader writes what the built import reads.** Each space gets an
  `ordo.space-export.v1` bundle and a people map. The import, its id
  mapping, its resume and its refusals stay as they are.
- **People are a separate act from rows.** The people-map ruling
  (2026-09-26) says a bundle can never grant membership. So people,
  sign-ins and memberships come in through their own step, run by the
  deployment owner and recorded, before any row loads.

---

## 2 · Users

### 2.1 What comes across from `auth.users`

| Supabase | Ordo | Note |
|---|---|---|
| `id` (uuid) | a new `usr_...` id, plus the uuid kept in the identity map (§2.6) | ids are minted, always |
| `email`, `email_confirmed_at` | an `email:<address>` sign-in, **only if confirmed** | an unconfirmed address is kept as profile data only |
| `phone`, `phone_confirmed_at` | profile data only | Ordo has no SMS sign-in |
| `encrypted_password` (bcrypt) | a `password:<identifier>` sign-in with a legacy hash, if turned on (§2.3) | |
| `raw_user_meta_data` | fields on the app's own profile entity, as a JSON field or named fields | drafted; the app developer picks |
| `raw_app_meta_data` | roles or groups, by a mapping | `provider` and `providers` are dropped; the sign-ins say it |
| `is_anonymous` | skipped by default (§2.5) | |
| `banned_until` in the future | imported, admitted nowhere, listed in the report | the app developer decides whether to admit them later |
| `deleted_at` set | skipped | a deleted account stays gone |
| `created_at`, `last_sign_in_at` | profile data, if the app developer keeps them | |
| `auth.mfa_factors` | not carried; listed in the report | the person enrols again |
| sessions and refresh tokens | not carried | everyone signs in once more |

### 2.2 Linked logins

Each row of `auth.identities` becomes one sign-in on the same person. The
source already joined them, so carrying the join is not automatic linking.
That is the owner's ruling on federated ask 6, and R12 still holds for
everything else: nothing is joined by matching an email.

| `provider` | Ordo sign-in | Needs |
|---|---|---|
| `email` | `email:<address>` | address confirmed; the mail channel set |
| `discord` | `oauth2:<name>:<provider_id>` | a Discord connection with that name |
| `github` | `oauth2:<name>:<provider_id>` | a GitHub connection |
| `google` | `oidc:<name>:<provider_id>` | a Google connection |
| `azure`, `keycloak`, other OIDC | `oidc:<name>:<sub>` | a connection whose subject is the same `sub` |
| `phone` | none | listed in the report |
| `anonymous` | none by default | §2.5 |
| anything else | refused by name | the report names the provider and the setting to map it |

The app developer writes the provider to connection map in the import plan,
for example `discord = discord`. A provider with no map entry is refused,
and the refusal names the map line to add.

**Why the person lands on the same account.** Discord's user id, GitHub's
numeric id and Google's `sub` belong to the provider. They are the same
whichever app asks. So the first Discord sign-in after the move finds
`oauth2:discord:<id>` already linked to the imported person, and
`resolve_or_onboard` returns that person. No new account is minted.

**What must be true for that.** The connection's subject claim must be the
same value Supabase stored as `provider_id`. The Discord and GitHub presets
use the numeric id, which matches. A connection with a mutable subject
(`_MUTABLE_SUBJECT=acknowledged`) is refused by the import, because a
mutable subject could point at someone else by move day.

**Two Supabase users with one address.** Supabase allows two users to hold
the same email when one signed up by Discord and one by email. The import
keeps them as 2 people. The person can join them afterwards with a claim
(`howto-sign-in.md` §12). An import that collides on one sign-in (two
people, one `email:` id) is refused by name, before anything is written.

### 2.3 Passwords

Supabase stores bcrypt (`$2a$10$...`). Ordo stores argon2id
(`crates/ordo-identity/src/provider.rs`, `Argon2idHasher`). Ordo cannot read
bcrypt today.

**The case that decides it.** Denizens signs people in with a username and a
password, through a made-up email address. Those people have no real
address, so an emailed code cannot reach them. Without their password, the
only way back is Discord, which some never linked.

**Recommendation [open, Q2]: accept bcrypt, opt-in, and retire it person by
person.**

- `ORDO_AUTH_LEGACY_HASHES=bcrypt` turns it on. Off by default. With it off,
  an import carrying hashes is refused, and the refusal names the setting.
- An imported hash is stored marked `legacy:bcrypt`. On a correct sign-in,
  Ordo verifies with bcrypt, rehashes with argon2id and drops the old hash
  in the same transaction.
- bcrypt reads only the first 72 bytes of a password. A legacy check refuses
  a longer password, so nobody signs in by a prefix.
- `ORDO_AUTH_LEGACY_HASH_DAYS`, default 180: after that, unconverted bcrypt
  hashes are deleted, and those people use another sign-in or a reset
  (Q3). 0 keeps them for ever.
- The identifier is the app developer's choice in the import plan: the
  username part of the made-up email for denizens, the real address for a
  hominem admin.
- New dependency: the `bcrypt` crate, pure Rust, verify only. To be checked
  with `cargo tree` before the build step starts.

With the setting off, people move to an emailed code or a linked provider,
and a person with neither can only be reached by an invitation the app
sends.

### 2.4 People, sign-ins and memberships, in one recorded act

The import plan names, per person, which spaces they join and with which
role (§3.4 decides which). The user step then:

1. Mints each person through the directory, so every built check applies:
   a deleted account stays refused (D1 to D7 in `plan-auth-federated.md`).
2. Links each sign-in with `link_first`, refusing a sign-in that already
   names someone else.
3. Admits each person to their spaces. An imported person with no space is
   reported. The registration sweep (90 days) would remove them, which is
   right for someone who never used the app.
4. Writes the identity map (§2.6) and the people map for each space.
5. Records one `people-imported` event per batch in the security stream,
   naming the import and the deployment owner who ran it.

It runs as the deployment owner, with presence, like `std.space.import`.
It is resumable on the same import name, as row batches are, and it has a
dry run that writes nothing and prints the report.

The sign-up ceiling (`ORDO_PROVISION_SIGNUPS_PER_HOUR`) does not count
these mints. They are one act by the deployment owner, recorded once.

### 2.5 Guests

Supabase guests (`is_anonymous`) are held by a refresh token on one device.
Ordo cannot read it, so no guest can sign in again after the move. Their
rows would belong to nobody.

**Ruled 2026-10-12 (Q8):** there is only 1 guest account, so leaving guests
behind is the default. The report lists their row counts. `--guests=import`
brings them in as Ordo guests with no way in, for an app that wants their data
kept for a later claim.

### 2.6 The identity map

A deployment-level table: source system, source user id, Ordo person. It is
written by the user step, read by every space's people map, and queryable by
the deployment owner. It lets other systems that hold a Supabase id find the
person: ellanor's integration links, Stripe customer metadata, log lines.
Deleting it is an attributed act after the move, as for `$importmap`.

### 2.7 What a person sees on move day

| They signed in with | After the move |
|---|---|
| Discord, GitHub, Google | The same button. Same account, same data. The provider may ask for consent once more. |
| An emailed code or link | The same address gets a code. Same account. A magic link becomes a code: the email says so. |
| Username and password | The same username and password, if legacy hashes are on. Otherwise a message from the app saying how to get back in. |
| A guest | A new guest. Their old data is not there (§2.5). |
| A passkey (ellanor) | Not this importer. Ellanor is a reference only (§6). |

Everyone is signed out once, because sessions do not move. The app should
show a one-line notice on its sign-in page during the first week.

---

## 3 · Data

### 3.1 Reading the schema

The reader takes `pg_dump --schema-only --schema=public` output. It reads
tables, columns, keys, foreign keys, unique and check constraints, indexes,
policies, triggers, functions and publications. It never runs any of it.

Anything it cannot parse goes on the worklist with the SQL quoted. A table
that is skipped is skipped by a line in the mapping, never by silence.

### 3.2 The draft program

Types follow `migration.md` §2a, with what has been built since:

| Postgres | Draft |
|---|---|
| `text`, `varchar` | `text` |
| `int`, `bigint` | `int` |
| `boolean` | `bool` |
| `timestamptz`, `date` | `datetime`, `date` |
| `numeric` | `money`, flagged when it is not currency |
| `real`, `double` | flagged; a decimal at a scale the app developer picks |
| `jsonb`, `json` | `json` (the JSON field is built) |
| `text[]`, `uuid[]` | `json`, or a link entity when the elements are foreign keys |
| enum or a `CHECK (x IN (...))` | `enum(...)` |
| `bytea` | `blob inline`, flagged |
| uuid or serial primary key | a minted id; the source id kept in `$importmap`, and as a `legacyId` field if the mapping asks |

Keys and constraints:

- **A foreign key** becomes a ref. Its direction already matches.
- **A foreign key to `auth.users`** becomes a ref to `members`, resolved
  through the people map. `on delete set null` becomes an optional ref.
- **`on delete cascade` from a parent** suggests `owned`, flagged, never
  set. Ownership is a claim the app developer makes.
- **`unique (user_id, name)`** becomes a unique within the space, which is
  the same thing when each person has a space.
- **A trigger that sets `updated_at`** is flagged: the op that writes the
  row sets the time. Any other trigger goes on the worklist as a routine
  sketch.
- **A plpgsql function** called by RPC (`upsert_prompt_with_tags`,
  `accept_invite`, `acquire_tool`) goes on the worklist as an op to write.
  `SECURITY DEFINER` is named, because it bypasses RLS.
- **A realtime publication** needs nothing: Ordo queries are live. Noted.

Every name is turned into Ordo's spelling (`prompt_sync` to `promptSync`),
and the mapping keeps the source name.

### 3.3 RLS policies to grants

RLS policies OR together. Ordo guards do not. A converted policy can parse
cleanly and still be wrong. So every suggestion is drafted commented out,
and the app developer switches each one on (Q9).

The reader matches known shapes and drafts the Ordo way. The rest is
flagged.

| Policy shape (seen in hominem) | Draft | Confidence |
|---|---|---|
| `auth.uid() = user_id` for all | an owner guard: `user == $caller` | high |
| `TO authenticated USING (true)` | any member of the space may read | high, once the tenant rule is right |
| `USING (status = 'published')` to `anon` | a public op with that condition, and the field marked public | high |
| `USING (expires_at > now())` to `anon` | a public op with the condition | medium: `now()` in a cached public op is flagged |
| `auth.role() = 'service_role'` | no client grant; written by a backend function's own grant | high |
| `email = auth.jwt() ->> 'email'` (allowlist) | an invitation or an admission rule for the space, plus the table kept as data | flagged |
| `EXISTS (SELECT ... FROM admins ...)` | a role or a group, by name | flagged |
| `auth.uid() = user_id OR auth.uid() = approved_by` | two grants; the OR is the app developer's to confirm | flagged |
| anything calling a function | worklist, SQL quoted | none |

`TO authenticated` in Supabase means anyone with an account. In Ordo it
means a member of this space. The draft says which, per table, because the
tenant rule changes the answer.

### 3.4 Tenants: which rows go to which space

Supabase has one database. Ordo has spaces, and a ref cannot cross a space.
So each table gets one tenant rule in the mapping:

| Rule | Rows land in | Fits |
|---|---|---|
| `person <column>` | the space of the person that column names | pambl's `prompt_sync`, `variable_set_sync`, `sync_metadata`, `tags`, `prompt_tags` |
| `column <column>` | one space per distinct value | a workspace id, a Discord server id |
| `shared <space>` | one named space | adnullius's documents, comments and versions |
| `deployment` | the deployment's own space | `supporters`, `blog_posts`, `feedback`, `usage_metrics` |
| `follow <ref>` | the space of the row this ref points at | a child table with no tenant column of its own |

The reader proposes a rule from the columns: a `user_id` foreign key to
`auth.users` suggests `person`. The app developer confirms each.

Before anything loads, the core checks every ref against the rules. A row
whose ref points into another space is a refusal that names both tables and
the source ids. A unique constraint that spanned all tenants goes on the
worklist, because Ordo's unique is per space.

`person` needs the person's own space to exist. With self-serve provisioning
(`on-signup`), the user step makes each person's space as their own, as a
first sign-up would. With it off, the import plan names the spaces.

### 3.5 Loading rows

The reader takes each table's `COPY ... TO` output (CSV, or JSONL from
`row_to_json`). It applies the mapping's transforms, splits rows by tenant
rule, and writes one `ordo.space-export.v1` bundle per space:

- the manifest with the load order (Kahn over the refs), columns, counts and
  the sha256 of every member;
- the destination's deployed artifact identity, so `artifact_mismatch`
  passes only against the program the app developer reviewed;
- `data/<entity>.jsonl` carrying source ids in every ref;
- `blobs/...` for files (§4);
- the people map beside the bundle, from the user step.

Then the built `import_space` runs per space. It mints ids, rewrites refs
through `$importmap`, seals sealed fields under the destination key, resumes
after a crash, and refuses rather than guesses (`plan-import.md` §4 to §6).

Throughput measured for the built import: 3,220 rows a second at batch 100,
in a debug build (`plan-import.md` §8). The hominem tables are small; a
million rows would load in about 5 minutes.

### 3.6 What stays the app developer's

- Edge functions: `stripe-webhook`, `create-checkout`, `generate-prompt`,
  `adnullius-ai` and the rest. They need backend functions, which are not
  built (`plan-backend-functions.md`).
- RPC functions, as ops.
- Denizens' `discord_user_links`: the Discord id is now a sign-in, so the
  table's job may be gone. The worklist says so; the app developer decides.
- Billing state still in user metadata (§0): the mapping can lift it into
  `user_subscriptions`, as `backfill_user_subscriptions.sql` did.

---

## 4 · Storage

Supabase Storage keeps a row per object in `storage.objects`: bucket, path,
owner (a user id) and metadata (mime, size). The bytes are in an S3 bucket
behind it.

**What the app developer exports.** A `COPY` of `storage.buckets` and
`storage.objects`, and the bytes downloaded into a folder per bucket, with
the Supabase CLI or any S3 tool against Supabase's S3 endpoint. Their keys,
their machine.

**Where each object goes.**

1. **A row names it.** A column holding the object's path, or its public
   URL, becomes a blob field on that entity. The object rides the bundle as
   `blobs/<entity>/<field>/<source id>` and the built blob leg attaches it
   after the row exists. The URL column itself is dropped.
2. **No row names it.** The reader drafts one file entity per bucket
   (`avatarsFile`: path, owner ref, blob), so nothing is lost. The app
   developer may skip the bucket instead, by a line in the mapping.
3. **The owner** comes through the people map, and the object lands in that
   person's space under the `person` rule.

**Public buckets** become blob fields on public rows. Files follow their row
on the public path (ruled), so a public avatar stays public. A private
bucket stays private.

**Checks.** The reader computes each file's sha256 as it writes the bundle,
and the import refuses bytes that differ. Supabase's size is compared too.
A bucket's `file_size_limit` and `allowed_mime_types` are compared with the
deployment's blob settings; a file over Ordo's limit is refused by name,
with the setting that raises it.

**Not carried.** Supabase image transformations (resize on request), signed
URLs already handed out, and storage RLS policies, which go through §3.3.
URLs to Supabase Storage inside text or JSON fields are listed in the
report, because nothing can rewrite them safely.

---

## 5 · Move day

### 5.1 Before

1. **Draft and review.** The app developer runs the reader, edits the
   program and the mapping, and deploys the program to a staging deployment.
2. **Providers.** Add Ordo's callback,
   `<ORDO_PUBLIC_URL>/auth/callback/<name>`, to the same Discord, GitHub and
   Google apps Supabase uses. Keep Supabase's callback too until the move is
   done. Same app means same user ids.
3. **Mail.** Set Ordo's mail channel and send a test code.
4. **Rehearsal 1 and 2.** Full export, dry run, real load into staging,
   verify. Fix the mapping, then do it again from scratch. Record how long
   each step took.
5. **Tell people.** One email or banner: the date, and that everyone signs
   in once more.

### 5.2 The day: a short write freeze

**Recommendation [open, Q6]:** a big-bang move with a freeze of minutes,
after 2 clean rehearsals. A side-by-side bridge is a large build for data
this size.

| # | Step | Undo |
|---|---|---|
| 1 | Turn off sign-ups in Supabase. Revoke `insert`, `update`, `delete` on `public` from `anon` and `authenticated` (one SQL script, written in rehearsal). Supabase is now read-only. | grant back |
| 2 | Pause the Stripe webhook endpoint, or point it at Ordo's function. Stripe retries for up to 3 days, so nothing is lost. | point back |
| 3 | Export: schema, tables, `auth.users`, `auth.identities`, storage. | |
| 4 | Import people, then each space's rows, then files. | drop the new deployment's spaces |
| 5 | Verify (§5.3). Any mismatch stops here. | |
| 6 | Point the apps at Ordo: their settings, or DNS. | point back |
| 7 | Watch the first sign-ins of each kind. | |

The freeze runs from step 1 to step 6. For the hominem apps, rehearsal will
give the number; expect well under an hour.

### 5.3 Verification

The verify report is a file. The move is not done until it is clean or the
app developer accepts each line.

- **Counts.** Per table, per space: source rows, rows in the bundle, rows in
  Ordo. People: users, sign-ins per provider, memberships.
- **Checksums.** Each source row is normalised under the mapping (ids
  replaced by source ids, names mapped) and hashed. The imported space is
  exported with the built `std.space.export`, normalised the same way and
  hashed. Every row compares, since the data is small; a sample is a
  setting for large ones.
- **Files.** Count and sha256 per object, from the export.
- **Refs.** Every ref resolved. A source foreign key pointing at a deleted
  row is listed, with the mapping choice that settles it.
- **What did not come across.** Skipped tables, guests, phone numbers, MFA,
  policies still commented out, unrewritten storage URLs.

### 5.4 The way back

- **Before step 6:** nothing has changed for anyone. Grant writes back in
  Supabase.
- **After step 6:** point the apps back at Supabase and grant writes back.
  Writes made in Ordo since step 6 would be lost, so the app developer
  decides a cut-off: after it, problems are fixed forward in Ordo.
- **Supabase stays up, read-only, for 30 days**, then is paused, and a final
  `pg_dump` and storage copy are kept. Pausing costs nothing to undo. This is
  the deployment owner's own step (ruled 2026-10-12, Q7). Ordo documents it
  and does nothing to Supabase.

---

## 6 · Other systems later

**General, shared by every source [plan]:**

- The neutral people file, `users.jsonl`: one person per line, with
  sign-ins, legacy hashes, profile fields and spaces. The user step reads
  only this.
- The mapping file: entity, fields, transforms, tenant rule, disposition.
- The drafter: relational schema to a draft program and worklist.
- Tenant splitting, the ref check, bundle writing, the verify report.
- The built import and its blob leg.

**Supabase-specific:** reading `auth.users`, `auth.identities` and
`storage.*`; the provider names; the RLS shape matcher; bcrypt.

| Source | Reader | Its hard part |
|---|---|---|
| **Plain Postgres** | the Supabase reader minus `auth` and `storage`. The app developer names their users table and its email, password-hash and provider columns | hash formats: bcrypt, argon2 and scrypt each opt-in by name |
| **Firebase** | Auth export (JSON) to `users.jsonl`; Firestore export to JSONL | Firebase hashes with a modified scrypt and a project key the app developer supplies; documents need shape inference (`migration.md` §2c) |

Ellanor is a reference only (ruled 2026-10-12, ask 1). It shows what ellanor
would need if it were on Supabase. No Jazz reader is planned.

### 6.1 Several apps on one Supabase project

A deployment is one program and one set of people (`glossary.md`). The
hominem project serves 3 apps that share people and 4 tables.

**Ruled 2026-10-12 (Q5):** the 3 hominem apps becoming 1 deployment is likely
in the end, but not now. Nothing here builds toward it yet. The shape if it
happens: one Ordo deployment for the hominem project, one program holding all 3 apps' entities. Each app is a site on its own
subdomain, signed in through the core site with the subdomain hand-off
(`howto-sign-in.md` §11). A person keeps one account across pambl and
adnullius, as today.

The other way, one deployment per app, would need sign-in shared across
deployments. Ordo has no such thing today: a deployment cannot vouch for a
person to another one. Denizens is a separate Supabase project with its own
people, so it becomes its own deployment either way.

---

## 7 · What Ordo lacks

Sizes as in AGENTS.md: S is part of one agent session, M about 1 session
and 1 to 2 CI rounds, L 2 to 3 sessions.

| # | Step | Size | Tier |
|---|---|---|---|
| I1 | `users.jsonl` and the user step: mint, link, admit, identity map, people maps, dry run, resume, security events | L | Opus |
| I2 | Legacy bcrypt hashes behind `ORDO_AUTH_LEGACY_HASHES`, rehash on sign-in, expiry | M | Opus |
| I3 | Mapping file format and a row source from CSV or JSONL (the `RowSource` `plan-import.md` §2 left for it) | M | Opus |
| I4 | Tenant rules, the split, and the cross-space ref check | M | Opus |
| I5 | Supabase reader: schema dump to draft program, mapping and worklist | L | Opus |
| I6 | RLS shape matcher to commented-out grants | M | Opus |
| I7 | Storage leg: objects listing and folder to bundle blobs; the per-bucket file entity | M | Sonnet |
| I8 | Verify report: counts, normalised checksums through export, refs, what did not come | M | Opus |
| I9 | Continue-on-reject and the did-not-migrate list (`plan-import.md` §7, deferred) | M | Opus |
| I10 | Getting a bundle onto a hosted deployment (unbuilt, `migration.md` §1) | S | Sonnet |
| I11 | `ordo import-supabase` command and a how-to with the export commands | S | Sonnet |

About 10 to 15 sessions in all, or 2 to 3 days with 2 agents at a time.
The two rehearsals on real exports and the owner's answers set the pace
more than the building does. I1, I3 and I4 are the core every
reader needs; build them first.

**Outside this plan, and blocking a move:**

- **Backend functions.** Billing (`stripe-webhook`) and every edge function
  need them. Denizens and pambl cannot move without them.
- **Summarised import groups** (`plan-import.md` §3.2): not needed at these
  sizes.

---

## 8 · Facts to confirm with the app developer

These are facts about the apps, not decisions.

1. Which Supabase project denizens uses, and where its migrations live.
2. The made-up email domain denizens uses for usernames.
3. Row counts per table and total storage size, for each project.
4. Whether any denizens table is per Discord server or per workspace.
5. Whether any person uses Supabase MFA.
6. Whether pambl's sign-in is a code or a magic link today.

---

## Questions for the owner

**Ruled 2026-10-12.** The owner's answer follows each question.

1. **Ellanor is on Jazz, not Supabase. Should this importer serve denizens and the hominem apps first, with a Jazz reader for ellanor after?**
   **Ruled 2026-10-12: yes for denizens and the hominem apps. Ellanor is a reference only,** for what ellanor would need if it were on Supabase. No Jazz reader is planned.
2. **Should Ordo accept imported bcrypt password hashes, off by default, and rehash each to argon2id on the person's next sign-in?**
   **Yes, opt-in.** Denizens' username accounts have no real email, so without their password they lose the account; rehashing retires bcrypt person by person.
3. **Should bcrypt hashes nobody has used after a set time (default 180 days) be deleted?**
   **Yes, as a setting.** An unused old hash only adds risk; after it, those people use email, a provider or a reset, and 0 keeps hashes for ever.
4. **Should importing people (mint, link sign-ins, admit to spaces) be one recorded act by the deployment owner, separate from the row bundles?**
   **Yes.** A bundle still cannot grant access, as ruled 2026-09-26; admission becomes a visible act with its own record and a dry run.
5. **Should the 3 hominem apps become one Ordo deployment, signed in through one core site with the subdomain hand-off?**
   **Ruled 2026-10-12: likely in the end, but not now.** They share one Supabase project and its people; one deployment per app would need sign-in across deployments, which Ordo lacks.
6. **On move day, a short write freeze after 2 rehearsals, instead of running both systems side by side?**
   **Yes, big-bang.** The data loads in minutes; a sync bridge is a large build for a window nobody needs.
7. **Keep Supabase up and read-only for 30 days after the move, as the way back, then pause it?**
   **Ruled 2026-10-12: this is the deployment owner's own step.** Ordo only documents it.
8. **Should Supabase guest accounts be left behind by default, with a flag to import them?**
   **Ruled 2026-10-12: yes.** There is only 1 guest account, so leaving guests behind is the default.
9. **Should RLS policies be drafted only as commented-out grants, each switched on by the app developer?**
   **Yes.** Policies combine by OR and guards do not, so a translation can parse and still grant the wrong rows.
10. **Should the importer read files the app developer exports, and never connect to Supabase with its keys?**
    **Yes.** Ordo never holds a service role key, the files can be reviewed, and each rehearsal reruns from the same files.
