#!/usr/bin/env bash
# Install cargo-nextest from its release, pinned by version and checksum.
#
# The binary goes in $RUNNER_TEMP/nextest-bin (or $TMPDIR outside Actions),
# which is added to $GITHUB_PATH.
#
# To move to a newer release: change VERSION, and take each SHA256 from the
# release's assets (each `.tar.gz` lists its digest on the release page).
set -euo pipefail

VERSION=0.9.148

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)
    target=x86_64-unknown-linux-gnu
    sha=223ba936714cc861a9cdedad5a8eb664dfdd69d6fbdf6cea6aa8184f523290dc
    ;;
  Linux-aarch64)
    target=aarch64-unknown-linux-gnu
    sha=81469eb963ef883fd590e60189e77d6aba3d91b418591d3391f50fe3e9480d2b
    ;;
  Darwin-*)
    target=universal-apple-darwin
    sha=6c23b7fb4ca82c571fc8dfedbab45bfa773d45918f933170c85d21cd7f4b852b
    ;;
  *)
    echo "No pinned cargo-nextest for $(uname -s) $(uname -m)." >&2
    exit 1
    ;;
esac

dir=${RUNNER_TEMP:-${TMPDIR:-/tmp}}/nextest-bin
mkdir -p "$dir"
tarball=$dir/cargo-nextest.tar.gz
url=https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-$VERSION/cargo-nextest-$VERSION-$target.tar.gz

curl -fsSL --retry 5 --retry-delay 5 -o "$tarball" "$url"
got=$(sha256sum "$tarball" 2>/dev/null || shasum -a 256 "$tarball")
got=${got%% *}
if [ "$got" != "$sha" ]; then
  echo "cargo-nextest $VERSION ($target): checksum $got, expected $sha." >&2
  exit 1
fi
tar xzf "$tarball" -C "$dir"
rm "$tarball"
"$dir/cargo-nextest" nextest --version

if [ -n "${GITHUB_PATH:-}" ]; then
  echo "$dir" >>"$GITHUB_PATH"
else
  echo "Installed in $dir; add it to PATH."
fi
