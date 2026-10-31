#!/usr/bin/env bash
# Reclaim disk from `target/` without throwing away work you will want back.
#
# Cargo never garbage-collects: every distinct hash of every test binary
# stays in `target/debug/deps` forever, and the incremental cache grows
# with each edit. On this workspace that reached 65G and 457,000 files in
# four days, with the disk at 90%.
#
# **Every git worktree is visited**, not just this one. Agent worktrees
# under `.claude/worktrees/` are real checkouts and a build inside one
# grows its own `target/`, which is invisible from the root and therefore
# the kind of thing that is discovered when the disk is already full.
#
# Three tiers, cheapest first. The default is the one that costs nothing
# but a slightly slower next build.
#
#   ./clean-build-artifacts.sh              # incremental cache only
#   ./clean-build-artifacts.sh --stale      # ... plus artifacts older than N days
#   ./clean-build-artifacts.sh --deep       # cargo clean: everything, full rebuild
#   ./clean-build-artifacts.sh --dry-run    # measure, delete nothing
#
#   --days N   how old an artifact must be for --stale (default 3)
#
# It never touches source, and it cannot: every path it deletes is under a
# `target/` directory, which is git-ignored and holds no tracked file.
set -euo pipefail

cd "$(dirname "$0")"

MODE=incremental
DRY=0
DAYS=3
while [ $# -gt 0 ]; do
  case "$1" in
    --stale)   MODE=stale ;;
    --deep)    MODE=deep ;;
    --dry-run) DRY=1 ;;
    --days)    shift; DAYS="${1:?--days needs a number}" ;;
    -h|--help) awk 'NR>1 && /^#/ { sub(/^# ?/, ""); print; next } NR>1 { exit }' "$0"; exit 0 ;;
    *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
  esac
  shift
done

# **Refuse while a build is running.** Deleting `deps` or `.fingerprint`
# under a live rustc gives you a corrupt-looking tree and a confusing
# rebuild, and the person who runs this is usually the person who just
# started a long test run in another pane.
if pgrep -x rustc >/dev/null 2>&1 || pgrep -x cargo >/dev/null 2>&1; then
  echo "a cargo or rustc process is running; refusing to delete underneath it" >&2
  echo "wait for it to finish, or stop it, then run this again" >&2
  exit 1
fi

# Sizes in KiB throughout, rendered once at the edge.
kib_of_dir() { du -sk "$1" 2>/dev/null | cut -f1 || echo 0; }
# The size of a matched set, summed from `find -ls` rather than by running
# `du` per file: 70,000 files is a lot of processes to fork.
kib_of_match() {
  find "$1" -mindepth 1 -mtime "+$DAYS" -type f -ls 2>/dev/null |
    awk '{ s += $7 } END { printf "%d\n", s/1024 }'
}
human() { awk -v k="${1:-0}" 'BEGIN { if (k >= 1048576) printf "%.1fG\n", k/1048576; else if (k >= 1024) printf "%.0fM\n", k/1024; else printf "%dK\n", k }'; }

# Every worktree, this one first. `git worktree list` is the authority
# rather than a glob, so a worktree somewhere unexpected is still visited
# and one that was pruned is not.
roots() {
  if git rev-parse --git-dir >/dev/null 2>&1; then
    git worktree list --porcelain 2>/dev/null | awk '/^worktree /{ print $2 }'
  else
    pwd
  fi
}

TOTAL_BEFORE=0
TOTAL_AFTER=0
VISITED=0
WITH_TARGET=0

[ "$DRY" = 1 ] && echo "dry run: measuring only, nothing will be deleted" && echo

for root in $(roots); do
  VISITED=$((VISITED + 1))
  [ -d "$root/target" ] || continue
  WITH_TARGET=$((WITH_TARGET + 1))

  before=$(kib_of_dir "$root/target")
  TOTAL_BEFORE=$((TOTAL_BEFORE + before))
  echo "$root"
  echo "  target/ uses $(human "$before")"

  case "$MODE" in
    incremental|stale)
      for d in "$root/target/debug/incremental" "$root/target/release/incremental"; do
        [ -d "$d" ] || continue
        echo "    incremental cache uses $(human "$(kib_of_dir "$d")")"
        [ "$DRY" = 1 ] || rm -rf "$d"
      done
      if [ "$MODE" = stale ]; then
        # `cargo-sweep` knows which artifacts the current toolchain still
        # considers live, so prefer it when it is installed.
        if cargo sweep --version >/dev/null 2>&1; then
          echo "    stale artifacts: cargo sweep --time $DAYS"
          [ "$DRY" = 1 ] || ( cd "$root" && cargo sweep --time "$DAYS" >/dev/null )
        else
          # Fallback: age. This can delete something cargo still considered
          # fresh, which costs a rebuild of that crate and nothing more.
          for d in "$root/target/debug/deps" "$root/target/debug/.fingerprint" \
                   "$root/target/release/deps" "$root/target/release/.fingerprint"; do
            [ -d "$d" ] || continue
            n=$(find "$d" -mindepth 1 -mtime "+$DAYS" 2>/dev/null | wc -l | tr -d ' ')
            [ "$n" = 0 ] && continue
            echo "    ${d#"$root"/}: $n entries older than ${DAYS}d use $(human "$(kib_of_match "$d")")"
            [ "$DRY" = 1 ] || find "$d" -mindepth 1 -mtime "+$DAYS" -delete 2>/dev/null || true
          done
        fi
      fi
      ;;
    deep)
      # Everything. Correct, and the next build is cold: on this workspace
      # that is tens of minutes.
      echo "    cargo clean: all $(human "$before"), next build is cold"
      [ "$DRY" = 1 ] || ( cd "$root" && cargo clean )
      ;;
  esac

  after=$(kib_of_dir "$root/target")
  TOTAL_AFTER=$((TOTAL_AFTER + after))
  [ "$DRY" = 0 ] && echo "  target/ now uses $(human "$after"), freed $(human "$((before - after))")"
  echo
done

echo "$VISITED worktree(s) visited, $WITH_TARGET with a target/"
echo "build artifacts used $(human "$TOTAL_BEFORE")"
if [ "$DRY" = 0 ]; then
  echo "build artifacts now use $(human "$TOTAL_AFTER")"
  echo "freed $(human "$((TOTAL_BEFORE - TOTAL_AFTER))")"
fi
df -h . | tail -1
