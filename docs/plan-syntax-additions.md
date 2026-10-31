# Syntax additions: public data, prev, orphans, settings, external entities

Status: **ruled, 2026-10-03 and 2026-10-04** (`syntax-additions`, all 15
asks). `prev` lives in the source; `ordo evolve` renders and stores plans,
and `ordo apply` offers to remove landed `prev` lines. Nothing here
is built (checklist 27ah). Per-op settings (§7, ask 13) wait for 18ac's
build.

Companions: `definition-language.md` (the language), `plan-public-data.md`,
`plan-evolution-plans.md`, `plan-external-entities.md`, `settings.md`,
`audit-settings-wiring.md`. Checklist: 18ac, 24a to 24f, 27i, 27n, 27o, 27x,
27y, 27ad.

---

## 0 · How to read this

Nine additions, one language. Each section gives:

1. what it is, in one line, and the ruling it comes from;
2. the proposed spelling, with an example;
3. the CDK equivalent (`clients/ordo-cdk`);
4. what the compiler checks, and the error the developer sees;
5. alternatives considered;
6. interactions with the other additions.

The owner's rule for all of it (`forward-gaps`, 2026-10-01): *"Keep
reasoning stupid simple, do as much provable magic under the hood as
possible, and work at producing keywords that have a low probability of
hallucination."* So the additions reuse the positions the language already
has, and add 6 reserved words in total.

### The positions the language already has

| Position | Today | Added here |
|---|---|---|
| Field tail: `name: type` then modifiers | `sealed indexed unique normalize(lower) refinable` | `public`, `prev(old)` |
| Entity line: one word inside the braces | `acl (…)`, `std sealed`, `access read …` | `public`, `public*`, `deployment` |
| Entity header paren | `(key: proj)` | `(prev: old)` |
| Op flag: between the params and `=` | `unbounded unguarded reproducible` | `public` |
| Reserved parameter | `$caller` | `$tenant` |
| Top-level block | `domain`, `entity`, `op` | `settings { }`, `external … { }`, `apikey … { }` |

### One form for every layered value

Settings, API key quotas and live pacing share one line shape, used inside
`settings { }` and inside `apikey { }`:

```
<name> = <value>  [tenant [bounds]]  [person [bounds]]
```

The value is the app developer's choice. `tenant` and `person` hand the
choice down a layer. Bounds (`<= v`, `>= v`, or both) say how far the lower
layer may move it. Section 7 defines it in full.

---

## 1 · `public` and `public*`

**What it is.** A declared mark that makes an entity, a field, or an op
readable by anyone, signed in or not. Ruled 2026-10-03 (`public-data` ask
16; `plan-public-data.md` §3, §5a, §7a; `corpus-rulings.md`, "Public data,
the model").

### Spelling

```
entity plans {
  name:    text    public
  price:   money   public
  margin:  money                   # private: never leaves the deployment
  notes:   text    sealed          # `public` here is refused
  deployment                       # rows live in the deployment's space (§3)
}

entity pages {
  slug:  text unique
  body:  text
  deployment
  public*                          # this entity and its owned children
}

entity pageImages {
  page  -> pages owned
  image: blob?                     # follows its row: public through `public*`
}

op pricing public =                # a public op: cached, unaudited, no guard walk
  plans(price > 0)[name, price]
```

Three places, one word:

| Where | Means |
|---|---|
| Field tail | this field may be served by a public op |
| Entity line `public` | every field of this entity is public, except as refused below |
| Entity line `public*` | `public`, and every entity owned by this one, transitively (the same path G6 inherits guards along) |
| Op flag | this op runs on the public path: anonymous caller, cache layer, no per-call audit, no guard walk |

`public*` lexes as `public` then `*`; no new token. It cascades through
`owned` refs only, never through plain refs, exactly as guard inheritance
does. A child that declares its own `public` or `public*` replaces the
inherited one (the G8 rule, reused).

**Files follow their row** (ruled): a `blob` field on a public row is served
by the blob door with no credential when the row's entity is public and the
field is public. `blob private` is refused under `public` (the platform holds
no key to serve).

### CDK

```ts
const plans = entity('plans', {
  name:   t.text().public(),
  price:  t.money().public(),
  margin: t.money(),
}).deployment();

const pages = entity('pages', { slug: t.text().unique(), body: t.text() })
  .deployment()
  .publicAll();                           // `public*`

const pricing = op('pricing', {}, (b) =>
  b.source(src(plans).where(f.price.gt(lit(0))).pick('name', 'price')),
).public();
```

`public()` joins `sealed()` and `indexed()` on `TypeSpec`; `public()` and
`publicAll()` join `acl()` on `Entity`; `public()` joins `unbounded()` on
`Op`. `FieldNode`, `EntityNode` and `OpFlagsNode` in `core/ir.ts` each gain
a boolean. The printer emits `public` after `sealed indexed` on a field, and
the entity line after `acl`.

### Compiler checks

| Class | Refused | Message |
|---|---|---|
| `public_sealed` | `public` on a `sealed` field, directly or through `public`/`public*` on the entity | `notes` is sealed, so it cannot be public: a sealed value never leaves its tenant in the clear. Mark the fields you mean. |
| `public_private` | `public` on a `blob private` field | `scan` is client-sealed; the platform cannot serve it. |
| `public_tenant_entity` | `public` on an entity, or a field of one, that has no `deployment` line | public data lives in the deployment's space; add `deployment` to `plans`, or remove `public`. |
| `public_reads_private` | a `public` op whose body reads, filters, sorts or groups by a field that is not public, on any source, through any composition | `pricing` is public and reads `plans.margin`, which is not. A public op reads public fields only. |
| `public_caller` | `$caller` or `$tenant` in a `public` op | a public op has no caller; drop the condition or remove `public`. |
| `public_guarded` | `public` on a field of an entity that declares `access read <walk>` or `acl` | `tasks` is guarded, so `title` cannot be public: the two lines disagree about who reads the row. Put public rows in an unguarded entity, or select them in the op body. |
| `public_composes_private` | a `public` read composing an op that is not public | `pricing` is public and sources `internalPlans`, which is not. |
| `public_write_mode` | a `public` write op with a statement that is not CREATE mode | see §2. |

Why `public_reads_private` covers conditions and sort keys, not only
projections: a condition on a private field is an oracle. `plans(margin >
100)[name]` discloses the margin one bit per call.

The check is static and whole-program, which is what lets the runtime skip
the guard walk and the audit row on the public path (ruled, §5a of the
plan). A `public` op is therefore also exempt from G2 and G14: it carries no
`unguarded` flag because it has no guards to bypass; its safety is the field
check.

**Rows.** Every row of a public entity's public fields is public. An entity
whose rows are public one at a time (a draft, then published) selects them
in the public op's body: `op posts public = posts(published)[title, body]`.
The condition is on `published`, which must itself be `public`
(`public_reads_private` otherwise). Visibility shaping lives in the op, as
`$caller` shaping does today.

### Alternatives considered

- **`public` as a contextual word**, like `refinable`. Rejected: it would
  appear in 3 positions (field tail, entity line, op flag), and inside
  braces the lexer emits no newline, so the entity line would need
  one-token lookahead to tell `public` from a field named `public`. A word
  that heads a clause is reserved (the G3 rule for `acl`, `access`). Cost:
  a field can no longer be named `public`; `isPublic` or `visibility` does.
  Draft ask 2.
- **A `public` block listing entities**, like `domain` lists shapes.
  Rejected: the mark would be far from the field it applies to, and a
  half-public row needs per-field marks.
- **Public by op only, no entity marks.** Rejected: the compiler could not
  refuse `public` on sealed fields at the declaration, and a reviewer would
  have to read every public op's body to know what is public. The field
  mark is what makes "nothing is public unless declared" a grep.

### Interactions

- **With `sealed`**: refused (`public_sealed`). With `std sealed`: a public
  op may not be a derived std op over a sealed-inclusive manifest; std ops
  are never public (they are not declared `public`).
- **With `deployment` (§3)**: required. Public rows are the deployment's.
- **With external entities (§8)**: allowed. An external entity is a
  deployment entity with no `sealed` fields, so `public` on its fields is
  always legal.
- **With settings (§7)**: the cache and the rate limit of the public path
  are settings (`public.cacheLife`, `public.readRate`), not syntax.
- **With API keys (§4)**: a key may list a public op; that is harmless and
  lets a customer's metering see it.
- **With grants from tenants** (`public-data` ask 7): a tenant's listing
  reaches the public face as a feed copy into the deployment's space, then
  a public op over the copied entity. No syntax; the feed is the mechanism.

---

## 2 · Public writes into a private entity, write-only

**What it is.** A contact form or a waitlist: anyone may write a row, nobody
reads it back publicly. Ruled 2026-10-03 (`public-data` ask 17): an op the
developer opens, and also a grant the deployment holds, such as a form or
an upload.

### Spelling: the op the developer opens

```
entity contacts {
  name:    text
  email:   text  sealed
  message: text  sealed
  received: datetime
  deployment
}

op contact($name: text(120), $email: text(254), $message: text(4000),
           $received: datetime filled) public =
  set contacts { name: $name, email: $email, message: $message, received: $received }
```

Same flag as a public read. The entity is private: no `public` anywhere on
it, so no public op can read it (`public_reads_private`). The write is an
ordinary write: checked, logged, attributed to the anonymous principal
(`plan-public-data.md` §5a).

### Spelling: the grant the deployment holds

Nothing in the program. The op is declared without `public`:

```
op submitEntry($title: text(200), $photo: blob?) =
  set entries { title: $title, photo: $photo, status: "new" }
```

and the deployment owner, or a tenant owner for a tenant entity, mints a
**form grant** at runtime: `std.deployment.formLink($op, $expires, $label)`
on the pattern of `std.space.shareLink` (`plan-sharing.md` §4.2), **write
ops only**, create mode only. The link's seat is the caller; the grant is
revocable and expires. An upload rides the same grant: the blob door accepts
a put against a row the grant's op created, and nothing else.

Two paths, one rule: a `public` flag is permanent and in the program; a form
grant is runtime state, revocable, and never in a file (the IaC boundary,
DL §9).

### CDK

```ts
const contact = op('contact', {
  name: t.text(120), email: t.text(254), message: t.text(4000),
  received: t.datetime().filled(),
}, (b, p) => b.set(contacts, { name: p.name, email: p.email, message: p.message, received: p.received }))
  .public();
```

### Compiler checks

| Class | Refused | Message |
|---|---|---|
| `public_write_mode` | a statement in a `public` write op whose derived mode is CONVERGE, CAS, CONVERGE-OR-CREATE or BULK; or a `del` | `contact` is public and its `set` addresses a row by `email`. A public write only creates: an anonymous caller may not change what exists. |
| `public_caller` | `$caller` in the body | as §1 |
| `public_unbounded_text` | a `text` parameter with no bound in a `public` write op | `$message: text` has no length. A public write states every bound, so the door can refuse before the row is planned. |

Why create-only: a converge-or-create addressed by a unique key lets a
stranger overwrite a row they guessed the key of. A form only ever makes a
new row. The mode label the compiler prints is `creates`, and it is in the
deploy diff.

Rate: `public.writeRate` is a setting (§7), default unlimited (the owner's
rule for ceilings), applied at the public read layer per address.

### Alternatives considered

- **A separate word, `submit` or `inbox`.** Rejected: the flag already says
  "anyone may call this", and the body's first word already says write.
  Two words for one fact is the drift the closed grammar forbids.
- **Public writes that may converge**, for an "edit your submission" form.
  Rejected for this round: the edit path is a form grant or a share link
  minted for the row, which gives the stranger a credential for that row
  and no other.

### Interactions

- **With `sealed`**: a public write may fill sealed fields. Sealing is about
  reading. The row is sealed on the way in like any other.
- **With `deployment` (§3)**: the entity may be a deployment entity (the
  app's inbox) or a tenant entity (a tenant's own form, through a form
  grant minted by that tenant). A `public` op over a tenant entity is
  refused (`public_tenant_entity`): the public path binds to the
  deployment's space only.
- **With settings (§7)**: `public.writeRate`, `door.bodyLimit` per op.

---

## 3 · The three levels of reading the deployment's space

**What it is.** A tenant reads the deployment's space at one of three
levels, every one decided by the app developer: public, any signed-in
person (the default), or narrowed by a guard. Ruled 2026-10-03
(`public-data` ask 23; `plan-public-data.md` §7a).

### What the guard context carries today

Checked in the code, 2026-10-03. **A guard cannot see the caller's tenant
today.**

| Fact | Where |
|---|---|
| `$caller` binds to the caller's `members.id` in the tenant the request is bound to | `crates/ordo-host/src/lib.rs:1619-1652`; `crates/ordo-sema/src/lib.rs:1575-1620` (`bind_as` inserts `"caller"` into the `ParamEnv`) |
| The only caller state the binder holds is `enum Caller { Slot, Anonymous }` | `crates/ordo-sema/src/lib.rs:437, 522` |
| The guard grammar admits `$caller` and literals; any other `$param` is refused: "guards compare against `$caller` or literals, not `$other`" | `crates/ordo-syntax/src/declparse.rs:777-795`; `GuardAtom::Cmp` at `crates/ordo-syntax/src/decl.rs:226-244` |
| `$space` is bound at the door and carried to the executor as its own argument; an op may not declare it | `crates/ordo-sema/src/lib.rs:132-165` (`check_reserved_params`) |
| The platform entities are `members` and `groupMembers` only; `members` has `role` but no tenant or plan field | `crates/ordo-schema/src/lib.rs:495, 546-553, 588` |
| Role, profile and allowlists are the door layer and never enter the binder (DL G18) | `crates/ordo-identity/src/access.rs:398-445` |
| The system space exists as a constant; its universal read window is not built | `crates/ordo-exec/src/erase.rs:338-362`; `crates/ordo-host/src/provision.rs:128-143`; `docs/roadmap-checklist.md:7962-7965` |
| Anonymous is "no caller": every guard lowers to `false` | `crates/ordo-sema/src/lib.rs:2087-2096` (G15) |

So a guard can reach the caller's **role** today only through data: a walk
that lands on a members row can test `owner.role == "manager"`, but only
when the guarded row refs that member. It cannot reach the caller's tenant
at all, and in the deployment's space the caller usually has no members row,
so every `$caller` guard there is false.

### Spelling

Three things, each small.

**3a. `deployment`: an entity line.** Says this entity's rows live in the
deployment's space only. Tenants have no rows of it.

```
entity announcements {
  title: text
  body:  text
  deployment                      # level 2 by default: any signed-in person reads it
}
```

**3b. `$tenant`: a second reserved parameter.** The row, in the platform
entity `tenants`, of the tenant the request is bound to. `tenants` is
platform-provided, per deployment, undeclarable (the status `members` has),
one row per tenant: `id` (`spc_…`), `name`, `status`, `created`. An app
attaches facts to a tenant by ref, in a deployment entity:

```
entity subscriptions {
  tenant -> tenants unique
  plan:   text                    # "free" / "pro"
  deployment
}
```

`$tenant` is legal where `$caller` is: guards, op bodies, write RHS. Bound
by the runtime from the request's tenant, never suppliable. In a call bound
to the deployment's space itself, or with no tenant, `$tenant` binds nothing
and every guard that names it is false (the G15 rule, extended).

**3c. Caller-side atoms in a guard (G1a).** A guard may test the caller's
own row and the caller's tenant:

```
entity betaNotes {
  title: text
  body:  text
  deployment
  access read   subscriptions(tenant == $tenant and plan == "pro")
                or $caller.role == "owner"
  access write  open
}

entity regionalPrices {
  region: text
  price:  money
  deployment
  access read   region == $tenant.region      # a row field against a caller constant
  access write  open
}
```

The new atom shapes, added to G1's closed list:

| Atom | Means | Compiles to |
|---|---|---|
| `== $tenant` as a comparison RHS | the row's ref points at the caller's tenant | an indexed equality, as `== $caller` |
| `$caller.<path> == <literal>`, `$tenant.<path> == <literal>`, or a bare bool at either | a fact about the caller or their tenant | one constant per call, evaluated at the door before planning; false removes every row |
| `<row path> == $caller.<path>` / `== $tenant.<path>` | a row field against a caller fact | an equality against a per-call constant |

`<path>` is forward hops only, through declared refs from `members` or
`tenants`, plaintext skeleton only (G3 holds: a sealed field anywhere on
either side is `guard_on_sealed`). G2 is widened by one word: a disjunct
references the caller when it names `$caller` or `$tenant`.

The example reads: a note is visible to callers whose tenant holds a `pro`
subscription, or to a tenant owner. The first disjunct is a reverse step
into `subscriptions`, which refs the tenant, so it is a walk G1 already
admits once `$tenant` exists. The second is new.

**How a tenant reaches a deployment entity.** The request is bound to the
tenant as today (`X-Ordo-Space`). The op's sources are deployment entities,
so the planner reads the deployment's partition; `$caller` is the caller's
members row in the tenant, `$tenant` the tenant's row. No window, no second
connection: the universal read window is this rule, built. An op that mixes
a tenant entity and a deployment entity in one pipeline is refused
(`cross_space_join`), because under one database per tenant (ruled
2026-09-25) they are two databases; the join is done on the device or
through a feed.

### The three levels, written

| Level | Who reads | Spelling |
|---|---|---|
| Public | anyone | `public` on the fields, `public` on the op (§1) |
| Signed in | any person signed in, through their tenant | `deployment` on the entity, no `access` clause |
| Narrowed | callers a guard admits | `deployment` and `access read <walk>`, with `$tenant` and caller-side atoms |

### CDK

```ts
const betaNotes = entity('betaNotes', { title: t.text(), body: t.text() })
  .deployment()
  .accessRead(g.walk(g.any(
    g.step('subscriptions', g.all(g.eq('tenant', g.TENANT), g.eq('plan', lit('pro')))),
    g.callerIs('role', lit('owner')),
  )))
  .accessWrite(g.OPEN);
```

`g.TENANT` joins `g.CALLER`; `g.callerIs(path, lit)` and `g.tenantIs(path,
lit)` are the caller-side atoms; `g.eq(path, g.tenantField('region'))` is
the row-against-constant form. `GuardAtomNode` gains `{ k: 'callerSide';
root: 'caller' | 'tenant'; path; lit }`, and the `cmp` atom's `caller:
boolean` becomes `rhs: 'caller' | 'tenant' | { root, path } | null`. The
`p.` namespace gains `p.tenant` beside `p.caller`.

### Compiler checks

| Class | Refused | Message |
|---|---|---|
| `cross_space_join` | a pipeline listing a tenant entity and a deployment entity, directly or through composition | `orders` is a tenant entity and `plans` is the deployment's. They are not one database; read them in two ops, or feed one into the other. |
| `deployment_ref_from_tenant` | a tenant entity with a ref to a deployment entity, or the reverse | a ref cannot cross the partition; store the id in an `id` field and look it up in a second op. (Lifted later if a stable cross-partition lookup is built.) |
| `tenant_in_public` | `$tenant` in a `public` op | as `public_caller` |
| `invalid_guard_walk` | a caller-side path through a sealed field, or a hop that is not a declared ref | unchanged class, new case |
| `caller_side_in_default` | `$tenant` in a shape default | as `caller_in_default` |

### Alternatives considered

- **An op flag instead of an entity line** (`op pricing deployment =`).
  Rejected: the compiler could not refuse a cross-partition join, because
  the partition would be a property of the op and not of the sources. The
  2026-09-12 ruling said the system space needs no placement syntax, which
  was true before any entity had to be served to the public and joined
  against nothing else.
- **Only data-side walks, no caller-side atoms**: write an `audience` link
  entity per deployment entity and test `audience(tenant == $tenant)`.
  Kept as legal, and it is the right shape when visibility is per row. It
  cannot say "every pro tenant reads all rows" without a row per tenant per
  row, which is why the caller-side atom exists.
- **`$caller.role` as sugar for a members walk.** No walk exists to
  abbreviate: the caller's members row is reachable from a deployment row
  by no ref, so the per-call constant is the only compilation.

### Interactions

- **With `public` (§1)**: `public` requires `deployment`.
- **With external entities (§8)**: an external entity is a deployment
  entity (it has no `$space` column), so all three levels apply to it.
- **With settings (§7)**: a developer who wants a tenant to opt in to a
  feed from the deployment's space exposes a setting, and the guard tests
  it through `subscriptions`-style data the setting op writes.
- **With API keys (§4)**: a key issued by a tenant binds `$tenant` to that
  tenant, so a narrowed deployment entity answers a customer's key the way
  it answers the tenant's people.

---

## 4 · API keys for an app's customers

**What it is.** A key the app's customer holds, naming the ops it may call,
with a quota, default unlimited. Ruled 2026-10-03 (`public-data` asks 13,
21). An API key is a capability token (glossary): an op allowlist, a pinned
scope hash, a mandatory expiry.

**Declared in the program, or issued at runtime?** Both, split at the IaC
boundary. The program declares a **key profile**: the ceiling of ops a key
of that kind may name, and the quota default. Keys are **issued at
runtime** against a profile, by a tenant owner for the tenant's customers,
or by the deployment owner for the app's. A key names a subset of the
profile's ops and never more. Grants stay out of files (DL §9); the app
developer still decides what a key can ever reach.

### Spelling

```
apikey storefront {
  ops   listProducts, getProduct, createOrder, orderStatus
  quota.requests = 100000 per 1d   tenant <= 100000 per 1d   # omitted: unlimited
}
```

- `ops` lists registered ops. Platform ops (`std.*`) are refused.
- Every other line is a setting in the shared form (§7), from the `quota`
  section of the settings catalogue: `quota.requests`, `quota.rowsReturned`.
  A rate is `<int> per <duration>`; `per` is contextual, legal only after
  an int in a setting value. No quota line means unlimited, the ruled
  default.
- `tenant` lets the issuing tenant set a lower quota per key at issue time.

Runtime:

```
std.space.issueKey($profile: text, $label: text, $expires: datetime, $ops: list of text?, $quota: …?)
std.space.revokeKey($key: id)
std.space.keys
std.deployment.issueKey(…)         # the deployment owner, for customers that are not tenants
```

`issueKey` mints a seat: a members row with `principal` `key_<id>`, role
null, `displayName` = `$label`, in the issuing tenant. `$caller` binds to
that seat, `$tenant` to the tenant, so guards and `$caller` conditions work
unchanged. The key's manifest hash is pinned per op as every capability
token's is; an op whose manifest expands kills the key until re-issue.

### CDK

```ts
const storefront = apikey('storefront', {
  ops: [listProducts, getProduct, createOrder, orderStatus],
  quota: { requests: rate(100000, dur(1, 'd')).tenant({ max: rate(100000, dur(1, 'd')) }) },
});
```

`apikey()` is a new declaration in `core/build.ts`; `ApikeyNode { kind:
'apikey'; name; ops: string[]; settings: SettingLineNode[] }` joins
`Declaration`. `rate()` is a new value constructor in `core/value.ts`.

### Compiler checks

| Class | Refused | Message |
|---|---|---|
| `apikey_platform_op` | a `std.*` op in `ops` | a key never calls a platform op. |
| `apikey_unknown_op` | a name that is not a registered op | `orderStatus` is not an op in this program. |
| `apikey_step_up_op` | an op that requires step-up | `exportAll` requires presence, which a key cannot give. |
| `unknown_setting` | a line that is not a `quota.*` setting | `quota.calls` is not a setting; did you mean `quota.requests`? |
| `apikey_empty` | no `ops` line | a profile names at least one op. |

At issue time the door refuses `$ops` outside the profile
(`key_beyond_profile`) and a quota above the profile's `tenant` bound
(`setting_out_of_bounds`).

### Alternatives considered

- **No profile: issue any ops at runtime.** Rejected: the app developer
  would have no say in what a key can reach, and quotas would have no home
  in the program (18ac wants every setting in the program).
- **The allowlist itself in the program** (`apikey acme { ops … }` per
  customer). Rejected: that is a grant in a file.
- **A `principal` of kind `apiKey`.** The `principal` block is `[lean]` and
  names one outsider at a time (`cameraHub`). A profile is a template for
  many; the shape differs.
- **`client` block.** A client is an OIDC relying party. A key holder never
  signs in.

### Interactions

- **With settings (§7)**: quotas are settings. `tenant` on the quota line
  is the same word with the same meaning. The individual act (one key's
  quota) is the `$quota` argument of `issueKey`, checked against the
  tenant's effective value.
- **With `$tenant` (§3)**: a key issued by tenant T answers deployment
  entities narrowed by T's facts.
- **With `public` (§1)**: a key may list public ops; metering then sees them.
- **With external entities (§8)**: an op over an external entity may be
  listed; the customer reads the other system's table through Ordo's door,
  which is the ruled direction (§7 of that plan).

---

## 5 · `prev`: renames and correspondence across versions

**What it is.** A declaration that a renamed entity or field is the same
thing as a name in the previous version, so the plan renders a rename and
never a drop-and-add. Ruled 2026-10-01 (`forward-gaps` asks 6, 8, 18;
checklist 27x), with the owner's spelling `entity derpderp (prev blabla)
{}`, `field macaroni (prev c)`. Where it lives, source or plan, was deferred
to this design (2026-10-03).

### Spelling, in source

```
entity customers (key: cust, prev: clients) {       # was `clients` in the previous version
  fullName:  text  prev(name)                       # was `name`
  email:     text  sealed indexed unique normalize(lower)
  note:      text? prev(orders.shippingNote)        # moved here from a related entity
}
```

- **Entity**: `prev: <oldName>` in the header paren, beside `key:`. The
  paren is the header's option slot already (`(key: proj)`), and its
  entries are `word: value`.
- **Field**: `prev(<old>)` in the modifier tail, the shape `normalize(lower)`
  has. `<old>` is a field of this entity in the previous version, or
  `entity.field` for a move.
- **Reach**: one version back only (ruled). A `prev` names something in
  the version serving now.
- **Splits and merges** build on it: two fields each `prev(name)` is a
  split; `prev(first, last)` is a merge. The mapping expression for either
  is a plan option (`forward-gaps` 7, 15), rendered when the plan sees two
  targets or two sources and refused with the options if none is chosen.

### Spelling, in the plan

The alternative, given in full because the owner asked for both.

`ordo plan` diffs the versions and finds a dropped `clients` and an added
`customers` with the same fields. It renders the options (`rename clients to
customers`, `drop and add`) and refuses until one is chosen:

```
ordo plan . --against prod --choose clients=customers --choose customers.name=fullName
```

The choice lands in the stored plan and in the receipt file beside the
`.ordo` sources. The `.ordo` carries nothing about the rename.

### Recommendation: source, and the receipt repeats it

- A rename is a fact about the program. The reader who meets `customers`
  wants to know it was `clients` where the new name is, in the diff of the
  file they are reviewing.
- Agents write Ordo. A grammar that cannot say "this is the old X" makes
  the agent say it in a command-line flag the next run forgets.
- The receipt still records the step (ruled: "a receipt that can be
  committed"), so reversibility is kept: the reverse of a rename is the
  rename read backwards, generated with the forward one.
- The cost is one line that goes stale after the step lands. Stale is a
  warning, never a refusal, so housekeeping never blocks a deploy.

The tension with the 2026-10-01 ruling (choices live in the plan) is real
and stated: `prev` moves one kind of choice into the source. The owner
flagged it (ask 23) and deferred to this design. Draft ask 1.

### CDK

```ts
const customers = entity('customers', {
  fullName: t.text().prev('name'),
  email:    t.text().sealed().indexed().unique().normalize('lower'),
  note:     t.text().optional().prev('orders.shippingNote'),
}, { key: 'cust', prev: 'clients' });
```

`prev()` on `TypeSpec` (refused on a parameter, like `sealed`);
`EntityOptions.prev`; `FieldNode.prev: string[] | null` (a merge has two);
`EntityNode.prev: string | null`.

### Compiler and plan checks

The compiler never sees a database, so `prev` is checked twice: shape at
compile time, correspondence at plan time.

| Class | When | Refused | Message |
|---|---|---|---|
| `prev_self` | compile | `prev` naming the field's or entity's own name | `fullName prev(fullName)` says nothing. |
| `prev_still_declared` | compile | `prev(name)` while `name` is still a field of the entity | `name` still exists, so `fullName` is not its rename. Remove one. |
| `prev_duplicate` | compile | two fields of one entity with the same `prev` target | both `first` and `fullName` claim `name`. A split needs the plan's mapping; name the split in one of them. |
| `prev_type` | plan | a rename whose types differ with no cast option chosen | `clients.name` was `text` and `customers.fullName` is `text(80)`. Options: cast, with the 3 rows that would truncate listed; refuse. |
| `prev_unresolved` | plan | the named previous thing is not in the served version | the version serving now has no `clients`. `prev` reaches one version back only. |
| `prev_stale` | plan, warning | the rename already landed in the served version | `customers (prev: clients)` is done; remove the mark. |

A `prev` on an entity makes the plan render **rename** (copy, swap, drop)
with reverse statements, in place of the drop-and-add the diff would
otherwise propose and refuse. Under dual writes (ruled 2026-10-01) both
columns stay current through the soak, so the reverse is a switch.

### Alternatives considered

- **The owner's exact spelling in both places, `(prev x)` with no colon.**
  Not taken, because the header paren's one existing entry is `key: proj`,
  and a slot with two entry shapes is two spellings of one thing. If the
  owner prefers `(prev clients)`, `key:` should lose its colon too, in the
  same change.
- **A `renames { clients -> customers }` block.** Rejected: far from the
  declaration, and `->` already means a ref.
- **Inferring renames from field similarity.** Rejected: that is the
  non-deterministic area the owner named; a guess that is wrong moves data.

### Interactions

- **With orphans (§6)**: a field that is removed and has no `prev` pointing
  at it is orphaned. A field with a `prev` pointing at it is renamed, and
  nothing is orphaned.
- **With external entities (§8)**: `prev` is legal on an external entity's
  name and fields, because a translation is regenerated when the foreign
  table changes (§3 of that plan) and the regenerated version carries
  `prev` so the plan knows which Ordo field moved.
- **With settings (§7)**: a renamed setting is a catalogue matter, not a
  program one; the catalogue keeps old names as aliases for one release.

---

## 6 · Orphan instead of drop

**What it is.** A field or entity the program stops declaring is kept,
unserved, reported, and never dropped by default; dropping is an explicit
step. Ruled 2026-10-01 (`forward-gaps` ask 20; checklist 27y).

### Spelling: the default needs no words

Removing a declaration orphans it. The program says nothing, and the plan
says:

```
orphaned (kept, unserved):
  customers.fax          since v12   text?    1,204 rows hold a value
  legacyNotes            since v9    entity   388 rows
```

The column or table stays. The new version never reads or writes it. New
rows have no value for an orphaned column, so an orphaned NOT NULL column
has its NOT NULL lifted in the same expand step (class A, additive).
Reverting to a version that declared it is a switch (ruled): the data was
never gone.

### Spelling: pruning is a plan choice, never a source line

Ruled 2026-10-03 (ask 11): *"yes - but maybe prune is better with an extra
confirmation."* The word is **prune**, and it takes a second confirmation.

```
ordo plan . --against prod --prune customers.fax --prune legacyNotes
```

The choice lands in the stored plan and the receipt, runs under the
destructive-change confirmation DL §9 already requires (explicit
confirmation, the apply is a command with an actor, a BYO-DB tenant's owner
confirms), and the plan prints the row counts it will destroy. Then a second
confirmation at apply time names each pruned field or entity and its row
count, typed back by whoever applies it.

`ordo prune` is already a command: it trims the command log. The plan flag
`--prune` lives under `ordo plan`, so the two never share a command line.

Why not a source spelling (`drop customers.fax`): a drop is an act on one
deployment's data, with an actor and a confirmation. It can differ between
dev and production, and it happens once. Source carries facts about the
program. The same reason `prev` belongs in source puts `drop` in the plan.

### Reclaiming an orphan

A later version that declares `customers.fax` again, with the same type,
reattaches it: the plan says `reclaimed: customers.fax (orphaned since v12)`.
A different type is a class B change and renders its options.

### CDK

Nothing. The CDK emits the program, and the program has no words for this.

### Compiler and plan checks

| Class | When | Refused | Message |
|---|---|---|---|
| `orphan_name_reused` | plan | a new declaration whose name matches an orphan of a different kind (a field where an entity was) | `legacyNotes` is an orphaned entity; a field cannot take its name until it is pruned. |
| `prune_unknown` | plan | `--prune` naming something that is not orphaned | `customers.fax` is still declared. Remove it from the program first. |
| `orphan_prev` | compile | a `prev` pointing at a name the program still declares (see §5) | as `prev_still_declared` |

### Alternatives considered

- **`orphan` as an explicit source mark** (`fax: text? orphan`). Rejected:
  the ruled default is orphan, so the mark would say what omission already
  says, and a field that is still declared is not orphaned.
- **Drop after N versions automatically.** Rejected: "pruning is an
  explicit step", ruled.

### Interactions

- **With `prev` (§5)**: a `prev` turns a would-be orphan into a rename.
- **With external entities (§8)**: an external entity cannot be orphaned or
  dropped by Ordo, because the table is not Ordo's. Removing its
  declaration simply stops serving it; the plan lists the table under
  "present, not Ordo's" (27j).
- **With `public` (§1)**: an orphaned public field stops being served at
  once, and the public cache drops every result that read it (the exact
  invalidation the plan already requires).

---

## 7 · Settings in the program and the CDK

**What it is.** Every setting an app developer may own gets a spelling in
the program and a function in the CDK, with the layers the developer
exposes declared there. Ruled 2026-09-29 (checklist 18ac, 24a to 24f;
`settings.md`, `audit-settings-wiring.md`). The principle, in the owner's
words: limits and choices are layered, a tenant or person can change a
setting when the app developer exposes it, never one hardcoded system
value.

### The four layers, and where each is written

| Layer | Who | Where it is written |
|---|---|---|
| Deployment default | Ordo ships it; the deployment owner may change it by environment or console | the settings catalogue; `ORDO_*` |
| App developer | the program | `settings { }` |
| Tenant | a tenant owner or manager | `std.space.setSetting`, only where the program says `tenant` |
| Person | one person, for themselves | `std.person.setSetting`, only where the program says `person` |
| The individual act | an op parameter (`$expires` on a share link) | checked at the door against the effective value |

The effective value is resolved top down: the deployment's default, then the
program's value if set, then the tenant's if exposed and set, then the
person's if exposed and set, then the act's parameter, each checked against
the bounds the layer above declared.

### Spelling

```
settings {
  session.life            = 12h     tenant <= 12h   person <= 12h
  share.linkLife          = 7d      tenant <= 30d
  commandlog.undoHorizon  = 90d     tenant >= 90d           # a tenant may lengthen, never shorten
  admission.defaultRole   = "viewer"
  live.refetchPace        = 250ms                           # §9
  door.pageSize           = 50
  public.cacheLife        = 60s
  public.writeRate        = 60 per 1h
  stepUp.freshness        = 300s    tenant <= 300s
}
```

One line per setting:

```
<name> = <value>  [tenant [<= v] [>= v]]  [person [<= v] [>= v]]
```

- **`<name>`** comes from the settings catalogue, a closed table the
  compiler holds: `settings.md`'s inventory, section by section, each with
  its type, Ordo's default and which layers it may be exposed to. Dotted,
  lowerCamel, from a closed list. An unknown name is refused with the
  nearest names.
- **`= <value>`** is the app developer's value. Omitting the line keeps
  Ordo's default. The type is the catalogue's; it is not written again.
- **`tenant`** exposes the setting to tenant owners and managers; **`person`**
  to each person for themselves. Omitted, the layer cannot change it.
- **Bounds** after a layer word say how far that layer may move it. `<= v`
  is a ceiling, `>= v` a floor, both may be given. No bound: any value of
  the type. Bounds are checked at the door when the layer sets, and the
  compiler checks the developer's own value lies within any bound it
  wrote.
- **Values**: the literal grammar the language has, plus `<int> per
  <duration>` for rates. No expressions.

Where it goes: one `settings` block per program, top level, anywhere.
Two blocks are `duplicate_settings`.

### Per-op settings

`door.pageSize` and `door.bodyLimit` are per op in `settings.md`. This round
sets them program-wide. The per-op form, filed for 18ac's build and not
decided here: a header entry `op listProducts settings(door.pageSize = 100)
=`. Draft ask 13.

### Runtime

```
std.space.setSetting($name: text, $value: …)      # owner or manager; checked against the program's bounds
std.space.settings                                 # every exposed setting with its effective value and who set it
std.person.setSetting($name: text, $value: …)
std.person.settings
```

One store in the control database, keyed by setting name and tenant (or
principal); one resolver; recording to the deployment stream and the
tenant's own (`audit-settings-wiring.md`, "Shared machinery", the four
pieces 24a names). Values read on every request are cached per replica
and announced on change, as membership changes are.

### CDK

```ts
settings(
  s.session.life(dur(12, 'h')).tenant({ max: dur(12, 'h') }).person({ max: dur(12, 'h') }),
  s.share.linkLife(dur(7, 'd')).tenant({ max: dur(30, 'd') }),
  s.commandlog.undoHorizon(dur(90, 'd')).tenant({ min: dur(90, 'd') }),
  s.admission.defaultRole(lit('viewer')),
  s.live.refetchPace(dur(250, 'ms')),
  s.public.writeRate(rate(60, dur(1, 'h'))),
);
```

`s.*` is **vocabulary**: generated from the catalogue by `ordo gen ts`, one
typed function per setting, so an unknown name is a TypeScript error before
it is a compile error. `.tenant()` / `.person()` with optional `{ min, max
}` are **skeleton**. `SettingLineNode { name; value; tenant: Bounds | null;
person: Bounds | null }` and `SettingsNode { kind: 'settings'; lines }` join
`core/ir.ts`.

### Compiler checks

| Class | Refused | Message |
|---|---|---|
| `unknown_setting` | a name not in the catalogue | `sessions.life` is not a setting. Did you mean `session.life`? |
| `setting_type` | a value of the wrong type | `session.life` is a duration; `12` is an int. |
| `setting_not_exposable` | `tenant` or `person` on a setting the catalogue keeps at the deployment layer | `passkey.ceremonyLife` is the deployment's; a tenant cannot set it. |
| `setting_out_of_bounds` | the developer's value outside a bound they wrote | `share.linkLife = 60d` is above its own `tenant <= 30d`. |
| `setting_bounds_empty` | `>= v` above `<= v` | the floor is above the ceiling. |
| `duplicate_setting` | one name twice | `session.life` is set twice. |
| `duplicate_settings` | two `settings` blocks | one block per program. |
| `setting_rate_only` | `per` on a setting that is not a rate | `door.pageSize` is a count, not a rate. |

Changing a setting's value is a class A change: no migration, applies on
activation. Adding `tenant` is additive. Removing `tenant` from a setting
tenants have set is refused with the options: keep their values as frozen,
or reset them to the program's value.

### Alternatives considered

- **Keep the `policy` blocks** (`policy commandlog { undoHorizon 90d }`).
  Rejected: they never parsed (2026-09-16), they group by subsystem rather
  than by what a reader is looking for, and they have no place for the
  layer words. `policy` stays reserved until 18g retires it. Draft ask 6.
- **A type on every line** (`session.life: duration = 12h`). Rejected: the
  catalogue holds the type; a second statement of it can only drift.
- **App-defined settings** readable in op bodies (`$settings.maxSeats`).
  Deferred: a value in expressions is a different feature with its own
  determinism questions. Draft ask 7.
- **Bounds as a range literal** (`tenant 1d..30d`). Rejected: the language
  has no range form and `<=` already means a bound on parameters.

### Interactions

- **With API keys (§4)**: quota lines inside `apikey` are settings lines;
  the catalogue's `quota` section defines them.
- **With live pacing (§9)**: `live.refetchPace` is a settings line.
- **With `public` (§1, §2)**: the public path's cache life and write rate
  are settings.
- **With external entities (§8)**: `external.checkEvery` (how often the
  foreign table is compared with its translation) is a setting.
- **With `$tenant` (§3)**: a tenant's choice of an exposed setting is data
  a guard may test, through a deployment entity the setting op maintains.
  `std.space.settings` is the read.

---

## 8 · External entities

**What it is.** A generated, read-only translation of a table Ordo does not
own, served as an ordinary entity. Ruled 2026-10-01 (`external-entities`;
checklist 27i; `plan-external-entities.md` §2).

### Spelling

```
external customers (key: cust) from "crm"."customers" {
  id              <- "customer_id"               # the foreign key column; prefixed at query time
  name:   text    <- "full_name"
  email:  text    <- "email"             public
  since:  date    <- "created_at"
  owner -> crmUsers <- "owner_id"                 # a ref to another external entity
  access read   region == $tenant.region
  access write  open
}
```

- **`external`** opens the block. **`from`** is contextual, legal only here,
  and names the foreign table as two string literals, schema then table.
  Strings because foreign names carry underscores and case that Ordo
  identifiers forbid.
- **`<-`** maps a field to a column. A new two-character token, the mirror
  of `->`: data comes in. Elsewhere it is a parse error (`x <- 1` no longer
  lexes as less-than-minus; write `x < -1`).
- **`id <- "…"`** is required: the foreign primary key, carried as a typed
  id with the entity's prefix at query time and stripped on the way back.
- Every field states its Ordo type, so ops type-check without the foreign
  catalogue.
- Refs point at other external entities only, through the column that holds
  the foreign key.
- An external entity is a **deployment entity** (it has no `$space`), so
  `deployment` is implied and §3's three levels apply: `public` fields, any
  signed-in person by default, or a guard.

The block is **generated** by `ordo introspect` (`migration.md`) and
regenerated when the foreign table changes; a person edits the guard, the
`public` marks and the field names, and commits.

### CDK

```ts
const customers = external('customers', ['crm', 'customers'], {
  id:    col('customer_id'),
  name:  t.text().from('full_name'),
  email: t.text().from('email').public(),
  since: t.date().from('created_at'),
  owner: t.ref(crmUsers).from('owner_id'),
}, { key: 'cust' })
  .accessRead(g.walk(g.eq('region', g.tenantField('region'))))
  .accessWrite(g.OPEN);
```

`external()` is a new declaration; `.from(column)` on `TypeSpec` (refused on
a field of an ordinary `entity`: `external_only`); `ExternalNode` joins
`Declaration`.

### Compiler checks

| Class | Refused | Message |
|---|---|---|
| `external_readonly` | `set` or `del` on an external entity | `customers` is another system's table; Ordo reads it and does not write it. |
| `external_modifier` | `sealed`, `unique`, `indexed`, `normalize`, `owned`, `tree`, `index`, `check`, `acl`, a default, `blob` | `email` is a column Ordo does not own; it cannot be sealed. |
| `external_unmapped` | a field with no `<-` | `name` names no column. |
| `external_no_id` | no `id <-` line | the translation needs the table's key. |
| `external_ref_target` | a ref from an external entity to an ordinary entity, or from an ordinary entity to an external one | a ref cannot cross into a table Ordo owns from one it does not. Store the id in an `id` field. |
| `external_type` | plan time: the column's type cannot carry the declared type | `created_at` is `text` in `crm.customers`; `date` needs a cast. Options: cast with the rows that fail listed; declare `text`. |
| `external_gone` | plan time: the table or a mapped column is missing | `crm.customers.full_name` is gone. Regenerate the translation and review the difference. |

### Alternatives considered

- **`from` as a reserved word.** Rejected: `from` is a plausible field name
  (`messages.from`) and `declparse.rs` already refuses it as a borrowed SQL
  spelling in source position with a hint. Contextual, in one position,
  keeps both.
- **`=` for the mapping** (`name: text = "full_name"`). Rejected: `=` is a
  default.
- **No type on the field, inferred from the column.** Rejected: ops would
  type-check against a catalogue the compiler never reads (compiling never
  looks at a database).
- **Refs from Ordo entities to external ones**, unenforced by a foreign
  key. Deferred: the write-time lookup it needs is a new mechanism; an `id`
  field and a second read serve the first cases.

### Interactions

- **With `public` (§1)**: always legal on an external field; nothing there
  is sealed.
- **With `$tenant` (§3)**: the guard above is the plan's "which tenants may
  see it": a setting on the entity, written as a guard over the tenant's
  facts.
- **With `prev` (§5)**: legal, so a regenerated translation can say which
  Ordo field a renamed column became.
- **With orphans (§6)**: never orphaned or dropped by Ordo; stops being
  served when undeclared.
- **With settings (§7)**: `external.checkEvery`.
- **With API keys (§4)**: a key may list an op over an external entity.

---

## 9 · Interface details: sort order and live pacing

### 9a · A read states its sort order (27o)

**What it is.** The interface descriptor states each read's order, so the
device copy returns rows in it. Ruled 2026-10-01 (`react-live-device`),
*"as long as the language allows it"*: a read whose order the language
cannot state keeps id order and says so.

**No new syntax.** `sort` already states the order in the body; the typed id
is always the final key (DL, Determinism). The addition is in the
interface: per read op,

```json
"order": { "keys": [{ "path": "placed", "desc": true }, { "path": "id", "desc": false }],
           "stated": true }
```

- `stated: false` when the op ends in `group` or `reduce` with no `sort`
  (bucket order is the planner's), or when a sort key is an expression the
  device cannot evaluate (`cond`, `lower`, a sealed field). Then `keys` is
  `[{ "path": "id" }]`.
- `cursor` (`stable` / `drifting`) rides beside it, as today.

Compiler check: none new. The CDK already has `.sort()`; the SDK reads
`order` and sorts the device copy with the same codepoint rule the server
uses (`COLLATE "C"`).

### 9b · Live refetch pacing is a setting (27n)

**What it is.** Group bursts and wait at least a quarter second between
refetches, as a setting. Ruled 2026-10-01.

```
settings {
  live.refetchPace = 250ms   person >= 100ms
}
```

The deployment default is Ordo's 250ms; the program may change it; a person
may lengthen it on their own devices when exposed. The individual act is
the hook's option (`useLive(op, { pace })`), checked against the effective
value by the client: a hook may slow down, never speed up past the floor.
The React client reads the value from the interface, where the resolved
settings for the caller ride with the descriptor.

Interaction with §7: this is the smallest complete example of the layering
rule, and the one to build first.

---

## 10 · Summary table

| Addition | Keyword or form | Attaches to | Compiler check (main class) | CDK form |
|---|---|---|---|---|
| Public field | `public` | field tail | `public_sealed`, `public_tenant_entity`, `public_guarded` | `t.text().public()` |
| Public entity, cascading | `public`, `public*` | entity line | as above, per field | `.public()`, `.publicAll()` |
| Public op | `public` | op flag | `public_reads_private`, `public_caller`, `public_composes_private` | `.public()` |
| Public write | `public` on a write op | op flag | `public_write_mode`, `public_unbounded_text` | `.public()` |
| Form grant | none | runtime: `std.deployment.formLink` | door: write ops, create mode | none |
| Deployment entity | `deployment` | entity line | `cross_space_join`, `deployment_ref_from_tenant` | `.deployment()` |
| Caller's tenant | `$tenant` | reserved parameter | `tenant_in_public`, G15 extension | `p.tenant`, `g.TENANT` |
| Caller-side guard atoms | `$caller.<path>`, `$tenant.<path>` | guard walk (G1a) | `invalid_guard_walk`, `guard_on_sealed` | `g.callerIs`, `g.tenantIs`, `g.tenantField` |
| API key profile | `apikey name { ops …; quota.* }` | top-level block | `apikey_platform_op`, `apikey_unknown_op`, `apikey_step_up_op` | `apikey(name, { ops, quota })` |
| Rename, entity | `(prev: old)` | entity header paren | `prev_still_declared`, plan `prev_unresolved` | `{ prev: 'old' }` |
| Rename, field | `prev(old)`, `prev(entity.old)` | field tail | `prev_self`, `prev_duplicate`, plan `prev_type` | `.prev('old')` |
| Orphan | omission | the plan reports | plan `orphan_name_reused` | none |
| Prune | `ordo plan --prune` | the plan and receipt | plan `prune_unknown`, two confirmations | none |
| Settings | `settings { name = value [tenant [bounds]] [person [bounds]] }` | top-level block | `unknown_setting`, `setting_not_exposable`, `setting_out_of_bounds` | `settings(s.a.b(v).tenant({ max }))` |
| Rate value | `<int> per <duration>` | setting value | `setting_rate_only` | `rate(n, dur)` |
| External entity | `external name (key: k) from "s"."t" { f: type <- "col" }` | top-level block | `external_readonly`, `external_modifier`, `external_unmapped`, `external_no_id` | `external(name, [s, t], fields)`, `.from(col)` |
| Sort order stated | none | interface `order` | none | none |
| Refetch pacing | `live.refetchPace` | settings line | as settings | `s.live.refetchPace(dur)` |

### New reserved words

Six reserved words, two contextual words, one token, one reserved
parameter:

| Kind | Words |
|---|---|
| Reserved (head a clause or a declaration) | `public`, `deployment`, `prev`, `settings`, `external`, `apikey` |
| Contextual (one position each, never reserved) | `from` (after an external entity's name), `per` (after an int in a setting value); `tenant` and `person` (after a setting value) are also contextual |
| Token | `<-` |
| Reserved parameter | `$tenant` |

`prev` is reserved rather than contextual because it appears in two
positions (header paren, field tail); `key` stays contextual because it has
one.

**The keyword table that must change, and the gates behind it:**

1. `crates/ordo-syntax/src/token.rs`, the `Keyword` enum and its `parse`
   table (lines 73 to 142). This is the lexer's table;
   `crates/ordo-syntax/src/lexer.rs` only calls `Keyword::parse`.
2. `crates/ordo-gen/src/spell.rs` matches `Keyword` exhaustively with no
   wildcard; `cargo build` fails until each new word has a spelling or a
   stated reason for none. `crates/ordo-gen/tests/ratchet.rs` guards that.
3. `clients/ordo-cdk/src/generated/vocabulary.ts` is regenerated by `ordo
   gen ts`; `test/vocabulary.test.ts` asserts every offered name resolves.
4. The lexer test `fully_reserved_words_lex_as_keywords`
   (`crates/ordo-syntax/src/lexer.rs:759`) and a conformance reject case
   per word (`entity x { public: int }` must fail).
5. `docs/definition-language.md` §2, the reserved-word list.

**A finding for the lead.** The list in `definition-language.md` §2 and the
`Keyword` enum disagree today. The document lists `read write open route
std`, which the enum does not reserve (`read` lexes as an identifier, by the
lexer's own test at line 881). The enum reserves `space auth compliance
cond`, which the document's list omits. The document says the list is
"mechanically regenerated from the grammar"; nothing regenerates it. One of
the two should be made the source and the other derived, in the same change
that adds these words.

### Draft board asks, in order

Each ask is one question of at most 160 characters. Each suggestion starts
with the answer in bold, then one sentence of why, at most 260 characters.

1. **ask:** Should `prev` be written in the .ordo source, with the plan's receipt repeating it, rather than chosen on the plan's command line?
   **suggest:** **Source.** A rename is a fact about the program; the reader and the agent meet it beside the new name, and the receipt still records the step for reversibility.

2. **ask:** Should `public` be a reserved word, so no field or entity may be named `public`?
   **suggest:** **Yes.** It heads an entity line and an op flag, and a word that is contextual in 3 positions is the kind of rule an agent gets wrong; `isPublic` serves the field.

3. **ask:** Should an entity whose rows live only in the deployment's space say so with a `deployment` line, required before any `public` mark?
   **suggest:** **Yes.** The compiler then knows which reads cross the tenant boundary and refuses a tenant-and-deployment join, which two databases cannot serve.

4. **ask:** Should `public` be refused on a field of an entity that has a read guard or an `acl`?
   **suggest:** **Yes.** The two lines disagree about who reads the row; public rows go in an unguarded entity, or the public op selects them in its body.

5. **ask:** May a guard test the caller's own row and tenant (`$caller.role`, `$tenant.plan`), compiled to one constant per call?
   **suggest:** **Yes.** It is the only way to write "every pro tenant reads this" without a link row per tenant per row, and it costs no join.

6. **ask:** Should one `settings { }` block replace the unparsed `policy` blocks as the single spelling for settings?
   **suggest:** **Yes.** One form carries every layered value and its exposure; `policy` never parsed, so nothing built changes.

7. **ask:** Should setting names come only from Ordo's closed catalogue this round, with app-defined settings deferred?
   **suggest:** **Yes, for now.** An app value readable in op bodies is a different feature with its own determinism rules, and 18ac is about Ordo's settings.

8. **ask:** Should API keys be issued at runtime against an `apikey` profile the program declares, naming the ops a key may ever reach and the quota default?
   **suggest:** **Yes.** The program sets the ceiling and the quota, a tenant issues keys inside it, and no grant ever lives in a file.

9. **ask:** Should a tenant owner or manager be able to issue API keys for the tenant's own customers by default, through `std.space.issueKey`?
   **suggest:** **Yes.** The app's customer is usually a tenant, and the profile already bounds what a key reaches; the deployment owner keeps `std.deployment.issueKey`.

10. **ask:** Should a `public` write op be limited to create mode, with no addressing, converge or delete?
    **suggest:** **Yes.** An anonymous converge-or-create could overwrite a row by a guessed key; a form only ever makes a new row.

11. **ask:** Should dropping an orphaned field or entity be a plan choice (`ordo plan --drop`) and never a line in the source?
    **suggest:** **Yes.** A drop is an act on one deployment's data with an actor and a confirmation, and can differ between dev and production.

12. **ask:** Is `<-` the right mark for a column mapping in an external entity, as the mirror of `->`?
    **suggest:** **Yes.** It says the data comes in, and a stray `<-` in an expression fails to parse instead of silently meaning less-than-minus.

13. **ask:** Should per-op settings such as page size wait for 18ac's build, spelled then as `op x settings(door.pageSize = 100) =`?
    **suggest:** **Later.** The block covers the app layer now, and the per-op form is best decided with the resolver that reads it.

14. **ask:** Should external entities be read at the same three levels as the deployment's space (public, signed in, narrowed by a guard)?
    **suggest:** **Yes.** They have no `$space` column, so they are the deployment's by construction, and the plan's "which tenants may see it" is then a guard.

15. **ask:** Should the reserved-word list in definition-language.md §2 be generated from the `Keyword` enum, ending today's disagreement between them?
    **suggest:** **Yes.** The document claims it is regenerated and nothing does it; the enum is what the lexer reads, so it is the source.
