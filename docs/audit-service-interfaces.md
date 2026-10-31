# Audit: one interface per service (AGENTS.md rule 9)

Date: 2026-10-22. Tree: origin/main at 7dbf6de5, after the sign-in busy/broken fix
(87f8309e server, 51f7afb0 SDK). Read-only audit. No code changed.

The rule: each service Ordo talks to has one client and one typed error. The
error keeps its cause (`source()`). Methods on the type say what a caller can do
(busy, refused, bad input, broken). Nothing matches message text. `From`
conversions let `?` carry it. One place at the edge maps it to HTTP. Public
error enums are `#[non_exhaustive]`. In the SDK: one request helper per
service, one retry policy, typed errors.

## The short version

- 0 of 45 public error enums are `#[non_exhaustive]`.
- 0 service error types implement a real `source()`. Every service error
  stores its cause as a `String`.
- Busy is still found by text at 7 call sites (`ordo_api::carries_server_busy`,
  an `ends_with` on the pool's words).
- The executor's wire protocol is text: `"<class>: <message>"`, parsed back by
  `ordo_host::classify` (`crates/ordo-host/src/lib.rs:947`).
- Workspace counts: 256 `map_err(|_| ...)`, 247 `map_err(|e| ... to_string())`,
  559 `Result<_, String>` signatures.
- The sign-in store fix is the pattern to copy (see "The pattern").
- 6 actual bugs, listed first.

## Bugs (fix these first)

| # | Bug | Where | Effect |
|---|---|---|---|
| 1 | HTTP 429 (and 408, 425) is not retryable | `crates/ordo-egress/src/refusal.rs:124` `Rejected { status } => (500..600).contains(status)` | Mail: Resend/SendGrid rate limit becomes `MailError::Refused` (`mail.rs:261`), and `run_pass` DELETEs the outbox row (`mail.rs:1334`). A sign-in code or contact email is lost. The same rule makes a provider 429 a sign-in refusal (bug 2) and a webhook 429 permanent. |
| 2 | Provider 5xx or 429 answered as "sign-in failed" | `crates/ordo-identity/src/oauth2.rs:499`, `oidc.rs:961` (`if f.kind == "rejected"`) | Token exchange or userinfo hitting a provider outage becomes `UpstreamRefused`, then the callback word `authentication_failed` (`auth/oidc.rs:276`, `federated.rs:91`). The person is told sign-in failed, not "try again". `TransportFailure` (`federation.rs:167`) keeps only a `&'static str` kind, so the status and `Refusal::retryable()` are lost. |
| 3 | SDK sync stops for good on a busy door | `clients/ordo-sdk/src/replica/manager.ts:202` `RETRYABLE = {internal, serialization_conflict}`, `:638` | A 503 `server_busy` or `tenant_not_ready` on stream open sets `entry.stopped`. The synced op is dead until reload. Stream open (`core/client.ts:372`) skips `fetchRetrying`. ordo-react `live.ts:84` does list `server_busy`, so the two copies disagree. Neither lists `tenant_not_ready`. |
| 4 | A busy key store reads as a destroyed key | `crates/ordo-exec/src/sealed.rs:131` `Err(_) => Ok(Resolved::Destroyed(payload.epoch))` | Any `KeyError`, including `KeyError::Store` from a full pool or a dead database, is reported as crypto-erasure. A transient fault reads as permanent destruction on a security path. The comment justifies hiding *why* an envelope did not open, not folding store faults in. |
| 5 | Credential ops: busy answered as `space_unavailable` | `crates/ordo-host/src/credential_ops.rs:131,144,166,182,212` (`map_err(\|_\| "...".to_string())`), then `unavailable()` `:260` | A full pool loses its class. The caller gets 503 `space_unavailable` without Retry-After, and the SDK does not retry it. A missing table gets the same answer. About 24 such `map_err(\|_\|)` over a `PoolError` in host: `admission.rs` (6), `session_policy.rs` (4), `recovery_hold.rs` (3), `share_links.rs` (2), `contact_email.rs` (2). |
| 6 | Blob store transient failures answered as 500 | `crates/ordo-host/src/blob.rs:318-323` `store_failure`; `crates/ordo-blob/src/s3.rs:577-585` | Only a text-matched full pool becomes `server_busy`. S3 5xx, SlowDown and connect failures become `internal("blob storage")`. S3 429 and 408 become `BlobError::Refused` (permanent). Nothing in `ordo-blob` or the blob door retries. |

Smaller defects found on the way:

- `crates/ordo-exec/src/secevent.rs:1454-1467`: the sync adapters `len`,
  `base_index`, `entries`, `head` turn a store error into `0`, empty or `None`.
  A failing store reads as an empty security stream to the verifier. Confirm
  whether any verifier path can then pass on an empty stream.
- `crates/ordo-host/src/serve.rs:1234`: the usage export answers 500 with the
  database's `e.to_string()` as the message. Internal text reaches the caller.
- `crates/ordo-host/src/schema_apply.rs:2095`: the validate loop retries
  `LockTimeout` only. `Saturated` and `Serialization` fail the deploy step
  outright.
- `crates/ordo-exec/src/jobs.rs:322,357,407,417`: `e.to_string()` on a driver
  error keeps only "db error". The SQLSTATE is lost for the LISTEN connection
  and the leader lock. `described()` (`lib.rs:898`) exists for this.
- `crates/ordo-identity/src/credentials.rs:261` and `passkeys.rs:192` find a
  missing table by `text.contains("does not exist")`.
- `crates/ordo-blob/src/s3.rs:~490`: PUT 409 and 412 both read as
  `AlreadyExists`. A 409 `OperationAborted` is transient.
- `blob.rs` likely: `PipelineError::Store(BlobError::NotFound)` falls into
  `store_failure` and answers 500, not 404. Not traced to a read path; confirm.
- The callback mislabels a store failure as `identity_provider_unavailable`
  (`auth/oidc.rs:262-281`).

## The pattern (sign-in store fix, on main)

| Piece | File | What it does |
|---|---|---|
| `PoolError::is_busy()` | `crates/ordo-exec/src/lib.rs:508` | Busy decided once, by variant. |
| `StoreFault` | `crates/ordo-identity/src/store_fault.rs:31` | `StoreClass` (Busy, Unavailable) plus reason plus `source`. |
| `StoreFailure` | `crates/ordo-host/src/auth/store_failure.rs:40` | `From` for 9 store errors; one `IntoResponse` (`:151`). Busy goes to `ApiError::server_busy()`. |
| Scan test | `crates/ordo-host/tests/auth_store_failure_scan.rs` | Keeps new routes on it. |
| `authRequest` | `clients/ordo-sdk/src/auth/request.ts:88` | One helper for every auth call, busy retry, typed `ServerBusyError`. |

What it still lacks, against the rule:

- `StoreFailure::from_text` (`store_failure.rs:61`) is the remaining text path.
- `StoreFault` keeps its cause only for Busy (`scrub_fault`, `passkeys.rs:204`).
- `StoreFailure::Unavailable` builds its own 503 at `:143` with no Retry-After.
- Two classes only. Refused, bad input and broken are not separate.
- Stores still text-typed: tickets, recovery, second factors, contact address,
  settings rows, renewal, share links, consent.

## Service by service

### 1. Database

| Item | State |
|---|---|
| Client | `Pool` (`crates/ordo-exec/src/lib.rs:681`). Only ordo-exec touches `tokio_postgres` (`lib.rs`, `jobs.rs`). Good: one client. |
| Error | `PoolError` (`lib.rs:411`), 9 variants. Not `non_exhaustive`. `impl Error {}` empty (`:513`). Every payload is `String`; the driver error is dropped after `simple_rows` builds the text (`~1390-1440`). No `From<tokio_postgres::Error>`. |
| Classifiers | `is_retryable` (Serialization only), `is_busy` (Saturated only). No refused, bad input or broken. |
| `Display` is the class | `"arithmetic_error: ..."` etc. (`lib.rs:515-540`). Callers carry the text and `classify` splits on `:`. A lost prefix becomes 500. |
| Wrappers | 12 `From<PoolError>` impls into per-module errors; almost all flatten to `String`. About 120 `.to_string()`/`format!` of a `PoolError` into a String variant: `schema_apply.rs` 12, `export.rs` 11, `admission.rs` 8, `undo.rs` 7, `import.rs` 7, `reseal.rs` 7, `credential_ops.rs` 7. `schema_apply::ExecFailure` (`:1883`) and `ApplyRefusal` (`:2366`) are String-typed. `CredentialStoreError(pub String)` (`identity/credentials.rs:44`). |
| Text busy | `carries_server_busy` at `serve.rs:1229`, `spaces.rs:568,774`, `lib.rs:178`, `blob.rs:319,451`, `store_failure.rs:63`. Also `from_exec` (`ordo-api/src/error.rs:1209`). |
| Retry copies | 5. `Pool::retrying` (`lib.rs:~1305`) and two write loops (`~2014`, `~2068`) share a shape. `release_ops.rs:656` and `profile_ops.rs:462` retry `Serialization` 3 times with no backoff. `schema_apply.rs:2059,2090` retry `LockTimeout` with their own pause. |

Fix: make `PoolError` the one typed error. `#[non_exhaustive]`, keep the
driver error as `source`, add `class()` returning busy, retryable, refused (check,
arithmetic, timeout), broken. Replace `Display`-as-protocol with
`From<PoolError> for ExecError` that reads `class()`. Retire
`carries_server_busy` and `classify(String)`. Move every store onto
`StoreFault` (rename to a shared `StoreError` in ordo-exec so non-identity
stores use it too). One `Pool::retrying` for all callers.
Size: L (3 sessions: type and edge; stores batch 1; stores batch 2 and the
retry merge). About 3 CI rounds.

### 2. Mail

| Item | State |
|---|---|
| Client | `Mailer` trait (`crates/ordo-host/src/mail.rs:~290`): `SmtpMailer`, `ApiMailer` (Resend, SendGrid). Outbox `MailQueue` (`:1076`), `PgMailQueue`, drained by `run_pass` (`:~1245`). Good: one client, one outbox. |
| Error | `MailError` (`:230`): `NotRepresentable`, `Unavailable`, `Busy`, `Refused`, all String but Busy. No `Error` impl, no `source()`, no `From` in. One classifier `retryable()` (`:247`). |
| Cause stripped | `from_refusal_with` (`:254-265`) turns typed `ordo_egress::Refusal` into `r.operational()` text. `ApiMailer::submit` (`:686-693`) formats the status into `Refused`. `:1103-1105` drops the `PoolError` after `is_busy()`. |
| Text | About 12 `map_err`/`to_string` sites in `mail.rs`; `email_code.rs:990,1167,1209,1221`. |
| Edge | `email_code.rs:944` maps Busy to `UpstreamUnavailable(MAIL_OUTBOX_BUSY)` and every other enqueue failure to `"mail-outbox-unavailable"`. The class rides in a `&'static str`. |
| Retry | One site (`run_pass`, `:1324`). SMTP 4xx correctly retryable. HTTP 429 not (bug 1). |

Fix: `MailError` keeps `Refusal` as `source`, gains `#[non_exhaustive]` and
`class()`. `Refusal::retryable` treats 408, 425, 429 as retryable and honours
Retry-After. Size: S for bug 1 alone; M for the full type.

### 3. File storage

| Item | State |
|---|---|
| Client | `BlobStore` trait (`crates/ordo-blob/src/lib.rs:184`): `FsStore`, `S3Store`. Pipeline on top (`pipeline.rs`). `StoreResolver` (`lib.rs:307`). Good: one client. |
| Errors | `BlobError` (`lib.rs:134`, 6 variants, 3 String) and `PipelineError` (`pipeline.rs:74`, 7 variants). Both empty `impl Error`. 0 classifier methods. `From<BlobError>` and `From<KeyError>` into `PipelineError` exist. |
| Text | About 30 `Transport(format!(...))`/`Refused(e.to_string())` in ordo-blob (`http.rs` 14). `ordo-exec/src/bucket.rs:862` wraps a pool error into `Transport` text; the door finds busy by suffix. `host/blob.rs:371` calls `classify(e.to_string())`. |
| Edge | One mapper, `host/blob.rs:333-383`. Good shape, but `Custody`, `Store` and `Undeclared` all go to `store_failure` (bug 6). `:575,721,841,853` `map_err(\|_\| internal("blob storage"))`. |
| Retry | None. |
| HTTP client | `ordo-blob/src/http.rs` is its own hand-rolled HTTP stack, separate from `ordo-egress`. |

Fix: `BlobError` gains `#[non_exhaustive]`, `source`, and `class()` (busy for
5xx, 408, 429, SlowDown; refused for 4xx; not found). The blob door maps by
class. One bounded retry in `S3Store` for busy. Consider moving S3 onto
`ordo-egress`. Size: M.

### 4. Outside sign-in providers

| Item | State |
|---|---|
| Client | No client type. Trait `FederationTransport::fetch` (`crates/ordo-identity/src/federation.rs:174`), host impl `EgressTransport` (`crates/ordo-host/src/federation.rs:109-118`). |
| Errors | `TransportFailure { kind: &'static str }` (`federation.rs:167`): no source, not an `Error`. `AuthError` (`provider.rs:350`): provider failures are `UpstreamUnavailable(&'static str)`/`UpstreamRefused(&'static str)`; empty `impl Error`; one `From` (WebAuthnError). `ConnectionError` (`oauth2.rs:110`), `OidcConnectionError` (`oidc.rs:151`): empty `Error` impls. |
| Cause stripped | `EgressTransport` does `.map_err(\|r\| TransportFailure { kind: r.kind() })`. The typed `Refusal`, its status and `retryable()` are gone. |
| Text | `f.kind == "rejected"` (2 sites). `oidc.rs:708` keeps `discovery_failed = Some(why)` as control flow. `tenant_providers.rs`: 8 `to_string`, 6 `map_err(\|_\|)` (`:414,424,437,441` on seal/open of client secrets fold busy into broken config). `jose.rs` 11 `map_err(\|_\|)` (deliberate, token verification). |
| Edge | Split across 8+ files: `auth.rs:1241` `refused()` is 401; 503 at `auth/oidc.rs:61`, `auth/tenant.rs:160`, `auth.rs:1404`; callback 303 words at `auth/oidc.rs:262-281`, `federated.rs:88-93`. |
| Retry | None for token, userinfo or discovery. JWKS has a refetch floor only. |

Fix: `TransportFailure` becomes the `Refusal` itself (or keeps it as `source`
with `retryable()` and status). `AuthError` gets an `UpstreamBusy` class. One
mapper for `AuthError` beside `StoreFailure`. Size: M.

### 5. Key custody

| Item | State |
|---|---|
| Client | `KeyProvider` trait (`crates/ordo-crypto/src/custody.rs:304`), `LocalKeyProvider` over `PgKeyStore` (`crates/ordo-exec/src/keystore.rs`). No remote KMS today. |
| Error | `KeyError` (`custody.rs:270`): `Store(String)` (`:282`), no source, one discarding `From<SealError>` (`:297`), no classifiers. |
| Text | `keystore.rs:325` `store_error` does `e.to_string()` then `contains("$keycustody") && contains("does not exist")`. About 12 `KeyError::Store(String)` builds in `keystore.rs`, 7 in `master.rs`. 6 `map_err(\|_\|)` in ordo-crypto, 2 in `pipeline.rs:713,884`. |
| Busy | `PoolError::Saturated` flattened to `KeyError::Store(text)`; `host/blob.rs:319` re-finds it by text. |

Fix: `KeyError::Store` holds the shared store error (busy or broken, with
source). `sealed.rs:131` matches `Store` separately (bug 4). Keep the
opaque "did not open" for real envelope failures. Size: S to M.

### 6. Security-event and attestation drains

| Item | State |
|---|---|
| Attestation | `crates/ordo-host/src/attestdrain.rs`. Good shape: `fault_of` (`:458-463`) uses typed `Refusal::retryable()`, then retry or dead-letter with `triggers::backoff_secs`. `Fault.reason` is text; DB faults hard-coded `retryable: true` (`:399,480`). |
| Security events | `crates/ordo-host/src/secdrain.rs`, `crates/ordo-exec/src/secevent.rs`. `StreamError` (`ordo-secevent/src/store.rs:154`): `Store(String)`, no source. `store_err` (`exec/secevent.rs:166`) stringifies. `append_batched` keeps `first_error: Option<String>` (`secdrain.rs:1153`), requeues held-back events (no loss). Health is text only; busy and dead look the same. `let _ =` on drain tasks at `secdrain.rs:542,624,640,665`. |
| Retry | 1 policy in attestdrain plus `triggers::backoff_secs`; secdrain requeues. |

Fix: `StreamError::Store` holds the shared store error. Health reports the
class. The sync adapters return `Result` (smaller defect above). Size: S.

### 7. The door seen from the SDK

| Item | State |
|---|---|
| Helpers | 3 in ordo-sdk: `authRequest` (auth only, 7 callers), `OrdoClient#send` plus `fetchRetrying` (ops), `files.ts` `send` (blobs). 9 live fetch sites. |
| ordo-react | Does not import ordo-sdk. Carries copies: `busy.ts`, `refusal.ts`, `client.ts`, `files.ts`, `refine.ts`, `json.ts`. Own raw fetch in `email.ts:85`, `tenant.ts:90`, `client.ts:267,289,338,370,383`. |
| ordo-cdk | No network code. Nothing to do. |
| Errors | About 10 shapes, no shared base, 4 status/code layouts, none with `retryable`. Only `OrdoNetworkError` keeps `cause`. `OrdoRefusal` (`core/refusal.ts:112`) has no `cause`. `tenant.ts:~96` (react) drops the fetch error. No control flow on `err.message`: good. |
| Retry | 4 policies: `fetchRetrying` (`core/busy.ts:276`, react copy), `authRequest` (busy only), `waitForTenant` `PAUSES_MS` (`provisioning.ts:~190`, ignores Retry-After), sync backoff (`replica/manager.ts:221`, react `live.ts:92`, ignore Retry-After). Blobs and downloads: none. |
| Busy gaps | Sync stream open (bug 3). Blob calls. react `email.ts:110`, `tenant.ts:107` map every 5xx to `unavailable`; 429 to `limit_reached` with no wait. |

Fix: one `doorRequest` in ordo-sdk (deadline, named network error, one busy and
conflict policy, Retry-After) used by ops, auth, blobs, downloads and stream
open. One `OrdoError` base with `status`, `code`, `retryable`, `cause`. The
sync `RETRYABLE` set derives from it. ordo-react depends on ordo-sdk instead of
copying. Size: L (2 sessions plus a review), 2 CI rounds.

### The server edge

| Item | State |
|---|---|
| Mapper | `ApiError` plus `ErrorCode::status()` (`crates/ordo-api/src/error.rs:957`, `IntoResponse` `:1282`). Emits Retry-After for `server_busy` (`:1292`) and `tenant_not_ready`. Good. |
| Type | `ApiError` (`:1115`) has pub fields, derives `Clone, PartialEq, Eq`, no `source`, no `Error` impl. `ErrorCode` is exhaustive. |
| Bypasses | 61 non-2xx `StatusCode::` picks outside `error.rs` in 20 files: `serve.rs` 9, `auth.rs` 9, `auth/delegate.rs` 7, `auth/account.rs` 7, `tenant.rs`, `store_failure.rs`, `recover.rs`, `link.rs` 3 each, others 1 to 2. About 36 hand-built `{"error":...}` bodies. About 12 hand-built 503s, none with Retry-After. A second `IntoResponse` in `store_failure.rs:151`. |

Fix: every route returns `Result<_, ApiError>` (or a type with
`Into<ApiError>`). The hand-built envelopes go. A scan test, like
`auth_store_failure_scan.rs`, refuses `StatusCode::` outside the edge.
Size: M.

Built (step 6, 2026-10-23): `error.rs` holds `ErrorCode` (ops) and a second
closed inventory, `EdgeCode` (sign-in, accounts, delegation, links; 37
words), each with its status. Handlers return `ApiError` or `EdgeError`;
`status_report` and `no_document` cover the two non-envelope answers.
`from_exec` no longer reads text: a busy fault arrives typed
(`ExecError::of_fault`, `ordo_host::Faulty`), and `carries_server_busy` is
gone. `ApiError::by_fault` is the one class-to-answer mapper (blob door).
`tests/edge_scan.rs` allows 3 non-success statuses outside the edge (CORS
preflight 403, replayed stored status, the proxy reading a 403). The 48
public error enums left are `#[non_exhaustive]`. One wire change: the
`/health/deploy` node-read failure was a flat `{"error": "<text>"}` that
quoted the pool error; it is now `space_unavailable` (or `server_busy`).

## Counts at a glance

| Pattern | Count | Heaviest files |
|---|---|---|
| `#[non_exhaustive]` on public error enums | 0 of 45 | all |
| Service errors with a real `source()` | 0 (only `StoreFault` keeps one) | all |
| `carries_server_busy` call sites (text busy) | 7 outside `error.rs`, plus `from_exec` | `spaces.rs`, `blob.rs` |
| `map_err(\|_\| ...)` | 256 | `webauthn.rs` 24, `recovery.rs` 17, `identity_ops.rs` 15, `jose.rs` 11, `second_factors.rs` 10 |
| `map_err(\|e\| ... to_string())` | 247 | `runtime.rs` 55, `registrations.rs` 12, `member_ops.rs` 11, `attest_ops.rs` 11, `bucket_ops.rs` 9 |
| `Result<_, String>` | 559 | `second_factor_store.rs` 30, `second_factors.rs` 21, `schema_apply.rs` 20 |
| Text `.contains` on DB errors | 3 | `credentials.rs:261`, `passkeys.rs:192`, `keystore.rs:325` |
| Non-2xx status picked outside the edge | 61 in 20 files | `serve.rs`, `auth.rs` |
| Hand-rolled HTTP stacks | 3 | `ordo-egress`, `ordo-blob/src/http.rs`, `ordo-cli/src/wire.rs` |
| Retry policies (server) | 8 | `ordo-exec` 5, mail 1, attestdrain 1, egress `Refusal` 1 |
| Retry policies (SDK) | 4, plus react copies | `busy.ts`, `manager.ts`, `provisioning.ts`, `live.ts` |

Some of these counts are grep counts. Not every `map_err(|_|)` is wrong: parse
errors, closed channels and the deliberate opaque paths in `jose.rs` and
`webauthn.rs` are fine.

## Suggested order

| Step | Work | Size | Why now |
|---|---|---|---|
| 1 | Bug 1: 408, 425, 429 retryable in `Refusal::retryable`, with Retry-After. Fixes mail drops and part of bug 2. | S | Mail is lost today. One function. |
| 2 | Bug 3: sync `RETRYABLE` adds `server_busy` and `tenant_not_ready`; stream open goes through `fetchRetrying`. | S | Live clients die on a busy pool. |
| 3 | Bug 4: `sealed.rs` tells a store fault from a closed envelope. | S | Security path reads a transient as erasure. |
| 4 | Database: shared typed store error (`StoreFault` moved to ordo-exec, `#[non_exhaustive]`, `class()`, `source`), `From<PoolError> for ExecError`. Fixes bug 5. | L | Every other service's store side rests on it. |
| 5 | Remaining text-typed stores onto it: tickets, recovery, second factors, contact, settings, renewal, share links, consent; credential ops; key store; secevent `StreamError`. Retire `carries_server_busy`. | L | Finishes the sign-in pattern. |
| 6 | Sign-in providers: `TransportFailure` keeps the `Refusal`; `AuthError::UpstreamBusy`; one `AuthError` mapper. Fixes bug 2. | M | Needs step 1. |
| 7 | File storage: `BlobError::class()`, S3 retry, door maps by class. Fixes bug 6. | M | Needs step 4 for custody faults. |
| 8 | Mail type: `MailError` keeps its `Refusal`, gains `class()`. | M | Step 1 already stops the loss. |
| 9 | Server edge: routes return `ApiError`; scan test for `StatusCode::` outside the edge. | M | Easier once steps 4 to 8 give typed errors. |
| 10 | SDK: one `doorRequest`, one `OrdoError` base; ordo-react depends on ordo-sdk. | L | Server classes are stable by then. |
| 11 | `#[non_exhaustive]` on every public error enum, in the same commit as each step above. | S each | Cheap when the type is open anyway. |

What sets the pace: each step is 1 to 2 CI rounds (about 12 minutes each).
Steps 4 and 5 touch many host files; run them one after the other, not in
parallel, to avoid merge conflicts.

## The fix order (owner, 2026-10-23)

Ruled on `audit-one-interface-and-one-typed`: the 6 bugs first, now; then the
structural fixes in this order. Each step lands with its own tests and keeps
CI green before the next starts.

| Step | What | Size |
|---|---|---|
| 0 | The 6 bugs above | S each, about 1 session |
| 1 | The database's typed error, moved down from sign-in (`StoreFault`, `PoolError::is_busy`) | L |
| 2 | The stores still typed as text: tickets, recovery, second factors, contact address, settings rows, renewal, share links, consent | L |
| 3 | Sign-in providers: one client, the full refusal kept, a busy class | M |
| 4 | File storage: classifiers and retry | M |
| 5 | Mail: keep the cause, add a class | M |
| 5b | Key custody and the security-event drain: typed errors, no text (left out of the first table by mistake; added 2026-10-23) | S to M |
| 6 | The server edge: every status chosen by the one mapper | M |
| 7 | The SDK: one request helper, one retry policy, one error shape; ordo-react imports ordo-sdk | L |

Alongside, ruled the same day: every public error enum is `#[non_exhaustive]`,
added in each step for the enums it touches; ordo-react imports ordo-sdk's
code instead of carrying copies.

Mail is already queued: the outbox holds every message and a worker sends it.
The bug was that a rate limit (429) from the relay counted as permanent and
deleted the message; it must stay queued and be retried later.
