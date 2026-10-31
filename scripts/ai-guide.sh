#!/bin/sh
# Rebuild docs/ai-guide.md from skills/ordo/*.md, then check every example.
# The order of the files is PARTS in crates/ordo-cli/tests/ai_guide_check.rs.
set -eu
cd "$(dirname "$0")/.."
ORDO_BLESS_AI_GUIDE=1 cargo test -p ordo-cli --test ai_guide_check
