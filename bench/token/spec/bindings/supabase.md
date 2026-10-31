## Stack: Supabase

### Where things go

| Path | What |
|---|---|
| `supabase/migrations/*.sql` | your schema, policies and functions, applied in name order to an empty database |
| `supabase/seed.sql` | optional. Rows loaded after the migrations |
| `web/` | the page. `@supabase/supabase-js` is installed |
| `guidance/` | Supabase's AI guidance |

The `supabase` CLI is on your PATH. It runs Supabase locally in containers.

### Running it

`bash run-stack.sh` starts local Supabase from `supabase/`, resets the
database from `supabase/migrations/`, and serves the page on
`http://localhost:5310`. `BENCH_NO_WEB=1 bash run-stack.sh` skips the page.
Ctrl-C stops it all.

- The page reaches Supabase at `import.meta.env.VITE_SUPABASE_URL` with the
  key `import.meta.env.VITE_SUPABASE_ANON_KEY`.
- Emailed codes go to Mailpit: `http://127.0.0.1:54324`.
- The start script brings its own `config.toml`: ports, the site URL
  `http://localhost:5310`, sign-in with a 6-digit emailed code, and high
  rate limits. A `config.toml` of yours is not used. Your app lives in SQL.
- `supabase/migrations/` starts empty. `supabase migration new <name>`
  makes a file there.

### The API the hidden checks call

The checks call Supabase directly with supabase-js, signed in as each kind
of person (the anon key and the person's session). Your schema must have
these tables, columns and functions in `public`. More columns are fine.

| Name | What |
|---|---|
| `teams` | `id uuid` primary key |
| `team_members` | `team_id uuid`, `user_id uuid` (the `auth.users` id), `email text`, `role text` (`owner` or `member`) |
| `tasks` | `id uuid` primary key with a default, `team_id uuid`, `title text`, `done boolean` default false, `assignee uuid` (a `user_id`, or null) |
| `create_invite(p_team_id uuid) returns text` | makes an invitation for that team and answers its token |
| `accept_invite(p_token text) returns uuid` | adds the caller to the token's team as a member, and answers the team id |

The checks read and write `tasks` and `team_members` with plain
`select`, `insert`, `update` and `delete`, as each kind of person, and call
the two functions with `rpc`. A removed member is a deleted `team_members`
row.
