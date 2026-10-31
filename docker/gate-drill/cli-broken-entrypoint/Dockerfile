# Deliberately broken fixture for the gate drill
# (.github/workflows/gate-drill.yml). Never built, tagged, or pushed outside
# that workflow, and unrelated to docker/Dockerfile.cli, which is untouched.
#
# It stands in for "an image whose bare invocation does not exit 2" — the
# specific defect the usage-exit check in `images.yml` (plan-ci-images.md
# §3.3) exists to catch: an ENTRYPOINT that is a shell, or otherwise
# swallows its arguments, so a caller who ran the image with no arguments
# gets something other than the documented usage exit. `/bin/true` always
# exits 0 regardless of arguments, which is the simplest image that fails
# this one gate and nothing else — it does not attempt to also stand in for
# ordo-cli's `check` smoke, which is a different assertion.
FROM busybox:stable
ENTRYPOINT ["/bin/true"]
