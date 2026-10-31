#!/usr/bin/env bash
# The Supabase bench stack: one app workspace, served on this machine.
#
#     bash up.sh <workspace> [state-dir]
#
# The workspace holds `supabase/migrations/*.sql` (and an optional
# `supabase/seed.sql`) and `web/` (a Vite React page). This script starts,
# in order, and stays in the foreground until it is stopped:
#
#   1. Local Supabase (`supabase start`) in Docker, from the bench's own
#      config.toml and the workspace's migrations, on a fresh database.
#   2. The page: `vite build`, then `vite preview` on http://localhost:5310.
#
# When the page answers, the stack is up. It writes <state>/stack.json.
#
# The bench owns the machine (config.toml here: ports, the site URL, sign-in
# by emailed code, rate limits, the email template). The workspace owns its
# SQL. Every value is a test value. Nothing is fit for a real deployment.
#
# Needs: Node 20 or later, Docker running, and the `supabase` CLI
# (SUPABASE_BIN, or on PATH). BENCH_NO_WEB=1 skips the page.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
ws=$(cd "${1:?usage: up.sh <workspace> [state-dir]}" && pwd)
state=${2:-"$ws/.stack"}
sb=${SUPABASE_BIN:-$(command -v supabase || true)}
page_port=${BENCH_PAGE_PORT:-5310}
page="http://localhost:$page_port"

[ -n "$sb" ] && [ -x "$sb" ] || { echo "stack: the supabase CLI is not on PATH (or set SUPABASE_BIN)" >&2; exit 1; }
docker info >/dev/null 2>&1 || { echo "stack: Docker is not running; start Docker Desktop" >&2; exit 1; }
[ -d "$ws/supabase/migrations" ] || { echo "stack: $ws/supabase/migrations is missing" >&2; exit 1; }
if (exec 3<>"/dev/tcp/127.0.0.1/$page_port") 2>/dev/null; then
  echo "stack: port $page_port is in use; stop what holds it" >&2
  exit 1
fi

rm -rf "$state"
project="$state/project"
mkdir -p "$project/supabase/templates"
cp "$here/config.toml" "$project/supabase/config.toml"
cp "$here/code.html" "$project/supabase/templates/code.html"
cp -R "$ws/supabase/migrations" "$project/supabase/migrations"
if [ -f "$ws/supabase/seed.sql" ]; then cp "$ws/supabase/seed.sql" "$project/supabase/seed.sql"; else : >"$project/supabase/seed.sql"; fi

pids=''
stop() {
  for p in $pids; do kill "$p" 2>/dev/null || true; done
  wait 2>/dev/null || true
  "$sb" stop --no-backup --workdir "$project" >>"$state/supabase-stop.log" 2>&1 || true
}
trap stop EXIT
trap 'exit 130' INT TERM

wait_for() { # what, url, pid
  local what=$1 url=$2 pid=$3 tries=0
  until curl -fsS "$url" >/dev/null 2>&1; do
    kill -0 "$pid" 2>/dev/null || { echo "stack: $what stopped before it answered (logs in $state)" >&2; exit 1; }
    tries=$((tries + 1))
    [ "$tries" -lt 600 ] || { echo "stack: $what did not answer $url in 2 minutes" >&2; exit 1; }
    sleep 0.2
  done
}

# ── 1. Supabase ────────────────────────────────────────────────────────────
# A stack left over from an earlier run (same project id) is removed first,
# volumes and all, so every run starts from an empty database.
"$sb" stop --no-backup --project-id bench-team-tasks >"$state/supabase-stop.log" 2>&1 || true
"$sb" start --workdir "$project" \
  -x studio,imgproxy,storage-api,edge-runtime,logflare,vector,realtime,postgres-meta,supavisor \
  >"$state/supabase-start.log" 2>&1 ||
  { tail -40 "$state/supabase-start.log" >&2; echo "stack: supabase start failed (a migration may not apply)" >&2; exit 1; }
"$sb" status -o json --workdir "$project" >"$state/status.json" 2>"$state/status.err"

api=$(node -e 'const s=require(process.argv[1]);process.stdout.write(s.API_URL||"")' "$state/status.json")
anon=$(node -e 'const s=require(process.argv[1]);process.stdout.write(s.ANON_KEY||s.PUBLISHABLE_KEY||"")' "$state/status.json")
mail=$(node -e 'const s=require(process.argv[1]);process.stdout.write(s.MAILPIT_URL||s.INBUCKET_URL||"http://127.0.0.1:54324")' "$state/status.json")
[ -n "$api" ] && [ -n "$anon" ] || { cat "$state/status.json" >&2; echo "stack: supabase status gave no API URL or key" >&2; exit 1; }
# The page reaches Supabase by `localhost`, the same name as its own site.
page_api=${api/127.0.0.1/localhost}

# ── 2. The page ────────────────────────────────────────────────────────────
if [ "${BENCH_NO_WEB:-}" != 1 ]; then
  [ -d "$ws/web/node_modules" ] || (cd "$ws/web" && npm install --no-audit --no-fund) >"$state/npm.log" 2>&1 ||
    { tail -30 "$state/npm.log" >&2; echo "stack: npm install failed in web/" >&2; exit 1; }
  (cd "$ws/web" && VITE_SUPABASE_URL="$page_api" VITE_SUPABASE_ANON_KEY="$anon" \
    npx --no-install vite build --outDir "$state/page" --emptyOutDir) >"$state/page-build.log" 2>&1 ||
    { tail -40 "$state/page-build.log" >&2; echo "stack: the page did not build" >&2; exit 1; }
  (cd "$ws/web" && exec npx --no-install vite preview --outDir "$state/page" --host localhost --port "$page_port" --strictPort) \
    >"$state/page.log" 2>&1 &
  last=$!
  pids="$pids $last"
fi

cat >"$state/stack.json" <<EOF
{
  "stack": "supabase",
  "url": "$api",
  "anonKey": "$anon",
  "page": "$page",
  "mail": "$mail",
  "logs": "$state"
}
EOF

[ "${BENCH_NO_WEB:-}" = 1 ] || wait_for "the page" "$page/" "$last"
echo "stack: up. Page $page, Supabase $api, mail $mail"
echo "stack: logs in $state. Ctrl-C stops everything."
if [ -n "$pids" ]; then wait $pids; else while :; do sleep 3600; done; fi
