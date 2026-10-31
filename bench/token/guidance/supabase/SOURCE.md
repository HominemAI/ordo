# Supabase AI guidance, pinned

Fetched once on 2026-10-29 and kept here unchanged, so every trial reads the
same text.

- Source: Supabase's official AI prompts, the files behind
  https://supabase.com/docs/guides/getting-started/ai-prompts
- Repository folder: `supabase/supabase`, `examples/prompts/`
- Commit: `e357ec8f9f0b530304f99d9e30c9785f2511bee8` (2026-09-21, the last
  change to that folder on the day of the fetch)
- Raw URL form:
  `https://raw.githubusercontent.com/supabase/supabase/<commit>/examples/prompts/<file>`

| File | sha256 |
|---|---|
| `code-format-sql.md` | `3b8f796bff8447623b136d460c73c5d467ab4be1ebc422748a411ce05ec639c1` |
| `database-create-migration.md` | `98a9a8f717efa5320a20b638032430cab571bdc77f6e7d4b9cf1a507fd63952c` |
| `database-functions.md` | `731c2aaf2bca3ed02ad2628e43db710e01ec3ff0adcbc18ad94629ac4b1ce6fa` |
| `database-rls-policies.md` | `7af1fd36623609cc76e2cb05405b0448dad91758063d0b6b6c6c725f70a00f0c` |
| `declarative-database-schema.md` | `958f7f75241fcad78d85fd62d8c7639360b21a8d595de5bb1d44ba4c6bdf7c62` |

Left out, and why:

- `nextjs-supabase-auth.md`: server-side auth for Next.js. The bench app is a
  Vite single page with supabase-js, so it would point the agent at the wrong
  framework.
- `edge-functions.md`, `use-realtime.md`: the app needs neither.

The runner copies the five files above into the trial workspace as
`guidance/`. Their tokens count as input, as Ordo's docs do.
