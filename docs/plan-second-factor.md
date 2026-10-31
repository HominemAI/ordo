# A second factor, and the backup codes that stand in for it

> **The owner answered this plan's five questions on 2026-09-29** (`corpus-rulings.md`): 24-character codes; available to anyone by default; a reset or recovery removes every second factor once the reset completes; after a passkey, a second step only when turned on; 20 failures lock a factor by default, configurable. Section 11 is answered.

Build items 8 and 9 of the owner's order of 2026-09-29: *"recovery then the
second factor"*. Checklist 23a (backup codes) and 23b (a second factor with
a hardware token as an option). Board task `second-factor`.

Companion to `plan-passkey-recovery.md` (R1 to R4 are built before this
and are used by it), `plan-passkey-signin.md` (S1 to S4), `plan-enrolment.md`
(E1, E2, E4, E6), `plan-auth-federated.md` (F1, F5, F9, F14) and
`security-logging-design.md` §2, §2.2 and §5.

Markers: **[settled]** ruled by the owner or restated from a settled document
· **[plan]** proposed here · **[seam]** work another plan owns · **[open]**
the owner's to decide.

Status: **planned, not built.** Every claim about the code was read against
the tree at `2082f74` on 2026-09-29. This plan reflects the owner's answers
of 2026-09-29.

---

## 0 · The starting state, verified

| Claim | Where |
|---|---|
| Nothing about TOTP, HOTP or a second step exists | `grep -rniE "totp\|hotp\|second factor\|2fa\|mfa" crates --include='*.rs'` hits one doc comment in `crates/ordo-cli/src/keys.rs:136`, about custody |
| `/auth/complete` mints a token the moment the ceremony verifies. There is no half-signed-in state | `crates/ordo-host/src/auth.rs:282-312` |
| A token carries principal, expiry, client, presence, capability, delegation, session. Nothing about which factors were proved | `crates/ordo-identity/src/token.rs:141-172`, encoding `v3` at `:221-224` |
| Presence is minted only by a WebAuthn assertion whose `UP` flag was set; `user_verified` is recorded beside it and step-up does not read it | `passkey.rs:500-510`, `:605-612`; `access.rs:1004-1037` reads `user_presence_at` alone |
| `UserVerification::Preferred` exists for keys that cannot verify the user, and makes the weaker proof visible rather than silent | `passkey.rs:97-105` |
| A password session can never satisfy step-up | `session.rs:407-428`; `security-logging-design.md` §2 |
| Linking a first-factor authenticator needs fresh presence; a password-only person cannot link anything | `directory.rs:227-246`, `:550-586`; `plan-auth-federated.md` §2.5 |
| The credential store and the directory are durable, on one pattern | `crates/ordo-host/src/credentials.rs`, `principal_directory.rs` |
| HMAC-SHA256 and a constant-time compare are written in-house over `sha2` | `token.rs:378-410` |
| `hmac`, `sha2`, `subtle`, `getrandom` are in the lockfile. **`sha1` is not** | `Cargo.lock` |
| Sealing under the deployment's key provider, with a purpose string, exists | `ordo_crypto::KeyProvider`; `plan-auth-federated.md` Q11 uses it for a client secret |
| Recovery codes are planned and not built: ten codes, issued with presence, HMAC under a deployment key, single use, voided on reissue and on recovery | `plan-passkey-recovery.md` §3.2, R4 |
| The auth routes record nothing, and `AuthenticationFailed` exists to be recorded | `auth.rs:50-52`; `crates/ordo-secevent/src/event.rs:401-404`; fixed by `plan-passkey-signin.md` S0 |
| Every settled document says attestation is not verified, so the platform cannot prove a key is hardware | `security-logging-design.md` §2.1; `webauthn.rs:583-586` |

Built before this plan starts, in the owner's order: passkey sign-in
(S0 to S5), enrolment (E1 to E7), recovery (R1 to R8). What each hands
over is in §8.

---

## 1 · What the owner ruled [settled]

Five answers on `second-factor`, 2026-09-29, recorded in `corpus-rulings.md`:

| | Answer | What it fixes here |
|---|---|---|
| **A1** | **Hardware tokens: both** security keys and code-generating tokens | a WebAuthn assertion as a second step (§4), and TOTP (§3) |
| **A2** | **A second step applies to password sign-in and code sign-in, and optionally to passkeys**, as a person's setting or a tenant's requirement. Outside providers were not named and follow the two-level rule on provider two-step claims | §5: which sign-ins get a second step |
| **A3** | **Hardware is offered without proof.** Attestation is added if people ask; the settled rule against verifying it stands | registration with `attestation: "none"`; the record says *security key*, never *hardware key* |
| **A4** | **Who turns it on:** a person for themselves, a tenant can require it, the app developer can enable it or require it | §5.2: three levels, tightening only |
| **A5** | **The same backup codes stand in for a lost second factor**, with the condition, verbatim: *"if theres an industry standard way to add more validity to the One time backup coes then lets add that"* | §6: the standard, what Ordo does, and one set of codes with two uses |

Two earlier rulings reach in:

- Recovery answer 2: *"a day's block on export after recovery, adjustable."*
  A code used **alone** is recovery and gets the block. A code used **as
  the second step** registers nothing and gets none (§6.4).
- Federated answer 2, amended: a provider's two-step claim counts as
  presence when the app developer says so per provider, and a tenant may
  say so for its own data. §5.3 reads that for the second step.

---

## 2 · What a second factor is here, and one rule that does not move

A second step proves possession of a second thing after the first thing
was proved. It changes who can **sign in**. It does not change who can
**step up**.

**Presence still comes only from a first-factor passkey.** A second-factor
security key is verified by the same WebAuthn code and would mint a
`UserPresence`. This plan does not put it in the token. The reason is the
attack `directory.rs:5-23` was written against: a stolen or guessed
password is enough to enrol a second factor (§7.1), so if a second-factor
key could carry presence, a password plus an attacker's own key would pass
step-up and export a tenant. Keeping presence on the first-factor link
(`$authenticator`), which only `link_first`, a ticket or a `LinkCeremony`
can write, keeps `security-logging-design.md` §2.2 true by construction.

Consequences, stated plainly:

| Session | Can sign in | Can step up |
|---|---|---|
| password + TOTP | yes | no |
| password + security key | yes | no |
| password + backup code | yes | no |
| passkey (first factor), with or without a second step | yes | yes, within the window |
| provider sign-in whose two-step claim is accepted (F14) | yes | as F14 says, for the level that accepted it |

A person who wants both a password and step-up registers their key **as a
passkey** through `plan-passkey-signin.md` S4 or S7 and links it as a
first factor. That is a different act, needs a ticket or a `LinkCeremony`,
and is not made easier by this plan.

---

## 3 · Code-generating tokens: TOTP [plan]

RFC 6238 over RFC 4226. Parameters are the ones every authenticator app
implements, and they are constants, not configuration:

| | Value | Why |
|---|---|---|
| Hash | HMAC-SHA1 | the only algorithm Google Authenticator, Microsoft Authenticator and most hardware tokens accept. `sha1` is one new lockfile entry (RustCrypto, the family `sha2` is from). HMAC is written in-house beside `token.rs:385-410` |
| Digits | 6 | |
| Step | 30 s | |
| Skew | ±1 step | 90 s of clock drift, the usual allowance |
| Secret | 20 bytes from the OS CSPRNG (`getrandom`, as `passkey.rs:455-459`) | RFC 4226 §4 R6: at least 128 bits; 160 matches SHA-1 |
| URI | `otpauth://totp/<rp name>:<display>?secret=<base32>&issuer=<rp name>&algorithm=SHA1&digits=6&period=30` | base32 written in-house, RFC 4648 §6, about 30 lines |

**Storage.** `$secondfactor`, one row per factor:

```
"$secondfactor" (
  "id"           text   NOT NULL,   -- sf_<128 random bits hex>
  "principal"    text   NOT NULL,
  "kind"         text   NOT NULL,   -- 'totp' | 'security-key'
  "label"        text   NOT NULL,   -- what the person called it
  "secret"       text   NOT NULL,   -- totp: the sealed secret; security-key: the $passkey authenticator id
  "last_counter" bigint NOT NULL DEFAULT 0,  -- totp: the last accepted time step; key: unused
  "failures"     bigint NOT NULL DEFAULT 0,  -- consecutive failed attempts, reset on success
  "disabled_at"  bigint NOT NULL DEFAULT 0,
  "enrolled_at"  bigint NOT NULL,
  "last_used_at" bigint NOT NULL DEFAULT 0,
  PRIMARY KEY ("id")
)
```

The TOTP secret is **sealed**, not hashed: the verifier has to compute the
same HMAC. Sealed under the deployment's `KeyProvider` with purpose
`second-factor`, deployment-scoped for the reason `plan-auth-federated.md`
Q11 gives: sign-in must work when a tenant's database is unreachable, and
the credential is the deployment's (P1).

**Replay.** `last_counter` holds the time step of the last accepted code.
A code whose step is at or below it is refused, so a code observed over a
shoulder is worthless once it was used, inside its own 30 s. RFC 6238 §5.2.

**Throttling.** Five wrong codes end the ceremony (§5.1). Twenty
consecutive failures on one factor across ceremonies set `disabled_at`;
the factor then refuses until the person re-enrols it with a fresh first
factor. NIST SP 800-63B §5.2.2 allows up to 100; twenty is tighter and is
the number a person mistyping reaches never and a script reaches in a
minute.

**Enrolment.** `POST /auth/factors/totp/begin` (a bearer token plus a
reauthentication handle, §7.1) answers the URI and the secret once.
`POST /auth/factors/totp/confirm {factor, code}` verifies one code and
only then writes the row. A secret that was never confirmed is a
`$authceremony` row that expires. Nothing is enrolled that the person's
app has not proved it holds.

**Tests, pure, in `crates/ordo-identity/src/totp.rs`:** the SHA-1 vectors of
RFC 6238 appendix B (`59 → 94287082`, `1111111109 → 07081804`, and the
rest) and the HOTP vectors of RFC 4226 appendix D.

---

## 4 · Security keys as a second factor [plan]

A second-factor key is a WebAuthn credential registered with
`resident_key: "discouraged"` and verified by the code that exists. What
differs from a passkey:

| | Passkey (first factor) | Second-factor key |
|---|---|---|
| registered through | `/auth/register` (S4) | `/auth/factors/key/begin` + `/complete`, from a session with a fresh first factor (§7.1) |
| `$passkey.role` (a column T3 adds) | `passkey` | `second-factor` |
| linked in | `$authenticator` | `$secondfactor` (`secret` = the authenticator id). Never `$authenticator`, so `link_count`, `LastAuthenticator` and `resolve_or_onboard` never see it |
| user verification | `required` by default | `preferred` (`passkey.rs:101-104`): many keys have no PIN. The record says whether UV happened |
| can sign in alone | yes | **no.** `complete_assertion` for a `second-factor` row outside a second-step ceremony is `BadCredentials` |
| can mint presence into the token | yes | **no** (§2) |
| attestation | none | none (A3). The person labels it; the platform records *security key* and nothing about the model |

At the second step the challenge is minted with `allow_credentials`
narrowed to the principal's second-factor keys, the way step-up narrows
(`passkey.rs:309-314`), because a non-discoverable credential cannot be
found without its id. The sign counter is written through as S1 does.

`PasskeyProvider` gains a second `RelyingParty` value with `Preferred` UV
for this ceremony, over the same stores. Two providers, one table, told
apart by `role`.

---

## 5 · The second step in the flow [plan]

### 5.1 The half-signed-in state

`/auth/complete` runs the first ceremony and `sign_in` as today. Then,
before minting:

```
SecondStep::decide(principal, method, via, space?) -> None | Required { enrolled: [...] } | MustEnrol
```

If `None`, a token is minted as today. Otherwise the response is `200` with
no token:

```
{ "second_step": "<handle>", "binder": "<256 bits>",
  "methods": ["totp", "security-key", "backup-code"],   // or "enrol": true
  "expires_at": <now + 300> }
```

The state is a `$authceremony` row (`plan-passkey-signin.md` §2.4):
`connection = 'second-step'`, `phase = 'pending'`, `binder` = SHA-256 of
the binder, `payload` = principal, method, `via`, display, and whether the
first step carried presence. Durable, so a second step begun on one
replica completes on another (`plan-auth-federated.md` §0.3 item 2). The
binder travels back in the completion, and a handle seen in a log is
worthless without it.

Answering `200` after a correct password does tell the caller the password
was right. That is what every service with a second step does (GitHub and
Google, §6.2), and the alternative, running a fake second step for every
wrong password, would ask people for codes on accounts that are not theirs.
The first step's refusals stay the one answer.

### 5.2 Completion

`POST /auth/second-step` with `{handle, binder, kind, …}`:

| `kind` | Body | Check |
|---|---|---|
| `totp` | `code` | §3, against every enabled TOTP row of the principal |
| `passkey_assertion` | the four byte strings, after `POST /auth/second-step/begin {handle, binder}` minted the challenge | §4 |
| `backup_code` | `code` | §6.4 |

On success the row is taken (one-shot across replicas), a token is minted
with the session claim as today, and `second_step: <kind>` is written into
the token (§5.4). Every failure is the one answer; a TOTP or backup-code
failure decrements an attempt counter in the payload, and the fifth
failure takes the row. `AuthenticationFailed { reason }` and
`Authenticated` are recorded as S0 records them.

### 5.3 Which sign-ins get a second step (A2)

| First step | Second step |
|---|---|
| password | yes, when required by any level of §5.2 and the person has a factor; `MustEnrol` when required and they have none |
| email code (F9) | the same |
| passkey | **only if** the person turned it on for their passkeys or the tenant requires it for passkeys. Default off: a passkey already needs the device plus its unlock |
| outside provider (F4, F6, F11) | none when the connection's two-step claim is accepted at the level that is asking (deployment for a deployment-wide requirement, the tenant for its own), per the two-level rule; otherwise as password |

### 5.4 Three levels, and where each lives (A4)

| Level | Setting | Where | Values |
|---|---|---|---|
| **app developer** (the deployment) | `ORDO_AUTH_SECOND_FACTOR` | environment, until the `auth { … }` block is parsed (`plan-auth-federated.md` §0.2) | `off` · `enabled` (default) · `required` · `required-for-non-passkey` |
| **tenant** | `second_factor` on the tenant's admission record (E4's `$admission`) | changed by an owner with presence, through the op E4 gives every admission setting | `inherit` (default) · `required` · `required-for-non-passkey` |
| **person** | `require_second_step` on `$principal` | `POST /auth/factors/policy` from a session with a fresh first factor | `when-enrolled` (default: any sign-in method gets the step once a factor exists) · `also-for-passkeys` |

Tightening only. A tenant cannot require what the deployment set `off`,
and cannot loosen `required`. A person can always add a factor where the
deployment is not `off`.

**How a tenant's requirement reaches a session.** Sign-in names no space
(`token.rs:141-172`), so a tenant's requirement is checked in two places:
at `/auth/complete` when `begin` named a space (`plan-auth-federated.md`
§2.3 adds that field), and **at the door** otherwise. The token's
`second_step` claim (§5.5) is read by `IdentityAccess::check` beside the
revocation check (`lib.rs:1481-1498`): a session with `second_step: None`
asking a tenant that requires one is `Deny` with a new reason
`SecondStepRequired`, recorded, and the client signs in again naming the
space. The same place F5 checks `via` against `accepts`.

### 5.5 The token

`TokenSpec` gains `second_step: Option<SecondStepKind>` and the encoding
moves `v3 → v4`, refusing older tokens outright, the rule `token.rs:228-238`
states. F5 adds `via` in the same field set; whichever lands first makes
the version change and the other appends. `refresh` and `refresh_presence`
carry the claim through unchanged (`session.rs:370-378`, `:566-578`).

---

## 6 · Backup codes: the standard, and what Ordo does [plan]

### 6.1 The owner's condition

*"If theres an industry standard way to add more validity to the One time
backup coes then lets add that."* The worry behind it: a backup code is a
string on paper, and a string on paper that signs somebody in is a
password with a shorter life. The standard answer has two halves. The
first is how the code is made, stored and checked: **NIST SP 800-63B's
rules for look-up secrets**. The second is what a code may do on its own:
**in every major service, a backup code replaces the second factor and is
used with the first, never alone.**

### 6.2 The standard, with sources

**NIST SP 800-63B, look-up secrets.** Revision 3 (2017, updated 2020), §5.1.2
([pages.nist.gov/800-63-3/sp800-63b.html](https://pages.nist.gov/800-63-3/sp800-63b.html));
revision 4 (final, 2025), §3.1.2, same rules renumbered
([pages.nist.gov/800-63-4/sp800-63b.html](https://pages.nist.gov/800-63-4/sp800-63b.html)).
Quoted from revision 3:

| Rule | The text |
|---|---|
| generation | *"CSPs creating look-up secret authenticators SHALL use an approved random bit generator to generate the list of secrets"* (§5.1.2.1) |
| entropy | *"Look-up secrets SHALL have at least 20 bits of entropy"* (§5.1.2.1); revision 4: *"at least six decimal digits (or equivalent) in length"* (§3.1.2.1) |
| single use | *"A given secret from an authenticator SHALL be used successfully only once"* (§5.1.2.2) |
| storage | *"Verifiers SHALL store look-up secrets in a form that is resistant to offline attacks. Look-up secrets having at least 112 bits of entropy SHALL be hashed with an approved one-way function"*; below 112 bits, *"salted and hashed using a suitable one-way key derivation function"* with a salt of at least 32 bits (§5.1.2.2) |
| rate limiting | below 64 bits of entropy, *"the verifier SHALL implement a rate-limiting mechanism"* (§5.1.2.2); and in every case *"SHALL limit consecutive failed authentication attempts on a single account to no more than 100"* (§5.2.2; revision 4 §3.2.2) |
| what it counts as | a look-up secret is a *something you have*, and AAL2 is satisfied by *"a Memorized Secret authenticator and one possession-based ('something you have') authenticator"*, which includes a look-up secret (§4.2.1). One code alone is one factor |
| delivery | *"SHALL deliver the authenticator list securely to the subscriber"* (§3.1.2.1, rev. 4) |

**The industry practice.** A backup code is entered after the password, in
place of the authenticator code:

- GitHub: sign in with *"your username and password"*, then *"Under 'More
  options', click 2FA recovery code. Type one of your recovery codes"*
  ([docs.github.com](https://docs.github.com/en/authentication/securing-your-account-with-two-factor-authentication-2fa/recovering-your-account-if-you-lose-your-2fa-credentials));
  each is single use and a new set invalidates the old
  ([configuring recovery methods](https://docs.github.com/en/authentication/securing-your-account-with-two-factor-authentication-2fa/configuring-two-factor-authentication-recovery-methods)).
- Google: *"you can use a backup code for the second step"*; *"After you
  use a backup code to sign in, that code becomes inactive"*; ten 8-digit
  codes, reissuable at will
  ([support.google.com](https://support.google.com/accounts/answer/1187538)).

Neither lets a code sign somebody in by itself.

### 6.3 What Ordo does, rule by rule

| Choice | Follows |
|---|---|
| **Ten codes**, each **24 characters from a 32-symbol alphabet** (Crockford base32: no `I`, `L`, `O`, `U`), shown as six groups of four, **120 bits** each, from the OS CSPRNG | generation; entropy well above 112 bits, so no salt or slow hash is owed; ten matches Google and GitHub |
| Stored as **HMAC-SHA256 under a deployment key** released from the `KeyProvider` with purpose `recovery-codes`; never the code | storage: an approved one-way function over a secret at or above 112 bits, and keyed, so a table dump alone confirms nothing. This is R4 as planned, with the length fixed |
| **`used_at` set on redemption; a used code is refused; issuing a new set voids the old; recovery voids the set** | single use |
| **Five attempts per ceremony, then the ceremony ends. Twenty consecutive failures per principal disable code redemption until a fresh set is issued with a first factor** | rate limiting, tighter than 100 |
| **Shown once**, in the response to the session that issued them, never mailed, never shown again; a person who lost them issues a new set | delivery |
| **As a second step, a code is accepted only after a first factor succeeded in the same ceremony** (§5.1) | one factor plus one factor is AAL2; a code alone is one factor |
| **Alone, a code opens a recovery case and nothing else** (§6.4) | a single factor gets the treatment recovery gives every proof weaker than a passkey: sessions ended, a new passkey, the export block |
| Every redemption is recorded (`RecoveryCodeUsed { used_as }`) and shown to the person at their next sign-in; the response says how many codes remain, and says so loudly at two or fewer | not a NIST rule; GitHub and Google both do it |

Twenty-four characters is more typing than GitHub's sixteen. It is chosen
because the lone-code use in §6.4 must resist an offline guess against a
leaked table even if the HMAC key leaks with it, and 120 bits does. It is
open question 1.

### 6.4 One set, two uses

This is where the owner's *"same backup codes"* meets
`plan-passkey-recovery.md` path B and R4. One table, `$recoverycode`, and
one column that says which use happened:

```
"$recoverycode" ( "principal", "hmac", "issued_at", "used_at", "used_as" )   -- used_as: '' | 'second-step' | 'recovery'
```

| | Use A: the second step | Use B: alone (recovery path B, R4) |
|---|---|---|
| Who | a person whose first factor still works and whose second factor is lost | a person whose **only** factor was a passkey and it is gone |
| Where | `POST /auth/second-step {kind:"backup_code"}` after a successful first step | `POST /auth/recover/code`, R4's route, with no session |
| Proves | possession of the paper, in addition to the password or email code | possession of the paper, and nothing else |
| Yields | an ordinary session for that first factor, `second_step: backup-code`. No presence, because the first factor had none | a recovery ticket that can only be redeemed by registering a **new passkey**; every session ended (R1); the export block for a day (recovery §7, owner answer 2); the contest window |
| Then | the person is told to enrol a new second factor; the code is spent | R3 and R4 as written |
| Recorded | `RecoveryCodeUsed { used_as: second-step }`, deployment stream | `RecoveryOpened`, `RecoveryCodeUsed { used_as: recovery }`, and the rest of recovery §8.1 |

What is **not** a use: a person with a password and a second factor who
lost the password. That is a credential reset, `plan-enrolment.md` §4,
and no code opens it.

R4 is amended, not duplicated: its code format is fixed at 24 characters
and 120 bits, its table gains `used_as`, and its done-when gains one test.
`plan-passkey-recovery.md` §3.2 and §10 carry the amendment and point here.

**Issuing codes needs presence (R4) or, for a person with no passkey, a
fresh first factor plus the second factor** (§7.1). Without the second
clause a password-only person could never hold codes, and the second step
would lock them out the first time a phone was lost.

---

## 7 · Managing factors [plan]

### 7.1 What proves the person before a factor changes

Linking a first factor needs presence (`directory.rs:550-586`) and a
password-only person has none. A second factor cannot demand it, or a
person could never enrol their first one. The rule, the same one GitHub's
*sudo mode* applies:

| Act | Proof |
|---|---|
| enrol a factor | a session, plus a **fresh first-factor ceremony** for the same principal within 300 s (a `/auth/complete` of the session's own method, answered with a `reauth` handle instead of a token). Presence in the session satisfies it too |
| remove a factor, change the policy, issue backup codes | the same, **plus a second-step completion** when a factor is enrolled |
| the deployment owner or a tenant administrator, for a managed identity (E6), removes every factor | the reset of `plan-enrolment.md` §4.3, recorded, ending every session (R1) |

The threat the first row does not stop: whoever holds the password before
any second factor exists can enrol one. They already hold the account. What
the row stops is a stolen bearer token enrolling a factor, which is the
attack `directory.rs:5-23` names. Open question 3 is about what a reset
does to factors an attacker enrolled.

### 7.2 Routes

| Route | Does |
|---|---|
| `GET /auth/factors` | the person's factors: id, kind, label, enrolled, last used, disabled; and how many backup codes remain |
| `POST /auth/factors/totp/begin`, `/confirm` | §3 |
| `POST /auth/factors/key/begin`, `/complete` | §4 |
| `POST /auth/factors/{id}/remove` | removes one; ends every other session of the principal (R1) |
| `POST /auth/factors/policy` | `require_second_step` (§5.4) |
| `POST /auth/recovery-codes` | R4's issue route, its gate widened as §6.4's last paragraph says |

Every act is recorded: `SecondFactorEnrolled { kind }`,
`SecondFactorRemoved { kind, by }`, `SecondFactorDisabled { kind }` (the
throttle), `SecondStepPolicyChanged`, appended to `EventKind` after
recovery's tags.

### 7.3 A person made to enrol

When the level that applies says `required` and the person has no factor,
`/auth/complete` answers `second_step` with `"enrol": true`. The handle
authorises `/auth/factors/totp/*` and `/auth/factors/key/*` for that
principal for 300 s, with no token issued until a factor is confirmed and
the second step completed with it. Nobody is signed in without a factor,
and nobody is locked out for lacking one.

---

## 8 · What this plan needs from recovery, and from the plans before it

| Needs | From | Used for |
|---|---|---|
| `$recoverycode`, issue, redeem, void | **R4** | use A of §6.4 is R4's redeem with `used_as = 'second-step'` and no case opened |
| `$recovery`, the ticket, the quarantine, the contest window | **R3** | use B is R3 and R4 unchanged |
| ending every session of a principal | **R1** | factor removal, and the reset in §7.1 |
| the auth routes recording, `authenticators_of`, BE and BS | **S0, S4, R2** | every event in §7.2; listing keys |
| `$passkey`, `PasskeyStore`, `$authceremony`, `CeremonyStore`, the software authenticator | **S1** | §4, §5.1 |
| `RelyingParty` from the environment | **S2** | the second `Preferred` relying party of §4 |
| registration options and the `/auth/register` shape | **S4** | `/auth/factors/key/begin` is the same JSON with `resident_key: "discouraged"` |
| the tenant's admission record and its op | **E4** | the tenant level of §5.4 |
| managed and independent identities, the reset | **E6** | §7.1's third row |
| `via` in the token and `space` at `begin` | **F5** [seam] | §5.3's provider row and §5.4's space-named check. Until F5 lands, every provider sign-in is treated as `password` for §5.3, and the tenant check runs at the door only |
| a provider's two-step claim as presence | **F14** [seam] | §5.3's first cell |

**Why recovery first, in one sentence:** every path a person takes when a
second factor is lost ends in R1's session ending, R3's ticket or R4's
codes, and building the second step before them would build a lock with no
way back.

---

## 9 · Increments [plan]

Each done-when names a test that fails without the increment. Host tests
in **`crates/ordo-host/tests/second_factor.rs`**, new.

| | What | Files | Done when |
|---|---|---|---|
| **T1** | **The half-signed-in state.** `SecondStep::decide`, the `second-step` ceremony row and binder, `/auth/second-step`, the `second_step` token claim (`v4`), the door's `SecondStepRequired` | `crates/ordo-identity/src/{second_step.rs (new), token.rs, access.rs}`, `crates/ordo-host/src/{auth.rs, lib.rs:1481-1498}`, `tests/second_factor.rs` | `second_factor.rs::a_password_sign_in_with_a_factor_enrolled_yields_no_token_until_the_second_step`, `::a_second_step_handle_without_its_binder_is_the_one_answer`, `::a_second_step_begun_on_one_process_completes_on_another_and_only_once`, `::a_v3_token_is_refused_outright` |
| **T2** | **TOTP.** `totp.rs` pure; `$secondfactor` and `PgSecondFactorStore`; sealed secret; replay counter; throttle; enrol begin and confirm | `crates/ordo-identity/src/{totp.rs (new), second_factor.rs (new)}`, `crates/ordo-identity/Cargo.toml` (`sha1`), `crates/ordo-host/src/{second_factors.rs (new), auth.rs, runtime.rs (ensure_table)}` | `ordo-identity::totp_matches_the_rfc_6238_sha1_vectors`, `::hotp_matches_the_rfc_4226_vectors`; `second_factor.rs::a_totp_code_is_accepted_once_and_the_same_code_is_refused_after`, `::five_wrong_codes_end_the_ceremony`, `::twenty_consecutive_failures_disable_the_factor_and_the_stream_says_so`, `::an_unconfirmed_secret_enrols_nothing` |
| **T3** | **Security keys as a second factor.** `$passkey.role`, the second relying party with `Preferred`, `/auth/factors/key/*`, refusal as a first factor, no presence into the token | `crates/ordo-identity/src/passkey.rs`, `crates/ordo-host/src/{passkeys.rs, auth.rs}` | `second_factor.rs::a_second_factor_key_completes_the_second_step`, `::a_second_factor_key_cannot_sign_in_alone`, `::a_password_plus_key_session_cannot_pass_step_up` (the control: the same key registered as a passkey through `/auth/register` can) |
| **T4** | **Backup codes as the second step.** `used_as`, the widened issue gate, the per-principal lock, the remaining count, `RecoveryCodeUsed` | `crates/ordo-host/src/{recovery.rs (R4's), auth.rs}`, `crates/ordo-secevent/src/event.rs` | `second_factor.rs::a_backup_code_completes_the_second_step_once`, `::a_backup_code_at_auth_complete_without_a_first_factor_is_the_one_answer`, `::a_code_spent_as_a_second_step_cannot_open_a_recovery`, `::a_password_only_person_can_issue_codes_with_a_fresh_first_factor_and_a_second_step` |
| **T5** | **Who requires it.** `ORDO_AUTH_SECOND_FACTOR`, the tenant's `second_factor`, the person's policy, the passkey exemption, the door check | `crates/ordo-host/src/{runtime.rs, admission.rs (E4's), auth.rs, lib.rs}` | `second_factor.rs::a_tenant_that_requires_a_second_factor_refuses_a_session_that_skipped_it_at_the_door`, `::a_passkey_session_is_exempt_unless_the_tenant_or_the_person_says_otherwise`, `::a_tenant_cannot_require_what_the_deployment_turned_off`; `tests/runtime.rs::an_unknown_second_factor_word_refuses_the_boot` |
| **T6** | **Managing factors.** `GET /auth/factors`, remove (ending other sessions), policy, the reauthentication handle, the enrol-at-sign-in flow | `crates/ordo-host/src/auth.rs`, `crates/ordo-secevent/src/event.rs` | `second_factor.rs::a_required_person_with_no_factor_is_made_to_enrol_before_a_token_is_minted`, `::enrolling_a_factor_needs_a_fresh_first_factor_and_a_bearer_token_alone_is_refused`, `::removing_a_factor_ends_the_other_sessions` |
| **T7** | **The SDK.** `clients/ordo-sdk/src/auth/second-step.ts`: the second-step loop over `signInWithPassword` and the passkey module | `clients/ordo-sdk/src/auth/{second-step.ts, second-step.test.ts}` (new), `index.ts` | `a_second_step_answer_is_completed_with_a_code`, `a_key_second_step_reuses_the_passkey_assertion_encoding` |

**Order: T1, T2, T4, T3, T5, T6, T7.** T4 before T3 because a person with
TOTP and no codes is one lost phone from a recovery case, and codes are
what the owner asked for first.

---

## 10 · What this plan does not do

- It does not make any second factor a source of presence (§2).
- It does not verify attestation or tell hardware from software (A3).
- It does not send codes by SMS or email. Email code is a first factor
  (F9) and is itself second-stepped.
- It does not add a session table or list live sessions.
- It does not build the tenant's admission record or its op (E4).
- It does not decide what a provider's two-step claim is worth (F14).
- It does not build a console page. Every setting is a route or an op.

---

## 11 · Open for the owner [open]

1. **Backup code length: 24 characters (120 bits), or 16 (80 bits) like
   GitHub?** Recommend **24**. NIST permits a plain approved hash only at
   112 bits and above; below that a salted slow hash is owed, and the lone
   use (§6.4) is the one that must survive a leaked table. Sixteen is
   easier to type and needs argon2id per code, ten verifications per
   attempt.
2. **Default for `ORDO_AUTH_SECOND_FACTOR`: `enabled`.** Recommend
   **enabled**: people may add one, tenants may require one, nobody is
   forced. `required` on a fresh deployment would make the first owner
   enrol a factor before anything else works.
3. **Should a credential reset (E6) or a recovery (R3) remove every second
   factor?** Recommend **yes, recorded.** Otherwise an attacker who held
   the password long enough to enrol a factor keeps the person out after
   the reset, and the authority that reset the credential is already
   trusted to hand the account back.
4. **Should a passkey sign-in ever get a second step?** The owner said
   *"optionally"*. Recommend **off by default, on per person or per tenant**
   (§5.3). A passkey already proves the device and its unlock.
5. **Twenty consecutive failures disable a factor.** NIST allows 100.
   Recommend **20**; a person does not mistype twenty times.

---

### Critical files for implementation

- `crates/ordo-host/src/auth.rs` (the second-step routes, the factor routes, the reauthentication handle)
- `crates/ordo-identity/src/token.rs` (the `second_step` claim, `v4`)
- `crates/ordo-identity/src/passkey.rs` (`role`, the second relying party)
- `crates/ordo-host/src/lib.rs` (the door's `SecondStepRequired` beside the revocation check at 1481-1498)
- `docs/plan-passkey-recovery.md` (R4, amended: §3.2, §10)

Supporting: `crates/ordo-identity/src/totp.rs` (new), `second_step.rs` (new), `second_factor.rs` (new), `crates/ordo-host/src/second_factors.rs` (new), `crates/ordo-secevent/src/event.rs`, `crates/ordo-host/src/runtime.rs`, `clients/ordo-sdk/src/auth/second-step.ts` (new).

---

## Built

T1 to T7 were built in the order T1, T2, T4, T3, T5, T6, T7, one commit each,
on a branch of their own. Where the plan turned out wrong against the code,
the entry says so.

### T1 · The half-signed-in state

- **The rule.** `crates/ordo-identity/src/second_step.rs`: `SecondStepKind`,
  the three levels (`DeploymentMode`, `TenantMode`, `PersonMode`) and
  `decide`, a pure function from the levels, the first factor and the
  factors held to `Owes::{Nothing, Step, Enrol}`. `off` at the deployment ends
  it. A passkey owes a step only when a level says so for passkeys. Any other
  first factor owes one when the person holds a factor, and an enrolment when
  a level requires one and they hold none.
- **The gate.** `crates/ordo-host/src/auth/second_step.rs`. In
  `/auth/complete`, after the first factor and the deployment's checks and
  before the `Authenticated` record and the mint, `second_step::gate` asks
  `SecondFactors::owes`. When a step is owed no token is minted: a
  `second-step` row goes in `$authceremony` (connection `second-step`, phase
  `pending`, handle and binder stored as hashes, payload the principal, the
  first method, the first step's presence, the space and admission fields) and
  the answer is `{second_step, binder, methods | enrol, expires_at}`. A store
  that cannot be read fails closed, a `503`, never a token.
- **The route.** `POST /auth/second-step {handle, binder, kind, ...}` takes
  the row (a delete that returns, so one replica wins), checks the binder
  (a wrong binder puts the row back and costs no attempt), asks the method,
  and only an accepted answer mints. A wrong answer puts the row back with one
  fewer attempt and the last one ends the ceremony. `POST /auth/second-step/begin`
  is the challenge for a method that has one (T3).
- **The token.** The claim lives on `SessionClaim.second_step`, not on
  `TokenSpec` (see below). Encoding `v3` moved to `v4`; a `v3` token is
  refused outright, as is an unknown second-step word.
- **Methods plug in.** `crates/ordo-host/src/second_factors.rs`:
  `SecondStepMethod` (`holds`, `begin`, `verify`), `SecondFactors` and
  `SecondFactorConfig`. The route does not know how any method checks.
  `AuthState::wire_second_factors` is once-set like `wire_recovery`.
- **Tests.** `tests/second_factor.rs`:
  `a_password_sign_in_with_a_factor_enrolled_yields_no_token_until_the_second_step`,
  `a_second_step_handle_without_its_binder_is_the_one_answer`,
  `a_second_step_begun_on_one_process_completes_on_another_and_only_once`
  (two replicas over one Postgres ceremony table, one handle answered on both
  at once), `a_v3_token_is_refused_outright`; `ordo-identity`
  `token::tests::the_second_step_claim_round_trips_and_a_v3_token_is_refused_outright`
  and the `second_step` unit tests. With the gate switched off the four host
  tests fail; with the token decoder accepting `v3` the two version tests fail.

**The plan was wrong** in one place. It puts `second_step` on `TokenSpec`.
`TokenSpec` and `TokenClaims` are built as struct literals in 95 places, most
of them tests, so a new field on either would break every one and every
branch open beside this one. The claim is about the sign-in, so it sits on
`SessionClaim`, which has three literals outside the crate. A benefit: `refresh`
and `refresh_presence` carry the whole session claim, so they cannot drop it.
The door reads it as `claims.session.second_step`.

`/auth/second-step` mints in its own module and does not run what
`/auth/complete` runs after its own mint (its answer's notices, and any hook
that runs after a session is minted). It builds the parts a sign-in with a
second step needs: the token, `recovery_codes_remaining`, `presence_available_at`
and the tenant `admission` outcome. A person who is signing up holds no factor,
so the sign-up hooks are not reached by a second step, except where a level
requires one of a person who has none (the enrol flow, T6).

### T2 · Code-generating tokens (TOTP)

- **The code.** `crates/ordo-identity/src/totp.rs`: HMAC-SHA1 written in-house
  over `sha1` (the one new dependency, one lockfile entry), HOTP, TOTP, base32
  (RFC 4648, no padding), the `otpauth://` URI, and `matches`, which compares
  every step inside the skew in constant time and returns the step it matched.
  Six digits, thirty-second steps and one step of skew are constants of the
  protocol, since no authenticator app would follow another value.
- **The store.** `crates/ordo-host/src/second_factor_store.rs`:
  `$secondfactor` in the plan's shape, `SecondFactorStore` (memory and
  Postgres), and the counters as single statements: `accept_counter` accepts a
  step only if it is later than the last one, `note_failure` adds one and locks
  when the limit is reached. `ensure_table` is called at boot beside
  `$recoverycode`.
- **The method.** `TotpMethod` in `second_factors.rs`. The secret is sealed,
  never hashed. Enrolment is two steps: `POST /auth/factors/totp/begin` seals a
  fresh secret into a `totp-enrol` ceremony row that expires and answers the URI
  and the secret once, `no-store`; `POST /auth/factors/totp/confirm` takes the
  row and writes the factor only for a right code, storing the step the code
  proved so that the same code cannot then sign in. Both are in
  `crates/ordo-host/src/auth/factors.rs`, whose routes are merged by the one
  `merge` line in each router.
- **Layers.** The five wrong answers a ceremony allows
  (`ORDO_AUTH_SECOND_STEP_ATTEMPTS`), the twenty that lock a factor
  (`ORDO_AUTH_SECOND_FACTOR_LOCK_AFTER`, owner 2026-09-29), how long a ceremony
  lasts and how many factors a person may hold are `SecondFactorConfig`
  fields, the deployment's defaults. The environment reading is T5.
- **The record.** `EventKind::SecondFactor { act, detail }`, chain tag 18, one
  variant for the family as recovery has. T2 writes `enrolled` and `disabled`.
- **Tests.** `ordo-identity`: `totp_matches_the_rfc_6238_sha1_vectors`,
  `hotp_matches_the_rfc_4226_vectors`. `tests/second_factor.rs`:
  `a_totp_code_is_accepted_once_and_the_same_code_is_refused_after`,
  `five_wrong_codes_end_the_ceremony`,
  `twenty_consecutive_failures_disable_the_factor_and_the_stream_says_so`
  (run at the default of 20 and at a deployment's 3),
  `an_unconfirmed_secret_enrols_nothing`. Seen failing: with a byte of the HMAC
  pad changed both vector tests fail; with the replay check taken out of both
  the matcher and the store the accepted-once test fails (either one alone
  leaves it passing, which is the two layers doing the same job); with the
  attempt count not ending the ceremony, with the failure counting skipped, with
  a locked factor not counted as held, and with the confirmation writing the
  factor regardless of the code, each of the other tests fails.

**The plan was wrong** in one place. It says sealing "under the deployment's
`KeyProvider` with a purpose string exists". `Purpose` is a closed set (`read`,
`write`, `reseal`) and every release names a space, so there is no purpose to
name. The secret is sealed in the deployment's own space,
`provision::SYSTEM_SPACE`, at the row of its factor (`Anchor::Row(<factor id>)`),
which opens nowhere else.

**A design point the plan left open, closed here.** A locked factor still
counts as held. Otherwise twenty wrong codes would turn a person with a second
step into one without, and their password alone would sign in. A locked factor
refuses every answer, the person's backup codes are still offered, and the
factor is cleared by removing it and enrolling again (T6).

### T4 · Backup codes as the second step

The same set recovery issues (`recovery_codes.rs`, `used_as` already in the
table) stands in for a lost second factor. Alone a code still opens a recovery
and nothing else; it is not a way in.

- **The method.** `BackupCodeMethod` (`second_factors.rs`) is offered beside a
  person's factor when they hold unused codes, never instead of one. It spends
  a code as **that person's**: `CodeStore::consume_for` is one statement that
  requires the code to be theirs, so another person's code is neither accepted
  nor consumed. It marks the code `used_as = 'second-step'`, so recovery refuses
  it (`/auth/recover/code` only finds unused codes).
- **The lock.** Wrong codes are counted per person on a lock row in
  `$secondfactor` (kind `backup-code`, never listed as a factor), by the same
  `note_failure` the code-generating factor uses. At the limit
  (`ORDO_AUTH_SECOND_FACTOR_LOCK_AFTER`, 20 by default) the person's code
  redemption is locked and `EventKind::SecondFactor disabled` is recorded. A
  fresh set, issued through `POST /auth/recovery-codes`, clears it.
- **The record and the notice.** A code spent as a second step is written as
  `recovery code-used ... used_as=second-step remaining=N` in the deployment's
  stream, and the answer says `recovery_codes_remaining` and
  `recovery_codes_low` (true at two or fewer).
- **The widened issue gate, and the fresh first factor it needs.** Issuing
  codes needs presence, as R4 built it, or, for a person with no passkey, a
  fresh first factor plus the second step when they hold a factor. The plan
  puts the fresh-first-factor mechanism in T6; T4's done-when needs it, so it is
  built here and T6 reuses it. `POST /auth/complete {"reauth": true, ...}` with
  the session's bearer runs the first factor again. It is answered a `reauth`
  handle, or, when the person holds a factor, a second step whose completion
  answers the handle. **It never answers a token**, and a deployment with no
  second step wired refuses it rather than letting it fall through to a mint.
  The handle is a ceremony row (`reauth`), one-shot, good for
  `ORDO_AUTH_REAUTH_SECS` (300), and is sent in the `x-reauth` header. It is
  bound to the person who proved it. `factors::prove_change` is the one place
  the rule lives.
- **Kinds.** The plan's spellings `backup_code` and `passkey_assertion` are
  accepted at the second-step routes beside the words the token uses
  (`backup-code`, `security-key`).
- **Tests.** `tests/second_factor.rs`:
  `a_backup_code_completes_the_second_step_once`,
  `a_backup_code_at_auth_complete_without_a_first_factor_is_the_one_answer`,
  `a_code_spent_as_a_second_step_cannot_open_a_recovery`,
  `a_password_only_person_can_issue_codes_with_a_fresh_first_factor_and_a_second_step`,
  `wrong_backup_codes_lock_after_the_limit_and_a_fresh_set_clears_it`;
  `recovery_codes::tests::a_second_step_spends_only_the_owners_code`. Seen
  failing with each of these taken out: the owner check on the code, the
  reauth branch of the issue gate, the reauth ending in a handle rather than a
  token, the lock clearing on a fresh set, the `second-step` mark on the code,
  and the method offering codes.

Not built: the plan says a redemption is "shown to the person at their next
sign-in". The record and the answer to the sign-in that spent the code carry
it; a notice carried on every later sign-in until acknowledged is recovery's
mechanism for cases (`recoveries`), and a code spent as a second step opens no
case for it to hang from. Left for the owner to ask for.

### T3 · Security keys as a second factor

A WebAuthn credential registered as a second step, verified by the code that
verifies a passkey, and told apart from one by a role.

- **The role.** `$passkey.role` (`passkey` or `second-factor`), a column added
  with `ADD COLUMN IF NOT EXISTS` and a default that makes every existing row a
  first-factor passkey, which is what it was. `StoredPasskey.role`,
  `ordo_identity::KeyRole`. `verify_assertion` refuses a credential whose role
  is not the one the ceremony asks for, as the one `BadCredentials`: a
  second-factor key cannot sign in or step up, and a first-factor passkey
  cannot answer a second step.
- **The second relying party is a parameter, not a value.** The plan says
  "a second `RelyingParty` value with `Preferred` UV". `RelyingParty` also
  holds the rp id and origins, which are the same for both, so a second value
  would be a copy that could drift. `PasskeyProvider` instead holds
  `second_factor_uv` (`Preferred`, settable with `with_second_factor_uv`) and
  asks `uv_for(purpose)`. A key with no PIN is accepted, and the enrolment
  record says `user_verified=no`.
- **Its own ceremonies.** `begin_second_factor_registration`,
  `verify_second_factor_registration` (phase `register-second-factor`, bound to
  the person who began it), `begin_second_step` and `complete_second_step`
  (phase `second-step`, narrowed to the person's own keys). A first-factor
  registration challenge cannot finish a second-factor registration and the
  other way round.
- **No presence, by type.** A second-factor registration's subject carries
  `presence: None`, and `complete_second_step` returns a `SecondStepProof`
  (principal, authenticator, whether the key verified the user) that has no
  presence in it to carry. The token a completed second step mints takes its
  presence from the first factor.
- **Linked in `$secondfactor` only** (`secret` = the authenticator id), never in
  `$authenticator`, so `link_count`, the last-authenticator rule and
  `resolve_or_onboard` never see it. `SecurityKeyMethod` (`second_factors.rs`)
  holds `begin`, which mints the challenge narrowed to the person's unlocked
  keys, and `verify`, which counts a wrong answer against the key it named or,
  when it named none of theirs, against each key they hold.
- **Routes.** `POST /auth/factors/key/begin` answers passkey registration
  options with `resident_key: discouraged`, `user_verification: preferred` and
  `attestation: none`; `/auth/factors/key/complete` stores the credential with
  its role and writes the factor. `POST /auth/second-step/begin` (T1's route)
  answers the assertion options.
- **Recorded as a security key.** `SecondFactor enrolled kind=security-key
  user_verified=...` and nothing about a model or hardware (owner, 2026-09-29:
  hardware is offered without proof).
- **Tests.** `tests/second_factor.rs`: `a_second_factor_key_completes_the_second_step`,
  `a_second_factor_key_cannot_sign_in_alone`,
  `a_password_plus_key_session_cannot_pass_step_up` (its control is a key
  registered as a passkey through `/auth/register`, which passes the same door);
  `ordo-identity`: `passkey_tests::a_second_factor_key_holds_a_role_and_proves_no_presence`.
  Seen failing with the role check taken out (the sign-in test), with the token
  taking `Some(now)` as presence (the step-up test and three others), and with
  the second factor's user verification `Required` (the completes-the-step test).

### T5 · Who requires it

Three levels, tightening only. Ordo holds the mechanism and the app developer
decides what is enforced.

| Level | Where it is set | Where it is read |
|---|---|---|
| **app developer** (the deployment) | `ORDO_AUTH_SECOND_FACTOR`: `off`, `enabled` (default), `required`, `required-for-non-passkey`. `SecondFactorConfig::from_env`, refused by name when unknown | the sign-in gate (`Owes`), the door (`SecondStepLevels::refuses`), and the ceiling on a tenant's choice |
| **tenant** | `secondFactor` on `std.space.setAdmission` (`inherit`, `required`, `required-for-non-passkey`), stored on `$admissionpolicy.second_factor` | the gate, when the sign-in names the tenant, and the door on every op |
| **person** | `$secondfactorpolicy` (`when-enrolled`, `also-for-passkeys`); the store has it and T6 adds the route | the gate |

- **The rule.** `ordo_identity::second_step::decide` (T1). `TenantMode::within`
  is the ceiling: a tenant cannot require what the deployment turned `off`, and
  cannot loosen `required` to passkey-exempt. It is checked when an owner sets
  the level and the answer names the deployment's setting.
- **The door.** `SecondStepLevels` (`second_factors.rs`) is one object holding
  the deployment's mode and each tenant's level, shared by the sign-in gate, the
  admission op and the door. `IdentityAccess::check`, beside the revocation
  check, refuses a human session with no `second_step` claim where the
  deployment or the tenant requires one, recorded as the new
  `RecordedDecision::SecondStepRequired` (chain tag 30). A capability or a
  delegated token is not a session and is not asked. The client signs in again
  naming the tenant. A level an owner sets is live at once on the replica that
  took the call, and the worker `second-step.levels.refresh` (the door's refresh period,
  `ORDO_ACCESS_REFRESH_SECS`, 30 s by default) tells the others. `SwappableAccess` reinstalls the object on every rebuilt door.
- **The passkey exemption at the door is presence in the token**, until the
  token names how it signed in (F5). A session with presence signed in with a
  passkey. The one wrong reading is a quarantined passkey's session, which has
  none and is refused under `required-for-non-passkey`, the safe direction.
- **The environment.** `ORDO_AUTH_SECOND_FACTOR` and five companions
  (`docs/deploying.md`), each floor-checked, each refusing the boot by name.
  `RuntimeConfig.second_factor` is a new field on every struct literal.
- **Tests.** `tests/second_factor.rs`:
  `a_tenant_that_requires_a_second_factor_refuses_a_session_that_skipped_it_at_the_door`,
  `a_passkey_session_is_exempt_unless_the_tenant_or_the_person_says_otherwise`,
  `a_tenant_cannot_require_what_the_deployment_turned_off` (through the op, over
  Postgres); `tests/runtime.rs`:
  `an_unknown_second_factor_word_refuses_the_boot` and
  `a_booted_deployment_serves_the_second_step_routes_and_has_wired_them`, which
  boots the binary and calls the routes, so an unwired or unrouted surface fails
  it. Seen failing with the door check taken out (two tests), with the passkey
  exemption taken out, with the ceiling taken out, with the tenant level not
  read at sign-in, with the person's level not read, and with an unknown word
  read as the default.

**The plan was wrong** in three places. It puts the door check in
`ordo-identity`'s `access.rs` as a `DenyReason`. The requirement is host state
that changes while the process runs, and the engine is rebuilt from `members`
rows, so the check is the door's (`lib.rs`) and the reason is a
`RecordedDecision`. It puts a person's level on `$principal`. That table is the
directory's and another branch is editing its neighbours, so it is its own
small table beside the factors. It puts the tenant's level on E4's `$admission`;
the table is `$admissionpolicy`.

**A provider's two-step claim** (§5.3, owner 2026-10-09, federated ask 11)
stands in for the step where it is accepted. `SecondStepLevels` holds the
deployment's `UpstreamAcceptance`. At the gate, a claim from a connection set
`on` owes nothing. A claim the named tenant accepts reads that tenant's level as
`inherit`, so the deployment's level and the person's own still ask. A reauth
never skips. The door applies the same rule per space. Recorded as
`upstream-claim` act `second-step-satisfied`. Tests in `tests/auth_oidc.rs`.

The limits (`ATTEMPTS`, `LOCK_AFTER`, the ceremony and reauthentication lifetimes,
the factor cap) have the deployment layer only. A tenant's choice and a person's
own are not built for them, because a factor is the deployment's credential and
what locks it has no natural tenant owner. Left for the owner to ask for.

### T6 · Managing factors

- **The routes** (`crates/ordo-host/src/auth/factors.rs`, merged by the one
  `merge` line in each router). `GET /auth/factors`, `POST
  /auth/factors/{id}/remove`, `POST /auth/factors/policy`, beside the enrolment
  routes T2 and T3 built.
- **What proves the person.** The reauth mechanism T4 needed is what T6 applies.
  Starting an enrolment, removing a factor and changing the level each need a
  fresh first factor: a passkey assertion inside the step-up window (presence
  stands for it, as it does for issuing codes), or a reauth handle with the
  second step behind it when the person holds a factor. Finishing an enrolment
  needs only the bearer: it is bound to the row the start made, which is bound
  to the person, so another person's session can neither finish it nor spend it.
  A bearer alone is `403 reauthentication_required`, and the answer says what to
  do.
- **Removing a factor ends every session the person has**, and hands the person
  who proved themselves a replacement token whose session starts a second later,
  which the epoch (`end_all`) does not reach. The plan says "ends every other
  session"; the epoch cannot spare one session, so the replacement is how this
  one survives. A key's credential is removed with its factor. A factor that is
  not theirs is the one refusal and ends nothing. Allowed when the deployment has
  turned the second step off, so a person can clean up.
- **A person made to enrol** (`plan-second-factor.md` §7.3). A sign-in that a
  level requires a second step of, by a person who holds none, answers `enrol:
  true` and no token. The handle and binder authorise `/auth/factors/totp/*` and
  `/auth/factors/key/*` for that person and nothing else, checked without
  consuming the state (`second_step::peek_enrolment`), and only a state that
  owes an enrolment does. The second step is then completed with the enrolled
  factor. The code that proved the enrolment is spent, so the person enters the
  next one.
- **A reset or a recovery removes every second factor once it completes**
  (owner, 2026-09-29: *"but only after the reset"*). The plan had no increment
  for this, and recovery's R3 left a `Settlement` for it. `RemoveSecondFactors`
  is registered on recovery, so it runs when a case completes and not when it is
  opened or cancelled. A credential reset completes when the person redeems its
  password ticket: `CredentialOps::redeem_password` runs `after_redeem`, a hook
  the runtime fills once the second step's store exists (a shared handle, as
  recovery's is). Both record `SecondFactor removed` with `by=recovery` or
  `by=credential-reset`.
- **Tests.** `tests/second_factor.rs`:
  `a_required_person_with_no_factor_is_made_to_enrol_before_a_token_is_minted`,
  `enrolling_a_factor_needs_a_fresh_first_factor_and_a_bearer_token_alone_is_refused`,
  `removing_a_factor_ends_the_other_sessions`,
  `a_person_lists_their_factors_and_sets_their_own_level`,
  `a_recovery_removes_every_second_factor_only_once_it_completes`;
  `tests/credential_reset.rs`:
  `a_reset_removes_every_second_factor_only_when_the_person_redeems_it`;
  `tests/runtime.rs`: the booted-deployment test now enrols with a session that
  has presence, lists, and shows a session without it told to reauthenticate.
  Seen failing with each of these taken out: the proof an enrolment needs, the
  proof a removal needs, the sessions ending, the recovery settlement's removal,
  the reset hook, and the check that a handle is for an enrolment.

**A locked factor and removal.** A locked factor counts as held, so a person
whose only factor is locked cannot complete the second step that removing it
asks for, except with a backup code. With no code either, they recover through
the deployment owner or the tenant, which removes every factor when it
completes. That is the path the owner described, and it is worth knowing that
nothing lets a locked-out person remove their own factor with a password alone.

### T7 · The SDK

`clients/ordo-sdk/src/auth/second-step.ts`, exported from `index.ts`, with
`test/second-step.test.ts`.

- **The loop.** `signInWithPassword` and `signInWithPasskeyStep` answer a
  `SignInResult`: a session, or a `SecondStep` (handle, binder, methods,
  whether the person must enrol). No token from the door is a second step and
  not a failure. `completeSecondStep` answers it: a code is one request, a
  security key is two (the door's challenge, then the assertion), and the answer
  is the session, or a proof when the step was for a change to a factor.
- **The key reuses the passkey module.** The challenge is decoded to the bytes
  the key signs, `clientDataJSON` is posted as the exact bytes the browser
  produced, every byte string is base64url without padding, and all of it is
  `assertion()` from `passkey.ts`, not a second copy.
- **Changing factors.** `reauthenticateWithPassword` and
  `reauthenticateWithPasskey` (never a session), `listFactors`, `removeFactor`
  (answers the replacement session), `setSecondStepLevel`, and
  `beginTotpEnrolment`, `confirmTotpEnrolment`, `enrolSecurityKey`, each taking
  an `Enroller`: a signed-in person and, to start, a proof, or the `SecondStep` of
  a person a level made enrol.
- **In `passkey.ts`**, three additive changes: `session` is exported, `call`
  takes optional extra headers (the `x-reauth` proof), and a `403
  reauthentication_required` is told apart as its own `PasskeyFailure`, the one
  refusal the door explains. Everything else still reads as `refused`.
- **The plan was wrong** that the SDK has a `signInWithPassword` to loop over.
  It did not; the passkey module was the only sign-in. It is written here.
- **Tests.** `a_second_step_answer_is_completed_with_a_code`,
  `a_key_second_step_reuses_the_passkey_assertion_encoding`, and ten more:
  a backup code, a passkey sign-in that is and is not stepped, a person made to
  enrol, the one refusal, a reauthentication that never yields a session and one
  that ends in a proof, the proof in `x-reauth`, the list and the level, and
  enrolment as a signed-in person and as one made to enrol. Seen failing with the
  binder left out, with a tokenless answer read as a failure, and with the key
  answered under another kind. `npm run check`: typecheck clean, 126 tests in 8
  files pass.
