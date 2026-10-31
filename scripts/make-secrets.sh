#!/usr/bin/env bash
# Make every secret file the compose files read, once.
#
#   scripts/make-secrets.sh one     # docker/compose.one.yml
#
# Options (after the layout):
#   --dir PATH    where to write (default: docker/secrets in this repository)
#   --no-group    skip chgrp and chown (a host without sudo, or a test)
#
# It never overwrites. If some of the files exist and others do not, it
# refuses and changes nothing. For `one`, a folder made before the identity
# service (all 8 older files there, the identity service's 2 missing) is
# upgraded: it adds those 2, and gives console-database-url to the
# supervisor (65530) with the console's group (65532), mode 0440, so the
# supervisor can hand the identity service a copy for the import. Nothing
# else is touched. That step is optional: an upgraded deployment that
# keeps its old compose file needs none of it (docs/howto-console.md).
# `ordo` must be on PATH (or set ORDO=/path/to/ordo) to seal the master key.
# Guide: docs/first-deployment.md
set -euo pipefail

usage() { sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }

layout="${1:-}"
[ -n "$layout" ] || usage
shift
case "$layout" in
  one) app_gid=65533 ;;   # the one image runs the deployment as 65533
  *) usage ;;
esac

here="$(cd "$(dirname "$0")/.." && pwd)"
dir="$here/docker/secrets"
set_group=1
while [ $# -gt 0 ]; do
  case "$1" in
    --dir) dir="${2:?--dir needs a path}"; shift 2 ;;
    --no-group) set_group=0; shift ;;
    *) usage ;;
  esac
done

ordo_bin="${ORDO:-ordo}"
command -v openssl >/dev/null || { echo "make-secrets: openssl is not installed. Install it and run again." >&2; exit 1; }
command -v "$ordo_bin" >/dev/null || { echo "make-secrets: the 'ordo' tool is not on PATH. Install it, or set ORDO=/path/to/ordo." >&2; exit 1; }

files=(token-key page-key deployment-db-password console-db-password master-passphrase master.key console-database-url deployment-db-password-app)
# The identity service's own database role (`plan-identity-service.md` 2.2).
identity_files=(identity-db-password identity-database-url)
mkdir -p "$dir"
existing=()
for f in "${files[@]}"; do [ -e "$dir/$f" ] && existing+=("$f"); done
as_root() { if [ "$(id -u)" = 0 ]; then "$@"; else sudo "$@"; fi; }
make_identity() {
  openssl rand -hex 24 > identity-db-password
  printf 'postgres://identity:%s@console-db:5432/ordo_identity' "$(cat identity-db-password)" > identity-database-url
  chmod 0440 identity-db-password identity-database-url
  if [ "$set_group" = 1 ]; then
    as_root chgrp 999 identity-db-password
    as_root chown 65530:65530 identity-database-url
    as_root chmod 0400 identity-database-url
  fi
}
console_url_for_supervisor() {
  if [ "$set_group" = 1 ]; then
    as_root chown 65530:65532 console-database-url
    as_root chmod 0440 console-database-url
  fi
}
if [ ${#existing[@]} -eq ${#files[@]} ] && [ ${#identity_files[@]} -gt 0 ]; then
  missing=()
  for f in ${identity_files[@]+"${identity_files[@]}"}; do [ -e "$dir/$f" ] || missing+=("$f"); done
  if [ ${#missing[@]} -eq ${#identity_files[@]} ]; then
    umask 077
    cd "$dir"
    make_identity
    console_url_for_supervisor
    echo "Added the identity service's 2 files to $dir:"
    for f in ${identity_files[@]+"${identity_files[@]}"}; do printf '  %s  %s\n' "$(ls -l "$f" | cut -c1-10)" "$f"; done
    echo "Every other file is as it was. Next: docs/first-deployment.md"
    exit 0
  fi
fi
for f in ${identity_files[@]+"${identity_files[@]}"}; do [ -e "$dir/$f" ] && existing+=("$f"); done
if [ ${#existing[@]} -gt 0 ]; then
  echo "make-secrets: refusing. These files already exist in $dir:" >&2
  printf '  %s\n' "${existing[@]}" >&2
  echo "Nothing was changed. Move them aside or delete them on purpose, then run again." >&2
  exit 1
fi

umask 077
cd "$dir"
openssl rand -hex 32 > token-key
openssl rand -hex 32 > page-key
openssl rand -hex 24 > deployment-db-password
openssl rand -hex 24 > console-db-password
openssl rand -hex 32 > master-passphrase
"$ordo_bin" keys seal-master --out master.key --passphrase-file master-passphrase >/dev/null
printf 'postgres://console:%s@console-db:5432/ordo_console' "$(cat console-db-password)" > console-database-url
cp deployment-db-password deployment-db-password-app

chmod 0440 "${files[@]}"
if [ "$set_group" = 1 ]; then
  as_root chgrp "$app_gid" token-key page-key master.key master-passphrase deployment-db-password-app
  as_root chgrp 999 deployment-db-password console-db-password
  console_url_for_supervisor
fi
[ ${#identity_files[@]} -gt 0 ] && make_identity

echo "Made $(( ${#files[@]} + ${#identity_files[@]} )) secret files in $dir:"
for f in "${files[@]}" ${identity_files[@]+"${identity_files[@]}"}; do printf '  %s  %s\n' "$(ls -l "$f" | cut -c1-10)" "$f"; done
cat <<EOF

Back up master.key and master-passphrase together, somewhere off this host.
Lose either and the data they seal cannot be opened. Keep them apart from
your other backups. Keep $dir out of git.
The other files can be made again, but the database passwords must
match the databases that already hold them.
Next: docs/first-deployment.md
EOF
