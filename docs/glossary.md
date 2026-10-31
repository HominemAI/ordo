# Ordo Glossary

The concept map. Each entry defines one term, links the document section that
owns it, and says when you can safely ignore it. Read this before the spec;
come back to it whenever a doc uses a word as if you already knew it.

---

## space

A space is **one world of data, membership, keys, and history**. It is the unit
of tenancy and of custody. Everything that happens in Ordo happens inside some
space. A space has its own membership (who belongs, with what role), its own
access profile (which ops each role may call), its own encryption key (the
space DEK that seals sensitive fields), and its own append-only, hash-chained
command log. It can optionally have its own database and its own blob bucket
(BYO-DB / BYO-bucket), and it sits somewhere on the custody dial, from
platform-managed keys to client-held, end-to-end keys. Spaces never merge.
Sharing between spaces goes through exactly two doors, a *window* (guest
access) or a *feed* (consented copy). See **feed vs window** below. Deleting a
space is crypto-erasure (the DEK is destroyed). Exporting it is total
disclosure. Both are owner-only, step-up-gated ops. **One space or many?**
Data that shares members, keys, and history belongs in one space. Genuinely
separate worlds (your household and your business) are separate spaces, bridged
by feeds or windows if needed. Multiple *apps* over one space are fine. They
are windows onto one world, with the same membership, profiles, and chain.
Owned by: `definition-language.md` §8 (Access model, Inter-space sharing, Exit
ops) and `implementation.md` (custody sections). You cannot ignore this term.
It is the frame every other term sits inside.

## tenant

A **tenant is a space that belongs to someone**, identified by a **minted,
globally unique id**. **The id carries the uniqueness. A name exists
alongside it for human reference** and carries nothing (ruled
2026-09-21). In the tree since 17aa the id is `spc_` followed by 26
Crockford base32 characters (128 bits), minted by `std.space.create`, and
an identifier `well_formed_space` already binds. The id is public. It
travels in request headers, metric labels and refusal messages, so it is
never a secret and authority never comes from knowing one. The name is what
a person reads, and the id is what everything keys on. The word names the
relationship, and the mechanism is always a space. Use
"space" when you mean the unit of data, keys and history. Use "tenant"
when you mean the customer, team or household whose space it is. Every
tenant is a space. Not every space is a tenant, because a deployment has
one space of its own that belongs to nobody but the platform (see **system
space**). Owned by: nothing of its own. If a sentence
would still be true with "space" in place of "tenant", write "space".

---

## deployment

A deployment is **one database, one program, one master key and one space
of its own**, served by **one or more replicas**. It is a logical thing,
not a process. Replicas are how it runs, and it is normal to have several,
which is why the supervisor elects a leader and why usage is flushed per
replica. Scaling out adds replicas and does not add deployments.

A deployment serves many spaces, and every space it serves is inside it.
It is the unit of operation and of blast radius: what you deploy a program
to, what you restore, what holds one custody perimeter, and what a
deployment owner's credentials reach.

**Not a Kubernetes Deployment.** The word collides, and the two are
different objects. A Kubernetes Deployment is one way to run an Ordo
deployment's replicas. Where both appear in a sentence, write "Ordo
deployment" and "Kubernetes Deployment", or rephrase.

What is per deployment and not per space: the compiled program and its
activation pointer, the master key that wraps every space's keys, identity
(a login belongs to the deployment, not to a space), the access engine,
and the deployment's own security-event chain. The worker supervisor runs
on every replica and its leader-only workers run on exactly one, which is
what makes several replicas safe.

There is **no deployment identifier in the engine**, and that is deliberate
while one process serves one deployment. It is defined by its process, its
database, its `ORDO_SPACE` and its master key. Naming deployments is the
platform service's job. Owned by: `deploying.md`,
`plan-runtime.md`. You cannot ignore this term if you operate Ordo.

---

## deployment owner

The **person who runs a deployment**: they hold its keys, apply programs to
it, and handle recovery and succession. They are not a customer of the app
the deployment serves.

**Today** a deployment owner is an ordinary principal made owner of the
deployment's own space at boot, by naming their principal id in
`ORDO_DEPLOYMENT_OWNERS`. They sign in the same way the app's users do. See
`deploying.md` §4c and the variable's row in §1.

**Ruled 2026-10-01** (board `deployment-owner-identity`): the deployment
owner signs in **separately, through the backend**. *"The console is simply
a tool to interact with the deployment, but a different credentialing
system."* Until that is built, today's model stands.

Not the same as the **app developer** (who writes the program) or a
**tenant owner** (who owns one space inside the app). You can ignore this
term unless you run a deployment.

## system space

The **space a deployment keeps for itself**. It is created at boot, named by a
constant compiled into the binary (not by configuration), and never a
tenant. It is where a deployment keeps what it knows about itself. In the
code and in the 2026-09-21 ruling it is also called the deployment's
**control plane**, in this sense: it boots on its own custody and
its own tables, and no tenant's state can stop it. A tenant whose chain was
rewound, whose custody is missing or whose program will not load is
quarantined at its own boundary and says so; the system space and every
other tenant keep serving. Dependence runs one way, from a tenant to the
system space.

It is separate from the platform service. It lives inside one deployment and
knows nothing about any other. Owned by: `corpus-rulings.md` (2026-09-21),
checklist 17v and 17aa.

---

## console (the Ordo Console)

The **surface above a deployment**, and above several of them when
someone is hosting. It registers deployments, mints their global names and
their spaces' names, provisions them (generating a master key and handing it
over once), gives each the control database its restore witness writes to, and
bills from the usage each one reports.

Three properties define it, and each is a line that must not be crossed:

- **One deployment, one separately managed master key.** The platform
  service never holds a deployment's key after provisioning, and two
  deployments never share one. That keeps one process equal to one custody
  perimeter.
- **A deployment has no concept of it.** It is configured and told where to
  live, the way a container is. It does not discover the platform service,
  does not call home, and cannot reach another deployment. Dependence runs
  one way and stays configuration.
- **It never reaches into a deployment's spaces.** It knows that a
  deployment exists, where it lives and what it used; it does not read its
  data.

**One console, two modes.** Bundled with a deployment it manages that one
deployment, which is what a self-hoster gets. Run by a hosting provider it
manages many, which is what a hosted service needs. The surface is the same
in both modes, and in both it reaches a deployment **through the door**, as an
owner with a token, and never into its database. A console that reached into
the database would be a privileged back door around the rule that the system
is the source of truth.

It is a different thing from the system space and from a bigger deployment.
Owned by: `corpus-rulings.md` (2026-09-21), checklist 18n and 18o. If you
run one Ordo for yourself, you still get the console. You never see its
second mode.

---

## control plane (the word, and its two meanings)

Used in this corpus for **two different things** at different scopes:

1. **A deployment's control plane** is its **system space**: the part of a
   deployment that runs without the tenants under it. This is what the
   2026-09-21 ruling means, and what the code means.
2. The surface **above** deployments is the **console**, and the software
   that runs deployments is the **operator** (in the Kubernetes sense:
   software that creates and configures them).

**Ruled 2026-09-21:** "control plane" belongs to meaning 1, the
deployment's own system space. Say **console** or **operator** for what is
above. Those three words are unambiguous.

---

## principal

Anything whose actions must be attributable and permissible. It is the unit
that identity, grants, and command-log attribution hang off. There are exactly
six kinds: **human** (users), **routine** (a domain's automated rules),
**worker** (platform pods, which are infrastructure), **agent** (an assistant or LLM client holding
a token), **source** (write-only outsiders: webhooks, sensors, camera hubs),
and **client** (an authenticating app shell, e.g. a PWA). Only
externally-originating kinds (agent, source, client) are declared in IaC;
everything platform-born is implicit. Every command in the log names its
principal (`user:<id>`, `routine:<name>`, `agent:<name> on behalf of
user:<id>`, `source:<name>`, `system:<component>`). Principals can be
collected into **groups** and granted as a set (see **group**), and the routine
kind has its own entry because it behaves differently from the rest (see
**routine principal**). Owned by: `definition-language.md` §8 (Principals) and
`security-logging-design.md`. Ignorable until you add automation or outside
callers to a first app.

## domain

A versioned, server-side package of definitions: shapes (entities), ops, and
routines, as in `domain finance @ 2.1.0 { shapes …, ops …, routines … }`.
Domains are code, deployed via `ordo plan` / `ordo apply`. They contain no
spaces, no grants and no data, because consent is runtime state and never ships
in a package. A space *enables* a domain, which instantiates its profile
template and applies its schema to that space's database. Domain ↔ app ↔ space
is deliberately many-to-many. Owned by: `definition-language.md` §8 (Domain package) and
`domain-packages.md`.

## app

A named UI surface manifest sitting *above* domains: it declares pages and
which ops render on them (`app portfolio { page dashboard { table projectCosts
live } … }`). Apps are plural per deployment, compile-checked against the
capability catalog (an unknown op or wrong param type is a build failure), and
may reference ops from any enabled domain. Several apps can share one identity
client, so one sign-in makes every app live. Apps targeting the same space are
windows onto one world. Owned by: `ui-gen.md` §5–6 and `definition-language.md`
§8 (Identity clients). Ignorable until the generated-UI phase matters to you.

## client

An OIDC relying party of the deployment's one issuer. It authenticates and
calls ops, such as a web shell or PWA. A client is not a domain (domains are
server-side cargo) and not an app (a single client can serve several apps). Declared in IaC with redirect URIs, token lifetimes, and
per-op step-up requirements (`stepUp [transfer.approve, space.delete]`). Owned
by: `definition-language.md` §8 (Identity clients) and
`security-logging-design.md` §4–5. Ignorable until you wire up real login.

## op

The one registration keyword: `op name($params) flags = <body>` wraps *every*
registered operation, read or write. The op's **kind is derived from the
body**: a pipeline makes it a read, and any `set`/`del` makes it a write. It is
never asserted (`query` is prose, not syntax, and the parser rejects it with a
teaching error). At deploy time each op compiles once into a named, versioned artifact:
planned SQL, mode label (`creates` / `converges` / …), cost class, and a
field-scope manifest derived from its body. Registered read ops are also
composition sources, callable by name inside other queries. There is no
free-form query text at runtime; ops are the entire callable surface. Owned by:
`definition-language.md` §13 (registration wrapper ruling), §5 (queries), §6
(writes). Core vocabulary, not ignorable.

## grant

Runtime consent that some principal may call some ops in some space. Grants are
**never IaC**. A grant declared in a config file would be consent granted by a
file, which the platform exists to prevent. Human access flows role → profile.
Synthetic callers get capability tokens (below). All permission changes are
commands, so permission history is auditable, and "who can call X" is one
exhaustive query (`std.space.callersOf`). **"Grant" is a concept and not a
statement:** there is no `grant` keyword in the definition language. Row-level
access is granted by writing the derived link entity,
`set <entity>Access { … }`, and by the `acl`-derived `grant…`/`revoke…` **ops**
gated at the highest level. Owned by: `definition-language.md` §8 (Access model)
and §9 (the IaC boundary).

## capability token

The credential a synthetic caller (agent, source, API client) presents.
Synthetic callers never inherit role defaults: the token carries an explicit
**op-name allowlist** enumerated by a human at consent time, a pinned **scope
hash** of exactly those ops' manifests, and a **mandatory expiry**. If any
listed op's manifest expands, the hash mismatches and the grant is dead until
re-consent; narrowing applies silently, audit-noted. Capability tokens can
never satisfy step-up, so step-up ops are structurally human-only. Owned by:
`security-logging-design.md` §5 and `definition-language.md` §8 (Access model,
layer 3). Ignorable until you connect an agent or integration.

## profile / role / override

The human access path, per space. **Roles** are fixed at four: owner / manager
/ editor / viewer. They are membership state and user-managed. A **profile** is
the space's role→ops mapping. It is runtime data, seeded from the domain's
template when the space enables the domain, then owned by the space (managers
tighten or loosen, and every change is a command). **Per-member overrides** add
or remove ops for a named member on top of their role. The roles stay fixed, and
overrides carry the long tail of "this person specifically." **Roles have no
implicit hierarchy:** a profile lists ops per role explicitly. Owner does not
silently inherit manager's ops, and no group inherits another group's grants.
Read that pin precisely, because it concerns *profiles*. The platform still
attaches a handful of **space-level capabilities** to the owner role
(export, space deletion, BYO custody changes, and administration of any `acl`
row in the space, the last checked at the door). Those confer no op and no rank
over another role. They are one capability each, scoped to one space. Owned by:
`definition-language.md` §8 (Access model) and `security-logging-design.md` §6.
How to set it: `howto-roles.md`.

## members / `$caller`

The two halves of "who is asking", and the hinge of the whole authorization
model. **`members`** is a **platform-provided** entity that you never declare.
It holds one row per principal that belongs to the space. The platform creates
the row when a principal joins and retires it when they leave. It is what
`-> members` refs in your shapes (and in every `acl`-derived link entity) point
at. Its fields are read-only to domain code and include `principal`, `kind`
(`principal` or `group`), `role`, `status`, `joined`, and a **display name**
(`displayName`). The display name was added last, because every generated list,
assignee column and audit view needs a human-readable name, and a domain
inventing its own profile table to supply one would create a second identity
source with its own bootstrap problem. Project `assignee.displayName`, not
`assignee.name`.
**`$caller`** is the one reserved parameter: the calling principal, bound by
the runtime from the session and **never suppliable by a client** (a request
carrying it is rejected). By behavior, `$caller` binds to the caller's
own `members` row **in the space the request is bound to**, so `member ==
$caller` inside a guard or an op body is the comparison you'd expect, with no
principal-id ↔ member-id reconciliation to write. A principal with no `members`
row in a space matches nothing, so every `$caller`-anchored guard evaluates
false for them, which is correct for strangers. **`== $caller` never expands
groups**: it is row equality, so a row assigned to a group you belong to does
*not* match, and group resolution is the access layer's job alone (see
**group**). Owned by:
`definition-language.md` §2 (`$caller`) and §3 (`members`, `acl`). Core
vocabulary the moment a space has more than one person in it.

## group

A **named collection of principals**, usable anywhere a principal is: named in an
`acl` row, reachable on the far side of a guard's relationship walk, named in a
grant. Group membership is ordinary runtime state changed by a command, so
"everyone on the design team can write in this project" is one access row plus a
membership list, with no N access rows to re-edit whenever the team changes.
Removing someone from the group removes their access everywhere the group was
granted. Mechanically a group *is* a `members` row with `kind: group`, which is
why it is nameable everywhere a principal is, with no new type and no new
grammar. Five boundaries, and the third is the one that surprises people:

- a group is a set of principals and **not** a role hierarchy. The four space
  roles (owner / manager / editor / viewer) stay fixed and separate, and no
  group inherits another group's grants;
- a group belongs to exactly one space;
- **groups expand in the access layer and nowhere else.** Guards, acl rows and
  grants resolve group membership when deciding access. Query and write bodies
  do not: `== $caller` is row equality and never expands a group, and `count`
  over an acl link entity counts a group as **one** row. To count humans, sum
  the read-only `memberCount` on the `members` row (`1` for a person, the
  group's size for a group). That is a cheap upper bound, since someone
  reachable both directly and through a group counts once per path. Exact
  headcount is a `distinct` pipeline over `groupMembers`;
- **groups cannot contain groups** in v1. Nesting is rejected at write time
  (`group_nesting`), which keeps a single expansion sufficient everywhere;
- **holding a level both directly and through a group is normal, and the more
  permissive one wins.** A caller's effective level on a row is the **maximum**
  of every path that applies to them: their own acl row and every acl row
  naming a group they belong to. There is no "direct beats group" and no
  subtraction. A group grant can raise someone above their direct row, which is
  the point of granting to a group, and adding somebody to a group can never
  reduce what they already had. Because access can arrive by several paths,
  revoking one path does not revoke access. A revoke reports the paths that
  **remain**, and removing someone's group access means removing them from the
  group. (Ruled: `corpus-rulings.md` 2026-09-05, Y-013; normative statement in
  `definition-language.md` §acl.)

Rule of thumb: if the sentence you are modeling contains a plural noun ("the
reviewers", "contractors"), grant to a group. If it names a person, grant to the
person. Owned by: `definition-language.md` §8 (Access model, Groups); modeling
advice in `modeling-guidance.md` §D. Ignorable until more than one person needs
the same permission.

## guard / acl

Row-level, data-driven authorization declared in the shape. An `access read` /
`access write` clause is a `$caller`-anchored relationship walk, an existence
test over declared refs (`access read project.projectMembers(member ==
$caller)`), that the compiler **inlines into every op touching the entity**.
Read guards filter, and write guards reject loudly (`access_guard`). The guard
grammar is closed (ref walks, `==` against `$caller` or a literal, and bare bools,
with no arbitrary predicates), so guards always compile to an indexed semi-join.
Bypass is explicit and greppable (`unguarded`), and an op composing an
`unguarded` op must carry the marker itself. Four rules worth knowing before you
write one:

- **Both verbs, always.** `access read` with no `access write` is a **compile
  error**. Write-open is a legitimate design and must be spelled
  **`access write open`**. The keyword is `open`, and `access write true` does
  not parse, because a guard position that accepted the boolean literal could not
  distinguish a decision to open from an expression that happens to be true.
  Write-open can never be reached by omission. This exact omission produced a
  real generated app that "looks secure and isn't".
- **An explicit clause replaces the inherited guard entirely.** There is no
  composition algebra. Restate the whole rule on the child.
- **Link entities never inherit** (two owners, no single root) and always declare
  their own guards.
- **Routines are bounded by their trigger and not by guards.** See **trigger
  provenance**.

`acl (read, write, admin)` is the derived one-liner: it generates a real link
entity, guards over the entity and its owned subtree, admin grant/revoke ops
gated by the highest level, and levels ordered left-to-right. Its **bootstrap**
is platform-derived and not written by hand, and has three parts: a **creating write on an
acl-root entity is exempt from the derived write guard** (the guard walks to an
access row that cannot exist yet, so without the exemption nothing is ever
creatable); the creating principal (a space **member**, never a guest)
receives the top-level access row **in the same command group** as the row
itself, while system/routine-created rows receive none; and a **space owner may
always administer any acl row in their space**, a check made **at the door**
where the space is already bound (the universal escape when everyone with admin
has left). There is no `grant` statement: the derived link entity is a real
entity, so seeding a second person is `set <entity>Access { … }` in the same op.
Owned by: `definition-language.md` §3 (Access guards, `acl`); worked end-to-end
in `tutorial.md` Part 2; test corpus in `guards-conformance.md`. Ignorable for a
single-user first app; essential the moment a space has members with different
powers.

## sealed / blind index

`sealed` on a field means it lives in the per-space encrypted payload.
Postgres holds ciphertext, and the field is not filterable or sortable in SQL
(predicates run post-decrypt in the host, and the compiler prices that in the
op's cost class). `sealed indexed` adds a **blind index**: an HMAC of the value
in an indexed column, giving **equality lookups only** at SQL speed with keys
never entering Postgres. Ranges, `contains`, and sorting on sealed fields stay
host-side, and there are no order-revealing schemes, ever. The skeleton (ids, refs, dates)
stays plaintext for indexing. Owned by: `definition-language.md` §3 (Sealed
fields) and §5 (Cost classes); see also the sealed entry in `traps.md`. **You
can ignore sealing entirely for a first app.** Leave fields open and come back
when you have data worth hiding from your own database.

## `decimal` vs `money` (and why division changes type)

Two numeric ideas that are deliberately different. **`money`** is a
storage type: exact decimal in major units (`10.50`), never float, and closed
under `+`, `-`, and `*`. Those stay exact and stay `money`. **`decimal`** is
what **division and `avg` produce**: an exact value at a **fixed fractional
scale of 12**, the same scale host-side and in SQL, so an expression's truth
value never depends on which venue the stage ran in. The consequence you will
meet first: **nothing rounds implicitly.** A `decimal` landing in a typed slot
(a `money`/`int` projection, or a write RHS for a `money`/`int` field) is a
compile error, class `implicit_rounding`, until an explicit `round(x, n)`,
`floor(x)` or `ceil(x)` converts it. The **mode** is pinned:
`round` is **half-up** (away from zero), globally and unconfigurably, matching
what division already does when it reduces scale. That gives one tie-breaking
behaviour across the whole numeric surface. The earlier rule here said the
author must state the mode, but the closed function list never had a slot to
state it in, so that was an obligation with no syntax (M4-002). An application
whose jurisdiction requires half-even expresses it from the primitives, in its
own code, where a reviewer and an auditor can find it. Comparisons are not
typed slots. `value > total / 3` widens both sides and compares exactly at the
pinned scale, and nothing is stored, so nothing needs a mode. `/ 0` is a typed
runtime error, uniform with `mod(a, 0)`, and there is no null-on-divide-by-zero
convention. Two habits follow. Compare by cross-multiplying and avoid dividing
(`done * 2 < members`, not `done / members < 0.5`). Split money with
**`allocate`** (below) and avoid N independent roundings. Owned by:
`definition-language.md` §2 (Numeric semantics, Division); the trap version is
in `traps.md`. Ignorable only in an app with no arithmetic.

## `allocate`

The blessed money-split primitive: `allocate(total, weights)` splits `total` in
proportion to `weights` so that the parts **sum to `total` exactly**. Each part
is the floor of its exact share, and the remaining minor units are handed out by
largest-remainder with ties broken by weight order (deterministic, so host and
SQL agree). `allocate(100.00, (1, 1, 1))` is 33.34 / 33.33 / 33.33, never 99.99.
It exists because the obvious alternative is wrong in a way that ships: rounding
three shares independently drops a cent, and no amount of care in the caller
reliably prevents it. **Who absorbs the odd minor unit is a business rule, so
the caller names it.** An optional third argument takes one bare name from the
closed set `largestRemainder` (the default) · `first` · `last` · `largestShare`.
Every mode floors the shares and hands the leftover out one apiece, and the mode
chooses only the order of the queue. So the sum guarantee and the "no part more
than one minor unit from its exact share" guarantee hold for all of them. **A
mode may depend only on `(total, weights)`**, which is why *rotation* (evening
the penny out across successive invoices) is not one. It has to remember what it
did last time, and that is application state. **Where it is legal is narrow:**
inside an `each` emission over a **list parameter**, with the weight written as
the element field (`allocate($total, item.weight)` returns *this* element's
part, reading the whole weight vector from the parameter). Anywhere else is
`invalid_allocate`. Splitting by query-derived weights is not expressible at
all, because it is neighbor-math over result rows, declined for the same reason
running totals are.
Owned by: `definition-language.md` §3 (Expression functions), with the split
rationale in §2 (Numeric semantics). Ignorable until you divide money among
people.

## security-event stream

The deployment owner's record of *attempts*, kept **separately from the command
log**. The command log is the tamper-evident answer to "who did what"; by
construction it records state changes, so it cannot answer "who **tried** what."
The security-event stream is where that second question lives: denials with the
attempted op, step-up demands and outcomes, key releases (space, purpose,
correlation), break-glass activation, grant mint/expiry, authenticator
link/unlink, and first-login provisioning. It is separately retained, and it is
the substrate for the mandatory **deny-volume-per-principal-per-space alert**
(every disclosure oracle in the security review is a high-call-count pattern,
and this is the only place one would show). Two things to know before you rely
on it. Whether denials belong in this stream, in the command chain, or in the
ordinary operational log is **still contradicted across documents**
(`critique-consolidated-3.md` Y-008). A *tamper-evident* (chained) version of
the stream is a **proposal** and is not built (`dashboard-plan.md` §2.1).
Owned by: `implementation.md` (Observability) and
`security-logging-design.md` §6. Ignorable while learning; not ignorable before
you operate a deployment.

## cost class / effective cost class

The compiler's honest price tag on every registered op, derived from its worst
stage: **`sql`** (fully pushed down) · **`blind-index`** · **`bounded-host`** ·
**`unbounded-host`**. *Bounded* means **statically provable**: a maximum
candidate count the compiler can derive from identity/unique addressing, a
`take`, a bounded list parameter, or a declared relationship cap. A date
predicate filters but proves no maximum, so sealed aggregates over date ranges
are `unbounded-host`, and `unbounded-host` ops **fail registration** unless the
op carries the explicit `unbounded` marker. The **effective** cost class is the
same idea computed **per request** instead of at registration. A call-time
refinement (a Tier-0 `$refine` condition, especially one touching a sealed field)
can push an op above its registered class, and the binder recomputes
`registered class ⊔ refinement` and rejects the request when the result has no
proven bound. The registered label is what a reviewer reads and the effective
label is what actually runs. A cheap-looking op plus an expensive refinement
would otherwise smuggle an O(space) scan past registration. Owned by:
`definition-language.md` §5 (Cost classes) and `std-library.md` §2.1/§5. Skim it
early. It becomes load-bearing the first time you seal a field you also
aggregate.

## `cursor: drifting`

A disclosure label on a registered read op's artifact. Ordo paginates with keyset
cursors ("the last row I saw, sorted like *this*"), which give no-skip,
no-duplicate paging **so long as the sort key doesn't move**. If a registered
write can change a row's sort-key value mid-pagination, that row can be seen
twice or missed. Where the compiler can prove no write moves the sort key, the
op's cursor is stable and says so; where it cannot, the artifact is labelled
**`cursor: drifting`**. That is not an error. It is a stated weakening of the
guarantee. Sort on `id`, a create-stamped date, or any field your ops never
update, and you keep the strong property. Sort on `status`, `updatedAt`, or a
running total, and you should expect the label and make the consumer idempotent.
**Read the scope of `stable` precisely:** write-stability is judged against
**authored** writes only. The std family's derived full-row converges are
excluded, because `std.<entity>.update` lists every field and including it would
make the label unreachable for anything but id order. So `stable` means "no
authored write moves this key", and granting `std.<entity>.update` re-opens
exactly what the analysis set aside. The artifact records per sort key whether
the std family could move it. Composed post-take pagination is always
`drifting`. If exactly-once paging is a promise you make to somebody else, sort
on something immutable, where the property holds by construction and needs no
proof. Owned by:
`definition-language.md` §5 (Other steps, Determinism); see the pagination entry
in `traps.md`. Ignorable until you paginate over mutable data.

## custody dial (managed → client-first)

Every space sits on a sovereignty spectrum, adjustable **in place**: platform-
managed keys (default) → BYO database/bucket → **client-first** (full E2E: the
client's local store is authoritative, rows and command images are sealed under
client-held keys, and the server holds a store it cannot read). Queries in
client-first spaces run on-device using the *same compiled artifacts*. The
server keeps doing everything structural (schema, ops, membership, the
command-log envelope, sync). Moving the dial is a tracked, logged, resumable
re-encryption job, and the capability loss is **computed**: the consent page
enumerates exactly which operations survive. Owned by: `implementation.md`
(Space custody conversion, Client-first mode). Ignorable until custody is a
requirement. The default managed mode is a complete product.

## DEK epoch / rotation

The versioning of a space's encryption key. A space's data encryption key (DEK)
is what `sealed` fields live under, and a key that can never be **rotated** is a
key that decrypts every backup and every archived command-log bundle forever once
it leaks. Rotation is therefore versioned and not destructive: a per-space
master key wraps a numbered **DEK epoch**, every sealed row and blob carries the
epoch tag it was written under, and rotating mints the next epoch so new writes
seal under it while old rows re-seal lazily through the same resumable
custody-conversion job that moves the custody dial. Two consequences matter.
Rotation is an **op**: owner-triggered or policy-scheduled, command-logged, and
visible in the space's own history and not in a deployment owner's runbook.
Crypto-erasure still destroys one thing, the master key, orphaning every epoch at
once. **Blind-index keys do *not* derive per epoch.** That is a common and
dangerous guess, and the earlier version of this entry taught it. They derive
from the space *master* key, so they change only when it does. The reason: a
per-epoch index key means the same plaintext hashes two ways during a re-index,
so a `sealed indexed unique` field admits duplicates and converge-or-create
silently *creates* instead of converging. DEK rotation is mandatory on every
membership revocation, which would make that window routine. The trade is stated
where the rule lives (`implementation.md`): indexes stop gaining forward secrecy
from DEK rotation, and get it from master-key rotation instead. The epoch tag
has to exist before the first customer row is written, because retrofitting
versioning onto unversioned ciphertext is a migration nobody enjoys.
**Two rotation gaps are open and worth knowing before you schedule one**
(Y-010; C4-015). The first is narrower than the earlier version of this entry
claimed, and applies to **master-key** rotation only. Rebuilding the blind-index
column under a new master key is a scan-and-swap, and the corpus does not yet
say whether writes are frozen for its duration or dual-written from a watermark.
Under the dual-write reading, a value written to a range the scan has already
passed lands in the old digest column and not the new one, so after the swap a
`unique` blind index can admit a duplicate. DEK rotation has no such window,
because the index key does not move with it, as the rule above states.
The second gap: command-log before-images written under an old epoch can outlive
it inside the undo horizon, so a revert may reach for a key generation that
re-sealing has moved past. Neither has a settled answer yet. Owned by:
`implementation.md` (custody sections) and the compliance pack. Ignorable while
you are learning the language, and not ignorable before real data lands.

## command log

The append-only, per-space, hash-chained record of every write. It is the safety
net under the whole write family. Every write compiles to per-row, actor-attributed
commands with before/after images; all commands from one op invocation form a
**command group** committed in a single transaction (triggers and live-query
invalidations fire only on commit). Revert operates on whole groups, applied
through the normal gate as new history, and history is never rewritten. The log
also serves as the event bus for triggers, and per-row history is callable as
`std.E.history`. Sealed field values appear in images only as references, never
plaintext. Owned by: `definition-language.md` §6 (Atomicity, Compilation) and
`security-logging-design.md` §7. Core to the mental model even if you never
touch it directly.

## routine

A domain's automation, in two tiers. **Tier 1 rules** are written in `.ordo`
itself: "query → `each ->` → propose writes," compiled like every op, with
scopes *derived* from the ops the body uses. The language has no I/O, so a rule
cannot misbehave and sandboxing is moot. **Tier 2 services** are heavy
intelligence (LLM categorizers, vision) living *outside* the platform as
external service-principals calling the generated API with capability tokens
and declared egress. The platform hosts no foreign code in v1. Owned by:
`definition-language.md` §8 (Routines, two tiers). Ignorable until your app
needs to act on its own. Routine names are ordinary identifiers and therefore
lowerCamel (`thirstCheck`, never `thirst-check`, because a hyphen lexes as
minus).

## routine principal

The identity a routine runs as: `routine:<name>`. It is platform-born,
attributed in every command it writes, and holds no membership or acl row
anywhere. It is not a caller. Routines are not registered ops and cannot be
invoked from outside, so there is no path by which someone launders their own
visibility through one. Three things bound a routine, and none of them is a
guard walk against a members row it does not have:

1. **Its trigger** is the row bound, and the one that changed this round. See
   **trigger provenance**: clock-woken runs pass guards, and listener-woken runs
   see only rows reachable from `$trigger.ids`.
2. **Its derived scope manifest** is the *field* bound: the entities and fields
   its body names, derived by the compiler and not hand-written, disclosed
   at deploy time, pinned into every grant, and the basis for its key release.
   It never bounded rows, and believing it did is what produced the old blanket
   rule. Keep bodies narrow, because a routine that projects a whole entity has
   granted itself that entity.
3. **Its attribution**: every command it writes says `routine:<name>@<version>`
   in the space's chain, so "who did this" is answerable in the ordinary way.

`run` accepts flags in the same slot ops use (`run($today: date) unguarded
unbounded:`). When auditing "who can see this data", the answer has three parts:
profiles, guards, and the domain's routines. Rows created by a routine get
**no** acl access row. There is no creator to grant to, so seeded data needs an
explicit `set …Access` write afterwards. **Tier-2 services are the opposite
case.** An external categorizer holding a capability token *is* an outsider, gets
an ordinary members row, and guards apply to it in full. The line is drawn at
the trust boundary and ignores whether the work is automated. Owned by: `definition-language.md` §8
(Routines) and §7 (Key release).

## trigger

The wake-up layer, deliberately **blind and logicless**. A trigger sees only
the command envelope (event kind, space, entity, actor) or the clock, and never
field values. Two primitives: listener (`when finance.transactions.ingested ->
run spendWatch debounce 60s`) and clock (`every day 02:00Z -> run sweep`).
Anything that must *see a value* ("amount > 5") is not a trigger. It is the
first line of the routine it wakes, running under a grant with key release
logged. Runs receive an opaque `$trigger` context (affected row ids, no
values) so rules can scope to what changed. For listener-woken runs, that
context is also the authorization bound (see **trigger provenance**). Owned by:
`definition-language.md` §7. Learn it together with **routine**.

## trigger provenance

The rule that decides what a running routine may touch. It turns on **who
chose the moment it ran**:

| Woken by | Bound |
|---|---|
| **clock** (`every day 02:00Z`) | guards pass, because nobody outside chose the moment |
| **listener** (`when …`) | the run may read and write **only rows reachable from `$trigger.ids`** |

The reason is the confused deputy. Listeners fire on `ingested` (source
principals such as webhooks and sensors) and `fed` (copies arriving from another
space). An **outsider** therefore picks both when a guard-free process runs and,
via `$trigger.ids`, which rows it starts from, steering work on rows they could
never touch themselves. The earlier blanket rule ("guards never apply to
routines, because a routine holds no members row") protected something real,
since a clock rule must never silently read zero rows forever. It also bought
that protection with an outsider-steerable master key. Provenance keeps the
first and removes the second. A listener that must legitimately range wider says
so with `unguarded` on its `run` header: one greppable word, a decision someone
made on purpose. Scope manifests are unaffected, because they bound fields and
never rows. Owned by:
`definition-language.md` §8 (Routines); the trap version is in `traps.md`.
Not ignorable the moment a routine is woken by anything a stranger can cause.

## feed vs window (sharing)

The two doors between spaces. Rule of thumb: **window vs copy**. A **window**
(guest access) is an ACL row naming an outside principal. The guest calls the
granting space's guard-bearing ops under their own identity, live and
revocable, with the same rows and the same guards. A
**feed** is a standing, both-ends-consented routine copying a *defined slice*
(a named query's output) one-way into the receiving space. The receiver owns
the copies outright, and because copies land as ordinary writes on the
receiver's chain, the receiver's own triggers fire on them. Feeds are the
blessed app-to-app interface. Revoking a window closes it at the next call.
Revoking a feed stops future copies, and already-copied data stays (stated at
consent). Owned by: `definition-language.md` §8 (Inter-space
sharing). Ignorable until you have more than one space.

## std library

The platform-derived operation baseline: for every deployed entity `E`,
`ordo apply` registers `std.E.get / list / count / whole / create / update /
delete / history` (plus tree and upsert variants) under the reserved `std.`
namespace. It is a pure function of the schema, so every deployment has a
predictable API before any authored op exists. std ops are ordinary registered
artifacts: guards inline into them, they appear in the catalog, and they are
**registry-present but exposure opt-in** (deriving an op and granting it are
different acts). `std.E.update` is deliberately a full-row CAS converge, and
field-subset updates are authored ops by design. For an entity carrying
**sealed** fields, std derives `get`/`list`/`update` over the **open skeleton
only** unless the shape explicitly opts in. Granting std therefore never
silently grants ciphertext-backed fields, and sealed disclosure stays an
authored, greppable act. Owned by: `std-library.md`.
Skim the derivation table early, and ignore the fine print until you ship.

## surface descriptor / app manifest

The two halves of generated UI. A **surface descriptor** is framework-neutral
JSON emitted per registered op describing how its results present: columns
with format hints, row identity, pagination, live capability, applicable
actions. It is derived, shape-verified against the catalog, and never
hand-written component code. The **app manifest** is the small declarative
layer that names apps and pages and binds ops onto them, compile-checked so a
generated or hand-written manifest either builds or fails loudly. Together they
let deploying shapes + ops yield working list/detail/form screens with
zero UI code. Owned by: `ui-gen.md` §1 (descriptors) and §5 (manifest).
Ignorable until the UI phase, because nothing else depends on them.
