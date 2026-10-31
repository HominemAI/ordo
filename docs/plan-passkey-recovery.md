# Passkey recovery: the way back in when the key is gone

This plan reflects the owner's answers of 2026-09-29 (`corpus-rulings.md`, *Passkey recovery*, all five). "Deployment owner" is the person who runs the deployment; the host-side command is what that person runs.

Companion to `plan-passkey-signin.md` (S0 to S7, built before this; the durable stores and the recording this plan depends on), `plan-second-factor.md` (built after this; shares the backup codes of §3.2 and R4), `plan-enrolment.md` (tickets, managed and independent identities, the membership commands), `plan-auth-federated.md` (SAML, OAuth2, email code), `security-logging-design.md` §2, §3, §6.1 and the section *Linking an authenticator is a dual-proof ceremony*, `f7-vault-plan.md` §3.3, and checklist 20b, 21b, 5g and 18h.

Markers: **[settled]** ruled by the owner or restated from a settled document · **[plan]** proposed here · **[open]** named and not answered · **[seam]** depends on work another plan owns.

Status: **planned; R1 to R8 built, see the final "Built" section (R9 is not scheduled).** Every claim about the code was read against the tree at `db1192b` on 2026-09-29. `docs/plan-enrolment.md` was present and untracked at that time and is cited as a draft. Where a section says nothing does X, §0 says what was searched.

---

## 0 · The starting state, verified

Every increment below is a delta on these facts.

### 0.1 Passkey verification is built, tested, and cannot be served

`crates/ordo-identity/src/passkey.rs` (667 lines) and `webauthn.rs` (691 lines) verify a ceremony. `auth_from_env` refuses any method other than `password` (`crates/ordo-host/src/runtime.rs:367-385`), with the text "passkey needs the browser ceremony and a relying-party id". `ORDO_APEX_DOMAIN` is read and recorded and nothing acts on it (`runtime.rs:704-710`). Serving it is `plan-passkey-signin.md`, built before this plan.

`PasskeyProvider` has no caller outside `ordo-identity`. Searched: `grep -rn "PasskeyProvider" crates --include='*.rs'`, which returns only `ordo-identity/src/{passkey,provider,lib}.rs` and tests.

### 0.2 Enrolled passkey public keys live in process memory

`PasskeyProvider.credentials` is a `Mutex<BTreeMap<Vec<u8>, PasskeyCredential>>` (`passkey.rs:370-375`), and the doc above it says production swaps it for a store. No such store exists. The directory's link (`$authenticator`, `crates/ordo-host/src/principal_directory.rs:82-87`) is durable; the key that verifies the assertion is not. **This matters to recovery:** revoking a lost passkey has to remove two things, the link and the public key, and only one of them has a table today. **[seam]** The durable passkey credential store is `$passkey` and `PasskeyStore`, `plan-passkey-signin.md` §2.4 and S1. R2's unlink deletes the `$passkey` row through it, and §7's `presence_not_before` is a column of that row.

### 0.3 The directory refuses the act recovery needs, on purpose

- `Directory::unlink` refuses to remove a human's last authenticator: `DirectoryError::LastAuthenticator` (`directory.rs:89-91`, enforced at `directory.rs:656-661`).
- `Directory::link` requires a `LinkCeremony`, which needs fresh presence from an authenticator the principal already holds (`directory.rs:550-586`).
- `Directory::link_first` refuses any principal that already has a link (`directory.rs:600-612`).

So a person whose only passkey is gone has no path: they cannot vouch, they are past bootstrap, and the last link cannot be removed. That is correct. The module doc says recovery "is a separate, delayed and notified path, never this one" (`directory.rs:27-30`), and `security-logging-design.md:511-514` says "recovery must never share a code path with linking" **[settled]**.

### 0.4 No route links, unlinks, or steps up

`Directory::link`, `Directory::unlink`, `LinkCeremony::from_claims` and `session::refresh_presence` have no caller in `crates/ordo-host/src`. Searched: `grep -rn "\.link(\|\.unlink(\|link_first(\|refresh_presence(\|LinkCeremony::from_claims" crates --include='*.rs'`; outside `ordo-identity` the only hit is `link_first` in `crates/ordo-host/tests/auth_durability.rs:329`. The auth router has four routes: begin, complete, logout, refresh (`crates/ordo-host/src/auth.rs:533-538`).

### 0.5 The audit events a sign-in returns are dropped

`sign_in` returns `SignIn.audit` (`session.rs:385-391`). `/auth/complete` calls it and never reads that field (`auth.rs:280-296`). Searched: `grep -rn "\.audit\b" crates/ordo-host/src` returns nothing. The conversion to a stream entry exists (`crates/ordo-secevent/src/event.rs:524-545`) and has no producer in the host. A recovery record that depended on the same plumbing would be silent too, so this is fixed first (R2).

### 0.6 Presence comes only from a passkey, and every platform op demands it

`UserPresence` has no public constructor (`passkey.rs:59-72`). Every spec in `platform_ops::specs()` is step-up gated by one line (`crates/ordo-host/src/lib.rs:1889`), and export, import, delete, attestTo, attachBucket and deployment apply are owner-only (`platform_ops.rs:877-902`). The window is 300 seconds (`access.rs:32`), decided in `step_up_decision` (`access.rs:1004-1037`) from `claims.user_presence_at` and nothing else.

Presence enters a token at exactly two points: `SignIn::token_spec` (`session.rs:407-428`) and `refresh_presence` (`session.rs:538-579`). §7 uses this.

### 0.7 Sessions: a deny list per session, and nothing per person

`SessionRevocations` answers `is_revoked(session_id)` from memory (`session.rs:180-191`); the door asks on every request (`crates/ordo-host/src/lib.rs:1481-1498`). `$sessionrevocation` has no principal column, by design (`crates/ordo-host/src/sessions.rs:59-76`). "Sign out everywhere" is not built, and the module says what it wants: "a per-principal epoch, which is the same deny-list mechanism keyed on the principal" (`session.rs:60-64`). Searched: `grep -rniE "sign out everywhere|revoke_all|logout_all|revoke_principal" crates`; the only hit is that doc comment.

The token already carries what an epoch check needs: the principal and the session's `started_at`, both under the MAC (`crates/ordo-identity/src/token.rs:221-224`). No token format change is required.

### 0.8 Membership is per space and refreshed every 30 seconds

Suspension is an authorization input checked ahead of everything (`access.rs:667`). The door re-reads members on `ORDO_ACCESS_REFRESH_SECS`, default 30 (`crates/ordo-host/src/workers.rs:1100-1125`). `plan-enrolment.md` §0 records that the door keys memberships by `members.id` and never reads `members.principal`, so one principal in two tenants does not work yet. **[seam]** That is enrolment's E1 and this plan depends on it.

### 0.9 The security stream has two scopes and a frozen encoding

`Scope::Space` and `Scope::Deployment` (`event.rs:39-48`). `EventKind` has 15 variants with tags 0 to 14 (`chain.rs:243-288`); the layout is "frozen and unversioned" (`chain.rs:41`). New kinds are appended tags, which is how `SchemaMigration` (tag 14) arrived. A space's owner reads the space's stream through `/security/events`, owner-only (`lib.rs:1790-1815`, `serve.rs:104`). The deployment owner does not thereby read tenant streams (`lib.rs:1802-1808`).

### 0.10 Custody: the deployment holds the keys today; vault mode is planned

`CustodyChoice` is `Local`, `SealedMaster` or `Test` (`runtime.rs:72-87`). In all three the deployment's `KeyProvider` opens sealed values for an authorised member, so a lost passkey loses access and loses no data. Vault mode is unbuilt. Its device-loss rule is settled: a user-held recovery phrase by default, platform escrow as a disclosed opt-in, and "no silent escrow, ever" (`f7-vault-plan.md` §3.3, lines 207-219).

### 0.11 There is no mail and no contact address

Searched: `grep -rniE "smtp|lettre|sendmail|mailer|send_mail|send_email" crates Cargo.toml` returns nothing. `Principal` is `id`, `kind`, `display`, `created` (`principal.rs:110-115`); there is no email or phone field. `AuthMethodKind::EmailOtp` is an enum variant with no implementation (`principal.rs:485`). `ordo-egress` is an outbound HTTP client whose doc names notification sinks as a future consumer (`crates/ordo-egress/src/lib.rs:14-18`).

### 0.12 Nothing about recovery exists

Searched: `grep -rniE "recover" crates --include='*.rs' -l`. Hits in `ordo-identity` are the two doc comments cited in 0.3 and an unrelated comment in `access.rs`; every other hit concerns key custody, restore, or error recovery. No type, table, route or op for credential recovery exists.

---

## 1 · The owner's rulings, as they bind this plan [settled]

Recorded verbatim in `plan-enrolment.md` §1 (2026-09-29) and `decisions.json` (`durable-auth`).

| | Ruling | What it forces here |
|---|---|---|
| **K1** | "Service owns the credentialing." | Recovery state lives in deployment tables. A tenant never stores a recovery secret |
| **K2** | No enforced flow | Every preventive measure (second passkey, recovery codes) is offered and skippable. No single recovery ritual is required of everybody |
| **K3** | Multi-tenant users need "control or management from the system above" | A tenant administrator cannot reset a credential that unlocks another tenant (§5) |
| **K4** | The tenant has power over a credential it created, logged; sovereignty may need to sit at the service level | Tenant-administered recovery exists for managed identities only, and is recorded |
| **K5** | Passkey is the better option | The only thing a recovery can produce is a new passkey. No path ends in a password |
| **K6** | "Configurable to some degree" | The deployment sets a floor; a tenant may tighten within it (§4.4) |

**The owner's answers of 2026-09-29** to this plan's five asks, recorded in `corpus-rulings.md`:

| | Answer | Where it lands |
|---|---|---|
| **K7** | **A recovery code works at once.** | §3.2, §3.6; question 1 answered |
| **K8** | **A day's block on export after recovery, adjustable.** | §4.4 and §7: one default of 24 hours for every path but A, adjustable; the 72 hours proposed for D and E is withdrawn. Question 2 answered |
| **K9** | **The deployment owner restores a tenant owner who has lost their only passkey, and nobody inside the tenant does.** *"Yes the deployment owner."* | §6, confirmed. The board ask was this, and not question 3 of §11, which stays open |
| **K10** | **A recovered person is not locked out of each tenant.** | §5.3: hold on recovery stays off by default; question 4 answered |
| **K11** | **Email alone is not enough by default**, and the option is documented well, because *"someone may want it"* | §3.5, §4.4; question 7 answered. The deployment owner guide gets a section that says what turning it on means |

Two answers on other tasks reach in: backup codes are shared with the second factor (`second-factor`, answer 5, in §3.2), and registration is kept for a set time then swept (`federated-auth-questions`, answer 3, in `plan-enrolment.md` E4), which is why R8's reconciliation must not report a swept principal's missing link.

---

## 2 · Threat model

Recovery proves "I am this person and I lost my key" with something weaker than the key. Each row says what the actor has, what they try, and which mechanism in this plan answers it. "Residual" is what remains true after the plan is built.

| Actor | Has | Attempts | Answer | Residual |
|---|---|---|---|---|
| **T1 · Attacker with the victim's email** | the mailbox | recovery through a mail link | No mail path exists, and when one does it is off by default and can only open a delayed case (§3, path E) | A deployment that opts in accepts that a mailbox compromise becomes an account takeover after the delay, if nobody contests |
| **T2 · Malicious or coerced tenant owner** | owner role and presence in tenant A | reset a member's credential to act as them, in A or in tenant B | May reset managed identities only, which by definition exist in A alone (§5). The act is recorded in a stream a domain revert cannot rewrite, and the person sees it at their next sign-in | Inside A the owner can impersonate a managed member after a recovery. The owner could already read and export A's data; what they gain is another person's name on actions, and the record shows the recovery that preceded them |
| **T3 · Malicious deployment owner with database access** | write access to `$authenticator`, `$principal` | insert a link row for their own passkey | No recovery design prevents this; whoever can write the directory table can mint a credential. The answer is detection: R8 reconciles every link row against the signed stream, and a link with no event is reported | A deployment owner who also holds the stream signing key can forge the record. Separating those two is `plan-production-ops.md`'s subject |
| **T4 · Support-desk social engineering** | a plausible story | persuade the deployment owner to recover somebody's account | Deployment-owner recovery has a mandatory delay, a contest window in which any existing passkey of the victim cancels it, a quarantine on step-up afterwards, and an optional second approver (§3, path D) | If the victim truly holds no working passkey and is not reachable, nobody contests. The delay and the quarantine bound the damage to ordinary reads |
| **T5 · Stolen unlocked device** | a live session, and perhaps the passkey behind the device's unlock | use the session; add their own passkey; generate recovery codes | Linking and code generation require fresh presence with user verification (`UserVerification::Required` is the default, `passkey.rs:97-100`). The owner signs in elsewhere, unlinks that device's passkey, and ends every session (R1, R2) | Until the owner acts, the session works for ordinary ops up to the 12-hour ceiling |
| **T6 · Departing employee** | a passkey, possibly recovery codes, possibly admin rights until the last day | keep access; open recovery cases for colleagues before leaving | Removing the membership ends access to the tenant whatever credentials remain. Recovery codes restore a credential and no membership. Cases they opened are recorded, delayed, and contestable | An independent identity keeps existing after the person leaves a tenant. That is correct: the identity is theirs and the membership was the tenant's |
| **T7 · Person locked out of the last owner account** | nothing that signs | regain the tenant | §6. The deployment owner restores, with evidence, delay and record | If the deployment owner is unreachable or the deployment has none, the tenant stays locked. Data is intact in managed custody |
| **T8 · Attacker who finds printed recovery codes** | one or more codes | redeem one | Redemption ends sessions and starts the contest window; the real owner's passkeys still work for one purpose, cancelling the recovery (§4.2). Step-up is quarantined (§7) | Ordinary reads in the interval between redemption and contest |

Two attacks are out of scope because other documents own them: a compromised platform keychain account (the ecosystem's recovery is the ecosystem's), and a client cache read off a stolen device (checklist 5g-rev).

---

## 3 · The recovery paths, compared [plan]

| Path | Who starts it | What proves the claim | What an attacker needs | Cost to a legitimate user |
|---|---|---|---|---|
| **A · Another passkey** (a second device, or a synced platform passkey) | the person | a full WebAuthn assertion from a passkey they still hold | that passkey and its user verification | none at the time of loss. One extra registration beforehand, or nothing if the platform keychain syncs |
| **B · One-time recovery codes** | the person | one unused code, issued at a time they had presence | the printed or stored codes | keeping a piece of paper; a quarantine on step-up after use |
| **C · Tenant administrator** | an owner or manager of the tenant, with fresh presence | the administrator's own judgement that this is their colleague | an administrator's passkey, or a persuaded administrator | asking a colleague; available to managed identities only |
| **D · Deployment owner** | the deployment owner (an owner of the system space, with fresh presence), or the host-side command | evidence the deployment owner records by class, plus the delay passing uncontested | a persuaded deployment owner and an uncontested delay | waiting out the delay |
| **E · Verified secondary channel** (email) | the person | control of the channel | the mailbox | waiting out a longer delay. **Not buildable today** (0.11) |
| **F · Waiting period with notification** | n/a | silence from the real owner | for the real owner to be unreachable | time |
| **G · Multi-party approval** | n/a | k people with presence agreeing | k passkeys, or k persuaded people | coordinating k people |

F and G are **modifiers** applied to C, D and E. Neither proves identity alone; each makes an assisted path harder to abuse.

### 3.1 Path A is prevention, and the ecosystem already does most of it

A synced platform passkey (iCloud Keychain, Google Password Manager, a password manager) survives the loss of a device because the ecosystem restores it. For those users device loss needs nothing from Ordo. A person with two registered passkeys who loses one signs in with the other and removes the lost one; that is `Directory::link` and `Directory::unlink` as built, behind routes that do not exist yet (0.4).

K2 rules out requiring a second passkey. **The plan offers and does not block:** after a first registration the client is told, in the response, that this identity has one authenticator and no recovery codes. What the application does with that is the application's.

One thing the server cannot know: whether a given passkey is synced. WebAuthn exposes backup eligibility and backup state flags in `authenticatorData`. `webauthn.rs` parses the flags byte; recording BE and BS at registration lets the hint say "this passkey is backed up by your platform" and lets a deployment owner see which identities have a single device-bound key. Recording them is R2 work. They are a hint and no policy hangs on them by default.

### 3.2 Path B: recovery codes, which are also the backup codes of the second factor

Ten codes, each **24 characters from a 32-symbol alphabet (Crockford base32), 120 bits**, from the OS CSPRNG, shown once as six groups of four. Stored as `HMAC-SHA256(deployment key, code)` in `$recoverycode` with the principal, `issued_at`, `used_at` and `used_as`. The entropy makes a slow hash unnecessary and satisfies NIST SP 800-63B §5.1.2.2's rule for look-up secrets at or above 112 bits; the HMAC key means a database dump alone cannot confirm a guess. The standard, its sources and every rule each choice follows are in `plan-second-factor.md` §6.

Issued only to a session with fresh presence, the same gate as linking, so a stolen bearer token cannot mint them (T5); or, for a person with no passkey, to a session with a fresh first factor and a completed second step (`plan-second-factor.md` §6.4). Generating a new set voids the old one. Offered at enrolment, skippable (K2). Five wrong codes end a ceremony; twenty consecutive failures on one principal disable redemption until a new set is issued.

**One set of codes, two uses [settled, `second-factor` answer 5].** Used **with** a first factor, a code is the second step and mints an ordinary session for that first factor (`plan-second-factor.md` §5.2, §6.4; `used_as = 'second-step'`). Used **alone**, a code is this path: it proves possession of a piece of paper and nothing else, so redeeming it does less than signing in. It opens a recovery case that matures immediately (K7: *"a recovery code works at once"*) and yields a **recovery ticket**, and everything in §4 and §7 applies (`used_as = 'recovery'`). A code spent one way cannot be spent the other.

### 3.3 Path C: the tenant administrator

Available only when the identity is **managed** in the sense of `plan-enrolment.md` §4.2: created by this tenant's administrator and a member of this tenant alone. The administrator needs fresh presence. The result is a recovery ticket the administrator hands over in person or however they already reach that colleague.

Default delay: zero. The ordinary case is an employee standing at the IT desk, and a delay there fails K2's test of being easy to use. The contest window and the quarantine still apply. A tenant may set a delay of its own.

### 3.4 Path D: the deployment owner

For independent identities, for anybody a tenant cannot recover, and for the last-owner case (§6). Two forms:

- **Through the door:** `std.identity.recover`, a platform op bound to the system space, owner-only, step-up gated like its siblings. It opens a case naming the principal and an evidence class.
- **At the host:** `ordo identity recover --principal <id>`, which uses the database credentials the CLI already holds for its other host-side commands. This is the only way back when the deployment owner's own last passkey is gone. It is an act by whoever controls the host, recorded under `Subject::platform("cli")`.

Default delay through the door: 24 hours. Default at the host: zero, because a person with the database and the master key is already sovereign over the deployment and a delay would constrain nobody. Both are recorded at deployment scope and printed in the startup log for as long as the case is inside its contest window.

### 3.5 Path E: a secondary channel **[seam]**

Nothing can send mail and no principal has an address (0.11). The plan defines where a channel would attach and builds nothing:

- a `RecoveryChannel` trait with one method, `deliver(principal, notice)`, implemented by whatever `plan-auth-federated.md` builds for email codes;
- used first for **notification** (a recovery was opened on your account), which helps every path;
- usable as **proof** only when a deployment enables it explicitly, and then only to open a case with the longest delay (default 72 hours). **[settled, K11]** Off by default, and the deployment owner's guide documents the option well, in the owner's words *"someone may want it"*: what it proves, what T1 in §2 says a mailbox compromise then becomes, and the delay it carries.

### 3.6 The recommended default

1. **Offer** a second passkey and recovery codes after first registration. Require neither.
2. **Self-service first:** another passkey (immediate, no quarantine), then a recovery code (immediate, quarantined; K7).
3. **Assisted second:** the tenant administrator for a managed identity; the deployment owner for everybody else.
4. **Every recovery ends the same way:** a new passkey, the lost one revoked, every session ended, the act recorded at two scopes, and step-up quarantined.
5. **Email as proof is off** until a channel exists and a deployment turns it on.

---

## 4 · The mechanism [plan]

### 4.1 One record, one ticket, one new function

**A recovery case** is a row in `$recovery`, a deployment table: id, principal, path, who opened it and under which authority, evidence class, opened-at, matures-at, contest-until, state (`pending`, `matured`, `redeemed`, `cancelled`, `expired`).

**A recovery ticket** is what a matured case yields. It is the enrolment ticket of `plan-enrolment.md` E2 with its purpose set to recovery: single-use, expiring, redeemable only by completing a **passkey registration** ceremony. There is no purpose value that accepts a password, which is how K5 is a property of the type.

**`Directory::recover`** is a new function beside `link` and `link_first`, sharing no code path with either (0.3). It takes a redeemed recovery ticket, which like `LinkCeremony` has private fields and one constructor, and does four things in one transaction: inserts the new link marked as recovered, marks the named authenticators as **contested-only** (§4.2), advances the principal's session epoch (§4.3), and returns the audit events.

### 4.2 The contest window

This is how the real owner cancels without a mail channel.

When a recovery completes, the authenticators it declared lost are not deleted. For the length of the contest window (default 72 hours, never shorter than the quarantine) each is kept in a state where an assertion from it is accepted for one act only: **contesting the recovery**. Contesting unlinks the authenticator the recovery added, ends every session again, restores the old authenticators to full use, and records who contested with what. It also removes **every passkey, other sign-in link and second factor enrolled at or after the case opened** (owner ruling 2026-10-25), and records each by kind and id, so the owner is never asked for a step only the thief can give.

Someone who really lost their phone never contests, and at the end of the window the old links and their public keys are deleted. Someone whose codes were stolen signs in with the passkey they still hold, is told a recovery happened, and cancels it.

The cost, stated: a thief holding the lost device **and** able to pass its user verification can cancel a legitimate recovery. That denies service and grants nothing, and the person goes to path D, where the deployment owner can close the contest early with a recorded reason (`std.identity.closeContest`, built 2026-10-24).

For a case that has a delay, the same rule applies before maturity: any assertion from an existing authenticator of that principal is shown the pending case and can cancel it.

### 4.3 Ending every session: recovery requires building it

**Yes, recovery requires "sign out everywhere", and it is the first increment (R1).** A recovery that left old sessions running would leave a thief with the stolen device signed in for up to twelve hours after the owner recovered.

The shape is the one `session.rs:60-64` names. A table `$principalepoch` (`principal`, `not_before`, `until`), loaded into memory beside the session deny list and refreshed on the same cadence. The door refuses a session token whose `started_at` is earlier than `not_before` for its principal. `session::refresh` makes the same check, so an ended session cannot be continued. Rows are swept once `not_before + ORDO_SESSION_MAX_SECS` has passed, since every older token is dead by its own ceiling. The door's cost is one more in-memory lookup.

`plan-enrolment.md` E6 needs the same capability for password reset. It is one mechanism; whichever increment ships first builds it and the other uses it.

**What it does not end:** capability tokens and delegated tokens name no session (`session.rs:244-248`) and are revoked by withdrawing the grant or delegation. See open question 6.

### 4.4 What is configurable, and by whom

The safe value is the default, and a weaker one is an explicit setting that is recorded in the deployment stream when it takes effect (K6).

| Setting | Default | Deployment may | Tenant may |
|---|---|---|---|
| Paths enabled | A, B, C, D | disable any; enable E once a channel exists | disable B-issued codes for its managed identities; disable C |
| Delay, path C | 0 | raise | raise |
| Delay, path D through the door | 24 h | raise, or lower to a floor of 1 h | n/a |
| Delay, path E | 72 h | raise, or lower to a floor of 24 h | n/a |
| Contest window | 72 h | raise | n/a |
| Step-up quarantine, the export block (K8) | **24 h after B, C, D and E**; none after A | raise, or lower to a floor of 1 h | raise for ops in its own space |
| Approvals required (G) | 1 | raise for D | raise for C |
| Hold on recovery (§5.3) | **off** (K10) | set the default | turn on for its own space |
| Email as proof (§3.5) | **off** (K11) | turn on, once a channel exists | n/a |

Deployment settings are environment variables read at boot in the house manner: an unparseable value or one below its floor refuses the boot and names the floor. Tenant settings are fields on the tenant's own record, changed by an owner with presence.

A tenant can only tighten. A tenant loosening a deployment's floor would let one tenant weaken a credential that other tenants rely on, which K3 forbids.

---

## 5 · The multi-tenant rule [plan]

**A membership is the tenant's. A credential is the deployment's and reaches every tenant its holder belongs to.**

### 5.1 What a tenant administrator can do to any member, whatever else they belong to

- suspend, reactivate or remove **the membership in this tenant**;
- change the role in this tenant;
- vouch for the person in a recovery case somebody else opened (path G), which adds evidence and decides nothing;
- read, in this tenant's stream, that a member's credential was recovered, when, and under what class of authority.

### 5.2 What a tenant administrator can do only to a managed identity

- open a recovery (path C);
- disable recovery codes for it.

An identity stops being managed the moment it is admitted to a second tenant (`plan-enrolment.md` §4.2). From then on the first tenant's administrators are refused path C with the door's one answer, and the refusal is recorded in their own tenant's stream.

### 5.3 What a tenant administrator can never do

- reset, recover, unlink or add an authenticator of an independent identity;
- learn which other tenants a person belongs to. The refusal in 5.2 does not say why; the administrator is told to refer the person to the deployment owner;
- read another tenant's record of the same recovery.

**The reverse direction needs a rule too.** When the deployment owner recovers an identity that belongs to tenants A and B, the new credential opens both. Each tenant is told in its own stream. A tenant that wants more can turn on **hold on recovery**: a recovered principal's membership in that tenant is suspended by the recovery and stays suspended until an owner or manager of that tenant reactivates it, which is the existing step-up-gated reactivation. **Off by default [settled, K10: a recovered person is not locked out of each tenant]**, because in the ordinary case it makes a person who lost a phone wait on every tenant they belong to. It is the control for a tenant whose compliance position is that no outside party's judgement admits anybody.

Hold is not applied to a principal who is the only active owner of the tenant, since nobody would be left to release it. That case is §6.

---

## 6 · The last-owner problem [plan]

A tenant has one owner, and that owner's only passkey is gone.

**Prevention, offered and not enforced.** When a tenant has exactly one active owner, and that owner has one authenticator and no unused recovery codes, the owner's sign-in response carries a notice saying so. `plan-enrolment.md` E3 already refuses to remove the last owner.

**Who can restore access, in order of preference [settled, K9: *"Yes the deployment owner"*]:**

1. **The owner, with a recovery code** (path B). No second party.
2. **The deployment owner** (path D). Under K1 the service owns credentialing, so the deployment owner is the authority of last resort for every identity.
3. **Nobody inside the tenant.** A manager cannot recover an owner: that would make manager a role that can become owner by reporting a lost key.

**How it is proven.** The deployment owner records an **evidence class** from a closed list, plus an opaque reference to their own ticketing system. No free text enters the stream. Classes: `vouched-by-members` (one or more managers of the tenant vouched with presence, and the case names them), `verified-out-of-band` (the deployment owner verified by means outside the platform), `contract-holder` (the requester is the party the deployment's agreement names), `host-break-glass`. The platform cannot check any of these except the first. Recording the class makes the deployment owner's claim specific and attributable, so an auditor can ask for the evidence behind it.

The delay is the path D default. If the tenant has managers, each is notified in-band at their next request, and any of them can **object**, which pauses the case until the deployment owner resolves the objection with a recorded decision.

**When the owner is gone for good.** A person who has left, or died, will not redeem a ticket. The act needed is a membership act: the deployment owner makes an existing member the owner. It is a separate op, `std.space.succeedOwner`, bound to the system space, carrying the same evidence class, delay and objection rules. It touches no credential. It is separate so the record says which of the two happened.

**How it is recorded.**

- In the **deployment** stream: case opened, by whom, evidence class, reference, each vouch and objection, matured, redeemed, the new authenticator, the authenticators retired.
- In the **tenant's** stream: the same acts, naming the deployment owner as "deployment owner" and the principal by id. This stream is the one the restored owner reads through `/security/events`, and a domain revert cannot rewrite it.
- **To the restored owner, at first sign-in:** the `/auth/complete` response carries the case id and its summary, and keeps carrying it until the owner acknowledges it with presence. The acknowledgement is itself recorded.

---

## 7 · Presence after recovery [plan]

**A passkey registered through recovery cannot satisfy step-up until its quarantine ends.** Default **24 hours after paths B, C, D and E, adjustable**, none after path A. **[settled, K8: *"a day's block on export after recovery, adjustable"*.]** The 72 hours this plan proposed for D and E is withdrawn; a deployment that wants it raises the setting.

**The argument.** Every recovery path other than A rests on a proof weaker than a passkey. Step-up exists so that the most damaging ops (`std.space.export`, `std.space.delete`, `std.deployment.apply`) need the strongest proof the platform has. If a recovered passkey could pass step-up at once, the strength of step-up would equal the strength of the weakest enabled recovery path, and an attacker would choose recovery as the route to export. The quarantine keeps the two apart for long enough that the contest window can do its work. It must therefore be no longer than the contest window, and the boot refuses a configuration where it is.

Path A carries no quarantine because the proof behind it was a passkey assertion, and nothing weaker was introduced.

**The mechanism.** The `$passkey` row (`plan-passkey-signin.md` §2.4) carries `presence_not_before`; the link row in `$authenticator` stays as it is. The two functions that put presence into a token (0.6) consult it: `SignIn::token_spec` and `refresh_presence` mint `user_presence_at: None` for a quarantined authenticator. The token is then valid for ordinary ops and structurally unable to pass step-up, by the same data flow that already governs a password session. No change to `AccessEngine` is needed. The sign-in response says when presence becomes available, so the person is told a time.

**What quarantine also blocks, deliberately:** linking a further authenticator and issuing recovery codes, both of which need presence. An attacker who got in through recovery cannot entrench during the window.

**What it costs.** A legitimately recovered sole owner cannot export, delete or deploy for a day. A deployment owner can lift a quarantine early through `std.identity.releasePresence`, with their own presence and a recorded reason. That is the explicit, recorded, weaker choice.

**What quarantine does not block:** ordinary reads and writes. A tenant that needs those blocked as well uses hold on recovery (§5.3).

---

## 8 · What recovery does besides restoring access [plan]

Done in one transaction where the stores allow, in this order, and recovery reports failure if any step fails:

1. **Link the new passkey**, marked recovered, with its quarantine.
2. **Retire the lost authenticators**: contested-only during the window, then the link row and the public key are deleted (0.2). The person chooses which authenticators were lost; the default is all of them.
3. **End every session** of the principal (R1).
4. **Void unused recovery codes.** The person is offered a new set once quarantine ends.
5. **Apply hold on recovery** in each tenant that has it on.
6. **Record.**
7. **Notify**: in-band to every session and authenticator of the principal at its next contact, and through a `RecoveryChannel` when one exists.

### 8.1 What is recorded, and at which scope

New `EventKind` variants, appended as tags 15 onward (0.9), each with a pre-change golden as the precedent requires:

| Event | Fields | Deployment stream | Space stream |
|---|---|---|---|
| `RecoveryCodesIssued` | count | yes | no |
| `RecoveryOpened` | case, path, authority class, evidence class | yes | every tenant the principal belongs to |
| `RecoveryVouched` / `RecoveryObjected` | case, by whom | yes | the voucher's tenant |
| `RecoveryCancelled` | case, by which authenticator or authority | yes | every tenant |
| `RecoveryCompleted` | case, authenticator added, authenticators retired | yes | every tenant |
| `SessionsEnded` | reason | yes | no |
| `PresenceReleased` | case, by whom, reason class | yes | every tenant |
| `OwnerSucceeded` | space, from, to, evidence class | yes | that tenant |
| `RecoveryRefused` | path, reason class | yes | the tenant whose administrator was refused |

Credential acts are deployment-scoped because the credential is the deployment's (K1). Each is mirrored into a tenant's stream because that tenant's owner is entitled to know that a member's credential changed hands. A mirrored entry names the principal, the act and the class of authority. It never names another tenant, and an administrator's identity from tenant A never appears in tenant B's stream; path C cannot produce that case, since a managed identity belongs to one tenant.

---

## 9 · Client-held keys [plan, against a design that is not built]

In a vault space the deployment has no path to the space key (`f7-vault-plan.md` §3.1). Recovering an **account** restores a principal and a passkey. Recovering **data** requires key material that only devices, a recovery phrase, or a disclosed escrow hold.

| Path | Managed custody | Vault custody |
|---|---|---|
| **A**, a synced passkey restored by the platform | account and data | account. Data **if** the vault wraps keys under that passkey's PRF output, because a synced passkey is the same credential and yields the same output. This depends on F7's open question Q4 |
| **A**, a second device that was enrolled in the vault | account and data | account and data; that device holds the space key and can enrol a replacement (§3.2 of the vault plan) |
| **B**, recovery code | account and data | **account only** |
| **C**, tenant administrator | account and data | **account only**. Another member's enrolled device can then enrol the recovered person's new device, which restores their access to the space's data. That is vault enrolment, step-up gated, performed by the other member |
| **D**, deployment owner | account and data | **account only** |
| **E**, secondary channel | account and data | **account only** |
| The vault's recovery phrase | n/a | data. It restores no account |
| Platform escrow, where the space opted in | n/a | data, as the disclosed custody step down it is |

**Stated plainly:** if the last enrolled device of a vault space is lost, and there is no recovery phrase and no escrow, the data is gone. No path in this plan changes that, and no screen or document may imply otherwise. A new passkey has a different PRF output and unwraps nothing the old one wrapped. The recovered account can sign in to a space it cannot read.

A recovery code is not a recovery phrase. The code proves identity to the server. The phrase is key material the server never sees. They must not be merged or presented as one thing, because merging them would put key material in a table the deployment owner can read, which is silent escrow.

In a vault space, retiring a lost authenticator should also trigger the device un-enrolment and DEK epoch roll of vault plan §3.4. That wiring belongs to F7 Phase 6.

---

## 10 · Increments [plan]

Each is independently shippable. Each done-when names a test that fails without the increment.

**Prerequisites owned elsewhere [seam]:** passkey sign-in served with its durable stores and the auth routes recording (`plan-passkey-signin.md` S0 to S5; S0 is the recording half of R2, S4 gives `authenticators_of` for R2's listing, S1 gives `$passkey` and `$authceremony`); the door resolving `(principal, tenant)` (enrolment E1); tickets (enrolment E2). R1 depends on none of them.

| | What | Done when |
|---|---|---|
| **R1** | End every session of a principal. `$principalepoch`, the in-memory check at the door and in `refresh`, a route `POST /auth/logout-all` taking the session's own token | `crates/ordo-host/tests/auth_door.rs`: `ending_every_session_refuses_each_token_minted_before_it` (two sign-ins, one call, both tokens refused, a sign-in afterwards accepted) and `an_ended_session_cannot_be_refreshed`. `auth_durability.rs`: `an_epoch_survives_a_restart` |
| **R2** | Authenticator management. Routes to list one's authenticators (over S4's `authenticators_of`), link with a `LinkCeremony`, unlink with presence; an unlink deletes the `$passkey` row. These routes record their `AuditEvent`s the way S0 made `/auth/complete` record its own. BE and BS are already stored by S1; R2 reads them for the notice. The one-authenticator notice | `auth_door.rs`: `a_link_is_recorded_naming_both_authenticators` reads the stream and finds `AuthenticatorLinked` with `vouched_by`; `a_lost_passkey_is_removed_by_the_one_that_remains` unlinks, ends sessions, and the removed credential's assertion is refused |
| **R3** | The recovery core and the host-side path. `$recovery`, the recovery ticket, `Directory::recover`, the contest window, the quarantine, the events of §8.1, `ordo identity recover` | `ordo-identity` tests: `a_recovery_ticket_cannot_be_redeemed_with_a_password`, `recover_shares_no_state_with_link` (a recovery ticket is refused by `link`, a `LinkCeremony` by `recover`), `a_retired_authenticator_can_contest_and_do_nothing_else`. Host `tests/recovery.rs`: `a_recovered_passkey_is_refused_export_until_quarantine_ends` |
| **R4** | Recovery codes as §3.2 now states them: 24 characters, 120 bits, `$recoverycode` with `used_as`; issue with presence; redeem once, at once (K7); void on reissue and on recovery; the per-principal failure lock. The second-step use of the same codes is `plan-second-factor.md` T4, which reuses this redeem with `used_as = 'second-step'` | `tests/recovery.rs`: `a_code_redeems_once`, `a_code_opens_a_case_that_matures_at_once`, `a_password_session_cannot_issue_codes`, `a_contested_code_recovery_restores_the_old_passkey_and_removes_the_new`, `a_code_spent_as_a_second_step_cannot_open_a_recovery` (written here, green once T4 lands) |
| **R5** | Deployment-owner recovery through the door, the last-owner flow, owner succession, evidence classes, objection, the first-sign-in notice | `tests/recovery.rs`: `a_sole_owner_is_restored_and_both_streams_say_so` reads the deployment stream and the tenant's; `a_manager_cannot_recover_an_owner`; `succession_touches_no_credential` |
| **R6** | Tenant-administered recovery for managed identities, and hold on recovery | `tests/recovery.rs`: `an_administrator_recovers_a_managed_member`; `the_same_administrator_is_refused_once_the_member_joins_a_second_tenant`, asserting the refusal equals the door's one answer; `a_held_tenant_stays_closed_until_its_own_owner_reactivates` |
| **R7** | Policy: the deployment's floors and the tenant's tightening, multi-party approval | `tests/runtime.rs`: `a_quarantine_longer_than_the_contest_window_refuses_the_boot`, `a_delay_below_the_floor_refuses_the_boot`. `tests/recovery.rs`: `a_tenant_cannot_lower_a_deployment_floor`, `two_approvals_required_means_one_is_not_enough` |
| **R8** | Reconciliation. `ordo verify --identity` walks `$authenticator` and reports any link with no matching `AuthenticatorLinked`, `PrincipalOnboarded` or `RecoveryCompleted` in the signed stream | `crates/ordo-cli/tests`: `a_link_row_inserted_by_hand_is_reported` |
| **R9** | **[seam]** `RecoveryChannel`, notification first, proof as an opt-in | not scheduled until `plan-auth-federated.md` builds a channel |

**Recommended order: R1, R2, R3, R4, R5, R6, R7, R8.**

R1 first because it has no prerequisite, enrolment needs it too, and it closes a gap that exists today for password sessions. R2 second because it makes path A work and fixes the dropped audit events, which every later increment's record depends on. R3 before R4 because the first deployment is one deployment owner with one passkey, and the host-side path is what stops that deployment from being bricked by a lost phone. R8 can move earlier if the owner weighs T3 more heavily.

---

## 11 · Questions for the owner: four answered, five open

1. **Should a recovery code restore access at once, or after a delay?**
   **Answered 2026-09-29: at once**, with the quarantine. A deployment holding data where a day of reads is unacceptable may still set a delay for path B (§4.4).

2. **Is 24 hours the right quarantine after self-service recovery, and 72 after assisted?**
   **Answered 2026-09-29: a day, adjustable.** One default for every path but A. §7.

3. **May the deployment owner's host-side recovery skip the delay?** **Open.** The board ask numbered 3 was §6's question, who restores a tenant owner, and the owner answered that one (*"Yes the deployment owner"*).
   Recommend yes. Whoever runs that command holds the database and the master key, so a delay enforced by the software they control restrains nobody and would only prolong an outage. The record is the control. The case against: a hosting provider running Ordo for a customer is a deployment owner the customer may not fully trust, and a delay would at least give notice. That deployment should set the delay and treat R8 as mandatory.

4. **Should hold on recovery be on by default?**
   **Answered 2026-09-29: off.** A recovered person is not locked out of each tenant. Available to a tenant that turns it on (§5.3).

5. **May a manager, and not only an owner, open path C?** **Open.**
   Recommend **yes for members below manager, no for peers and owners**. The manager administers who may do what, and sending every lost phone to the owner pushes tenants to hand out owner, which is the argument `callersOf` already makes. A manager recovering another manager or an owner would be a route to escalation.

6. **Should recovery revoke the capability grants and delegations the person consented to?** **Open.**
   Recommend **no by default, and show them**. After recovery the person is given the list of live grants and delegations with the date each was consented, and can withdraw any. Revoking all of them would break every integration the person set up because they lost a phone. The case for revoking: if the account was compromised before the loss was noticed, a grant minted by the attacker survives the recovery. A middle course the owner may prefer: revoke only grants consented within N days before the recovery.

7. **Should email ever be sufficient proof on its own?**
   **Answered 2026-09-29: not by default**, and the option is documented well because *"someone may want it"*. §3.5, §4.4.

8. **Who may object to a last-owner recovery?** **Answered 2026-10-24: managers of that tenant only**, as recommended (board `owner-succession-what-the-app-developer`). Built: see R5b.
   Recommended **managers of that tenant only**. They know whether the owner really lost a key. Letting every member object would let one editor stall a legitimate recovery.

9. **What happens to a tenant whose last owner cannot be restored and has no successor?** **Open.**
   Recommend that this plan leave it locked and intact, and that ending or exporting such a tenant be decided under the exit ops' own rules. A deployment owner exporting a tenant's data with no owner's consent is a custody question larger than credential recovery.

---

## 12 · What this plan does not do

- It does not wire passkey sign-in, build the browser ceremony, or make passkey public keys durable. `plan-passkey-signin.md` does all three, before this plan (0.1, 0.2).
- It does not build the second step or the factor routes. `plan-second-factor.md` does, after this plan, and shares §3.2's codes.
- It does not design SAML, OAuth2, email sign-in or password enrolment. Its seams with them are: the ticket (enrolment E2), the managed and independent distinction (enrolment §4.2), sign out everywhere (shared with enrolment E6), and `RecoveryChannel` (whatever `plan-auth-federated.md` builds for mail).
- It does not build a mail channel, store contact addresses, or send anything outside the deployment. Until a channel exists, notification is in-band and a ticket is delivered by a person.
- It does not recover data in a vault space. §9 says which paths restore data and which restore only the account. The recovery phrase, escrow, device enrolment and the DEK roll on revocation are F7's.
- It does not recover a password. A password reset is `plan-enrolment.md` §4. No path here produces a password, and a passkey identity does not become a password identity by being recovered.
- It does not prevent a person with write access to the control database from minting a credential. R8 detects it afterwards.
- It does not verify attestation or enforce a hardware policy (`security-logging-design.md` §2.1 rules that out), so it cannot require that a recovered passkey be of a particular kind.
- It does not build a console. Every act is an op or a route, which is what a console will call.
- It does not add a session table. Sign out everywhere is an epoch per principal, and the deployment still cannot list a person's live sessions.
- It does not protect a client cache on a stolen device. That is checklist 5g-rev.

---

### Critical Files for Implementation

- crates/ordo-identity/src/directory.rs
- crates/ordo-identity/src/session.rs
- crates/ordo-host/src/auth.rs
- crates/ordo-host/src/sessions.rs
- crates/ordo-secevent/src/event.rs

Also touched: crates/ordo-secevent/src/chain.rs (appended tags), crates/ordo-host/src/lib.rs (the door's epoch check beside line 1481), crates/ordo-host/src/principal_directory.rs (`$authenticator` columns), crates/ordo-host/src/platform_ops.rs (the new system-space ops), crates/ordo-host/src/runtime.rs (policy variables and boot refusals), docs/plan-enrolment.md (the neighbouring plan whose E1, E2 and E6 this one depends on).

---

## Built

R1 to R8 were built in order, one commit each, on the branch
`worktree-agent-a1b86163cd7861585`. Where the plan turned out wrong against
the code, the entry says so. What was not built because an owner question
is open is listed at the end of this section.

### R1 · End every session of a principal

Most of R1 was already in the tree. Enrolment E6 built the epoch:
`$principalepoch`, `SessionRevocations::end_all` and `principal_ended`,
the door's check and `session::refresh`'s check, and the
`sessions.refresh` worker that re-reads it. What R1 added is the route and
its record.

- `POST /auth/logout-all` (`crates/ordo-host/src/auth.rs`, `serve.rs`). The
  session's own bearer token is the whole authorization, checked the way
  step-up checks it (a live session, not a capability, not a delegation).
  It calls `end_all(principal, now, now + ceiling)`. This session ends with
  the rest. A write that failed is a `503`, never a `204`.
- `EventKind::SessionsEnded { reason }`, chain tag 16, deployment scope.
- Tests. `auth_door.rs`:
  `ending_every_session_refuses_each_token_minted_before_it` (two
  sign-ins, one call, both refused at the door with `SessionRevoked`, a
  sign-in afterwards admitted), `an_ended_session_cannot_be_refreshed` and
  `logout_all_is_refused_without_a_live_session_and_recorded_when_it_works`.
  `auth_durability.rs`: `an_epoch_survives_a_restart`. The three route tests
  fail with the route not mounted (404). The restart test fails with the
  database write in `end_all` removed.

**The plan was wrong** that R1 builds the epoch. It was built by E6. A
sign-in in the same second as the call is ended too (`end_all(at)` ends
sessions that started at or before `at`, a tie going to the safe side), so
a script that ends everything and signs in at once is refused once. A
browser flow takes longer than a second.

### R2 · A person's own authenticators

Recovery's path A: a person with two passkeys who loses one signs in with
the other and removes the lost one.

- `crates/ordo-host/src/auth/authenticators.rs`. `GET /auth/authenticators`
  lists each authenticator with its kind, state and (for a passkey) the
  backup flags and last use, and a `single_authenticator` notice when the
  person holds one active way in. `POST /auth/authenticators/link/begin` and
  `/link` add a passkey. `POST /auth/authenticators/unlink` removes one.
- **Link and unlink each carry their own assertion**, in the body, from a
  passkey the person holds, through the step-up challenge. The token does not
  say which authenticator made its presence, so a link built from a token
  could not name the one that vouched. With the assertion in the request the
  audit event names it (`AuthenticatorLinked { vouched_by }`), a stolen
  session token alone adds and removes nothing, and a session whose presence
  went stale needs no re-step-up first. A link ceremony is still a
  `LinkCeremony`, built from claims re-stamped with the presence just proved.
  The registration half is bound to the person (`link:<principal>`), so a
  sign-up challenge cannot finish a link.
- Unlink removes the directory link **and the public key**
  (`PasskeyProvider::remove_credential`), refuses the last active
  authenticator with a `409 last_authenticator` the person is told, and ends
  every session by default (`end_sessions: false` keeps them; either is
  recorded).
- `/auth/complete` carries `authenticators` (a count) and a
  `single_authenticator` notice. Nothing is enforced on it (K2).
- Tests, in `auth_passkey.rs` (where the passkey harness lives):
  `a_link_is_recorded_naming_both_authenticators`,
  `a_link_needs_a_vouching_assertion_from_the_persons_own_authenticator`,
  `a_lost_passkey_is_removed_by_the_one_that_remains`,
  `the_last_passkey_and_another_persons_passkey_are_not_removable`,
  `a_person_with_one_authenticator_is_told_so`. Seen failing with the key
  removal, the ownership check and the link binding each taken out.

**The plan was wrong** that `/auth/complete` drops the audit events it
returns. S0 fixed that before recovery began. The BE and BS flags were
stored by S1. The two named tests are in `auth_passkey.rs`, not
`auth_door.rs`, because the passkey harness is there. A password-only person
has no passkey to vouch and adds a first one with a ticket, which is
enrolment's.

### R3 · The recovery core and the host-side path

- **The case.** `crates/ordo-host/src/recovery.rs`: `$recovery`, `Case`,
  `State` (`pending`, `matured`, `completed`, `cancelled`, `closed`),
  `RecoveryStore` (memory and Postgres), and `Recovery`, the service. One
  live case per person, by a partial unique index and not a check.
- **The ticket.** `Purpose::Recovery` in `tickets.rs`. No route that takes a
  password asks for it. A ticket names its case by naming its person, so the
  ticket needs no case column.
- **The directory.** `Directory::recover` beside `link`, sharing no function
  and no argument type with it: it takes a `RecoveryTicket` and asks for no
  presence and no vouching factor. It adds the new link marked with its case
  and its quarantine, and retires the lost links. Nothing is deleted at once.
  `Directory::contest`, `undo_recovery` and `purge_retired` complete it.
  `$authenticator` gained `presence_not_before`, `recovered_case`,
  `retired_case` and `retired_until`, added with `ADD COLUMN IF NOT EXISTS`.
- **Retired.** A retired authenticator is refused at sign-in by name
  (`AuthenticatorRetired`, before anything that could onboard it as a
  stranger), cannot step up, cannot vouch a link, and does not count as a way
  in for the last-authenticator rule. It can contest, and nothing else.
- **Quarantine.** `sign_in` and `refresh_presence` mint no presence for a
  quarantined authenticator, and say when it counts (`presence_available_at`
  in the sign-in and step-up answers). Link and unlink refuse it as a voucher.
  Default one day after every path (owner, 2026-09-29: a day's block on export
  after recovery, adjustable).
- **Routes.** `POST /auth/recover/begin` (ticket in, creation options out,
  bound to the ticket), `/auth/recover/complete` (ticket and registration in;
  no token out, because every session ended and the new passkey is
  quarantined), `/auth/recover/contest` (an assertion from a retired passkey
  contests a completed recovery; one from an active passkey cancels one that
  has not completed). A retired passkey that signs in is answered `409
  recovery_contestable` with the case and its window.
- **The order of completion.** The ticket is looked at, the case locked
  (`matured` to `completed`), the directory recovers, the ticket is spent,
  every session ends, settlements run, the acts are recorded. A refusal
  before the spend takes the lock back and spends nothing.
- **Settlements.** `trait Settlement`, run when a recovery completes and not
  when it is opened. The second factor registers one to remove every second
  factor (owner, 2026-09-29: *"but only after the reset"*). None is registered
  yet, because no second factor exists.
- **The record.** `EventKind::Recovery { act, detail }`, chain tag 17, at
  deployment scope and mirrored into each tenant the person belongs to
  (`with_tenants`, wired from the door's engine). An entry names the person,
  the act, the path and the class of authority, and never another tenant.
- **The host.** `ordo identity recover --principal <id> --as <who>
  [--evidence <class>] [--ref <r>]` (`crates/ordo-cli/src/identity.rs`) opens a
  case with no delay and prints the ticket once. The evidence class is from a
  closed list. It is recorded under `platform cli`. The sweep worker
  `auth.recovery.sweep` removes retired links and their keys when a window
  closes.
- Tests. `ordo-identity` (`passkey_tests.rs`):
  `recover_shares_no_state_with_link`,
  `a_retired_authenticator_can_contest_and_do_nothing_else`,
  `the_contest_window_closes_and_the_retired_are_removed`,
  `a_recovered_passkey_signs_in_without_presence_until_the_quarantine_ends`,
  `recover_refuses_what_is_not_the_persons`. `tests/recovery.rs`:
  `a_recovered_passkey_is_refused_export_until_quarantine_ends`,
  `a_recovery_ticket_cannot_be_redeemed_with_a_password`,
  `a_recovery_completes_once_and_only_with_its_own_ticket`,
  `a_retired_passkey_contests_and_the_recovery_is_undone`,
  `a_closed_window_removes_the_retired_passkeys_and_their_keys`,
  `a_person_has_one_live_case`,
  `a_delayed_case_yields_its_ticket_late_and_an_active_passkey_cancels_it`,
  `a_settlement_runs_only_once_the_recovery_completes`,
  `a_recovery_is_recorded_and_mirrored_into_each_tenant`.
  `auth_durability.rs`: `a_case_survives_a_restart_and_a_person_has_one_live_case`,
  `a_retired_link_and_a_quarantine_survive_a_restart`. `ordo-cli/tests/identity.rs`:
  the command opens a case and prints one ticket, and refuses a stranger and an
  evidence class off the list. With the quarantine removed from `sign_in`,
  the export test and the identity test fail; with the retired refusal removed
  from sign-in, the contest test fails.

Deviations and gaps, stated:

- One event kind for the family (`Recovery { act, detail }`, tag 17) and one
  for ended sessions (tag 16), not the nine variants of §8.1. It is the shape
  E2 chose for enrolment, so a new act is a word and not a change to the
  chain's encoding. The acts are `opened`, `completed`, `contested`,
  `cancelled`, `closed`, `settlement-failed` and `sessions-not-ended`.
- **The quarantine is one day after every path**, because the owner said a
  day. The plan's 72 hours after paths D and E was a proposal the owner did
  not take up. It is a setting, and the two numbers are separate.
- `a_recovery_ticket_cannot_be_redeemed_with_a_password` is in
  `tests/recovery.rs`, not `ordo-identity`, because the ticket purpose lives
  in the host.
- A settlement that fails after the recovery completes is recorded and
  returned as a warning, and the recovery stands. The plan says recovery
  reports failure if any step fails. Undoing a completed recovery because a
  second-factor removal failed would leave the person locked out by the step
  meant to help them.
- **`ordo identity recover` does not mirror into a tenant's stream.** Which
  tenants a person belongs to is the running door's to answer, and the
  command has the database and not the door.
- Recovery policy is `RecoveryPolicy::default()` in the runtime. R7 reads it
  from the environment.

### R4 · Recovery codes

Built to `plan-second-factor.md` §6, which fixed the format and added the
`used_as` column so the second factor's T4 reuses the same table and codes.

- `crates/ordo-host/src/recovery_codes.rs`. **Ten codes, each 24
  characters from Crockford base32 (no `I`, `L`, `O`, `U`), 120 bits from
  the operating system's generator, shown as six groups of four**, typed back
  in any case, with or without the dashes, and with Crockford's aliases (`O`
  is `0`, `I` and `L` are `1`). Stored as `SHA-256` of the normalised code
  under a domain tag in `$recoverycode` (`principal`, `hash`, `issued_at`,
  `used_at`, `used_as`), never the code. `used_as` is `''`, `second-step` or
  `recovery`. R4 writes `recovery`. Single use is one `UPDATE ... WHERE
  "used_at" IS NULL RETURNING`, so two replicas racing one code cannot both
  win. A new set replaces the old one in one statement. A recovery completing
  voids the unused ones (`VoidCodes`, a `Settlement`).
- `POST /auth/recovery-codes` (a session's bearer) issues a set, once, with
  `Cache-Control: no-store`. **It needs fresh presence**, the gate linking has:
  a password session, a passkey session whose presence went stale, and a
  passkey session whose passkey is quarantined are each refused `403
  presence_required`. It records `codes-issued` with the count. A person who
  holds no passkey may send a reauth handle (`x-reauth`) instead. **The
  handle is refused to anyone who holds a passkey** (fixed 2026-10-24, audit
  F-1): a quarantined passkey got a handle from `/auth/complete {"reauth":
  true}` and minted 10 codes with it. The rule lives in `factors::prove_change`,
  the one gate every act taking the handle goes through (recovery codes, a
  second factor's enrolment and removal, backup codes).
- `POST /auth/recover/code {"code"}` spends one: it opens a case (path `code`,
  authority `recovery-code`) and returns the recovery ticket **at once** (owner,
  2026-09-29). The case is opened before the code is spent, so a person who
  already has a live case does not burn a code on a refusal, and a request that
  loses the race for the code cancels the case it opened. It records
  `code-used` with `used_as=recovery` and how many remain. A code alone opens a
  recovery and nothing else; it does not sign anybody in.
- **A throttle on wrong codes**, deployment-wide: 30 in a minute, then `429
  too_many_attempts` with `Retry-After` for the rest of the window, right code
  or wrong.
- `GET /auth/authenticators` and `/auth/complete` say how many codes remain
  (`recovery_codes_remaining`), and the list says `recovery_codes_low` at one or
  two.
- The policy has `code_delay_secs`, zero. A deployment that cannot allow a day
  of reads after a stolen code sets a delay, and the answer then carries no
  ticket until the case matures.
- Tests. `tests/recovery.rs`: `a_code_redeems_once`,
  `a_password_session_cannot_issue_codes`,
  `a_contested_code_recovery_restores_the_old_passkey_and_removes_the_new`,
  `a_set_is_ten_grouped_codes_and_the_table_holds_none_of_them`,
  `a_code_typed_loosely_is_the_same_code`, `a_new_set_voids_the_old_one`,
  `too_many_wrong_codes_are_throttled`,
  `a_delay_on_the_code_path_holds_the_ticket_back`. `recovery_codes.rs` unit
  tests: the alphabet and length, the grouping and the typed forms, the hash.
  `auth_durability.rs`:
  `a_code_is_spent_once_across_two_stores_and_a_set_survives_a_restart`.
  `runtime.rs`: `a_booted_deployment_serves_the_recovery_routes_and_has_wired_recovery`.
  With the presence check removed the issue test fails; with the void
  settlement removed the redeem test fails.

**A defect in R3, found and fixed here.** R3 mounted its three routes on the
auth router the harnesses use and not on the platform router the binary serves
(`serve.rs`), and no test booted the binary to say so. They are on it now, with
R4's two, and the runtime test above boots a deployment and calls them.

Deviations, stated:

- **Hashed with plain SHA-256, not an HMAC under a deployment key.** At 120
  bits the two are as strong against a leaked table, and a keyed hash would tie
  every printed set to a key whose rotation would void all of them without
  anyone being told. A keyed form waits on the key service (18y), where a key
  that can rotate safely belongs.
- **The failure limit is the deployment's, not the person's.** A wrong code
  names nobody, so failures cannot be counted against a person. The person's
  lock (twenty in a row, owner 2026-09-29) belongs to the second step, where
  the first factor names the person. It is `RecoveryCodes`' to gain, with
  the `second-step` use.
- The plan's "five attempts per ceremony" has no ceremony to bound here: the
  route takes one code and answers.

### Audit fixes, 2026-10-24 (F-1, F-3, F7)

Built after the security audit of 2026-10-24 confirmed each by reproduction.

- **F-1. The reauth stand-in is for people without a passkey.** See R4 above.
- **F-1. A contest voids every unused recovery code** (`VoidCodes`, the
  settlement completion runs). Completion voided the codes from before, so
  every code left was issued since by whoever held the recovered passkey.
  The contest also ends every session and renewal (`end_all`), and records
  `contested` with `codes-voided=yes`. A contest of a case that is no longer
  `completed` is refused.
- **F-3. A passkey a recovery added may not cancel a later case** while that
  recovery's contest window, as first given (`contest_until`), is open. It is
  refused and recorded `recovered-passkey-in-contest-window`.
- **F-3. `std.identity.closeContest {case, reason}`** is built, the early
  close §4.2 promised. Deployment owner only, home space, presence (every
  platform write asks for it), reason one of `recovery-verified`,
  `recovery-disputed`, `opened-in-error`. The case goes `completed` to
  `closed`; each retired link's window ends now, so it cannot contest and
  the sweep removes it and its key. A new case may open at once. Recorded
  `contest-closed` in the deployment's stream, naming who, and in each
  tenant's as the deployment owner. The console does not call it yet.
- **F7. Password challenge ids are 128 random bits**, where they were a
  counter that let anyone spend a neighbour's challenge with junk. A wrong id
  and an expired one are the same `UnknownChallenge`. The waiting challenges
  are capped by `ORDO_AUTH_PASSWORD_PENDING_MAX` (10000); a begin past it is
  `429` naming the setting.
- Tests. `tests/recovery.rs`:
  `a_quarantined_passkey_cannot_issue_codes_with_a_reauth_handle`,
  `a_person_with_no_passkey_issues_codes_with_a_reauth_handle`,
  `a_contest_voids_the_codes_the_recovered_passkey_issued`,
  `closing_the_contest_lets_the_owner_recover_at_once_and_the_thief_cannot_cancel`,
  `a_recovered_passkey_cannot_cancel_a_case_inside_its_window`,
  `password_begins_past_the_cap_are_refused_naming_the_setting`.
  `tests/recovery_ops.rs`:
  `the_deployment_owner_closes_a_contest_and_a_new_case_opens_at_once`.
  `ordo-identity` unit tests:
  `password_challenge_ids_are_random_and_a_guess_burns_nobody`,
  `password_challenges_past_the_cap_are_refused_naming_the_setting`.
  The F-1 and F-3 tests in `tests/recovery.rs` were each run with their fix
  taken out, and failed.
- Second factors enrolled after the quarantine surviving a contest: ruled
  2026-10-25 and built, see below.

### What a contest resets, 2026-10-25 (owner ruling)

Board `after-a-contest-what-comes-back` ask 2, answered "ok": a contest
resets the sign-in settings the thief changed during the case to their value
when it opened (example: "ask for a second step after a passkey too",
`also-for-passkeys`).

- **One place.** `recovery_settings::RestoreSettings` is a `Settlement`.
  `Settlement::opened` runs when a case opens (not a succession), before the
  case is stored, and notes each `PersonSetting` in `$recoverysnapshot`
  (`case`, `kind`, `value`; null is a setting the person did not have). A
  failure stops the case opening. `Settlement::reset` runs in
  `Recovery::contest` right after each `Settlement::contest`. It reads each
  setting again and, where it differs from the note, writes the noted value
  back. A refusal from the store refuses the contest as unavailable with the
  directory untouched, and a retry is safe.
- **The settings.** Every per-person sign-in setting a signed-in person can
  change:

  | Kind | Stored in | Absent means |
  |---|---|---|
  | `second-step-level` | `$secondfactorpolicy` | row removed, the default `when-enrolled` |
  | `contact-email` | `$contactemail` | row removed |

  Remember-me and the session lifetimes are the app developer's and the
  tenant's, not a person's, so a recovered passkey cannot change them. Sign-in
  methods and second factors are covered by the removal above. A new
  per-person setting is added by implementing `PersonSetting` and listing it
  in `runtime.rs` where the service is wired.
- **Recorded.** One `contest-reset` per setting put back, in the person's
  stream and each tenant's: `case`, `principal`, `kind`, `old` (the thief's
  value), `new` (the value when the case opened). Words only: `none`,
  `when-enrolled`, `also-for-passkeys`, `address(<source>)`. An address is
  never recorded. `contested` adds `settings-reset=`. No mail goes to either
  address when the email is put back.
- **Notes.** A note holds the address, in the control database. Notes older
  than twice the contest window plus 90 days are pruned when a case opens. A
  case opened before this existed has no note and resets nothing.
- Tests. `tests/recovery.rs`:
  `a_contest_puts_back_the_settings_the_thief_changed` and
  `a_contact_email_the_owner_never_had_is_removed_by_a_contest`.

### What a contest undoes, 2026-10-25 (owner ruling)

Board `what-a-contest-undoes-after-a`, answered "ok": a contest removes every
second factor and passkey enrolled since the recovery case opened, and the
record names each one.

- **One place.** `Recovery::contest` asks each settlement's
  `Settlement::contest(principal, case, opened)` first, then
  `Directory::contest(by, at, opened)`. `RemoveSecondFactors` removes every
  factor whose `enrolled_at` is at or after the case's `opened_at` (TOTP,
  security key, and a key's credential with it). The directory removes every
  link whose `linked_at` is at or after it and that the case did not retire:
  passkeys, and any other sign-in linked since (a password or an outside
  provider's link alike). They are listed in `Contested::enrolled_since` and
  in `removed`, so the route removes their public keys as it does the
  recovery's own.
- **Backup codes** are recovery codes, and the contest already voids every
  unused one (F-1).
- **Recorded.** One `contest-removed` per thing removed, in the person's
  stream and each tenant's: `case`, `principal`, `kind`, `id`,
  `enrolled-since`. Never a secret: a TOTP is named by its `sf_` id, a passkey
  by its credential id. `contested` adds `enrolled-since-removed=`.
- **Order.** The second factors go before anything else changes. If their
  store fails, the contest is refused as unavailable and the retired passkey
  can contest again. The directory, the case, sessions and codes are separate
  stores and are not one database transaction; this keeps the same order of
  steps the contest had.
- **Never left with nothing.** The contest is made with a retired passkey,
  linked before the case and restored by it, so the owner always keeps that
  one. Other retired passkeys come back too, and links from before the case
  that the recovery did not retire are untouched.
- **What does not come back.** Completion removed every second factor the
  owner had before the case, and a contest does not restore them (their
  secrets are gone). The owner enrols again.
- `linked_at` is read from `$authenticator` (it was always written). Rows
  without one read as 0, before any case.
- Tests. `tests/recovery.rs`:
  `a_contest_removes_what_the_thief_enrolled_since_the_case_opened` (stolen
  code, the thief links a second passkey and enrols a TOTP after the
  quarantine; the contest removes and records both, the owner signs in with
  the restored passkey and owes no step, a spare linked before the case still
  signs in). `ordo-identity`:
  `a_contest_removes_links_made_since_the_case_opened` (a link from before
  the case that the recovery did not retire survives). The host test was run
  with the removal taken out, and failed.

### R5 · The deployment owner through the door, the last owner, succession

- **Who may.** The deployment owner, and nobody inside a tenant (owner,
  2026-09-29: *"Yes the deployment owner."*). The four ops are owner-only and
  step-up gated like their siblings, and answer only in the deployment's own
  space (`crates/ordo-host/src/identity_ops.rs`). A tenant's manager is not a
  member of that space and is refused by the door. A tenant's **owner** is
  refused too: the door lets them call the op in their own space and the op
  refuses any space but the home. No case is opened by either.
- `std.identity.recover {principal, evidence, reference?}` opens a case (path
  `operator`, authority `deployment-owner`). The evidence class is one of
  `vouched-by-members`, `verified-out-of-band`, `contract-holder`,
  `host-break-glass`, and the reference is opaque (at most 120 characters from
  a narrow alphabet), so no free text enters the stream. **The delay is a day**
  (`RecoveryPolicy::operator_delay_secs`), so an answer names when the case
  matures and carries no ticket. In that day any passkey the person still holds
  cancels it (`/auth/recover/contest`, with an assertion).
- `std.identity.recoveryTicket {case}` collects the ticket once the case has
  matured, once. `std.identity.cancelRecovery {case, reason?}` ends a case that
  has not completed, with a reason from a short fixed list.
- `std.space.succeedOwner {tenant, principal, evidence, reference}` opens a
  succession case (path `succession`, same delay); the same op with `{tenant,
  case}` after the delay makes the named **existing, active** member an owner.
  It admits nobody, touches no credential and ends no session, and the departed
  owner's membership stays for the tenant's new owner to remove. It is
  separate from recovery so the record says which of the two happened.
  A succession shares the `$recovery` table and nothing else: `look`,
  `complete` and `issue_ticket` refuse its cases, and it yields no ticket.
- **The record has two faces** (`Recovery::record_split`). The deployment's
  stream names the deployment owner who acted (`by=<principal>`), the evidence
  class and the reference, and says `checked=no`: the platform cannot verify
  what a deployment owner says they saw, so the class is recorded and the record
  says it was not checked. Each tenant's stream gets the same act under the
  subject `platform deployment-owner` and with `by=deployment-owner`, never the
  person and never another tenant. A succession is mirrored into the tenant it
  is about and no other.
- **The restored person is told.** `/auth/complete` carries `recoveries`, one
  entry per recovery of theirs they have not acknowledged (case, path, class of
  authority, class of evidence, state, end of the contest window), at every
  sign-in, until `POST /auth/recover/ack {"case"}` with a session that holds
  fresh presence. A quarantined passkey has none, so a person recovered a
  minute ago cannot clear it, and an intruder in their session cannot either.
  The acknowledgement is recorded. `$recovery` gained `space` and
  `acknowledged_at`.
- The ops are registered on the binary (`runtime::platform_specs`, which a
  test requires to name every op `platform_ops::specs` defines) and wired
  through a handle the authentication state fills, because the platform ops
  are built before it.
- Tests, `tests/recovery_ops.rs` (through the real door, Postgres):
  `a_sole_owner_is_restored_and_both_streams_say_so` (reads the deployment's
  stream and the tenant's, the notice, the refused and the accepted
  acknowledgement), `a_manager_cannot_recover_an_owner` (a manager and an owner
  of the tenant, in the tenant and in the home space, for recovery and for
  succession), `the_door_delay_holds_the_ticket_and_a_held_passkey_cancels`,
  `succession_touches_no_credential` (links, keys and sessions compared before
  and after, both streams read), `a_recovery_and_a_succession_are_not_interchangeable`.
  `runtime.rs`: `a_booted_deployment_serves_the_deployment_owners_recovery_ops`.
  With the home-space check removed the manager test fails; with the tenant
  subject naming the deployment owner the sole-owner test fails.

Deviations and gaps, stated:

- **The delay through the door is a day**, which is §4.4's default and not
  something the owner has ruled. It is a setting and the numbers are separate.
- **`vouched-by-members` is recorded and not checked.** Making it checkable is
  members vouching with presence and the case waiting for them, which is
  multi-party approval (R7).
- **`std.identity.releasePresence` (§7) is not built.** It lifts a
  quarantine early, and the owner's ruling is a day's block, adjustable by a
  setting. A per-person lift is not named by an increment and is left for the
  owner to ask for.
- **The objection is built** (R5b, 2026-10-24).
- The host path (`ordo identity recover`) still does not mirror into tenants.
  The door path does.

### R5b · Managers told and may object; succession ends grants (built 2026-10-24)

Owner, 2026-10-24, the three succession asks, each "Ok" on the
recommendation (`corpus-rulings.md`, "Owner succession").

- **Which cases.** A succession of a tenant, and a recovery through the
  door (path `operator`) of a person who is an owner of the tenant. The
  second is the last-owner recovery of section 6, which this plan gives the
  same objection rule, so both are built. A recovery by code, by a tenant or
  at the host is not covered.
- **Told.** Each active **manager** of that tenant is told in-band:
  `/auth/complete` and `/auth/refresh` carry `owner_cases` (case, tenant,
  path, principal, evidence, state, `matures_at`, `paused`, `objected`),
  next to `recoveries`, while the case is `pending` or `matured`. No
  acknowledgement: the notice ends with the case. A case about the manager
  themselves is left out.
- **Object.** `std.space.objectToCase {case}`, in the tenant. The door lets
  a manager reach it (`platform_ops::manager_reachable`); the op refuses
  anyone but an **active manager** (an owner is refused and recorded,
  `reason=not-a-manager`). One objection per manager per case. An open
  objection **pauses** the case: `mature_succession`, `issue_ticket`,
  `look` and `complete` all refuse (`case-paused`), and the refusal names
  `std.identity.resolveObjection`.
- **Resolve.** `std.identity.resolveObjection {case, decision, reason}`,
  the deployment owner, home space. `resume` with `evidence-rechecked`,
  `objector-withdrew` or `objector-answered`; `cancel` with
  `objection-upheld`, `opened-in-error` or `not-verified`. It resolves every
  open objection at once. A resume does not restart the delay: the case
  goes on from where it was, so one past its delay may be completed at
  once.
- **Recorded.** `objected` goes to the deployment's stream and to the
  objecting tenant's only, naming the manager (a member there).
  `objection-resolved` goes to the deployment's (`by=<principal>`) and to
  the case's tenants (`by=deployment-owner`). Objections are rows in
  `$recoveryobjection` (case, objector, space, at, resolved_at,
  resolution, reason, resolved_by).
- **Grants end.** The deployment owner names who has left when the case
  opens: `departed`, a required list of 1 to 10 principal ids, each an
  owner of the tenant then, never the successor (`$recovery.departed`).
  Only those named are departed: a living co-owner never loses anything
  because someone else's succession completed (coordinator, 2026-10-24).
  The managers' notice carries `departed`. Completing it ends,
  for each departed owner, every live delegation they granted in the
  tenant and the grant to the app that delegation's consent minted
  (`consent::Consents::end_all_of`, the code `std.delegation.revoke`
  uses). The completing call's `endGrants` decides; unset, the
  deployment's `ORDO_SUCCESSION_ENDS_GRANTS` (default `on`).
  `owner-succeeded` carries `ends-grants`, `grants-ended=<id>@<agent>`,
  `delegations-ended=<id>@<agent>` and `not-ended`.
- **The console.** `std.identity.openCases` (a read, home space) lists
  every open door recovery and succession with maturity, approvals,
  departed owners and each objection with its resolution. The Tenants
  screen's Owner succession section shows the successions and opens one
  from the tenant, the successor's typed id, who has left (typed ids),
  evidence and reference, with no member list. Resolve and complete are dialogs.
- Tests: `recovery_ops.rs`
  `a_managers_objection_pauses_a_succession_until_the_deployment_owner_resumes_it`,
  `a_cancel_ends_a_paused_case_and_a_last_owner_recovery_shares_the_rule`,
  `completing_a_succession_ends_the_departed_owners_grants_unless_told_not_to`;
  `auth_durability.rs` `an_objection_and_its_resolution_survive_a_restart`;
  `ordo-console` `tenants.rs`
  `succession_lists_opens_resolves_and_completes_with_both_touches`.

Deviations and gaps, stated:

- **"Grants to apps" are the grants a delegation's consent minted.** These
  are the only app grants the registry holds. Share grants a departed owner
  holds are not touched.
- **The notice is not recorded per delivery.** Each step of the case is.

### R6 · A tenant's part: recovering a member it created, and hold

- **`std.space.recoverMember {member, case?}`** (`identity_ops.rs`), for a
  tenant's owner, in the tenant. It works only for a person **the tenant
  created, who belongs nowhere else, and who is not an owner**. The first two
  are the rule the password reset already uses (`credential_ops::authorise_managed`,
  shared: one refusal for never managed, released, and belongs elsewhere,
  checked against every database served, failing closed). The third is the
  owner's ruling: an owner is restored by the deployment owner and by nobody
  inside the tenant, another owner included. It opens a case (path `tenant`,
  authority `tenant-administrator`) and, with a tenant delay of zero, returns
  the ticket at once. With `case`, after a delay, it collects the ticket.
- **Every refusal is one sentence**, and it tells the administrator to refer
  the person to the deployment owner. It does not say whether the person
  belongs to another tenant or is an owner, so a refusal learns an
  administrator nothing about who else a person belongs to (`K3`). The reason
  class (`not-managed-here`, `target-is-owner`) is recorded as `refused` in the
  stream of the tenant whose administrator was refused, and nowhere else.
- **Hold on recovery** (`recovery_hold.rs`). `std.space.setRecoveryPolicy
  {hold}`, a tenant owner's op, writes `$recoverytenant`. A tenant that has not
  chosen takes `RecoveryPolicy::hold_default`, which is **off** (owner,
  2026-09-29: a recovered person is not locked out of each tenant). With hold on,
  a `Settlement` suspends the recovered person's membership in that tenant
  **when the recovery completes**, not when it is opened, so a cancelled or
  contested case holds nobody. The tenant's own owner or manager reactivates
  them with the ordinary, step-up-gated `std.space.reinstate`. A tenant's **only
  active owner is never held**, since nobody would be left to release it: the
  owner rows are locked in the statement that writes, as the membership
  commands' last-owner guard does. The hold is recorded as a membership
  suspension in the tenant's stream.
- Wired in the runtime where credentials are (`$managed` exists there), and
  the hold settlement is registered where recovery is built, over the door's
  engine for who belongs to which tenant.
- Tests, `tests/recovery_ops.rs`: `an_administrator_recovers_a_managed_member`,
  `the_same_administrator_is_refused_once_the_member_joins_a_second_tenant`
  (three different reasons, one status, one code and one sentence, the reasons
  read from the tenant's stream and absent from the other tenant's),
  `a_held_tenant_stays_closed_until_its_own_owner_reactivates` (a tenant that
  holds, one that did not choose, the only active owner, and the reinstating).
  With the owner check removed the refusal test fails; with the hold skipped
  the hold test fails.

Deviations and gaps, stated:

- **A tenant's owner opens path C, and a manager does not.** Whether a manager
  may is open question 5, and the owner alone is inside every answer to it.
  The recommendation, members below manager, is not built.
- The administrator named in a tenant-path record is the member id the door
  hands an op (`by=mem_olga`), as the other platform ops name theirs. The
  plan's `RecoveryRefused` event is one act of the `Recovery` kind
  (`refused`), as in R3.
- The tenant's delay for its own members is zero and is the deployment's
  setting. A tenant raising it for itself is R7.

### R7 · Policy: the deployment's floors, a tenant's tightening, approvals

Owner, 2026-09-29: *"We do not enforce an app developer's choices but we can
have it as default."* Every behaviour below is a setting with a default, and
the default is the safe or the ruled one.

- **The deployment's settings** (`RecoveryPolicy::from_env`, `ORDO_RECOVERY_*`,
  read with the rest of `RuntimeConfig` before anything is dialled). Each is
  **refused rather than clamped**, naming its variable: an unparseable value, a
  value below its floor, an approvals count outside 1 to 10, a word that is
  not a path, and a **quarantine longer than the contest window**. The
  defaults are the owner's where the owner ruled (a day's quarantine, a code
  that works at once, no hold) and the plan's where he did not (the door's
  one-day delay). The startup log prints what was settled. Settings and their
  defaults are tabled in `docs/deploying.md`.
- **Floors.** The quarantine, the door's delay and the succession delay may
  not go below `3600` seconds. A quarantine of nothing would make step-up
  exactly as strong as the weakest recovery path.
- **A tenant can only tighten** (`std.space.setRecoveryPolicy`,
  `recovery_hold.rs`): `hold`, a longer `delaySecs`, more `approvals`,
  `pathEnabled` (its administrators' recovery of its members) and
  `codesEnabled` (codes for the people it created). A delay or an approvals
  count below the deployment's is refused with the floor in the sentence, and
  the case waits the longer of the two. Only what a call names changes, and a
  tenant that changed one setting still takes the deployment's default for
  the rest (`hold` is null until chosen). A tenant's switch on codes is read
  through a gate at the two places codes are used, and fails closed.
- **Several people agreeing** (`Recovery::approve`, `$recoveryapproval`).
  `std.identity.approveRecovery` for a case through the door or a succession,
  `std.space.approveMemberRecovery` for a tenant's case. The opener is the first
  approval. Neither the person the case is about nor somebody who has approved
  already may, and each is recorded. Until it has enough approvals a case yields
  no ticket and cannot be carried out, whatever its delay.
- **Paths can be switched off** (`ORDO_RECOVERY_PATHS`); a succession follows
  the door's path. The host command reads the same settings.
- Recovery now names people by **principal**, not by the member id the door
  hands an op: the ops resolve it once, so an approval counts a person once
  and every stream names them the same. Earlier records in this section that
  say `by=mem_olga` read `by=olga`.
- Tests. `tests/runtime.rs` (against the binary, before any dial):
  `a_quarantine_longer_than_the_contest_window_refuses_the_boot`,
  `a_delay_below_the_floor_refuses_the_boot` (three variables),
  `a_bad_recovery_setting_refuses_the_boot_by_name`. `tests/recovery_ops.rs`:
  `a_tenant_cannot_lower_a_deployment_floor`,
  `two_approvals_required_means_one_is_not_enough`,
  `a_tenant_may_switch_off_its_administrators_recovery_and_its_members_codes`.
  Unit tests in `recovery.rs` (`policy_tests`): the defaults are the owner's
  rulings, a floor is a refusal and not a clamp, the quarantine and the
  window, the rest by name, the startup line. Each fails with its check removed.

Deviations and gaps, stated:

- **The floors and the door's delay are the plan's numbers**, and the owner's
  answer on the quarantine (*"a day's block ... adjustable"*) did not name a
  floor. They are constants (`recovery::FLOOR_SECS`) and easy to move.
- **A tenant raising the quarantine for ops in its own space is not built.**
  The plan lists it. The quarantine lives in the token's presence, which does
  not say which tenant an op is in, so it would need the door to check a
  passkey's quarantine per space. No open question depends on it. It is
  unscheduled.
- The plan's path E (email) has no setting, since no channel exists.
- Approvals for a case through the door count deployment owners, and for a
  tenant's case its owners. Managers approving depends on question 5 and is
  not built.

### R8 · Reconciliation: `ordo verify --identity`

- `ordo verify --identity` (`crates/ordo-cli/src/identity.rs`, `operator.rs`)
  verifies the deployment's own security stream (the chain, with the keys the
  other `verify` modes take) and then reconciles **every link in
  `$authenticator`** against it. Nothing that can write the table can be stopped
  from writing a row, and a recovery design cannot change that. What it can do
  is make the row detectable (T3): every way a link comes to exist writes an
  event to a signed stream, so a link no event explains is one somebody wrote by
  hand. It is named, with its principal and its time, and the run **exits 3**,
  the code for a record that is wrong, because the stream is the artifact meant
  to be believed when a table is in doubt.
- What explains a link is each way one is made and nothing more: an
  `AuthenticatorLinked` event naming it (sign-up, a ticket, a link ceremony); a
  `Recovery` `completed` that names it as `added`; for a `password:` link an
  enrolment act that gave the person a password (`credential-set`,
  `temporary-password-set`); and `PrincipalOnboarded`, which explains **one**
  link of the principal it names, the first, and no more. Without that last
  limit a passkey attached to an onboarded person's account would look
  explained.
- The report also says where retention has trimmed the stream, since a link
  older than the retained position shows as unexplained, and `--identity`
  refuses `--space`.
- Tests, `crates/ordo-cli/tests/identity.rs`:
  `a_link_row_inserted_by_hand_is_reported` (a link with no event exits 3 and is
  named with its principal, the explained one is not listed, the chain is
  verified beside it, and an event that explains it makes the run clean),
  `every_way_a_link_is_made_explains_it_and_an_onboarding_explains_one`,
  `identity_reconciliation_takes_no_space`. With the reconciliation made to
  find nothing the first two fail.

Deviations and gaps, stated:

- The plan's list is three events. The tree has a fourth way a link is made
  (a password given, E6), which writes an enrolment act and not a link event,
  so it is a rule here too.
- The reconciliation reads the deployment's stream only. A link recorded in a
  tenant's stream alone would show as unexplained, and none is: credentials are
  the deployment's and every link's event is written at deployment scope.
- It is a command, not a worker. Somebody has to run it, on a schedule if they
  want the detection to be timely.

### The SDK (after R8, not an increment)

`clients/ordo-sdk/src/auth/recovery.ts`, exported from the package: the
routes of R1 to R4 and R5's acknowledgement as functions a page calls
(`listAuthenticators`, `linkPasskey`, `removePasskey`, `endEverySession`,
`issueRecoveryCodes`, `redeemRecoveryCode`, `recoverWithTicket`,
`contestRecovery`, `acknowledgeRecovery`), and `PasskeySession` now carries
`notices`, `presenceAvailableAt` and `recoveries` when the door sends them.
`passkey.ts` exports the calls it already made (`call`, `assertion`, and a new
`attestation` extracted from `registerPasskey`), and `call` reads a `204` as an
answer with nothing to say. `clients/ordo-sdk/test/recovery.test.ts` (16 tests,
with `navigator.credentials` stubbed, so no gesture is faked) checks the order
of the routes, the bytes posted, that a code and a recovery ticket carry no
token, and what a sign-in's notices read as. `npm run check`: 114 tests pass
(98 before). The deployment owner's and a tenant owner's ops are platform ops
and are called through the generated client like any other, so the SDK adds no
wrapper for them.

### Managers recover members, and outside providers (checklist 23c)

The owner's answers of 2026-09-29 (`corpus-rulings.md`, "Recovery details")
gave two things to build.

**1. A tenant manager may recover a member when the app developer sets it.**

- The setting is `ORDO_RECOVERY_MANAGERS`, `on` or `off`, default `off`
  (`RecoveryPolicy::managers_may_recover`, `crates/ordo-host/src/recovery.rs`).
  It is read and refused like the other `ORDO_RECOVERY_*` settings and appears
  in the startup line. It is a row in `docs/deploying.md`.
- Layers: the deployment sets it, and a tenant may turn it off for itself with
  `std.space.setRecoveryPolicy` `managersMayRecover: false`
  (`$recoverytenant.managers_enabled`, default true). A tenant cannot turn it
  on when the deployment left it off, and the op says so. A manager recovers a
  member only when both are on. The answer of `setRecoveryPolicy` reports the
  effective value.
- The door now lets a manager call `std.space.recoverMember` and
  `std.space.approveMemberRecovery`
  (`platform_ops::manager_reachable`, one additive line in `lib.rs`). The
  grant alone opens nothing: `IdentityOps::administrator_may`
  (`crates/ordo-host/src/identity_ops.rs`) checks the caller's role, both
  settings and the member's role on every open, approval and ticket
  collection. A manager recovers a member below manager. A peer, an owner and
  the manager themselves are refused, and the stream records which reason
  (`target-not-below-manager`, `managers-not-allowed`, `target-is-self`).
  The manager is told one sentence whichever it was, so a refusal does not
  disclose a peer's role.
- A single-person tenant: its owner is the only member, an owner is never
  recovered by the tenant, and nobody recovers themselves. The op refuses,
  no case is opened, and the setting can still be read and changed.
- Tests, `crates/ordo-host/tests/recovery_ops.rs`:
  `a_manager_may_not_recover_a_member_unless_the_app_developer_set_it`,
  `a_manager_recovers_a_member_below_manager_when_the_setting_is_on`,
  `a_single_person_tenant_recovers_nobody`, and the parse test in
  `recovery.rs`. The second was seen failing with the door grant removed.

**2. A sign-in that belongs to an outside provider is recovered by that
provider.** The owner: "third party is managed elsewhere."

- **How an outside-provider link is recognised.** The directory has no column
  for it and needs none. An authenticator's id is spelled by the provider that
  mints it, `oidc:<connection>:<sub>`, `saml:<connection>:<name-id>`,
  `oauth2:<connection>:<sub>` (`provider.rs`, and `plan-auth-federated.md`),
  and nothing a person or a tenant supplies reaches that prefix. So the marker
  is the spelling, read by the new
  `AuthenticatorId::is_outside_provider` (`crates/ordo-identity/src/provider.rs`).
  No schema change was made. If the federated build later wants the provider
  kept as a column, this function is the one place to change.
- `Directory::recover` never retires an outside provider's link when the
  person has other sign-ins, refuses a `lost` list that names one, and refuses
  a person whose links are all an outside provider's
  (`DirectoryError::RecoveryRefused(OUTSIDE_PROVIDER)`), so a completion cannot
  give such a person a passkey the provider never granted. A contest restores
  only what a recovery retired, so it leaves them alone too. Nothing in
  recovery unlinks or replaces one.
- `Recovery::open` (all four paths, so the door, a tenant, a recovery code and
  `ordo identity recover`) refuses a person whose only sign-ins are an outside
  provider's, before any case exists: `OpenError::OutsideProvider`, with the
  sentence "this person signs in through an outside provider, which manages
  that sign-in. Recover it at the provider; Ordo does not". A directory that
  cannot be read opens nothing. Owner succession is not held back, since it
  touches no credential. The runtime gives `Recovery` the directory
  (`with_directory`).
- Outside providers are not built yet, so no deployment has such a link. The
  tests write the links straight into the directory.
- Tests: `ordo-identity` `passkey_tests.rs`
  `an_outside_providers_link_is_recognised_by_its_spelling`,
  `recovery_leaves_an_outside_providers_link_alone`,
  `recovery_refuses_a_person_whose_only_sign_in_is_an_outside_providers`;
  `ordo-host` `recovery_ops.rs`
  `no_recovery_is_opened_for_a_person_whose_only_sign_in_is_an_outside_providers`,
  `a_recovery_leaves_a_persons_outside_provider_sign_in_alone`. Each was seen
  failing with `is_outside_provider` answering false.

## What waits on the owner

Nothing below was built, because each depends on a question the owner has not
answered. Each is with the plan's recommendation (`§11`). The board asked the
owner five recovery questions and they map to the plan's 1, 2, 4, 7 and the
last-owner ruling. Questions 3, 5, 6, 8 and 9 of the plan are still open.

| | Question | The plan recommends | State here |
|---|---|---|---|
| 3 | May the host-side recovery skip the delay? | **Yes.** Whoever runs it holds the database and the master key, so a delay restrains nobody | Built to the recommendation as a default (`ORDO_RECOVERY_HOST_DELAY_SECS=0`), and either answer is one setting. Confirm or flip the default |
| 5 | May a manager, not only an owner, open path C (recover a member the tenant created)? | **Members below manager, not peers or owners**, so lost phones do not push tenants to hand out owner | Answered 2026-09-29 (the app developer's setting, off unless set) and built, see "Managers recover members, and outside providers" above |
| 6 | Should a recovery revoke the capability grants and delegations the person consented to? | **No by default, and show them** the list with the date of each so the person can withdraw any | Not built. A recovery ends sessions and touches no grant, which is the recommended default. Showing the list, and the middle course (revoke grants made within N days before), wait |
| 8 | Who may object to a last-owner recovery? | **Managers of that tenant only**, since they know whether the owner really lost a key | Answered 2026-10-24 (managers only) and built, see R5b |
| 9 | What happens to a tenant whose last owner cannot be restored and has no successor? | **Leave it locked and intact**, and decide ending or exporting it under the exit ops' own rules | Nothing is built, which is that recommendation. `std.space.succeedOwner` covers the case where there is a successor |

Also confirm, since the owner ruled the direction and not the numbers: the
one-hour floors, the one-day delay through the door and for a succession, the
three-day contest window, and one day of quarantine after **every** path (the
plan proposed three days after an assisted one).

## Not built, and no question in the way

- **R9**, the `RecoveryChannel` seam, as instructed: it waits on a mail channel
  from `plan-auth-federated.md`.
- **`std.identity.releasePresence`** (§7), lifting a quarantine early for one
  person with a recorded reason. No increment names it and the owner ruled a
  day's block adjustable by setting.
- **A tenant raising the quarantine for its own ops** (§4.4). The quarantine is
  in the token's presence, which does not say which tenant an op is in.
- **Vouching as checkable evidence.** `vouched-by-members` is recorded and not
  checked. Approvals (R7) are the mechanism that would check it.
- The second-factor plan's use of the same codes (`used_as: second-step`, the
  per-person lock) and its removal of every second factor on a recovery. The
  `Settlement` seam is built and waits for a second factor to register one.
- Vault-space data recovery (`§9`). It waits on F7.

## What the plan got wrong against the code

- R1: the per-principal epoch, its table, its check at the door and in
  `refresh`, and its refresh worker were already built (E6). R1 was the route.
- R2: the dropped audit events (`§0.5`) were fixed by S0, and the BE and BS
  flags by S1. The token does not say which authenticator made its presence,
  so a link cannot be built from a token; link and unlink carry their own
  assertion.
- §0.4: `Directory::link` and `unlink` still had no caller before R2, but the
  auth router had eleven routes, not four.
- `§8.1`: nine event variants would each have changed the chain's encoding.
  Two kinds (`SessionsEnded`, tag 16, and `Recovery`, tag 17) carry every act,
  as `Enrolment` does.
- `§4.1` puts a case id on the ticket. A person has one live case, so the ticket
  names the case by naming the person and needs no column.
- `§3.2` keys the code hash with an HMAC under a deployment key. At 120 bits a
  plain hash is as strong, and a keyed one would void every printed set at the
  key's rotation. It waits on the key service (18y).
- `§10` says R8 reports a link with no `PrincipalOnboarded`. That event
  explains one link, not all of a principal's, or T3 goes unseen.
- R3 mounted its routes on the auth router the harnesses use and not on the
  platform router the binary serves. R4 found it and fixed it; the tests that
  boot the binary are what keep it fixed.
