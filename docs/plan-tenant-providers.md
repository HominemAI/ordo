# Tenant-owned sign-in providers (L1)

`plan-auth-federated.md` row L1, §13 Q9 (the old Q11). Ruled by the owner on
2026-10-09: `corpus-rulings.md`, "Tenant-owned providers".

A tenant's owner adds an OAuth2 or OIDC provider of their own. The tenant's
people then sign in through it, and newcomers it brings land in that tenant.

## 1 · The rulings this builds

- Off by default. The app developer sets `ORDO_AUTH_TENANT_PROVIDERS=on`.
- A tenant's owner adds a provider, with presence (a fresh passkey or an
  accepted two-step claim).
- The client secret is kept in the tenant's own database, sealed with the
  tenant's key, and never shown back.
- A tenant always keeps one owner who signs in without the tenant's own
  providers. A change that would leave none is refused.
- Any provider address is allowed, private addresses too. A cloud metadata
  address is always refused, and so is link-local.
- A tenant's provider admits newcomers into that tenant only. Its two-step
  claim counts as presence only inside that tenant.

## 2 · The setting

| Setting | Values | Default |
|---|---|---|
| `ORDO_AUTH_TENANT_PROVIDERS` | `off`, `on` | `off` |

Off, every op and route below answers a refusal that names the setting and
the value `on`. Turning it off again stops every tenant's provider at once.
The rows stay, so turning it on brings them back.

## 3 · The ops

Platform ops, in the tenant's own space. Owner-only, and presence is required
unless the app developer exempts them (`ORDO_STEP_UP_EXEMPT_OPS`). They are
refused in the deployment's own space, whose providers are its settings.

| Op | What it does |
|---|---|
| `std.space.addConnection` | Add a provider: `name`, `kind` (`oidc` or `oauth2`), its addresses, `clientId`, `clientSecret`, `scope`, claim settings, `role` |
| `std.space.updateConnection` | Change any of those but `kind` and `issuer`. A new `clientSecret` replaces the old one |
| `std.space.retireConnection` | Stop it. New sign-ins through it are refused at once, on every replica |
| `std.space.connections` | List them. The secret is never in the answer; `secret` says `set` |

Why the issuer cannot change: a subject id is unique only within its issuer.
A connection that moved to another issuer would sign in whoever holds the same
subject id there. To move, add a new connection.

Every answer row carries `callback`, the address to register at the provider.

## 4 · Names and the internal key

A connection has the name its owner chose (1 to 32 of `a-z`, `0-9`, `-`) and a
**key** the platform mints: `t` and 20 hex digits from a hash of the tenant id,
`-`, and 10 random hex digits. 32 characters at most.

The key is what the deployment's machinery sees: the ceremony row, the
authenticator id (`oidc:<key>:<sub>`), the session's `via`. So two tenants
that both name a provider `okta` never meet, and a deployment connection
called `okta` is a third, separate thing.

The hash prefix binds a key to its tenant. The access engine reads it to keep a
two-step claim inside the tenant that owns the key (§7). A retired name added
again gets a new key, so people linked through the old one do not carry over.

## 5 · Routes

| Route | Change |
|---|---|
| `POST /auth/begin` | `{"method": "<name>", "tenant": "<space>", "return_to": ...}` |
| `GET /auth/callback/t/{tenant}/{name}` | The provider's redirect URI. New |
| `POST /auth/complete` | `{"method": "<name>", "tenant": "<space>", "kind": "handoff", ...}` |

Why this callback shape: it is distinct per tenant and per connection, it
reads as what it is, and it cannot collide with a deployment connection's
`/auth/callback/{name}`, which has one segment.

Each `begin` and callback reads the connection's row from the tenant's
database. A retired row stops the sign-in on the next request, on every
replica. The built provider is cached by the row's revision, so discovery and
keys are not fetched again while nothing changed.

## 6 · Who it admits

- `complete` names the tenant, so the arrival is at that tenant. A `space`
  naming any other tenant is the one refusal, before anyone is minted.
- A newcomer is minted under the deployment's provisioning, as with every
  method. Under `invite-only` they need an invitation.
- They are admitted at the connection's `role` (default `viewer`), under the
  deployment's ceiling: the provider way must be allowed.
- Built-in provisioning (P2) gives them no tenant of their own.
- A sign-in through a tenant's provider is a sign-in like any other for
  linking by claim (owner: "a login method is a login method"). A person who
  signed up through it may add a passkey by claim. The passkey names the same
  person, so it reaches only the tenants they belong to.

## 7 · Presence

A tenant connection has no `presence` setting of its own.

- Its two-step claim is always made when the sign-in asks (`presence: true` at
  `begin`, `oidc` only).
- It counts as presence only inside its own tenant.
- It counts only once that tenant lists the connection's `key` (from
  `std.space.connections`) in `presenceFrom` of `std.space.setAdmission`.
- Another tenant that lists that key is refused.
- The app developer's `ORDO_AUTH_<NAME>_PRESENCE` never reaches a tenant's
  connection. Deployment-level presence is always off for it.
- It never counts in the deployment's own space, and never for an act on the
  deployment (`presence::DEPLOYMENT_ACTS`).

## 8 · The originating admin

An owner "signs in without the tenant's provider" when they hold an
authenticator that is not one of this tenant's connections: a passkey, a
password, an emailed code, or a deployment connection. While a tenant has an
active connection, these are refused when they would leave no such owner:

- adding a connection;
- `std.space.setRole`, `std.space.suspend`, `std.space.remove` on an owner;
- `std.space.leave` by an owner (account deletion leaves every tenant first);
- `std.space.resetCredential` of an owner whose only such sign-in is a
  password;
- `POST /auth/authenticators/unlink` of an owner's last such authenticator.

The refusal names the way out: make another owner who has a passkey first.

**The check and the write are one step.** Each change takes the tenant's
lock, an advisory lock in a transaction in the tenant's own database, so
every replica sees it. It checks, writes, then lets go. Two changes at the
same moment are taken one after the other, and the second is checked
against what the first did. An unlink holds the lock of every tenant the
person owns, in tenant id order. The lock is asked for without waiting
inside Postgres and asked again after a pause, so a waiting change holds no
connection.

The recovery ops need no check. `std.space.recoverMember` already refuses an
owner. A deployment recovery replaces a person's way in: it adds a passkey as
it retires the old ones, and a contest restores them.

## 9 · Addresses

A tenant's provider is reached through the egress kernel with its own deny
list. Private ranges (RFC 1918, unique-local, carrier-grade NAT) are let
through. Refused always: loopback, link-local, multicast, reserved, the
deployment's own ranges (`ORDO_EGRESS_DENY_CIDRS`), and the cloud metadata
addresses (169.254.169.254, fd00:ec2::254, 100.100.100.200).

The check runs at connect time, on every address the name resolves to. No
address is checked when the connection is added, since a name can change what
it resolves to. `ORDO_EGRESS_PRIVATE_HOSTS` does not apply: it is the app
developer's list for the deployment's own egress.

## 10 · Departures and open points

- Loopback is refused for a tenant's provider. The ruling allows private
  addresses; loopback is the server itself. Owner to confirm.
- `ORDO_EGRESS_DENY_CIDRS` still applies to tenants' providers.
- `presenceFrom` takes a tenant connection's `key`, not its name. Mapping a
  name to a key would tie admission to this module.

## 11 · What was built

- **The setting:** `ORDO_AUTH_TENANT_PROVIDERS` (`off` or `on`, default `off`).
  On, it needs `ORDO_PUBLIC_URL` and `ORDO_AUTH_RETURN_URLS`.
- **The ops:** `std.space.addConnection`, `std.space.updateConnection`,
  `std.space.retireConnection`, `std.space.connections`.
- **The routes:** `POST /auth/begin` and `POST /auth/complete` take `tenant`.
  `GET /auth/callback/t/{tenant}/{name}` is the provider's redirect address.
- **Files:**
  - `crates/ordo-host/src/tenant_providers.rs`: the ops, the registry, sealed secrets.
  - `crates/ordo-host/src/auth/tenant.rs`: begin, callback and complete for a tenant.
  - ordo-identity `upstream.rs`: the tenant keys.
  - ordo-egress `DenyList::for_tenant_providers`: the address rules of §9.
- **Tests:** `crates/ordo-host/tests/auth_tenant_providers.rs`:
  - `a_newcomer_signs_in_through_the_tenants_provider_and_lands_in_that_tenant_only`
  - `the_client_secret_is_sealed_at_rest_and_never_answered`
  - `one_tenants_connection_cannot_sign_in_to_another`
  - `retiring_a_connection_stops_its_sign_ins_at_once`
  - `with_the_setting_off_the_ops_and_begin_name_it`
  - `only_an_owner_with_presence_may_add_a_provider`
  - `the_last_owner_who_signs_in_without_the_tenants_provider_is_kept`
  - `a_provider_on_the_metadata_address_is_refused_at_connect`
  - `a_tenant_connections_two_step_claim_counts_only_inside_that_tenant`
  - `a_person_from_a_tenants_provider_adds_a_passkey_by_claim`
  - `two_concurrent_changes_cannot_both_leave_no_originating_admin`
