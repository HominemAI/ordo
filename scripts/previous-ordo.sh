#!/usr/bin/env bash
# Build the previous Ordo release's `ordo-host` and `ordo`, for the notes e2e
# case with 2 images (examples/notes/e2e/playwright.ordo-upgrade.config.ts).
#
#     eval "$(bash scripts/previous-ordo.sh)"
#
# The previous release is the newest `v*` tag that does not point at HEAD, so
# on a release tag it is the one before (`E2E_PREVIOUS_TAG` names another). Its source is exported with `git archive` and built into a target
# folder of its own, by scripts/upgrade-continuity.sh `build`, so this
# checkout's build is not touched. It needs the tags (CI checks out with
# fetch-depth 0) and no secret and no registry login: the published image is
# private, and the release's tarball holds the `ordo` CLI alone, so the tag
# is built from source, the way upgrade_continuity.rs's previous Ordo is.
#
# Prints, on stdout, the `export` lines the stack reads (E2E_PREVIOUS_HOST,
# E2E_PREVIOUS_ORDO, E2E_PREVIOUS_RELEASE). Progress goes to stderr. In CI the
# same lines are appended to $GITHUB_ENV as well.
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
tag=${E2E_PREVIOUS_TAG:-}
if [ -z "$tag" ]; then
  # The newest tag that does not point at HEAD: on a release tag, the one before.
  head_commit=$(git -C "$repo" rev-parse HEAD)
  for t in $(git -C "$repo" tag --list 'v[0-9]*' --sort=-v:refname); do
    if [ "$(git -C "$repo" rev-parse "$t^{commit}")" != "$head_commit" ]; then
      tag=$t
      break
    fi
  done
fi
[ -n "$tag" ] || { echo "previous-ordo: no v* tag here; fetch the tags (git fetch --tags)" >&2; exit 1; }
work=${ORDO_UPGRADE_DIR:-$HOME/.ordo-upgrade-$tag}

ORDO_UPGRADE_FROM=$tag ORDO_UPGRADE_DIR=$work ORDO_UPGRADE_OWN_PG=0 \
  bash "$repo/scripts/upgrade-continuity.sh" build >&2

lines="export E2E_PREVIOUS_HOST=$work/target/debug/ordo-host
export E2E_PREVIOUS_ORDO=$work/target/debug/ordo
export E2E_PREVIOUS_RELEASE=$tag"
echo "$lines"
if [ -n "${GITHUB_ENV:-}" ]; then
  echo "$lines" | sed 's/^export //' >>"$GITHUB_ENV"
fi
