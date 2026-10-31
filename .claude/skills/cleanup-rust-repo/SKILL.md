---
name: cleanup-rust-repo
description: Reclaim disk from Rust build artifacts (./clean-build-artifacts.sh) and from the Postgres test clusters agents leave behind (scripts/clean-test-clusters.sh). Use when the disk is filling, when target/ has grown large, when a build or test run fails on no space left on device, after parallel agents finish, or when asked to clean up the repo.
---

# Reclaiming disk from `target/`

Cargo never garbage-collects. Every distinct hash of every test binary
stays in `target/debug/deps` forever, and the incremental cache grows with
each edit. On this workspace that reached **65G and 457,000 files in four
days**, with the disk at 90%.

`./clean-build-artifacts.sh` at the repo root does the reclaiming. Three
tiers, cheapest first.

## Which tier

| Run | Frees | Costs |
|---|---|---|
| `./clean-build-artifacts.sh` | the incremental cache, usually half the tree | one slower build of whatever you edit next |
| `./clean-build-artifacts.sh --stale` | that, plus artifacts older than 3 days (`--days N` to change) | a rebuild of whatever was swept |
| `./clean-build-artifacts.sh --deep` | everything, via `cargo clean` | a **cold** rebuild, tens of minutes here |

`--dry-run` measures and deletes nothing. Start there when you are not
sure which tier is wanted.

**Default to the first tier unless asked otherwise.** It is the one that
cannot cost anybody anything they will notice, and on this workspace it
has returned 23G on its own.

**`--deep` is the user's call, not Claude's.** A cold rebuild here is tens
of minutes, and the person who asked for disk back may have a suite
queued behind it. Offer it; do not reach for it.

## Before running it

The script refuses while `cargo` or `rustc` is running, because deleting
`deps` or `.fingerprint` under a live build produces a corrupt-looking
tree and a confusing rebuild. If it refuses, something is compiling in
another pane. Wait rather than overriding it.

**Check for orphans first if the refusal is a surprise**: `ps aux | grep
-E "[r]ustc|[c]argo"`. A build that lost its parent can sit for hours, and
this repository has had one burn CPU for 29 hours.

## Always check all three places

A disk that is filling has three usual causes here, and on 2026-09-29 all
three were present at once, with 5.2G free:

| Where | What grows | Seen |
|---|---|---|
| `target/` at the root | `deps` and the incremental cache | 89G (28G incremental) |
| an agent worktree's `target/` | the same, per worktree | 8 to 9G each |
| `~/.ordo-pg/<name>` | one Postgres test cluster per agent | 2 to 9G each, ten of them: 57G |

Measure first, all at once:

```
df -h .
./clean-build-artifacts.sh --dry-run
scripts/clean-test-clusters.sh
ps -axo pid,etime,command | grep -E "[c]argo|[r]ustc|[p]ostgres -D"
```

`df -h /` shows the small system volume on macOS; `df -h .` from the repo
shows the data volume, which is the one that fills.

## Test clusters

Every agent that runs tests starts its own cluster
(`ORDO_PGDATA=$HOME/.ordo-pg/<name>` on its own port), so that parallel
agents never share test databases. A test run creates many databases, since
a tenant is a database, and **a finished agent leaves its cluster on disk,
sometimes still running**.

- `scripts/clean-test-clusters.sh` reports each cluster, its size, and
  whether it is running, plus any orphan postgres whose directory is gone.
- `scripts/clean-test-clusters.sh --remove` deletes the **stopped** ones
  under `~/.ordo-pg`. It never touches a running cluster or the default one.
- A running cluster may belong to an agent still working. Check the agents
  before stopping one; a cluster whose agent has reported back can be
  stopped with its own variables (see the script's header) and then removed.
- It is not blocked by a running build, unlike `clean-build-artifacts.sh`,
  so it works while agents compile.

**When launching test-running agents, tell each to stop its cluster when it
finishes.** Most did on 2026-09-29; one left its cluster running.

## A worktree's `target/`

A finished agent's worktree holds a full `target/`. Removing the worktree
(`git worktree remove`, after its branch is merged or its work committed)
removes that too. That is the cheapest way to free 8 to 9G per agent, and it
is the user's call only when the worktree may hold uncommitted work.

## What it will not touch

Source, `.git`, and the Postgres test clusters. Those are
`scripts/clean-test-clusters.sh`'s, above. The default cluster lives in
`$TMPDIR` and has its own failure mode; see `scripts/conformance-pg.sh` and
note that macOS reaps it independently.

## It walks every worktree

`git worktree list` is the authority, not a glob over
`.claude/worktrees/`. An agent worktree is a real checkout, and a build
inside one grows its own `target/` that is invisible from the root, which
is the kind of thing found when the disk is already full. The script
reports how many worktrees it visited and how many had a `target/`, so a
run that says `19 worktree(s) visited, 1 with a target/` has told you
both that it looked and that it found nothing hiding.

**It does not delete worktrees.** That is a different and riskier act: a
dead agent worktree can hold uncommitted work, and the rule for those is
to commit in place and merge rather than discard. Pruning them is the
user's call after checking `git status` in each.

## Reporting back

Quote the script's own numbers rather than describing them. It prints,
per item, **how much that item uses** before deleting it, then **how much
was freed**, then the totals and `df`. So a report should carry:

- what each item used (`target/debug/deps: 72066 entries older than 3d
  use 12.2G`),
- the total before and after,
- what was freed, and
- where the disk landed.

The point of the exercise is the number, so give it.

## After a `--stale` or `--deep` run

Run `cargo build --workspace` once before reporting success. A sweep that
deleted something cargo still wanted costs a rebuild rather than
correctness, and confirming the rebuild works is what turns "I deleted
12G" into "I deleted 12G and the tree still builds".
