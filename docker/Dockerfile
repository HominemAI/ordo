# The one image, `ordo` (`docs/plan-console.md` 9.1, ruled 2026-10-23;
# `docs/plan-identity-service.md` 3.3): `ordo-supervisor` (the entrypoint),
# `ordo-identity-service`, `ordo-console`, `ordo-host` and the `ordo` CLI,
# with the console's built page. The identity service's page is built into
# its binary.
#
#   docker build -f docker/Dockerfile -t ordo:dev .
#
# The supervisor runs 3 children as siblings, none another's parent
# (`plan-identity-service.md` 3.1). It moved out of the console, because
# "if the thing that manages the thing depends on the thing its managing its
# no bueno" (owner, 2026-10-27). Each child starts through the supervisor's
# own binary in launcher mode, with a cleared environment and its secrets as
# files under /run/ordo/<child>.
#
# Four unix users. The supervisor runs as 65530; the identity service as
# 65531, the console as 65532, the deployment (`ordo-host`) as 65533
# (ORDO_SUPERVISOR_*_USER). No child can read another's volume, secrets or
# memory. The supervisor binary carries four file capabilities, permitted
# and not effective: CAP_SETUID and CAP_SETGID (the launcher raises them
# only to switch), CAP_SETPCAP (it empties the capability bounding set) and
# CAP_CHOWN (the supervisor gives each secret file to its child's user, and
# the socket to the console's group). No CAP_KILL: a child of another user
# is started under a relay, its launcher, which runs as that user and
# signals it when the supervisor asks, so a compose file made before the
# one image, which grants no KILL, still stops cleanly. The launcher drops
# every capability and sets no-new-privileges before it starts the child.
# The console binary carries none any more. Docker grants all four by
# default; the least a platform can grant is
#   --cap-drop ALL --cap-add SETUID --cap-add SETGID --cap-add SETPCAP --cap-add CHOWN
# The supervisor binary is root:65530 mode 0750, so no child's user can run
# it at all. The launcher also refuses any caller but the supervisor, root
# as a target, and the supervisor's own identity as a target. Without all
# four (Kubernetes' restricted profile drops ALL) the launcher cannot switch
# users, and the supervisor refuses to start, naming the setting: it never
# runs a child as its own user while that child's setting is given.
#
# The console still manages the deployment (restart, settings, logs) by
# asking the supervisor over /run/ordo/supervisor.sock, mode 0660, group
# 65532 (ORDO_CONSOLE_SUPERVISOR_SOCKET, set by the supervisor).
#
# The separate deployment-only and console-only images are no longer
# something Ordo provides (ruled 2026-10-27). The binaries stay in this
# image and in the release's binary artifacts, so a deployment owner can
# build an image of their own.

ARG RUST_VERSION=1.97
ARG DEBIAN_RELEASE=bookworm
ARG NODE_VERSION=20.18.1

# ── the page ─────────────────────────────────────────────────────────────
#
# Plain React and Vite (`clients/ordo-console-web`). `npm ci` installs the
# exact versions in its lockfile. Only the page's own files are copied, so a
# local node_modules or dist never enters the build.

FROM node:${NODE_VERSION}-${DEBIAN_RELEASE}-slim AS page
WORKDIR /page
COPY clients/ordo-console-web/package.json clients/ordo-console-web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY clients/ordo-console-web/index.html clients/ordo-console-web/tsconfig.json clients/ordo-console-web/vite.config.ts ./
COPY clients/ordo-console-web/src ./src
RUN npm run build

# ── the binaries ─────────────────────────────────────────────────────────
#
# The same cargo-chef layering as `Dockerfile.cli`.

FROM rust:${RUST_VERSION}-slim-${DEBIAN_RELEASE} AS chef
RUN cargo install cargo-chef --locked --version ^0.1
WORKDIR /build

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
# `ordo-setcap` writes the supervisor's file capabilities below. It is a
# one-file program built with the stage's own rustc (docker/setcap.rs), so
# the build fetches no Debian package: CI's build containers could not
# reach the Debian mirrors on 2026-10-31.
COPY docker/setcap.rs /tmp/setcap.rs
RUN rustc -O -o /usr/local/bin/ordo-setcap /tmp/setcap.rs
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json
COPY . .
# The last two lines make the 3 volumes and the run folder, each given to
# its user in the runtime stage: distroless has no shell to make them at
# run time. `setcap` comes after `strip`, which rewrites the file and its
# attributes, and after `chown` and `chmod`, since a chown clears file
# capabilities. COPY --from keeps the owner, mode and capabilities, so the
# runtime stage copies the supervisor plainly; a --chown or --chmod there
# would drop them. Each volume is 0700, so no other child can list or read
# it; the run folder is 0711, so a child reaches only the secret files it is
# given.
RUN cargo build --release --locked \
      -p ordo-supervisor --bin ordo-supervisor \
      -p ordo-identity-service --bin ordo-identity-service \
      -p ordo-console --bin ordo-console \
      -p ordo-host --bin ordo-host \
      -p ordo-cli --bin ordo \
 && strip target/release/ordo-supervisor target/release/ordo-identity-service \
      target/release/ordo-console target/release/ordo-host target/release/ordo \
 && chown 0:65530 target/release/ordo-supervisor \
 && chmod 0750 target/release/ordo-supervisor \
 && ordo-setcap set target/release/ordo-supervisor \
 && ordo-setcap check target/release/ordo-supervisor \
 && ordo-setcap none target/release/ordo-console \
 && mkdir -p -m 0700 /out/var/lib/ordo-supervisor /out/var/lib/ordo-console /out/var/lib/ordo-identity \
 && mkdir -p -m 0711 /out/run/ordo \
 && mkdir -p -m 0700 /out/var/cache/ordo-fn

# ── runtime: the one image ───────────────────────────────────────────────
#
# distroless/cc: pure-Rust crypto and `tokio-postgres` without TLS, so
# glibc and libgcc_s are the whole requirement.

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

USER 65530:65530

COPY --from=builder /build/target/release/ordo-supervisor /usr/local/bin/ordo-supervisor
COPY --from=builder /build/target/release/ordo-identity-service /usr/local/bin/ordo-identity-service
COPY --from=builder /build/target/release/ordo-console /usr/local/bin/ordo-console
COPY --from=builder /build/target/release/ordo-host /usr/local/bin/ordo-host
COPY --from=builder /build/target/release/ordo /usr/local/bin/ordo
COPY --from=builder --chown=65530:65530 /out/var/lib/ordo-supervisor /var/lib/ordo-supervisor
COPY --from=builder --chown=65531:65531 /out/var/lib/ordo-identity /var/lib/ordo-identity
COPY --from=builder --chown=65532:65532 /out/var/lib/ordo-console /var/lib/ordo-console
COPY --from=builder --chown=65530:65530 /out/run/ordo /run/ordo
# The compiled function engine, cached so a restart skips its compile.
# ordo-host (65533) owns it; the cache refuses a directory it does not own.
COPY --from=builder --chown=65533:65533 /out/var/cache/ordo-fn /var/cache/ordo-fn
ENV ORDO_FN_CACHE_DIR=/var/cache/ordo-fn
COPY --from=page /page/dist /usr/share/ordo-console/page

# The settings with no default, each refused at boot by name: the console's
# ORDO_CONSOLE_DATABASE_URL_FILE (its own Postgres database) and
# ORDO_CONSOLE_ORIGIN (the address its page is opened at), and the identity
# service's ORDO_IDENTITY_DATABASE_URL_FILE (its own database, on the
# console's cluster in compose.one.yml). Every other identity setting it
# needs the supervisor fills from the console's when unset
# (`plan-identity-service.md` 3.1): ORDO_IDENTITY_ORIGIN from the console's
# origin with port 8082, ORDO_IDENTITY_RP_ID from the console's RP ID, so a
# passkey made at the console still signs in, and the import of the
# console's owners from the console's database.
#
# The listen addresses are every interface: inside a container, loopback is
# unreachable from outside it. Each binary's own default stays loopback.
#
# ORDO_HOST_ADDR is the deployment's: revision 1 is built from the ORDO_*
# variables the container starts with that are no other child's, and after
# that the supervisor's volume holds the live revision
# (`plan-identity-service.md` 3.2).
#
# /run/ordo holds the socket and each child's secret files. Mount a tmpfs
# there owned by the supervisor, for example
#   --tmpfs /run/ordo:uid=65530,mode=0711
# so a secret never reaches a disk. ORDO_SUPERVISOR_RUN_DIR is not set
# here on purpose: unset, the supervisor also honours the console's old
# ORDO_CONSOLE_RUN_DIR, and takes /run/ordo-console when only that one is
# usable, which is where a compose file from before this image mounts its
# tmpfs.
#
# The identity service is reached under the console's own origin at
# /identity by default (ORDO_SUPERVISOR_IDENTITY_ADDRESS=console-path), so
# 8082 need not be published; with `own-port` it is.
#
# The child users need no entry in /etc/passwd. A volume the deployment
# itself writes must be writable by 65533.
ENV ORDO_SUPERVISOR_IDENTITY_USER="65531:65531" \
    ORDO_SUPERVISOR_CONSOLE_USER="65532:65532" \
    ORDO_SUPERVISOR_DEPLOYMENT_USER="65533:65533" \
    ORDO_SUPERVISOR_STATE_DIR="/var/lib/ordo-supervisor" \
    ORDO_IDENTITY_LISTEN="0.0.0.0:8082" \
    ORDO_IDENTITY_DATA_DIR="/var/lib/ordo-identity" \
    ORDO_CONSOLE_LISTEN="0.0.0.0:8081" \
    ORDO_CONSOLE_DATA_DIR="/var/lib/ordo-console" \
    ORDO_CONSOLE_PAGE_DIR="/usr/share/ordo-console/page" \
    ORDO_HOST_ADDR="0.0.0.0:8080"

# Mount a volume on each, and back them up with the databases:
# - /var/lib/ordo-identity holds the identity service's sealing key
#   (ORDO_IDENTITY_KEY_FILE): its signing keys do not open without it.
# - /var/lib/ordo-console holds the console's sealing key (console.key).
# - /var/lib/ordo-supervisor holds the deployment's live revision, sealed,
#   and the key that opens it.
VOLUME ["/var/lib/ordo-identity", "/var/lib/ordo-console", "/var/lib/ordo-supervisor"]

# 8080 the deployment's door, 8081 the console, 8082 the identity service.
EXPOSE 8080 8081 8082

# Probes: the supervisor's /health/live on 127.0.0.1:8079, and each
# child's own /health/live and /health/ready on its port. No HEALTHCHECK:
# distroless has no shell or curl, and probes belong to the orchestrator.

ENTRYPOINT ["/usr/local/bin/ordo-supervisor"]
