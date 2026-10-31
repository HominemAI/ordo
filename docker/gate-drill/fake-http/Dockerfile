# Deliberately fake fixture for the gate drill
# (.github/workflows/gate-drill.yml). Never built, tagged, or pushed outside
# that workflow, and unrelated to docker/Dockerfile (whose `host` target
# replaced docker/Dockerfile.host) or
# docker/Dockerfile.playground, which are untouched.
#
# It answers configurable HTTP statuses (FAKE_ROUTES) and prints configurable
# boot log lines (FAKE_LOG_LINES) so the drill can feed `images.yml`'s host
# and playground smoke assertions the exact broken responses those gates are
# built to catch -- 404/200 on /health/deep, an unexpected code on
# /health/ready, an UNAVAILABLE boot line or no seeded line at all -- without
# rebuilding or patching the real Rust services. See fake_server.py.
FROM python:3-alpine
COPY fake_server.py /fake_server.py
ENTRYPOINT ["python3", "/fake_server.py"]
