# Ordo settings: what exists, and which layer can change it

> **Corrections, 2026-09-29:** `audit-settings-wiring.md` found nine rows here wrong against the code: sharing (section 5) is partly built, `ORDO_SHARE_MAX_SECS` has no default ceiling, invitations are issued by `std.space.invite`, no code mints a grant or delegation, the inline cap and `share_ttl_default` are never read, and several line numbers moved. That audit is the more recent word.

Written 2026-09-29 from the code at commit `8bf57c8` and later. Every row was
traced to where the value is read. Where a document says a setting exists and
the code does not read it, the row says "planned" and names the plan.

**The registry.** `ordo_host::settings::REGISTRY` lists every setting in
sections 1 to 13 and the first table below, with the default as written
here, and `std.deployment.settings` (G7) answers from it. A test fails when
the two disagree, so a new setting gets a row here and an entry there in
the same change.

## How to read this

The four layers, in order: **Deployment** (the deployment owner, on the server
or, once it exists, in the console), **App developer** (the program or its
apply-time config), **Tenant** (a tenant owner or manager, through a platform
op) and **Person** (one person, for themselves).

Each layer cell holds one word, or is blank:

- **sets**: this layer can change it today.
- **exposable**: it can be handed to this layer when the app developer
  chooses, and the mechanism exists.
- **could be exposed**: the value belongs at this layer under the owner's
  principle (limits and choices layered, never one hardcoded system value),
  and no mechanism exists yet.
- **planned**: a plan gives this layer the setting. Nothing in code yet.
- blank: does not apply at this layer.

**Built?** is `built`, `planned (plan)`, `hardcoded` (a Rust constant with no
way to change it) or `not built`. The code decides these words, not the plans.

An environment variable is set by the deployment owner. Today no console
exists, so the deployment layer means environment, file or boot flag, plus the
few ops the deployment space's owner can call. The program has no spelling for
settings yet: `policy` is lexed and not parsed (`syntax/trigger.rs:518-528`,
`exec/cadence.rs:69-75`), so nothing is "set by the app developer" in a
program today. Where a value is tenant-shaped, the tenant cell says what the
tenant owner can do through a platform op.

Paths in the last column drop `crates/` and `/src`: `host/` is
`crates/ordo-host/src/`, `exec/` is `crates/ordo-exec/src/`, and likewise
`api/`, `identity/`, `crypto/`, `blob/`, `secevent/`, `commandlog/`,
`egress/`, `cli/`, `syntax/`, `schema/`.

## Hardcoded today and should be a setting

Each item is a Rust constant that behaves like a setting. The layer is where
the owner's principle puts the choice.

| Constant | Value | Where | Should be exposed to |
|---|---|---|---|
| Blob upload cap (`max_object`) | 100 MiB | `blob/policy.rs:56`, used `host/runtime.rs:2611` | App developer, tenant lowers it |
| Blob inline cap (`inline_cap`) | 256 KiB | `blob/policy.rs:55` | App developer |
| Share link default lifetime (`share_ttl_default`) | 7 days when nothing is set; not a ceiling | `blob/policy.rs` | Renamed 2026-09-29 from `share_ttl_max`: the owner ruled it a default |
| Invitation ticket lifetime, default and cap | 3 days, cap 30 days | `host/tickets.rs:54,57,287-289` | Tenant per invitation, app developer default |
| Step-up freshness | 300 s | `identity/access.rs:32,451` | App developer, tenant tightens |
| Passkey ceremony life | 300 s | `identity/passkey.rs:59,711`, `identity/provider.rs:41` | Deployment |
| Page size for reads | 50 | `host/runtime.rs:3748` and `:3938` | App developer (docs say deployment-wide setting). A sync feed's page size is its own setting, `ORDO_SYNC_PAGE_ROWS` |
| `ORDO_PUBLIC_CACHE_BYTES` | Size of the in-process cache of public read answers, per node. 0 turns the cache off. 0 to 68719476736 (64 GiB). A value outside the range, or not a whole number, refuses the boot naming the range and the default | 268435456 (256 MiB) | built | sets | could be exposed | | | api/public.rs; host/public.rs |
| `ORDO_PUBLIC_CDN_S_MAXAGE` | The `s-maxage` a public answer carries, in seconds, for a CDN. 0 to 604800 (7 days). A value outside the range, or not a whole number, refuses the boot naming the range and the default | 60 | built | sets | could be exposed | | | host/public.rs |
| `ORDO_PUBLIC_CDN_SWR` | The `stale-while-revalidate` a public answer carries, in seconds. 0 to 604800 (7 days). A value outside the range, or not a whole number, refuses the boot naming the range and the default | 300 | built | sets | could be exposed | | | host/public.rs |
| `ORDO_PUBLIC_MISSES_PER_MINUTE` | How many cache misses one public op may send to the database per minute, on one node. Every distinct parameter value is a miss. A miss over the cap is refused `429 quota_exceeded`, naming the setting. 0 turns the cap off. 0 to 1000000; a value outside the range refuses the boot naming the range and the default | 600 | built | sets | could be exposed | | | api/public.rs (`admit_miss`); api/router.rs (`public_call`); host/public.rs |
| `ORDO_PUBLIC_CDN_PURGE_URL` | Where to POST `{"tags":[...]}` when cached public answers are dropped. https only. Sent through `ordo-egress`. Unset: nothing is posted. A bad value refuses the boot | unset | built | sets | | | | host/public.rs |
| `ORDO_PUBLIC_CDN_PURGE_SECRET_FILE` | A file holding the bearer secret for the purge call. A secret in `ORDO_PUBLIC_CDN_PURGE_SECRET` is refused at boot | unset | built | sets | | | | host/public.rs |
| `ORDO_PUBLIC_CDN_PURGE_TIMEOUT_SECS` | How long one CDN purge call may take, connect to last byte. 1 to 60; outside refuses the boot naming the range and the default. A purge that times out is logged naming this setting | 5 | built | sets | | | | host/public.rs (`purge_from_vars`) |
| `ORDO_PUBLIC_WRITES_PER_HOUR` | How many writes one public write op may make per hour, on one node. The hour is read before the human check and taken after it, so only writes that passed the check use it. Over it: `429 quota_exceeded`, naming the setting. 0 turns the cap off. 0 to 1000000; A value outside the range, or not a whole number, refuses the boot naming the range and the default | 10000 | built | sets | could be exposed | | | host/public.rs; api/router.rs (`public_call`) |
| `ORDO_PUBLIC_HUMAN_CHECK` | `on` or `off`. Whether a public write needs a human check the provider confirms (header `X-Ordo-Human`). A bad value refuses the boot | on | built | sets | | | | host/public.rs |
| `ORDO_PUBLIC_HUMAN_PROVIDER` | Which provider verifies the human check: `turnstile`, `hcaptcha` or `recaptcha`. A bad value refuses the boot | turnstile | built | sets | | | | host/public.rs |
| `ORDO_PUBLIC_HUMAN_SECRET_FILE` | A file holding the provider's secret key for public writes. Falls back to `ORDO_AUTH_HUMAN_CHECK_SECRET_FILE` when the provider is Turnstile, so a deployment sets Turnstile once; set, it overrides it. Neither set with the check on: public writes are refused `403 human_check_failed`, naming both. Another provider with only the sign-in secret refuses the boot. A secret in `ORDO_PUBLIC_HUMAN_SECRET` refuses the boot | `ORDO_AUTH_HUMAN_CHECK_SECRET_FILE` | built | sets | | | | host/public.rs |
| `ORDO_PUBLIC_HUMAN_VERIFY_URL` | Where public-write tokens are verified. Must be https. Falls back to `ORDO_AUTH_HUMAN_CHECK_VERIFY_URL` for Turnstile, then to the provider's own address | `ORDO_AUTH_HUMAN_CHECK_VERIFY_URL`, then the provider's | built | sets | | | | host/public.rs |
| `ORDO_PUBLIC_HUMAN_VERIFY_SECS` | How long one public-write verification may take. 1 to 30; outside refuses the boot naming the range and the default. Falls back to `ORDO_AUTH_HUMAN_CHECK_VERIFY_SECS` for Turnstile. A timeout fails closed and is logged | `ORDO_AUTH_HUMAN_CHECK_VERIFY_SECS`, then 5 | built | sets | | | | host/public.rs |
| `ORDO_PROXY_HEADER` | The header naming the visitor's address, such as `CF-Connecting-IP` or `X-Forwarded-For`. Turns proxy mode on with `ORDO_PROXY_TRUSTED`; one without the other refuses the boot naming the other. Unset: no decision uses a client address | unset (proxy mode off) | built | sets | | | | host/proxy.rs |
| `ORDO_PROXY_TRUSTED` | The proxies' addresses: a comma list of addresses or CIDR blocks. The header is believed only when the socket peer is one of them | unset | built | sets | | | | host/proxy.rs |
| `ORDO_PROXY_SLOW_AFTER` | Tokenless requests to `/op/` or `/blob/` one visitor may make per minute before the delay starts. Proxy mode slows, it never refuses. 0 never slows. 0 to 1000000; A value outside the range, or not a whole number, refuses the boot naming the range and the default | 120 | built | sets | | | | host/proxy.rs |
| `ORDO_PROXY_SLOW_MS` | The delay in ms for each further `ORDO_PROXY_SLOW_AFTER` requests in the minute. 1 to 60000; A value outside the range, or not a whole number, refuses the boot naming the range and the default | 250 | built | sets | | | | host/proxy.rs |
| `ORDO_PROXY_SLOW_MAX_MS` | The longest delay in ms. 0 to 60000; A value outside the range, or not a whole number, refuses the boot naming the range and the default | 5000 | built | sets | | | | host/proxy.rs |
| Request body limit | 2 MiB | `api/router.rs:137` | Deployment, app developer per op |
| Idempotency window, reservation, entry cap | 24 h, 60 s, 10 000 | `api/idempotency.rs:136-145`, `host/replay.rs:157` | Deployment |
| Command log policy: undo horizon | 90 days | `exec/cadence.rs:117`, used `host/runtime.rs:1589,1609,2916` | App developer, tenant may lengthen |
| Command log policy: checkpoint interval | 30 days | `exec/cadence.rs:114,118` | App developer |
| Command log policy: attest every | 1000 commands or 1 h | `exec/cadence.rs:119-120` | App developer |
| Command log policy: consumer stall | 30 days | `exec/cadence.rs:121` | App developer |
| Tenant security stream retention | 90 days | `secevent/store.rs:490`, `host/secdrain.rs:283` | App developer, tenant may lengthen |
| Security event collapse window and cap | 60 s, 256 | `secevent/collapse.rs:89,97` | Deployment |
| Security checkpoint period | 1 h | `exec/secevent.rs:1020` | Deployment |
| Commandlog prune period | 1 h | `host/runtime.rs:4106` | Deployment |
| Trigger attempts before dead letter | 5 | `exec/triggers.rs:527` | App developer, per listener |
| Debounce max wait | 10 windows | `syntax/trigger.rs:543` | App developer |
| CORS preflight cache | 600 s | `api/cors.rs:69` | Deployment |
| Egress timeout and body cap | 20 s, 8 KiB | `egress/client.rs:60,63` | Deployment, tenant per sink |
| Signed delivery replay window | 300 s | `egress/sign.rs:73` | Deployment |
| Admitted default role | viewer | `host/admission.rs:60`, table default `:228` | App developer (owner: "app dev territory") |
| Quota quantities | requests, rows_returned | `host/quotas.rs:43-48` | Deployment adds more (writes are 18i) |
| Worker supervision: backoff unit, leader tick, census | 1 s, 5 s, 60 s | `host/supervisor.rs:181-183` | Deployment |
| Quarantine after failed runs | 10 | `host/supervisor.rs:107` | Deployment |
| Shutdown wait for worker acts | 10 s | `host/runtime.rs:3552` | Deployment |
| Ceremony sweep period | 60 s | `host/workers.rs:1174`, `host/runtime.rs:3874` | Deployment |
| Schema apply lock wait | 30 s | `host/apply_op.rs:154` | Deployment |
| Routing and bucket lookup caches | 30 s | `exec/route.rs:357`, `exec/bucket.rs:607` | Deployment |
| Security events page maximum | 200 | `host/serve.rs:939` | Deployment |
| Worker `accounts.removal` (runs under `now` and `<n>d`, not `never`): period 600 s, batch 32 per pass | 600 s, 32 | `host/account_removal.rs` | Deployment |

Fixed on purpose, not proposed as settings: the Argon2id profile for the
master key and passwords (`crypto/master.rs:98-102`,
`identity/provider.rs:324-330`), token and cursor domains, the 64 KiB blob
chunk (`crypto/stream.rs:85`), the tenant name limit of 256
(`host/tenant_ops.rs:81`), the CDN purge reply cap of 16 KiB (`host/public.rs`
`PURGE_CAP`; nothing in the reply is read), the metrics series cap of 10 000
(`host/metrics.rs:240`), the OIDC discovery and JWKS reply caps of 256 KiB and
the ID token cap of 16 KiB (`identity/oidc.rs`, `identity/jose.rs`; they bound
what a provider can make the deployment read) and the group name limit of 120
(`host/profile_ops.rs`, `std.space.createGroup`, like the tenant name limit).

## 1 · Sign-in and sessions

| Setting | What it does | Default | Built? | Deployment | App developer | Tenant | Person | Where |
|---|---|---|---|---|---|---|---|---|
| `ORDO_AUTH_METHODS` | Turns sign-in on and lists the methods (password, passkey) and the names of OAuth2 connections (`passkey,discord`). `email` signs in with an emailed code and refuses the boot unless `ORDO_MAIL_KIND` is set. Names of OIDC connections (`passkey,google`) work the same way. SAML refuses the boot by name | unset: no sign-in | built | sets | | | | host/runtime.rs:378,900 |
| `ORDO_AUTH_METHODS_MAX_AGE` | The `max-age`, in seconds, of `GET /auth/methods`, plain or with `?tenant=`. A page or a shared cache may keep the answer that long, so a retired tenant connection can show on a page for up to that many seconds. It refuses new sign-ins at once either way. 0 to 3600; a value outside the range refuses the boot naming the range and the default | 60 | built | sets | | | | host/auth/methods.rs; host/runtime.rs |
| `ORDO_SESSION_SECS` | Life of one token | 3600 s | built | sets | could be exposed | could be exposed | | host/runtime.rs:401; identity/session.rs:79 |
| `ORDO_SESSION_MAX_SECS` | How long refresh can continue a session before a new sign-in | 43200 s | built | sets | could be exposed | could be exposed | could be exposed | host/runtime.rs:405; identity/session.rs:92 |
| `ORDO_SESSION_SECS_LIMIT` | The longest token life any layer may choose: the app developer's default and range, and a tenant's own. A value above it is refused naming this setting. `ORDO_SESSION_SECS` above it refuses the boot | 86400 s | built | sets | | | | host/session_policy.rs |
| `ORDO_SESSION_MAX_SECS_LIMIT` | The longest session ceiling any layer may choose, the same way | 2592000 s (30 days) | built | sets | | | | host/session_policy.rs |
| `ORDO_REMEMBER_SECS_LIMIT` | The longest renewal idle lifetime any layer may choose, the same way | 34560000 s (400 days) | built | sets | | | | host/session_policy.rs |
| `ORDO_REMEMBER_MAX_SECS` | The overall limit on a remembered sign-in: this many seconds after the sign-in, sign in again, however often it was used. `none` or unset is no limit. 60 or more. The original sign-in time travels with the cookie through each rotation | none | built | sets | sets (`rememberMaxSecs`) | sets (`rememberMaxSecs`) | | host/session_policy.rs; host/auth.rs `resume` |
| `ORDO_REMEMBER_MAX_SECS_LIMIT` | The absolute cap on the overall limit. With it set, no layer may choose none or longer, and a sign-in with nothing chosen ends at the cap. `ORDO_REMEMBER_MAX_SECS` above it refuses the boot | none | built | sets | | | | host/session_policy.rs |
| `std.deployment.setSessionPolicy` | The app developer's session layer, in the deployment's own space: `tokenSecs`, `maxSecs`, `remember` (`on`, `off`), `rememberSecs`, `rememberMaxSecs` (seconds or `none`), and an optional range for tenants per value (`tokenSecsRange=300-7200`). `default` clears a value, `none` a range. Each must sit inside the deployment's limits. `remember=on` is refused while `ORDO_REMEMBER=off`. Read back on `/health/deep` under `session_policy` | none: the deployment's values, and tenants inside its limits | built | | sets | | | host/session_policy.rs |
| `std.space.setSessionPolicy` | A tenant's own session values, the same five words. Longer or shorter than the app developer's default, inside the app developer's range, or the deployment's limits when it set none. A value outside is refused naming the range and who sets it. Applies to every session for that tenant, its own sign-in connections included. The tenant owner's by default; the app developer closes it with `ORDO_ACCESS_TEMPLATE="std.space.setSessionPolicy="` | none | built | | can close | sets | | host/session_policy.rs |
| `std.space.sessionPolicy` | Reads what is in force in a tenant, where each value came from (`tenant`, `app`, `deployment`), the ranges, and the tenant's own choice. Owners and managers by default; the app developer opens it wider with `ORDO_ACCESS_TEMPLATE` | | built | | can open | reads | | host/session_policy.rs |
| `ORDO_REMEMBER` | Keeps a person signed in across a reload and a browser restart (`on` or `off`). Each sign-in sets a renewal cookie the page's scripts cannot read (`HttpOnly`, `SameSite=Strict`, `Path=/auth/refresh`, `Secure` and the `__Secure-` name prefix except on plain http to a loopback name). On load the page calls `POST /auth/refresh` with no token and gets one. Off sets no cookie, and a cookie-only refresh is refused with `remember_off`, naming `ORDO_REMEMBER=on`. Another value refuses the boot | `on` | built | sets | | | | host/renewal.rs; host/auth.rs `resume` |
| `ORDO_REMEMBER_SECS` | How long a renewal lives unused. Each use spends the cookie, sets a new one and starts the time again. A spent cookie used again after 10 s ends that sign-in everywhere. Sign-out, sign-out everywhere, passkey removal, account recovery, a password change and account deletion end it. 0 refuses the boot and names `ORDO_REMEMBER=off` | 2592000 s (30 days) | built | sets | | | | host/renewal.rs |
| `ORDO_AUTH_PROVISIONING` | Ceiling on the ways a tenant may admit strangers (`invite-only` or `auto-onboard`) | `invite-only` | built | sets | | | | host/runtime.rs:425; host/admission.rs:88-107 |
| `ORDO_AUTH_HUMAN_CHECK` | Asks for a Cloudflare Turnstile token at sign-in `begin` (`turnstile` or `off`). The token is the `human` field of the begin body. A missing or failed token is the one refusal and nothing is sent out. Ordo sends Cloudflare no visitor address | `off` | built | sets | | | | host/auth/human.rs; host/runtime.rs |
| `ORDO_AUTH_HUMAN_CHECK_SECRET_FILE` | File holding the Turnstile secret key. Required when the check is on. `ORDO_AUTH_HUMAN_CHECK_SECRET` as a value is refused | none | built | sets | | | | host/auth/human.rs |
| `ORDO_AUTH_HUMAN_CHECK_SITE_KEY` | The Turnstile widget's public site key, shown to pages by `GET /auth/methods` so they need no build variable for it. Optional. Letters, digits, `_` and `-`, at most 128; anything else refuses the boot and names the setting. Not a secret | none | built | sets | | | | host/auth/human.rs; host/auth/methods.rs |
| `ORDO_AUTH_HUMAN_CHECK_VERIFY_URL` | Where tokens are verified. Must be https. Cloudflare's address unless a test points it elsewhere | `https://challenges.cloudflare.com/turnstile/v0/siteverify` | built | sets | | | | host/auth/human.rs |
| `ORDO_AUTH_HUMAN_CHECK_VERIFY_SECS` | How long one Turnstile verification may take, connect to last byte. 1 to 30; outside that refuses the boot and names the default | 5 | built | sets | | | | host/auth/human.rs |
| `ORDO_AUTH_EMAIL_CODE_DIGITS` | Digits in an emailed sign-in code. 6 to 10; outside that refuses the boot | 8 | built | sets | | | | host/email_code.rs (DIGITS_ENV) |
| `ORDO_AUTH_EMAIL_CODE_SECS` | Seconds a code lives, counted from `begin`. 120 to 900; outside that refuses the boot | 600 | built | sets | | | | host/email_code.rs (LIFE_ENV) |
| `ORDO_AUTH_EMAIL_CODE_ATTEMPTS` | Tries at one code. 1 to 10. The last wrong try ends the ceremony | 5 | built | sets | | | | host/email_code.rs (ATTEMPTS_ENV) |
| `ORDO_AUTH_EMAIL_PER_HOUR` | Sign-in messages to one address in a clock hour. 1 to 1000. Over the limit nothing is sent and the caller sees the usual answer | 5 | built | sets | | | | host/email_code.rs (PER_HOUR_ENV) |
| `ORDO_AUTH_EMAIL_PER_DAY` | Sign-in messages to one address in a clock day. 1 to 10000. Must be at least the per-hour value | 15 | built | sets | | | | host/email_code.rs (PER_DAY_ENV) |
| `ORDO_AUTH_EMAIL_DEVICE_PER_HOUR` | Messages an hour kept for a returning device, past the address limit. 0 to 1000. 0 turns the returning-device lane off | 3 | built | sets | | | | host/email_code.rs (DEVICE_PER_HOUR_ENV) |
| `ORDO_AUTH_EMAIL_DEVICE_DAYS` | Days a device marker is honoured. 1 to 400 | 30 | built | sets | | | | host/email_code.rs (DEVICE_DAYS_ENV) |
| `ORDO_EMAIL_REQUIRED` | Which ops need a verified email address on the caller's account: `off`, `all` (every op of the app, never a platform op), or a comma list of ops. A person without one is refused `403 email_required`, naming the setting. If the address table cannot be read, the op is refused `503 space_unavailable` with `Retry-After`, not let through. Not `off` without `ORDO_MAIL_KIND` refuses the boot | off | built | sets | sets (`std.deployment.setEmailPolicy {required}`; `default` clears it) | | | host/contact_email.rs (REQUIRED_ENV) |
| `ORDO_EMAIL_CHANGE_TOUCH` | Whether changing or removing a contact email needs a fresh passkey touch: `on` or `off`. A first address never needs one. Without a touch the answer is `403 presence_required`, naming the setting | on | built | sets | sets (`std.deployment.setEmailPolicy {changeTouch}`; `default` clears it) | | | host/contact_email.rs (TOUCH_ENV) |
| `ORDO_AUTH_EMAIL_DEPLOYMENT_PER_HOUR` | Sign-in messages an hour for the whole deployment. 1 to 10,000,000 | 1000 | built | sets | | | | host/email_code.rs (DEPLOYMENT_PER_HOUR_ENV) |
| `ORDO_AUTH_REGISTRATION_TTL_SECS` | How long a sign-up that no tenant admits is kept | 90 days | built | sets | could be exposed | | | host/registrations.rs:42-66; host/runtime.rs:3885 |
| `ORDO_AUTH_TEMPORARY_PASSWORDS` | Lets a tenant type a temporary password for a person it created | `off` | built | sets | could be exposed | could be exposed | | host/runtime.rs:2733-2745 |
| Step-up freshness | How old a passkey assertion may be for a gated act | 300 s | hardcoded | could be exposed | could be exposed | could be exposed | | identity/access.rs:32,451 (setter at :464 has no caller in host) |
| Passkey ceremony life | How long a sign-in challenge stays valid | 300 s | hardcoded | could be exposed | | | | identity/passkey.rs:59,711 (setter at :715 has no caller in host) |
| Password length or strength rule | A minimum for passwords | none found in code | not built | could be exposed | could be exposed | could be exposed | | host/credentials.rs, host/auth.rs (no check found) |
| `ORDO_AUTH_SECOND_FACTOR` | Deployment level of the second step: off, enabled, required, required for non-passkey | `enabled` | built | sets | planned | | | host/second_factors.rs |
| `ORDO_AUTH_SECOND_STEP_ATTEMPTS` | Wrong second-step answers allowed in one ceremony | 5 | built | sets | | | | host/second_factors.rs |
| `ORDO_AUTH_SECOND_FACTOR_LOCK_AFTER` | Wrong second-step answers in total before the factor locks | 20 | built | sets | | | | host/second_factors.rs |
| `ORDO_AUTH_SECOND_STEP_SECS` | How long a second-step ceremony stays valid. Floor 30 | 300 s | built | sets | | | | host/second_factors.rs |
| `ORDO_AUTH_SECOND_FACTOR_MAX` | Most second factors one person may hold | 10 | built | sets | | | | host/second_factors.rs |
| `ORDO_AUTH_REAUTH_SECS` | How long a fresh touch counts for step-up. Floor 30 | 300 s | built | sets | | | | host/second_factors.rs |
| `ORDO_RECOVERY_QUARANTINE_SECS` | Block on step-up after a recovery completes. Floor 3600. May not exceed the contest window | 86400 s (1 day) | built | sets | | | | host/recovery.rs |
| `ORDO_RECOVERY_CONTEST_SECS` | Window in which the person can contest a recovery. Floor 3600 | 259200 s (3 days) | built | sets | | | | host/recovery.rs |
| `ORDO_RECOVERY_HOST_DELAY_SECS` | Wait before a host-path recovery completes | 0 | built | sets | | | | host/recovery.rs |
| `ORDO_RECOVERY_CODE_DELAY_SECS` | Wait before a recovery-code recovery completes | 0 | built | sets | | | | host/recovery.rs |
| `ORDO_RECOVERY_OPERATOR_DELAY_SECS` | Wait before an operator-path recovery completes. Floor 3600 | 86400 s | built | sets | | | | host/recovery.rs |
| `ORDO_RECOVERY_SUCCESSION_DELAY_SECS` | Wait before a succession recovery completes. Floor 3600 | 86400 s | built | sets | | | | host/recovery.rs |
| `ORDO_RECOVERY_TENANT_DELAY_SECS` | Wait before a tenant-path recovery completes | 0 | built | sets | | | | host/recovery.rs |
| `ORDO_RECOVERY_OPERATOR_APPROVALS` | Approvals an operator-path recovery needs, 1 to 10 | 1 | built | sets | | | | host/recovery.rs |
| `ORDO_RECOVERY_TENANT_APPROVALS` | Approvals a tenant-path recovery needs, 1 to 10 | 1 | built | sets | | | | host/recovery.rs |
| `ORDO_RECOVERY_PATHS` | Which recovery paths are open: `code`, `tenant`, `operator`, `host`, comma separated | all four | built | sets | | | | host/recovery.rs |
| `ORDO_RECOVERY_HOLD_DEFAULT` | `on` holds every recovery for review by default | off | built | sets | | | | host/recovery.rs |
| `ORDO_RECOVERY_MANAGERS` | `on` lets a tenant manager start a recovery for a member | off | built | sets | | | | host/recovery.rs |
| `ORDO_RECOVERY_TICKET_SECS` | How long a recovery ticket lives. 3600 to 2592000 (30 days); outside that refuses the boot and names the default | 259200 s (3 days) | built | sets | | | | host/recovery.rs |
| `ORDO_RECOVERY_CODES_PER_SET` | How many codes one recovery-code set holds. 1 to 50; outside that refuses the boot and names the default | 10 | built | sets | | | | host/recovery.rs; host/recovery_codes.rs |
| `ORDO_SUCCESSION_ENDS_GRANTS` | `off` keeps a departed owner's delegations and their grants to apps when an owner succession completes. The completing call may say `endGrants` either way | on | built | sets | | | | host/recovery.rs; host/identity_ops.rs |
| Tenant `second_factor` | A tenant requires a second step for its own tenant, tighten only | `inherit` | planned (plan-second-factor §5.4) | | | planned | | plan-second-factor.md:259 |
| Person `require_second_step` | A person asks for the second step on passkey sign-in too | `when-enrolled` | planned (plan-second-factor §5.4) | | | | planned | plan-second-factor.md:260 |
| Wrong-code lockout | Wrong backup or second-step codes in a row that lock a factor | 20 | planned (plan-second-factor) | planned | planned | | | corpus-rulings.md, second factor answers |
| Outside provider connections | Which outside providers may sign people in. OAuth2 (Discord, GitHub) and OIDC (Google, Microsoft, a company login) are built, through the `ORDO_AUTH_<NAME>_*` rows. SAML is planned. The emailed code is built (`howto-sign-in.md` section 10) | none | built for OAuth2, OIDC and the emailed code, planned for the rest (plan-auth-federated §4) | planned | | planned | | plan-auth-federated.md:282-288 |
| Provider two-step counts as presence | Whether a provider's own second step satisfies presence, per provider | never until set | built: `ORDO_AUTH_<NAME>_PRESENCE` (OIDC only; SAML waits) | sets | | | | plan-auth-federated.md §8.2 |
| `ORDO_AUTH_RETURN_URLS` | Allowlist of return origins after an outside sign-in or a subdomain hand-off. Comma-separated, `https` (or `http://localhost`, `http://127.0.0.1`), no fragment. Matched by origin (scheme, host, port); any path on a listed origin is accepted. Required with a connection or the hand-off. Takes `https://*.<domain>` with `ORDO_WILDCARD_ORIGINS=on` | none | built | sets | | | | host/federation.rs (parse_return_urls); host/runtime.rs (auth_from_env); identity/origin.rs |
| `ORDO_AUTH_HANDOFF_SECS` | Seconds a subdomain hand-off code lives (`POST /auth/handoff`). 1 to 300; outside that refuses the boot | 60 | built | sets | | | | host/runtime.rs (handoff_secs_from); host/auth/handoff.rs |
| `ORDO_AUTH_LINKING` | `on` or `off`. Whether a signed-in person may claim another sign-in (linking by claim). `off` restores the old rule: a second sign-in is added only with a passkey's fresh presence. Any other value refuses the boot | on | built | sets | | | | host/auth/claim.rs (from_vars); host/runtime.rs |
| `ORDO_AUTH_LINKING_SECS` | Seconds between the first ceremony and the claimed one. 60 to 900; outside that refuses the boot | 300 | built | sets | | | | host/auth/claim.rs (from_vars); host/runtime.rs |
| `ORDO_AUTH_GUEST` and `std.deployment.setGuest` | `on` or `off`. Whether a visitor may start as a guest (`howto-sign-in.md` section 13). Stored in `$deploymentsetting`: the variable seeds it on a boot that finds none, the op changes it, and the stored value wins. Off answers a new guest 403 `guests_off`, naming this setting. Existing guests still resume. Any other value refuses the boot | off | built | sets | sets (through the op) | | | host/auth/guest.rs; host/runtime.rs |
| `ORDO_AUTH_GUEST_TTL_SECS` | How long a guest that claimed nothing, and that no tenant admitted, is kept. Can only shorten `ORDO_AUTH_REGISTRATION_TTL_SECS`. `0` or a word refuses the boot | the registration lifetime (90 days) | built | sets | | | | host/auth/guest.rs; host/registrations.rs (sweep_guests) |
| `ORDO_AUTH_CONSOLE_SESSION_SECS` | Only on a door without the identity service: where `ORDO_IDENTITY_URL` is set no console key is made (step 8). Seconds a console key session lasts (`plan-console.md` 8.2, G14). It carries no presence and is not refreshed: the console signs again. It reaches the door's ops in the deployment's own space only, and among `/auth/*` only `/auth/step-up` and `/auth/logout`. 60 to 3600; outside that refuses the boot, naming this setting | 900 | built | sets | | | | host/console_keys.rs (session_secs_from); host/runtime.rs |
| `ORDO_IDENTITY_URL` | Where the door reaches the identity service (`plan-identity-service.md` 4.2): `http://127.0.0.1:8082` in the one image. The door fetches its keys from `/.well-known/jwks.json` at boot, every 10 minutes, and once a minute at most when a token names a key it does not hold, and keeps the last set in `$identitykeys` so a restart with the service down still verifies. Plain `http://` only to loopback unless `ORDO_IDENTITY_PLAIN_HTTP=on`. Unset, the door accepts no deployment token and says so at boot. A bad value refuses the boot. The console reads it too (step 6): set, people sign in at the identity service and the console verifies their tokens with the same keys, held in memory; its own sign-in and console key routes answer `moved_to_identity_service`. Unset, the console keeps its own sign-in | none | built | sets | | | | host/identity_keys.rs (IdentitySettings); host/runtime.rs; console/identity.rs |
| `ORDO_IDENTITY_ORIGIN` | The identity service's public origin, the `iss` the door and the console require of a deployment token. The same value the service has. The console page sends people there to sign in and may call it (its `connect-src`). Required when `ORDO_IDENTITY_URL` is set; unset or not `scheme://host[:port]` refuses the boot | none | built | sets | | | | host/identity_keys.rs (IdentitySettings); console/identity.rs |
| `ORDO_IDENTITY_DEPLOYMENT` | The deployment handle a deployment token must name in `aud` (`ordo-deployment:<handle>`). The same value the service has. Not a handle refuses the boot. The console takes its own handle, `ORDO_CONSOLE_DEPLOYMENT`, and refuses to boot when this is set to another | main | built | sets | | | | host/identity_keys.rs (IdentitySettings); console/identity.rs |
| `ORDO_IDENTITY_PLAIN_HTTP` | `on` lets `ORDO_IDENTITY_URL` be plain `http://` to a host that is not loopback, for a door or console and the service on a private network the deployment owner trusts. Anyone on the way could swap the keys. Another value refuses the boot | off | built | sets | | | | host/identity_keys.rs (IdentitySettings); console/identity.rs |
| `ORDO_IDENTITY_TOKEN_SECS` | Read by the identity service. How long a deployment token lives, in seconds, 60 to 3600; outside that refuses the boot. **Ending a person's access or a machine key takes effect at the door and the console within this many seconds**: a token already issued keeps working until it expires, because neither checks back with the service on each call (`plan-identity-service.md` 5.7). New sign-ins and renewals are refused at once. Shorter means faster removal and more renewals | 600 | built | sets | | | | identity-service/config.rs (TOKEN_SECS) |
| `ORDO_IDENTITY_BASE_PATH` | Read by the identity service and the console. Where browsers reach the service under its origin, such as `/identity`: its pages, files, calls and invite links are under it, its session cookie is scoped to it, and the console passes everything under it through to the service, adding the visitor to `X-Forwarded-For`. The service still answers at the root, for the door and the console on loopback. In the one image the supervisor sets it to `/identity` with `ORDO_IDENTITY_ORIGIN` the console's own origin, unless `ORDO_SUPERVISOR_IDENTITY_ADDRESS` is `own-port` or `ORDO_IDENTITY_ORIGIN` is set. Not `/` and plain lower-case segments refuses the boot | unset (the root) | built | sets | | | | identity-service/config.rs; console/identity.rs |
| `ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER` | Read by the identity service and `ordo-supervisor`. Opt-in (owner, 2026-10-30: the database is explicit, or a setting enables the same server). `true`: with no `ORDO_IDENTITY_DATABASE_URL`, the service makes its own database, `ordo_identity`, on the console's Postgres server with the console's role at first start, and uses it. Back it up with the console's. `false`: no database named refuses the identity service's boot. In the one image, with neither set, the supervisor does not start the identity service at all: the console keeps its own sign-in, and the boot log and a banner on every console page name both settings; a later, announced release will require one. A role that may not create a database refuses the boot, naming `ORDO_IDENTITY_DATABASE_URL`. Any value but `true` or `false` refuses the boot | false | built | sets | | | | identity-service/config.rs; store.rs; supervisor/children.rs |
| `ORDO_RUNTIME_REVISION`, `ORDO_RUNTIME_APPLIED_BY`, `ORDO_IMAGE_DIGEST` | Read once at boot into the boot record (G16): the security record and `/health/deep` under `boot`. The supervisor (`ordo-supervisor`) sets the first two and refuses them as settings; the platform may set the digest. Unset reads `none` | none | built | | | sets the first two | | host/boot_record.rs; supervisor/spec.rs |
| `ORDO_SUPERVISOR_IDENTITY_USER`, `ORDO_SUPERVISOR_CONSOLE_USER`, `ORDO_SUPERVISOR_DEPLOYMENT_USER` | Read by `ordo-supervisor`, the one image's entrypoint (`plan-identity-service.md` 3.1). The unix user, `uid:gid`, each child runs as: the identity service, the console, the deployment. The image sets 65531, 65532 and 65533. Unset or empty, that child runs as the supervisor's own user and the log says so. Root, a value that is not `uid:gid`, or 2 children on one uid refuses the boot. Set, and the supervisor cannot switch to it (no CAP_SETUID, CAP_SETGID, CAP_SETPCAP and CAP_CHOWN), it refuses the boot naming the setting: it never falls back to its own user. The deployment's falls back to the console's old `ORDO_CONSOLE_CHILD_USER` when unset | unset | built | sets | | | | supervisor/config.rs |
| `ORDO_SUPERVISOR_IDENTITY_BIN` | Read by `ordo-supervisor`. The identity service's binary. No file there refuses the boot | /usr/local/bin/ordo-identity-service | built | sets | | | | supervisor/config.rs |
| `ORDO_SUPERVISOR_CONSOLE_BIN` | Read by `ordo-supervisor`. The console's binary. No file there refuses the boot | /usr/local/bin/ordo-console | built | sets | | | | supervisor/config.rs |
| `ORDO_SUPERVISOR_HOST_BIN` | Read by `ordo-supervisor`. The deployment's binary, `ordo-host`. Falls back to the console's old `ORDO_CONSOLE_HOST_BIN`. No file there refuses the boot | /usr/local/bin/ordo-host | built | sets | | | | supervisor/config.rs |
| `ORDO_SUPERVISOR_STATE_DIR` | Read by `ordo-supervisor`. Its own volume, made 0700: the deployment's live revision and the last good one, each sealed (`plan-identity-service.md` 3.2). Empty, the supervisor builds revision 1 from the container's `ORDO_*` variables. Mount a persistent volume there | /var/lib/ordo-supervisor | built | sets | | | | supervisor/config.rs |
| `ORDO_SUPERVISOR_KEY_FILE` | Read by `ordo-supervisor`. The key the live revision is sealed with, made at first boot, mode 0600. Back it up with the volume: a key that does not open the live revision refuses the boot, naming this setting | <state dir>/supervisor.key | built | sets | | | | supervisor/config.rs |
| `ORDO_SUPERVISOR_RUN_DIR` | Read by `ordo-supervisor`. The socket and each child's secret files, `<run dir>/<child>`, each file 0600 and the child's. Mount a tmpfs there owned by 65530, so a secret never reaches a disk. Unset, the console's old `ORDO_CONSOLE_RUN_DIR` is the default; with neither, the first usable of /run/ordo and /run/ordo-console (where a compose file from before the one image mounts its tmpfs) is taken, made the supervisor's own with CAP_CHOWN. A folder already at mode 0711 on a read-only root is taken as it is. No usable folder refuses the boot, naming this setting and why each was not | /run/ordo | built | sets | | | | supervisor/config.rs |
| `ORDO_SUPERVISOR_SOCKET` | Read by `ordo-supervisor`. The control socket the console manages the deployment through: `status` and `logs` for each child, `current`, `apply` and `restart` for the deployment only. Mode 0660, the console's group; a caller of any other uid is refused | <run dir>/supervisor.sock | built | sets | | | | supervisor/config.rs |
| `ORDO_SUPERVISOR_LISTEN` | Read by `ordo-supervisor`. Where its own `/health/live` answers, for the container's probe, with each child's state. Not bindable refuses the boot | 127.0.0.1:8079 | built | sets | | | | supervisor/config.rs |
| `ORDO_SUPERVISOR_GRACE_SECS` | Read by `ordo-supervisor`. How long a child has after SIGTERM before SIGKILL. Shutdown stops the deployment, then the console, then the identity service, each with this grace. Falls back to the console's old `ORDO_CONSOLE_CHILD_GRACE_SECS` | 30 | built | sets | | | | supervisor/config.rs |
| `ORDO_SUPERVISOR_READY_SECS` | Read by `ordo-supervisor`. How long an applied revision of the deployment has to answer `/health/ready` before the live one is started again. Falls back to the console's old `ORDO_CONSOLE_CHILD_READY_SECS` | 120 | built | sets | | | | supervisor/config.rs |
| `ORDO_SUPERVISOR_RELAY` | Read by `ordo-supervisor`. `auto`: a child that runs as another unix user is started under a relay, its launcher, which runs as that same user and signals the child when the supervisor asks. So a stop needs no CAP_KILL, and a compose file that grants none still stops cleanly. `always`: every child is, the supervisor's own user too; one small process more per child. Any other value refuses the boot | auto | built | sets | | | | supervisor/config.rs |
| `ORDO_SUPERVISOR_IDENTITY_ADDRESS` | Read by `ordo-supervisor`, when `ORDO_IDENTITY_ORIGIN` is unset. `console-path`: the identity service is reached under the console's own origin, at `ORDO_IDENTITY_BASE_PATH` (`/identity`), passed through by the console, so no new port is needed and the passkeys' relying party stays the console's host; the service then reads each visitor from `X-Forwarded-For` and trusts the console on loopback (`ORDO_PROXY_TRUSTED` gains 127.0.0.1/32 and ::1/128 for the service only). `own-port`: its own origin, the console's host with port 8082, published separately. Any other value refuses the boot | console-path | built | sets | | | | supervisor/children.rs |
| `ORDO_WILDCARD_ORIGINS` | `on` or `off`. `on` lets an entry be `https://*.<domain>` in `ORDO_AUTH_RETURN_URLS`, `ORDO_AUTH_ORIGINS` and `ORDO_CORS_ORIGINS`. The `*` is one label, `https` only, never the bare apex, and at least 2 labels follow it. A wildcard entry with it `off` refuses the boot, naming this setting | off | built | sets | could be exposed | | | host/runtime.rs (wildcards_from_env); identity/origin.rs |
| `ORDO_PUBLIC_URL` | The deployment's public `https` address. The redirect address to register at a provider is `<it>/auth/callback/<name>`. Required with a connection | none | built | sets | | | | host/federation.rs (connection_from_vars); host/runtime.rs (auth_from_env) |
| `ORDO_AUTH_<NAME>_PRESET` | Names a built-in provider (`discord` or `github`) for connection `<NAME>`. Fills endpoints, scope and claims | none | built | sets | | | | host/federation.rs (connection_from_vars); identity/oauth2.rs (presets) |
| `ORDO_AUTH_<NAME>_KIND` | `oauth2`, for a provider with no preset. Endpoints and subject claim are then required | none | built | sets | | | | host/federation.rs (connection_from_vars) |
| `ORDO_AUTH_<NAME>_CLIENT_ID` | The client id registered at the provider. Required. For `NAME=DISCORD` it is `ORDO_AUTH_DISCORD_CLIENT_ID`, for `NAME=GH` it is `ORDO_AUTH_GH_CLIENT_ID` | none | built | sets | | | | host/federation.rs (connection_from_vars) |
| `ORDO_AUTH_<NAME>_CLIENT_SECRET_FILE` | Path to a file holding the client secret. Required. `ORDO_AUTH_<NAME>_CLIENT_SECRET` as a value refuses the boot, and the refusal names the `_FILE` form | none | built | sets | | | | host/federation.rs (connection_from_vars) |
| `ORDO_AUTH_<NAME>_AUTHORIZE_URL`, `_TOKEN_URL`, `_USERINFO_URL`, `_SCOPE`, `_SUBJECT_CLAIM`, `_EMAIL_CLAIM`, `_EMAIL_VERIFIED_CLAIM`, `_DISPLAY_CLAIMS`, `_AVATAR_CLAIM` | Override the preset, or required without one. URLs are `https` | the preset's | built | sets | | | | host/federation.rs (connection_from_vars); identity/oauth2.rs (presets) |
| `ORDO_AUTH_<NAME>_PRESET` (OIDC) | `google` or `microsoft`, for an OIDC connection. Google fills the issuer, scope and claims. Microsoft needs `_TENANT`. Endpoints come from discovery, which refreshes on `ORDO_AUTH_OIDC_CACHE_SECS` | none | built | sets | | | | host/oidc.rs (connection_from_vars); identity/oidc.rs |
| `ORDO_AUTH_<NAME>_KIND` (OIDC) | `oidc`, for a company login with no preset (Okta, Keycloak, Auth0, Entra). `_ISSUER` is then required | none | built | sets | | | | host/oidc.rs (connection_from_vars); identity/oidc.rs |
| `ORDO_AUTH_<NAME>_ISSUER` | The OIDC issuer, an `https` address. Discovery is read from `<issuer>/.well-known/openid-configuration`. Fixed for life under the connection name: a later boot with another issuer refuses (the fingerprint is taken from the issuer) | Google's, with the preset | built | sets | | | | host/oidc.rs (connection_from_vars); identity/oidc.rs |
| `ORDO_AUTH_<NAME>_TENANT` | Microsoft only. The Entra directory (tenant) id, a GUID. `common`, `organizations`, `consumers` and domain names are refused: a connection names one tenant | none | built | sets | | | | host/oidc.rs (connection_from_vars); identity/oidc.rs |
| `ORDO_AUTH_<NAME>_CLIENT_ID`, `_CLIENT_SECRET_FILE` (OIDC) | The same two settings as OAuth2, both required. A secret in a variable's value (`_CLIENT_SECRET`) refuses the boot. The redirect URI to register is `<ORDO_PUBLIC_URL>/auth/callback/<name>`. `ORDO_AUTH_RETURN_URLS` is required, as for OAuth2 | none | built | sets | | | | host/oidc.rs (connection_from_vars); identity/oidc.rs |
| `ORDO_AUTH_<NAME>_SCOPE` (OIDC) | The scope asked for. Must include `openid` | Google `openid email profile`; Microsoft `openid profile`; else `openid email profile` | built | sets | | | | host/oidc.rs (connection_from_vars); identity/oidc.rs |
| `ORDO_AUTH_<NAME>_USE_EMAIL` | `on` or `off`. Whether the ID token's email is kept. An address is kept only when `email_verified` is the JSON boolean `true` | on; off for Microsoft | built | sets | | | | host/oidc.rs (connection_from_vars); identity/oidc.rs |
| `ORDO_AUTH_<NAME>_DISPLAY_CLAIMS` (OIDC) | Comma-separated claims tried in order for the display name | `name`, then `preferred_username` | built | sets | | | | host/oidc.rs (connection_from_vars); identity/oidc.rs |
| `ORDO_AUTH_<NAME>_AVATAR_CLAIM` (OIDC) | The claim holding the picture address | Google `picture`; else none | built | sets | | | | host/oidc.rs (connection_from_vars); identity/oidc.rs |
| `ORDO_AUTH_<NAME>_AUTHORIZE_URL`, `_TOKEN_URL`, `_JWKS_URL` (OIDC) | All three or none. All three pin the endpoints and skip discovery. Each is `https`. Any one alone refuses the boot | none: discovery | built | sets | | | | host/oidc.rs (connection_from_vars); identity/oidc.rs |
| `ORDO_AUTH_<NAME>_ISS_PARAMETER` | `required`. Demands the `iss` parameter (RFC 9207) on the callback. Only with pinned endpoints; with discovery it refuses the boot | unset | built | sets | | | | host/oidc.rs (connection_from_vars); identity/oidc.rs |
| `ORDO_AUTH_<NAME>_CLOCK_SKEW_SECS` | Seconds of clock skew allowed on `exp`, `iat` and `nbf`, for one connection. 0 to 120 | `ORDO_AUTH_OIDC_CLOCK_SKEW_SECS` (60) | built | sets | | | | host/oidc.rs (connection_from_vars); identity/oidc.rs |
| `ORDO_AUTH_<NAME>_ALGS` | Comma-separated subset of `RS256,PS256,ES256,EdDSA` allowed for the ID token, for one connection. `none` and `HS*` are never accepted | `ORDO_AUTH_OIDC_ALGS` (all four) | built | sets | | | | host/oidc.rs (connection_from_vars); identity/oidc.rs |
| `ORDO_AUTH_OIDC_CACHE_SECS` | Deployment-wide. 300 to 86400. How long an OIDC connection's keys (JWKS) and discovery document stay fresh before they are fetched again. A provider's `Cache-Control: max-age` shortens this but never lengthens it, and never below 300 (`no-store` and `no-cache` count as 0, so 300). A token whose `kid` is not cached refetches the keys at once, at most once a minute (60 s, fixed, not a setting). A failed refresh keeps the last good copy. It also reaches a tenant's own OIDC connections (`host/tenant_providers.rs`, `with_oidc_defaults`, wired in `host/runtime.rs`) | 3600 | built | sets | | | | host/runtime.rs; host/oidc.rs; plan-auth-federated.md O9 |
| `ORDO_AUTH_OIDC_CLOCK_SKEW_SECS` | Deployment-wide. The clock skew every OIDC connection allows, unless its own `_CLOCK_SKEW_SECS` says otherwise. A tenant's connection has no value of its own and always takes this one. 0 to 120; outside refuses the boot naming the range and the default | 60 | built | sets | | | | host/oidc.rs (`defaults_from_vars`); host/tenant_providers.rs |
| `ORDO_AUTH_OIDC_ALGS` | Deployment-wide. The ID token algorithms every OIDC connection accepts, a subset of `RS256,PS256,ES256,EdDSA`, unless its own `_ALGS` says otherwise. A tenant's connection always takes this one. `none` and `HS*` refuse the boot | all four | built | sets | | | | host/oidc.rs (`defaults_from_vars`); host/tenant_providers.rs |
| `ORDO_AUTH_OIDC_STALE_GRACE_SECS` | Deployment-wide. How long past freshness cached keys still verify while an issuer cannot be reached, for every OIDC connection, a tenant's too. 0 to 604800; outside refuses the boot naming the range and the default | 86400 (24 h) | built | sets | | | | host/oidc.rs; identity/oidc.rs (`stale_grace_secs`) |
| `ORDO_AUTH_TENANT_LOCK_WAIT_MS` | How long a guarded change to a tenant's owners waits for that tenant's lock, as clock time from the request: the refusal comes within it plus about one database round trip. 100 to 120000; outside refuses the boot naming the range and the default. A change that waits it out is refused naming this setting and its value | 10000 | built | sets | | | | host/tenant_providers.rs (`lock`); host/runtime.rs |
| `ORDO_AUTH_<NAME>_PRESENCE` (OIDC) | `off` or `on`. Whether this connection's two-step claim counts as presence and as Ordo's second step. `on`: everywhere, acts on the deployment included. `off`: only in a tenant that accepts it for its own data (`std.space.setAdmission` `presenceFrom`); a tenant may accept any connection. A page asks with `"presence": true` in `/auth/begin`, at any connection | off | built | sets | | | | host/oidc.rs (presence_setting); identity/upstream.rs; host/presence.rs |
| `ORDO_AUTH_TENANT_PROVIDERS` | `off` or `on`. Whether a tenant's owner may add sign-in providers of their own (`std.space.addConnection` and the other three ops). Off, the ops and routes refuse and name this setting. On needs `ORDO_PUBLIC_URL` and `ORDO_AUTH_RETURN_URLS`. See `howto-sign-in.md` section 16 | off | built | sets | | | | host/tenant_providers.rs; host/auth/tenant.rs |
| `ORDO_AUTH_<NAME>_PRESENCE_AMR` (OIDC) | Comma list of `amr` words that mean two steps. Each word 1 to 64 of letters, digits and `:._-`. Both lists empty refuses the boot | `mfa` | built | sets | | | | host/oidc.rs; identity/upstream.rs |
| `ORDO_AUTH_<NAME>_PRESENCE_ACR` (OIDC) | Comma list of `acr` values that mean two steps | none | built | sets | | | | host/oidc.rs; identity/upstream.rs |
| `ORDO_AUTH_UPSTREAM_PRESENCE_SECS` | Deployment-wide. 30 to 300. How old the ID token's `auth_time` may be at the callback, and how old the claim may be when an op uses it. Never longer than the 300 s step-up window; the shorter wins | 300 | built | sets | | | | host/oidc.rs; host/presence.rs; identity/access.rs |
| `ORDO_AUTH_<NAME>_MUTABLE_SUBJECT` | `acknowledged` accepts a subject claim that can change (`email`, `login`, `username`, `name`, `preferred_username`, `global_name`). Otherwise the boot refuses | refuses | built | sets | | | | host/federation.rs (connection_from_vars) |
| `ORDO_AUTH_PROVIDER_HANDOFF_SECS` | Deployment-wide. How long the code a provider callback files (the hand-off the browser carries back) can be redeemed, for OAuth2 and OIDC. 10 to 600; outside that refuses the boot and names the default. Not the subdomain hand-off (`ORDO_AUTH_HANDOFF_SECS`) | 60 | built | sets | | | | identity/federation.rs (`handoff_ttl_secs`); host/runtime.rs (`install_limits`) |
| `ORDO_AUTH_PASSWORD_PENDING_MAX` | Deployment-wide. How many password challenges may wait unanswered at once. 10 to 1000000; outside that refuses the boot and names the default. Past it `/auth/begin` answers `429 too_many_attempts` naming this setting until some complete or expire | 10000 | built | sets | | | | identity/provider.rs (`with_pending_max`); host/runtime.rs (`auth_from_env`) |
| `ORDO_PASSWORD_SLOW_AFTER` | Wrong passwords in a row on one account before each further try waits a doubling delay (1 s, 2 s, 4 s, ...), answered `429 too_many_attempts` with `Retry-After` and refused unchecked. Always on; never locks; the right password resets it. Counted per account (a hash of the identifier typed, known or not), never per address, in the control database, shared by every node. `off` (or 0) turns it off. Outside 0 to the limit refuses the boot | 5 (owner, 2026-10-25) | built (2026-10-25) | sets | | | | host/guess_limits.rs |
| `ORDO_PASSWORD_SLOW_AFTER_LIMIT` | The most wrong tries before slowing any layer may choose. 1 to 1000 | 100 | built (2026-10-25) | sets | | | | host/guess_limits.rs |
| `ORDO_PASSWORD_SLOW_CAP_SECS` | The longest wait of the password slowdown. Outside the limit to 86400 refuses the boot | 60 | built (2026-10-25) | sets | | | | host/guess_limits.rs |
| `ORDO_PASSWORD_SLOW_CAP_SECS_LIMIT` | The shortest longest wait any layer may choose. 1 to 86400 | 1 | built (2026-10-25) | sets | | | | host/guess_limits.rs |
| `std.deployment.setPasswordSlowdown` | The app developer's `slowAfter` (a number or `off`), `slowCapSecs`, and an optional range for tenants per value (`slowAfterRange=3-10`), in the deployment's own space. Each inside the deployment's limits, else refused (`password_slowdown_out_of_range`) naming the limit. `default` clears a value, `none` a range | none: the deployment's values | built (2026-10-25) | | sets | | | host/guess_limits.rs `set_app`; `$deploymentsetting` key `password_slowdown` |
| `std.space.setPasswordSlowdown` | A tenant owner's own `slowAfter`, `slowCapSecs`, for sign-ins that name the tenant (`space`), inside the app developer's range, or the deployment's limits when there is none. `off` only where the app developer's `slowAfterRange` starts at 0. Recorded on the tenant's stream (`password-slowdown-set`). The app developer closes it with `ORDO_ACCESS_TEMPLATE="std.space.setPasswordSlowdown="` | none: the app developer's, else the deployment's | built (2026-10-25) | | can close | sets | | host/guess_limits.rs `set_tenant`; `$admissionpolicy.password_slow_*` |
| `ORDO_AUTH_PROVIDER_REQUEST_SECS` | Deployment-wide. How long one request to an outside provider may take, connect to last byte, for OAuth2 and OIDC. 1 to 60; outside that refuses the boot and names the default | 10 | built | sets | | | | host/federation.rs (`fetch_timeout`); host/runtime.rs (`install_limits`) |
| `ORDO_AUTH_PROVIDER_CA_FILE` | A PEM file of extra trust anchors for the outside sign-in providers, the deployment's and each tenant's. Unset, the public root program alone. A file that cannot be read or holds no certificate refuses the boot by name | none | built | sets | | | | host/runtime.rs (provider_roots) |
| `ORDO_TRUSTED_PROXY_HOPS` | Replaced by `ORDO_PROXY_HEADER` and `ORDO_PROXY_TRUSTED` (proxy mode): the visitor is the right-most forwarded address that is not a trusted proxy | not read | withdrawn | | | | | host/proxy.rs |
| Link another sign-in to my account | A person chooses to claim a second sign-in | off, person's act | planned (plan-auth-federated) | | | | planned | corpus-rulings.md, federated answer 5 |
| `ORDO_AUTH_UNSAFE_MEMORY_DIRECTORY` | Named in a comment only. The gate is gone | none | not built | | | | | host/runtime.rs:375 |

## 2 · Enrolment and admission

| Setting | What it does | Default | Built? | Deployment | App developer | Tenant | Person | Where |
|---|---|---|---|---|---|---|---|---|
| Tenant admission: open | Anyone who signs in is admitted | off | built | | | sets | | host/admission.rs:225,547; host/platform_ops.rs:1015 |
| Tenant admission: default role | Role a stranger gets when admitted | viewer | built | | could be exposed | sets | | host/admission.rs:60,228,565 |
| Tenant admission: provider rules | Admit people arriving through a named provider, optionally by mail domain, at a role | none | built | | | sets | | host/admission.rs:186,225 |
| Tenant admission: first-comer | First sign-in to an empty tenant becomes its owner | off | built | | | sets | | host/admission.rs:225,238 |
| Invitation (ticket) | An owner or manager invites a named person at a role | always available | built | ceiling only | | sets | | host/admission.rs:20-50; host/tickets.rs |
| `ORDO_AUTH_CLAIM_CODE` | Removed (2026-10-30), with `std.deployment.setClaimCode`. A door that still has it, or `ORDO_AUTH_CLAIM_CODE_FILE`, boots and logs it as ignored, naming the first-comer setting: a tenant's `firstComer=true` (`std.space.setAdmission`), off by default, under `ORDO_AUTH_PROVISIONING=auto-onboard`. The deployment's own space never admits a first-comer. The stored row stays until the drop migration | removed | built | sets | | | | host/admission.rs `ignored_claim_code_note`; host/runtime.rs |
| Invitation lifetime | How long an invitation ticket lives | 3 days, cap 30 days | hardcoded | could be exposed | could be exposed | could be exposed | could be exposed | host/tickets.rs:54,57; host/credential_ops.rs:323,626 |
| `ORDO_PROVISION_SELF_SERVE` and `std.deployment.setSelfServe` | Gives each person who signs in a tenant of their own | `off` | built | sets | sets (through the op) | | | host/self_serve.rs; host/auth/signup.rs |
| `ORDO_PROVISION_SIGNUPS_PER_ADDRESS_PER_HOUR` | Sign-ups one address may make per hour before its next sign-up is slowed by `ORDO_PROXY_SLOW_MS` (never refused). Read only in proxy mode; set without it, the boot prints one line naming `ORDO_PROXY_HEADER` and `ORDO_PROXY_TRUSTED`. `off` or 1 to 1000000 | off | built | sets | | | | host/proxy.rs |
| `ORDO_PROVISION_SIGNUPS_PER_HOUR` | Deployment-wide sign-up cap, counted only under `on-signup`, except a new guest, which counts whatever the mode; a number or `unlimited` | 60 | built | sets | sets (through `setSelfServe`) | | | host/self_serve.rs |
| `ORDO_DEPLOYMENT_OWNERS` | Names the deployment's owners on the server, by account id (`usr_…`). Read only without the identity service: with `ORDO_IDENTITY_URL` set it is ignored, with a boot-log line naming it, never a refusal | none | built | sets | | | | host/provision.rs, runtime.rs |
| Clean-up of tenants of people who never return | Deletes idle own tenants | none, platform deletes nothing | not built | | could be exposed | could be exposed | | corpus-rulings.md, provisioning-signup |

## 3 · Passkeys and presence

| Setting | What it does | Default | Built? | Deployment | App developer | Tenant | Person | Where |
|---|---|---|---|---|---|---|---|---|
| `ORDO_APEX_DOMAIN` | The passkey relying party id. Changing it strands every passkey | none, required with passkey | built | sets | | | | host/runtime.rs:461 |
| `ORDO_AUTH_ORIGINS` | Pages allowed to run a passkey ceremony, under the relying party id. Takes `https://*.<domain>` under the apex with `ORDO_WILDCARD_ORIGINS=on` | none, required with passkey | built | sets | | | | host/runtime.rs:473; identity/origin.rs |
| `ORDO_AUTH_RELATED_ORIGINS` | Pages on other domains allowed to run a ceremony | none | built | sets | | | | host/runtime.rs:508 |
| `ORDO_AUTH_RP_NAME` | Name an authenticator shows | the relying party id | built | sets | | | | host/runtime.rs:515 |
| `ORDO_AUTH_USER_VERIFICATION` | Whether the device must verify the person (`required`, `preferred`) | `required` | built | sets | | | | host/runtime.rs:520 |
| `ORDO_PRESENCE_ON_READS` | `on` or `off`. Whether a platform op that only reads (its kind is `Read`, served on `GET`) asks for a fresh passkey assertion. Off: a signed-in person reads with no touch (owner, 2026-10-22). Such a read stays closed to delegated tokens. Any other value refuses the boot naming this setting | `off` | built | sets | | | | host/presence.rs `reads_from_env`; host/lib.rs |
| `ORDO_STEP_UP_EXEMPT_OPS` | Platform changes that do not need a fresh passkey assertion. Reads need none unless `ORDO_PRESENCE_ON_READS=on`. A read named here is also open to delegated tokens. A passkey is not the only presence: an OIDC connection's two-step claim can also satisfy step-up under `ORDO_AUTH_<NAME>_PRESENCE`. The tenant provider ops (`std.space.addConnection`, `std.space.updateConnection`, `std.space.retireConnection`, `std.space.connections`) are platform ops and ask for presence like the rest. `std.deployment.addConsoleKey` and `std.deployment.revokeConsoleKey` can never be exempt: a prefix leaves them out, and naming one refuses the boot | none: every change needs it | built | sets | could be exposed | could be exposed | | host/presence.rs:53; host/runtime.rs:875; host/lib.rs:1965 |
| `ORDO_DELEGABLE_OPS` | Ops that delete rows (a pure delete, or a write with a `del` beside its `set`) that a delegated token (a worker or an agent acting for a person) may call, each named exactly. Naming `download` also opens a person's own download (`GET /download`) to delegated tokens; it is closed to them by default and never open to a share guest. Platform writes cannot be named: naming one, or a pattern, refuses the boot | none: every delete and every platform write is closed to delegated tokens | built (11-4d) | sets | could be exposed | | | host/delegable.rs `DelegablePolicy` |
| `ORDO_ACCESS_TEMPLATE` | The app developer's default for who may call each program op, as `op=role,role` entries split on `;` or a newline. `op=` closes the op to every role until a tenant opens it. A tenant's owner may change it for their tenant (`howto-roles.md`). Naming a platform op, a pattern, an unknown role, or an op twice refuses the boot | none: every role calls authored reads, editors and up authored writes, managers and owners the `std.*` ops | built | sets | | | | host/access_template.rs `ORDO_ACCESS_TEMPLATE` |
| `ORDO_DELEGATION_AGENTS` | The agents that may ask a person for delegation through the consent ceremony, each as `id=name`. The name is what the person sees on the review | none: no agent may ask, and a request is refused naming this setting | built (11-4b) | sets | | | | host/consent.rs `Settings` |
| `ORDO_DELEGATION_MAX_HOURS` | The longest delegation a person may grant, 1 to 720 hours. A bad boot value refuses naming the range and the default. A longer request is refused naming the setting, its value, and the value to set | 8 | built (11-4b) | sets | | | | host/consent.rs |
| `ORDO_DELEGATION_REQUEST_SECS` | How long a delegation request waits for the person, then for the agent to redeem it, 60 to 3600 seconds. A bad boot value refuses naming the range and the default | 600 | built (11-4b) | sets | | | | host/consent.rs |
| `ORDO_DELEGATION_MAX_OPS` | The most ops one delegation request may name, 1 to 100. A bad boot value refuses naming the range and the default. A request over it is refused naming the setting, its value, and the value to set | 10 | built (11-4b) | sets | | | | host/consent.rs |
| `ORDO_DELEGATION_OPEN_PER_AGENT` | The most delegation requests one agent may have waiting at once, across every tenant and person. 1 to 1000000. Over it, `POST /auth/delegation/request` answers `429 delegation_limit`, naming the setting and the value that would admit it | 1000 | built | sets | | | | host/consent.rs (`Ceremony::request`) |
| `ORDO_DELEGATION_OPEN_PER_PERSON` | The most delegation requests one person may have waiting at once, from every agent. Counted by the person id the request names, whether or not that member exists, so the answer says nothing about who is a member. 1 to 1000. Over it, `POST /auth/delegation/request` answers `429 delegation_limit`, naming the setting and the value that would admit it | 10 | built | sets | | | | host/consent.rs |
| Recovery paths enabled | Which recovery paths a deployment or tenant allows | A, B, C, D | planned (plan-passkey-recovery §4.4) | planned | | planned | | plan-passkey-recovery.md:224 |
| Recovery delays and contest window | Waiting time before a recovery completes | 0 h, 24 h, 72 h, 72 h | planned (plan-passkey-recovery §4.4) | planned | | planned | | plan-passkey-recovery.md:225-228 |
| Export block after recovery | Time a recovered passkey cannot satisfy step-up | 24 h | planned (plan-passkey-recovery §4.4, K8) | planned | | planned | | plan-passkey-recovery.md:229 |
| Approvals required for recovery | Number of approvers | 1 | planned (plan-passkey-recovery §4.4) | planned | | planned | | plan-passkey-recovery.md:230 |
| Hold on recovery | Holds a recovery for review | off | planned (plan-passkey-recovery §5.3) | planned | | planned | | plan-passkey-recovery.md:231 |
| Email as proof of recovery | Lets a mailed code count as proof | off | planned (plan-passkey-recovery §3.5) | planned | | | | plan-passkey-recovery.md:232 |
| Sealed replica on the device: idle lock and retention | Locks the local copy when idle, and how long it keeps rows | none built | planned (plan-client-cache, 22m) | | planned | could be exposed | could be exposed | plan-client-cache.md:20,138 |

## 4 · Custody and keys

| Setting | What it does | Default | Built? | Deployment | App developer | Tenant | Person | Where |
|---|---|---|---|---|---|---|---|---|
| `ORDO_KEY_PROVIDER` | Selects custody: `local` (dev only), `sealed-master`, `test` (dev only) | none, required | built | sets | | | | host/runtime.rs:632-694 |
| `ORDO_MASTER_KEY` | Plaintext master key in the environment, or under `local` a file holding it in `ORDO_MASTER_KEY_FILE` (both set refuses the boot). Refused outside development | none | built | sets | | | | crypto/master.rs:125-156; host/runtime.rs:641 |
| `ORDO_MASTER_KEY_FILE` | The sealed master key file from `ordo keys seal-master`. Under `local`, a file holding the plaintext key | none, required for `sealed-master` | built | sets | | | | host/runtime.rs:659; crypto/master.rs:128 |
| `ORDO_MASTER_KEY_PASSPHRASE_FILE` | The passphrase file that opens the sealed master | none, required for `sealed-master` | built | sets | | | | host/runtime.rs:663; crypto/master.rs:129 |
| `ORDO_DEV_UNSAFE_TEST_KEYS` | Allows the local and test providers | off | built | sets | | | | crypto/master.rs:109; host/runtime.rs:636 |
| `ORDO_TOKEN_KEY` | 64 hex characters that sign tokens. Or `ORDO_TOKEN_KEY_FILE`, a file holding them; both set refuses the boot | none, required | built | sets | | | | identity/token.rs:86; host/runtime.rs:787 |
| `ORDO_PAGE_KEY` | 64 hex characters that sign read cursors. Or `ORDO_PAGE_KEY_FILE`, a file holding them; both set refuses the boot | none, required | built | sets | | | | api/page.rs:112; host/runtime.rs:799 |
| `ORDO_CHECKPOINT_KEY` and `ORDO_CHECKPOINT_KEY_ID` | Signs command log checkpoints. The key may be given as `ORDO_CHECKPOINT_KEY_FILE`. Unset turns the feature off | none | built | sets | | | | commandlog/sign.rs:107-118; host/runtime.rs:812 |
| `ORDO_CHECKPOINT_PUBKEY` | Public key the prune worker and CLI verify against | none | built | sets | | | | host/runtime.rs:4110; cli/operator.rs:213 |
| `ORDO_SECURITY_CHECKPOINT_KEY`, `ORDO_SECURITY_CHECKPOINT_KEY_ID`, `ORDO_SECURITY_CHECKPOINT_PUBKEY` | Signs and verifies the security stream. The key may be given as `ORDO_SECURITY_CHECKPOINT_KEY_FILE`. Unset, the first boot makes one, sealed under the master key in `$securitykey`, and prints the key id and public key to verify with; set always wins | none | built | sets | | | | secevent/store.rs:113-127; host/security_key.rs |
| `ORDO_RESEAL_CHUNK` | Rows re-sealed per step after a key rotation | 256 | built | sets | | | | host/workers.rs:820,829 |
| `ORDO_RESEAL_PAUSE_MS` | Pause between re-seal steps | 250 ms | built | sets | | | | host/workers.rs:821,834 |
| `ORDO_DECRYPT_SEEN_SECS` | How often changed "last decrypted" entries are written to the control database, and once more at graceful shutdown | 60 | built | sets | | | | host/decrypt_seen.rs (FLUSH_SETTING); host/runtime.rs |
| `ORDO_KMS_KEY_URI` and key service providers | Master key held in AWS or GCP KMS, Vault or OpenBao | none | planned (plan-key-service, 18y) | planned | | | | plan-runtime.md:412; plan-key-service.md |
| Key cache time in memory | How long a released key stays in memory | 5 min | planned (plan-key-service §6) | planned | | | | plan-key-service.md:106 |

## 5 · Sharing

Guest shares (G1 to G9) and open share links (L1 to L3) are built. A link's
lifetime has three levels: the deployment's default, a tenant's choice when
the app developer exposes it, and the lifetime each link is minted with
(`plan-sharing.md:3`). The platform sets no maximum of its own. A link may
need a password (owner, 2026-10-22). Its wrong-try limit, lock and shortest length are
layered like session settings: the deployment's defaults and absolute
limits, the app developer's values and ranges, and a tenant's own, looser or
tighter, inside them.

| Setting | What it does | Default | Built? | Deployment | App developer | Tenant | Person | Where |
|---|---|---|---|---|---|---|---|---|
| `ORDO_SHARE_LINKS` | Turns open share links on or off | `on` | built (L1) | sets | could be exposed | could be exposed | | host/share_links.rs `LinkSettings` |
| `ORDO_SHARE_LINK_DEFAULT_SECS` | Default expiry for a new link. A default, never a cap | 7 days | built (L1) | sets | could be exposed | | | host/share_links.rs; blob/policy.rs `share_ttl_default` |
| `ORDO_SHARE_LINK_TENANT_LIFETIME` | Whether tenants may choose their own link lifetime | `off` | built (L1) | | sets | | | host/share_links.rs |
| Tenant link lifetime choice (`std.space.setShareLinkLifetime`) | A tenant owner sets how long its links live and may be, when the app developer exposes it | inherits the default | built (L1), stored on the admission policy row until 24a's store exists | | | sets | | host/share_links.rs `set_lifetime`; `$admissionpolicy.share_link_secs` |
| Lifetime of one link (`$expires`) | The expiry the sharer states when minting | the tenant's choice, else the default | built (L1) | | | | sets | host/share_links.rs `mint` |
| Link label length | Longest label on a link | 120 characters | hardcoded (L1) | | could be exposed | | | host/share_links.rs `MAX_LABEL` |
| `ORDO_SHARE_LINK_MISS_LIMIT` | Wrong share-link secrets one address may send per minute before its next open is slowed by `ORDO_PROXY_SLOW_MS` (never refused; the answer stays `link_refused`). Read only in proxy mode; set without it, the boot prints one line naming `ORDO_PROXY_HEADER` and `ORDO_PROXY_TRUSTED`. `off` or 1 to 1000000 | off | built | sets | | | | host/proxy.rs |
| A link's password (`$password` on `std.space.shareLink`, `std.space.setLinkPassword`) | The sharer makes opening the link need a password, or clears it. Stored as an argon2id hash only. At least the tenant's minimum length, at most 256 characters | none | built (2026-10-22) | | | | sets | host/share_links.rs `set_password`; `$sharelink.password_hash` |
| `ORDO_SHARE_LINK_PASSWORD_TRIES` | Default wrong passwords one link takes before it locks, counted per link. `off` (or 0): wrong tries are refused one by one and never lock. Outside 0 to the limit refuses the boot | off (owner, 2026-10-22) | built (2026-10-22) | sets | | | | host/link_password.rs `Deployment` |
| `ORDO_SHARE_LINK_PASSWORD_TRIES_LIMIT` | The most tries any layer may choose. 1 to 1000 | 100 | built (2026-10-22) | sets | | | | host/link_password.rs |
| A link's maximum opens (`$maxOpens` on `std.space.shareLink`, `std.space.setLinkMaxOpens`) | The sharer limits how many times a link may be opened, and changes or removes the limit later (`none`). An open is the first successful open by a device, not a renewal. At the limit the link is used up and refused like an expired one (`link_refused`); `std.space.links` shows `opensUsed`, `maxOpens`, `state: "used-up"`. Counted with one guarded update. 1 to 1000000 | none: unlimited | built (2026-10-22) | | | | sets | host/share_links.rs `claim_open`, `set_max_opens`; `$sharelink.max_opens`, `uses`, `$sharelinkdevice` |
| `ORDO_SHARE_LINK_PASSWORD_LOCK_SECS` | Default time a locked link stays locked. Outside the limit to 2592000 refuses the boot | 900 (15 minutes) | built (2026-10-22) | sets | | | | host/link_password.rs `Deployment` |
| `ORDO_SHARE_LINK_PASSWORD_LOCK_SECS_LIMIT` | The shortest lock any layer may choose. 1 to 2592000 | 60 | built (2026-10-22) | sets | | | | host/link_password.rs |
| `ORDO_SHARE_LINK_PASSWORD_MIN_LENGTH` | Default shortest link password. Only a password being set is measured: a link given a shorter one before keeps it and still opens. Outside the limit to 256 refuses the boot | 8 (owner, 2026-10-25) | built (2026-10-22, 8 since 2026-10-25) | sets | | | | host/link_password.rs `Deployment` |
| `ORDO_SHARE_LINK_PASSWORD_MIN_LENGTH_LIMIT` | The shortest minimum any layer may choose. 1 to 256; 1 is the absolute floor. Below 8 is the deployment owner's opt-in to shorter link passwords | 8 | built (2026-10-22) | sets | | | | host/link_password.rs |
| `ORDO_SHARE_LINK_PASSWORD_SLOW_AFTER` | Default wrong passwords in a row on one link before each further try waits a doubling delay (1 s, 2 s, 4 s, ...), answered `429 too_many_attempts` with `Retry-After`. Always on; never locks; the right password resets it. Counted per link in the control database, shared by every node. `off` (or 0) turns it off. Outside 0 to the limit refuses the boot | 5 (owner, 2026-10-25) | built (2026-10-25) | sets | | | | host/link_password.rs; host/guess_limits.rs `Counter` |
| `ORDO_SHARE_LINK_PASSWORD_SLOW_AFTER_LIMIT` | The most wrong tries before slowing any layer may choose. 1 to 1000 | 100 | built (2026-10-25) | sets | | | | host/link_password.rs |
| `ORDO_SHARE_LINK_PASSWORD_SLOW_CAP_SECS` | Default longest wait of the link slowdown. Outside the limit to 86400 refuses the boot | 60 | built (2026-10-25) | sets | | | | host/link_password.rs |
| `ORDO_SHARE_LINK_PASSWORD_SLOW_CAP_SECS_LIMIT` | The shortest longest wait any layer may choose. 1 to 86400 | 1 | built (2026-10-25) | sets | | | | host/link_password.rs |
| `std.deployment.setLinkPasswordLimits` | The app developer's `tries` (a number or `off`), `lockSecs`, `minLength`, `slowAfter` (a number or `off`), `slowCapSecs`, and an optional range for tenants per value (`triesRange=2-10`), in the deployment's own space. Each inside the deployment's limits. `default` clears a value, `none` a range | none: the deployment's values and limits | built (2026-10-22) | | sets | | | host/share_links.rs `set_app_limits`; `$deploymentsetting` key `share_link_password_limits` |
| `std.space.setLinkPasswordLimits` | A tenant owner's own `tries`, `lockSecs`, `minLength`, `slowAfter`, `slowCapSecs`, looser or tighter (`slowAfter=off` only where the app developer's `slowAfterRange` starts at 0), inside the app developer's range, or the deployment's limits when there is none. Outside is refused (`link_password_limit_out_of_range`) naming the range and who sets it. The `link_locked` and the slowdown's `too_many_attempts` answers name this op. The app developer closes it with `ORDO_ACCESS_TEMPLATE="std.space.setLinkPasswordLimits="` | none: the app developer's, else the deployment's | built (2026-10-22) | | can close | sets | | host/share_links.rs `set_tenant_limits`; `$admissionpolicy.link_password_*` |
| `ORDO_SHARE_LINK_MAX_SECS` | A fixed deployment cap on link life | withdrawn, never built | withdrawn 2026-09-29 | | | | | plan-sharing.md:3 |
| `ORDO_SHARE_MAX_SECS` | Ceiling on a guest share's expiry | none: unset | built (G2) | sets | | could be exposed | could be exposed | host/share_ops.rs `MAX_SECS_ENV` |
| Share ops and rows per share | Most ops and rows one share or link may name | 64 ops, 100 rows | hardcoded (G2) | | could be exposed | | | host/share_ops.rs `MAX_OPS`, `MAX_ROWS` |
| Guest writes | A guest may write, not only read | read only | planned (18ab, after first deployment) | | | | | corpus-rulings.md, sharing answer 3 |
| Capability grant and delegation lifetime | Expiry the granting person states on each grant | stated per act | built | | | | exposable | host/grants.rs:237; host/delegation.rs:63 |

## 6 · Tenants and provisioning

| Setting | What it does | Default | Built? | Deployment | App developer | Tenant | Person | Where |
|---|---|---|---|---|---|---|---|---|
| `ORDO_MEMBER_LEFT_NAME` | Text that replaces a seat's `displayName` when its person leaves. Max 256 characters | unset, the name is kept | built (27aq, 2026-10-06) | | sets | | | host/release_ops.rs `LEFT_NAME_ENV` |
| `ORDO_ACCOUNT_REMOVAL` | When a deleted account is removed: `never`, `now`, or `<n>d` (or `<n>`) days. Accounts already deleted follow the setting in force: the due time is worked out from `deleted_at` at each pass, and `now` removes everyone waiting at boot. Anything else refuses the boot | `never` | built (27aq, 2026-10-06) | | sets | | | host/account_removal.rs `Removal` |
| `ORDO_SPACE` | The deployment's own tenant, which holds its members and settings | none, required | built | sets | | | | host/runtime.rs:626 |
| `ORDO_DEPLOYMENT_SPACE` | The tenant that may read `/metrics` and `/health/deep` | the fixture tenant | built | sets | | | | host/lib.rs:155-177 |
| `ORDO_PG_ROUTING` | Removed (2026-10-06). Every tenant has a database of its own. `attached` boots with a warning, any other value refuses the boot | removed | built | sets | | | | host/runtime.rs `legacy_routing` |
| `ORDO_PG_TENANT_HOST`, `ORDO_PG_TENANT_PORT`, `ORDO_PG_TENANT_USER`, `ORDO_PG_TENANT_CREDENTIAL` | Cluster where new tenant databases are created. Port and user fall to the control connection's own when unset | the deployment's own connection | built | sets | | | | host/tenant_ops.rs:145-161 |
| Where one tenant's database lives | Chosen when the tenant is created through `std.space.create` | the default cluster | built | sets | | | | host/tenant_ops.rs:74; host/provision.rs |
| `ORDO_TENANT_ATTACH_HOSTS` and `ORDO_PG_ATTACH_HOSTS` | Hosts a tenant database or bucket may attach to (two spellings, unioned) | none: attach refused | built | sets | | | | host/provision.rs:677-688 |
| `ORDO_TENANT_CREDENTIALS=dir` and `ORDO_TENANT_CREDENTIALS_DIR` | Optional directory of credentials. Needed only for a label other than `$deployment` (a per-tenant cluster's `$dbCredential`, an `ORDO_PG_TENANT_CREDENTIAL` label of its own) and for attached buckets | none | built | sets | | | | host/runtime.rs `tenant_credentials` |
| `ORDO_BOOTSTRAP_TIMEOUT_SECS` | How long a tenant `std.space.create` made may stay `provisioning` before it is called failed. A person's own tenant is never called failed (owner, 2026-10-21): the setup sweep keeps trying it | 900 s | built | sets | | | | host/workers.rs `ProvisioningReconciler`; host/tenants.rs `time_out_stale` |
| `ORDO_PROVISION_SWEEP_SECS` | How often every running server sweeps for own-tenant setups that stalled, takes the ones that are due and reports the stuck ones. 1 to 3600; outside that refuses the boot, naming the default | 30 | built (2026-10-21) | sets | could be exposed | | | host/tenant_setup.rs `SetupSettings` |
| `ORDO_PROVISION_LEASE_SECS` | How long one server holds the right to run one setup. Renewed while it runs, so it bounds only how long a server that stopped keeps the others waiting. 10 to 3600; outside that refuses the boot, naming the default | 120 | built (2026-10-21) | sets | | | | host/tenant_setup.rs; host/tenants.rs `claim_setup` |
| `ORDO_PROVISION_RETRY_BASE_SECS` | The wait after a setup's first failed try. Each later wait doubles. Setups are never given up on. 1 to 3600; outside that refuses the boot, naming the default | 15 | built (2026-10-21) | sets | could be exposed | | | host/tenant_setup.rs `backoff` |
| `ORDO_PROVISION_RETRY_CAP_SECS` | The longest wait between tries of one setup. At least the base. 1 to 86400; outside that refuses the boot, naming the default | 900 | built (2026-10-21) | sets | could be exposed | | | host/tenant_setup.rs `backoff` |
| `ORDO_PROVISION_STUCK_SECS` | How long a setup may take before it is reported stuck: in `GET /auth/provisioning`, once on the security-event stream, in `/health/deep` and as the gauge `ordo_tenant_setup_stuck`. It is still tried. 1 to 86400; outside that refuses the boot, naming the default | 300 | built (2026-10-21) | sets | could be exposed | | | host/tenant_setup.rs; host/self_serve.rs `OwnStatus` |
| `ORDO_PROVISION_NOT_READY_RETRY_SECS` | The `Retry-After` on `503 tenant_not_ready`, the refusal a person's call into their own tenant gets before it is set up. 1 to 300; outside that refuses the boot, naming the default | 2 | built (2026-10-21) | sets | could be exposed | | | host/tenant_setup.rs `NotReady`; api/router.rs `check_access` |
| `ORDO_TENANT_PAUSE_ROLES` | Tenant roles that may pause their own tenant (`std.space.pause`). The deployment owner always may | none: only the deployment owner | built (27ar) | sets | could be exposed | | | host/pause.rs `PausePolicy` |
| `ORDO_TENANT_RESUME_ROLES` | Tenant roles that may lift a pause made from inside the tenant (`std.space.resume`). Never a deployment pause | the pause roles | built (27ar) | sets | could be exposed | | | host/pause.rs `PausePolicy` |
| `ORDO_TENANT_PAUSE_ANSWERS` | What a paused tenant answers, per reason word: the HTTP status (any 4xx or 503) and the message text. JSON, e.g. `{"billing":{"status":402,"message":"The plan is unpaid."}}`. The class stays `space_paused`. An unknown reason or a status outside the range refuses the boot | 423 and the built-in message for every reason | built (2026-10-06) | sets | could be exposed | | | host/pause.rs `PauseAnswers` |
| `nopause` on an op | The op keeps answering while its tenant is paused | off: refused `space_paused` | built (27ar) | | sets | | | syntax/declparse.rs; host/pause.rs `gate` |

## 7 · Databases and pools

| Setting | What it does | Default | Built? | Deployment | App developer | Tenant | Person | Where |
|---|---|---|---|---|---|---|---|---|
| `ORDO_PG_HOST` | Control database host | none, required | built | sets | | | | host/runtime.rs:564 |
| `ORDO_PG_PORT` | Port | 5432 | built | sets | | | | host/runtime.rs:573 |
| `ORDO_PG_USER` | Database user | none, required | built | sets | | | | host/runtime.rs:568 |
| `ORDO_PG_DB` | Database name | none, required | built | sets | | | | host/runtime.rs:569 |
| `ORDO_PG_PASSWORD` and `ORDO_PG_PASSWORD_FILE` | Password, file form preferred. Both set, or an unreadable file, refuses the boot | none | built | sets | | | | exec/lib.rs:131-135 |
| `ORDO_PG_POOL_SIZE` | Connections in the pool | cores, clamped to 2..8 | built | sets | | | | host/runtime.rs:574; exec/lib.rs:136-152 |
| `ORDO_PG_IDLE_SECS` | Seconds an idle pool connection is kept before it closes. `0` keeps them | 30 s | built | sets | | | | exec/lib.rs `PoolLimits` |
| `ORDO_PG_TENANT_DIAL_CONCURRENCY` | How many tenant databases the boot pass, the sweeps after new tenants and a deploy dial at once. Each walk opens a connection to a tenant, uses it and closes it, so a server with 121 tenants holds a few connections at a time instead of 121. 1 to 64; outside refuses the boot naming the range and the default | 8 | built | sets | | | | exec/route.rs `AttachedPools`; host/runtime.rs refuses out of range |
| `ORDO_PG_TENANT_DIAL_WAIT_MS` | How long a walker (boot pass, catch-up sweep, deploy) waits for a free dial slot (the cap above) before giving up on that tenant. The step for that tenant then fails with `dial_wait_exceeded`, which names this setting. The walker reports the tenant held, as it does an unreachable one, and carries on with the rest; the next pass tries it again. 1 to 3600000; outside refuses the boot naming the range and the default | 60000 | built | sets | | | | exec/route.rs `wait_for_dial_gate`; host/runtime.rs refuses out of range |
| `ORDO_PG_ACQUIRE_TIMEOUT_MS` | Wait for a free connection | 2000 ms | built | sets | | | | host/runtime.rs:581 |
| `ORDO_PG_STATEMENT_TIMEOUT_MS` | Longest a statement may run, 0 for none | 30000 ms | built | sets | could be exposed | | | host/runtime.rs:589; exec/lib.rs:275-288 |
| `ORDO_PG_STORE_POOL_SIZE` | Connections in the door's store pool, which the sign-in stores and the keystore share, apart from the request pool. Each store runs one statement at a time, so this is how many stores can be in the database at once. More than 1 lets the others carry on while one commit is slow (board task `store-pool-held-blocks-signin`: one fsync took 2 s and every other sign-in store was refused `server_busy`). Dialled on demand and closed when idle, so a quiet server holds 1. 1 to 32; outside refuses the boot naming the range and the default | 4 | built | sets | | | | host/runtime.rs `store_pool_config` |
| `ORDO_PG_STORE_ACQUIRE_TIMEOUT_MS` | How long a store waits for a free connection in that pool before it is refused `server_busy`. Long enough to ride out a disk stall that slows every commit at once (2 s seen on CI), well inside the 30 s statement bound. The request pool keeps `ORDO_PG_ACQUIRE_TIMEOUT_MS`. 1 to 60000; outside refuses the boot naming the range and the default | 10000 ms | built | sets | | | | host/runtime.rs `store_pool_config` |
| `ORDO_PG_BUSY_SNAPSHOT` | When the door's store pool turns a sign-in away as busy, also log Postgres's view once: this deployment's connections, what each waits on, who blocks it, and the locks waited for. Taken through a connection of its own with a 2 s bound, at most once a minute, statements shown without their values. `on` or `off`; anything else refuses the boot | on | built | sets | | | | exec/incident.rs; host/runtime.rs |
| `ORDO_PG_SPACE_RESERVE` | Connections held out of one tenant's reach | 0 | built | sets | | | | host/runtime.rs:590; exec/lib.rs:292 |
| `ORDO_REGEX_MAX_LEN` | Longest `matches` pattern a statement runs, in characters. A longer one, or one holding a backreference (`\1` to `\9`), is refused as `invalid_param` before the regex engine runs it. 1 to 10000; outside refuses the boot naming the range and the default | 256 | built | sets | | | | exec/lib.rs `regex_max_len`; plan/lib.rs `regex_guard`; host/runtime.rs refuses out of range |
| `ORDO_WRITE_RETRIES` | How many more times a write is tried after it lost a race with another write to the same rows (`40001`). 0 means one try. When every try loses, the write is refused as `serialization_conflict`, 409 and retryable, naming this setting and its value, and the SDK sends it again. 0 to 20; outside refuses the boot naming the range and the default | 5 | built | sets | | | | exec/lib.rs `PoolLimits`; host/runtime.rs refuses out of range |
| `ORDO_WRITE_RETRY_MAX_MS` | The longest pause between two tries of such a write. The pause before try n is a random time from 0 to the smaller of this and 4 ms times 2 to the n. With the default retries the windows are 8 to 128 ms, a worst total of about 250 ms. 1 to 10000; outside refuses the boot | 250 ms | built | sets | | | | exec/lib.rs `backoff`; host/runtime.rs |
| `ORDO_APPLY_LOCK_TIMEOUT_MS` | How long one schema statement waits for its table lock before it gives up that attempt (`lock_timeout`, plan-evolution-plans §3.7) | 2000 ms | built | sets | | | | host/schema_apply.rs `LockPolicy`; host/runtime.rs refuses a non-number |
| `ORDO_APPLY_LOCK_RETRIES` | How many more times the apply tries a statement that waited out its lock. 0 means one attempt | 5 | built | sets | | | | host/schema_apply.rs `LockPolicy` |
| `ORDO_APPLY_LOCK_BACKOFF_MS` | The pause before the first retry. It doubles on each retry after that | 500 ms | built | sets | | | | host/schema_apply.rs `LockPolicy` |
| `ORDO_APPLY_SAFE_CHANGES` | Whether `ordo apply` (and `std.deployment.apply`) makes safe additive schema changes itself: a new field with a constant default (A3), a new optional field, a new index, a new table, any other class A change. It runs them the way `ordo migrate --apply` does and records them in `$migrations`. A change that needs a probe or could lose or reject data stays a refusal that prints the `ordo migrate --apply` command for this database. `off` restores the refusal for every change. Any value but on or off refuses the boot | on | built | sets | | | | host/schema_apply.rs `apply_safe`; host/apply_op.rs; host/runtime.rs refuses a bad value |
| Reserve per attached database | A pool size and reserve for each attached database | none | planned (plan-space-routing, 18v) | planned | | | | plan-space-routing.md:793 |

## 8 · Storage and blobs

| Setting | What it does | Default | Built? | Deployment | App developer | Tenant | Person | Where |
|---|---|---|---|---|---|---|---|---|
| `ORDO_BLOB_STORE` | The deployment's store: `s3` or `fs:<path>` (dev only) | none | built | sets | | | | host/runtime.rs:1210 |
| `ORDO_BLOB_S3_ENDPOINT` | The S3 endpoint URL | none, required with `s3` | built | sets | | | | blob/s3.rs:179 |
| `ORDO_BLOB_S3_REGION` | The bucket's region | none, required with `s3` | built | sets | | | | blob/s3.rs:180 |
| `ORDO_BLOB_S3_BUCKET` | The bucket name | none, required with `s3` | built | sets | | | | blob/s3.rs:181 |
| `ORDO_BLOB_S3_ACCESS_KEY_ID` | The access key id | none, required with `s3` | built | sets | | | | blob/s3.rs:182 |
| `ORDO_BLOB_S3_SECRET_ACCESS_KEY` | The access key's secret. Or `ORDO_BLOB_S3_SECRET_ACCESS_KEY_FILE`, a file holding it (preferred) | none, required with `s3` | built | sets | | | | blob/s3.rs:183 |
| `ORDO_BLOB_S3_RETRIES` | How many times a read, a HEAD or a delete is sent again after S3 answers a 5xx, 408, 425 or 429, or a connection fails. An upload is never resent. 0 to 10; 0 turns it off; outside that refuses the boot | 2 | built | sets | | | | blob/s3.rs (`S3Retry::from_vars`) |
| `ORDO_BLOB_S3_RETRY_BASE_MS` | The wait before the first retry, in milliseconds. It doubles after each, up to 2 s. 1 to 10000; outside that refuses the boot | 200 | built | sets | | | | blob/s3.rs (`S3Retry::from_vars`) |
| `ORDO_BLOB_ROUTING` | `single` store or `attached` buckets per tenant | `single` | built | sets | | | | host/runtime.rs:762 |
| `ORDO_BLOB_GC_SECS` | How often the storage sweep runs. One cadence for the whole deployment, every database and every store (ruled 2026-09-21, `st-gc-cadence`). Zero or unreadable is the default | 3600 s | built | sets | | | | host/workers.rs `BlobGc::period_from_env` |
| Attach a bucket (`std.space.attachBucket`) | A tenant owner points its files at a bucket of its own | not attached | built | ceiling only | | sets | | host/bucket_ops.rs:98; host/platform_ops.rs:1125 |
| `ORDO_DEV_UNSAFE_BLOB_STORE` | Allows `fs:` in a deployment | off | built | sets | | | | host/runtime.rs:1242 |
| `ORDO_DEV_UNSAFE_BUCKET_ENDPOINT` | Allows a non-HTTPS or loopback bucket endpoint | off | built | sets | | | | host/bucket_ops.rs:233,294 |
| `ORDO_EXIT_BUNDLE_DIR` | Where export writes bundles. Unset turns export off | none | built | sets | | | | host/runtime.rs:986 |
| `ORDO_IMPORT_BUNDLE_DIR` | Where import reads staged bundles. Unset turns import off | none | built | sets | | | | host/runtime.rs:1010 |
| `ORDO_FN_CACHE_DIR` | Where the compiled backend function engine is kept between starts. A start that finds it skips about a second of compiling. Ordo makes the directory with mode 0700 and loads from it only while this user owns it and no other user can write to it. A corrupt or stale file is compiled again, never an error. `off` compiles at every start. A relative path refuses the boot | $XDG_CACHE_HOME/ordo/fn, else $HOME/.cache/ordo/fn | built | sets | | | | host/functions.rs `engine_cache_from_vars`; fn/cache.rs |
| `op … download` | A person's own download, `GET /download`. Off until the program marks a read op `download`. Each marked op is one entity: its fields, its filter, and its files when it answers a blob field. The row guard decides the rows (`howto-leave.md`) | off | built | | sets | | | plan/download.rs; api/download.rs |
| `ORDO_OWN_DOWNLOAD_PER_HOUR` | How many downloads one person may start in one tenant in an hour, per node. Past it, `quota_exceeded` with `Retry-After`. At least 1 | 6 | built | sets | | | | api/download.rs `DownloadLimits` |
| `ORDO_OWN_DOWNLOAD_MAX_BYTES` | The most bytes of files one download holds. A file past it is listed as missing, and the manifest says `complete: false`. At most 4000000000 | 1073741824 (1 GiB) | built | sets | | | | api/download.rs `DownloadLimits` |
| Blob field tier (`inline`, `private`) | Whether a field is stored inline or as an object | object | built | | sets | | | syntax/declparse.rs:913-923 |
| Blob upload cap | Largest single upload | 100 MiB | hardcoded | could be exposed | could be exposed | could be exposed | | blob/policy.rs:56 |
| Blob inline cap | Largest inline write | 256 KiB | hardcoded | could be exposed | could be exposed | | | blob/policy.rs:55 |

## 9 · Metering and quotas

| Setting | What it does | Default | Built? | Deployment | App developer | Tenant | Person | Where |
|---|---|---|---|---|---|---|---|---|
| `ORDO_USAGE_LEDGER` | Turns the usage ledger and quotas on | off | built | sets | | | | host/metrics.rs:397 |
| `ORDO_USAGE_FLUSH_SECS` | How often each replica writes its counters | 60 s | built | sets | | | | host/runtime.rs:865 |
| `ORDO_USAGE_ROLLUP_SECS` | Rollup period, the billing grain | 3600 s | built | sets | | | | host/runtime.rs:887 |
| `ORDO_REPLICA_ID` | This process's name in the ledger | 8 random hex characters | built | sets | | | | host/runtime.rs:861 |
| `ORDO_METRICS_PER_SPACE` | Adds a tenant label to some metrics | off | built | sets | | | | host/metrics.rs:388 |
| `ORDO_OBSERVED_COST_MAX_OPS` | How many ops the observed cost table holds | 4096 | built | sets | | | | host/observed_cost.rs |
| `ORDO_OBSERVED_COST_MAX_SPACE_ENTRIES` | How many (tenant, op) entries it holds; `0` keeps none | 10000 | built | sets | | | | host/observed_cost.rs |
| Quota per tenant and quantity | Limit, grace and period, by `ordo quota set` | none | built | sets | | | | host/quotas.rs:43-110; cli/operator.rs:2191 |
| Quota quantities | Which quantities can be limited | requests, rows_returned | hardcoded | could be exposed | | | | host/quotas.rs:48 |
| Limit on writes | A quota on writes | none | planned (18i, after first deployment) | planned | | | | host/quotas.rs:26,46 |

## 10 · The security stream and audit

| Setting | What it does | Default | Built? | Deployment | App developer | Tenant | Person | Where |
|---|---|---|---|---|---|---|---|---|
| `ORDO_DENY_ALERT_THRESHOLD` | Denials from one person in one tenant that raise the alert | 50 | built | sets | | could be exposed | | secevent/alert.rs:89,113 |
| `ORDO_DENY_ALERT_WINDOW_SECS` | Window for that count | 300 s | built | sets | | could be exposed | | secevent/alert.rs:91,114 |
| `ORDO_DEPLOYMENT_SECLOG_RETENTION_DAYS` | How long the deployment's own security stream is kept | 90 days | built | sets | | | | secevent/store.rs:482-490 |
| Tenant security stream retention | How long a tenant's stream is kept | 90 days | hardcoded | could be exposed | could be exposed | could be exposed | | secevent/store.rs:490; host/secdrain.rs:283 |
| `ORDO_DEPLOYMENT_SECLOG` | Turns the deployment stream on or off | on | planned (plan-opslog) | planned | | | | plan-opslog.md:264 |
| `ORDO_WORKER_PRUNE` | Turns automatic command log pruning on | off | built | sets | | | | host/runtime.rs:4099 |
| Command log undo horizon | Fine-grained revert reaches back this far | 90 days | hardcoded | could be exposed | could be exposed | could be exposed | | exec/cadence.rs:117 |
| Revert rounding of money past a rescale | What a revert does when converting an old amount would lose digits: `Refuse` or `HalfEven`. Per field over program-wide | Refuse (off) | engine only (Rust struct `RevertPolicy`; no variable, flag or program spelling, so `ordo revert` always uses the default and refuses) | | could be exposed | | | exec/undo.rs (`RevertPolicy`) |
| Command log checkpoint interval | Time between signed anchors | 30 days | hardcoded | could be exposed | could be exposed | | | exec/cadence.rs:114,118 |
| Command log attest cadence | Signed head every N commands or T | 1000 or 1 h | hardcoded | could be exposed | could be exposed | | | exec/cadence.rs:119-120 |
| Command log consumer stall | Idle mirror cursor leaves the prune floor after this | 30 days | hardcoded | could be exposed | could be exposed | | | exec/cadence.rs:121 |
| Security event collapse | Window and cap for merging repeated events | 60 s, 256 | hardcoded | could be exposed | | | | secevent/collapse.rs:89,97 |
| Security checkpoint period | Time between signed security checkpoints | 1 h | hardcoded | could be exposed | | | | exec/secevent.rs:1020 |
| Attestation destination (`std.space.attestTo`) | Owner-signed address that receives signed heads | none | built | | | sets | | host/attest_ops.rs:77; host/platform_ops.rs:1121 |
| `ORDO_EGRESS_DENY_CIDRS` | Extra address ranges outbound delivery must never reach. Also covers outside sign-in providers | none | built | sets | | | | host/attest_ops.rs:487; host/runtime.rs:3988 |
| Egress timeout and body cap | Time and reply size for outbound delivery. The 20 s is also the mail send timeout (host/mail.rs) | 20 s, 8 KiB | hardcoded | could be exposed | | could be exposed | | egress/client.rs:60,63 |

## 11 · CORS and the door

| Setting | What it does | Default | Built? | Deployment | App developer | Tenant | Person | Where |
|---|---|---|---|---|---|---|---|---|
| `ORDO_HOST_ADDR` | Address the door listens on | `127.0.0.1:8080` | built | sets | | | | host/runtime.rs:618 |
| `ORDO_CORS_ORIGINS` | Pages allowed to call the door. Exact origins, or `https://*.<domain>` with `ORDO_WILDCARD_ORIGINS=on`. The answer names the one asking origin, never `*`. The bare `*` refuses the boot | none: same origin only | built | sets | could be exposed | | | host/runtime.rs:829; api/cors.rs |
| `ORDO_CORS_TENANT_ORIGINS_MAX` | How many origins one tenant may list with `std.space.setOrigins`. Exact origins only. 0 lets no tenant list any: the ops refuse `tenant_refused`, naming this setting. 0 to 1000; A value outside the range, or not a whole number, refuses the boot naming the range and the default | 0 | built | sets | | sets, up to the max | | host/tenant_origins.rs; api/cors.rs |
| `ORDO_SHUTDOWN_GRACE_SECS` | Wait for requests to finish at shutdown | 30 s | built | sets | | | | host/runtime.rs:623 |
| `ORDO_LIVE_IDLE_CLOSE_SECS` | How long a shared live stream (`GET /live`) with no reads stays open | 60 s | built | sets | could be exposed | | | api/live.rs `LiveSettings`; host/runtime.rs refuses a non-number |
| `ORDO_SYNC_PAGE_ROWS` | Rows in each `load` frame of a sync feed, and in each page of its difference pass. The interactive read page stays 50. Outside 1 to 1,000, or not a number, refuses the boot naming the setting | 50 | built | sets | could be exposed | | | api/sync.rs `SyncPacing`; host/runtime.rs `sync_pacing_from` |
| `ORDO_SYNC_PAGE_WAIT_MS` | The wait between one sync page read and the next, in a load and in a difference pass. 0 loads as fast as the database answers. Outside 0 to 10,000, or not a number, refuses the boot naming the setting | 50 ms | built | sets | could be exposed | | | api/sync.rs `SyncPacing`; host/runtime.rs `sync_pacing_from` |
| `ORDO_LIVE_MAX_READS` | The most reads one shared live stream carries; a screen with more opens another stream | 256 | built | sets | could be exposed | | | api/live.rs `LiveSettings`; host/runtime.rs refuses 0 and a non-number |
| `ORDO_LIVE_RECHECK_SECS` | How often a quiet live feed (`?live=1`, a sync, a read on `/live`) asks the door again with no write: the token gate (delegation, share) and the access check (membership, session). A refusal ends the feed with an `error` frame. A feed also asks before every pass. Outside 1 to 3,600, or not a number, refuses the boot naming the setting | 30 s | built | sets | could be exposed | | | api/live.rs `LiveSettings`; api/router.rs `Door`; host/runtime.rs refuses out of range |
| `ORDO_REFINE_MAX_CONDITIONS` | The most conditions one `$refine` carries. Past it the door refuses with `invalid_refinement`, naming the limit and this setting. The interface descriptor states it as `refine.limits.conditions` | 16 | built | sets | could be exposed | | | api/refine.rs `RefineLimits`; host/runtime.rs refuses 0 and a non-number |
| `ORDO_REFINE_MAX_IN_VALUES` | The most values in one `in` condition of a `$refine`. Refused the same way. Descriptor: `refine.limits.in_values` | 200 | built | sets | could be exposed | | | api/refine.rs `RefineLimits`; host/runtime.rs refuses 0 and a non-number |
| `ORDO_JSON_MAX_BYTES` | The largest `json` document a parameter may carry, in bytes of its compact form. Past it the door answers `bound_violation`, naming this setting | 262144 (256 KiB) | built | sets | could be exposed | | | api/json.rs `JsonLimits`; host/runtime.rs refuses 0 and a non-number |
| `ORDO_JSON_MAX_DEPTH` | How deeply arrays and objects may nest in one `json` document. Refused the same way | 64 | built | sets | could be exposed | | | api/json.rs `JsonLimits`; host/runtime.rs refuses 0 and a non-number |
| `ORDO_TENANT_CATCHUP_SECS` | How often a replica catches up with changes forwarded between tenants. Zero or unreadable is the default | 15 s | built | sets | | | | host/tenant_forward.rs `period_from_env` |
| `ORDO_ROUTINE_EACH_MAX_ROWS` | The most rows one run of an `each ->` routine handles. Zero or unreadable is the default | 500 | built | sets | | | | plan/each.rs `each_max_rows_from_env` |
| Request body limit | Largest request body | 2 MiB | hardcoded | could be exposed | could be exposed | | | api/router.rs:137 |
| Page size for reads | Rows per page | 50 | hardcoded | could be exposed | could be exposed | | | host/runtime.rs:3748 and :3938 |
| Idempotency window | How long a stored response answers a retry | 24 h | hardcoded | could be exposed | | | | api/idempotency.rs:136; host/replay.rs:157 |
| Idempotency reservation and entry cap | Takeover time and cache size | 60 s, 10 000 | hardcoded | could be exposed | | | | api/idempotency.rs:141,145 |
| CORS preflight cache | How long a browser may cache a preflight | 600 s | hardcoded | could be exposed | | | | api/cors.rs:69 |
| Stream heartbeat | Keep-alive interval on streaming reads | 15 s | hardcoded | could be exposed | | | | api/router.rs:52 |

## 12 · Workers and periods

| Setting | What it does | Default | Built? | Deployment | App developer | Tenant | Person | Where |
|---|---|---|---|---|---|---|---|---|
| `ORDO_WORKERS` | Which workers this process runs (role by config) | all | built | sets | | | | host/runtime.rs:4153; host/supervisor.rs:487 |
| `ORDO_WORKER_<NAME>_TICK_MS` | Overrides one worker's tick, never a schedule period | each worker's own | built | sets | | | | host/supervisor.rs:504-538 |
| `ORDO_ACCESS_REFRESH_SECS` | Longest a replica keeps serving a removed member | 30 s | built | sets | | | | host/workers.rs:1106-1119 |
| `ORDO_DOOR_STALE_GRACE_SECS` | How long the door keeps serving a space's last good member list when rebuilding it fails for a passing reason (deadlock, serialization conflict, timeout, full pool). Retried meanwhile. Past it, or on a lasting error, the space is denied. A removed member keeps access at most this long. `0` denies at once. A bad value refuses the boot | 60 s | built | sets | | | | host/directory.rs `stale_grace_from`; host/runtime.rs |
| `ORDO_SESSION_REFRESH_SECS` | Longest a replica keeps honouring an ended session | 15 s | built | sets | | | | host/workers.rs:1229-1239 |
| `ORDO_TRIGGERS_IDLE_MAX_SECS` | Longest gap between two `triggers.drain` visits to a tenant whose chain has not moved. Zero or unreadable is the default | 10 s | built | sets | | | | host/routines.rs `idle_max_from_env` |
| `ORDO_NODE_BEAT_SECS` | How often a node writes its row for `/health/deploy` | 5 | built | sets | | | | host/node_status.rs (BEAT_SETTING); host/runtime.rs |
| `ORDO_FN_RUN_RETENTION_DAYS` | Days a backend function's run and its log are kept in `$fnruns`. The `functions.runs.sweep` worker deletes older runs once an hour. 1 to 3650; anything else refuses the boot and names this setting | 14 | built | sets | | | | host/fn_runs.rs (RETENTION_ENV) |
| Supervisor timing | Backoff unit, leader tick, census period | 1 s, 5 s, 60 s | hardcoded | could be exposed | | | | host/supervisor.rs:181-183 |
| Quarantine threshold | Failed runs before a listener is quarantined | 10 | hardcoded | could be exposed | | | | host/supervisor.rs:107 |
| Trigger attempts | Runs before a dead letter | 5 | hardcoded | could be exposed | could be exposed | | | exec/triggers.rs:527 |
| Trigger schedule (`every 5m`, `every day 02:00Z`) | When a routine or listener runs | none | built in the parser, no listener registered by the binary (12a) | | sets | | | syntax/trigger.rs:313-345; host/runtime.rs:4213 |
| Trigger debounce and max wait | Trailing window and its ceiling | none, max wait 10 windows | built in the parser, max wait hardcoded | | sets | | | syntax/trigger.rs:531-543 |
| Registration sweep period | How often unadmitted sign-ups are cleared | min(600 s, lifetime) | hardcoded | could be exposed | | | | host/registrations.rs:399 |
| Ceremony sweep period | How often expired ceremonies are cleared | 60 s | hardcoded | could be exposed | | | | host/workers.rs:1174 |
| Prune worker period | How often pruning runs when on | 1 h | hardcoded | could be exposed | | | | host/runtime.rs:4106 |

## 13 · Mail

Step 4 of the auth-federated plan built the channel: SMTP (implicit TLS, required STARTTLS, or plaintext only when `ORDO_MAIL_TLS=none` is named),
Resend and SendGrid. It is off by default. The owner narrowed version 1 to
these three (2026-09-29); the other kinds wait for 18z.

| Setting | What it does | Default | Built? | Deployment | App developer | Tenant | Person | Where |
|---|---|---|---|---|---|---|---|---|
| `ORDO_MAIL_KIND` | `smtp`, `resend` or `sendgrid`. Unset is off. `mailgun`, `postmark`, `brevo`, `mailjet` and `ses` refuse the boot and name 18z | none (off) | built | sets | | | | host/mail.rs (`MailChannel::from_vars`) |
| `ORDO_MAIL_FROM` | The sender address, bare (no display name). Required when mail is on | none | built | sets | | | | host/mail.rs (`MailChannel::from_vars`); egress/smtp.rs |
| `ORDO_MAIL_HOST` | `smtp` only: the relay host name, no scheme. Required for `smtp` | none | built | sets | | | | host/mail.rs (`MailChannel::from_vars`); egress/smtp.rs |
| `ORDO_MAIL_PORT` | `smtp` only: the relay port. Any port from 1 to 65535. 25 and 587 are allowed | 465 | built | sets | | | | host/mail.rs (`MailChannel::from_vars`); egress/smtp.rs |
| `ORDO_MAIL_TLS` | `smtp` only: `implicit` (port 465), `starttls` (port 587) or `none` (no encryption). `starttls` is required, never opportunistic: if the relay does not offer it, the send is refused and nothing falls back to plaintext. `none` is plaintext SMTP. It is used only when named and is never a fallback. With `none`, the relay password and every message cross the network readable. The startup line says NO TLS. Use it for a local relay or a mock such as Mailpit. Any other value refuses the boot and lists the three. A refused send names the setting to change in the worker's log line (owner, 2026-10-08) With `resend` or `sendgrid` it refuses the boot ("does not apply") | implicit | built | sets | | | | host/mail.rs (`MailChannel::from_vars`); egress/smtp.rs |
| `ORDO_MAIL_CREDENTIALS_FILE` | `smtp` only: path to a file with the user on line 1 and the password on line 2. Optional. Without it no `AUTH` is sent, the startup line says "no login", and a relay that answers 530 (authentication required) gets a refusal naming this setting. `ORDO_MAIL_PASSWORD` as a value refuses the boot | none | built | sets | | | | host/mail.rs (`MailChannel::from_vars`); egress/smtp.rs |
| `ORDO_MAIL_KEY_FILE` | `resend` and `sendgrid` only: path to a file holding the API key. Required. `ORDO_MAIL_KEY` as a value refuses the boot | none | built | sets | | | | host/mail.rs (`MailChannel::from_vars`); egress/smtp.rs |
| `ORDO_MAIL_MAX_ATTEMPTS` | Tries per message before it is dropped. 1 to 20; outside that refuses the boot | 5 | built | sets | | | | host/mail.rs (`MailChannel::from_vars`) |
| `ORDO_MAIL_SEND_SECS` | How often the `mail.send` worker reads the outbox, in seconds. 1 to 60 | 2 | built | sets | | | | host/mail.rs (`MailChannel::from_vars`) |
| `ORDO_MAIL_RETRY_BASE_SECS` | Wait before the second attempt, in seconds. It doubles after each failure. 1 to 3600; outside that refuses the boot | 15 | built | sets | | | | host/mail.rs (`MailChannel::from_vars`) |
| `ORDO_MAIL_RETRY_MAX_SECS` | The longest wait between attempts, in seconds. 1 to 86400, and at least the base; outside that refuses the boot. A relay's `Retry-After` (a 429 or 5xx) is waited out even when longer, up to one day | 900 | built | sets | | | | host/mail.rs (`MailChannel::from_vars`) |
| `ORDO_MAIL_BATCH` | Rows the worker claims per pass. 1 to 500; outside that refuses the boot | 16 | built | sets | | | | host/mail.rs (`MailChannel::from_vars`) |
| `ORDO_MAIL_LEASE_SECS` | How long a claimed row is hidden from other replicas, in seconds. 30 to 3600; outside that refuses the boot. Each row is claimed one at a time and leased when its own send starts, so a lease shorter than a batch cannot let another replica send the tail twice. The lease covers one send; the floor is above the 20 s send timeout. Delivery is still at least once if a replica crashes mid-send; every try of one email carries the same `Message-ID` | 120 | built | sets | | | | host/mail.rs (`MailChannel::from_vars`) |
| `ORDO_MAIL_SUBJECT_MAX_BYTES` | The longest mail subject, in bytes, printable ASCII. 20 to 998 (the header line limit); outside that refuses the boot and names the default. A longer subject is refused when the message is made | 200 | built | sets | | | | egress/smtp.rs (`subject_ok`); host/runtime.rs (`install_limits`) |
| `ORDO_MAIL_TEXT_MAX_BYTES` | The longest mail text, in bytes. 1024 to 1048576; outside that refuses the boot and names the default. Each line is still at most 998 bytes | 16384 (16 KiB) | built | sets | | | | egress/smtp.rs (`text_ok`); host/runtime.rs (`install_limits`) |
| `ORDO_EGRESS_PRIVATE_HOSTS` | Comma list of exact host names, exact IP addresses or CIDR ranges (IPv4 and IPv6, `10.0.0.0/8`, `fd12::/16`) the deployment owner lets through the egress deny list. A listed host, or an address in a listed range, may reach loopback, RFC1918, unique-local, CGNAT or the deployment's own ranges (`ORDO_EGRESS_DENY_CIDRS`). It never opens link-local, the cloud metadata addresses (169.254.169.254, fd00:ec2::254, 100.100.100.200), multicast or reserved addresses, even inside a listed range. Names are case-insensitive. A CIDR with host bits set (`10.1.0.0/8`), a wildcard, a port or a scheme refuses the boot and names this setting. Parsed once at every boot, whether or not mail is on, and applied to every outbound call: mail, OIDC, OAuth2 and Turnstile. A space's sinks never read it. The startup line lists the entries. Replaces the earlier name `ORDO_EGRESS_OPERATOR_HOSTS` | none (empty) | built (mail and sign-in connections; functions are planned to read the same list) | sets | | | | egress/deny.rs (`DenyList::check_host`); host/mail.rs; host/oidc.rs; plan-auth-federated.md §3.4, §7.6 |

## 14 · Dev and tooling

| Setting | What it does | Default | Built? | Deployment | App developer | Tenant | Person | Where |
|---|---|---|---|---|---|---|---|---|
| `ORDO_DEV_ADDR` | Address `ordo dev` serves on | `127.0.0.1:4000` | built | | sets | | | cli/dev.rs:866 |
| `ORDO_PGBIN` | Directory of Postgres tools for the local cluster | probes known paths | built | | sets | | | cli/dev_db.rs:211 |
| `ORDO_PG_*` under `ordo dev` | Points the loop at an existing database | own cluster | built | | sets | | | cli/dev.rs:977; cli/dev_db.rs:81-85 |
| `ORDO_HOST`, `ORDO_TOKEN`, `ORDO_SPACE` (CLI) | Where `ordo apply`, `space create` and `import` send the call | `ordo.inf.smg.icu`, none, none | built | | sets | | | cli/wire.rs:33,72,125 |
| `--as`, `--db`, `--control-db`, `--key-id`, and similar CLI flags | Choose the actor, database or key for one command. None stores a setting | none | built | sets | | | | cli/operator.rs:99-135 |
| `ordo quota set` | Writes a quota row (see section 9) | none | built | sets | | | | cli/operator.rs:2191 |
| `ORDO_KEY_ABBREVIATIONS` | File of extra key abbreviations for generated names | none | built | | sets | | | schema/lib.rs:425 |
| `ORDO_PLAYGROUND_ADDR` | Address the playground serves on | `127.0.0.1:7878` | built | | sets | | | playground/main.rs:37 |
| `ORDO_INTERFACE_PATH` | Set by `ordo dev` for the `--gen` command. Not a setting to write | none | built | | | | | cli/dev.rs:531-539 |
| `ORDO_UPDATE_GOLDEN`, `ORDO_TEST_DB_SUFFIX`, `ORDO_PSQL` | Test harness switches | none | built | | | | | conformance/src/golden.rs:16; conformance/src/lib.rs:97,114 |
| Console | Editing deployment settings in a browser | none | planned (dashboard-plan) | planned | | | | dashboard-plan.md |

## Settings that disagree with the principle

Each is a single deployment-wide value where the owner's rule wants layers.

| Setting | Why it disagrees | Recommendation |
|---|---|---|
| `ORDO_SESSION_SECS`, `ORDO_SESSION_MAX_SECS` | One life for every tenant and person | Done 2026-10-21 for the app developer and the tenant (`std.deployment.setSessionPolicy`, `std.space.setSessionPolicy`). A person's own layer is still to build |
| `ORDO_AUTH_TEMPORARY_PASSWORDS` | One switch for every tenant. The owner made typed passwords a recorded option | Keep it as the ceiling. Store the tenant's choice like the admission policy |
| `ORDO_STEP_UP_EXEMPT_OPS` | One list for every tenant and every op. The owner said the app developer decides | Give it a program spelling. Let a tenant tighten its own tenant-scoped ops |
| Step-up freshness (300 s) | Fixed for everyone | Make it a deployment setting, then an app developer setting, tightened by a tenant |
| `ORDO_AUTH_REGISTRATION_TTL_SECS` | The owner said this is "really up to the app dev" and it is deployment only | Keep the deployment default. Add an app developer value beside it |
| `ORDO_DENY_ALERT_THRESHOLD`, `_WINDOW_SECS` | One threshold for every tenant. Busy and quiet tenants differ | Keep the default. Let a tenant set its own threshold on its own stream |
| `ORDO_DEPLOYMENT_SECLOG_RETENTION_DAYS` and the tenant stream's fixed 90 days | Only the deployment stream has a setting. Tenant retention has none | Add tenant retention. Deployment sets the default, the tenant may lengthen it |
| `BlobPolicy.share_ttl_default` (7 days) | Was a ceiling (`share_ttl_max`); now the default when nothing is set | Done 2026-09-29. The tenant and link layers of `plan-sharing.md` still to build |
| `MAX_TTL_SECS` (30 days) on tickets, cited as the link cap in `plan-sharing.md:304` | A fixed system cap. The owner ruled no platform maximum | Take the cap from the deployment default and the tenant's choice, not a constant |
| `ORDO_SHARE_MAX_SECS` (built, no ceiling unless set) | A deployment ceiling on shares | Re-plan it as a default with a tenant choice, like link lifetime |
| Command log policy and blob policy, one value in every tenant | The language document says the app developer writes them, and `policy` does not parse | Parse `policy`. Deployment default, app developer sets, tenant may tighten where safe |
| Page size 50 and body limit 2 MiB | `deploying.md` calls page size a deployment setting and no variable exists | Add a deployment variable. Let the app developer set it per op |
| `ORDO_PG_SPACE_RESERVE` | One reserve for every tenant. The owner ruled this, and per-database reserve is 18v | No change now. Recorded so the ruling is not read as an oversight |
| `ORDO_CORS_ORIGINS` | One list, edited on the server. The owner asked for console control | Keep the deployment list. Let the app developer own it once the console exists |
| Invitation lifetime (3 days, cap 30) | A fixed default and cap on an act a tenant performs | Let the tenant owner state it per invitation, with a tenant default under the deployment's |

## Not traced

- How a person mints a delegation or capability grant through the door, and so
  whether the person layer for that lifetime is "exposable" or "sets".
- The cost and default of the per-tenant `$quotas` grace, beyond what
  `ordo quota set` prints.
- `ORDO_TENANT_ROUTING` (named in a comment at `host/runtime.rs:146`) is not
  read anywhere. `ORDO_DB_CREDENTIALS` and `ORDO_DENIAL_VOLUME_THRESHOLD_PER_SPACE`
  appear only in plans and are superseded by the names above.
