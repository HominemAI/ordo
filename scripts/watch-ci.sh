#!/bin/bash
# Follow the CI run for one commit to the end, then report its step timings,
# its totals and its failures.
#
#   scripts/watch-ci.sh [sha]     any prefix; defaults to HEAD
#
# The run is found by matching the `headSha` prefix, because
# `gh run list --commit <short sha>` returns nothing.
set -uo pipefail
cd "$(git rev-parse --show-toplevel)" || exit 1
sha="${1:-$(git rev-parse HEAD)}"

id=""
for _ in $(seq 1 60); do
  # The test workflow only: since 2026-09-19 `Images` runs on the same
  # commit too, and it has no test results to report.
  id=$(gh run list --workflow ci.yml --limit 10 --json databaseId,headSha \
        -q ".[] | select(.headSha | startswith(\"$sha\")) | .databaseId" | head -1)
  [ -n "$id" ] && break
  sleep 5
done
[ -z "$id" ] && { echo "no run appeared for $sha"; exit 1; }
echo "run $id"

gh run watch "$id" --interval 30 >/dev/null 2>&1
gh run view "$id" --json status,conclusion,createdAt,updatedAt,jobs \
  -q '"result: \(.status)/\(.conclusion)  \(.createdAt[11:19]) to \(.updatedAt[11:19])", (.jobs[] | .steps[] | select(.startedAt != null) | "  \(.name): \(.startedAt[11:19]) to \((.completedAt // "")[11:19]) \(.conclusion // "")")'

log=$(mktemp)
gh run view "$id" --log > "$log" 2>/dev/null

# Python, not sed: the log lines carry a job/step/timestamp prefix separated
# by tabs, and BSD sed does not treat `\t` as a tab.
python3 - "$log" <<'PY'
import re, sys

strip = re.compile(r'^[^\t]*\t[^\t]*\t[0-9T:.\-]+Z ')
ansi = re.compile(r'\x1b\[[0-9;]*m')
total_passed = total_failed = results = 0
seen, failures, panics = set(), [], []

for raw in open(sys.argv[1], errors='replace'):
    line = ansi.sub('', strip.sub('', raw.rstrip('\n')))
    m = re.search(r'test result: \w+\. (\d+) passed; (\d+) failed', line)
    if m:
        results += 1
        total_passed += int(m.group(1))
        total_failed += int(m.group(2))
    # A panic line is not by itself a failure. A passing test can assert that
    # a debug guard fires and catch the unwind, which is exactly what
    # `a_space_scope_entry_carries_no_target` does. So panics are held back
    # and shown only when something actually failed, or a green run prints
    # failure-shaped output.
    if re.search(r'\.\.\. FAILED|^error: |to rerun pass', line):
        bucket = failures
    elif 'panicked at' in line:
        bucket = panics
    else:
        continue
    if line not in seen:
        seen.add(line)
        bucket.append(line)

print(f'test result lines: {results}')
print(f'passed {total_passed} failed {total_failed}')
for line in failures[:40]:
    print(line)
if failures or total_failed:
    for line in panics[:40]:
        print(line)
PY
rm -f "$log"
