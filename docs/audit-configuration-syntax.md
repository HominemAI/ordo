# Audit: configuration the `.ordo` syntax cannot express

Date: 2026-11-13. Asked by the owner: "I also want an audit of anything missing from ordo syntax in terms of configuration."

This audit lists what a deployment owner sets outside the `.ordo` file today. For each group of settings it judges whether the file should be able to say it. Sources: `docs/settings.md`, `docs/definition-language.md` section 8, the `ORDO_` strings in `crates/`, and the console screens `Settings.tsx` and `SignIn.tsx`. The rules are in `docs/corpus-rulings.md` under `settings-layers` (24a to 24d, Ordo v2): layers are deployment, app developer, tenant, person, with secure defaults.

## What the file can say today

- Built: `domain`, `entity`, `op`, `routine`, `function`, and `secret <name> for <host>`.
- Reserved, and refused by the parser with "reserved and not implemented": `auth`, `policy`, `client`, `principal`, `channel`, `route`, `app`, `page` (`crates/ordo-syntax/src/declparse.rs`, `UNBUILT_DECLARATIONS`).
- So every sign-in, session, rate, retention, mail and sharing choice lives outside the file. Where an app developer layer exists, it is a platform op such as `std.deployment.setSessionPolicy`, not syntax.

## Count

- `docs/settings.md` rows with an `ORDO_` name: about 330 distinct names (333 including patterns).
- Names in `crates/` strings: 439 distinct, including test fakes, console, identity service and supervisor settings, and name prefixes.
- The 302 names in `ordo_host::settings::REGISTRY` are the ones the console can list.
- The table groups these into 47 rows. A row stands for every setting in its family.

## Table

Layer is where the owner's principle puts the choice. "Where today" is env unless stated. Console means the Settings screen edits it as a runtime revision.

| Setting | Where today | Layer | In the file? | Recommendation |
|---|---|---|---|---|
| `ORDO_AUTH_METHODS` (passkey, password, email code, providers) | env, console | app developer | no | Declare in `auth { methods ... }`. Deployment owner can only narrow it |
| `ORDO_AUTH_PROVISIONING`, `ORDO_PROVISION_SELF_SERVE`, signup rates | env, console | app developer | no | Declare provisioning mode in `auth`. Rates stay deployment ceilings |
| `ORDO_AUTH_GUEST`, `_GUEST_TTL_SECS` | env, console | app developer | no | Declare in `auth`. App decides if guests exist |
| `ORDO_AUTH_LINKING`, `_LINKING_SECS` | env, console | app developer | no | Declare in `auth` |
| `ORDO_AUTH_SECOND_FACTOR*`, `_SECOND_STEP_*` | env, console | app developer | no | Declare "second factor: required, optional, off" in `auth`. Attempt limits stay env |
| `ORDO_AUTH_REAUTH_SECS`, `ORDO_STEP_UP_REQUIRED_OPS`, `_EXEMPT_OPS`, `ORDO_PRESENCE_ON_READS` | env, console | app developer | partly | Step-up is per op in the design (`client` block adds). Give the op a `stepUp` spelling and drop the env lists |
| `ORDO_SESSION_SECS`, `_MAX_SECS`, `ORDO_REMEMBER*` (and `_LIMIT` ceilings) | env, platform op `setSessionPolicy`, console | deployment ceiling, app developer value, tenant | no | Declare the app value in `auth { session ... }`. Ceilings stay env |
| `ORDO_AUTH_EMAIL_CODE_*`, `_EMAIL_PER_HOUR/DAY`, `_EMAIL_DEVICE_*` | env, console | deployment | no | Stay out. Abuse limits protect the deployment. Allow the app to tighten only |
| `ORDO_EMAIL_REQUIRED`, `ORDO_EMAIL_CHANGE_TOUCH` | env, console | app developer | no | Declare which ops need a verified email, beside the op |
| `ORDO_AUTH_HUMAN_CHECK` (and provider, verify url and secs) | env, console | app developer | no | Declare "human check: on" in `auth`. Site key and verify URL stay deployment |
| `ORDO_AUTH_HUMAN_CHECK_SECRET_FILE`, `ORDO_PUBLIC_HUMAN_SECRET_FILE` | env file | deployment | no | Never in the file. Declare only that a secret is needed |
| `ORDO_PUBLIC_HUMAN_CHECK`, `_PROVIDER`, `ORDO_PUBLIC_WRITES_PER_HOUR`, `_CACHE_BYTES`, `_CDN_*` | env, console | app developer (check, cache lifetime), deployment (bytes, purge) | no | Declare per `public` op: human check, cache lifetime. Purge URL stays deployment |
| `ORDO_AUTH_<NAME>_*` OIDC providers (preset, kind, issuer, scope, claims, algs) | env family, console | app developer chooses provider, deployment holds credentials | no | Declare `method google` in `auth`. Client id and secret file are deployment values the file names as needed |
| `ORDO_AUTH_<NAME>_CLIENT_SECRET_FILE` and `ORDO_AUTH_X_CLIENT_SECRET` style | env file | deployment | no | Never in the file |
| `ORDO_AUTH_TENANT_PROVIDERS`, `_PRESENCE*`, `_MUTABLE_SUBJECT` | env | tenant, deployment | no | Stay out. Tenant sign-in is a tenant op |
| `ORDO_AUTH_RETURN_URLS`, `_HANDOFF_SECS` | env, console | app developer | no | Declare return URLs in `client`. They name the app's own pages |
| `ORDO_APEX_DOMAIN`, `ORDO_AUTH_ORIGINS`, `_RELATED_ORIGINS`, `_RP_NAME` | env, console | deployment | no | Stay out (host values). Declare `auth needs apex, origins` so the console shows what is missing. `RP_NAME` is app-facing text: put in the file |
| `ORDO_AUTH_USER_VERIFICATION` | env | app developer | no | Declare in `auth` (required, preferred) |
| `ORDO_CORS_ORIGINS`, `ORDO_CORS_TENANT_ORIGINS_MAX`, `ORDO_WILDCARD_ORIGINS` | env, console | deployment, tenant | no | The settings audit already says the app developer owns the list. Declare `client` origins. Wildcards stay env |
| `ORDO_HOST_ADDR`, `ORDO_PUBLIC_URL`, `ORDO_IDENTITY_*`, `ORDO_PROXY_*` | env, supervisor | deployment | no | Stay out. Host values. `ORDO_PUBLIC_URL` is a candidate for `needs` |
| `ORDO_KEY_PROVIDER`, `ORDO_MASTER_KEY*`, `ORDO_TOKEN_KEY`, `ORDO_PAGE_KEY`, `ORDO_CHECKPOINT_*`, `ORDO_SECURITY_CHECKPOINT_KEY`, `ORDO_MACHINE_KEY*` | env, env file | deployment | no | Never in the file. Key material |
| `ORDO_RESEAL_*`, `ORDO_DECRYPT_SEEN_SECS` | env | deployment | no | Stay out |
| `ORDO_RECOVERY_*` (delays, approvals, paths, codes) | env, console | tenant, app developer default | no | Declare recovery policy in `auth { recovery ... }`. Tenant tightens |
| `ORDO_SUCCESSION_ENDS_GRANTS`, `ORDO_TENANT_PAUSE_ROLES`, `_RESUME_ROLES`, `_PAUSE_ANSWERS`, `ORDO_MEMBER_LEFT_NAME`, `ORDO_ACCOUNT_REMOVAL` | env, console | app developer | no | Declare beside `members` and `release member`. These name roles and retention of the app |
| `ORDO_DELEGABLE_OPS`, `ORDO_DELEGATION_*`, `ORDO_ACCESS_TEMPLATE`, `ORDO_DEPLOYMENT_OWNERS` | env, console | app developer (ops, roles), deployment (owners, caps) | no | Declare `delegable` on the op and the role template in `acl`. Owners and caps stay env |
| `ORDO_SHARE_LINKS`, `_SHARE_LINK_*`, `ORDO_SHARE_MAX_SECS` | env, console | app developer, tenant | no | Declare per entity or `public` block (on or off, default lifetime, password rules). Lockout numbers stay deployment |
| `ORDO_PG_HOST/PORT/USER/DB/PASSWORD`, `ORDO_DB_CREDENTIALS`, `_PASSWORD_FILE` | env, env file | deployment | no | Never in the file. Declare `needs database` only |
| `ORDO_PG_POOL_*`, `_IDLE_SECS`, `_STATEMENT_TIMEOUT_MS`, `_TENANT_DIAL_*`, `_SPACE_RESERVE` | env, console | deployment | no | Stay out. Capacity tuning |
| `ORDO_WRITE_RETRIES`, `ORDO_APPLY_*`, `ORDO_PINS_RECONCILE_SECS`, `ORDO_REGEX_MAX_LEN` | env, console | deployment | no | Stay out. `ORDO_APPLY_SAFE_CHANGES` is a candidate for the apply command, not the file |
| `ORDO_TENANT_ATTACH_HOSTS`, `ORDO_TENANT_CREDENTIALS_DIR`, `ORDO_PROVISION_*`, `ORDO_BOOTSTRAP_TIMEOUT_SECS`, `ORDO_SPACE`, `ORDO_DEPLOYMENT_SPACE` | env | deployment | no | Stay out, except the tenant provisioning mode in `policy spaces` |
| `ORDO_BLOB_STORE`, `_S3_*`, `_ROUTING`, `_GC_SECS` | env, console | deployment | no | Stay out. Declare `needs blob store`. S3 keys never in the file |
| Blob caps (100 MiB upload, 256 KiB inline), `ORDO_OWN_DOWNLOAD_*` | hardcoded, env | app developer | no | Declare per blob field: max size, allowed types |
| `ORDO_FN_*` (cache dir, body bytes, run retention) | env | deployment, app developer | partly | Function limits belong in the `function` block (it already declares secrets). Cache dir stays env |
| `ORDO_MAIL_KIND/HOST/PORT/TLS/FROM` | env, console | deployment (transport), app developer (from name) | no | Transport stays env. Declare `needs mail`. Subject, text and template belong to the file |
| `ORDO_MAIL_CREDENTIALS_FILE`, `_MAIL_KEY_FILE` | env file | deployment | no | Never in the file |
| `ORDO_MAIL_MAX_ATTEMPTS`, `_BATCH`, `_RETRY_*`, `_LEASE_SECS`, `_SUBJECT/TEXT_MAX_BYTES` | env, console | deployment | no | Stay out |
| Mail templates (sign-in code, invitation, `notify` text) | code constants, routine literal | app developer | partly | `notify` text is in the file. Sign-in and invite mail text is not. Add `channel email { templates ... }` |
| `ORDO_EGRESS_PRIVATE_HOSTS`, `ORDO_EGRESS_DENY_CIDRS` | env, console | deployment | no | Stay out. The secret `for <host>` line already declares the app's hosts |
| `ORDO_USAGE_*`, `ORDO_METRICS_PER_SPACE`, `ORDO_OBSERVED_COST_*`, `ORDO_REPLICA_ID`, `ORDO_NODE_BEAT_SECS` | env, console | deployment | no | Stay out |
| Quotas (requests, rows returned) | platform ops, console | deployment, tenant | no | Declare per op cost class and per-tenant defaults in `policy quota` |
| `ORDO_DENY_ALERT_*`, `ORDO_DEPLOYMENT_SECLOG_*`, tenant stream retention (90 days hardcoded) | env, hardcoded | deployment, tenant | no | Retention: declare in `policy retention`. Alert thresholds stay env |
| Command log policy (undo 90 days, checkpoint 30 days, attest, stall) | hardcoded in `exec/cadence.rs` | app developer | no | Declare `policy commandlog`. Design exists, parser does not |
| Trigger dead-letter attempts (5), debounce cap | hardcoded | app developer | no | Declare on the trigger or routine |
| `ORDO_ROUTINE_EACH_MAX_ROWS`, `ORDO_LIVE_*`, `ORDO_SYNC_PAGE_*`, `ORDO_JSON_*`, `ORDO_REFINE_*` | env, console | deployment, app developer | no | Per-op page size and live limits are app choices. Declare on the op or query. Rest stay env |
| `ORDO_WORKERS`, `ORDO_WORKER_<NAME>_TICK_MS`, `ORDO_*_REFRESH_SECS`, `ORDO_WORKER_PRUNE` | env, console | deployment | no | Stay out |
| `ORDO_SUPERVISOR_*`, `ORDO_CONSOLE_*`, `ORDO_IDENTITY_*` (service settings) | env, supervisor, manifest | deployment | no | Stay out. Not app configuration |
| `ORDO_DEV_UNSAFE_*`, `ORDO_DEV_ADDR`, `ORDO_PGBIN`, `ORDO_UPDATE_GOLDEN`, `ORDO_REQUIRE_*` | env | developer tooling | no | Stay out. Never in a file that ships |
| Feature flags and app-defined settings | not built (settings-layers 24a to 24d) | all four | no | Declare `setting` and `flag` in the file. This is the owner's own ask |

## Gaps worth closing

Ordered by value to a deployment owner.

1. **`auth` block (methods, provisioning, guest, linking, second factor, user verification).** The app decides who may sign in. Today the deployment owner must know the app to set 8 env values. The block is designed and reserved.
2. **`needs` declaration (apex domain, origins, database, mail, blob store, secret files).** The file says what the deployment must supply, so the console shows "missing" before the app fails at runtime.
3. **Session and remember policy.** `ORDO_SESSION_SECS` and friends already have an app developer layer by platform op. The file should hold the value so a redeploy does not lose it.
4. **Mail templates and sender name.** Sign-in and invite mail text is not authored anywhere in the file. An app cannot brand its own mail.
5. **`policy commandlog` and `policy retention`.** Undo horizon, checkpoint and retention are hardcoded. Designed in section 8, no parser.
6. **Step-up per op and `delegable` per op.** Today two env lists name ops. A list of op names in env drifts from the file when an op is renamed.
7. **Share-link and public-op policy.** Lifetime, password rules, human check and cache lifetime describe how the app exposes data.
8. **`client` block (return URLs, CORS origins, token lifetimes).** The design exists. The settings audit already assigns CORS to the app developer.
9. **Recovery policy defaults.** 14 `ORDO_RECOVERY_*` values describe the app's account recovery. Tenant tightens.
10. **Blob field caps and trigger retry limits.** Hardcoded constants that an app developer will ask to change first.

A rule for all of them: the file sets the app developer's value. The deployment owner's `_LIMIT` or ceiling wins. The file can tighten and never loosen.

## Settings the console cannot show

The console lists `ordo_host::settings::REGISTRY` (302 names). Names the host code reads that are not in it:

- Secret file twins: `ORDO_MAIL_CREDENTIALS_FILE`, `ORDO_MAIL_KEY_FILE`, `ORDO_MASTER_KEY_FILE`, `ORDO_MASTER_KEY_PASSPHRASE_FILE`, `ORDO_PAGE_KEY_FILE`, `ORDO_PG_PASSWORD_FILE`, `ORDO_PUBLIC_CDN_PURGE_SECRET_FILE`, `ORDO_PUBLIC_HUMAN_SECRET_FILE`, `ORDO_AUTH_HUMAN_CHECK_SECRET_FILE`. These should show as "set or not set", never the value.
- Provider family members read by literal name: `ORDO_AUTH_CORP_*`, `_DISCORD_*`, `_GH_*`, `_GOOGLE_*`, `_WORK_*`. The registry holds them as the `ORDO_AUTH_<NAME>_` family, so a named provider shows only through the family row.
- `ORDO_BLOB_S3_SPACE`.
- Service settings of `ordo-console`, `ordo-identity-service` and the supervisor (`ORDO_CONSOLE_*`, `ORDO_IDENTITY_*`, `ORDO_SUPERVISOR_*`). They live in each service's own environment and a Kubernetes manifest. The console shows none of them.

## Found in code, absent from `docs/settings.md`

- Host: `ORDO_APPLY_WAIT_SECS`, `ORDO_AUTHZ_KEEP`, `ORDO_BLESS_AI_GUIDE`, `ORDO_BLOB_S3_SPACE`, `ORDO_CONFIG_DIR`, `ORDO_MACHINE_KEY`, `ORDO_MACHINE_KEY_FILE`, `ORDO_MAIL_CREDENTIALS`, `ORDO_MASTER_KEY_PASSPHRASE`, `ORDO_SHARED_INSTANCE`, `ORDO_X_KEY`, `ORDO_X_KEY_FILE`, `ORDO_REFINE_AUDIT_FULL`, `ORDO_UPGRADE_FUNCTIONS`, `ORDO_VERSIONS`.
- Identity service: `ORDO_IDENTITY_CLIENT_ORIGINS`, `_DATA_DIR`, `_DATABASE_POOL_SIZE`, `_DATABASE_URL_FILE`, `_GUESSES_PER_ADDRESS`, `_GUESSES_SHARED`, `_IMPORT_CONSOLE_URL*`, `_INVITE_DAYS`, `_KEY_FILE`, `_KEY_ROTATE_DAYS`, `_LISTEN`, `_PROVISIONER_KEY`, `_RP_ID`, `_SESSION_IDLE_HOURS`, `_SESSION_MAX_HOURS`.
- Console: `ORDO_CONSOLE_DATA_DIR`, `_DATABASE_POOL_SIZE`, `_DATABASE_URL(_FILE)`, `_DOOR_PLAIN_HTTP`, `_DOOR_SPACE`, `_DOOR_URL`, `_KEY_FILE`, `_KEY_ROTATE_DAYS`, `_LISTEN`, `_MODE`, `_ORIGIN`, `_PAGE_DIR`, `_RP_ID`, `_SESSION_IDLE_SECS`, `_SESSION_MAX_SECS`, `_SUPERVISOR_SOCKET`.
- Supervisor: `ORDO_SUPERVISOR_LAUNCH_RELAY`, `_LAUNCH_USER`.
- Worker ticks: `ORDO_WORKER_ATTEST_TICK_MS`, `_AUTH_CEREMONY_SWEEP_TICK_MS`, `_SESSIONS_REFRESH_TICK_MS`, `_TICK_GARBAGE_TICK_MS` (docs cover the `ORDO_WORKER_<NAME>_TICK_MS` pattern only).
- Test and tool switches (no action): `ORDO_FAKE_*`, `ORDO_*_FAKE_*`, `ORDO_REQUIRE_OWNED_PG`, `_S3_TESTS`, `_WEB_TESTS`, `ORDO_UPGRADE_OLD_CLI`, `_OLD_HOST`.

Some of these may sit in other docs (`docs/plan-console.md`). The audit searched `docs/settings.md` only.

## Documented, no code reads it

- `ORDO_AUTH_UNSAFE_MEMORY_DIRECTORY` (not built).
- `ORDO_KMS_KEY_URI` (planned, 18y).
- `ORDO_DEPLOYMENT_SECLOG` (planned, plan-opslog).
- `ORDO_TRUSTED_PROXY_HOPS` (withdrawn).
- `ORDO_SHARE_LINK_MAX_SECS` (withdrawn 2026-09-29).
- `ORDO_TENANT_ROUTING`, `ORDO_PG_ROUTING`, `ORDO_AUTH_CLAIM_CODE` (documented as removed).
- `ORDO_DENIAL_VOLUME_THRESHOLD_PER_SPACE` and `ORDO_EGRESS_OPERATOR_HOSTS`: named in docs and not found as a string in `crates/`. They may be renamed or removed. The second name uses the word "operator".
- `ORDO_BLOB_S3_SECRET_ACCESS_KEY_FILE`, `ORDO_CHECKPOINT_KEY_FILE`, `ORDO_SECURITY_CHECKPOINT_KEY_FILE`, `ORDO_DB_CREDENTIALS`: the code may build these names from `ORDO_X` plus `_FILE`, so a string search misses them. Verify before removing.

## Limits of this audit

- Counts come from string search. A name built at run time is missed.
- The table groups settings by family, so a row can hold several layers. The layer column names the main one.
- Nothing here is ruled. Each recommendation is a proposal for the board.
