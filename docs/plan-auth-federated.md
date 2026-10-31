# Ordo federated and email sign-in: OIDC, OAuth2, SAML and email one-time code

This plan reflects the owner's answers of 2026-09-29 (`corpus-rulings.md`, *Sign-in through outside providers and email*, all six, and the amendment on two-step claims given later the same day).

**Revised 2026-10-08** for the last step of the onboarding order the owner ruled that day (*"Execute."*: JSON fields, delegation for workers, triggers, a plan for backend functions, **then emailed-code and OAuth sign-in**). What changed is listed under *What changed on 2026-10-08* below, and each changed section says so.

## On one screen

**What this is.** Three new ways to sign in, beside passkey and password: an emailed code, an outside provider (Discord, Google, a company login), and, if you say yes, a guest sign-in. SAML stays last.

**What denizens uses today** (read 2026-10-08, `denizens-web/src/auth/LoginPage.tsx`):

| Supabase call | What it is | Ordo |
|---|---|---|
| `signInWithOtp` + `verifyOtp` | an emailed 8-digit code (the page also says "link") | **step 5** |
| `signInWithOAuth` | **Discord only**, scopes `identify email guilds` | **step 3** |
| `signInWithPassword` | username and password, through a made-up email | exists (password) |
| `signInAnonymously` | "Continue as Guest", behind the app's own on/off setting | **step 8, if ruled** |

All four sit behind Cloudflare Turnstile. No Google, no company login, no SAML.

**The build, in denizens' order** (§10):

| Step | What | Tier | Size |
|---|---|---|---|
| 1 | Handoff and binder on the ceremony table; trait changes **built** | Opus | M |
| 2 | `fetch` in the egress kernel, shared with backend functions **built** | Opus | M |
| 3 | **OAuth2 served, Discord preset**, providers as settings **built** | Opus | L |
| 4 | **The mail channel**: SMTP, Resend, SendGrid **built** | Opus | M |
| 5 | **Emailed code** | Opus | M |
| 6 | Optional human check (Turnstile) **built** | Sonnet | S |
| 7 | Linking by claim **built** | Opus | M |
| 8 | Guest sign-in, off by default **built** | Opus | M |
| **9** (built, `556b051d`, `ae9ba8b1`) | OIDC: Google and a company login | Opus | L |
| **10** (built, `e7c27656`) | Two-step claims as presence | Opus | M |
| later | tenant-owned providers, SAML, more mail APIs | Opus / Sonnet | L / S |

**Five choices made here:**

- **Mail goes through Ordo's own mail setting, not backend functions.** Sign-in must work before any function exists, and no app code may ever see a code (§7.6).
- **No limit uses a visitor's address** (`proxy-mode`, 2026-10-06). Limits count per ceremony, per recipient address and per deployment (§7.4).
- **OAuth2 before OIDC**, because Discord is OAuth2 and denizens needs nothing else.
- **A deleted account stays refused on every new path.** Each path resolves through the one check that already refuses it (§2.7).
- **Guest sign-in is in scope, off by default**, if you agree (§7A).

**Your decisions** are §13: thirteen questions, each with a one-line recommendation.

### What changed on 2026-10-08

| Section | Change |
|---|---|
| top | this summary |
| §0.5 | new: what the code gained since 2026-09-29 (ceremony table, recorder, admission, sign-up ceiling, deleted accounts) |
| §1 | the six rulings quoted one by one, with how each is honoured; later rulings that reach this plan; §1.2 what denizens uses |
| §2.6, §2.7 | new: the first sign-in under self-serve provisioning; a deleted account cannot come back |
| §3.3, §3.5 | `source` removed from `begin`; the private-host list is the backend functions plan's `ORDO_EGRESS_PRIVATE_HOSTS` |
| §5.3 | new: providers as settings, with presets; Discord first |
| §7.4 | limits rewritten without client addresses: E6 and E14 withdrawn, E7 narrowed; E15 human check and E16 deleted accounts added |
| §7.5 | a row for a deleted account |
| §7.6 | the mail choice for v1, argued against backend functions |
| §7A | new: guest sign-in |
| §8.1 | presence on platform ops is the app developer's setting (ruled 2026-09-29) |
| §10 | increments renumbered in denizens' order, with tier and size; the old F numbers are mapped |
| §11, §13 | answered questions compacted in §11; every open one is in §13, which ends the plan |

---

Companion to `plan-passkey-signin.md` (built first; it creates `$authceremony` in the shape §3.1 defines and makes the auth routes record, which was F0), `plan-second-factor.md` (§5.3 reads the two-level rule of §8.2 for the second step), `security-logging-design.md` (§1 to §5 and §7b), `definition-language.md` §8 (*Login methods*, *Who creates the row*), `roadmap-checklist.md` **20b**, **21b** and **17r**, and the board entries `auth-bootstrap`, `membership-commands` and `durable-auth` in `docs/decisions.html`.

Markers: **[settled]** restated from a settled document or from an owner ruling · **[plan]** designed here, not built · **[open]** the owner's to decide, with a recommendation.

Status: **planned 2026-09-29, revised 2026-10-08, steps 1 to 7 built (step 7 on 2026-10-09), the rest not built.** Every claim about the code in §0.1 to §0.4 was read from the code on 2026-09-29, at commit `db1192b`; line numbers there have moved. §0.5 was read on 2026-10-08 at `76c88fbf`. Claims about third-party crates that are not in `Cargo.lock` come from memory and are marked **(unverified)**; each must be checked with `cargo tree` in a scratch workspace before the increment that depends on it starts.

Reading order for a reviewer short of time: the summary above, §1 (the rulings), §7.4 (limits), §10 (the build), §13 (decisions).

---

## 0 · The starting state, verified

### 0.1 What exists

| Claim | Where |
|---|---|
| Six method families are named: Password, EmailOtp, Passkey, Oidc, Saml, OAuth2 | `crates/ordo-identity/src/principal.rs:482-490`; wire words at `:492-503` |
| The server starts with `password` only. Any other word in `ORDO_AUTH_METHODS` is a refused boot | `auth_from_env`, `crates/ordo-host/src/runtime.rs:367-385` |
| The boot loop that builds the registry has one arm and asserts it | `runtime.rs:1999-2003` (`debug_assert_eq!(method, "password")`) |
| `ORDO_AUTH_PROVISIONING` is one deployment-wide boolean | parsed at `runtime.rs:410-419`, applied at `runtime.rs:2021-2027` |
| `AuthProvider` is synchronous, takes the server clock, and `begin` cannot fail | `crates/ordo-identity/src/provider.rs:244-252` |
| `StubOidcProvider` models state and nonce and performs no I/O | `provider.rs:601-702` |
| Four routes: `/auth/begin`, `/auth/complete`, `/auth/logout`, `/auth/refresh` | `crates/ordo-host/src/auth.rs:533-540`, mounted at `serve.rs:337-340` |
| One refusal for every authentication failure; a store outage is the only distinct answer | `auth.rs:164-183`, `auth.rs:273-277` |
| The directory links an authenticator id to a principal, in two durable tables | `crates/ordo-host/src/principal_directory.rs:63-86` (`$principal`, `$authenticator`) |
| `resolve_or_onboard` mints a human principal under `AutoOnboard`, refuses `NotInvited` under `InviteOnly` | `crates/ordo-identity/src/directory.rs:682-710` |
| Linking a second authenticator requires a fresh presence assertion from an authenticator already linked | `LinkCeremony::from_claims`, `directory.rs:227-246`; `Directory::link`, `directory.rs:550-586` |
| `UserPresence` has private fields and no constructor outside `passkey.rs` | `crates/ordo-identity/src/passkey.rs:69-72` |
| A presence-less ceremony cannot refresh presence | `session.rs:555-559` (`MethodCannotProvePresence`) |
| Every platform op is step-up gated | registration loop, `crates/ordo-host/src/lib.rs:1889`; owner-only list at `platform_ops.rs:877-912` |
| A token names a principal and no space | `TokenSpec`, `crates/ordo-identity/src/token.rs:141-172` |
| Principals are deployment-wide; membership is per space | `$principal` in the control database (`principal_directory.rs`); `members` per space (`definition-language.md` §3, *$caller ↔ member reconciliation*) |
| The egress kernel enforces six pins by type | `crates/ordo-egress/src/lib.rs:40-49` |

### 0.2 What does not exist, and how each absence was checked

| Absence | Search |
|---|---|
| No mail-sending code | `grep -rniE "smtp\|sendmail\|lettre\|mailgun\|send_mail\|send_email\|mailer"` over `crates/**/*.rs` and every `Cargo.toml`: zero files |
| No JWT, JWKS or discovery code | `grep -rniE "jwks\|jwt\|openid-configuration\|id_token"` over `crates`: two files, both prose (`provider.rs` doc comments, one test comment in `ordo-host/tests/http.rs`) |
| No SAML or XML code | `grep -rniE "saml\|\bxml\b"`: four files, all enum names or doc comments |
| No HTTP client, XML, JOSE, RSA or mail crate in the lockfile | `Cargo.lock` has 226 packages; zero entries named `reqwest`, `ureq`, `quick-xml`, `roxmltree`, `samael`, `openidconnect`, `jsonwebtoken`, `oauth2`, `rsa`, `lettre`, `url` |
| No rate limiter | `grep -rni "rate.limit\|ratelimit\|throttle"` over `ordo-host/src`, `ordo-api/src`, `ordo-identity/src`: two hits, both about worker ticks |
| No reading of the caller's network address | `grep -ri "x-forwarded\|ConnectInfo\|peer_addr\|remote_addr"` over `ordo-host/src` and `ordo-api/src`: zero hits |
| No invitation code | `grep -rni "invit"` over `ordo-host/src`, `ordo-exec/src`, `ordo-api/src`: only the `invite-only` policy word and comments |
| No served way to add a member | `roadmap-checklist.md` 21b: "the only production writer of `members` is provisioning's founding owner" |
| The `auth { … }` block is lexed and not parsed | `auth` is a keyword (`ordo-syntax/src/token.rs:130`, `lexer.rs:815`); `grep -n "Auth" crates/ordo-syntax/src/*.rs` finds the token definition and nothing in `parser.rs` |

### 0.3 Five things the code does today that this plan has to change

These were found while verifying the brief. Each one shapes an increment.

1. **The egress client cannot fetch.** `ordo_egress::deliver` writes a `POST` (`client.rs:220`), signs it with a per-sink secret, and returns `Accepted { status }` and nothing else (`client.rs:124-126`). The crate's own doc states that no byte a destination sent reaches a caller (`lib.rs:65-75`). OIDC discovery, JWKS, the token exchange, OAuth2 userinfo and SAML metadata all need the response body. Going through the rail therefore means adding a second, deliberately narrow function to it (§3.2). The response reader also has no chunked-transfer decoding (`grep -ni "chunked"` over `client.rs`: zero hits), because today it discards the body.

2. **Ceremony state is process-local.** Every provider holds its pending challenges in a `Mutex<BTreeMap>` field (`provider.rs:453`, `provider.rs:608`, `passkey.rs:373`). A redirect flow leaves the process for seconds to minutes and returns through a load balancer. With two replicas, half of all callbacks would land on the replica that never saw the `begin`. `security-logging-design.md` §4 says horizontally scaled pods are one logical platform, so ceremony state has to be durable before any redirect method is served (§3.1).

3. **The OIDC completion payload carries the identity in the request.** `CompleteRequest::OidcCallback` has `subject` and `nonce` fields (`provider.rs:96-101`) and `parse_complete` reads them from the caller's JSON (`auth.rs:494-499`). That is correct for a stub and is a sign-in-as-anyone hole the moment it is wired. `auth_from_env` refuses the method for this reason. The fields are removed in F1.

4. **The auth routes record nothing.** `auth.rs`'s module doc says refusal reasons "go to the security stream through the recorder" (`auth.rs:50-52`). `grep -n "recorder\|secevent\|audit" crates/ordo-host/src/auth.rs` returns that doc line and nothing else. `AuthState` holds no recorder, and `complete` drops `signed.audit` (the `PrincipalOnboarded` and `AuthenticatorLinked` events `sign_in` returns) at `auth.rs:282-292`. Federated sign-in multiplies the number of reasons a sign-in can fail, and a deployment owner debugging an IdP integration with a single-answer route and no record has nothing to read. F0 closes this first, and F0 is built as `plan-passkey-signin.md` S0, which lands before any increment here.

5. **The display name is whatever the caller posts.** `complete` reads `display` from the request body (`auth.rs:278-281`). `definition-language.md` §3 settles that the name comes from the identity service's profile (the OIDC `name` claim, then `preferred_username`, then a generated handle). For a federated sign-in the name must come from the verified assertion.

Two smaller discrepancies, reported so somebody corrects them:

- `principal.rs:479-481` says "there is no sixth family" above an enum with six variants.
- `security-logging-design.md` §2 lists email OTP under `LocalPasswordProvider`. This plan gives it a provider of its own (§7), because it shares nothing with password verification except the single-answer rule.

### 0.4 What this plan depends on and does not own

| Dependency | Owner | State |
|---|---|---|
| Passkey served, relying-party id | `plan-passkey-signin.md` S0 to S5 | planned, built first. S0 is F0; S1 creates `$authceremony` and `CeremonyStore` |
| First-owner bootstrap | `plan-enrolment.md` E5 (first-comer, claim code as a setting) | planned |
| Passkey recovery | `plan-passkey-recovery.md` | planned, built after enrolment |
| Password enrolment and reset | `plan-enrolment.md` E2, E6 | planned, built in parallel with passkey sign-in |
| A command that writes a `members` row | 21b, `membership-commands` | open, the owner's |
| Notifications and routes | 17r | settled spec, zero code |

**Nothing in this plan unlocks the deployment's own operations by default.** Passkey does. A deployment serving only the methods planned here has people who can sign in and use applications, and, under the default, nobody who can call `std.deployment.apply`. The app developer may lower that requirement (§8.1, revised 2026-10-08). §8 states this in full.

### 0.5 What the code gained since 2026-09-29 [revised 2026-10-08]

Read at `76c88fbf`. Each row changes what an increment here has to build.

| Then (§0.1 to §0.3) | Now | Where | Effect on this plan |
|---|---|---|---|
| Ceremony state in a process map (§0.3 item 2) | `$authceremony` and `CeremonyStore` exist, with `handle`, `connection`, `phase`, `binder`, `payload`. Passkey uses them; `binder` is empty for it | `crates/ordo-identity/src/ceremony.rs` | Step 1 adds the `handoff` phase and fills `binder`. No new table |
| Auth routes record nothing (§0.3 item 4) | `AuthState` holds the recorder | `crates/ordo-host/src/auth.rs` (`with_recorder`) | F0 is done |
| `password` only | `password` and `passkey`. Anything else refuses the boot, naming OIDC as a stub (since step 9 an OIDC connection is served; only SAML is still named) | `auth_from_env`, `crates/ordo-host/src/runtime.rs` | Each step adds its method word to the allowed set |
| The OIDC callback carries `subject` and `nonce` from the caller (§0.3 item 3) | **Unchanged.** `StubOidcProvider` still exported | `crates/ordo-identity/src/provider.rs`, `lib.rs` | Step 1 removes the fields |
| Admission is one boolean | Per-tenant policy with four ways: open, invitation, **provider** (optionally by domain), first-comer; `ORDO_AUTH_PROVISIONING` is the ceiling | `crates/ordo-host/src/admission.rs`, `$admissionpolicy` | Old F5 is mostly built. The provider way needs to know the connection: the `via` claim (§2.4) |
| No sign-up control | Self-serve provisioning (`ORDO_PROVISION_SELF_SERVE=on-signup`) and a deployment-wide sign-up ceiling, 60 an hour. No per-address limit | `crates/ordo-host/src/self_serve.rs`, `auth/signup.rs` | Every new path counts its first sign-ins against the same ceiling (§2.6) |
| Registrations kept for ever | Swept after `ORDO_AUTH_REGISTRATION_TTL_SECS` (90 days, ruled) | `crates/ordo-host/src/registrations.rs` | R10 is built; new paths inherit it |
| No account deletion | Deleted accounts are **marked and kept**; their links stay, so no identifier of theirs signs in. `refuse_deleted` is asked by `resolve_or_onboard`, `insert_link` and recovery. Removal follows `ORDO_ACCOUNT_REMOVAL` | `crates/ordo-identity/src/directory.rs`, `crates/ordo-host/src/auth/account.rs`, `identity_removal.rs` | §2.7 |
| No reading of the caller's address | Still none, now by ruling (`proxy-mode`, 2026-10-06) | `self_serve.rs` module doc | §7.4: E6 and E14 are withdrawn |
| No mail, no `fetch` | **Unchanged.** `ordo-egress` still exports `deliver` only | `crates/ordo-egress/src/client.rs` | Steps 2 and 4 |

---

## 1 · The rulings, restated as constraints [settled]

| # | Ruling (2026-09-29) | What it fixes in this plan |
|---|---|---|
| R1 | The service owns credentialing | Connections, authenticators and principals live in the control database and belong to the deployment. A tenant never holds a credential table |
| R2 | No enforced flow | No method is a prerequisite for another. A person may use an application with a federated or email sign-in and never hold a passkey |
| R3 | Each new sign-in is an enrolment unless the tenant chooses otherwise | Admission to a tenant defaults to automatic where the tenant has said so; invitation stays available and does not block signups elsewhere |
| R4 | Provisioning policy is the tenant's choice | Policy moves from one boolean to a per-space record (§2.2) |
| R5 | Multi-tenant users need control from the system above | One principal, many memberships, and a deployment-level rule about which connection's sessions a space accepts (§2.4) |
| R6 | Single-tenant: first come, first served | Read here as: open signup is the natural default when there is one tenant. The other reading was Q1, answered (§11) |
| R7 | Configurable to some degree, compliance varies | Every number in §4 to §7 is a default with a bounded range, and a deployment can narrow what tenants may choose (Q2, answered, §11) |

### 1.1 The six rulings, quoted, and how this plan honours each [revised 2026-10-08]

The owner's answers of 2026-09-29 to this plan's six asks (board `federated-auth-questions`, `corpus-rulings.md` 2026-09-29). Quotes are his words from the board.

| | Ask | His answer | How the plan honours it |
|---|---|---|---|
| **R8** | Which first? | *"Outside providers, then email codes"* | Step 3 (Discord, an outside provider) ships before step 5 (emailed code). OIDC moves after email because denizens does not need it: that is a change of order inside "outside providers", put to you as §13 Q1 |
| **R9** | Does a provider's two-step claim count as presence? | *"This should be up to the app dev to decide what is safe or not. 2FA from a third party is often a standard... forcing internal auth in addition would be a nuisace."* Amended: *"A tenant should also decide what it is what they need for their tenant."* | §8.2: a per-connection setting at the deployment (the default) and at each tenant, reaching that tenant's data only. Step 10 |
| **R10** | Keep a sign-in nobody admits? | *"it should be registration and they stay there for some x amount of time and then get nuked... this is really up to the app dev."* | §2.3. Built: the registration sweep, 90 days by default. Every new path mints through the same directory, so the sweep covers it with no new code |
| **R11** | How is mail sent? | *"secure SMTP settings and also add API mailing setup for "resend" as well as the top 5 API mail providers"*. Narrowed the same day: *"Leave just resend, sendgrid as the main ones and leave the rest for post deploy."* | §7.6. Step 4 builds SMTP over TLS, Resend and SendGrid. Mailgun, Postmark, Brevo, Mailjet and SES are 18z, after the first deployment |
| **R12** | Join accounts on a matching email? | *"Agreed - no not at first - but account linking should be an option if someone chooses. Existing account logs in and claims another account."* | §2.5: nothing joins on an attribute. Step 7 is the claim the owner described. Guest sign-in (§7A) upgrades by the same claim |
| **R13** | How fast must a removal at the company be noticed? | *"As long as the session tokens last - if that's 12 hours then it is 12 hours. We could add some advanced features to post deployment roadmap."* | Nothing is built for it. A session ends at its ceiling and the next sign-in fails at the provider. Faster is 18w |

### 1.2 Later rulings that reach this plan [revised 2026-10-08]

| Date | Ruling | Effect here |
|---|---|---|
| 2026-09-29 | Presence on every platform op is the app developer's setting, default required (*"What is enforced is app design ... not our design."*) | §8.1: "cannot" becomes "cannot by default" |
| 2026-09-29 | A sign-up nobody admits is kept 90 days, configurable | §2.3 |
| 2026-09-29 | Sign-up ceiling 60 an hour for the deployment, configurable | §2.6, §7.4 |
| 2026-09-29 | Built-in provisioning: a tenant per person at sign-up, *"an app dev needs built in provisioning"* | §2.6 |
| 2026-10-06 | A deleted account is marked, kept, and never signs in again; removal of its data is the app developer's setting | §2.7 |
| 2026-10-06 | *"IPs are just not something we should rely on."* Proxy mode after the first deployment; until then nothing decides by a client's address | §7.4 |
| 2026-10-11 | Proxy mode comes before the first deployment, as a slow-down only (`public-data` ask 28). Built: off until the deployment names its proxy; an address is only ever slowed | §7.4 |
| 2026-10-08 | Onboarding order: emailed-code and OAuth sign-in come after the backend functions plan | this revision |

### 1.3 What denizens uses [new 2026-10-08]

Read from `denizens-web` (no `.env` read). The login page is `src/auth/LoginPage.tsx`; the session reader is `src/auth/SupabaseProvider.tsx`.

| What | Detail | What Ordo needs |
|---|---|---|
| Emailed code | `signInWithOtp({email})`, then `verifyOtp({type: 'email'})`. The input takes 6 to 8 digits; the placeholder is 8. The page calls it "an email link" but accepts the code | §7. 8 digits matches. No links in v1 (§7.1), so the page's wording changes |
| OAuth | `signInWithOAuth({provider: 'discord', scopes: 'identify email guilds'})`. No other provider | OAuth2 with a Discord preset (§5.3) |
| Provider profile | After sign-in the page reads the Discord identity (`provider_id`, `full_name` or `global_name`, `avatar_url`) and upserts its own `discord_user_links` row | The person can read their own linked identities and the profile fields the connection keeps (§5.3, `GET /auth/identities`) |
| `guilds` scope | Requested; no code in `denizens-web` reads the provider token | Ordo keeps no upstream token (§4.1). A later call to Discord for the person is an app function (`plan-backend-functions.md` Q10) |
| Password | Username sign-up through the `username-auth` edge function, which makes `<username>@auth.hominem.ai`, then `signInWithPassword` | Ordo's password sign-in. Not this plan |
| Guest | `signInAnonymously()`, shown only when the app's `platform_config.anonymous_login_enabled` is true | §7A |
| Bot check | Cloudflare Turnstile before every one of the above | §7.4 E15, optional |
| Email change | `updateUser({email})`, which mails a confirmation | Not this plan. Noted for `denizens-readiness` (28j) |

---

## 2 · The shape: identity is the deployment's, admission is the tenant's [plan]

### 2.1 Two decisions that today are one

`Provisioning` (`directory.rs:43-46`) answers two questions with one value: *does this stranger get a principal* and *does this stranger get in*. The rulings pull them apart. R1 puts identity at the deployment. R4 puts admission at the tenant. The code already has the right storage split (principals in the control database, `members` per space, a token that names no space), so the change is to the policy and leaves the data model alone.

| Layer | Question | Who decides | Where it lives |
|---|---|---|---|
| Identity | Which principal is this authenticator? | the deployment | `$principal`, `$authenticator` (exist) |
| Admission | May this principal become a member of this space, and with what role? | the tenant | `$admission` (new, control database, keyed by space) |
| Acceptance | Does this space accept a session that was authenticated by this connection? | the tenant, inside bounds the deployment sets | `$admission.accepts` (new) plus a token claim (new) |

### 2.2 How `ORDO_AUTH_PROVISIONING` and the tenant's choice reconcile

`ORDO_AUTH_PROVISIONING` stays and changes meaning. It becomes **the admission mode of a space that has not chosen one**. The default remains `invite-only`, so a deployment that has not thought about it admits nobody by accident, which is the argument `membership-commands` already makes.

A space's admission record has one of three modes:

| Mode | Meaning |
|---|---|
| `invite` | Only a person holding an invitation to this space is admitted |
| `open` | Any person who completes a sign-in on an accepted connection, naming this space, is admitted with the space's default role |
| `connection` | Any person who completes a sign-in on one of the listed connections is admitted with the default role. This is the "third-party auth system that auto-provisions" case: the tenant's own IdP decides who exists |

Three pins that hold in every mode:

- **The default role is never `owner`.** `definition-language.md` §3 settles this for auto-onboard.
- **An invitation works in every mode.** R3: invitation does not stop signups, and open signup does not remove invitation.
- **Admission is evaluated at sign-in and writes a `members` row through 21b's command.** This plan does not write `members` itself. It defines `Admission::decide(space, subject, attributes) -> Admit { role } | NotAdmitted` and calls whatever command 21b ships.

Whether the deployment owner can forbid tenants from choosing `open` was Q2: yes, by the ceiling (§11).

### 2.3 When a principal is minted, and how long it is kept [settled, R10]

A principal is minted at the first completed sign-in of an authenticator nobody has seen, as `resolve_or_onboard` does today. If no tenant admits the person, the registration is **kept for a set time and then removed**: the owner's words, *"they stay there for some x amount of time and then get nuked"*. The time is `ORDO_AUTH_REGISTRATION_TTL_SECS`, a deployment setting, 90 days by default (ruled 2026-09-29; built, `crates/ordo-host/src/registrations.rs`); the sweep is `plan-enrolment.md` E4, which removes the principal, its links, its passkey rows and its credential. Admission anywhere before the sweep keeps it.

This replaces what this plan proposed (mint nothing until admitted) and what `plan-enrolment.md` proposed (keep every enrolment). The growth argument still holds and is what the sweep answers: `$principal` grows with every visitor of a public sign-in page for at most the TTL. The door answers `NoMembership` to every one of them until a tenant admits them.

To carry the space, `/auth/begin` gains an optional `space` field, so that a sign-in that names a tenant can be admitted to it in the same act. A single-tenant deployment (one row in `$spaces` besides the platform's own) defaults it, so its clients never send it.

### 2.4 Multi-tenant users and the control above

One person, one principal, several memberships. The token is already deployment-wide, so nothing about the session changes when a person moves between tenants.

The risk this creates is specific to federation. If tenant A configures its own IdP, A's IdP administrator can assert any subject of that IdP. A principal linked to `saml:acme-sso:alice` who is also a member of tenant B can therefore be impersonated into B by A's administrator. Nothing in B's control caused that.

The control, which is the "management from the system above" R5 asks for:

- A token gains one claim, `via`: the connection that authenticated this session (for example `oidc:google`, `saml:acme-sso`, `email`, `passkey`, `password`). It is under the MAC like every other claim.
- A **deployment-scoped** connection (configured by the deployment owner) is accepted by every space unless the space narrows its `accepts` list.
- A **tenant-scoped** connection (configured by a tenant owner) is accepted **only** by spaces that list it. Tenant B never accepts tenant A's IdP unless B's owner adds it.
- The door checks `via` against the bound space's `accepts` list in the same place it checks membership, and a mismatch is the ordinary `Deny`.

A person who belongs to A through A's IdP and to B by invitation signs in to B with a method B accepts (the deployment's Google connection, email, a passkey), linked to the same principal (§2.5).

### 2.5 Account linking [settled, R12]

**No automatic linking. Not by email, not by name, not by verified email.** Two sign-ins by two methods are two authenticators, and they resolve to one principal only when the person has linked them deliberately. The owner: *"Existing account logs in and claims another account."* That sentence is F13's shape: the person signs in as the account that will survive, then completes a fresh ceremony on the method being claimed, inside one window, and the two authenticators become one principal.

Why an email match must never link on its own:

- **Unverified email.** Many IdPs let a user type any address into their profile. An attacker sets the victim's address at such an IdP, signs in, and is merged into the victim's principal. This is a complete account takeover that requires no secret.
- **Verified email from a tenant-scoped IdP.** The tenant's administrator controls what their IdP asserts, including `email_verified: true` for an address at somebody else's domain. Auto-linking on it lets one tenant's administrator take over another tenant's user.
- **Recycled addresses.** An address can be reassigned by its provider. The `sub` claim and a persistent SAML NameID exist because addresses are not stable identifiers.

What email **is** used for: addressing an invitation, evaluating a domain constraint on `connection` admission, and display. For those uses an address counts only if the assertion marks it verified, and an assertion that omits `email_verified` is treated as unverified.

Explicit linking today requires a `LinkCeremony`, which requires presence, which only a passkey has. A person whose only authenticators are federated or email therefore cannot link anything through `Directory::link`. F13 gives them the path the owner described: a link proved by two fresh ceremonies, the existing account's and the claimed one's, inside one 300 s window, recorded naming both authenticators. A deployment may switch it off, which restores today's rule. What F13 does **not** do is link a first passkey to a presence-less account as a first factor with presence rights; that is `plan-passkey-signin.md` S7 with a ticket, or recovery R2 with a `LinkCeremony`.

### 2.6 The first sign-in, with self-serve provisioning [new 2026-10-08]

A first sign-in through Discord, Google or an emailed code is an enrolment (P3). It follows the same path a first passkey does today (`howto-sign-in.md` §4), so an app developer sees one behaviour whatever the method.

| Moment | What happens | Where |
|---|---|---|
| Callback, or a correct email code | The authenticator is unknown, so `resolve_or_onboard` mints a principal and links it. The display name comes from the verified profile, never from the request (§0.3 item 5) | directory |
| Same moment | The mint counts against the deployment's sign-up ceiling (`ORDO_PROVISION_SIGNUPS_PER_HOUR`, 60). Over it: no principal is minted | `self_serve.rs` |
| `/auth/complete` redeems the handoff | The token is minted. Then the self-serve hook runs, exactly as for a passkey: with `on-signup`, the person gets a tenant of their own as its owner | `auth/signup.rs` `given` |
| The answer | The token, plus `space` and `provisioning`, the same fields a passkey sign-up returns | unchanged |
| A tenant named at `begin` | Admission runs for it: open, invitation, or the provider way, which now reads the connection from the `via` claim | `admission.rs` |
| Nobody admits them, self-serve off | They hold a principal and no seat. The registration sweep removes them after 90 days (R10) | `registrations.rs` |

**Why the ceiling is counted at the mint and not at `begin`.** A passkey registration is known to be new at `begin`. A Discord or email sign-in is known to be new only once the provider or the code has answered. A returning person is never counted.

**What a person over the ceiling sees.** Redirect methods: `303` to `return_to?error=quota_exceeded`. Email code: `429 quota_exceeded`. The same class a passkey sign-up gets today. It tells the person only that this is a new account, which they already know.

**Presence and the new tenant.** A tenant made at sign-up is made by the hook, not by the person calling `std.space.create`, so it needs no presence. A Discord-only owner of that tenant can use it. Exporting or deleting it needs presence under the default, which the app developer may lower (§8.1).

### 2.7 A deleted account cannot come back [new 2026-10-08, 25af]

25af found that a ticket brought a deleted person back. The fix (f7c0aae8) put the refusal in the directory. Every path in this plan goes through the directory, so the same refusal covers it. These pins make that a tested property, not an accident.

| # | Pin | Stops |
|---|---|---|
| D1 | **Every new method resolves through `resolve_or_onboard`**, which asks `refuse_deleted` before anything else. No provider mints or links on its own | a new method being a side door |
| D2 | **A deleted account's links stay** (ruled 2026-10-06), so its Discord id, Google `sub` and email address name it and nobody else. None of them signs in, and none mints a fresh account while it is marked | the same identifier making a new account beside the deleted one |
| D3 | **The handoff re-asks `refuse_deleted` at redemption.** A deletion inside the 60-second gap between callback and `complete` wins | a session minted for a person who deleted their account a second earlier |
| D4 | **The email worker sends nothing to an address linked to a deleted account** (§7.5). `begin` answers as usual | mailing a person who left; a code that could only be refused |
| D5 | **A claim (step 7) is refused if either side belongs to a deleted account.** `insert_link` already refuses | reviving a deleted account by claiming a live identifier into it, or claiming its identifier out of it |
| D6 | **A guest resume (§7A) is refused for a deleted guest** | a guest secret outliving the account |
| D7 | **The refusal is the one answer to the caller**, and `identity-deleted` names the method in the security stream | an oracle for "this address once had an account" |

**After removal.** When the app developer's `ORDO_ACCOUNT_REMOVAL` removes a deleted account's data, its links go with it. The same Discord account or address can then sign up again. That makes a **new** person: a new random `usr_…` id, no seats, no data. Nothing of the old account returns. Whether those identifiers should stay blocked after removal is §13 Q8.

**A new identifier is a new person.** A deleted person who signs in with a method they never linked (a Google account the deleted account never used) gets a new account. Matching it to the deleted one by email would be the automatic joining R12 forbids.

Tests, one per method, in the form of the 25af tests: `a_deleted_account_cannot_sign_in_through_discord`, `a_deleted_account_is_sent_no_email_code`, `a_deletion_between_callback_and_handoff_mints_no_token`, `a_claim_into_a_deleted_account_is_refused`, `a_deleted_guest_cannot_resume`.

---

## 3 · Shared machinery

### 3.1 The durable ceremony [plan]

One table in the control database, `$authceremony`, holds the ceremony state for every redirect method and for email code. **`plan-passkey-signin.md` S1 creates it first**, in exactly this shape, with the `CeremonyStore` trait (`put`, one-shot `take`, `sweep`) and its Postgres store, for passkey challenges. F1 adds the `handoff` phase and the binder and changes no column.

```
"$authceremony" (
  "handle"      text   NOT NULL,   -- SHA-256 of the state value; the value itself is never stored
  "connection"  text   NOT NULL,
  "phase"       text   NOT NULL,   -- 'begun' | 'handoff'
  "binder"      text   NOT NULL,   -- SHA-256 of the browser binder
  "payload"     text   NOT NULL,   -- nonce, PKCE verifier, SAML request id, return URL, space; or the resolved subject in phase 'handoff'
  "minted_at"   bigint NOT NULL,
  "expires_at"  bigint NOT NULL,
  PRIMARY KEY ("handle")
)
```

- **One-shot across replicas** is `DELETE … WHERE handle = $1 AND phase = $2 AND expires_at > $3 RETURNING payload`. The database decides which of two racing callbacks wins. An expired row and an absent row are the same empty result, which keeps the existing rule that expired reads as unknown.
- **The PKCE verifier and nonce are secrets at rest for five minutes.** They are useless without the authorization code and the binder. They are stored in the clear, and the table is swept by a supervised worker on the existing supervisor.
- **Passkey challenges already live here** (S1). **Password keeps its in-memory map in this plan.** Moving it is a separate, small change that belongs to whoever next touches that provider.

### 3.2 Routes: two additions, argued [plan]

OIDC and OAuth2 return to the relying party with a browser `GET` carrying `code` and `state`. SAML returns with a browser form `POST` carrying `SAMLResponse` and `RelayState`. `/auth/complete` takes JSON from application code. A SAML IdP cannot post JSON and a single-page application cannot receive a form post, so SAML needs a served endpoint whatever is decided for OIDC.

**Recommendation: Ordo serves the return leg for all three, and hands the session to the application with a one-time handoff.**

| Route | Method | Purpose |
|---|---|---|
| `/auth/callback/{connection}` | `GET` | OIDC and OAuth2 redirect URI |
| `/auth/saml/{connection}/acs` | `POST`, `application/x-www-form-urlencoded` | SAML assertion consumer service |
| `/auth/saml/{connection}/metadata` | `GET` | SP metadata for the IdP administrator to import. Static, public, no secrets |

The flow, identical for all three from the application's side:

1. The application posts `/auth/begin` `{method, space?, return_to}`. Ordo answers `{redirect, binder}`. `binder` is 256 random bits. The application keeps it in memory or `sessionStorage` and navigates to `redirect`. The binder is never placed in a URL.
2. The IdP returns the browser to Ordo's callback or ACS. Ordo consumes the ceremony row, validates the assertion (§4 to §6), resolves the principal (§2), writes a `handoff` row bound to the same binder hash with a 60 second life, and answers `303` to `return_to?handoff=<code>`.
3. The application posts `/auth/complete` `{method, kind: "handoff", handoff, binder}`. Ordo consumes the handoff row, checks the binder, mints the token. The response is the same JSON `complete` returns today.

Why this shape:

- **R1.** The redirect URI registered at the IdP belongs to the deployment. With several applications on one deployment (`definition-language.md` §8: apps and domains are many-to-many), application-owned redirects would mean registering every application at every IdP.
- **Exact match is Ordo's to enforce.** There is one redirect URI per connection, built from `ORDO_PUBLIC_URL` and the connection name, compared byte for byte.
- **The authorization code never reaches application JavaScript.**
- **Login CSRF is closed without a cookie.** An attacker who starts a flow and plants the resulting callback URL in a victim's browser produces a handoff the victim's application cannot redeem, because the victim's browser holds no binder for it. This is what "state and nonce bound to the session" means in a deployment where Ordo is an API on a different origin from the application and a cookie would be third-party.
- **No token ever appears in a URL.** The handoff code does, and it is single-use, lives 60 seconds, and is worthless without the binder.

*Update 2026-10-09: `ORDO_AUTH_RETURN_URLS` is now matched by origin, with any path, and takes wildcard origins with `ORDO_WILDCARD_ORIGINS=on` (`plan-subdomain-signin.md`).* `return_to` is matched exactly against an allowlist (`ORDO_AUTH_RETURN_URLS`, later the `client { redirect … }` block when it is parsed). An unlisted value is refused at `begin`, before any redirect is built, so the callback can never be used as an open redirector.

The argument for the alternative (the application owns the redirect URI and posts `code` and `state` to `/auth/complete`) is §13 Q5.

### 3.3 Trait changes [plan]

Nothing is deployed, so these are plain edits.

- `AuthProvider::begin` returns `Result<ChallengeOrRedirect, AuthError>`. A federated `begin` can fail (discovery not yet loaded, `return_to` not allowlisted).
- `BeginRequest` gains `space` and `return_to`. **Revised 2026-10-08:** it no longer gains `source`, the caller's network address. No limit reads it until proxy mode exists (§7.4).
- `ChallengeOrRedirect::Redirect` gains `binder`.
- `CompleteRequest::OidcCallback` loses `subject` and `nonce` and gains `iss: Option<String>`. New variants: `OAuth2Callback`, `SamlResponse`, `EmailCode`, `Handoff`.
- `AuthenticatedSubject` gains `attributes: SubjectAttributes { email: Option<VerifiedEmail>, display: Option<String>, assurance: Option<UpstreamAssurance> }`. `VerifiedEmail` has one constructor, which takes the address and the evidence that it was verified, so an unverified address has no type to travel in. `UpstreamAssurance` carries `amr`, `acr`, `auth_time` or the SAML `AuthnContextClassRef` **for the record only** (§8).
- `StubOidcProvider` becomes `#[cfg(test)]`, the same treatment `PlainTestHasher` received and for the same reason.
- `AuthError` gains `UpstreamUnavailable`, the federated counterpart of `StoreUnavailable`.

**The trait stays synchronous.** Network I/O reaches a provider through a small synchronous trait, `FederationTransport`, defined in `ordo-identity` and implemented in `ordo-host` over `ordo-egress`. This is the pattern `CredentialStore` and `PgCredentialStore` already use (`credentials.rs:9-15`): `complete` already runs inside `spawn_blocking` (`auth.rs:265`), and the implementation blocks on a thread that owns a runtime. `ordo-identity` stays free of sockets, and every verifier in it stays a pure function of bytes, keys and an instant, testable without a network.

### 3.4 The egress fetch [plan]

A new function in `ordo-egress`, beside `deliver` and sharing its dialler:

```
pub async fn fetch(dest: PinnedDestination, roots: &TlsRoots, req: FetchRequest<'_>, cap: usize, timeout: Duration)
    -> Result<Fetched, Refusal>
```

| Pin | `deliver` | `fetch` |
|---|---|---|
| 1 · https only, certificates validated, TLS 1.2 floor | yes | yes, same code |
| 2 · resolve then pin, consumed by value | yes | yes, same type |
| 3 · no redirects | yes | yes. A discovery document that answers `302` is a refusal |
| 4 · signed | yes | **no.** An IdP holds no sink secret. The request is authenticated by TLS and, for the token endpoint, by the client secret |
| 5 · holds nothing | yes | yes. The client secret is passed in per call, the way `SinkSecret` is. `tests/holds_nothing.rs` must pass unchanged |
| 6 · allowlist before resolution | yes | yes. The allowlist is exactly the hosts named in the connection's configuration |
| response bytes | never returned | **returned, bounded, to one kind of caller** |

The last row is the departure from `lib.rs:65-75` and needs its argument. That paragraph closes the exfiltration half of SSRF: a tenant names a URL, the platform fetches it from inside the perimeter, and the response travels back to the tenant. `fetch` keeps that closed by a different means:

- `Fetched` has no `Debug`, no `Display` and no `serde`. Its body can be read only through `into_verifier_input()`.
- Its only consumers are the parsers in `ordo-identity` (JWKS, discovery, token response, userinfo, SAML metadata), whose outputs are keys and claims.
- The caller of a sign-in route receives the one answer or a token. No parser error text, no status code and no timing class derived from the body is returned to a caller. They go to the security stream.
- The body cap is per purpose: 64 KiB for a token or userinfo response, 256 KiB for a JWKS or discovery document, 1 MiB for SAML metadata. Over the cap is `MalformedResponse`.

**Revised 2026-10-08: one `fetch`, two callers.** `plan-backend-functions.md` §6 plans a `fetch` for functions, which returns the body to the function and injects host-bound secrets. Both are one dialler and one response reader with two front doors: sign-in's returns an opaque `Fetched` to a parser, a function's returns bytes to code the app developer wrote. Whichever increment starts first builds the shared core; the other adds its door.

Work the reader needs that it does not have today: `Transfer-Encoding: chunked` decoding with a bounded chunk count, `Content-Length` honoured, `Accept-Encoding: identity` sent so no decompression is ever needed, and `GET` plus form-encoded `POST` request writers.

Discovery and JWKS fetches are **not** exempted from the deny list for deployment-scoped connections, even though the deployment owner named the host. A deployment owner whose IdP is on a private network (a Keycloak inside the cluster) is the case that needs an answer. **Revised 2026-10-08:** the answer is the one list `plan-backend-functions.md` §6 names, `ORDO_EGRESS_PRIVATE_HOSTS`, which replaces this plan's `ORDO_EGRESS_OPERATOR_HOSTS`. It is backend functions question 6, decided once for both. **Built 2026-10-08** for the mail channel (commit `3af386be`). Sign-in connections read it too (step 9, 2026-10-09). Functions are to read the same list.

### 3.5 Configuration [plan]

Deployment-scoped connections are configured in the environment, in the style `ORDO_AUTH_METHODS` already uses. `ORDO_AUTH_METHODS=password,google,corp-sso,email` names the methods. Each non-built-in name has its own variables:

```
ORDO_AUTH_GOOGLE_KIND=oidc
ORDO_AUTH_GOOGLE_ISSUER=https://accounts.google.com
ORDO_AUTH_GOOGLE_CLIENT_ID=…
ORDO_AUTH_GOOGLE_CLIENT_SECRET_FILE=/run/secrets/google
```

- A connection name matches `[a-z0-9-]{1,32}`. It becomes part of every authenticator id, so it may not contain `:`.
- A secret is read from a file, never from a variable's value, so it does not appear in a process listing or a crash dump of the environment.
- **A connection's issuer (OIDC), endpoint set (OAuth2) or IdP entity id (SAML) is immutable.** Changing it under the same name would re-point every existing `oidc:<name>:<sub>` authenticator at a different IdP's subject space, where `sub` values may collide with or be chosen to match the old ones. The boot records a fingerprint of each connection's identity in `$authconnection` and refuses to start if a name's fingerprint changed. Retiring a connection tombstones its name.
- `auth_from_env`'s refusal stays and grows arms. A method whose increment has not shipped keeps refusing the boot by name.
- **Presets** (§5.3) fill the endpoints and claim names for a known provider, so `ORDO_AUTH_DISCORD_PRESET=discord` plus a client id and a secret file is a whole connection.

Tenant-scoped connections are created at run time by a platform op and are F12. They need a registry that can change while serving, which `ProviderRegistry` is documented not to be (`registry.rs:36-37`), and a refresh worker on the pattern of `access.refresh` (`workers.rs:1068`).

### 3.6 What a refusal looks like [plan]

| Situation | Answer to the caller | Recorded |
|---|---|---|
| Any validation failure at the callback or ACS: unknown state, bad signature, wrong audience, replay, expired, not admitted | `303` to `return_to?error=authentication_failed` | `AuthenticationFailed` with the specific reason and the connection |
| The ceremony row is missing, so `return_to` is unknown | `401` with the existing `authentication_failed` body | same |
| Handoff or email code refused at `/auth/complete` | the existing `refused()` | same |
| The IdP or the mail relay cannot be reached, or the IdP answers 5xx, 408, 425 or 429 | `identity_provider_unavailable` (503), or the same as a query parameter | `UpstreamUnavailable` (`provider-busy` in the security stream for an answered 5xx or 429) |
| Success | token, as today | `Authenticated`, plus `PrincipalOnboarded` and `AuthenticatorLinked` where they happened |

`identity_provider_unavailable` is safe to keep distinct for the reason `credential_store_unavailable` is: it is decided before any identity is known, so it carries no information about an account.

A person who authenticated correctly at their IdP and is not admitted receives the same answer as a forged assertion. They have learned only that a sign-in did not succeed. A deployment owner reads the difference in the security stream.

---

## 4 · OIDC [plan]

### 4.1 Flow

1. `begin`: mint `state` (256 bits), `nonce` (256 bits), PKCE `code_verifier` (256 bits, base64url), `binder`. Store the ceremony row. Build the authorization URL from the connection's pinned `authorization_endpoint` with `response_type=code`, `client_id`, the exact `redirect_uri`, `scope=openid email profile`, `state`, `nonce`, `code_challenge`, `code_challenge_method=S256`.
2. Callback: consume the ceremony row by `state`. Check the connection in the path equals the connection in the row. Check `iss` if present.
3. Exchange `code` at the connection's pinned `token_endpoint` through `fetch`, with `code_verifier`, the exact `redirect_uri`, and client authentication.
4. Validate the ID token (§4.3).
5. Resolve the principal, write the handoff, redirect.

The access token and refresh token in the token response are dropped after step 4. `security-logging-design.md` §4: upstream tokens are never stored.

### 4.2 Configuration, storage, identifier

| | |
|---|---|
| Deployment owner supplies | issuer, client id, client secret file. Optionally the four endpoints, which pins them and skips discovery |
| Registered at the IdP | one redirect URI: `<ORDO_PUBLIC_URL>/auth/callback/<connection>` |
| Tenant supplies | for a deployment-scoped connection, nothing but their admission record. For a tenant-scoped one (F12), the same fields through a platform op |
| Stored | `$authconnection` (name, kind, identity fingerprint, scope), `$authceremony` rows, the `$authenticator` link, `$identityattr` (authenticator, verified email, display, last seen). JWKS and discovery are cached in memory per process |
| Identifier | `oidc:<connection>:<sub>`, the existing spelling (`provider.rs:48-51`). The connection pins the issuer, so the pair (issuer, `sub`) is what is linked. `sub` is compared as an exact, case-sensitive string |

Issuers that are templates are refused at boot. The example that matters is Microsoft's multi-tenant `common` endpoint, whose tokens carry a per-tenant issuer: a connection names one concrete issuer.

### 4.3 Security pins

Every pin names the attack it stops. Each is a test in F3 or F4.

| # | Pin | Stops |
|---|---|---|
| O1 | **Authorization code flow with PKCE `S256`, and nothing else.** `response_type` is the constant `code`. No implicit, no hybrid, no `plain` challenge. A callback carrying `id_token` or `access_token` in its parameters is refused | token leakage through the front channel; code injection |
| O2 | **`state` is 256 random bits, one-shot, five minutes, stored hashed, and bound to the binder** | login CSRF; replay of an observed callback |
| O3 | **`nonce` is in the ceremony row and must equal the ID token's `nonce` exactly** | replay of an ID token from another session |
| O4 | **`iss` must equal the connection's configured issuer, byte for byte.** No trailing-slash normalisation, no case folding | a token from another issuer signed by a key the attacker controls |
| O5 | **`aud` must contain this connection's client id. If `aud` has more than one value, `azp` must be present and equal the client id** | a token issued to a different client of the same IdP |
| O6 | **`exp` must be in the future and `iat` must not be more than 60 seconds ahead of the server clock.** Skew is bounded at 60 seconds in each direction, configurable from 0 to 120 | stale and post-dated tokens |
| O7 | **The signature algorithm comes from an allowlist: `RS256`, `ES256`, `EdDSA`, with `PS256` optional.** `none` and every `HS*` are refused by name. The key's `kty` must match the algorithm family, and a JWK carrying `alg` or `use` must agree. RSA keys under 2048 bits are refused | algorithm confusion, where an RSA public key is used as an HMAC secret |
| O8 | **The key is chosen by `kid` from the connection's JWKS only.** `jku`, `jwk` and `x5u` in the token header are never read | an attacker supplying their own verification key |
| O9 | **JWKS caching and rotation.** Cached per connection for one hour or the response's `max-age`, whichever is shorter, with a floor of five minutes. An unknown `kid` triggers one refetch, and refetches for one connection are at least 60 seconds apart. A key that disappears from the JWKS stops verifying at the next refresh | both halves: rotation without a restart, and `kid` spraying used to make the deployment hammer the IdP |
| O10 | **An unreachable JWKS with a warm cache verifies against the cache for at most 24 hours past its expiry, and says so in the security stream. With a cold cache the answer is `identity_provider_unavailable`** | an IdP outage becoming a total sign-in outage, without trusting keys indefinitely |
| O11 | **`email` is used only when `email_verified` is the JSON boolean `true`.** Absent, `false`, or the string `"true"` all mean unverified. An unverified address is not stored, does not match an invitation, and does not satisfy a domain constraint | §2.5's takeover |
| O12 | **No email, verified or not, links to an existing principal** | §2.5's takeover by a tenant-scoped IdP |
| O13 | **Redirect URI exact match.** The value sent at `begin` and at the token exchange is the same constant, built once at boot | code theft through a near-miss redirect |
| O14 | **Mix-up defence.** Each connection has its own callback path. The connection in the path, the connection in the ceremony row and the connection whose token endpoint receives the code must be one connection. Where the IdP's metadata advertises `authorization_response_iss_parameter_supported`, the `iss` response parameter is required and must equal the configured issuer (RFC 9207) | a hostile or compromised IdP causing the code from an honest IdP to be sent to it |
| O15 | **Discovery is validated against configuration.** The document's `issuer` must equal the configured issuer. Every endpoint it names must be `https` and its host is added to that connection's allowlist only at boot or at an deployment-owner-acknowledged refresh | a poisoned discovery document re-pointing the token endpoint |
| O16 | **The client secret never appears in a log, a refusal or a security event** | disclosure through diagnostics |
| O17 | **The ID token is parsed once, from the bytes whose signature verified.** The payload segment is decoded after verification and claims are read from that decode. Duplicate JSON member names in the header or payload are refused | parser differentials between the verifier and the claim reader |

---

## 5 · OAuth2 without OIDC [plan]

For providers with no ID token and no discovery (GitHub and Discord are the usual examples). The design doc's row: "generic authorization-code; subject identity from a configured userinfo claim".

### 5.1 Flow and differences from OIDC

Steps 1 to 3 of §4.1 without `nonce` and without `openid` in the scope. Step 4 becomes: call the configured `userinfo_endpoint` through `fetch` with the access token as a bearer, and read the subject from the configured claim.

| | |
|---|---|
| Deployment owner supplies | authorization, token and userinfo endpoints; client id; client secret file; scope; `subject_claim`; optionally `email_claim` with `email_verified_claim`, and `display_claim` |
| Identifier | `oauth2:<connection>:<value of subject_claim>`. A new spelling beside the four documented ones |
| Stored | as OIDC |

### 5.2 Security pins

| # | Pin | Stops |
|---|---|---|
| A1 | O1, O2, O13, O14's path rule, O16 apply unchanged. PKCE is sent even when the provider ignores it | as OIDC |
| A2 | **The access token is used only if it came from this ceremony's own code exchange.** No route accepts an access token from a caller | token substitution: an access token issued to another application, presented as proof of identity |
| A3 | **`subject_claim` must name an immutable identifier.** Boot refuses a connection whose `subject_claim` is `email`, `login`, `username` or `name` unless `ORDO_AUTH_<NAME>_MUTABLE_SUBJECT=acknowledged` is set | an account renamed or an address reassigned upstream becoming somebody else's Ordo principal |
| A4 | **The subject value is read as a JSON string or integer and rendered canonically.** A float, an object, an array or `null` is a refusal | type confusion: `123` and `"123"` and `123.0` resolving to different or identical principals by accident |
| A5 | **Email is used only when the connection configures a verified-email claim and it is `true`.** Where a provider exposes verification through a second endpoint, the connection is configured without email | O11's reasoning |
| A6 | **There is no nonce and no signed assertion, so the userinfo response is trusted because of TLS to a pinned host and nothing else.** The documentation for deployment owners says so, and says OIDC is preferred wherever the provider offers it | the deployment owner believing the two are equivalent |

### 5.3 Providers as settings, with presets [new 2026-10-08]

A provider is a **connection**: a name the app developer chooses, a kind (`oauth2` or `oidc`), and its settings. The connection name is what a page passes to `begin` and what the `via` claim carries.

**Where the settings live.** In v1, the environment, as §3.5 describes, with every secret in a file. A secret never sits in a variable's value. The issuer or endpoints of a connection are fixed once used (§3.5's fingerprint). Setting a connection through an op or the console is the tenant-scoped work (later, §10), and uses the same record.

**Presets** fill everything except the client id and secret. A preset is a constant table in the code, reviewed once; the app developer may override any field.

| Preset | Kind | Subject | Verified email | Display, avatar | Notes |
|---|---|---|---|---|---|
| `discord` | oauth2 | `id`, a string | `email` when `verified` is `true` | `global_name`, then `username`; avatar URL from `id` and `avatar` | **Step 3, for denizens.** Scope `identify email`. PKCE sent (A1) **(unverified: whether Discord enforces it)** |
| `github` | oauth2 | `id`, an integer rendered as text (A4) | none: GitHub's verified flag is on a second endpoint, so A5 leaves email out | `name`, then `login`; `avatar_url` | Step 3, small |
| `google` | oidc | `sub` | `email` when `email_verified` is `true` | `name`; `picture` | Step 9 |
| `microsoft` | oidc | `sub` | not used: Entra's `email` is not verified for every account type | `name` | Step 9. One tenant id per connection; `common` is refused (§4.2) |
| none | oidc | `sub` | as O11 | `name`, then `preferred_username` | **A company login**: Okta, Entra, Keycloak, Auth0, anything with discovery. Step 8 |
| none | oauth2 | the configured `subject_claim` | the configured pair (A5) | configured | anything else, with A3's acknowledgement for a mutable subject |

Example, denizens' whole provider setting:

```
ORDO_AUTH_METHODS=password,discord,email
ORDO_AUTH_DISCORD_PRESET=discord
ORDO_AUTH_DISCORD_CLIENT_ID=…
ORDO_AUTH_DISCORD_CLIENT_SECRET_FILE=/run/secrets/discord
```

**What the app can read.** Denizens copies the Discord id, name and avatar into its own table after sign-in (§1.3). Ordo keeps those fields in `$identityattr` beside the link, refreshed at each sign-in, and serves them to the person themselves:

```
GET /auth/identities  →  [{ "connection": "discord", "subject": "80351110224678912",
                            "display": "Nelly", "avatar": "https://cdn.discordapp.com/…",
                            "email": "nelly@example.com", "linkedAt": 1791100000 }]
```

Only the caller's own identities, only fields the connection keeps, `email` only when verified. Whether a program's ops may also read them, so the app writes its Discord link on the server instead of trusting the page, is §13 Q6.

---

## 6 · SAML [plan, build last, see §13 Q12]

### 6.1 Flow

SP role only and SP-initiated only, both settled (`security-logging-design.md` §2 and §8).

1. `begin`: mint a request id (`_` followed by 160 random bits, hex), `RelayState` (the state handle, under 80 bytes), `binder`. Store the ceremony row with the request id. Build an `AuthnRequest` with `ID`, `IssueInstant`, `Destination`, `AssertionConsumerServiceURL`, `Issuer` (this SP's entity id) and `NameIDPolicy Format=persistent`. Deflate, base64, and redirect to the IdP's single sign-on URL (HTTP-Redirect binding).
2. ACS: the browser posts `SAMLResponse` and `RelayState`. Consume the ceremony row by `RelayState`.
3. Validate (§6.3), in the order given there.
4. Resolve the principal, write the handoff, `303` to `return_to`.

### 6.2 Configuration, storage, identifier

| | |
|---|---|
| Deployment owner or tenant supplies | the IdP's entity id, its single sign-on URL, and one or more signing certificates. Metadata XML may be pasted at configuration time to extract those three; it is not fetched on a schedule in v1 |
| Given to the IdP administrator | SP entity id (`<ORDO_PUBLIC_URL>/auth/saml/<connection>`), ACS URL, and the metadata document that states both |
| Stored | `$authconnection` with the certificate fingerprints (SHA-256 of the DER); `$samlreplay` (connection, assertion id, expires_at) |
| Identifier | `saml:<connection>:<NameID>`, the documented spelling, **when the NameID format is `persistent`**. Otherwise a configured attribute (`subject_attribute`), which must be declared immutable by the deployment owner with the same acknowledgement as A3. `transient` is refused: it names nobody twice. `emailAddress` is a mutable subject and needs the acknowledgement |

Certificates are pinned by value. A certificate's expiry date is **not** enforced, because IdPs routinely sign with expired self-signed certificates and the trust is in the pin. Rotation is a configuration change that lists the new certificate beside the old one before the IdP switches.

### 6.3 Security pins

| # | Pin | Stops |
|---|---|---|
| S1 | **No DTD.** A document containing `<!DOCTYPE` is refused before any tree is built. No entity other than the five predefined ones and numeric character references is expanded. No external resource is ever loaded | XXE, billion-laughs entity expansion, external DTD retrieval as SSRF |
| S2 | **Bounded input.** 256 KiB after base64 decoding, 10 000 nodes, depth 64 | memory and CPU exhaustion |
| S3 | **The assertion itself must be signed.** A signed `Response` wrapping an unsigned `Assertion` is refused. A signature on the `Response` is verified if present and is never sufficient | an IdP or an intermediary that signs the envelope and lets the assertion be replaced |
| S4 | **Exactly one `Assertion` element in the document**, and it is a direct child of `Response`. Zero or two is a refusal. `EncryptedAssertion` is refused in v1 | the simplest wrapping attacks, which add a second assertion |
| S5 | **The signature is structurally bound.** The `Signature` element is a direct child of the `Assertion` it signs. `SignedInfo` has exactly one `Reference`. Its `URI` is `#` followed by that assertion's own `ID`. `ID` values are unique in the document | signature wrapping, where a valid signature over one element is presented beside a forged element the consumer reads |
| S6 | **Claims are read from the bytes that were digested.** After the reference digest and the signature verify, the canonical octets that were hashed are parsed again as a standalone document, and every value (NameID, conditions, attributes) is read from that second parse. The original tree is discarded | the whole class of wrapping attacks, including the ones not yet catalogued, because no code path can read an element the signature did not cover |
| S7 | **Transforms are an allowlist of two:** enveloped-signature and exclusive canonicalisation (with or without comments handled as "without"). Anything else, XPath and XSLT included, is refused | transform-based signature bypass and code execution |
| S8 | **Algorithms are an allowlist:** `rsa-sha256` and `ecdsa-sha256` for the signature, `sha256` for the digest. SHA-1 in either position is refused by name | collision forgery |
| S9 | **The verification key comes from the connection's pinned certificates.** `KeyInfo` in the document is ignored entirely | an attacker signing with their own key and enclosing the certificate |
| S10 | **Text is the concatenation of all text nodes of the element, and comments are absent from the canonical form that is parsed** | the comment-truncation attack, where `user@victim.example<!---->.attacker.example` is read as the shorter name |
| S11 | **Audience.** `Conditions/AudienceRestriction/Audience` must contain this SP's entity id. A missing `AudienceRestriction` is a refusal | an assertion issued for another service provider at the same IdP |
| S12 | **Recipient and destination.** `SubjectConfirmationData/@Recipient` must equal this connection's ACS URL exactly. `Response/@Destination`, when present, must equal it too | an assertion replayed from another SP's ACS |
| S13 | **Issuer.** `Assertion/Issuer` must equal the configured IdP entity id exactly | an assertion from another IdP that happens to share a key |
| S14 | **Time, with bounded skew.** `Conditions/@NotBefore` and `@NotOnOrAfter`, and `SubjectConfirmationData/@NotOnOrAfter`, are checked against the server clock with 60 seconds of skew, configurable from 0 to 180. An assertion whose validity window is longer than one hour is refused regardless of the clock | stale assertions; an IdP misconfigured to issue day-long assertions |
| S15 | **Replay.** `(connection, Assertion/@ID)` is inserted into `$samlreplay` with `expires_at = NotOnOrAfter + skew`. The insert is the check: a primary-key conflict is a refusal. Rows are swept after expiry | a captured response posted a second time |
| S16 | **SP-initiated only.** `SubjectConfirmationData/@InResponseTo` must be present and must equal the request id in the ceremony row that `RelayState` consumed. A response with no `InResponseTo` is an IdP-initiated response and is refused | unsolicited responses, which have no ceremony to bind to and therefore no login-CSRF defence |
| S17 | **Subject confirmation method is `bearer`** and exactly one `SubjectConfirmation` satisfies S12, S14 and S16 together | confirmation data taken from different elements to satisfy different checks |
| S18 | **Status.** `Response/Status/StatusCode/@Value` must be `…:status:Success` | treating a failure response with a stale assertion as success |
| S19 | **The ACS accepts `POST` only, form-encoded only**, and `RelayState` is looked up and never used as a URL | open redirect through `RelayState` |

**IdP-initiated SAML stays refused.** The request is common, because IdP dashboards ("click the Ordo tile") are how many enterprise users start. The supported answer is a link on the dashboard to the application's sign-in page with the connection preselected, which starts an SP-initiated flow and completes without a prompt because the IdP session exists.

---

## 7 · Email one-time code [plan]

### 7.1 Email is a bearer channel

Whoever can read the mailbox can sign in. That includes the mailbox provider, a corporate mail administrator, anybody holding a forwarding rule, anybody looking at a lock-screen preview, and whoever receives the address after it is recycled. Mail between servers is often unencrypted. None of this can be fixed from Ordo's side, so the design assumes it:

- An email session carries no presence and never will.
- The code is made useless on its own by binding it to the browser that asked (E4).
- Codes are short-lived and single-use, and a new code revokes the old one.
- There are **no sign-in links** in v1. Mail scanners follow links and would consume them, and a link opened on a different device from the one that asked cannot satisfy E4.
- The message says what to do if the recipient did not ask for it, and contains nothing else: no name, no tenant, no account state.

### 7.2 Flow

1. `begin` `{method: "email", identifier: <address>, space?, human?}` (`human` is the Turnstile token when E15 is on). Ordo normalises the address, mints a ceremony row and a `binder`, enqueues a send job, and answers `{challenge, binder, method: "email-otp"}`. The answer and its timing are the same for every address.
2. A supervised worker takes the job, applies the rate limits (§7.4), decides whether a message is owed (§7.5), and if so mints the code, stores its hash in the ceremony row and submits the message.
3. `complete` `{method: "email", kind: "email_code", challenge, binder, code}`. Ordo checks the binder, decrements the attempt counter, compares the code in constant time, and on success consumes the row.

The code is minted by the worker and never returned by `begin`, so the HTTP response cannot leak it.

### 7.3 Configuration, storage, identifier

| | |
|---|---|
| Deployment owner supplies | the mail setting (§7.6: SMTP, Resend or SendGrid, with a key or credentials file and the sender address), and the limits in §7.4 if the defaults do not suit |
| Tenant supplies | nothing in v1. Message wording per tenant belongs to 17r |
| Stored | the ceremony row, holding `HMAC(k_otp, address)` and the code's hash, never the address or the code in the clear; `$authrate` counters keyed by HMAC of the recipient address, never by a client's network address; the `$authenticator` link; `$identityattr` with the address as a verified email (verified by this ceremony) |
| Identifier | `email:<normalised address>`. A new spelling |

Normalisation: trim, lowercase the whole address, require exactly one `@`, require an ASCII domain (an internationalised domain is accepted in its punycode form as typed). No provider-specific folding: dots and `+tags` are left alone, because folding them is wrong for every provider except the one the rule was written for.

### 7.4 Security pins

| # | Pin | Stops |
|---|---|---|
| E1 | **The code is 8 decimal digits from the OS CSPRNG, by rejection sampling.** 10^8 values. Stored as `HMAC(k_otp, handle ‖ code)` | guessing; modulo bias; a database read yielding usable codes |
| E2 | **Lifetime 600 seconds**, configurable from 120 to 900. This is longer than `CEREMONY_TTL_SECS` and is argued in §13 Q7 | codes that outlive their purpose; greylisting delays making sign-in impossible |
| E3 | **Five attempts per code. The fifth failure consumes the ceremony.** With E1 that is a 5 in 10^8 chance per code | online guessing |
| E4 | **A code is accepted only with the binder of the `begin` that caused it.** Attempts are therefore counted per ceremony, against a ceremony only its starter can reach | a code read from a mailbox and typed on another device; **and the lockout denial of service**, because an attacker has no ceremony of the victim's to burn attempts on |
| E5 | **A new code for an address invalidates every outstanding ceremony for that address** | an attacker holding several live codes at once |
| E6 | **Withdrawn 2026-10-08.** Was a per-source limit by /24 or /48. Proxy mode (28a) is built (2026-10-11), and an address only ever slows a visitor. A per-source sign-in limit is not part of it yet; L5 | (one machine spraying addresses: now bounded by E9, the sign-up ceiling, and E15 where the app developer turns it on) |
| E7 | **Per-recipient limit: 5 messages per hour and 15 per day to one address**, keyed by `HMAC(k_otp, address)`. **Revised 2026-10-08:** the per-source share inside it is withdrawn with E6. The key is the address being mailed, which the caller types; it is not the caller's network address | mail-bombing a victim |
| E8 | **A returning device has a reserved lane.** A browser that completed an email sign-in for an address receives a device marker (256 random bits, stored hashed against the address HMAC). A `begin` presenting a valid marker draws on its own allowance of 3 per hour, which E7 does not consume | a distributed attacker exhausting E7 to lock the real user out of email sign-in |
| E9 | **A deployment-wide ceiling on messages per hour**, default 1000, after which sends stop and the security stream says so | the deployment being used as a mail cannon and losing its sender reputation |
| E10 | **A limit that fires is invisible to the caller.** `begin` answers as usual and no message is sent | the limit becoming an oracle for "this address was recently used" |
| E11 | **`begin` does constant work.** Normalise, HMAC, one insert, one enqueue. Whether the address is known is first examined by the worker | enumeration by timing, the same oracle the argon2id decoy closed for passwords |
| E12 | **Comparison is constant-time, and a wrong code, an expired code, a consumed ceremony and a wrong binder are the one answer** | enumeration and ceremony probing |
| E13 | **The address travels in exactly one header and the body is a constant template with one substitution.** The address is validated against a strict grammar with no control characters before it is written | SMTP header injection |
| E14 | **Withdrawn 2026-10-08.** Was: the source address from the socket or `ORDO_TRUSTED_PROXY_HOPS`. Nothing here reads a client's address. Proxy mode (28a, built 2026-10-11) names the header and the trusted proxies: `ORDO_PROXY_HEADER`, `ORDO_PROXY_TRUSTED` | (an attacker choosing their own source by sending a header: no longer reachable, since no source is read) |
| E15 | **An optional human check at `begin`** [new 2026-10-08]. `ORDO_AUTH_HUMAN_CHECK=turnstile` with a secret file. `begin` for email, and for a first guest sign-in, then requires a Turnstile token, verified through `fetch` at Cloudflare's siteverify. Off by default. A failed check is the one answer. Denizens already puts Turnstile in front of every method | a script spraying `begin` from many addresses, which the address-free limits bound but do not stop |
| E16 | **A deleted account gets no mail** [new 2026-10-08]. §2.7 D4 | mailing a person who left |

**Every number above is a setting with a default** (ruled 2026-09-29: *"We do not enforce an app developer's choices but we can have it as default"*). The defaults: code length 8, life 600 s, 5 attempts, 5 an hour and 15 a day per recipient, 3 an hour on the returning-device lane, 1000 an hour for the deployment. A first sign-in also counts against the deployment's sign-up ceiling (60 an hour, §2.6).

**What the limits count, in one table** [revised 2026-10-08]:

| Counted per | Limit | Reads a client address? |
|---|---|---|
| ceremony | 5 attempts, binder-bound (E3, E4) | no |
| recipient address | 5 an hour, 15 a day (E7) | no: the address mailed |
| returning device | 3 an hour on its own lane (E8) | no: a marker the browser holds |
| deployment | 1000 messages an hour (E9); 60 new accounts an hour | no |
| human check | one token per `begin`, if on (E15) | no: Cloudflare may use the address on its side; Ordo sends none |

The residual after E8: a first-time user whose address is under attack cannot receive a code during the window. Without per-source limits that attack no longer needs many machines: one script can spend a victim's hourly allowance. E15 is the answer an app developer can turn on. Their other methods are unaffected. This is stated in the app developer's guide, not hidden.

A second residual, also new with the withdrawal: one script can spend the deployment's 1000 an hour (E9) on many addresses, which stops email sign-in for everyone for the rest of that hour. The ceiling protects the sender's reputation, and E15 protects the ceiling.

### 7.5 Enumeration and the choice of whether to send

| Admission outcome for this address | Message sent | What the caller sees |
|---|---|---|
| Already linked to a principal | yes | the standard `begin` answer |
| Unknown, and the named space admits it (`open`, or `connection` listing `email`, or an invitation addressed to it) | yes | the same |
| Unknown and not admitted anywhere | **no** | the same |
| Linked to a deleted account [new 2026-10-08] | **no** (§2.7 D4) | the same |

The third row sends nothing, so an address that has no business with the deployment is never mailed by it. The only person who can observe the difference is the person who reads that mailbox, and what they learn concerns their own address.

### 7.6 The mail channel [revised 2026-10-08]

**The v1 choice: Ordo's own mail setting.** Not backend functions.

| | Ordo's mail setting | A backend function sends it |
|---|---|---|
| Works before any program or function is deployed | **yes** | no: the app's function must exist first |
| Who sees the code | the host's sender, for one message | **the app developer's code** |
| The send decision (§7.5) and the constant-time `begin` (E11) | in the host, tested once | each app's code, untested |
| Ready when | step 4, size M | backend functions increment 1 (size L), whose placement before the first deployment is still open (`plan-backend-functions.md` Q2) |
| What the owner ruled | R11 names this exactly: secure SMTP, Resend, SendGrid | nothing |

Why, in order:

1. **A code is a bearer secret.** If the app's function sends it, the app's code holds every code for every person. A bug or a log line in one app leaks sign-ins. The host's sender holds one for the length of one submission.
2. **Sign-in comes first.** A fresh deployment must sign people in before it runs any app code.
3. **The backend functions plan already says so.** `plan-backend-functions.md` §11.5: *"Email is the platform's mail."*

What the two share, built once:

- **One private-host list**, `ORDO_EGRESS_PRIVATE_HOSTS` (backend functions §6 and its Q6). A private mail relay is on it. This plan's `ORDO_EGRESS_OPERATOR_HOSTS` is that list. It is built for the mail channel (`3af386be`). Sign-in connections and functions are to read it too.
- **A caller-supplied `Authorization` header on a pinned `POST`.** The mail APIs need it; backend functions' secret injection is the general form. Whichever is built first builds it.

**Later, not v1:** an app developer who wants their own template or provider could name a function to send sign-in mail. The function would then hold codes, so it would be an explicit setting with that warning. Post-deployment, if anybody asks.

**The setting.** A deployment configures one `Mailer`:

```
trait Mailer: Send + Sync { fn submit(&self, msg: &Message) -> Result<(), MailError>; }
```

| Kind | How | Settings |
|---|---|---|
| **SMTP over TLS** | implicit TLS on port 465 by default, or required STARTTLS (`ORDO_MAIL_TLS=starttls`), `AUTH PLAIN`, written out in `ordo-egress` as a module beside `client.rs`, dialled through the egress pins. One connection, one message, a closed set of replies. No new crate (§9.3). Plaintext SMTP is refused at boot | `ORDO_MAIL_KIND=smtp`, `ORDO_MAIL_HOST`, `ORDO_MAIL_PORT`, `ORDO_MAIL_TLS`, `ORDO_MAIL_CREDENTIALS_FILE`, `ORDO_MAIL_FROM` |
| **Resend** | one `POST` of a JSON body with a bearer key, through the egress dialler and pins. The status is the answer; no body is read | `ORDO_MAIL_KIND=resend`, `ORDO_MAIL_KEY_FILE`, `ORDO_MAIL_FROM` |
| **SendGrid** | the same shape | `ORDO_MAIL_KIND=sendgrid`, the same two |

> **Narrowed by the owner, 2026-09-29:** *"Leave just resend, sendgrid as the
> main ones and leave the rest for post deploy."* v1 builds secure SMTP,
> Resend and SendGrid. Mailgun, Postmark, Brevo, Mailjet and SES are checklist
> 18z, after the first deployment. Each is a small module: URL, body shape,
> which status means accepted. SES needs SigV4 signing, so it is the largest.

Delivery is from an outbox drained by a supervised worker, so an unreachable relay or API delays a code and does not block `begin`. One template, one substitution, one recipient, whatever the kind. The template's wording per tenant belongs to 17r.

**Who else uses it.** Password reset (`plan-enrolment.md`) will send through the same `Mailer`. Notifications (17r) may adopt it.

---

## 7A · Guest sign-in [new 2026-10-08, in scope if §13 Q4 is yes]

Denizens offers "Continue as Guest" through `signInAnonymously()`, shown only when its own setting is on. A guest gets a real account with no email and no provider, and uses the app.

**Recommendation: in scope, as step 8, off by default.**

| | |
|---|---|
| Setting | `ORDO_AUTH_GUEST=off` (default) or `on`. Seeded from the environment, stored, changeable by an op, on the self-serve pattern |
| Start | `POST /auth/complete {method: "guest", kind: "new", human?}`. Ordo mints a principal marked `guest`, a 256-bit guest secret, and a session. The secret is returned once. The page keeps it |
| Come back | `{method: "guest", kind: "resume", secret}`. The authenticator is `guest:<SHA-256 of the secret>`, so the secret itself is never stored |
| Becoming a full account | The guest, signed in, **claims** Discord, email or a passkey (step 7). The principal keeps its id, its seats and its data. That is R12's *"Existing account logs in and claims another account"*, with the guest as the existing account |
| First sign-in | as §2.6: counts against the sign-up ceiling, runs the self-serve hook, admission as for anyone |
| What a guest cannot do | Step up (no presence), unless the app developer lowered the requirement. Be found by email: it has none |
| Clean-up | A guest admitted nowhere is removed by the registration sweep, as anyone is (R10). A guest with a seat or a tenant of its own is kept, like any account: the platform deletes none on its own (ruled 2026-09-29, *"if deletion is required thats completely an app dev decision"*). An app developer who wants idle guests removed sets `ORDO_AUTH_GUEST_IDLE_SECS`, off by default |
| Deleted | A deleted guest cannot resume (§2.7 D6) |

**The risk, stated.** With no address-based limit, guest creation is bounded only by the sign-up ceiling (60 an hour) and the human check (E15). A deployment that turns guests on without E15 lets one script spend the ceiling. The setting's documentation says so, and the boot prints one line when guests are on and the check is off.

**Why not the public path's anonymous principal.** `$anonymous` (ruled 2026-10-03) reads public data with no session and no state. A denizens guest chats and keeps history, so it needs an identity of its own.

---

## 8 · Presence and step-up

### 8.1 What these sessions are

A federated, OAuth2 or email sign-in proves that somebody controls an identity at another system or a mailbox. It proves nothing about who is at the keyboard now. The code already says this for OIDC (`provider.rs:694-696`: "presence on somebody else's word") and the types enforce it: a provider outside `passkey.rs` has no way to construct a `UserPresence`.

**Revised 2026-10-08.** The right-hand column is the **default**. The owner ruled on 2026-09-29 that where presence is required, on every platform op, is the app developer's setting (*"What is enforced is app design ... not our design."*). The platform still decides how presence is proved: a provider's claim proves it only under §8.2.

| Such a session **can** | Such a session, under the default, **cannot** |
|---|---|
| Call every op whose step-up requirement is `None`, subject to membership, role, groups and overrides | Call any platform op: `std.deployment.apply`, `std.space.create`, `std.space.import`, `std.space.export`, `std.space.delete`, and the rest of the step-up list registered at `lib.rs:1889` |
| Read and write application data the member's role allows | Call any domain op that the artifact or a client marks `stepUp` |
| Refresh up to the session ceiling and log out | Link another authenticator by `LinkCeremony`. It can claim one by two fresh ceremonies (step 7, R12, built) |
| Accept an invitation | Create or change a connection, or change a space's admission record (both are platform ops) |
| Later step up, **if** the principal has a linked passkey: `refresh_presence` accepts a passkey assertion inside a session any method started, as long as the passkey belongs to the session's principal (`session.rs:560`) | Satisfy step-up by signing in again, however recently |

Consequence, stated plainly: **under the default, a tenant whose owner signs in only through Discord, SSO or email cannot export or delete their own space.** They need a passkey for those acts, or the app developer lowers the requirement, or (step 10) the provider's two-step claim counts. How a person with no presence-capable authenticator enrols their first passkey belongs to the passkey plans; this plan names the seam and does not design it.

### 8.2 A provider's two-step claim as presence: two levels [settled, R9]

**Ruled 2026-09-29, off this plan's recommendation.** The owner: *"2FA from a third party is often a standard... If a company was using external auth, forcing internal auth in addition would be a nuisace."* And, amended the same day: *"A tenant should also decide what it is what they need for their tenant."*

The design that follows, F14:

| Level | Sets | Reaches |
|---|---|---|
| **the app developer** (the deployment), per provider connection | whether that connection's two-step claim counts as presence. This is the default | everything, including acts on the deployment itself: `std.deployment.apply`, `std.space.create`, delete |
| **a tenant**, for its own tenant | the same, for connections it accepts | **that tenant's data and no further.** An act on the deployment follows the deployment's setting whatever a tenant chose |

Mechanism, kept as narrow as the earlier "least damaging form" so the risks below stay bounded:

- The provider yields `UpstreamAssurance` (§3.3) as before. From it, and only when the request carried `max_age=0` (OIDC) or `ForceAuthn="true"` (SAML) **and** the assertion's `auth_time` or `AuthnInstant` is within the step-up window of the server clock, the host mints a claim `upstream_presence: { at, via }` into the token. It is a separate claim with its own type, never a `UserPresence`; `passkey.rs`'s rule stands.
- `AccessEngine` takes a policy: for an op scoped to a space, `upstream_presence` satisfies `StepUpRequirement::Presence` when the deployment's setting for `via` is on, or the space's is. For an op scoped to the deployment, only the deployment's setting counts.
- A **tenant-scoped** connection (F12) can be marked by its own tenant only, and the deployment's setting for it is always off, so one tenant's IdP never grants presence outside that tenant. That is the risk the recommendation named, and this boundary is how it is kept.
- `amr`, `acr`, `auth_time` and `AuthnContextClassRef` are recorded in the security event for every sign-in whether or not they counted.

`plan-second-factor.md` §5.3 reads the same two levels for the second step: a provider whose claim is accepted at the level that is asking gets no second step from Ordo.

Built as step 10 (§10).

The argument this plan made against, kept for the record so the boundary above is not loosened by accident:

1. **The claim is the IdP's word about an event Ordo did not see.** `amr: ["mfa"]` is a string the IdP chose to write. Its meaning varies by vendor, and several IdPs emit it for a session that was established hours earlier.
2. **It cannot be bound to the act.** Step-up means a human approved *this* operation within the last five minutes. An IdP round trip with a live IdP session completes silently. `max_age=0` and `ForceAuthn` ask for a fresh prompt, and honouring them is the IdP's choice.
3. **Tenants will configure IdPs (F12).** A tenant administrator controls every claim their IdP emits. If a claim could mint presence, a tenant could grant itself presence, and the ops behind step-up include ones that act on the whole deployment.
4. **It would be a second way past the gate.** `session.rs:547-549` says of delegation: "A second way past it would be a second thing to get right." The same holds here. Today the property "presence comes only from a passkey" can be checked by reading one module.

The argument against this recommendation, which the owner should weigh:

- **R2.** Requiring a passkey for owner-level acts is an enforced flow for exactly the people who administer a tenant.
- **Enterprise buyers often forbid local credentials.** Their policy is that every authentication happens at the corporate IdP, where phishing-resistant MFA, device posture and conditional access are already enforced, often better than a lone passkey. Telling them their administrators must also enrol a credential at Ordo can fail a security review from the other direction.
- **R7.** Compliance regimes differ, and some will name the IdP as the required control point.

The owner ruled for the second column, and the mechanism above is the least-damaging form this plan proposed for that case, with two changes the ruling made: the deployment's setting reaches deployment acts too, and a tenant may accept a claim for its own data. It is F14.

---

## 9 · Dependencies

The inventory is 226 lockfile entries. The rule applied: a new crate must replace more code than it adds risk, and a crate already in the lockfile is preferred over a new one even when the new one is more convenient.

### 9.1 OIDC and OAuth2: no new third-party crate

| Need | Recommendation | Already present as |
|---|---|---|
| RS256 and PS256 verification | `ring` `RSA_PKCS1_2048_8192_SHA256`, `RSA_PSS_2048_8192_SHA256`, called through `RsaPublicKeyComponents { n, e }`, which takes a JWK's two numbers directly | `ring 0.17.14` in `Cargo.lock` through `rustls`. Verified present in `ring-0.17.14/src/rsa/verification.rs:88,129`. Becomes a direct dependency of `ordo-identity`; adds no lockfile entry |
| ES256 | `p256` | direct dependency of `ordo-identity` |
| EdDSA | `ed25519-dalek` | direct dependency of `ordo-identity` |
| JSON, base64url, SHA-256 | `serde_json`, `base64`, `sha2` | direct dependencies of `ordo-identity` |
| Query and form encoding | `form_urlencoded`, `percent-encoding` | in the lockfile through `axum` |
| HTTP | `ordo_egress::fetch` (§3.4) | workspace |

`ordo-identity` has used RustCrypto crates only. Adding `ring` to it is a second crypto family in one crate. The alternative, RustCrypto's `rsa`, is a new crate with a big-integer stack under it and an open timing advisory against its private-key operations **(unverified: recalled as RUSTSEC-2023-0071; verification is not the affected path)**. `ring` is already trusted by this workspace for every TLS handshake.

Rejected:

| Crate | Why not |
|---|---|
| `openidconnect` | Brings `oauth2`, an HTTP client abstraction, `url`, `chrono` and its own JOSE stack **(unverified, recalled as several dozen transitive crates)**. Its HTTP layer would bypass the egress rail unless replaced, and replacing it leaves mostly type definitions |
| `jsonwebtoken` | Smaller, and built to accept many algorithms through one `decode` call with validation options. O7 and O8 would be configuration of somebody else's defaults. The closed-set verifier is about 200 lines with RFC 7515's own vectors as tests |
| `reqwest`, `ureq` | `client.rs:3-23` already argues this for the workspace |

The argument against hand-writing JWT validation: JOSE has a long history of implementation mistakes, and a maintained crate has met more hostile inputs than new code has. The answer this plan gives is scope. The verifier accepts compact serialisation only, three or four algorithms, no `crit` header (a token carrying one is refused), no nested tokens and no encryption. That is a small enough surface to test exhaustively, and it is the same call `security-logging-design.md` §2.1 made about `webauthn-rs`.

### 9.2 SAML: one new crate, and the largest risk in the plan

| Need | Recommendation |
|---|---|
| XML parsing | **`roxmltree`**, one new crate. **(unverified, from memory: pure Rust, no `unsafe` in its public path, zero or one transitive dependency depending on version, a read-only tree that keeps namespaces, and `ParsingOptions::allow_dtd` defaulting to `false` with a `nodes_limit`.)** S1 is enforced twice: a byte scan for `<!DOCTYPE` before the parser is called, and the parser's own option |
| Exclusive canonicalisation | **in-house**, over the parsed tree, for the subset S7 allows. Roughly 300 lines: namespace rendering by visible use, attribute ordering, text and attribute escaping, no comments |
| XML signature verification | **in-house**: digest, canonicalise `SignedInfo`, verify with `ring`. S5 to S9 are structural checks written as code |
| Certificate to public key | `rustls-webpki`, already in the lockfile (`0.103.15`). **(unverified: `EndEntityCert::verify_signature` verifies a signature under a certificate's key without building a chain, which is the operation a pinned SAML certificate needs.)** If that does not hold, `der` and `spki` are in the lockfile through `p256` and can extract the key |
| Deflate for the redirect binding | **in-house, stored blocks only.** RFC 1951 permits uncompressed blocks, an `AuthnRequest` is under 1 KiB, and an encoder that only emits stored blocks is about 30 lines. No inflate is needed, because the response arrives by POST uncompressed |

Rejected:

| Crate | Why not |
|---|---|
| `samael` | **(unverified)** Verifies signatures only with its `xmlsec` feature, which links `libxml2`, `xmlsec1` and OpenSSL through FFI. That is native code in a workspace that forbids `unsafe` in its own crates, a build dependency on system libraries for every image in `plan-ci-images.md`, and the XML stack with the longest CVE history there is |
| `quick-xml` | A pull parser. The tree, namespace resolution and the duplicate-ID check would be written here anyway |
| A hand-written XML parser | More code to get wrong than the canonicaliser, for no dependency saved that matters |

**The risk, stated:** in-house exclusive canonicalisation and signature checking is the most dangerous code in this plan. A mistake in canonicalisation makes valid assertions fail, which is loud. A mistake in the structural checks makes forged assertions pass, which is silent. The mitigations are S6 (read only what was digested, which makes most structural mistakes harmless), an attack corpus as the done-when of F10, and building SAML last so the shared machinery under it has already met real traffic.

### 9.3 Email: no new third-party crate

SMTP submission written out over `tokio-rustls`, which `ordo-egress` already depends on. `lettre` is the maintained alternative and brings its own MIME, address parsing and connection pooling **(unverified, recalled as thirty or more transitive crates with TLS enabled)**. The message this plan sends is one fixed template with one substitution to one recipient.

`tests/holds_nothing.rs::the_dependency_set_is_the_one_that_was_argued_for` pins `ordo-egress`'s dependency list. Neither `fetch` nor the SMTP module changes that list.

---

## 10 · Increments [revised 2026-10-08]

Each step ships alone and leaves `auth_from_env` refusing whatever is not yet built. Tests are named as sentences, the form `auth_door.rs` uses. **Ordered by what denizens needs first** (§1.3), inside R8. Tier: Opus for anything on the security boundary, Sonnet for well-fenced additions. Size, as in AGENTS.md "Sizing work": S is part of one agent session, M about 1 session, L 2 to 3 sessions.

| Step | Increment | Was | Depends on | Done when | Tier | Size |
|---|---|---|---|---|---|---|
| **1** (built) | **Handoff and binder.** The `handoff` phase and the binder on the existing `$authceremony`; fallible `begin`; `SubjectAttributes` with `VerifiedEmail`; `subject` and `nonce` removed from the callback payload; `StubOidcProvider` behind `#[cfg(test)]`; display name from the verified profile | F1 (the table half is built) | nothing | `auth_ceremony.rs::a_ceremony_begun_by_one_process_completes_in_another`, `::two_racing_completions_produce_one_session`, `::a_handoff_without_its_binder_is_refused` | Opus | M |
| **2** (built) | **`fetch` in the egress kernel.** GET and form POST writers, chunked decoding, per-purpose caps, the opaque `Fetched`; the shared core `plan-backend-functions.md` increment 1 also needs | F2 | nothing | `fetch.rs::a_redirect_is_a_refusal_and_no_second_request_is_made`, `::a_body_over_its_cap_is_refused_before_it_is_held`, `::a_chunked_body_is_read_whole`; `holds_nothing.rs` unchanged | Opus | M |
| **3** (built) | **OAuth2 served, Discord first.** `OAuth2Provider`, `/auth/callback/{connection}`, connections from the environment with the connection fingerprint, presets `discord` and `github` (§5.3), `$identityattr`, `GET /auth/identities`, the `via` claim and the provider admission way reading it, §2.6 (ceiling at the mint, self-serve hook after the handoff), §2.7 D1 to D3 | F6, with F4's served half and F5's `via` | 1, 2 | `auth_oauth2.rs::a_discord_sign_in_through_a_real_tls_listener_mints_a_token_the_door_accepts`, `::the_subject_is_the_configured_claim_and_nothing_else`, `::a_mutable_subject_claim_refuses_the_boot`, `::a_first_sign_in_gets_its_own_tenant_under_self_serve`, `::a_first_sign_in_over_the_ceiling_mints_nobody`, `::a_deleted_account_cannot_sign_in_through_discord`, `::a_deletion_between_callback_and_handoff_mints_no_token` | Opus | L |
| **4** (built) | **The mail channel.** `Mailer`, SMTP over implicit TLS or required STARTTLS, Resend, SendGrid, the outbox worker, the `Authorization` header on a pinned POST | F8 | nothing; runs beside 3 | `smtp.rs::one_message_is_submitted_over_tls_to_a_real_listener`, `::any_port_boots_and_no_plaintext_mode_exists`, `::starttls_upgrades_before_the_credential_and_asks_again_inside`, `::a_relay_that_does_not_offer_starttls_is_sent_no_credential`, `::bytes_injected_after_the_starttls_answer_are_refused`, `::a_plaintext_relay_on_starttls_is_sent_no_credential`, `::an_address_with_a_line_break_is_refused_before_a_socket_opens`; `mail_api.rs::one_message_is_posted_with_a_bearer_key_and_an_accepted_status_is_the_only_success` | Opus | M |
| **5** (built, `b4862c02`) | **Emailed code.** `EmailOtpProvider`, the limits of §7.4 without client addresses, §7.5 with the deleted row | F9, with F7 shrunk to the recipient counters and the device marker | 1, 4 | `auth_email.rs::a_code_signs_in_once`, `::a_code_without_the_asking_browser_is_refused`, `::five_wrong_codes_end_the_ceremony_and_leave_the_address_usable`, `::begin_answers_the_same_for_a_known_and_an_unknown_address`, `::the_sixth_message_in_an_hour_to_one_address_is_not_sent`, `::a_deleted_account_is_sent_no_email_code`, `::no_limit_reads_a_forwarded_header` | Opus | M |
| **6** (built, `5ae9173c`) | **The optional human check** (E15). Turnstile through `fetch`, off by default. Built on `/auth/begin` for every method; steps 5 and 8 call `AuthState::check_human(&body, now)` first in their `begin` (`crates/ordo-host/src/auth/human.rs`) | new | 2, 5 | `auth_human.rs::with_the_check_on_a_begin_without_a_token_sends_nothing`, `::with_the_check_off_no_request_leaves_the_deployment` | Sonnet | S |
| **7** (built, `7097a101`) | **Linking by claim** (R12). The signed-in account claims another identifier by completing its ceremony inside 300 s; both recorded; a deployment may switch it off; D5 | F13 | 3, 5 | `a_stolen_token_cannot_link_because_it_cannot_complete_the_first_ceremony`; `auth_linking.rs::an_existing_account_signs_in_and_claims_another_and_both_then_resolve_to_it`, `::a_verified_email_match_links_nothing`, `::a_claim_into_a_deleted_account_is_refused` | Opus | M |
| **8** (built, `7d55cacb`) | **Guest sign-in** (§7A). Off by default; upgrade is step 7's claim | new | 7 | `auth_guest.rs::a_guest_resumes_with_its_secret_and_with_nothing_else`, `::a_guest_that_claims_discord_keeps_its_id_and_its_data`, `::a_deleted_guest_cannot_resume`, `::guests_off_refuses_a_new_guest` | Opus | M |
| **9** (built, `556b051d`, `ae9ba8b1`) | **OIDC: Google and a company login.** The pure verifier (JWT, JWKS, pins O1 to O17) and the served provider; presets `google` and `microsoft`, and plain OIDC by discovery | F3, F4 | 1, 2, 3 | RFC 7515 vectors; one test per pin O3 to O8, O11, O17, each seen failing with its check removed; `auth_oidc.rs::a_sign_in_through_a_real_tls_idp_mints_a_token_the_door_accepts`, `::a_code_from_one_connection_is_refused_at_another`, `::a_rotated_key_verifies_without_a_restart` | Opus | L |
| **10** (built, `e7c27656`) | **Two-step claims as presence** (§8.2, R9). `upstream_presence`, the freshness rule, the deployment-level setting; the tenant level arrives with tenant-owned providers | F14 | 9 | `::a_fresh_two_step_claim_from_an_accepted_connection_exports_a_space_and_a_stale_one_does_not`, `::a_tenants_acceptance_does_not_reach_a_deployment_op` | Opus | M |

**Steps 1 to 10 are built (2026-10-09).** Where the work is:

- Step 1: `crates/ordo-identity/src/federation.rs`, `provider.rs`, `ceremony.rs` (`take_bound`), `crates/ordo-host/src/ceremonies.rs`.
- Step 2: `crates/ordo-egress/src/client.rs` (`fetch`), test `crates/ordo-egress/tests/fetch.rs`.
- Step 3: `crates/ordo-identity/src/oauth2.rs`, `crates/ordo-host/src/federation.rs`, `crates/ordo-host/src/auth/federated.rs`. Tests: `crates/ordo-host/tests/auth_oauth2.rs`, `auth_ceremony.rs`, and the Discord cases in `provision_own.rs` and `signin_enrolment.rs`.
- Step 4 (commit `3836b802`): `crates/ordo-egress/src/smtp.rs`, `post` and `resolve_and_pin_host` in `crates/ordo-egress`, `crates/ordo-host/src/mail.rs`. Tests: `crates/ordo-host/tests/smtp.rs`, `mail_api.rs`, `mail_outbox.rs`. Settings in `settings.md` §13; guide in `howto-sign-in.md` §9.

- Step 5 (commit `b4862c02`): `crates/ordo-host/src/email_code.rs`, the email arms of `begin` and `complete` in `crates/ordo-host/src/auth.rs`, the composer in `mail::boot_composers`, `$authrate`. Tests: `crates/ordo-host/tests/auth_email.rs` (the seven the table names, plus wrong and expired codes, the daily, device and deployment limits, E5, a duplicate delivery, the human check), `email_outbox.rs` against Postgres, and `provision_own.rs::a_first_email_sign_in_runs_the_self_serve_hook`, `::a_first_email_sign_in_over_the_ceiling_mints_nobody`. SDK: `beginEmailSignIn`, `completeEmailSignIn`. Settings in `settings.md`; guide in `howto-sign-in.md` §10.

- Step 7 (commit `7097a101`): `crates/ordo-host/src/auth/claim.rs`, `Directory::claim` and `Directory::vouches` in `crates/ordo-identity/src/directory.rs`, `CeremonyStore::peek`, and the `claim` the email `begin` takes. Tests: `crates/ordo-host/tests/auth_linking.rs` (`a_stolen_token_cannot_link_because_it_cannot_complete_the_first_ceremony`, `an_existing_account_signs_in_and_claims_another_and_both_then_resolve_to_it`, `a_verified_email_match_links_nothing`, `a_claim_into_a_deleted_account_is_refused`, `a_deleted_accounts_sign_in_cannot_be_claimed_out_of_it`, `a_sign_in_that_names_another_account_is_not_taken_from_it`, `a_matching_verified_email_does_not_stand_in_for_the_first_ceremony`, `a_claim_after_its_window_is_refused`, `a_window_links_once`, `a_window_belongs_to_the_session_that_opened_it`, `a_claimed_address_is_mailed_under_invite_only_and_a_wrong_code_spends_nothing`, `a_hand_off_session_claims_only_through_two_fresh_ceremonies`, `a_claim_takes_no_passkey_assertion_and_a_registration_opens_no_window`, `a_discord_only_account_adds_a_passkey_by_a_claim_and_it_proves_presence`, `a_stolen_token_alone_cannot_add_a_passkey`, `linking_switched_off_refuses_and_names_the_setting`, `the_linking_settings_refuse_what_they_do_not_take_and_name_themselves`), `auth_ceremony.rs::a_look_at_an_open_ceremony_leaves_it_for_the_take` against Postgres, and five unit tests in `directory.rs` (`claim_tests`). SDK: `beginClaim`, `completeClaim`, `claimPasskey`. A passkey by claim: `POST /auth/claim/passkey` and the registration branch of `/auth/claim/complete`. Settings: `ORDO_AUTH_LINKING`, `ORDO_AUTH_LINKING_SECS`, in `settings.md`. Guide in `howto-sign-in.md` §12.

- Step 8 (commit `7d55cacb`): `crates/ordo-host/src/auth/guest.rs` (`GuestProvider`, the setting and its store, `std.deployment.setGuest`), `CompleteRequest::GuestResume` and `GuestNew` and `AuthMethodKind::Guest` in `ordo-identity`, two lines in `/auth/complete`, `registrations::sweep_guests`, `Signup::admit_guest`. Tests: `crates/ordo-host/tests/auth_guest.rs` (`a_guest_resumes_with_its_secret_and_with_nothing_else`, `a_guest_that_claims_discord_keeps_its_id_and_its_data`, `a_guest_adds_a_passkey_through_a_claim`, `a_deleted_guest_cannot_resume`, `guests_off_refuses_a_new_guest`, `the_human_check_blocks_a_first_guest_without_a_token`, `under_invite_only_a_new_guest_is_refused_like_any_newcomer`), `provision_own.rs::the_sign_up_ceiling_counts_guests`, `::a_first_guest_runs_the_self_serve_hook`, `registrations.rs::an_unclaimed_guest_is_swept_after_its_ttl`. SDK: `startGuest`, `resumeGuest`, `guestCeremony`. Settings: `ORDO_AUTH_GUEST`, `ORDO_AUTH_GUEST_TTL_SECS`, in `settings.md`. Guide in `howto-sign-in.md` §13.

What step 8 built differs from §7A in 4 places:

- **A new guest counts against the ceiling whatever the self-serve mode.** The ceiling otherwise counts only under `on-signup`. Nothing else bounds guests, since no limit reads an address.
- **`ORDO_AUTH_GUEST_TTL_SECS` replaces `ORDO_AUTH_GUEST_IDLE_SECS`.** It removes a guest that claimed nothing and that no tenant admitted, sooner than the registration sweep. Removing an idle guest that holds a seat is not built: it is data removal, the app developer's `ORDO_ACCOUNT_REMOVAL` territory.
- **Turning guests off stops new guests only.** A guest that exists still resumes, as any account keeps its sign-in.
- **No per-device limit.** A guest has no device marker before its first sign-in, and a page could leave it out. The ceiling and the human check are the bounds.

What step 7 built differs from the plan in 5 places:

- **The window is a setting.** `ORDO_AUTH_LINKING_SECS`, 60 to 900, default 300 (R7: every number a default with a range).
- **A claimed sign-in that already names another account is refused** with 409 `identifier_taken`, and nothing is moved. Merging two existing accounts is not built.
- **A claim can add a new passkey, vouched for by any sign-in the account holds.** §2.5 kept a first passkey out of F13. The coordinator moved it in on the owner's rulings (*"a login method is a login method"*, *"Passkey requirements are up to the appdev"*), so an account with no passkey can add one. The registration is begun at `POST /auth/claim/passkey`, bound to the account, and stored only once linked. A claim still takes no passkey assertion. `/auth/authenticators/link` is unchanged.
- **An emailed code being claimed is mailed to an unknown address under `invite-only`** when its `begin` names an open claim, as an invitation does.
- **Linking is on by default.** `ORDO_AUTH_LINKING=off` restores the old rule.

- Step 9 (commits `556b051d`, `ae9ba8b1`): `crates/ordo-identity/src/jose.rs`, `oidc.rs`; `crates/ordo-host/src/oidc.rs`, `auth/oidc.rs`, the wiring in `runtime.rs`. Tests: `jose.rs` unit tests `o7_`, `o8_` (including `o8_a_token_without_a_kid_needs_exactly_one_usable_key`) and `o17_`; `oidc_tests.rs` `o3_` to `o15_`, `discovery_refreshes_on_the_cache_schedule_and_keeps_the_last_good_copy`, `the_cache_follows_max_age_inside_the_setting`, `a_token_without_a_kid_verifies_against_a_sole_usable_key`; `crates/ordo-identity/tests/jose_rfc7515.rs` (RFC 7515 A.1 to A.5 and RFC 8037 A.4); `crates/ordo-host/tests/auth_oidc.rs`: `a_sign_in_through_a_real_tls_idp_mints_a_token_the_door_accepts`, `a_code_from_one_connection_is_refused_at_another`, `a_rotated_key_verifies_without_a_restart`, `a_callback_carrying_a_token_in_its_query_is_refused`, `a_discovery_document_naming_another_issuer_is_never_used`, `the_client_secret_appears_in_no_answer_and_no_event`, `an_issuer_on_a_private_address_names_the_setting_that_lets_it_through` (seven tests). Settings in `settings.md` section 1; guide in `howto-sign-in.md` section 14.

What step 9 built differs from §4 in these places:

- (a) Discovery loads on first use, not at boot. It then refreshes on the JWKS cache schedule, which replaces O15's "only at boot or acknowledged refresh". Each refresh is checked against the configured issuer again. A failed refresh keeps the last good copy (`oidc-discovery-stale-copy-used`). A failed first load is retried no sooner than 60 s later.
- (b) The cache length is a setting, `ORDO_AUTH_OIDC_CACHE_SECS` (300 to 86400, default 3600). `fetch` exposes `max-age` (a number, never body bytes), which shortens it and never lengthens it. The 60 s refetch gap stays fixed (O9).
- (c) PS256 is allowed by default with the other three, narrowed by `_ALGS`.
- (d) A token without `kid` is matched to the sole key usable for its algorithm, and refused (`kid-missing`) when two or more are usable. O8 names `kid` only.
- (j) `ORDO_EGRESS_PRIVATE_HOSTS` applies to sign-in connections now, not only mail. A `begin` 503 names the connection and the setting that fixes it.
- (e) The stale-cache note is recorded as an `AuthenticationFailed` event with reason `oidc-jwks-stale-cache-used`, since the stream has no warning kind.
- (f) The pinned endpoints are three (authorize, token, JWKS), not four, because no userinfo call is made.
- (g) `at_hash` is not checked, since the access token is never used.
- (h) Google's endpoints come from discovery. The preset fills only the issuer and claims.
- (i) The key-family check is enforced twice (key admission and the verify dispatch), so the O7 family test fails only with both removed.

- Step 10 (commit `e7c27656`): `crates/ordo-identity/src/upstream.rs` (`UpstreamPresence`, `PresenceMode`, `PresenceRule::judge`, `UpstreamAcceptance`), `access.rs` (`AccessEngine::accept_upstream`, `upstream_decision`), `oidc.rs` (`begin` adds `max_age=0`, `finish` judges), `token.rs` (v6), `federation.rs` (the hand-off carries `amr`, `acr` and `auth_time`, never the claim); `crates/ordo-host/src/oidc.rs` (settings, `presence_setting`), `presence.rs` (acceptance, `DEPLOYMENT_ACTS`, `install_upstream`), `auth.rs` and `auth/oidc.rs` (the `begin` refusal, recording), `admission.rs` (`presenceFrom`); `crates/ordo-secevent/src/event.rs` (`EventKind::UpstreamClaim`). Tests: `crates/ordo-host/tests/auth_oidc.rs`: `a_fresh_two_step_claim_from_an_accepted_connection_exports_a_space_and_a_stale_one_does_not`, `a_tenants_acceptance_does_not_reach_a_deployment_op`, `a_connection_not_accepted_mints_no_presence_even_with_a_fresh_mfa_claim`, `a_single_factor_claim_mints_no_presence`, `the_presence_setting_is_off_by_default`, `asking_presence_of_a_connection_set_off_names_the_setting_to_change`; `crates/ordo-host/tests/admission.rs::a_tenant_accepts_presence_only_from_a_connection_opened_to_tenants`; these two were replaced on 2026-10-09, see (a) below; `crates/ordo-host/src/oidc.rs::the_presence_setting_is_off_by_default_and_names_itself_when_wrong`; six unit tests in `upstream.rs`; `token.rs::an_upstream_presence_claim_round_trips_inside_the_session`. Settings in `settings.md`; guide in `howto-sign-in.md` section 15.

What step 10 built differs from §8.2 in these places:

- (a) Step 10 let a tenant accept only a connection the app developer set to `tenant` or `on`. The owner ruled on 2026-10-09 (federated ask 10, "Open") that a tenant may accept any, as this plan had it. Built since: the setting is `off` or `on`, and `tenant` is gone, because every connection is now open to tenants. A claim is minted whenever asked, whatever the setting. `presenceFrom` refuses only a name that is no connection, listing those there are. `begin` no longer refuses a connection set `off`. Test `auth_oidc.rs::a_tenant_accepts_a_connection_the_app_developer_left_off_and_gets_presence_only_inside_it`; `admission.rs::a_tenant_accepts_presence_from_any_connection_of_the_deployment` replaces the one that pinned the ceiling.
- (b) The tenant level is a tenant accepting a deployment connection, since tenant-owned providers (L1) are not built. L1 still owes the rule that a tenant-owned connection's deployment setting is always off.
- (c) The claim rides in the session claim (token v6), so capability and delegated tokens cannot carry it.
- (d) The freshness window is a setting, `ORDO_AUTH_UPSTREAM_PRESENCE_SECS`, 30 to 300, default 300.
- (e) The `amr` words and `acr` values that count are per-connection settings, `_PRESENCE_AMR` (default `mfa`) and `_PRESENCE_ACR` (default none).
- (f) The acts on the deployment that a tenant's acceptance never reaches are every op in the deployment's own space, plus `std.deployment.*`, `std.space.create`, `std.space.delete` and `std.space.reap`.
- (g) SAML's `ForceAuthn` half waits for SAML.
- (h) `plan-second-factor.md` §5.3 was not built by step 10. The owner ruled on 2026-10-09 (federated ask 11, "yes") and it is built since. `on` meets every level of the second step. A tenant's acceptance meets that tenant's level when the sign-in names it; the deployment's level and the person's own still ask. The door (`SecondStepLevels::refuses`) reads the same rule. A reauth always takes Ordo's step. Recorded as `upstream-claim` act `second-step-satisfied`. Tests in `auth_oidc.rs`: `an_accepted_fresh_mfa_claim_skips_the_second_step`, `a_stale_a_single_factor_or_an_unaccepted_claim_still_takes_the_second_step`, `a_tenants_acceptance_meets_its_own_second_step_and_no_other`.

What step 5 built differs from §7 in five places:

- **Who an unknown address is mailed.** §7.5 reads "the named space admits it". `begin` names no space (step 3 moved the space to `complete`), so the worker asks what the sign-in would: under `ORDO_AUTH_PROVISIONING=auto-onboard` an unknown address is mailed, under `invite-only` it is not. Since 2026-10-09 (25ar, owner ruling on federated ask 9) there is one exception. A `begin` that carries `space` and a valid `invitation` for it marks the ceremony `invited`, and the worker mails that unknown address. The answer to `begin` is the same either way. `complete` checks and spends the invitation again.
- **Settings names.** `ORDO_AUTH_EMAIL_CODE_DIGITS`, `_CODE_SECS`, `_CODE_ATTEMPTS`, `_PER_HOUR`, `_PER_DAY`, `_DEVICE_PER_HOUR`, `_DEVICE_DAYS` (30, new: how long a device marker is honoured) and `_DEPLOYMENT_PER_HOUR` (E9). E9 counts sign-in messages, not every message the channel sends.
- **The windows are clock hours and days**, so up to twice a limit can land across a boundary.
- **E5 keeps the newest `begin`.** A new code ends older ceremonies for the address, and a message for an older `begin` that arrives late is not sent, so it never ends the ceremony the page is waiting on.
- **`via` is `email-otp`**, the method's word, and `k_otp` is derived from the token secret. The device marker is returned as `device` by `complete`.

What step 4 built, and what step 5 calls:

- **Off by default.** `ORDO_MAIL_KIND` unset sends nothing, and the census names `mail.send` as disabled.
- **`Mailer::submit(&Message)`**, synchronous as §7.6 has it. `Message::new(MailAddress, subject, text)` checks everything before a socket opens: a strict address grammar, a printable ASCII subject, printable ASCII lines.
- **The outbox, `$mailoutbox`.** Step 5's `begin` queues with `MailQueue::enqueue(&MailJob::new(kind, to, payload), now)`: one insert (E11). The row holds a kind, the address and a payload, never the message.
- **`Composer::compose(&QueuedMail, now) -> Result<Option<Message>, MailError>`** makes the message on the worker's thread, when it is sent. Step 5 registers its composer in `mail::boot_composers`. It mints the code there, so a code is never at rest, and `Ok(None)` is §7.5's "no message is owed".
- **`mail.send`** claims due rows under a lease, retries a failure that may pass (15 s doubling by default, `ORDO_MAIL_MAX_ATTEMPTS`, default 5) and drops one that cannot. Delivery is at least once.

Four differences from the plan:

- `ORDO_MAIL_CREDENTIALS_FILE` holds the user on line 1 and the password on line 2.
- Ports were first refused (25 and 587). The owner ruled on 2026-10-08: "why are we blocking other ports? It should be configurable". Commit `d777cd93` built it. `ORDO_MAIL_PORT` takes any port from 1 to 65535. `ORDO_MAIL_TLS` is `implicit` (the default) or `starttls`. STARTTLS is required, never opportunistic: no offer, a missing 220, unasked bytes around the upgrade or a failed handshake refuse the send. There was no plaintext mode then. The owner ruled on 2026-10-08: "Allow it, disabled by default but whatever errors says where in settings, or what setting to change." and "security by default, flexibility for quick setups and mockups." Commit `3af386be` built `ORDO_MAIL_TLS=none` (plaintext, only when named, never a fallback) and refusals that name the setting to change. Commit `de418e8c` leases each outbox row when its own send starts.
- The outbox numbers are settings now: `ORDO_MAIL_RETRY_BASE_SECS`, `ORDO_MAIL_RETRY_MAX_SECS`, `ORDO_MAIL_BATCH` and `ORDO_MAIL_LEASE_SECS` (`settings.md` §13).
- A mail API's answer counts as sent only on its one accepted status (Resend 200, SendGrid 202). Another 2xx is not retried, since it may have been sent.
- E9 (the deployment's 1000 messages an hour) is not in step 4. It lands with step 5's limits. A relay inside the deployment's own network is let through by `ORDO_EGRESS_PRIVATE_HOSTS` (built 2026-10-08, commit `3af386be`; `plan-backend-functions.md` Q6). Exact host names only, and never link-local, multicast or reserved addresses.

Three differences from the plan:

- The principal is resolved when the handoff is redeemed at `/auth/complete`, not at the callback. The sign-up ceiling answers `429 quota_exceeded` there, not a `303 ?error=quota_exceeded`, and D3 holds by construction.
- A tenant is named at `/auth/complete` (`space`), as with every other method, not at `/auth/begin`.
- The self-serve hook does not read the platform-op presence list (owner, 2026-10-08) and does not depend on which sign-in method was used, so a Discord sign-in is given a tenant as a password one is. Its test is `provision_own.rs::a_first_discord_sign_in_runs_the_self_serve_hook_after_the_handoff`, beside `provision_own.rs::a_first_sign_in_over_the_ceiling_mints_nobody`.

**Later** (recommended for after the first deployment, §13 Q9 and Q12):

| | Increment | Was | Tier | Size |
|---|---|---|---|---|
| L1 | Tenant-owned providers: ops to create and retire a connection, a registry that changes while serving, sealed secrets (the old Q11), and tenant-owned connections in step 10's tenant level **(built, see `plan-tenant-providers.md`)** | F12 | Opus | L |
| L2 | SAML verifier, pure | F10 | Opus | L |
| L3 | SAML served | F11 | Opus | M |
| L4 | Mailgun, Postmark, Brevo, Mailjet; then SES with SigV4 (18z) | part of F8 | Sonnet | S each, SES M |
| L5 | A per-source limit, once proxy mode exists (28a) | part of F7 | Sonnet | S |

F0 is done (§0.5). F5's tables are built (`admission.rs`); its `via` half is in step 3.

### 10.1 The order, and why [revised 2026-10-08]

**Steps 1 and 2 first**, in parallel. They have no overlap.

**Discord (step 3) is the first outside provider**, which keeps R8: *"Outside providers, then email codes."* It is OAuth2, not OIDC, so OAuth2 comes first. That moves off the earlier order (OIDC first). The reasons that order gave hold for OAuth2 too: no new crate, and it forces the shared machinery (handoff, `via`, the identities record) into existence.

**Mail (step 4) is built beside step 3.** It shares nothing with it.

**Emailed code (step 5)** follows. It is denizens' default sign-in.

**Human check, linking and guests (6 to 8)** finish what denizens' page does today.

**OIDC (step 9) after**, because denizens uses neither Google nor a company login. This is §13 Q1. If you say no, step 9 moves to third and the steps after it move down one.

**SAML stays last.** `roadmap-checklist.md` holds it *"until a buyer asks"*.

**Model tiering** (2026-10-04): each step gets its own short plan first, is built by the tier named, and is audited at the end.

---

## 11 · Questions answered so far [revised 2026-10-08]

| Q | Question | Answer | Where |
|---|---|---|---|
| 1 | First come, first served for one tenant | Reading B, a claim code as a setting; and since 2026-09-29 never a default (notes app ruling) | `plan-enrolment.md` E5 |
| 2 | May the deployment forbid open signup? | Yes: admission is the tenant's, under a ceiling the deployment sets | built, `admission.rs` |
| 3 | Can a provider's two-step claim satisfy step-up? | Yes, at two levels (R9) | §8.2, step 10 |
| 4 | How does a person with no passkey link? | *"Existing account logs in and claims another account."* (R12) | step 7 |
| 5 | Link on a verified email? | No (R12) | §2.5 |
| 8a | How is mail sent? | SMTP, Resend, SendGrid in v1; the rest after (R11) | §7.6 |
| 8b | May the deployment owner name private hosts? | Folded into `plan-backend-functions.md` Q6: one list for both | §3.4 |
| 12 | Keep a sign-in nobody admits? | Yes, 90 days, then removed (R10) | built, `registrations.rs` |
| 14 | When the company removes somebody? | At the session ceiling (R13); faster is 18w | nothing to build |

The old Q6, Q7, Q9, Q10 and Q13 are open and are in §13. The old Q11 (where a tenant's provider secret lives) waits with tenant-owned providers, §13 Q9.

---

## 12 · What this plan does not do

- **It does not serve passkey**, design the first owner's enrolment, or design passkey recovery. `plan-passkey-signin.md`, `plan-enrolment.md` E5 and `plan-passkey-recovery.md` own those. Seams: `refresh_presence` already accepts a passkey inside a session any method started, and S5 gives it a route; a presence-less principal gets a first passkey through a ticket (S7).
- **It does not build the second step.** `plan-second-factor.md` does, and its §5.3 says which of this plan's sign-ins get one.
- **It does not design password enrolment or reset.** Seam: the mail channel (step 4) is what a reset will send through.
- **It does not write `members` rows or define invitation records.** 21b owns the commands. Seam: `Admission::decide` and the verified email in `$identityattr`.
- **It does not build notifications (17r).** It builds one `Mailer` with one template.
- **It does not unlock any platform op by default.** A session from these methods carries presence only under step 10's settings, and a platform op needs presence only where the app developer leaves the default on.
- **It does not parse the `auth { … }` block.** Configuration is environment variables until somebody builds the block's grammar, at which point the variables become its lowering.
- **No IdP-initiated SAML, no single logout, no encrypted assertions, no scheduled metadata refresh.**
- **No SCIM, no back-channel logout, no group or role mapping from IdP claims.** Roles are assigned in Ordo.
- **No sign-in links by email, no SMS.** Denizens' page says "email link" and already accepts the code; its wording changes.
- **No automatic account linking by any attribute.**
- **No storage of upstream tokens**, and therefore no calls to an IdP's APIs on a user's behalf after sign-in.
- **Ordo does not become an identity provider for third parties.** `security-logging-design.md` §4 and §8 already settle that.
- **It does not move the password and passkey providers onto the durable ceremony table.** Their two-request ceremonies have the same multi-replica weakness (§0.3 item 2) and it should be fixed by whoever next changes them.
- **It does not change session length, refresh or rotation.** `durable-auth` ruled those.
- **It does not refuse by a client's network address.** Proxy mode (28a, built 2026-10-11) only slows a busy address, and only once the deployment names its proxy.
- **It does not hand upstream tokens to the app.** A later call to Discord or Google for the person is an app function (`plan-backend-functions.md` Q10).
- **It does not import denizens' users.** The import (28j, `bulk-import`) carries their existing identities; §13 Q11.

---

### Critical Files for Implementation

- `crates/ordo-identity/src/provider.rs` (the trait, `CompleteRequest`, `AuthenticatedSubject`, the stub to retire)
- `crates/ordo-host/src/auth.rs` (the routes, the single refusal, the recorder, the new callback, handoff, identities and ACS handlers)
- `crates/ordo-host/src/runtime.rs` (`auth_from_env`, registry construction, `ORDO_AUTH_PROVISIONING`)
- `crates/ordo-egress/src/client.rs` (the dialler `fetch` shares, the response reader that needs chunked decoding; with `lib.rs:65-75` as the rule being narrowed)
- `crates/ordo-identity/src/directory.rs` (`resolve_or_onboard`, `refuse_deleted`, `insert_link`, `LinkCeremony`: where §2.6, §2.7 and the claim land)

Supporting: `crates/ordo-identity/src/ceremony.rs` (the table step 1 extends), `crates/ordo-host/src/admission.rs` (the provider way reads `via`), `crates/ordo-host/src/self_serve.rs` and `auth/signup.rs` (the ceiling and the hook every first sign-in passes), `crates/ordo-identity/src/session.rs`, `crates/ordo-identity/src/token.rs` (the `via` claim), `crates/ordo-host/src/principal_directory.rs` (the pattern for every new table), `crates/ordo-egress/tests/holds_nothing.rs` (must pass unchanged), `docs/security-logging-design.md` (§2 table and §3 need amending once this is ruled).


---

## 13 · The owner's decisions [new 2026-10-08]

Each has a one-sentence recommendation. Answer on the board.

1. **May OIDC (Google, a company login) come after emailed codes, since denizens needs neither?** *Recommendation: yes; Discord is still the first outside provider, so R8's order holds.*
2. **Does sign-in mail go through Ordo's own mail setting in v1, not through a backend function?** *Recommendation: yes; a function would hold every person's code, and sign-in must work before any function exists.*
3. **Is the optional Turnstile check (E15) built with emailed codes?** *Recommendation: yes, off by default; it stops scripted abuse without reading a visitor's address, and denizens already uses it.*
4. **Is guest sign-in in scope, off by default, upgraded by claiming another method?** *Recommendation: yes, step 8; denizens offers it, and the claim (step 7) lets the guest keep their data.*
5. **Does Ordo own the redirect URI and hand the session over with a one-time handoff?** *Recommendation: yes (§3.2); it keeps the provider's code out of app JavaScript and needs one registration per provider, not one per app.*
6. **May a program's ops read the caller's own linked identities, such as their Discord id?** *Recommendation: yes, read-only and only the caller's own, so denizens writes its Discord link on the server instead of trusting the page.*
7. **Is an emailed code 8 digits, living 10 minutes, with 5 tries, all as settings?** *Recommendation: yes; denizens already uses 8 digits, and mail is often minutes late.*
8. **Once a deleted account's data is removed, may the same Discord account or address sign up again as a new, empty person?** *Recommendation: yes; blocking it would mean keeping the very identifier the removal was meant to erase.*
9. **Do tenant-owned providers, and setting providers by op or console, wait until after the first deployment?** *Recommendation: yes; denizens needs only providers the deployment sets in its environment.*
10. **Which steps come before the first deployment?** *Recommendation: 1 to 8, which is everything denizens' page does; 9 and 10 after, unless a customer asks for Google or a company login.*
11. **When denizens' users are imported, does one Supabase user with both email and Discord become one Ordo account with both linked?** *Recommendation: yes; it carries a join the person already has, which is not the automatic joining R12 forbids.*
12. **Does SAML stay last, built in-house only when a buyer is named, with no encrypted assertions in v1?** *Recommendation: yes; it is the riskiest code in the plan, and nobody has asked for it.*
13. **May a provider connection shorten the session below the deployment's length, never lengthen it?** *Recommendation: yes; a 12-hour emailed-code session is generous, and some compliance regimes want 1 hour.*
