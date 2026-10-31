# Passkey sign-in, served: the two ceremonies at the door

Build item 1 of the owner's order of 2026-09-29: *"passkey sign-in in parallel
with enrolment and the membership commands"*. Checklist 20b. Board task
`auth-bootstrap`.

Companion to `plan-enrolment.md` (E1 to E7, built in parallel with this),
`plan-passkey-recovery.md` (R1 onward, built after), `plan-auth-federated.md`
(F0 onward), `plan-second-factor.md`, and `security-logging-design.md` §2,
§2.1, §2.2 and §3.

Markers: **[settled]** ruled by the owner or restated from a settled document
· **[plan]** proposed here · **[seam]** work another plan owns.

Status: **planned, not built.** Every claim about the code was read against
the tree at `2082f74` on 2026-09-29. This plan reflects the owner's answers
of 2026-09-29.

**No buildable plan for serving passkey sign-in existed before this one.**
`plan-enrolment.md` §7, `plan-passkey-recovery.md` §12 and
`plan-auth-federated.md` §12 each say they do not wire it, and checklist 20b
names it *"first in the queue"* with no plan behind it. This is that plan.

---

## 0 · The starting state, verified

### 0.1 What is built

| Claim | Where |
|---|---|
| WebAuthn verification is complete: challenge, RP id, origin, presence flag, user verification, signature, sign counter | `crates/ordo-identity/src/passkey.rs:513-613`, `webauthn.rs:486-691` |
| Only `fmt: "none"` attestation is accepted, by a settled rule | `webauthn.rs:583-586`; `security-logging-design.md` §2.1 |
| ES256 and EdDSA only | `webauthn.rs:315-316`, `passkey.rs:401-403` |
| `UserPresence` has private fields and one producer | `passkey.rs:68-72`, minted at `passkey.rs:549-552` and `608-611` |
| `RelyingParty::new` validates the RP id and every origin, and refuses an empty origin list | `passkey.rs:193-245` |
| **The door already parses both passkey payloads.** `parse_complete` reads `passkey_registration` and `passkey_assertion` | `crates/ordo-host/src/auth.rs:482-493` |
| A completed ceremony reaches a token with presence: `complete` → `sign_in` → `token_spec` → `user_presence_at` | `auth.rs:284-299`, `session.rs:436-450`, `session.rs:407-428` |
| Step-up reads only `user_presence_at`, within 300 s | `access.rs:1004-1037`, `access.rs:32` |
| A presence refresh inside a session exists and checks that the asserting passkey belongs to the session's principal | `session.rs:538-579`, the binding check at `:560` |
| A software authenticator that signs real assertions exists for tests | `crates/ordo-identity/src/passkey_tests.rs:102-280` (`TestAuthenticator`, `TestKey`) |

### 0.2 What stops it being served

| Gap | Where |
|---|---|
| `auth_from_env` refuses every method but `password` | `runtime.rs:367-385` |
| The registry loop has one arm and asserts it | `runtime.rs:1999-2003` |
| `ORDO_APEX_DOMAIN` is read and recorded, and nothing acts on it. The census says so | `runtime.rs:704-710`, `runtime.rs:2863` |
| Enrolled public keys are process memory: `credentials: Mutex<BTreeMap<Vec<u8>, PasskeyCredential>>` | `passkey.rs:370-375` |
| The sign counter is written to that map only, so the clone check does not survive a restart | `passkey.rs:595-604` |
| Challenges are process memory too, so a challenge minted on one replica cannot be completed on another | `passkey.rs:373`, `461-471`, `483-487` |
| `/auth/begin` answers `{challenge, method}` and nothing else. A browser needs the RP id, the user-verification policy and, for registration, the user handle, the algorithms and the RP name | `auth.rs:216-225`; `CreationOptions` at `passkey.rs:290-301` has no `user` field |
| Registration cannot be started through the registry: `AuthProvider::begin` mints an assertion challenge only, and `begin_registration` is a method of the concrete type | `passkey.rs:624-630`, `passkey.rs:423-432`, `registry.rs:59-64` |
| The directory cannot list a principal's authenticators, which registration needs for `exclude_credentials` and step-up needs for `allow_credentials` | `DirectoryStore` at `directory.rs:273-319` has `link_count` and no listing |
| No route steps up, links or unlinks | `auth.rs:533-539`; `plan-passkey-recovery.md` §0.4 |
| The audit events a sign-in returns are dropped | `auth.rs:282-292`; found as F0 and R2 |
| No browser code. Nothing under `clients/` calls `navigator.credentials` | `grep -rn "credentials.create\|credentials.get\|webauthn" clients --include='*.ts'` finds two comments about PRF wrapping and nothing else |
| The software authenticator is `#[cfg(test)]` inside `ordo-identity`, so a host test cannot use it | `crates/ordo-identity/src/lib.rs:40-41` |

### 0.3 One fact that shapes the design

**A registration through `/auth/complete` is already a sign-in.**
`complete_registration` returns a subject with presence (`passkey.rs:546-553`);
`complete` hands it to `sign_in`; `resolve_or_onboard` mints a principal and
links the credential under `AutoOnboard`, or refuses `NotInvited` under
`InviteOnly` (`directory.rs:682-710`). So at the door, *registering a
passkey* and *enrolling an identity* are one act. That is P3, *"each new
sign in is an enrollment"*, already in the code's shape. This plan keeps it.

---

## 1 · What binds this plan [settled]

From `corpus-rulings.md`, 2026-09-29:

| | Ruling | What it fixes here |
|---|---|---|
| P1 | *"Service owns the credentialing."* | Public keys, challenges and links live in the control database. No tenant table holds one |
| P3 | *"Each new sign in is an enrollment (auto enrollment) unless they want to manually provision and invite."* | A first registration mints a principal where the policy allows it. No prior step is required |
| P4 | *"It does not make sense to enforce a particular flow."* Overrules the one-time-code proposal on `auth-bootstrap` | Registration is reachable with a ticket, without one under an open policy, and as first-comer. None is compulsory |
| P6 | One person, one identity, many tenants | The token names a principal and no space, as today (`token.rs:141-172`) |
| Federated answer 3 | A registration *"stay[s] there for some x amount of time and then get[s] nuked"* if no tenant admits the person | A registered principal with no membership is kept and swept by enrolment E4. This plan mints it |
| Build order | Passkey sign-in in parallel with enrolment and the membership commands | §7 names every file both touch |

The settled rules of `security-logging-design.md` stand: RP id is the apex
domain (§2), presence comes only from here (§2.2), no attestation (§2.1),
`linkFirst` only for an account with zero factors (§ *Linking an
authenticator*).

---

## 2 · The shape [plan]

### 2.1 Two ceremonies, three routes

| Ceremony | Begin | Complete | Yields |
|---|---|---|---|
| **Sign-in** (assertion) | `POST /auth/begin {"method":"passkey"}` | `POST /auth/complete {"method":"passkey","kind":"passkey_assertion",…}` | a session token with `user_presence_at` |
| **Registration** (attestation, `none`) | `POST /auth/register {"method":"passkey","ticket"?:…,"display"?:…}` | `POST /auth/complete {"method":"passkey","kind":"passkey_registration",…}` | the credential stored, the principal resolved or minted, a session token with presence |
| **Step-up** inside a session | `POST /auth/step-up/begin` with the bearer token | `POST /auth/step-up` with the bearer token and an assertion | the same session re-minted with fresh presence (`refresh_presence`) |

`/auth/complete` stays the one completion route. It already parses both
kinds, and one route means one refusal (`auth.rs:37-52`). `/auth/register`
is a separate begin because registration options are a different shape and
the registry trait cannot express them (§0.2).

### 2.2 What `begin` answers, for a browser

Sign-in, `/auth/begin`:

```
{ "challenge": "<base64url>", "method": "passkey",
  "rp_id": "example.com", "user_verification": "required",
  "allow_credentials": [], "timeout_ms": 300000 }
```

`allow_credentials` is empty: the credential is discoverable and the
browser finds it (`passkey.rs:309-314`). No identifier is asked for, so the
route is not an existence oracle.

Registration, `/auth/register`:

```
{ "challenge": "<base64url>", "method": "passkey",
  "rp_id": "example.com", "rp_name": "example.com",
  "user": { "id": "<base64url 32 bytes>", "name": "<display>", "display_name": "<display>" },
  "algorithms": [-7, -8], "user_verification": "required",
  "resident_key": "required", "attestation": "none",
  "exclude_credentials": ["<base64url>", …], "timeout_ms": 300000 }
```

- `user.id` is the **user handle**: 32 random bytes minted at begin, stored
  in the ceremony row, written to the credential row at complete. One
  handle per principal: a later registration for a principal that already
  has one (recovery R2, a second passkey) reuses it. The handle is never a
  principal id, because the principal may not exist until complete (§0.3).
- `user.name` and `user.display_name` are the display the caller posted, or
  the RP name when none was. They are shown by the authenticator and by
  nothing else.
- `exclude_credentials` lists the existing credential ids of the principal
  a ticket names, and is empty for a fresh registration.
- `resident_key: "required"` so that sign-in needs no identifier.
  `attestation: "none"` matches the verifier's only accepted format.

The JSON shape is the route's, as `passkey.rs:286-288` intends.

### 2.3 What complete does, by case

| Case | Who may | What happens |
|---|---|---|
| Assertion, credential known | anybody holding the passkey | `complete_assertion` verifies, the sign counter is written through to the store, `sign_in` resolves the principal, a token is minted |
| Assertion, credential unknown or signature bad | | the one answer (`auth.rs:164-171`) |
| Registration with a **ticket** (enrolment E2) | whoever holds the ticket | the ticket is redeemed once; the credential is stored; `Directory::link_first` links it to the principal the ticket names, or a principal is minted for a ticket that names none; a token is minted |
| Registration, no ticket, policy admits it | anybody | as today: `resolve_or_onboard` mints a principal and links (`directory.rs:695-707`); the credential is stored; a token is minted. Enrolment E4 sweeps the principal if no tenant admits it in time |
| Registration, no ticket, policy refuses | | the one answer, and the credential is **not** stored: the store insert runs after `sign_in` succeeds, inside the same blocking task |
| Registration of a credential id already stored | | `CredentialAlreadyEnrolled` (`passkey.rs:541-544`), the one answer to the caller, the specific reason to the stream |

The policy is `Provisioning` as read today (`runtime.rs:410-419`,
`2020-2027`). Enrolment E4 turns it into the deployment's ceiling and adds
the tenant's admission. This plan reads whatever `Directory` holds and
does not know which.

**A registration never reaches `Directory::link`.** A second passkey for a
principal that already holds one is the dual-proof ceremony of recovery R2
and needs a `LinkCeremony`. `/auth/register` without a ticket on a
principal that exists cannot happen, because without a ticket the caller
is a stranger, and `link_first` refuses a principal with any link
(`directory.rs:606-610`).

### 2.4 Where things live so that a restart keeps them

Two new control-database tables, two new traits in `ordo-identity`, two
Postgres stores in `ordo-host` on the `PgCredentialStore` pattern
(`crates/ordo-host/src/credentials.rs:114-243`), opened on the shared
single-connection pool (`open_on`, `credentials.rs:147-149`).

**`$passkey`, the public keys.**

```
"$passkey" (
  "authenticator"  text   NOT NULL,   -- passkey:<base64url credential id>, the same string as $authenticator.authenticator
  "algorithm"      bigint NOT NULL,   -- -7 | -8
  "public_key"     text   NOT NULL,   -- base64url: 65-byte SEC1 point, or 32-byte Ed25519 key
  "sign_count"     bigint NOT NULL,
  "user_handle"    text   NOT NULL,   -- base64url, 32 bytes
  "backup_eligible" boolean NOT NULL, -- BE flag at registration
  "backed_up"      boolean NOT NULL,  -- BS flag at registration
  "registered_at"  bigint NOT NULL,
  "last_used_at"   bigint NOT NULL DEFAULT 0,
  PRIMARY KEY ("authenticator")
)
```

No principal column. The link to a principal is `$authenticator`
(`principal_directory.rs:81-86`) and stays in one place. A row here with
no link row is a credential nobody can sign in with, which is what
revocation leaves behind until R2 deletes it.

`BE` and `BS` are bits 3 and 4 of the flags byte, which `webauthn.rs:465-468`
does not name yet. Two constants and two accessors beside `user_present`.
Recovery §3.1 wanted them recorded; they are recorded here because the row
is created here.

Trait, in `passkey.rs`:

```
pub trait PasskeyStore: Send + Sync {
    fn get(&self, id: &[u8]) -> Result<Option<StoredPasskey>, PasskeyStoreError>;
    fn insert(&self, cred: &StoredPasskey) -> Result<(), PasskeyStoreError>;   // refuses an existing id
    fn set_sign_count(&self, id: &[u8], count: u32, at: Timestamp) -> Result<(), PasskeyStoreError>;
    fn remove(&self, id: &[u8]) -> Result<(), PasskeyStoreError>;
}
```

`StoredPasskey` is `PasskeyCredential` plus `user_handle`, the two flags
and the timestamps. `CoseKey` gains `to_bytes` and `from_bytes(alg,
bytes)`, the latter re-running the on-curve check of `webauthn.rs:445-446`
so a hand-edited row cannot load a bad point. `PasskeyStoreError` is the
store-unavailable answer, distinct from a refusal for the reason
`CredentialStoreError` is (`provider.rs:186-190`), and `complete` answers
it with `store_unreadable()` (`auth.rs:176-183`).

**The sign counter is written before the subject is returned.** A
counter that advanced only in memory is not a clone detector after a
restart (`passkey.rs:588-590` states the intent). If the write fails the
assertion is refused, so a store outage cannot make a replayed assertion
succeed.

**`$authceremony`, the challenges.** The table `plan-auth-federated.md`
§3.1 defines, created here first, in that exact shape, so F1 extends a
table rather than creating one:

```
"$authceremony" (
  "handle"      text   NOT NULL,   -- SHA-256 of the challenge, base64url; the challenge itself is never stored
  "connection"  text   NOT NULL,   -- 'passkey' here; a connection name for F1
  "phase"       text   NOT NULL,   -- 'register' | 'assert' | 'step-up' here; 'begun' | 'handoff' for F1
  "binder"      text   NOT NULL,   -- '' here: a WebAuthn challenge is bound by the signature over clientDataJSON
  "payload"     text   NOT NULL,   -- JSON: user handle, ticket reference, principal (step-up)
  "minted_at"   bigint NOT NULL,
  "expires_at"  bigint NOT NULL,
  PRIMARY KEY ("handle")
)
```

Trait, in a new `crates/ordo-identity/src/ceremony.rs`:

```
pub trait CeremonyStore: Send + Sync {
    fn put(&self, row: &CeremonyRow) -> Result<(), CeremonyStoreError>;
    /// DELETE … WHERE handle AND connection AND phase AND expires_at > now RETURNING payload.
    /// Absent and expired are the same None. The database decides which of two racing callers wins.
    fn take(&self, handle: &str, connection: &str, phase: &str, now: Timestamp)
        -> Result<Option<String>, CeremonyStoreError>;
    fn sweep(&self, now: Timestamp) -> Result<u64, CeremonyStoreError>;
}
```

`PasskeyProvider` drops its `pending` map for an `Arc<dyn CeremonyStore>`.
`consume` (`passkey.rs:477-498`) becomes one `take`, keeping its property
that a failed attempt burns the challenge. `MemoryCeremonyStore` is the
test double. The sweep runs on the supervisor as a worker on the
`access.refresh` pattern (`workers.rs:1101-1134`), every 60 s.

`LocalPasswordProvider` keeps its in-memory map (`provider.rs:453`). Moving
it is a small separate change, as `plan-auth-federated.md` §3.1 says.

### 2.5 Configuration, and the refusal that ships with its producer

`auth_from_env` (`runtime.rs:367-426`) accepts `passkey` in
`ORDO_AUTH_METHODS` and, when it is present, reads:

| Variable | Required | Meaning | Refusal |
|---|---|---|---|
| `ORDO_APEX_DOMAIN` | yes, with `passkey` | the RP id | `BootRefusal::Missing`, *"passkey needs a relying-party id; changing it later strands every enrolled passkey"*. Without `passkey` in the list it stays as today: read, recorded, not acted on |
| `ORDO_AUTH_ORIGINS` | yes, with `passkey` | comma-separated origins that may run a ceremony | `Missing`; each value goes through `RelyingParty::new`, whose five refusals (`passkey.rs:137-157`) become `BootRefusal::Invalid` with their own text |
| `ORDO_AUTH_RP_NAME` | no | `rp.name` shown by the authenticator | default: the apex domain |
| `ORDO_AUTH_USER_VERIFICATION` | no | `required` (default) or `preferred` | any other word refuses |

This is the refusal `plan-runtime.md` §478-495 deferred to *"the increment
that mounts the passkey endpoints"*. The census line at `runtime.rs:2863`
changes from *"recorded; no passkey provider is mounted"* to naming the RP
id and the origin count.

`ORDO_AUTH_ORIGINS` and the CORS allowlist (14g) will usually hold the same
values and are two settings, because they answer two questions: which page
may run a WebAuthn ceremony, and which page may call the door at all. The
guide says to set both. They are not derived from each other, so that a
deployment whose app and sign-in page differ can say so.

### 2.6 Presence, and step-up inside a session

Nothing new is needed for a fresh passkey sign-in to pass step-up: the
path in §0.1 exists and is typed end to end (`security-logging-design.md`
§2.2). A token from S3 satisfies `std.deployment.apply` for 300 s.

After that the person needs `refresh_presence`, which has no route. S5
adds one pair:

- `POST /auth/step-up/begin`, bearer token required and verified. Mints an
  assertion challenge with phase `step-up` and the session's principal in
  the payload, and answers the request options with `allow_credentials`
  narrowed to that principal's passkeys (`passkey.rs:309-314` says the
  caller must narrow; the server re-checks anyway).
- `POST /auth/step-up`, bearer token plus a `passkey_assertion` body. Runs
  `complete_assertion`, then `refresh_presence` (`session.rs:538-579`),
  which refuses a capability or delegated token, a presence-less method,
  and a passkey of another principal. The revocation list is consulted
  first, as `refresh` does (`session.rs:357-361`), so a logged-out session
  cannot step up. Answers a re-minted token for the same session.

Every refusal is the one answer.

### 2.7 The browser half

A new module `clients/ordo-sdk/src/auth/passkey.ts`, exported from the
SDK:

| Function | Does |
|---|---|
| `registerPasskey(client, {ticket?, display?})` | `POST /auth/register`, builds `PublicKeyCredentialCreationOptions` from the answer, calls `navigator.credentials.create`, posts `client_data_json` and `attestation_object` as base64url without padding, returns the token |
| `signInWithPasskey(client)` | the same over `/auth/begin` and `navigator.credentials.get` with an empty allow list |
| `stepUp(client, token)` | the same over the two step-up routes |

The rule `webauthn.rs:607-610` states is the client's too: `clientDataJSON`
is posted as the bytes the browser produced, never re-serialised. No React
hook in this plan; `ordo-react` wraps these later.

No test fakes a user gesture (checklist 5h, *"no test fakes a user
gesture"*). The SDK's tests stub `navigator.credentials` and prove the
encoding; the host's tests use the software authenticator; a person clicks
through the first real one.

### 2.8 What is recorded

The onboarding and the link a registration produces are `AuditEvent`s
that `sign_in` returns and `complete` drops (§0.2). A first registration
with no record breaks `security-logging-design.md` §3 (*"the onboarding is
itself an audited event"*), so this plan carries F0 as its first increment:
`AuthState` takes the recorder; every success records `Authenticated`;
every refusal records `AuthenticationFailed` with the reason
(`EventKind` at `crates/ordo-secevent/src/event.rs:401-404`); the audit
events are recorded through the existing conversion (`event.rs:523-545`).
`plan-auth-federated.md` F0 and the recording half of
`plan-passkey-recovery.md` R2 are then done.

---

## 3 · What this needs from enrolment, and gives it

| Needs | From | Until it lands |
|---|---|---|
| The door resolves `(principal, tenant)` to a member | **E1** | a passkey token is refused at every op with `NoMembership`, because `MEMBERS_SQL` keys by member id (`lib.rs:1600-1601`). S3's test sets the membership on the engine directly, as `deploy_apply.rs:239` does today |
| A ticket to redeem | **E2** | `/auth/register` refuses a body with `ticket` (the one answer). S7 wires it |
| A members row for the first owner | **E5** (first-comer) or **E3** (admit) | a fresh public deployment has passkey sign-in and nobody who is a member. **The first owner's path is S4 + E1 + E5** |
| The sweep of registrations no tenant admitted | **E4** | principals minted by S4 accumulate; harmless before the first deployment (no deployment exists) |

| Gives | To |
|---|---|
| `DirectoryStore::authenticators_of(principal)` | E3 (listing members' credentials), R2 (listing one's authenticators) |
| `$authceremony` and `CeremonyStore` | F1, which adds the `handoff` phase and the binder; `plan-second-factor.md` T1, which adds `second-step` |
| `$passkey` and `PasskeyStore` | R2 (delete on unlink), R3 (`presence_not_before` is added to this row, not to `$authenticator`; recovery §7 is amended to say so), `plan-second-factor.md` T3 (a `role` column) |
| The `test-support` feature exposing `TestAuthenticator` | every later host test that signs |

---

## 4 · Increments [plan]

Each is shippable alone; `auth_from_env` keeps refusing `passkey` until S2.
Each done-when names a test that fails without the increment. New host
tests go in **`crates/ordo-host/tests/auth_passkey.rs`**, a new file, so the
`auth_door.rs` harness is shared with enrolment's tests and not fought over.

| | What | Files | Done when |
|---|---|---|---|
| **S0** | **The auth routes record what they decide** (= F0, = R2's recording half). `AuthState` gains `recorder: Option<Arc<Recorder>>`; `complete` records `Authenticated` or `AuthenticationFailed { reason }` and the audit events; `logout`, `refresh` record their refusals | `crates/ordo-host/src/auth.rs`, `runtime.rs` (the recorder is created at `2046`, after `AuthState` at `2028-2040`; S0 adds `AuthState::with_recorder` and calls it once the recorder exists), `tests/auth_door.rs` | `auth_door.rs::a_refusal_is_one_answer_to_the_caller_and_a_reason_to_the_operator` (two refusals, same body, two different reasons in the stream) and `::an_onboarding_is_in_the_security_stream` (`PrincipalOnboarded` and `AuthenticatorLinked` read back) |
| **S1** | **Durable keys and challenges.** `PasskeyStore`, `CeremonyStore`, memory doubles, `PasskeyProvider::with_stores(rp, keys, ceremonies)`, `CoseKey::to_bytes/from_bytes`, BE and BS accessors, the sign counter written through; `PgPasskeyStore`, `PgCeremonyStore`, `ensure_table` for both at boot; the sweep worker; the `test-support` feature moving `TestAuthenticator` to `crates/ordo-identity/src/software_authenticator.rs` | `crates/ordo-identity/src/{passkey.rs, webauthn.rs, ceremony.rs (new), software_authenticator.rs (new), lib.rs, Cargo.toml}`, `crates/ordo-host/src/{passkeys.rs (new), ceremonies.rs (new), lib.rs (mod lines), runtime.rs:1975-1984, workers.rs}`, `crates/ordo-host/Cargo.toml` (dev-dependency feature) | `ordo-identity`: `a_credential_registered_through_one_provider_asserts_through_another_over_the_same_store`, `a_challenge_taken_once_is_gone_for_every_provider_over_the_store`, `a_cose_key_round_trips_and_a_bad_point_does_not_load`. `auth_durability.rs::a_passkey_survives_a_restart` (register, drop the store, open a new one, assert), `::a_sign_count_regression_is_detected_after_a_restart` |
| **S2** | **The relying party from the environment.** `auth_from_env` accepts `passkey`, reads the four variables of §2.5, refuses as stated; `AuthConfig` gains `relying_party: Option<RelyingParty>`; the registry loop registers `PasskeyProvider`; `AuthState` holds a typed handle to it for the routes that need the concrete type; the census line | `crates/ordo-host/src/runtime.rs` (`367-426`, `240-254`, `704-710`, `1998-2019`, `2848-2863`), `auth.rs` (`AuthState`), `docs/deploying.md:254` | `tests/runtime.rs::passkey_without_an_apex_domain_refuses_the_boot`, `::an_origin_outside_the_apex_refuses_the_boot_by_name`, `::password_alone_still_boots_without_an_apex_domain` |
| **S3** | **Sign-in at the door.** `/auth/begin` answers the request options of §2.2 for `passkey`; `/auth/complete` with `passkey_assertion` mints a token with presence | `auth.rs` (`begin`, `complete`), `tests/auth_passkey.rs` (new) | `auth_passkey.rs::a_passkey_ceremony_mints_a_token_that_passes_step_up` (software authenticator through the routes; `IdentityAccess::check` on a step-up op is `Allow`; the control, a password token, is `Deny` with `StepUpRequired`), `::an_assertion_for_an_unknown_credential_is_the_one_answer`, `::a_replayed_assertion_is_refused` |
| **S4** | **Registration at the door.** `/auth/register`; `DirectoryStore::authenticators_of`; the user handle; `resident_key`, `attestation`, `exclude_credentials`; the store insert after `sign_in`; `ticket` present is refused until S7 | `auth.rs` (new handler, `router`), `serve.rs:337-340`, `crates/ordo-identity/src/directory.rs` (trait, `MemoryDirectoryStore`, `Directory::authenticators_of`), `crates/ordo-host/src/principal_directory.rs` (one query), `tests/auth_passkey.rs` | `auth_passkey.rs::a_registration_under_auto_onboard_mints_a_principal_and_a_token_with_presence`, `::a_registration_under_invite_only_is_the_one_answer_and_stores_nothing` (the `$passkey` count is unchanged), `::registering_the_same_credential_twice_is_refused`, `::a_registration_challenge_cannot_complete_a_sign_in` |
| **S5** | **Step-up inside a session.** The two routes of §2.6 | `auth.rs`, `serve.rs`, `tests/auth_passkey.rs` | `auth_passkey.rs::a_password_session_steps_up_with_a_passkey_linked_to_the_same_principal` (needs a principal holding both: enrolled through `link_first` in the test, as `auth_durability.rs:329` does), `::a_passkey_of_another_principal_cannot_step_up_a_session`, `::a_logged_out_session_cannot_step_up`, `::a_capability_token_cannot_step_up` |
| **S6** | **The browser ceremony in the SDK.** `clients/ordo-sdk/src/auth/passkey.ts`, exported | `clients/ordo-sdk/src/auth/passkey.ts` (new), `clients/ordo-sdk/src/index.ts`, `clients/ordo-sdk/src/auth/passkey.test.ts` (new, vitest, `navigator.credentials` stubbed) | `the_four_byte_strings_reach_the_server_as_unpadded_base64url`, `client_data_json_is_posted_byte_for_byte`, `a_challenge_is_decoded_to_the_bytes_the_server_minted` |
| **S7** | **Registration by ticket** [seam: E2]. `/auth/register` redeems the ticket through E2's `tickets::redeem`; the credential links to the principal the ticket names with `link_first`; `exclude_credentials` from `authenticators_of` | `auth.rs`, `tests/auth_passkey.rs` | `auth_passkey.rs::a_ticket_registers_a_passkey_for_the_person_it_names`, `::a_redeemed_ticket_registers_nothing_a_second_time`, `::a_ticket_for_a_person_who_already_holds_a_passkey_is_refused` (the case for R2's `link`) |

**Order: S0, S1, S2, S3, S4, S5, S6, S7.** S0 and S1 have no dependency
on each other and can run in parallel. S6 can start once S3's JSON shape
is fixed, which is §2.2, so it can start now. S7 waits for E2.

---

## 5 · Threats this plan answers, briefly

| | Attack | Answer |
|---|---|---|
| a phishing page runs a real ceremony | origin check against `ORDO_AUTH_ORIGINS` (`webauthn.rs:660-665`), refused at boot if an origin is outside the apex (`passkey.rs:232-238`) |
| a captured assertion is replayed | the challenge is one-shot across replicas (`take` deletes), and the sign counter is durable |
| a stolen bearer token adds a passkey | `/auth/register` never calls `link`; a second passkey needs a `LinkCeremony` (R2) |
| a stolen bearer token steps up with the thief's own passkey | `refresh_presence`'s binding check (`session.rs:560`) |
| a stranger enumerates who has a passkey | no identifier at `begin`; unknown credential and bad signature are one answer (`passkey.rs:576-580`) |
| a challenge outlives the presence window | `CHALLENGE_TTL_SECS` is pinned to the step-up window (`passkey.rs:53-57`), and `expires_at` enforces it in the table |

---

## 6 · What this plan does not do

- It does not link a second passkey, unlink one, or list them for the
  person. That is R2, which uses `authenticators_of` from S4.
- It does not build the admission policy, tickets, first-comer or the
  sweep of unadmitted registrations. E2, E4, E5 own those; §3 says what is
  taken from each.
- It does not read the assertion's `userHandle`. The credential id resolves
  the key, the key's authenticator id resolves the principal, and that is
  the binding that matters.
- It does not verify attestation. `security-logging-design.md` §2.1.
- It does not move the password provider's challenges into `$authceremony`.
- It does not add a `role` to `$passkey`. `plan-second-factor.md` T3 does.
- It does not build a console page or a React hook.

---

## 7 · Where this collides with enrolment E1 to E7

Both are built at once. This table is the agreement; a builder who has to
break it says so in the commit.

| File | This plan | Enrolment | Rule |
|---|---|---|---|
| `crates/ordo-host/src/runtime.rs` | S2 edits `auth_from_env` (367-426), `AuthConfig` (240-254), the registry loop (1998-2019), the census (2848-2863); S1 adds two `ensure_table` calls at 1975-1984 | E4 rewrites the `ORDO_AUTH_PROVISIONING` read (410-419) and the `Provisioning` match (2020-2027); E5 adds the claim-code variable | **S2 lands first.** E4 rebases onto it. Neither renames a field of `AuthConfig` the other reads |
| `crates/ordo-host/src/auth.rs` | S0, S3, S4, S5, S7 change `begin`, `complete`, `AuthState`, `router` | E2's ticket redemption, E6's reset | **E2 and E6 do not edit `auth.rs`.** E2 lives in `crates/ordo-host/src/tickets.rs` with one function this plan calls: `redeem(ticket: &str, now) -> Result<Redeemed { principal: Option<PrincipalId>, purpose: Purpose, display: String }, TicketError>`. E6's reset is a platform op in `platform_ops.rs`, as `plan-enrolment.md` §7 already says every setting is |
| `crates/ordo-host/src/serve.rs` | S4, S5 add routes beside 337-340; S0 hands the recorder to `AuthState` | E-routes, if any, and 14g's CORS layer | routes are added one per line and E adds none; CORS wraps the router and does not edit these lines |
| `crates/ordo-host/src/lib.rs` | S1 adds two `pub mod` lines | E1 rewrites `MEMBERS_SQL` (1600-1601) and `engine_from_members` (1669-1703) | no overlap |
| `crates/ordo-identity/src/directory.rs` | S4 adds `authenticators_of` to the trait, the memory store and `Directory` | E6 adds the managed/independent state to `Principal` or beside it | both append; E6 does not touch `link`, `link_first`, `resolve_or_onboard` |
| `crates/ordo-host/src/principal_directory.rs` | S4 adds one `SELECT` | E6 adds columns to `$principal` and their reads | both append. `PRINCIPAL_DDL` (63-69) is E6's to change; S4 does not touch it |
| `crates/ordo-host/tests/auth_door.rs` | S0 adds two tests and the recorder to the harness (30-61) | E-tests | **E writes its tests in new files** (`enrolment.rs`, `tickets.rs`, `admission.rs`); this plan's are in `auth_passkey.rs` |
| `crates/ordo-host/tests/auth_durability.rs` | S1 adds two tests | E2 may add a ticket durability test | append only |
| `docs/deploying.md` | S2 changes the `ORDO_APEX_DOMAIN` row (254) and adds three rows | E4, E5 add rows | append; the lead session merges |
| `docs/roadmap-checklist.md`, the board | the lead session's | the lead session's | not edited by builders |

Two increments touch nothing enrolment touches: S1 (stores) and S6 (SDK).
Start with those if the two teams start on the same day.

---

## 8 · Open for the owner

None found. Every choice above follows a ruling or a settled document, or
is a build detail a builder may make. Two choices are recorded so they can
be disagreed with:

1. `ORDO_AUTH_ORIGINS` is a separate setting from the CORS allowlist
   (§2.5). If the owner prefers one setting, S2 derives the origins from
   the allowlist and drops the variable.
2. The user handle is random and not the principal id (§2.2). If the owner
   wants a person's passkeys to be findable by principal id without a
   join, S4 can store the principal id in the handle for principals that
   exist at begin, and the random handle only for strangers.

---

### Critical files for implementation

- `crates/ordo-identity/src/passkey.rs` (the provider, the two stores it gains, the options)
- `crates/ordo-host/src/auth.rs` (the three routes, the recorder)
- `crates/ordo-host/src/runtime.rs` (`auth_from_env`, the registry loop, the census)
- `crates/ordo-host/src/credentials.rs` (the store pattern the two new stores copy)
- `crates/ordo-identity/src/passkey_tests.rs` (the software authenticator that moves behind `test-support`)

Supporting: `crates/ordo-identity/src/webauthn.rs` (flags, `CoseKey` bytes), `crates/ordo-identity/src/directory.rs` (`authenticators_of`), `crates/ordo-host/src/principal_directory.rs`, `crates/ordo-host/src/serve.rs`, `crates/ordo-host/src/workers.rs` (the sweep), `clients/ordo-sdk/src/auth/passkey.ts` (new).

---

## Built

### S0 · The auth routes record what they decide (commit `e21d7cc`)

- `AuthState` has `recorder: Option<Arc<Recorder>>` and `with_recorder`.
  `begin`, `complete`, `refresh` and `logout` record every refusal as
  `AuthenticationFailed { reason }` with `Subject::Unverified`, and the
  caller still gets the one answer each gave before. Reasons come from a
  fixed vocabulary (`bad-credentials`, `unknown-challenge`, `unknown-method`,
  `malformed-request`, `not-invited`, `sign-in-refused`, `token-unverified`,
  `no-token`, `refresh-refused`, `not-a-session`, `webauthn-refused`, and the
  store-unavailable ones). Nothing the caller sent is copied into one.
- A sign-in records the audit events `sign_in` returns
  (`PrincipalOnboarded`, `AuthenticatorLinked`) and then `Authenticated`, all
  naming the one principal.
- Tests in `tests/auth_door.rs`: `a_refusal_is_one_answer_to_the_caller_and_a_reason_to_the_operator`,
  `an_onboarding_is_in_the_security_stream`, and
  `a_refused_refresh_and_a_refused_logout_are_recorded`. With `record` made a
  no-op all three fail. The harness gained one line: a recorder on the state.

Where the plan was wrong against the code:

- `EventKind::AuthenticationFailed` had no payload, so the reason had nowhere
  to go. It is now `AuthenticationFailed { reason: String }`
  (`ordo-secevent/src/event.rs`, `chain.rs`: tag 2 followed by the string,
  rendered as `authentication-failed reason=...`). No deployment exists, so
  the encoding change owes no migration.
- The plan said `AuthState::with_recorder` is called once the recorder exists.
  `AuthState` is held in an `Arc` inside a tuple by then, so instead the
  `Arc<Recorder>` is created a few lines earlier, before the auth block, and
  passed in. The `let recorder` line moved; nothing else about the drain or
  the door changed.

### S1 · Durable keys and challenges (commit `479ea7a`)

- `ordo-identity`: `ceremony.rs` (`CeremonyStore`, `CeremonyRow`,
  `MemoryCeremonyStore`); `PasskeyStore`, `StoredPasskey`,
  `MemoryPasskeyStore` in `passkey.rs`; `PasskeyProvider::with_stores`
  (`new` keeps the memory doubles, so every earlier test is unchanged);
  `CoseKey::to_bytes` and `from_bytes` (re-runs the on-curve check); `BE` and
  `BS` accessors on `AuthenticatorData`; the counter written through before
  the subject is returned, a failed write refusing the assertion; the
  challenge filed under the SHA-256 of its bytes, never as itself; a 32-byte
  user handle minted at `begin_registration`, held in the ceremony payload and
  written to the credential row.
- `software_authenticator.rs`: `TestAuthenticator`, `TestKey` and the CBOR
  helpers moved out of `passkey_tests.rs`, fields public, plus `origin`,
  `backup_eligible` and `backed_up`. Compiled under `cfg(any(test, feature =
  "test-support"))`. `ordo-host` enables the feature in `[dev-dependencies]`.
- `ordo-host`: `passkeys.rs` (`PgPasskeyStore`, `$passkey`, and `SyncPg`, the
  thread-and-runtime bridge of `PgCredentialStore` written once), `ceremonies.rs`
  (`PgCeremonyStore`, `$authceremony` in the shape of `plan-auth-federated.md`
  §3.1), both `ensure_table`s at boot, the ceremony store opened on the shared
  single-connection pool, and `workers::CeremonySweep` (`auth.ceremony.sweep`,
  60 s, listed as disabled in the census when no authentication is
  configured). `PgPasskeyStore` has no boot caller until S2 wires the provider.
- Tests. `ordo-identity`:
  `a_credential_registered_through_one_provider_asserts_through_another_over_the_same_store`,
  `a_challenge_taken_once_is_gone_for_every_provider_over_the_store`,
  `a_cose_key_round_trips_and_a_bad_point_does_not_load`, and also the flags and
  handle, the write-through counter, a refused write, a wrong phase, a
  challenge stored by hash, and expiry. `auth_durability.rs`:
  `a_passkey_survives_a_restart` (the challenge is minted before the restart
  and answered after it), `a_sign_count_regression_is_detected_after_a_restart`,
  and three more (flags in the row, a hand-edited key, one-shot across two
  stores). With the counter write removed, the identity write-through tests and
  both restart tests fail.

Where the plan was wrong, or a choice the plan left open:

- `set_sign_count` returns `Result<bool>`, not `Result<()>`. It advances the
  count only when the new count is greater, in one statement, and `false`
  means a second assertion with the same count got in between. That is the
  race a read-then-write cannot see across replicas. A count of zero only
  records the use.
- `PasskeyStore::insert` refuses an existing id with
  `PasskeyStoreError::AlreadyExists`, distinct from `Unavailable`.
- The plan says `consume` keeps the property that a failed attempt burns the
  challenge. That holds for a bad signature, origin or type. It does **not**
  hold for a challenge presented at the wrong phase: `take` has the phase in
  its `WHERE`, so a registration challenge shown to the sign-in route is
  refused and left open. Nothing depends on the old behaviour, and the other
  ceremony is not consumed by a stranger's attempt.
- `begin_registration` and `begin_assertion` now return `Result`, because the
  store can be down. The `AuthProvider::begin` trait method cannot report
  that, so on a store failure it hands back a challenge that was never filed,
  and completing it is the ordinary refusal. S3 and S4 call the two concrete
  methods, which report it.
- `CreationOptions` gained `user_handle`. The plan put the user block in S4;
  the handle is minted here, so the field is here.
- The ceremony store is held on `AuthState` (`ceremonies`, `with_ceremonies`)
  and the sweep worker is built from `rt.platform.auth`, so `Runtime` gained no
  field.
- `expires_at` is `minted_at + ttl`, and `take` at exactly that second finds
  nothing (`expires_at > now`). The old map allowed the answer at exactly the
  ttl. One second, and the existing tests pin `ttl + 1`.

### S2 · The relying party from the environment

`auth_from_env` accepts `passkey`. With it, `ORDO_APEX_DOMAIN` and
`ORDO_AUTH_ORIGINS` are required and each refusal names its variable;
`ORDO_AUTH_RELATED_ORIGINS`, `ORDO_AUTH_RP_NAME` and
`ORDO_AUTH_USER_VERIFICATION` are optional. `AuthConfig` has
`relying_party`. The registry registers the provider through a shared
handle (`impl AuthProvider for Arc<PasskeyProvider>`), and `AuthState.passkey`
holds the same provider typed, for the routes that need it. The census line
names the relying party, its origin counts, and what to publish. Rows for all
five variables are in `docs/deploying.md`.

**The owner's answer of 2026-09-29 is built.** Origins are their own setting,
not derived from `ORDO_CORS_ORIGINS`, and hold several addresses. An address
under the apex works as before. An address on another domain is the explicit
option `ORDO_AUTH_RELATED_ORIGINS` (`RelyingParty::with_related_origins`:
https only, no path, no loopback). A bare `ORDO_AUTH_ORIGINS` entry on another
domain still refuses the boot, and the refusal names the option.

**The well-known file.** The server can serve it and does: `GET
/.well-known/webauthn` answers `{"origins": [...]}` when related origins are
set, and 404 otherwise. That helps only when the door answers on the relying
party's own host. A browser asks `https://<apex>/.well-known/webauthn`, and the
apex is usually a different site from the door. So the deployment owner
publishes the same JSON at the apex host, or routes that path to the door.
`docs/deploying.md` says so. What no server can do: browsers cap the number of
distinct domains in the file (five in the current draft), and the server does
not police that.

Tests: `tests/runtime.rs`: `passkey_without_an_apex_domain_refuses_the_boot`,
`passkey_without_origins_refuses_the_boot`,
`an_origin_outside_the_apex_refuses_the_boot_by_name`,
`a_bad_apex_a_bad_related_origin_and_a_bad_word_each_refuse_by_name`,
`password_alone_still_boots_without_an_apex_domain`,
`a_passkey_deployment_names_its_relying_party_and_publishes_its_related_origins`.
`ordo-identity`: three related-origin tests (the refusal without the option,
the https-only rules, a ceremony from a related origin completing and an
unlisted one not). The plan said S2 edits `docs/deploying.md:254` and adds
three rows; it adds five, because the well-known publication needs its own.

### S3 · Sign-in at the door

`/auth/begin` for `passkey` mints through `PasskeyProvider::begin_assertion`
off the worker's thread and answers the options of §2.2: `challenge`,
`method`, `rp_id`, `user_verification`, an empty `allow_credentials`, and
`timeout_ms` (the challenge's own life). A store outage at begin is
`credential_store_unavailable`, recorded. `/auth/complete` with
`passkey_assertion` was already served by the registry provider, so the token
with presence needed no new code.

Tests in `tests/auth_passkey.rs` (a new file, with a harness later increments
share): `a_passkey_begin_answers_what_a_browser_needs`,
`a_passkey_ceremony_mints_a_token_that_passes_step_up` (a password token,
the control, is denied with `StepUpRequired`),
`an_assertion_for_an_unknown_credential_is_the_one_answer`,
`a_replayed_assertion_is_refused`.

**The plan overstated what S3 changes.** The step-up and replay tests pass
against S2's code, because the registry's `begin` already minted a usable
challenge. What S3 adds is the options a browser needs, and only the first
test fails without it.

### S4 · Registration at the door

`POST /auth/register {"method":"passkey","display"?}` answers the creation
options of §2.2 (user handle, algorithms `[-7,-8]`, `resident_key: required`,
`attestation: none`, `exclude_credentials`, `timeout_ms`). `POST
/auth/complete` with `passkey_registration` verifies, runs `sign_in`, and only
then stores the key, so a registration the policy refuses leaves nothing in
`$passkey`. To make that order possible the provider gained
`verify_registration` and `store_registration` around a `PendingRegistration`;
`complete_registration` is those two together, so the registry path and every
earlier test are unchanged. `DirectoryStore::authenticators_of` (trait,
memory store, `PgDirectoryStore`, `Directory`) lists a principal's
authenticators sorted.

Tests: `auth_passkey.rs`: `a_registration_begin_answers_the_creation_options`,
`a_registration_under_auto_onboard_mints_a_principal_and_a_token_with_presence`,
`a_registration_under_invite_only_is_the_one_answer_and_stores_nothing`
(fails, with the key stored, if the store runs inside the provider as before),
`registering_the_same_credential_twice_is_refused`,
`a_registration_challenge_cannot_complete_a_sign_in`,
`register_without_a_passkey_method_is_refused`. `auth_durability.rs`:
`authenticators_of_lists_one_principals_links_sorted`, on Postgres and the
memory double.

Choices the plan left open:

- A duplicate is caught twice: `verify_registration` reads the store before
  the directory is touched, and the insert refuses the race that check cannot
  see. If the insert then loses, the link `sign_in` made is left pointing at a
  key that is somebody else's row. It is the same authenticator id, so it
  resolves to whoever linked first. No compensating removal is built.
- A registration for a credential id that is linked but has no key row (a
  store outage after `sign_in`) leaves a link nobody can use. The stream
  records `credential-store-unavailable`.
- `ticket` in the body is refused until S7.

### S5 · Step-up inside a session

`POST /auth/step-up/begin` (bearer) mints an assertion challenge in its own
phase, `step-up`, with the session's principal in the ceremony row, and
answers request options with `allow_credentials` narrowed to that principal's
passkeys (`authenticators_of`). `POST /auth/step-up` (bearer plus a
`passkey_assertion`) verifies through `complete_step_up`, checks the
challenge was minted for this session's principal, then `refresh_presence`
checks the passkey's own link, and the same session comes back re-minted with
fresh presence: same principal, client and session id, expiry capped at the
session's ceiling. Before any ceremony the bearer must verify, name a session
that has not ended (`is_revoked` and `principal_ended`), and be neither a
capability nor a delegation. Every refusal is the one answer; the stream has
the reason (`session-ended`, `presence-not-delegable`,
`passkey-of-another-principal`, `challenge-for-another-principal`, ...). A
success records `StepUpSatisfied`.

Tests, `auth_passkey.rs`:
`a_password_session_steps_up_with_a_passkey_linked_to_the_same_principal`,
`a_passkey_of_another_principal_cannot_step_up_a_session` (both crossings: a
foreign passkey on my challenge, and my passkey on a challenge minted for
another session), `a_logged_out_session_cannot_step_up` (no new challenge, and
an old one is no use), `a_capability_token_cannot_step_up`,
`a_step_up_challenge_and_a_sign_in_challenge_are_not_interchangeable`.

The principal holding a password and a passkey is built with
`DirectoryStore::insert_link` in the tests, because the production path for a
second link (`Directory::link`) is recovery R2's ceremony and needs a presence
assertion this test does not have.

### S6 · The browser ceremony in the SDK

`clients/ordo-sdk/src/auth/passkey.ts`, exported from `src/index.ts`:
`registerPasskey`, `signInWithPasskey`, `stepUp`, a `PasskeyError` with a
`failure` (`unsupported`, `cancelled`, `refused`, `unavailable`,
`malformed`), and `toBase64Url` and `fromBase64Url`. Byte strings are
base64url without padding. `clientDataJSON` is encoded from the browser's
`ArrayBuffer` and never parsed. The challenge is used two ways: the decoded
bytes go to `navigator.credentials`, and the string the server sent goes back.
The optional `space`, `ticket` and `claimCode` ride on sign-in and
registration for the hooks in the sections below.

Tests are `clients/ordo-sdk/test/passkey.test.ts` (the SDK's vitest include is
`test/**`, not the plan's `src/auth/passkey.test.ts`); 98 SDK tests pass. Named as
planned: `the_four_byte_strings_reach_the_server_as_unpadded_base64url`,
`client_data_json_is_posted_byte_for_byte`,
`a_challenge_is_decoded_to_the_bytes_the_server_minted`. The stub stands in for
`navigator.credentials`, so these prove the encoding and no gesture.

The functions take a `PasskeyEndpoint` (`baseUrl`, and optional `fetch` and
`credentials` for tests) and not the plan's `client`, because `OrdoClient`
keeps its base URL and `fetch` private and its calls are bound to a space and
a token, which an unauthenticated sign-in has neither of.

### S7 · Registration by ticket

`/auth/register` with a `ticket` looks at it (`Tickets::peek`, new: the store's
`TicketStore::peek` answers what `take` would without spending) for the
purpose `passkey`, refuses a person who already holds any factor, and binds
the ceremony to the ticket's hash in the ceremony row
(`begin_registration_bound`, `PendingRegistration::binding`). `/auth/complete`
then requires the same ticket, looks again, spends it (`redeem_for`), and
links the credential with `link_first` to the person it names, or to a person
minted for a ticket that names nobody. The deployment's admission policy is
not consulted: the ticket is the invitation, so it works under `InviteOnly`.
A refusal spends nothing. The routes get `Tickets` through `Enrolment`, wired
once from the platform ops in `runtime.rs`, so the store a ticket is redeemed
against is the one `std.space.createPerson` issued into.

Tests, `auth_passkey.rs`:
`a_ticket_registers_a_passkey_for_the_person_it_names`,
`a_redeemed_ticket_registers_nothing_a_second_time`,
`a_ticket_for_a_person_who_already_holds_a_passkey_is_refused`, and also a
ticket that names nobody, a ticket of another purpose, a ceremony bound to its
ticket (three crossings, none of which spends it), and tickets not honoured
where none are wired.

Where the plan was wrong or open:

- The plan says `tickets::redeem(ticket, now)`. E2 built
  `redeem(store, ticket, now)` and `Tickets::redeem_for`, and this uses the
  second, with a `Want` so a ticket of the wrong purpose is refused unspent.
- The plan's `exclude_credentials` "from `authenticators_of`" has nothing to
  list: a ticket is only good for a person who holds no factor, so the list is
  empty. It is not wired. A ticket that lets a person with a password add a
  passkey is the second-factor plan's, and would list it then.
- The bound ceremony is not in the plan. Without it the begin-time checks
  (does the person hold a factor?) and the complete-time spend could be run
  with two different tickets, and a ceremony begun with no ticket could
  finish by presenting one. The stream words are `ticket-binding` and
  `ticket-required`.
- A tenant invitation at sign-in travels as `invitation`, not `ticket`, in the
  request body, so a passkey ticket and an invitation cannot be confused. The
  SDK's `SignInOptions` and `RegisterOptions` say `invitation`.
- I did not merge the two one-connection bridges (`passkeys::SyncPg` and
  E2's `pgsync::PgSync`). They are the same pattern written twice.

### The four hooks enrolment left on the sign-in route

Enrolment's platform ops reach the routes through one struct,
`auth::Enrolment { tickets, admission, credentials }`, wired once in
`runtime.rs` from the platform ops' own stores, so the ticket store a route
redeems against is the one `std.space.createPerson` issued into.

**1 · `Admission::on_arrival` at sign-in.** `/auth/complete`, for any method,
takes an optional `space` (with `invitation` and `claim_code`) and answers
`"admission": {"space", "outcome"}`, `outcome` being `admitted`,
`already-member` or `refused`. A refusal does not fail the sign-in. Every
error is `refused`, so a tenant that does not exist reads as one that does not
admit strangers; the stream keeps `admission-unavailable`. The body field is
`invitation` and not `ticket`, so an invitation and a passkey registration
ticket are never confused. Tests, `tests/signin_enrolment.rs`:
`a_sign_in_that_names_a_tenant_runs_the_tenants_admission` and
`an_invitation_presented_at_sign_in_admits_at_its_role_and_a_wrong_one_is_not_spent`.

**2 · `CredentialOps::redeem_password` at the door.** `POST
/auth/password/redeem {"ticket","identifier","password"}`: `204`, `409
identifier_taken` for the holder of a valid ticket (not spent), and the one
refusal for anything else. No token comes back; the person signs in the
ordinary way. Test: `a_ticket_redeemed_at_the_door_sets_a_first_password`.

**3 · A temporary password's forced change.** After a password proves who
signed in, `ManagedStore::must_change` is asked (a table that cannot be read
refuses the sign-in). If it is true the answer is `403 password_change_required`
and no session. A passkey sign-in is not held up. The person calls the new
`POST /auth/password/change {"identifier","password","new_password"}`
(`CredentialOps::change_password`): the current password proves who is asking,
an unknown identifier costs the same work as a wrong one, the marker is
cleared, and **every session the person had ends**. Test:
`a_temporary_password_cannot_sign_in_until_its_holder_chooses_their_own`.

One consequence to know: `end_all(at)` ends sessions that started **at or
before** `at`, so a person who changes their password and signs in again
within the same second gets a session that is already ended. The test moves
the clock five seconds. A browser flow takes longer than a second. A script
that changes a password and signs in at once will see a refusal, and E6 chose
that side of the tie deliberately.

**4 · A worker that refreshes `PgSessionRevocations`.** `sessions.refresh`
(`workers::SessionsRefresh`), every `ORDO_SESSION_REFRESH_SECS` (default 15,
zero or unparseable falls back), on every replica. The census lists it as
disabled when no authentication is configured. Test, against the binary:
`tests/runtime.rs::a_second_replica_stops_serving_an_ended_session_without_a_restart`
(a second store revokes a session, and the running binary starts answering
`401` at `/auth/refresh` with no restart; with the worker not pushed it fails
after ten seconds).

### Where presence is required: the app developer's setting

Built last, as ruled. Owner, 2026-09-29: about the membership and admission
ops, *"THis is not ours to decide - that is for the developer"*, and then
about every platform op, *"What is enforced is app design ... not our
design."* `docs/security-logging-design.md` gains §2.3, which says the
requirement is a deployment setting with today's behaviour as its default and
cites the ruling. (I did not edit `corpus-rulings.md`; the second quotation
comes from the lead session's message and is the lead session's to record
there.)

**What I chose.** One environment list, `ORDO_STEP_UP_EXEMPT_OPS`, of the
platform ops that **do not** require presence. An entry is an exact op name or
a prefix ending `.*` (`std.space.*`). Unset or empty is presence required on
every platform op. Code: `crates/ordo-host/src/presence.rs`
(`PresencePolicy`), applied where the engine is built
(`engine_from_members_under`, which `engine_from_members` calls with the
installed policy, so every rebuild of the door agrees).

**Why a list of exceptions, and not a table of per-op values.**

- The default has to be the safe one, and a list of exceptions is a list of
  departures from it: reading the variable is reading what was loosened.
- An op added by a later release requires presence until somebody names it. A
  table of values would need a default for the ops it does not mention, which
  is the same decision made twice.
- It covers every platform op with one mechanism, so the five the owner named
  (`std.deployment.apply`, `std.space.create`, `std.space.import`,
  `std.space.export`, `std.space.delete`) and the membership and admission ops
  are not special cases.
- A value per op ("presence within 60 seconds", "none") would invite the
  freshness window becoming a second setting. The window stays the engine's.

**What it does not do.** It keeps the role gate: an exempt op is refused to a
caller the engine does not list, and the owner-only ops stay owner-only. It
does not change how presence is proved. An entry that names no platform op
refuses the boot and lists the ones that exist, so a misspelt exemption cannot
leave the op asking while its owner believes it is not. The startup log says
what is exempt, or that presence is required on every op.

**A consequence the owner should know.** An exempt op is one a capability
token may be granted. The structural rule that keeps a delegation from
satisfying step-up has nothing to say about an op that asks for none, so
exempting `std.space.export` is also letting a granted app export. The
deployment reference says so.

**The policy is process-wide**, installed once by `assemble` before any engine
is built, because the engine is rebuilt in four places (deploy, member change,
the refresh timer, the dev loop) and none of them carries configuration. A
second install is ignored. `RuntimeConfig` gained a `presence` field, so its
four literals (`ordo-cli`'s `dev.rs` and three tests) say
`presence: Default::default()`.

Tests: `tests/presence.rs` (six: the default asks on every op; a named op stops
asking and its neighbour does not; a prefix is a whole word; each of the five
owner-named ops exempted alone; the role gate still applies; a misspelt name
is refused with the list) and, against the binary in `tests/runtime.rs`,
`presence_is_required_by_default_and_a_named_op_stops_asking` (two boots of the
same deployment: `callersOf` with no presence is 403, then 200 once exempted,
while `export` still asks) and
`an_exempt_op_that_names_nothing_refuses_the_boot_by_name`.

### Findings across S2 to S7 and the hooks

- **A flake, named:** `ordo-cli` `tests/operator.rs::verify_reports_the_security_stream_as_its_own_verdict`
  failed once in a combined `cargo test -p ordo-host -p ordo-identity
  -p ordo-secevent -p ordo-cli` run (exit 2, `--key-id was given without
  --public-key`) and passed alone and on a second run of the file. I did not
  find the cause. It touches the security stream's verdict, and this branch
  changed `EventKind::AuthenticationFailed`'s chain encoding, so it deserves a
  look on `main` before it is dismissed.
- The two one-connection bridges, `passkeys::SyncPg` and `pgsync::PgSync`
  (enrolment's), are the same pattern. Merging them is a small follow-up.
- `AuthState` now carries `passkey`, `ceremonies`, `session_store` and an
  `Enrolment`. The last is set once through a `OnceLock`, because the platform
  ops it holds are built after the auth state.
