# [audit] Usability and performance, 2026-10-25 (Fable)

Measured on a debug `ordo-host` against a private Postgres (notes v2 applied,
guest sign-in, 77 tenants). Finding 1 was re-read by the session lead:
`crates/ordo-plan/src/lib.rs:1935` compiles every `==` to `IS NOT DISTINCT FROM`.

## High

1. **Access guards cannot use an index.** `=` compiles to `IS NOT DISTINCT FROM`, which Postgres never indexes, so every guard `EXISTS` scans the whole access table per row. `myNotes` at 48 notes: 2 ms. At 5,000 notes with 5,000 `notesAccess` rows: 450 ms per read (`Seq Scan on notesAccess loops=5048`); the device-copy sync load (101 pages) took 33 s. Fix: emit `=` when either side is a NOT NULL column, keep distinctness for nullable sides; `ordo check` warns when a sort key has no index. Check: EXPLAIN shows an index scan; 5k-row read under 10 ms; sync load under 3 s.
2. **A failed upload leaves a stuck row** (`examples/notes-v2/web/src/components/Files.tsx:112-123`): the row is committed before the bytes and shows "still being sent" forever; no size or type check before upload (`:165-175`). Fix: delete the row on failure or offer retry; show the limit first.
3. **No first-run path for the deployment owner.** `howto-console.md` starts at the compose file; the secrets recipe is a comment in `docker/compose.one.yml:8-33` (about 20 commands, 8 secret files); bring-your-own Postgres is covered nowhere; `deploying.md` never mentions the console or `ORDO_DEPLOYMENT_OWNERS`. A refused database at boot says only `Connection refused`. Fix: one "First deployment" page and a secrets script; name `ORDO_PG_HOST:ORDO_PG_PORT` in the refusal.
4. **Notes v2 offline reload looks like being signed out** (`App.tsx:113-117`), and the sharer sees "Reader 1, Reader 2" (`SharePanel.tsx:249`).

## Medium

5. Sign-up provisioning serialised per process (`tenant_ops.rs:101` `own_gate`): 60 sign-ups, the last waited 14.1 s; 200 would wait about 50 s.
6. Five round trips per read (`ordo-exec/src/lib.rs:2589-2611`, `host/src/attest.rs:114-131`): about 5 ms floor on a 1 ms-RTT cloud database.
7. One pool per tenant database (`exec/src/route.rs:295`), no deployment-wide budget: 77 tenants held 54 of 100 connections.
8. `ordo check --help`, `dev --help`, `apply --help` fail; bare `--help` is 300 lines.
9. Boot refusals name the setting but not an example value (`ORDO_KEY_PROVIDER`, `ORDO_SPACE`, `ORDO_PG_HOST`; the passphrase file names none).
10. `deploying.md` has no upgrade section; `howto-console.md` says `ORDO_CONSOLE_DOOR_SPACE` defaults to `conformance` while compose sets `system`; `deploying.md:3` says "operator".
11. Notes v2: false empty states while loading (`SharedView.tsx:56`, `SharePanel.tsx:128,245`), "Loading" for a failed query (`NotesView.tsx:175`), offline is `navigator.onLine` only, failed saves not retried, no focus management, Turnstile error lacks `role=alert`.
12. Tutorial fences untagged (`b13`, `b21` fail); SDK README's first example uses fixture op names; React README opens with a changelog.

## Low

- 1 MiB blob read assembled before the first byte (`ordo-blob/src/fs.rs:160`); confirm on release.
- Parse errors lack "did you mean"; `bucket_ops.rs:297` names no setting.
- `settings.md` misses `ORDO_APPLY_WAIT_SECS`, `ORDO_MAIL_CREDENTIALS`, about 25 `ORDO_CONSOLE_*`.
- Compose files have no healthcheck.
- Page bundle one 118 kB gzipped chunk; `ordo-sdk/dist` 3.2 MB, mostly `.d.ts`.

## Do not break

- Door: write p50 2.3 ms, read p50 2.0 ms, guest sign-in 4 to 9 ms, blob meta 1.3 ms (debug, local).
- Boot 0.24 s empty, 1.6 s with 77 tenants, RSS 55 MB (about 0.15 MB per tenant).
- Live re-check: 50 streams at 1 s cost about 2% CPU.
- A new tenant rebuilds the door in 100 ms.
- `ordo check` 0.1 s, diagnostics with `file:line:col` and stable codes.
- Notes v2 reads offline from the device copy and keeps the person's text on conflict.
