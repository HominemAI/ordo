# Plan: configuration in the `.ordo` file

Date: 2026-11-14. Status: step 1a built (parser, checks, compiled program section, descriptor key, CDK builder); the host does not read the block yet. Source: `docs/audit-configuration-syntax.md`.

## 1. Purpose and the rule

The owner ruled on 2026-11-14 (`audit-configuration-in-the-syntax`, `docs/corpus-rulings.md`): a setting that is not a credential and is not needed before the program is applied is defined in the `.ordo` file. A value that must differ per instance of the same file (apex domain, origins, database, capacity ceilings) is set on the instance. The file declares that need, so the console can show what is missing. The file sets the app developer's value. The deployment owner's ceiling wins. The file can tighten and never loosen. The owner approved this order: the `auth` block, then the `requires` declaration, then gaps 3 to 10 as one batch, each with its SDK equivalent. This plan covers the first two in full and outlines the batch.

Three terms used below:

- **Value**: what the app developer writes in the file.
- **Ceiling**: the deployment owner's bound, an environment setting, usually named `_LIMIT`. It narrows the value and never widens it.
- **Platform range**: the hard range the host already enforces (for example 60 to 900 s for the linking window). A file value outside it is a compile error.

The layers stay those of `settings-layers` (24a to 24d): deployment, app developer, tenant, person. The file is the app developer's layer. Tenant and person layers keep their ops.

## 2. The `auth` block

### 2.1 Where it stands

- **Built, step 1a (2026-11-14).** `crates/ordo-syntax/src/auth.rs` parses the block. `crates/ordo-schema/src/auth.rs` checks it and fills the defaults. The compiled program carries an `auth` section (`ordo_plan::artifact::auth_artifact`). The descriptor carries an `auth` key (`ordo_plan::interface::with_auth`). The CDK writes it with `auth({ … })`. The language reference is `definition-language.md`, "Sign-in: the `auth` block".
- What the build settled that this plan left open:
  - Platform ranges: `window` 1m to 15m, `keep` 1m to 400d, `session token` and both ends of `tenants` 1m to 1d, `session max` 1m to 30d and not below `token`, `remember idle` and `max` 1m to 400d. They follow the host's own floors and default ceilings.
  - Also refused, as `invalid_auth`: a method, provider or domain listed twice, a provider named like a built-in method, an empty `passkeyName`, a `tenants` range that runs backwards.
  - A domain with a hyphen, or a label that starts with a digit, is written in quotes.
  - The descriptor gives each provider's preset beside its name, kind and label, so a page can draw the right button.
  - Left for a later check: `secondFactor required` with no method that can carry a second step. Every method today can carry one, so the rule waits until a method that cannot exists.
- Before the build: `auth` was a reserved keyword in the lexer (`crates/ordo-syntax/src/lexer.rs`) with no parser arm.
- `docs/definition-language.md` section 8 shows an old sketch: `methods passkey, password, email-otp` and `method google { client: $GOOGLE_CLIENT }`. The sketch puts a client id in the file. The ruling forbids that, so the sketch is replaced.
- The host builds `AuthConfig` once at boot in `auth_from_env` (`crates/ordo-host/src/runtime.rs`).
- The app developer layer exists today only as platform ops: `std.deployment.setSessionPolicy`, `setGuest`, `setSelfServe`, `setEmailPolicy`.

### 2.2 Syntax

The block follows `function` and `policy`: one clause per line, a keyword then values, commas between list items, durations as the language's duration literals (`30s`, `5m`, `1h`, `30d`). Keys are camelCase, as in `policy commandlog`. One `auth` block per program. Clauses go in any order.

```
auth {
  methods           passkey, email, google, work
  provider google   preset google
  provider work     kind oidc, label "Work account"
  passkeyName       "Human Agent Link"
  provisioning      domains example.com, example.org
  selfServe         off
  guest             on, keep 7d
  linking           on, window 5m
  secondFactor      optional
  userVerification  required
  session           token 1h, max 12h, tenants 15m to 4h
  remember          on, idle 30d, max none
}
```

Grammar:

```
auth { <clause> ... }
clause = methods <method>, ...
       | provider <name> (preset <preset> | kind oauth2 | kind oidc) [, label "<text>"]
       | passkeyName "<text>"
       | provisioning invite-only | open | domains <domain>, ...
       | selfServe on | off
       | guest off | on [, keep <duration>]
       | linking off | on [, window <duration>]
       | secondFactor off | optional | required | required-without-passkey
       | userVerification required | preferred
       | session token <duration> [, max <duration>] [, tenants <duration> to <duration>]
       | remember off | on [, idle <duration>] [, max <duration> | none]
method = passkey | password | email | <provider name>
```

Compile checks, in `ordo-schema`:

- A method that is not built in names a `provider` in the same block.
- A `provider` that no `methods` line uses is an error, since it would never show.
- A preset must be one the host ships (`discord`, `github`, `google`, `microsoft`).
- A duration outside the platform range is an error naming the range.
- `secondFactor required` with no method that can carry a second step is an error.

### 2.3 Fields

Default is what holds when the clause is absent. "Deployment may" is what the ceiling can do.

| Clause | Replaces | Default | Deployment may |
|---|---|---|---|
| `methods` | `ORDO_AUTH_METHODS` | none: no sign-in | Narrow with `ORDO_AUTH_METHODS_LIMIT`, a list. A file method outside it is off |
| `provider` | `ORDO_AUTH_<NAME>_PRESET`, `_KIND`, `_SCOPE`, `_DISPLAY_CLAIMS`, `_AVATAR_CLAIM`, `_USE_EMAIL` | none | Keep the credentials and the instance facts: `_CLIENT_ID`, `_CLIENT_SECRET_FILE`, `_ISSUER`, `_TENANT`, pinned endpoints, `_ALGS`, `_CLOCK_SKEW_SECS`, `_PRESENCE` |
| `passkeyName` | `ORDO_AUTH_RP_NAME` | the relying party id | Nothing. It is app text |
| `provisioning` | the value of `ORDO_AUTH_PROVISIONING` | `invite-only` | `ORDO_AUTH_PROVISIONING` stays, as the ceiling it already is. Order: `invite-only` < `domains` < `open`. `auto-onboard` keeps working as a spelling of `open` |
| `selfServe` | `ORDO_PROVISION_SELF_SERVE`, `std.deployment.setSelfServe` | `off` | Force off with `ORDO_PROVISION_SELF_SERVE_LIMIT=off`. Sign-up rates stay deployment settings |
| `guest` | `ORDO_AUTH_GUEST`, `_GUEST_TTL_SECS`, `std.deployment.setGuest` | `off` | Force off with `ORDO_AUTH_GUEST_LIMIT=off`. `keep` can only shorten `ORDO_AUTH_REGISTRATION_TTL_SECS`, as today |
| `linking` | `ORDO_AUTH_LINKING`, `_LINKING_SECS` | `on`, 5m | Force off with `ORDO_AUTH_LINKING_LIMIT=off`. Window range 60 to 900 s stays |
| `secondFactor` | `ORDO_AUTH_SECOND_FACTOR` | `optional` (today's `enabled`) | Set a floor with `ORDO_AUTH_SECOND_FACTOR_FLOOR`. Order: `off` < `optional` < `required-without-passkey` < `required`. A tenant still tightens. Attempt and lock numbers stay deployment settings |
| `userVerification` | `ORDO_AUTH_USER_VERIFICATION` | `required` | Hold it at `required` with `ORDO_AUTH_USER_VERIFICATION_FLOOR=required`. The floor's default is `preferred`, so the file may opt into the looser mode |
| `session` | `ORDO_SESSION_SECS`, `ORDO_SESSION_MAX_SECS`, the app layer of `std.deployment.setSessionPolicy` | 1h, 12h, tenants inside the ceilings | `ORDO_SESSION_SECS_LIMIT` and `ORDO_SESSION_MAX_SECS_LIMIT` stay |
| `remember` | `ORDO_REMEMBER`, `ORDO_REMEMBER_SECS`, `ORDO_REMEMBER_MAX_SECS` | `on`, 30d, none | `ORDO_REMEMBER=off` stays, absolute, as today. `ORDO_REMEMBER_SECS_LIMIT` and `ORDO_REMEMBER_MAX_SECS_LIMIT` stay |

Notes on the table:

- The file's defaults are the secure ones. The ceilings' defaults are the loosest the platform allows. So a looser mode is opted into by the file and permitted by the deployment, as `refusals-name-the-setting` asks.
- A file value the ceiling cuts is held at the ceiling. The apply succeeds and its report says so, naming the ceiling setting and its value. A tenant's view (`std.space.sessionPolicy`) already names the source of each value; it gains the source `file`.
- `provisioning domains` is new behaviour. It admits a stranger whose verified email is at a listed domain. A person with no verified email (a passkey-only sign-up) is held as a registration until they verify one. Provider rules per tenant (`host/admission.rs`) are unchanged.
- Not in this block yet: the human check, recovery policy, return URLs, step-up lists, mail text. They are gaps 4, 6, 8 and 9 in the appendix, plus the human check row of the audit.

### 2.4 How the host reads it

- `ordo-syntax` gains an `AuthDecl` node. `ordo-schema` checks it. The compiled program artifact carries an `auth` section, so the block travels with the apply and is versioned with the program.
- `AuthConfig` splits in two:
  - `AuthCeilings`, read from the environment at boot. It also loads every `ORDO_AUTH_<NAME>_*` family present, checked, secrets read from their files.
  - `AuthApp`, built from the active program's `auth` section, joined with the ceilings.
- Activation swaps `AuthApp` in place, behind one shared pointer, in the same step that activates the program. No restart is needed to change sign-in methods.
- At boot the host builds `AuthApp` from the stored active program, with the same function the apply uses. One function, two callers, so boot and apply cannot disagree.
- The tenant and person layers keep their ops and their resolution order: tenant, else file, else deployment.

## 3. The `requires` declaration

> **Renamed 2026-11-14.** The owner chose `requires` over `needs` (ask 9). The text below says `requires` everywhere; an entry is a mandatory requirement, and an unmet one refuses the change. The env-only list is settled (ask 7): the deployment handle, the database, the key provider, the apex domain, the relying party id and the listen addresses are never in the file.

### 3.1 Syntax

`requires` heads a declaration only at the start of a top-level line, like `function` and `secret`. It is not a reserved word, so a field may still be called `requires`.

```
requires {
  apex
  origins
  publicUrl
  database
  mail
  blobs
  secret llmKey
  provider google
}
```

Grammar:

```
requires { <need> ... }
need = apex | origins | publicUrl | database | mail | blobs
     | secret <name> | provider <name>
```

### 3.2 Derived requirements

Most requirements follow from the file. The compiler derives them and the block adds only what it cannot see.

| Need | Derived from | Met by |
|---|---|---|
| `apex` | `methods passkey` | `ORDO_APEX_DOMAIN` |
| `origins` | `methods passkey` | `ORDO_AUTH_ORIGINS` |
| `publicUrl` | any `provider` | `ORDO_PUBLIC_URL` |
| `database` | any entity | `ORDO_PG_*` or `ORDO_DB_CREDENTIALS` |
| `mail` | `methods email`, `notify`, invitations by mail | `ORDO_MAIL_KIND` and its transport settings |
| `blobs` | a `blob` field or parameter | `ORDO_BLOB_STORE` and its settings |
| `secret <name>` | `secret <name> for ...` | the secret's sealed value |
| `provider <name>` | `provider <name>` in `auth` | `ORDO_AUTH_<NAME>_CLIENT_ID` and `_CLIENT_SECRET_FILE`, plus `_ISSUER` for `kind oidc` |

`ordo check` prints the full list, each need with the line that caused it. An explicit line that repeats a derived need is allowed and changes nothing.

A need says that a value must exist. It says nothing about its size. Capacity ceilings (pool sizes, rates) stay deployment settings with defaults, and the file cannot require them.

### 3.3 How the host checks them

- **Met** means the setting is present and the service's own client accepts it at construction. Reachability stays the job of `/health`.
- Each check goes through the service's one client (AGENTS.md rule 9). The database client, the mail client, the blob client and the outside sign-in client each answer `ready()` with their own typed error. The needs check never opens a connection of its own.
- The check returns `Vec<Unmet>`. One `Unmet` holds the need, the settings that would meet it, the file line that caused it, and the client's error as its cause.
- The edge turns it into one answer.

| Where | What the deployment owner sees |
|---|---|
| `ordo plan` | The requirements, met or missing, each unmet one as a refusal (owner, 2026-11-14, ask 6: the check belongs to the planning phase). Nothing is applied |
| `ordo apply` and `std.deployment.apply` | A refusal, `requirements_unmet`, listing each unmet need with its setting. The live program stays active |
| Boot with the stored program | A boot refusal naming the first unmet setting, as today's `BootRefusal::Missing` |
| Boot log and apply report | The same rows, in words. The console is incidental: a deployment may run without it, so nothing below is the only place a message appears (owner, 2026-11-14, ask 8) |
| Console, Settings | A section "Needed by the program": each need, met or missing, its settings, its file line |
| Console, future setup wizard | The missing requirements in order, one screen each, from the same rows |

A new owner-only op, `std.deployment.requirements`, answers the rows the console shows. It reads the stored program and the process environment. The registry (`ordo_host::settings::REGISTRY`) gains a `need` column, so a setting row can say which need it meets.

A deployment never runs with an unmet need. So a public page never sees "incomplete". It sees the methods that work.

## 4. Migration

### 4.1 What happens to each setting

| Kind | Settings | After |
|---|---|---|
| Moves to the file | `ORDO_AUTH_METHODS`, `ORDO_AUTH_RP_NAME`, `ORDO_AUTH_GUEST`, `_GUEST_TTL_SECS`, `ORDO_AUTH_LINKING`, `_LINKING_SECS`, `ORDO_AUTH_USER_VERIFICATION`, `ORDO_AUTH_SECOND_FACTOR`, `ORDO_PROVISION_SELF_SERVE`, `ORDO_SESSION_SECS`, `ORDO_SESSION_MAX_SECS`, `ORDO_REMEMBER_SECS`, `ORDO_REMEMBER_MAX_SECS`, `ORDO_AUTH_<NAME>_PRESET`, `_KIND`, `_SCOPE`, the claim settings | Removed |
| Becomes a ceiling | `ORDO_AUTH_PROVISIONING`, `ORDO_REMEMBER` (`off` only) | Kept, same name, ceiling meaning only |
| New ceilings | `ORDO_AUTH_METHODS_LIMIT`, `ORDO_AUTH_GUEST_LIMIT`, `ORDO_AUTH_LINKING_LIMIT`, `ORDO_PROVISION_SELF_SERVE_LIMIT`, `ORDO_AUTH_SECOND_FACTOR_FLOOR`, `ORDO_AUTH_USER_VERIFICATION_FLOOR` | Added, each unset by default |
| Stays deployment-only | `ORDO_APEX_DOMAIN`, `ORDO_AUTH_ORIGINS`, `ORDO_PUBLIC_URL`, provider credentials and instance facts, every `_LIMIT`, email-code and second-step attempt numbers, OIDC cache and algorithms, sign-up rates | Unchanged |

### 4.2 The transition

- **No `auth` block in the file.** The host reads the old settings for one release, as today. The console shows a banner: "Sign-in is set in the environment. Move it into the file." Recommended in question 3.
- **An `auth` block in the file and an old setting still set.** The boot is refused. The refusal names the setting, the clause that replaces it and the ceiling, if any. Example: "`ORDO_AUTH_METHODS` is set, and the program's `auth` block now says which methods are on. Remove it. To narrow the methods on this instance, set `ORDO_AUTH_METHODS_LIMIT`."
- **After the release.** An old setting refuses the boot with the same text, block or no block.
- **Stored values from the setter ops.** `std.deployment.setSessionPolicy`, `setGuest` and `setSelfServe` stored the app developer's values in `$deploymentsetting`. They are retired for values the file holds. A call answers `set_in_file`, naming the clause. Recommended in question 2.
- **A helper.** `ordo config export` prints an `auth` block from the current environment and the stored values. The deployment owner pastes it into the file. Nothing is lost by moving.

### 4.3 The two test instances

The `ordo` namespace (console at `ordo.inf.smg.icu`) and the `hal` namespace each set sign-in in the environment today. Each moves by: run `ordo config export`, add the block to the file, apply, remove the old settings from the runtime revision, restart. The rehearsal checks that every signed-in person stays signed in across it.

## 5. SDK equivalents

### 5.1 The CDK: writing the blocks in TypeScript

`@hominemai/ordo-cdk` emits `.ordo` text with `synth()`. It gains two builders, typed from the same closed tables the parser uses (`crates/ordo-gen/src/cdk.rs` regenerates `vocabulary.ts`):

```ts
const signIn = auth({
  methods: ['passkey', 'email', 'google'],
  providers: { google: { preset: 'google' } },
  provisioning: { domains: ['example.com'] },
  guest: { on: true, keep: '7d' },
  secondFactor: 'optional',
  session: { token: '1h', max: '12h' },
});
const required = requires(['publicUrl', { secret: 'llmKey' }]);
synth(domain('hal', '1.0.0'), signIn, required, ...rest);
```

A method that names no provider fails to typecheck. `synth` prints canonical `auth` and `requires` text, so the round trip test holds.

### 5.2 The interface descriptor

`ordo.interface/2` (`crates/ordo-plan/src/interface.rs`) gains 2 additive keys. They are additions, so the version stays `/2`.

- `auth`: the declared methods; each provider's name, label and kind; the provisioning mode and its domains; whether guests, linking and self-serve are on; the second factor level; user verification.
- `requires`: the need names and kinds, derived and declared. No setting value.

`ordo gen ts` works offline from the file. So the descriptor states the file's values. The deployment's ceilings may narrow them at run time.

### 5.3 What `ordo-sdk` generates

A new generated file, `src/generated/auth.ts`:

```ts
export type SignInMethod = 'passkey' | 'email' | 'google';
export type Provisioning =
  | { readonly mode: 'invite-only' }
  | { readonly mode: 'open' }
  | { readonly mode: 'domains' };
export const declaredAuth: {
  readonly methods: readonly SignInMethod[];
  readonly provisioning: Provisioning;
  readonly guest: boolean;
  readonly secondFactor: 'off' | 'optional' | 'required-without-passkey' | 'required';
} = { ... } as const;
```

The static core changes:

- `signInMethods()` stays the run-time truth, since a ceiling can turn a declared method off. It becomes generic over `SignInMethod`. A method in the answer that the file never declared is `malformed`, since a ceiling only narrows.
- `GET /auth/methods` adds `provisioning` as the mode alone. The domains stay on the server (owner, 2026-11-14, ask 10): Ordo's basic sign-in page lists none, and the app developer's own page decides what a person sees. A sign-up from an address outside the list is refused with a message that names the rule, not the list.
- `begin` and `beginTenantSignIn` take `SignInMethod`, so a typo fails to compile.
- The platform ops table gains `deploymentRequirements()`, typed `RequirementRow[]`: `{ need, kind, state: 'met' | 'missing', settings: string[], line: string }`. It is owner-only at the door.
- A refusal code `set_in_file` and `requirements_unmet` join the refusal union, with the clause or the requirements as data.

### 5.4 What `ordo-react` gets

- `useSignInMethods()`: the run-time answer, typed by the generated `SignInMethod`. The sign-in screen builds its buttons from it.
- `useProvisioning()`: the mode, so a sign-up screen can say "invitation only" or "use your example.com address".
- `useDeploymentNeeds()`: for the console and an owner's admin page only. It returns the rows, never values.

### 5.5 What stays server-side

- Every ceiling's value.
- Credentials, client ids, issuers, Entra tenant ids, endpoints.
- Which setting is unset, outside the owner-only op.
- Rate and attempt numbers.

## 6. Tests

| Part | Tests |
|---|---|
| Parser | Golden parse of the full example. One refusal per bad clause, with its span. `requires` as a field name still parses. `auth` leaves the reserved refusal tests |
| Schema | Undeclared provider. Unused provider. Unknown preset. Duration outside the platform range, naming the range. Derived needs for each row of 3.2 |
| Host apply | A file value above a ceiling is held and the report names the ceiling. An unmet need refuses the apply and the live program keeps serving. Methods change on apply without a restart. The tenant layer still resolves over the file. Boot and apply build the same `AuthApp` |
| Host migration | Each removed setting with a block refuses the boot, naming setting, clause and ceiling. Without a block the old settings still work for one release. `ordo config export` round-trips to the same effective config |
| Settings registry | `the_registry_and_the_settings_doc_agree` with the new ceilings. The `need` column matches 3.2 |
| One interface | Each service client's `ready()` error carries its cause. The requirements check calls only those |
| Console e2e | Sign-in shows each value with source `file` and its line. Settings shows a missing need, then met after the setting is added |
| SDK | `crates/ordo-gen/tests/generated_files.rs` byte-identical with `auth.ts`. A type test that `begin('discord')` fails to compile when undeclared. Vitest for the new `/auth/methods` field and both refusal codes |
| CDK | `synth` of the builders equals the hand-written example. Vocabulary test covers the new keywords |

## 7. Sizing

| Step | Size | What |
|---|---|---|
| 1a | M | `auth` parser arm, schema checks, artifact section, CDK builder |
| 1b | L | Host: split `AuthConfig`, live swap on apply, ceilings, migration refusals, `ordo config export` |
| 1c | M | Console Sign-in sources, descriptor key, generated `auth.ts`, `ordo-react` hooks |
| 2 | M | `requires` parse and derivation, `ready()` per client, `requirements_unmet`, `std.deployment.requirements`, console section |
| 3 | L per 3 or 4 gaps | The batch in the appendix |

What sets the pace:

- The owner's answers to section 9. Questions 1 to 3 block step 1b.
- CI rounds. A full run is about 35 minutes. Step 1b is likely 2 rounds.
- The rehearsal on the 2 test instances (`ordo` and `hal`), one each, after 1b and 1c. Sessions must survive it.

## 8. Appendix: the batch that follows

3. **Session and remember policy**: done inside the `auth` block above; the batch only retires `setSessionPolicy` for good.
4. **Mail templates and sender name**: `channel email { from "...", template signInCode "...", template invitation "..." }`; transport stays on the instance; needs `mail`.
5. **`policy commandlog` and `policy retention`**: build the parser for the 2 designed blocks; the hardcoded values in `exec/cadence.rs` become their defaults.
6. **Step-up and `delegable` per op**: an op clause `stepUp` and `delegable`; `ORDO_STEP_UP_REQUIRED_OPS`, `_EXEMPT_OPS` and `ORDO_DELEGABLE_OPS` are removed; a rename then moves with the op.
7. **Share-link and public-op policy**: per entity `share { lifetime 7d, password optional }` and per `public` op `humanCheck on, cache 5m`; lockout numbers stay ceilings.
8. **`client` block**: return URLs and CORS origins as named origin roles; the values stay instance needs (`needs origins`), the token lifetimes go in the file.
9. **Recovery policy**: `auth { recovery paths code, tenant; delay code 1d; approvals tenant 2 }`; the `ORDO_RECOVERY_*` values become floors a tenant tightens.
10. **Blob caps and trigger retries**: per blob field `max 20 mb, types image/*`; per trigger `retries 5`; the constants become ceilings.

## 9. Open questions for the owner

1. Should an unmet requirement refuse the apply? **Answered 2026-11-14: the check runs in the planning phase.** `ordo plan` lists each unmet requirement as a refusal, apply runs the same check before it changes anything, and the live program keeps serving.
2. Once the file holds a value, should the setter ops (`setSessionPolicy`, `setGuest`, `setSelfServe`) be retired, with the console editing only ceilings? **Answered yes, 2026-11-14.** The owner adds: a value that cannot change once used is env-only. The list (deployment handle, database, key provider, apex domain, relying party id, listen addresses) is ask 7 on the board.
3. Should a deployment whose file has no `auth` block keep reading the old settings for one release? **Answered yes, 2026-11-14.** The banner is the console's copy of a line that is also in the boot log and the apply report. With a block present, an old setting refuses the boot from the start.
4. Should the compiler derive needs, with the `requires` block only for what it cannot see? **Yes. The file stays the one definition, and most needs are already implied by it.**
5. Should `GET /auth/methods` list the domains of `provisioning domains`? **Answered no, 2026-11-14.** The owner: the sign-in page is the app developer's, who may hide it or gate it with their own logic; Ordo's basic page does not list the domains, and the visuals belong to the developer. The SDK carries the mode alone.
