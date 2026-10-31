# Access Guards: Conformance Suite

Test specification for Ordo's access-guard machinery (definition-language.md §3
"Access guards" / "`acl`", §2 "`$caller`", §8 "Access model" / "Inter-space
sharing"). Sibling to `composition-conformance.md`: same pipeline, same fixture
domain (`market`, extended below), same case discipline. Four parts: the
caller-parameterized harness dimension, a per-component test matrix, the
fixture/pipeline extension, and a 100-case corpus with hand-computed expected
outputs against a fixed seed.

Case kinds: **exec** (compile + run against seeded Postgres, compare rows) ·
**write** (compile + run write op against a *fresh* seed per invocation; assert
applied state or typed runtime rejection) · **compile** (artifact-metadata
assertions only) · **reject** (expected compile-error class) · **behavioral**
(runtime protocol: `?live`, revocation).

---

## 0. Error classes (inventory extension)

New classes introduced by this suite, extending the composition suite's closed
inventory. As there: the harness asserts the class, never the message text.

| Class | Kind | Raised when |
|---|---|---|
| `access_guard` | runtime | a write fails its guard **on a row the caller may read**: addressed/create write outside the caller's write view but inside their read view; derived admin op called below the top `acl` level. Typed rejection; nothing applied; the whole atomic group aborts. This is the *informative* case: the caller already knows the row exists (R-G10) |
| `not_found` | runtime | an addressed read/write names a row that does not exist, **or** names a **guarded** row outside the caller's read view. The two are byte-identical on the wire, with the same status, code and message (R-G10, `api-engine-design.md` §4.1). On an **unguarded** entity there is no read view to be outside of, so `not_found` means only "absent" |
| `access_denied` | runtime | **door-level only**, and identical for every reason the door refuses: unregistered op name, registered op this principal may not call in this space, missing/malformed/expired token, unsatisfied step-up. Upstream of this suite (R-G13); listed so that no case ever expects it from a guard |
| `unbindable_write` | compile | `let x = set …` on a write whose derived mode is not CREATE (converge, CAS, bulk); see R-G19 |
| `caller_supplied` | runtime | the request envelope carries `$caller` as a client parameter. It is rejected at the door, before planning, and the guard is never evaluated |
| `guest_exposure_unguarded` | compile | a share/exposure registration exposes a non-guard-bearing op (no `access` guard reached, no `$caller` condition) to a guest principal |
| `guard_on_sealed` | compile | a guard predicate references a sealed field, directly or through a walk (ruling R-G1: guards walk the plaintext skeleton only) |
| `caller_in_default` | compile | `$caller` appears in a shape default (the closed default set is literals + `today`/`now`/`newid`) |
| `invalid_guard_walk` | compile | a guard predicate falls outside the closed guard grammar (R-G3): walk not over declared refs, operator other than `==`/bare-bool, function call, or no `$caller` reference in any disjunct |
| `unguarded_superfluous` | compile | `unguarded` on an op whose own body writes no guarded entity source directly (incl. marking an op that only composes guarded callees, because the marker must mean something, R-G8) |
| `unacknowledged_unguarded` | compile | an op composes a callee whose guard bypass is live (`unguarded` callee over guarded entities) without carrying its own `unguarded` marker (R-G8, mirroring the composition suite's R9) |

Reused from the composition suite where guards intersect its machinery:
`parse_error` (guard-clause grammar violations, including the withdrawn
`access write true` spelling), `not_output_field` (guard-internal fields are
not composition outputs, R-G16).

**Where the boundary between `parse_error` and `invalid_guard_walk` falls**
(ruling 2026-08-30, `corpus-rulings.md`). The parser owns the **shape** of an
`access` clause: which of the two forms it is (the reserved word `open`, or a
walk) and whether the walk is a well-formed chain of atoms. Failing that is
`parse_error`. R-G3 owns what a well-formed shape may **mean**, and a violation
is `invalid_guard_walk` wherever it is noticed: in `ordo-schema` once the walk
resolves against the graph, in the parser where the grammar alone can decide
(an operator that is not `==`, an `==` against a `$param` other than
`$caller`). So `… or length(title) > 3` is `invalid_guard_walk` (GC077) and
`… or open` is `parse_error` (GC100): the same rule, applied to a question
about meaning and a question about shape.

**Producer audit (2026-08-29, checklist 9q).** Four of these classes had no
code path that could raise them. Where each stands now:

- `caller_supplied`: **built.** The router refuses `$caller` in the request
  envelope on both transports (JSON body and query string), before binding or
  planning, so no guard is ever evaluated against a client-proposed principal.
  403 on the wire, `param: "$caller"`.
- `caller_in_default`: **built.** It was worse than unraisable: the compiler
  *accepted* GC069's declaration verbatim. `ordo_schema`'s entity lowering now
  refuses `$caller` anywhere inside a shape default (nested in a function
  included), so it fails at `ordo check` with a span.
- `unbindable_write`: **unreachable, and the feature it guards is unbuilt.**
  R-G19 is about `let x = set …`; `LetDecl` binds a *read* `Pipeline` and there
  is no grammar production for a write-valued `let` at all, so the shape the
  class refuses cannot be written down. It becomes raisable when write-valued
  `let` bindings exist.
- `guest_exposure_unguarded`: **raisable since 2026-09-13.**
  `ordo_identity::grants::register_share` is where sharing an op to a guest
  is a thing you can register, and it refuses an op whose pin carries
  `unguarded` before it builds anything. The reasoning that made this class
  worth reserving is the one the refusal now states: a guard-bearing op
  shown to a guest answers only what that guest's own walk reaches, so the
  sharer grants a window, while an `unguarded` op answers everything, so
  the sharer grants the table. There is still no share *declaration* and no
  guest principal kind, and there does not need to be: the share is
  grant-shaped by ruling, and the guest remains an ordinary principal
  holding one grant.

---

## 1. The caller dimension (harness extension)

Guard semantics are a function of *who is asking*. This suite therefore extends
the case format: **cases are caller-parameterized**: one case may invoke the
same compiled op as several principals and assert a different result per
caller. The compiled artifact is caller-independent (one plan, `$caller` is a
binding slot, asserted in GC057); only the binding varies.

### 1.1 The `as:` field

Every exec/write/behavioral invocation names its principal:

```
as: <principal>
```

where `<principal>` is one of:

- a fixture member name, lowercase (`ana`, `bram`, `cleo`, `dov`, `ora`).
  It binds `$caller` to that member's id. `ora` is the **space owner**: she holds
  no membership and no ACL row anywhere, so anything she can reach comes from
  the door-level acl-administration capability rather than from data (R-G13,
  GC091);
- `guest:<name>` (`guest:gil`): a principal external to the space. It holds no
  role and no profile, and participates in guards exactly through the ACL rows
  naming it;
- `anonymous`: no principal at all, so every guard evaluates false (R-G12).

### 1.2 Invocation blocks

A case body is one or more invocation blocks. Each block is: optional local
registration (shown once, above), the op reference, `as:`, `params:` where
any, `→` expected result. Multiple blocks per case are the norm: the *point*
of most cases is that the same op answers differently per caller:

```
openTasks
as: ana            → (audit books, proj_3) · (draft spec, proj_1)
as: dov            → 0 rows
as: guest:gil      → 0 rows
```

### 1.3 Binding rules (pinned)

- The harness binds `$caller` from `as:` **exactly as the runtime binds it from
  a session**: injected after the door, into the reserved binding slot. It is
  never delivered through client params. A case that *wants* to test
  client-supplied `$caller` writes it under `params:` and expects
  `caller_supplied` (GC067–GC068).
- compile/reject cases carry no `as:`, because compilation is caller-independent.
- **write** invocations each run against a freshly recreated seed (§3.5), so
  ordering between invocations of one case carries no state; readbacks are
  stated per invocation.
- The door layer (roles, profiles, capability allowlists) is upstream of
  guards and out of scope for this suite (R-G13): the harness assumes the op
  is exposed to the principal and tests what guards then do. The one
  door-adjacent surface tested is exposure *registration* (guest cases
  GC063–GC065), because the guard-bearing requirement is checked when a share
  is registered. Registration is the host's (`share_ops::validate`), so those
  three cases run in `crates/ordo-host/tests/sharing.rs`, not in this
  crate's harness (re-spelled 2026-09-29, `plan-sharing.md` G8).

---

## 2. Component test matrix

Each row: behavior → expected compiler/runtime behavior → covering corpus
cases. Rows without a case id are unit-only micro-checks.

### (a) Guard declaration: grammar and placement

| Behavior | Expect | Cases |
|---|---|---|
| `access read` / `access write` with `$caller`-anchored walk | accept; compiles to indexed semi-join | GC001, GC080 |
| reverse walk into a collection with inner condition (`project.projectMembers(member == $caller)`) | accept (the spec's canonical form) | GC001, GC002 |
| `or`-combination of disjuncts (`owner == $caller or projectMembers(…)`) | accept; semi-join union, no row duplication | GC003 |
| bare bool field as guard atom | accept inside R-G3 grammar | (unit) |
| guard referencing a sealed field | `guard_on_sealed` (R-G1) | GC074, GC075 |
| guard with no `$caller` in any disjunct | `invalid_guard_walk` (R-G3) | GC076 |
| function call / non-`==` operator in guard | `invalid_guard_walk` | GC077 |
| walk not reachable over declared refs | `invalid_guard_walk` | GC078 |
| `access` verb other than read/write (`access admin`) | `parse_error` | GC079 |
| explicit-open clause `access write open` (R-G21) | accept; entity is **not** guard-bearing | GC099 |
| withdrawn spelling `access write true` | `parse_error`; the boolean literal was never a legal walk (R-G21) | GC100 |
| `entity members { … }` in a domain | `parse_error`; platform-provided, undeclarable | GC088 |
| `$caller` in a shape default | `caller_in_default` | GC069 |

### (b) Read-guard filtering

| Behavior | Expect | Cases |
|---|---|---|
| same op, different rows per `as:` | per-caller filtering | GC001, GC002, GC037, GC038 |
| member with no memberships | **empty result, never an error** | GC001, GC002, GC009, GC037 |
| `or`-disjuncts (owner ∨ member) both admit; doubly-qualified rows appear once | semi-join semantics | GC003 |
| guard on a link entity | filters like any entity | GC004 |
| caller conditions conjoin with (never replace) the guard | narrowed guarded set | GC005 |
| conditions cannot launder the guard (`assignee == $caller` on an invisible row) | row stays hidden | GC006 |
| dot-path through a second guarded entity stacks its read guard (R-G6) | conjunctive filter + both guards in manifest | GC007 |
| group/reduce over the guarded view | buckets from visible rows only; empty view ⇒ no buckets / `count` 0 | GC008, GC009 |
| guard applies before `sort`/`take` in the same body (R-G7) | "top N of what the caller may see" | GC010 |

### (c) Write guards

| Behavior | Expect | Cases |
|---|---|---|
| allowed addressed converge | applied; descriptor returned | GC011 |
| disallowed addressed converge | `access_guard`, nothing applied | GC012 |
| addressed write on a row the caller **may read** but may not write | `access_guard` (R-G10) | GC012, GC013 |
| addressed write on a **guarded** row the caller cannot read | `not_found` (R-G10), **indistinguishable** from an absent id, including in cost | GC013, GC017, GC026, GC029, GC072, GC089 |
| same op, id that never existed | `not_found`, byte-identical to the line above | GC089 |
| `not_found` on an **unguarded** entity | means absent, and only absent (R-G10 exception) | GC090 |
| create naming a parent the caller **cannot read** | `not_found` on the parent (R-G10), byte-identical to a parent that never existed | GC016 |
| create whose parent is readable, failing the would-be row's guard (R-G11) | `access_guard` | GC014, GC015 |
| create on an **acl root**: guard is self-rooted (R-G11a) | guard **not evaluated**; artifact says `create: guard-exempt` | GC091 |
| creator auto-grant lands in the same command group | member ⇒ top level; **guest ⇒ nothing** | GC091, GC092 |
| create-exemption does not reach a parent-rooted guard (R-G11a) | evaluated normally | GC015, GC041, GC091 |
| `let x = set …` on a create, then a dependent guarded write (R-G19) | create exempt, dependent write guarded, one atomic group | GC094 |
| `let x = set …` on a converge / CAS / bulk write | `unbindable_write` (R-G19) | GC095 |
| atomic group aborts if any statement fails its guard | `access_guard`, zero statements applied | GC017, GC018 |
| bulk writes range over the caller's write view (R-G9) | selection ∩ write view; empty ⇒ vacuous no-op | GC019, GC020 |
| distinct read/write guards diverge (reader ≠ writer) | per-guard results | GC012, GC021 |
| guard on membership entity itself (meta-permissions, hand-rolled) | owner-gated | GC022 |

### (d) Inheritance and override (owned subtree)

| Behavior | Expect | Cases |
|---|---|---|
| depth-1/2 read inheritance: `taskNotes` governed by the project ACL through `tasks` (R-G4) | rebased guard `task.project.projectMembers(…)` | GC023 |
| inherited write guard at depth 2 | allowed/denied like the ancestor | GC024, GC025, GC026 |
| manifest shows the rebased guard, attributed `inherited from tasks` | metadata | GC027 |
| child `access` clause REPLACES the inherited guard entirely | parent privilege does not leak down | GC028, GC029 |
| link entities never inherit (two owners, R-G5) | must declare own guards | (fixture: `projectMembers` declares) |

### (e) `unguarded`

| Behavior | Expect | Cases |
|---|---|---|
| bypass works: full rows for any principal | exec | GC030 |
| manifest carries `unguarded: true` + the bypassed guards | metadata | GC031 |
| marker on an op touching no guarded entity | `unguarded_superfluous` | GC032 |
| marker cannot strip a composed callee's guard | `unguarded_superfluous` (bypass is direct-source only, R-G8) | GC033 |
| composing a live-bypass callee needs own marker | `unacknowledged_unguarded` / accept with marker | GC034, GC035 |

### (f) `acl` derivation

| Behavior | Expect | Cases |
|---|---|---|
| derived link entity shape + derived admin ops exist | metadata | GC036 |
| level ordering: read ⊂ write ⊂ admin for read purposes | any level reads | GC037, GC038 |
| write requires level ≥ write (R-G14) | write/admin apply; read denied | GC039, GC040, GC041 |
| admin ops gated by the top level only | non-admin grant/revoke ⇒ `access_guard` | GC042, GC043, GC044, GC045 |
| ACL rows themselves: read at any level, write at admin (R-G14) | roster visible; direct grants blocked | GC046, GC047 |
| inheritance to owned children (`cards`) | per-caller card views | GC038, GC048 |
| grant is converge-or-create on `unique(board, member)` (R-G17) | mode label | GC083 |

### (g) Guards × composition and refinement

| Behavior | Expect | Cases |
|---|---|---|
| composing a guarded query keeps the guard (no laundering) | per-caller results through composition | GC049, GC053, GC056 |
| refinement envelope: guard first, refinement narrows the guarded result | conditions over callee output | GC050 |
| guard applies before the callee's `take` (R-G7: guard is callee-body semantics) | post-take filtering of the *caller-visible* top-N | GC051 |
| composed op's manifest discloses inherited guards | metadata | GC052 |
| guarded composed source under `match any` beside a guarded entity source | both guards compose | GC055 |
| guarded composed source in a joined pipeline | guard rides the source | GC054 |
| `$caller`-independent artifact: one plan, one hash across callers | metadata | GC057 |
| guard internals are not outputs (R-G16) | `not_output_field` | GC058 |

### (h) Guest access

| Behavior | Expect | Cases |
|---|---|---|
| guest passes exactly their ACL row's guard | acl-granted rows only | GC059 |
| no role fallback: everything else is empty | empty results, not errors | GC060 |
| guest write/admin attempts | `access_guard` | GC061, GC062 |
| exposing a non-guard-bearing op to a guest | `guest_exposure_unguarded` at share registration (`std.space.share`) | GC063, GC064 |
| exposing guard-bearing ops to a guest | accepted; guard-bearing basis recorded | GC065 |
| revocation closes the window at the next call | empty after revoke | GC066, GC087 |

### (i) `$caller` lifecycle

| Behavior | Expect | Cases |
|---|---|---|
| client supplies `$caller` in params (read or write) | `caller_supplied` at the door | GC067, GC068 |
| `$caller` in a shape default | `caller_in_default` | GC069 |
| `$caller` in write RHS (R-G2) | legal; binds the session principal | GC070 |
| `$caller` condition + entity guard conjoin | both apply | GC006, GC071 |
| anonymous principal (R-G12) | guards false: reads empty; addressed write on a guarded row `not_found`; creating write `access_guard` | GC072 |
| caller-scoped ops flagged in manifest/catalog | metadata | GC073 |
| delegated token under a live delegation (R-G22) | `$caller` is the delegator: the delegator's rows, the delegator's write answers; the agent is no guard input | GC101 |

### (j) Metadata and cost

| Behavior | Expect | Cases |
|---|---|---|
| guard semi-join classifies `sql`; guard-read fields join the scope manifest | metadata | GC080, GC082 |
| write-guard precondition in manifest; mode labels unchanged by guards | metadata | GC081 |
| derived admin op metadata (mode, gated-by) | metadata | GC083 |

### (k) Behavioral tier

| Behavior | Expect | Cases |
|---|---|---|
| membership-row write invalidates a `?live` guarded query (guard entities are inlined base entities, R-G15) | invalidation + refetch | GC084 |
| no false invalidation from unrelated entities | silence | GC085 |
| ACL grant invalidates acl-guarded live queries | invalidation + refetch | GC086 |
| live revocation empties a guest's subscription | invalidation + empty refetch | GC087 |

### (l) Routines: trigger provenance is the bound (R-G20)

| Behavior | Expect | Cases |
|---|---|---|
| clock-woken routine (`every …`) | guards pass, because nobody outside chose the moment | GC097 |
| listener-woken routine (`when …`) | reads and writes only rows reachable from `$trigger.ids` | GC096 |
| listener-woken routine reaching outside `$trigger.ids` | those rows are invisible to the run | GC096 |
| listener-woken routine marked `run(…) unguarded` | bound lifted, greppable, manifest-flagged | GC098 |
| rows a routine creates get **no** auto-grant (no members row to grant to) | acl root created by a routine has zero access rows | GC096 |
| space-owner acl administration is a **door** capability | owner reaches any acl row; every reach is an attributed command | GC093 |

---

## 3. Testing pipeline

### 3.1 Fixture schema: `market` extension

The composition suite's `market` domain (§2.1 there) is **extended, not
replaced**: all six original entities, their seed rows, and the registered
callee library remain loaded and unguarded (vendors, categories, products,
customers, orders, orderItems; used here as the unguarded contrast surface).
Version bumps to `domain market @ 1.1.0`, adding six entities: a hand-rolled
guard cluster (projects/projectMembers/tasks/taskNotes, the spec's canonical
example, plus one deeper owned level), and an `acl` cluster (boards/cards).

**`members` is not among them and must not be.** It is platform-provided, per
space, and **undeclarable**: the name is reserved in the entity namespace, so
`entity members { … }` is a registration error and no fixture (spec §3,
the section on `members`, the principal entity). The harness *seeds* members rows through
the platform's space-membership surface, and the domain only **refs** them
(`owner -> members`, `assignee -> members?`). Its field list is fixed: `id`,
`principal`, `displayName`, `kind`, `memberCount`, `role`, `status`, `joined`,
all read-only to domain code. Cases that render a member name project
**`displayName`**; there is no `members.name` (GC088).

```
domain market @ 1.1.0    # adds:

entity projects {
  name:  text
  owner -> members
  access read   owner == $caller or projectMembers(member == $caller)
  access write  owner == $caller
}

entity projectMembers {              # link entity: declares its own guards (R-G5)
  project -> projects  owned
  member  -> members   owned
  canWrite: bool = false
  unique (project, member)
  access read   project.projectMembers(member == $caller)
  access write  project.owner == $caller
}

entity tasks {                       # explicit guards REPLACE inherited (R-G4)
  project  -> projects  owned
  assignee -> members?
  title:  text
  status: text = "open"
  access read   project.projectMembers(member == $caller)
  access write  project.projectMembers(member == $caller and canWrite)
}

entity taskNotes {                   # no access clause: inherits tasks' guards,
  task -> tasks  owned               #   rebased: task.project.projectMembers(…)
  body: text
}

entity boards {
  name: text
  acl (read, write, admin)
}

entity cards {                       # inherits the boards ACL (owned subtree)
  board -> boards  owned
  title: text
  done:  bool = false
}
```

`acl` derives (asserted in GC036): a real link entity

```
entity boardsAccess {
  board  -> boards   owned
  member -> members  owned
  level: enum("read", "write", "admin")
  unique (board, member)
}
```

with derived guards (R-G14: read at any level, write at admin) and derived
admin ops `grantBoardsAccess($board, $member, $level)`,
`revokeBoardsAccess($board, $member)`,
`setBoardsAccessLevel($board, $member, $level)`, each gated by the top level.

**Principal mapping (R-G13/R-G18):** `members` rows are the domain
representation of principals; `as: ana` binds `$caller` to `mem_1`, etc. Guest
`gil` has a members row (`mem_g`) so ACL refs can name him; his guest-ness (no
role, no profile, external to the space) is platform state the harness models
at exposure time.

### 3.2 Seed dataset (fixed, exhaustive)

Inserted in suffix order (id order = suffix order, as in the sibling suite).
The composition suite's §2.2 seed loads unchanged first; these rows follow.

**members**: platform rows, seeded through the membership surface, not through
a domain `entity` declaration. `role` matters to the door and to the acl
bootstrap rule (a guest gets no auto-grant, GC092), so it is seeded explicitly.

| id | displayName | role | status | notes |
|---|---|---|---|---|
| mem_1 | Ana | manager | active | |
| mem_2 | Bram | editor | active | |
| mem_3 | Cleo | editor | active | |
| mem_4 | Dov | viewer | active | **no memberships, no ACL rows anywhere** |
| mem_g | Gil | *(null)* | active | guest principal; exactly one ACL row (ba_5) |
| mem_o | Ora | **owner** | active | space owner; **no memberships and no ACL rows**, so every result she gets comes from the door-level acl-administration capability and never from data (GC091) |

No existing case invokes `as: ora`: she holds nothing, so she cannot change any
expectation that was computed from ACL and membership rows. She exists so the
owner override can be asserted where it actually lives, at the door.

**projects**

| id | name | owner |
|---|---|---|
| proj_1 | Alpha | mem_1 |
| proj_2 | Beta | mem_2 |
| proj_3 | Gamma | mem_3 |

**projectMembers** (note: Cleo owns proj_3 but holds **no membership** in it)

| id | project | member | canWrite |
|---|---|---|---|
| pm_1 | proj_1 | mem_1 | true |
| pm_2 | proj_1 | mem_2 | false |
| pm_3 | proj_2 | mem_2 | true |
| pm_4 | proj_2 | mem_3 | true |
| pm_5 | proj_3 | mem_1 | false |

**tasks**

| id | project | assignee | title | status |
|---|---|---|---|---|
| task_1 | proj_1 | mem_1 | draft spec | open |
| task_2 | proj_1 | null | review spec | done |
| task_3 | proj_2 | mem_3 | build site | open |
| task_4 | proj_2 | null | ship site | open |
| task_5 | proj_3 | mem_2 | audit books | open |

(task_5: assigned to Bram, who is *not* a member of proj_3, as the laundering
probe. No proj_3 member has `canWrite`, so task_5 is writable by nobody.)

**taskNotes**

| id | task | body |
|---|---|---|
| note_1 | task_1 | outline ready |
| note_2 | task_3 | site copy wip |
| note_3 | task_5 | numbers pending |

**boards**

| id | name |
|---|---|
| board_1 | roadmap |
| board_2 | icebox |

**boardsAccess**

| id | board | member | level |
|---|---|---|---|
| ba_1 | board_1 | mem_1 | admin |
| ba_2 | board_1 | mem_2 | write |
| ba_3 | board_1 | mem_3 | read |
| ba_4 | board_2 | mem_2 | admin |
| ba_5 | board_1 | mem_g | read |

**cards**

| id | board | title | done |
|---|---|---|---|
| card_1 | board_1 | q3 goals | false |
| card_2 | board_1 | q4 goals | true |
| card_3 | board_2 | someday | false |

### 3.3 Hand-computed visibility matrix

Every expected output below reduces to this table (derived by hand from the
guards + seed; the corpus re-derives per case where subtle).

| view | Ana (mem_1) | Bram (mem_2) | Cleo (mem_3) | Dov (mem_4) | Gil (guest) | anonymous |
|---|---|---|---|---|---|---|
| projects read (owner ∨ member) | proj_1, proj_3 | proj_1, proj_2 | proj_2, proj_3 | none | none | none |
| projects write (owner) | proj_1 | proj_2 | proj_3 | none | none | none |
| tasks read (member) | task_1, task_2, task_5 | task_1–task_4 | task_3, task_4 | none | none | none |
| tasks write (member ∧ canWrite) | task_1, task_2 | task_3, task_4 | task_3, task_4 | none | none | none |
| taskNotes read (inherited) | note_1, note_3 | note_1, note_2 | note_2 | none | none | none |
| taskNotes write (inherited) | note_1 | note_2 | note_2 | none | none | none |
| projectMembers read | pm_1, pm_2, pm_5 | pm_1–pm_4 | pm_3, pm_4 | none | none | none |
| projectMembers write (owner) | proj_1 rows | proj_2 rows | proj_3 rows | none | none | none |
| boards read (any level) | board_1 | board_1, board_2 | board_1 | none | board_1 | none |
| cards read (inherited) | card_1, card_2 | card_1–card_3 | card_1, card_2 | none | card_1, card_2 | none |
| cards write (level ≥ write) | card_1, card_2 | card_1–card_3 | none | none | none | none |
| boards admin | board_1 | board_2 | none | none | none | none |

Cross-checks worth naming: Cleo reads proj_3 (owner disjunct) but none of its
tasks, notes, or roster (tasks/projectMembers guards have no owner disjunct,
which is what the override/replacement demonstrations rely on). task_5/note_3 are readable by Ana
and writable by nobody.

### 3.4 Registered fixture ops (the guarded library)

Registered with the extended schema before any corpus case runs (on top of the
composition suite's §2.3 library, still present). Cases may register
additional local ops; those are shown inline.

```
# Row-unbounded ops carry the acknowledgment the settled registration gate
# requires ("independent of venue"), the same repair as the composition suite's
# §2.3). `firstTask`/`specCount` prove bounds and carry nothing.
op myProjects unbounded      = projects[name, owner]                   sort name
op taskBoard unbounded       = tasks[title, status, project]           sort title
op openTasks unbounded       = tasks(status == "open")[title, project] sort title
op myAssignedTasks unbounded = tasks(assignee == $caller)[title]       sort title
op noteFeed unbounded        = taskNotes[body, task]                   sort body
op teamRoster unbounded      = projectMembers[project, member, canWrite]
op allTasks unbounded unguarded = tasks[title, status]                 sort title
op boardList unbounded       = boards[name]                            sort name
op cardList unbounded        = cards[title, done, board]               sort title
op aclRoster       = boardsAccess[board, member, level]
op firstTask       = tasks[title] sort title take 1
op specTasks unbounded = openTasks(title contains "spec")[title]
op specCount       = specTasks[title] reduce { n: count }
op openCards       = cardList(done == false)[title, board]
op tasksIn($p: id) = taskBoard(project == $p)[title, status]
op vendorNames     = vendors[name]                             sort name

op addTask($p: id, $t: text)        = set tasks { project: $p, title: $t }
op renameTask($id: id, $t: text)    = set tasks { id: $id, title: $t }
op closeTask($id: id)               = set tasks { id: $id, status: "done" }
op closePair($a: id, $b: id)        = set tasks { id: $a, status: "done" }
                                      set tasks { id: $b, status: "done" }
op claimTask($id: id)               = set tasks { id: $id, assignee: $caller }
op purgeDone                        = del tasks(status == "done")
op shipAll                          = set tasks(status == "open") { status: "done" }
op addNote($t: id, $b: text)        = set taskNotes { task: $t, body: $b }
op editNote($id: id, $b: text)      = set taskNotes { id: $id, body: $b }
op renameProject($id: id, $n: text) = set projects { id: $id, name: $n }
op addMember($p: id, $m: id, $w: bool = false)
                                    = set projectMembers { project: $p, member: $m, canWrite: $w }
op addCard($b: id, $t: text)        = set cards { board: $b, title: $t }
op setCardDone($id: id, $d: bool)   = set cards { id: $id, done: $d }
```

Plus the acl-derived admin ops (§3.1): `grantBoardsAccess`,
`revokeBoardsAccess`, `setBoardsAccessLevel`.

### 3.5 Harness stages

Stages 1–5 are the sibling suite's (parse → compile → execute → compare →
assert metadata), with these extensions:

1. **Caller binding.** Execution binds `$caller` per invocation from `as:`
   (§1.3) into the reserved slot, by the same code path the runtime uses for
   sessions. `guest:` principals are flagged guest; `anonymous` binds no
   principal.
2. **Fresh seed per write invocation.** exec cases share one seeded database
   per run (reads mutate nothing); every **write** invocation recreates the
   seed first, so each invocation's readback is computed from the pristine
   seed plus exactly that invocation's effect.
3. **Share-registration stage.** Guest exposure cases (GC063–GC065) run a
   share registration (an op list, by a sharer who reaches each op) through
   the host's registration, `share_ops::validate`, which calls
   `register_share` and adds the reachable-guard rule;
   `guest_exposure_unguarded` is asserted there, in
   `crates/ordo-host/tests/sharing.rs::gc063_gc064_gc065_registration`.
   There is no `share to guest:` line in source: nothing is added to the
   language (ruled 2026-09-08). Invitation and acceptance are platform-tier
   and out of scope.
4. **Metadata.** `meta.json` gains guard fields asserted by compile cases:
   `guards` (per entity touched: clause, read/write, `inherited from`
   attribution), `unguarded` flag with bypassed-guard list, `callerScoped`
   flag, and guard-read fields folded into the scope manifest.
5. **CI tiers** as in the sibling: compile tier on every push (all reject +
   compile cases, no database); execution tier (exec/write/behavioral) in the
   merge queue + nightly. Behavioral cases run against the op runtime.
   Goldens maintained under the same review policy.

---

## 4. Rulings adopted by this suite (R-G1…R-G22)

Points the spec leaves open, pinned here so every expected output is
computable. Each is a fold-into-spec candidate.

- **R-G1** A guard predicate referencing a **sealed field** (anywhere in the
  walk or its inner conditions) is a compile error, class `guard_on_sealed`.
  Guards compile to indexed semi-joins and must walk the plaintext skeleton;
  host-side guard evaluation would make every read's cost depend on its guard,
  and a guard that cannot be pushed down is a guard that silently degrades.
- **R-G2** `$caller` is **legal in write RHS** (`assignee: $caller`,
  `createdBy: $caller`): it binds the session principal like any id-typed
  parameter. (It remains illegal in shape defaults, per R-G below / class
  `caller_in_default`, because defaults are evaluated without a session.)
- **R-G3** The guard predicate grammar is **closed**: walk steps over declared
  refs (forward `ref.` hops, reverse collection steps `entity(…)` where that
  entity declares a ref to the current position), `==` tests against `$caller`
  or literals, bare `bool` fields, combined with `and`/`or` and the
  parens-when-mixing rule. No other operators, no functions, no arithmetic.
  Additionally at least one disjunct must reference `$caller`. A guard that
  never consults the caller is a hidden filter, which belongs in a named
  query, not an `access` clause. Violations: `invalid_guard_walk`.
- **R-G4** Explicit `access` guards **inherit down owned subtrees** exactly as
  `acl` guards do: a child with no `access` clause takes its nearest
  guard-bearing ancestor's clauses, **rebased** through the ownership path
  (`taskNotes` read = `task.project.projectMembers(member == $caller)`). A
  child with its own `access` clause replaces the inherited guard entirely.
  This is the spec's no-composition-algebra rule, generalized to hand-rolled guards.
- **R-G5** **Link entities never inherit** guards: two owners make the
  inheritance source ambiguous, so a link entity either declares its own
  `access` clauses or is unguarded. (The fixture's `projectMembers` declares;
  the acl-derived link gets derived guards, R-G14.)
- **R-G6** Guards attach to the *entity*, wherever it is read: **every entity
  the flattened plan reads contributes its read guard as a conjunctive
  semi-join**, dot-path targets included. A row whose dotted-through target
  fails its guard is filtered (guard filtering is row-removal, never
  null-blanking, because nulls would misreport data absence). All contributing
  guards appear in the manifest per entity. (In this fixture the guard
  implications are coherent: task-readers are always project-readers, so
  the pin is asserted via metadata, GC007.)
- **R-G7** **The guard is part of the entity's body semantics**: it applies
  before the op's own `sort`/`take` ("top N of what the caller may see"), and
  therefore inside a callee before any caller refinement. Composition order
  is: callee guard → callee pipeline (incl. take) → caller conditions.
  Guards survive composition; there is no position that outruns them.
- **R-G8** `unguarded` bypasses guards **only for entity sources written
  directly in the op's own body**. A composed callee's guards are callee-body
  semantics (R-G7) and cannot be stripped from outside. Corollaries, both
  compile-checked: marking an op whose own body touches no guarded entity is
  `unguarded_superfluous` (the flag must be load-bearing and greppable at the
  point of bypass); composing a callee whose bypass is live requires the
  caller to carry its own `unguarded` (`unacknowledged_unguarded`), which
  mirrors the composition suite's R9 for the `unbounded` marker. The
  manifest lists the bypassed guards alongside the flag.
- **R-G9** **Bulk writes range over the caller's write view**: the write guard
  composes into the bulk selection, and an empty selection is the ordinary
  vacuous no-op. The spec's "never a silent skip" governs *addressed*
  writes: a named row failing its guard is a loud `access_guard`. Rationale:
  write conditions are row-scoped (no existence predicates), so without this
  ruling caller-scoped bulk ops would be inexpressible.
- **R-G10** **Non-enumerability: you learn a row exists only if you may reach
  it.** An addressed read or write on a **guarded** row **outside the caller's
  read view** answers `not_found`, **byte-identical** to the answer for a row
  that never existed. An addressed write on a row the caller **may read** but
  may not write answers `access_guard`; that is the informative case, and the
  safe one, because the caller already knows the row is there.
  **Creates are covered by the same rule, through their parent reference**
  (V-010): a create naming a parent outside the caller's read view answers
  `not_found` on that parent, and only a create whose parent *is* readable can
  reach `access_guard` on the would-be row. A create addresses no existing row
  but it names one, and an id in a caller's hand is the whole of the oracle.
  *This ruling replaces the suite's earlier one* ("outside the read view is
  `access_guard`; typed ids are unguessable, so the suite does not require the
  runtime to blur the two"), which is **withdrawn**: it let any caller holding
  an id distinguish *exists but forbidden* from *never existed*, deployment-wide.
  That oracle does not depend on guessing ids at all, only on collecting
  them. Resolved in favour of non-enumerability by `api-engine-design.md` §4.1,
  which names this corpus as the losing side.
  Three consequences the suite asserts rather than assumes:
  - **The exception is unguarded entities.** On an entity with no `access`
    clause and no inherited `acl` there is no read view to be outside of, so
    `not_found` means exactly and only "absent" (GC090). Non-enumerability is a
    property of *guarded* data, and claiming it for unguarded data would be empty.
  - **Timing must not restore the oracle.** The guard semi-join runs whatever
    the outcome, so absent and forbidden are naturally close in cost. That is a
    conformance assertion (GC089), not a hope.
  - **The wire is uniform, and the record differs.** The space's security-event
    stream distinguishes `not_found`-because-guard from
    `not_found`-because-absent. The suite asserts the *response*; the log
    assertion belongs to `security-logging-design.md`.
- **R-G11** Creating writes evaluate the write guard **against the would-be
  row** (post-defaults, pre-insert): `addTask` into proj_2 is checked against
  proj_2's memberships. Rejection aborts before anything is inserted. (Spec
  G10.)
- **R-G11a** **A creating write is exempt from a write guard that can only be
  satisfied by the row it is creating** (spec G10a). The test is static and
  per-entity, decided at registration: a write guard is **create-exempt** iff
  *every* `$caller`-referencing disjunct of its walk begins with a reverse
  collection step rooted at the entity being created. Such a guard is **not
  evaluated** on a creating write; it applies in full from the next touch
  onward. Corollaries the suite asserts:
  - It covers the acl-derived write guard on an **acl root** (`boards`), whose
    walk is `boardsAccess(member == $caller and level == "admin")`, a reverse
    step into the derived link, which cannot exist before the board does
    (GC091). This is what closes the bootstrap deadlock; R-G11 alone only
    *moved* it.
  - It covers a **hand-rolled** `access write` of the same shape. One rule
    serves `acl` and explicit clauses.
  - It does **not** exempt a guard rooted at an already-existing row: `tasks`'
    `project.projectMembers(member == $caller and canWrite)` starts at a
    `project` the create supplies, so it is evaluated normally (GC014–GC016
    stand unchanged). A guard **mixing** self-rooted and parent-rooted
    disjuncts is **not** exempt, because the answerable disjunct is answered.
  - What bounds an exempt create is the **door** plus the op body. The compiled
    artifact says so per entity (`create: guard-exempt (self-rooted guard)`), so
    a reviewer can see which creates are door-gated only (GC091).
  - **The auto-grant rides the same command group** and follows **space
    membership, not authorship**: a space *member* who creates an acl-root row
    receives the top level in the same group; a **guest** (members row, null
    `role`) receives **nothing** (GC092); a routine, system, or source
    principal (anything with no members row) receives nothing (GC096).
- **R-G12** `anonymous` = no principal: **every guard evaluates false** (as a
  defined rule, not via null comparison). Reads return empty.
  Writes: on a **guarded** entity every row is outside an anonymous caller's
  read view, so an addressed write answers **`not_found`** under R-G10, and a
  *creating* write answers `access_guard` (there is no addressed row to be
  coy about, and the guard simply evaluates false). Unguarded ops do answer
  anonymous callers (guards are bypassed and the door is out of scope).
  This is asserted deliberately in GC030: it is *why* the `unguarded` flag is loud.
- **R-G13** The **door layer is upstream and out of scope**: roles, profiles,
  per-member overrides, and capability allowlists decide whether the op may be
  called at all; this suite binds `$caller` post-door and tests what guards
  do. Fixture principals map 1:1 to `members` rows. Two pins the suite records
  without owning:
  - **Every door refusal is `access_denied` (403), identically**: unregistered
    name, forbidden op, missing/malformed/expired token, unsatisfied step-up.
    No case in this suite may expect `access_denied` from a guard, and no case
    may expect a guard to distinguish a registered op from an unregistered one.
  - **The space-owner acl-administration capability is checked at the door**,
    once per call, before planning, and **not** as an extra disjunct inside each
    row's guard (spec §8, bootstrap rule 6). So it cannot deadlock on the rows
    it exists to repair, and no arrangement of data can lock an owner out.
    Corollary, disclosed: **no acl row is private from the
    space owner**. An owner who can administer acl rows can grant themselves
    any level on any row. Every such reach is an ordinary attributed command in
    the chain (GC093).
- **R-G14** The acl-derived link entity carries **derived guards**: read
  requires any level on the parent row (members may see who else has access,
  which keeps "who can call X" one query); write, direct or via the derived
  admin ops, requires the **top level**. Level ordering is full-ordering for
  permissions: read ⊂ write ⊂ admin (the spec's "for read purposes" phrasing
  is pinned to mean each level includes all lower levels' rights).
- **R-G15** Guard walks count as part of the op's inlined base-entity set:
  their entities appear in the scope manifest and are subscribed by `?live`,
  so a membership or ACL write invalidates guarded live queries (GC084, GC086).
- **R-G16** **Guard internals are not outputs**: composition sees a callee's
  output shape only, and guard-only fields (`canWrite`) are not in it, so
  conditions on them are `not_output_field`. There is no introspection
  channel through which a caller can probe another principal's guard facts.
- **R-G17** The derived `grant` op is **converge-or-create** on
  `unique(board, member)`: granting an existing holder converges the level;
  mode label `creates-or-converges`. `revoke` is an addressed delete by the
  same key.
- **R-G18** Guest exposure is checked at **share registration**
  (`std.space.share`, `share_ops::validate`): the listed ops are read against
  the deployed program's pins, and any listed op that is not guard-bearing
  (no read guard on any entity it touches and no `$caller` condition, or
  marked `unguarded`) fails with `guest_exposure_unguarded`. An op is
  guard-bearing if *any* inlined stage is.
- **R-G19** **`let x = set …` binds a create, and only a create** (spec §3,
  "Naming a freshly created row in the same op"). The bound value is a **row
  descriptor** (`id`, `rev`, and the fields that statement wrote), usable in
  *later* statements of the same op, never in the same statement and never in
  a condition of the statement that produced it. `let` on a write whose derived
  mode is CONVERGE, CAS, or BULK is the compile error `unbindable_write`. The
  guard consequence is the one this suite cares about: `let b = set boards {…}`
  followed by `set boardsAccess { board: b.id, … }` is **two statements in one
  atomic group**: the create is guard-exempt under R-G11a, the dependent write
  is **not** and is evaluated in the ordinary way (GC094, GC095).
- **R-G20** **A routine's bound follows its trigger's provenance, not its
  principal** (spec §8). A **clock**-woken routine (`every …`) runs with guards
  passing, because nobody outside chose the moment. A **listener**-woken routine
  (`when …`) may read and write **only rows reachable from `$trigger.ids`**;
  every other row is invisible exactly as a guard would make it, because
  `ingested` and `fed` triggers are fired by outsiders and a globally
  guard-exempt listener is a confused deputy an outsider steers. A listener
  that must legitimately range wider says `run(…) unguarded`. The suite
  asserts all three (GC096, GC097, GC098).
  *The rationale "a routine holds no members row, so `$caller` is null and
  every guard evaluates false" is wrong and must not appear in a case
  justification*: it describes R-G12's anonymous caller, and it would make
  every routine useless.
- **R-G21** **The explicit-open spelling is `access read open` /
  `access write open`.** `open` is a **keyword and no expression**: it is the
  entire clause or it is a `parse_error`, so there is no partially-open guard.
  The earlier spelling `access write true` **did not parse and is withdrawn**
  (R-G3 admits literals only as an `==` right-hand side, and every disjunct
  must reference `$caller`), and an entity whose verb is `open` is **not
  guard-bearing**, so exposing an op over it to a guest is still
  `guest_exposure_unguarded` under R-G18 (GC099, GC100).
- **R-G22** **Under a live delegation, `$caller` is the delegator**
  (`plan-delegation-surface.md` §7, option 2; owner ruling). The door has
  already bounded the call by the delegator's reach and the agent's
  allowlist; the guards then answer for the same person, so an agent sees
  exactly the delegator's rows and meets exactly the delegator's write
  answers. The agent is **never a guard input**: it reaches the command log
  only, which records `agent:<agent> on behalf of user:<delegator>`. Without
  a live delegation the agent is its own caller, and with no seat it sees
  nothing (GC101).

---

## 5. Corpus

Format: case id, then kind. Ordo source (fixture library §3.4 assumed
registered), then one invocation block per principal: `as:`, `params:` where
any, `→` expected rows as tuples in projection order (top to bottom = expected
row order), or `→ reject:`/`→ access_guard` outcomes. **write** invocations
each start from a fresh seed (§3.5).

### A. Read-guard filtering (GC001–GC010)

**GC001: exec.** The flagship: one op, four principals, four answers.
```
taskBoard
```
- `as: ana` → 3 rows: (audit books, open, proj_3) · (draft spec, open, proj_1)
  · (review spec, done, proj_1)
- `as: bram` → 4 rows: (build site, open, proj_2) · (draft spec, open, proj_1)
  · (review spec, done, proj_1) · (ship site, open, proj_2)
- `as: cleo` → 2 rows: (build site, open, proj_2) · (ship site, open, proj_2)
- `as: dov` → 0 rows (**an empty result, not an error**)

**GC002: exec.** Guard conjoins with the op's own condition (`status ==
"open"`).
```
openTasks
```
- `as: ana` → 2 rows: (audit books, proj_3) · (draft spec, proj_1)
- `as: bram` → 3 rows: (build site, proj_2) · (draft spec, proj_1) ·
  (ship site, proj_2)
- `as: dov` → 0 rows

**GC003: exec.** `or`-combination: the owner disjunct admits Cleo to proj_3
with no membership row; Ana qualifies for proj_1 through *both* disjuncts and
appears once (semi-join, never a join fan-out).
```
myProjects
```
- `as: ana` → 2 rows: (Alpha, mem_1) · (Gamma, mem_3)
- `as: bram` → 2 rows: (Alpha, mem_1) · (Beta, mem_2)
- `as: cleo` → 2 rows: (Beta, mem_2) · (Gamma, mem_3)
- `as: dov` → 0 rows

**GC004: exec.** Guard on the link entity itself. No sort → id order. Note
Cleo, *owner* of proj_3, cannot see proj_3's roster: `projectMembers`' read
guard has no owner disjunct, and guards mean exactly what they say.
```
teamRoster
```
- `as: ana` → 3 rows: (proj_1, mem_1, true) · (proj_1, mem_2, false) ·
  (proj_3, mem_1, false)
- `as: cleo` → 2 rows: (proj_2, mem_2, true) · (proj_2, mem_3, true)
- `as: dov` → 0 rows

**GC005: exec.** Caller conditions narrow the guarded set; they never widen
it.
```
op openIn($p: id) =
  tasks(status == "open" and project == $p)[title]
  sort title
```
- `as: ana`, params `$p = proj_1` → 1 row: (draft spec)
- `as: ana`, params `$p = proj_2` → 0 rows (guard hides proj_2 from Ana)
- `as: bram`, params `$p = proj_2` → 2 rows: (build site) · (ship site)

**GC006: exec.** Conditions cannot launder the guard: Bram is *assigned*
task_5 but not a member of proj_3, so the row stays hidden.
```
myAssignedTasks
```
- `as: ana` → 1 row: (draft spec)
- `as: cleo` → 1 row: (build site)
- `as: bram` → 0 rows
- `as: dov` → 0 rows

**GC007: exec + compile.** Dot-path guard stacking (R-G6): the plan reads
`projects`, so both tasks' and projects' read guards inline and both appear in
the manifest. (Fixture guards imply each other for these rows, so the rows
match GC001's; the stacking is the metadata assertion.)
```
op taskSheet = tasks[title, project.name]  sort title
```
- `as: ana` → 3 rows: (audit books, Gamma) · (draft spec, Alpha) ·
  (review spec, Alpha)
- compile: manifest `guards` lists tasks.read AND projects.read; scope
  manifest includes `projects.{name}` + guard-read fields

**GC008: exec.** Group over the guarded view: buckets are per-caller;
unsorted group orders by key asc (sibling R4).
```
tasks[status]
group status { n: count }
```
- `as: ana` → 2 rows: (done, 1) · (open, 2)
- `as: cleo` → 1 row: (open, 2)
- `as: dov` → 0 rows (no visible rows, no buckets)

**GC009: exec.** Reduce over an empty guarded view: `count` of nothing is 0.
The result is one row, not zero rows, and still not an error.
```
tasks[title]
reduce { n: count }
```
- `as: bram` → 1 row: (4)
- `as: dov` → 1 row: (0)

**GC010: exec.** Guard precedes `sort`/`take` in the same body (R-G7):
take-1 ranks the *caller-visible* tasks.
```
firstTask
```
- `as: ana` → 1 row: (audit books)
- `as: cleo` → 1 row: (build site)
- `as: dov` → 0 rows

### B. Write guards and atomicity (GC011–GC022)

**GC011: write.** Allowed addressed converge.
```
renameTask
```
- `as: ana`, params `$id = task_1, $t = "draft spec v2"` → applied; descriptor
  returns the row with the new title and bumped `rev`; readback `taskBoard`
  `as: ana` shows (draft spec v2, open, proj_1)

**GC012: write.** Disallowed addressed converge: Bram *reads* task_1
(member) but `canWrite` is false.
```
renameTask
```
- `as: bram`, params `$id = task_1, $t = "hijack"` → **access_guard**; nothing
  applied; readback `as: ana` shows title unchanged (draft spec)

**GC013: write.** Addressed write on a row outside the caller's read view
is **`not_found`**, not `access_guard` (R-G10, re-ruled): Dov reads no task in
proj_1, so for him task_1 is indistinguishable from a row that never existed.
Contrast GC012, one case above: same op, same row, and Bram gets
`access_guard` *because he can read it*.
```
renameTask
```
- `as: dov`, params `$id = task_1, $t = "x"` → **not_found**; nothing applied
- *(previously asserted `access_guard`; withdrawn with the ruling)*

**GC014: write.** Create allowed: guard evaluated against the would-be row
(R-G11). Bram has canWrite in proj_2.
```
addTask
```
- `as: bram`, params `$p = proj_2, $t = "deploy"` → applied; readback
  `openTasks` `as: bram` → (build site, proj_2) · (deploy, proj_2) ·
  (draft spec, proj_1) · (ship site, proj_2)

**GC015: write.** Create denied: member without canWrite (Bram in proj_1).
```
addTask
```
- `as: bram`, params `$p = proj_1, $t = "sneak"` → **access_guard**; nothing
  created

**GC016: write.** Create denied: outsider and anonymous. Both answer
**`not_found`**, not `access_guard` (R-G10, re-ruled in V-010; this case
previously asserted the opposite and was the last place in the corpus that
contradicted the spec). The old reasoning was that a create addresses no
existing row, so there is no row whose existence the answer could disclose.
That misses the parent: `$p = proj_1` is an id in the caller's hand, and
answering `access_guard` confirms proj_1 exists to someone who may not read it
That is the enumeration oracle R-G10 exists to close, reached through the one verb
that was never addressed at a row. The parent reference resolves under the
ordinary read view *before* the would-be row's guard runs.

Contrast **GC015** directly above: Bram *can* read proj_1 and lacks
`canWrite`, so the parent resolves and the would-be row's guard (R-G11) is what
fails with `access_guard`, correctly, because Bram has already been shown the
project. The rule across both: **informative exactly when the caller has
already been shown the row.**
```
addTask
```
- `as: dov`, params `$p = proj_1, $t = "sneak"` → **not_found** (on the parent;
  byte-identical to `$p = proj_nonexistent`)
- `as: anonymous`, params `$p = proj_1, $t = "sneak"` → **not_found**
  (R-G12: anonymous has no read view, so the parent is not visible)
- `as: dov`, params `$p = proj_nonexistent, $t = "sneak"` → **not_found**,
  indistinguishable from the line above in code, message and cost

**GC017: write.** Atomic group aborts: Ana may close task_1 but not task_3,
and the second statement's failure aborts the whole group. task_3 is in proj_2,
where Ana holds no membership row, so it is outside her **read** view and the
rejection class is `not_found` (R-G10, re-ruled; *previously asserted
`access_guard`*). The abort behaviour is what this case exists to pin and is
unchanged: the class of the failure does not change the atomicity of the
group.
```
closePair
```
- `as: ana`, params `$a = task_1, $b = task_3` → **not_found**; **zero
  statements applied**; readback `taskBoard` `as: ana` still shows
  (draft spec, **open**, proj_1)

**GC018: write.** Same op, qualified caller: both statements land.
```
closePair
```
- `as: bram`, params `$a = task_3, $b = task_4` → applied; readback
  `openTasks` `as: bram` → 1 row: (draft spec, proj_1)

**GC019: write.** Bulk delete ranges over the caller's write view (R-G9).
Done tasks in seed: task_2 (proj_1) only.
```
purgeDone
```
- `as: bram` → 0 rows deleted (task_2 is outside Bram's write view); vacuous
  no-op, **not** an error; readback `taskBoard` `as: bram` still 4 rows
- `as: ana` → 1 row deleted; readback `taskBoard` `as: ana` → 2 rows:
  (audit books, open, proj_3) · (draft spec, open, proj_1)

**GC020: write.** Bulk converge, same scoping: Cleo's write view is
task_3/task_4; task_1, task_5 (open, but unwritable by her) are untouched.
```
shipAll
```
- `as: cleo` → 2 rows affected; readback `taskBoard` `as: cleo` →
  (build site, done, proj_2) · (ship site, done, proj_2); readback
  `taskBoard` `as: ana` shows (draft spec, **open**, proj_1) ·
  (audit books, **open**, proj_3)

**GC021: write.** Read guard ≠ write guard: Ana reads proj_3 (member) but
only the owner writes projects.
```
renameProject
```
- `as: cleo`, params `$id = proj_3, $n = "Gamma2"` → applied (owner)
- `as: ana`, params `$id = proj_3, $n = "Mine"` → **access_guard**

**GC022: write.** Hand-rolled meta-permission: membership rows are
owner-writable. Cleo has canWrite in proj_2 for *tasks*, not for the
roster.
```
addMember
```
- `as: bram`, params `$p = proj_2, $m = mem_4` → applied; readback
  `teamRoster` `as: bram` gains (proj_2, mem_4, false)
- `as: cleo`, params `$p = proj_2, $m = mem_4` → **access_guard**

### C. Inheritance and override (GC023–GC029)

**GC023: exec.** Depth-2 read inheritance: `taskNotes` carries no `access`
clause; the effective guard is tasks' clause rebased to
`task.project.projectMembers(member == $caller)` (R-G4).
```
noteFeed
```
- `as: ana` → 2 rows: (numbers pending, task_5) · (outline ready, task_1)
- `as: bram` → 2 rows: (outline ready, task_1) · (site copy wip, task_3)
- `as: cleo` → 1 row: (site copy wip, task_3)
- `as: dov` → 0 rows

**GC024: write.** Inherited write guard, create at depth 2: Cleo has
canWrite in proj_2.
```
addNote
```
- `as: cleo`, params `$t = task_3, $b = "final copy in"` → applied; readback
  `noteFeed` `as: cleo` → (final copy in, task_3) · (site copy wip, task_3)

**GC025: write.** Inherited write guard denies: Bram reads note_1's task but
lacks canWrite in proj_1; nobody holds canWrite in proj_3.
```
addNote
```
- `as: bram`, params `$t = task_1, $b = "drive-by"` → **access_guard**
- `as: ana`, params `$t = task_5, $b = "drive-by"` → **access_guard** (member
  of proj_3, canWrite false)

**GC026: write.** Inherited guard on addressed converge, both failure
shapes: readable-but-unwritable and entirely hidden.
```
editNote
```
- `as: ana`, params `$id = note_3, $b = "x"` → **access_guard** (Ana reads
  note_3; no proj_3 canWrite; the informative case, unchanged)
- `as: cleo`, params `$id = note_3, $b = "x"` → **not_found** (R-G10,
  re-ruled: Cleo owns proj_3 but the inherited guard hides its notes from
  her, so the row is outside her read view and the answer is identical to
  absence. *Previously asserted `access_guard`.*) This pair is the whole
  ruling in one case: same op, same row, two callers, and the difference is
  exactly whether the caller could already see it.

**GC027: compile.** Inheritance is disclosed, rebased: `noteFeed`'s manifest
lists read guard `task.project.projectMembers(member == $caller)` attributed
`inherited from tasks`; scope manifest includes `tasks.{project}` and
`projectMembers.{project, member}` as guard reads.

**GC028: exec.** Override replaces inherited guard (read side). Cleo owns
proj_3: the *projects* guard (owner disjunct) admits her; tasks' explicit
clause replaced any inheritance from projects, so its membership-only guard
excludes her. Same caller, adjacent entities, opposite answers.
```
op gammaProject($p: id) = projects(id == $p)[name]
```
- `as: cleo`, params `$p = proj_3` → 1 row: (Gamma)
```
tasksIn
```
- `as: cleo`, params `$p = proj_3` → 0 rows

**GC029: write.** Override replaces inherited guard (write side). If tasks
inherited projects' `owner == $caller` write guard, Cleo could close task_5.
She cannot; neither can Ana (member, canWrite false). task_5 is writable by
nobody, exactly as the guards read.
```
closeTask
```
- `as: cleo`, params `$id = task_5` → **not_found** (R-G10, re-ruled: Cleo
  owns proj_3 but holds no membership row in it, so tasks' own guard hides
  task_5 from her entirely. *Previously asserted `access_guard`*, which
  would have confirmed to her that the row exists, the exact leak the override
  ruling exists to prevent.)
- `as: ana`, params `$id = task_5` → **access_guard** (pm_5 makes task_5
  readable to Ana; `canWrite` is false, so this stays the informative case)

### D. `unguarded` (GC030–GC035)

**GC030: exec.** Bypass works for any principal, including anonymous
(R-G12: this is why the flag is loud and greppable).
```
allTasks
```
- `as: dov` → 5 rows: (audit books, open) · (build site, open) ·
  (draft spec, open) · (review spec, done) · (ship site, open)
- `as: anonymous` → same 5 rows

**GC031: compile.** The manifest carries the flag *and* what it bypasses:
`allTasks` meta shows `unguarded: true`, bypassed guard list `[tasks.read]`,
cost class `sql`.

**GC032: reject.** Superfluous marker: vendors is unguarded, so the flag
would be decoration (R-G8).
```
op v unguarded = vendors[name]
```
→ reject: `unguarded_superfluous`

**GC033: reject.** The marker cannot strip a composed callee's guard.
`openTasks` is guarded, `leak`'s own body writes no guarded entity source
directly, so its marker is superfluous and the guard survives regardless
(R-G7/R-G8).
```
op leak unbounded unguarded = openTasks[title]
```
→ reject: `unguarded_superfluous`

**GC034: reject.** Composing a live-bypass callee requires the caller's own
acknowledgment (R-G8, mirroring composition-R9 for `unbounded`).
```
op openAudit unbounded = allTasks(status == "open")[title]
```
→ reject: `unacknowledged_unguarded`

**GC035: exec + compile.** Same op, acknowledged: results are
caller-independent; the manifest discloses the inherited bypass.
```
op openAudit unbounded unguarded = allTasks(status == "open")[title]
```
- `as: ana` → 4 rows: (audit books) · (build site) · (draft spec) ·
  (ship site)
- `as: dov` → same 4 rows
- compile: `unguarded: true`, bypassed `[tasks.read]` (attributed through
  `allTasks`)

**Why all three carry `unbounded`.** `openTasks` and `allTasks` are declared
`unbounded` in §3.4, and composition-R9 puts the acknowledgment on the
**caller**, so a composing op that omits it is `unacknowledged_unbounded`
at the registration gate, before the gate ever asks what the op bypasses.
GC033–GC035 are about the bypass gate, so the row-bound acknowledgment is
part of their text and not part of what they assert. *(Their earlier text
omitted it and did not compile; corrected 2026-08-30, see
[`corpus-rulings.md`](corpus-rulings.md).)*

### E. `acl` derivation (GC036–GC048)

**GC036: compile.** Derived machinery exists and has the pinned shape:
entity `boardsAccess` with `board -> boards owned`, `member -> members owned`,
`level: enum("read", "write", "admin")` (levels ordered left-to-right),
`unique (board, member)`; registered ops `grantBoardsAccess`,
`revokeBoardsAccess`, `setBoardsAccessLevel`, each with a manifest and mode
label; boards + owned subtree guard-bearing; derived guards on `boardsAccess`
per R-G14. All rendered in the shape summary.

**GC037: exec.** Any level reads (read ⊂ write ⊂ admin).
```
boardList
```
- `as: ana` → 1 row: (roadmap)
- `as: bram` → 2 rows: (icebox) · (roadmap)
- `as: cleo` → 1 row: (roadmap)
- `as: dov` → 0 rows

**GC038: exec.** ACL inheritance to the owned child (`cards`).
```
cardList
```
- `as: ana` → 2 rows: (q3 goals, false, board_1) · (q4 goals, true, board_1)
- `as: bram` → 3 rows: (q3 goals, false, board_1) · (q4 goals, true, board_1)
  · (someday, false, board_2)
- `as: cleo` → 2 rows: (q3 goals, false, board_1) · (q4 goals, true, board_1)
- `as: dov` → 0 rows

**GC039: write.** Write requires level ≥ write (R-G14): write-holder and
admin-holder both succeed.
```
setCardDone
```
- `as: bram`, params `$id = card_1, $d = true` → applied
- `as: ana`, params `$id = card_1, $d = true` → applied (admin ⊇ write)

**GC040: write.** Read level cannot write.
```
setCardDone
```
- `as: cleo`, params `$id = card_1, $d = true` → **access_guard**
- `as: guest:gil`, params `$id = card_1, $d = true` → **access_guard**

**GC041: write.** Create into the owned child under the inherited ACL.
```
addCard
```
- `as: bram`, params `$b = board_1, $t = "q5 goals"` → applied; readback
  `cardList` `as: cleo` gains (q5 goals, false, board_1)
- `as: cleo`, params `$b = board_1, $t = "sneak"` → **access_guard**

**GC042: write.** Grant by the admin: the granted member's view opens.
```
grantBoardsAccess
```
- `as: ana`, params `$board = board_1, $member = mem_4, $level = "read"` →
  applied; readback `boardList` `as: dov` → 1 row: (roadmap); `cardList`
  `as: dov` → 2 rows: (q3 goals, false, board_1) · (q4 goals, true, board_1)

**GC043: write.** Grant below the top level: the derived admin ops are gated
by `admin` only, and write is not enough.
```
grantBoardsAccess
```
- `as: bram`, params `$board = board_1, $member = mem_4, $level = "read"` →
  **access_guard** (Bram holds write on board_1)
- `as: cleo`, params `$board = board_1, $member = mem_4, $level = "read"` →
  **access_guard**

**GC044: write.** Revoke: admin succeeds; non-admin rejected; the revoked
member's view closes.
```
revokeBoardsAccess
```
- `as: ana`, params `$board = board_1, $member = mem_3` → applied; readback
  `boardList` `as: cleo` → 0 rows
- `as: cleo`, params `$board = board_1, $member = mem_2` → **access_guard**

**GC045: write.** Admin is per-row, not global: Bram is admin of board_2
only.
```
grantBoardsAccess
```
- `as: bram`, params `$board = board_2, $member = mem_1, $level = "write"` →
  applied
- `as: bram`, params `$board = board_1, $member = mem_4, $level = "read"` →
  **access_guard**

**GC046: exec.** The ACL rows themselves read at any level (R-G14), so the
roster is visible to everyone on the board, guests included. No sort → id
order.
```
aclRoster
```
- `as: ana` → 4 rows: (board_1, mem_1, admin) · (board_1, mem_2, write) ·
  (board_1, mem_3, read) · (board_1, mem_g, read)
- `as: bram` → 5 rows: the four above · (board_2, mem_2, admin), in id order
  (ba_1 … ba_5): (board_1, mem_1, admin) · (board_1, mem_2, write) ·
  (board_1, mem_3, read) · (board_2, mem_2, admin) · (board_1, mem_g, read)
- `as: guest:gil` → 4 rows: the board_1 rows
- `as: dov` → 0 rows

**GC047: write.** Derived guards protect the link entity against direct
writes too, so grant-by-`set` is still admin-gated.
```
op sneakGrant($b: id, $m: id) =
  set boardsAccess { board: $b, member: $m, level: "admin" }
```
- `as: bram`, params `$b = board_1, $m = mem_4` → **access_guard**
- `as: ana`, params `$b = board_1, $m = mem_4` → applied (admin of board_1)

**GC048: compile.** Inherited-ACL disclosure: `cardList`'s manifest lists the
boards ACL read guard attributed `inherited from boards (acl)`; guard reads
`boardsAccess.{board, member}` appear in the scope manifest.

**`level` is not among them, and this sentence used to say it was.** `level` is
tested only by the **write** guard and `cardList` is a read, so post-pruning
drops it, following the same discipline this document states for `canWrite` in GC080.
The two entries could not both be right, and the engine has always followed
GC080's: the conformance assertion has never included `level`. Owner-ruled
2026-09-26, settled by fixing this document rather than the engine. The
divergence GC048 recorded is retired.

### F. Guards × composition and refinement (GC049–GC058)

**GC049: exec.** Composing a guarded query keeps the guard: `specTasks`
composes `openTasks`; the guard rides inside.
```
specTasks
```
- `as: ana` → 1 row: (draft spec)
- `as: bram` → 1 row: (draft spec)
- `as: cleo` → 0 rows
- `as: dov` → 0 rows

**GC050: exec.** Refinement envelope: the guard applies first (in the callee
body), and the refinement narrows the guarded result. It can never reach rows
the guard removed.
```
openTasks(project == $p)[title]
```
- `as: bram`, params `$p = proj_2` → 2 rows: (build site) · (ship site)
- `as: ana`, params `$p = proj_2` → 0 rows
- `as: dov`, params `$p = proj_2` → 0 rows

**GC051: exec.** Guard before the callee's `take`, observed through
composition (R-G7): `firstTask` yields each caller's own top row; the caller
condition then filters that one-row *result*.
```
firstTask(title contains "audit")[title]
```
- `as: ana` → 1 row: (audit books)
- `as: bram` → 0 rows (his firstTask is (build site) and the filter drops it;
  it is not "first matching task")
- `as: cleo` → 0 rows

**GC052: compile.** Composed manifests disclose inherited guards:
`specTasks` meta lists tasks' read guard attributed through `openTasks`;
scope manifest = `tasks.{status, title, project}` +
`projectMembers.{project, member}`; callee list `{openTasks: 1}`.

**GC053: exec.** Depth-2 composition, guard intact into an aggregate.
```
specCount
```
- `as: ana` → 1 row: (1)
- `as: dov` → 1 row: (0)

**GC054: exec.** Composed guarded source with a refinement over acl-guarded
data; callee order (cardList's `sort title`) is the result order.
```
openCards
```
- `as: ana` → 1 row: (q3 goals, board_1)
- `as: bram` → 2 rows: (q3 goals, board_1) · (someday, board_2)
- `as: guest:gil` → 1 row: (q3 goals, board_1)

**GC055: exec.** Guarded composed source under `match any` beside a guarded
entity source: both guards compose, so a project qualifies only if the caller
reads it AND has a visible open task in it. No sort → anchor id order.
```
projects[name]
match any openTasks
```
- `as: ana` → 2 rows: (Alpha) · (Gamma)
- `as: bram` → 2 rows: (Alpha) · (Beta)
- `as: cleo` → 1 row: (Beta); proj_3 is readable via ownership but has no
  Cleo-visible tasks

**GC056: exec.** Parameter flow-through leaves the guard intact.
```
tasksIn
```
- `as: bram`, params `$p = proj_1` → 2 rows: (draft spec, open) ·
  (review spec, done)
- `as: cleo`, params `$p = proj_1` → 0 rows

**GC057: compile.** Caller-independent artifact: `taskBoard` compiles to
exactly one planned statement; `$caller` is a reserved binding slot in the
plan; the version hash is identical across all `as:` invocations (the harness
asserts no per-caller compilation ever happens).

**GC058: reject.** Guard internals are not outputs (R-G16): `canWrite` is
read by tasks' *write* guard, never projected by `openTasks`.
```
openTasks(canWrite == true)[title]
```
→ reject: `not_output_field`

### G. Guest access (GC059–GC066)

**GC059: exec.** The guest passes exactly the guard their ACL row satisfies
(ba_5: read on board_1), nothing more and nothing less.
```
boardList
```
- `as: guest:gil` → 1 row: (roadmap)
```
cardList
```
- `as: guest:gil` → 2 rows: (q3 goals, false, board_1) ·
  (q4 goals, true, board_1)

**GC060: exec.** No role fallback: guests hold no role and no profile, so
guard-bearing ops over entities where they hold no ACL/membership return
empty. They never return an error or a default view.
```
openTasks
```
- `as: guest:gil` → 0 rows
```
myProjects
```
- `as: guest:gil` → 0 rows

**GC061: write.** Guest write attempt at read level → `access_guard`
(already GC040 second invocation; repeated here as the guest-flow assertion
with a create).
```
addCard
```
- `as: guest:gil`, params `$b = board_1, $t = "guest card"` →
  **access_guard**

**GC062: write.** Guest cannot reach admin ops' guard.
```
grantBoardsAccess
```
- `as: guest:gil`, params `$board = board_1, $member = mem_4,
  $level = "read"` → **access_guard**

**GC063: reject.** Exposing a non-guard-bearing op to a guest fails at share
registration (R-G18): the op has no reachable guard (no entity it touches has
a read guard) and no `$caller` condition. In the fixture, `activeProducts`
touches only `products`, which nothing guards.
```
std.space.share  ops: activeProducts
```
→ reject: `guest_exposure_unguarded`

**GC064: reject.** An `unguarded`-marked op is by definition not
guard-bearing for its guarded entities, so it cannot be exposed to guests.
```
std.space.share  ops: allTasks
```
→ reject: `guest_exposure_unguarded`

**GC065: accept.** Guard-bearing ops register cleanly; the registration
records the basis per op, from the pins.
```
std.space.share  ops: openTasks, cardList, myAssignedTasks
```
→ accepted; the basis per op: entity guard (openTasks: `tasks` read walk),
inherited acl guard (cardList: `cards` read walk through `boardsAccess`),
`$caller` condition + entity guard (myAssignedTasks: `tasks` is
caller-restricted and guarded)

**GC066: write + exec.** Revocation closes the window at the next call.
Sequence (one seed): `revokeBoardsAccess` `as: ana`, params
`$board = board_1, $member = mem_g` → applied; then
```
boardList
```
- `as: guest:gil` → 0 rows
```
cardList
```
- `as: guest:gil` → 0 rows

### H. `$caller` lifecycle (GC067–GC073)

**GC067: behavioral (runtime reject).** A request supplying `$caller` as a
client parameter is rejected at the door, before planning. The binding never
happens and the guard is never consulted.
```
openTasks
```
- `as: dov`, params `$caller = mem_1` → runtime reject: **caller_supplied**
  (not an empty result, because the request itself is malformed)

**GC068: behavioral (runtime reject).** Same on a write op.
```
claimTask
```
- `as: cleo`, params `$id = task_3, $caller = mem_2` → runtime reject:
  **caller_supplied**; nothing applied

**GC069: reject.** `$caller` in a shape default: defaults are evaluated from
the closed set (literals, `today`, `now`, `newid`) with no session in scope.
```
entity drafts {
  author -> members = $caller
  body: text
}
```
→ reject: `caller_in_default`

**GC070: write.** `$caller` in write RHS is legal and binds the session
principal (R-G2).
```
claimTask
```
- `as: bram`, params `$id = task_3` → applied; task_3.assignee = mem_2;
  readback `myAssignedTasks` `as: bram` → 1 row: (build site); task_5 is
  also assigned to Bram but stays guard-hidden (GC006's point, post-write)

**GC071: exec.** `$caller` condition and entity guard conjoin. Both apply,
and neither substitutes for the other.
```
op myOpenIn($p: id) =
  tasks(assignee == $caller and project == $p and status == "open")[title]
```
- `as: cleo`, params `$p = proj_2` → 1 row: (build site)
- `as: bram`, params `$p = proj_2` → 0 rows (no visible assignment there)

**GC072: exec + write.** Anonymous principal (R-G12): every guard evaluates
false, so every guarded row is outside an anonymous caller's read view.
```
openTasks
```
- `as: anonymous` → 0 rows
```
closeTask
```
- `as: anonymous`, params `$id = task_1` → **not_found** (R-G10: addressed
  write, guarded entity, no read view. *Previously asserted `access_guard`*,
  which was the sharpest form of the old leak, since an anonymous caller with
  a stolen id list could have enumerated the space.)
- `as: anonymous`, params `$id = task_404` (no such row) → **not_found**,
  byte-identical

**GC073: compile.** Caller-scoping is disclosed metadata:
`myAssignedTasks` meta carries `callerScoped: true` (body uses `$caller`);
`taskBoard` carries it too (guard-injected). The catalog/hover surface for
both names the guard clause. Visibility shaping lives in the op, never in an
invisible policy.

### I. Guard grammar rejects (GC074–GC079)

**Every entity in this section spells both verbs.** These cases are about the
READ walk, but an `access` declaration is *both verbs or neither* (spec G9),
so an entity carrying only `access read` is `incomplete_access_clause` before
the walk is looked at, and the case would never reach the gate it is written
to test. The write verb is therefore `open`, R-G21's own prescribed repair,
which adds nothing to what is asserted. *(Their earlier text declared `access
read` alone and did not compile; corrected 2026-08-30, see
[`corpus-rulings.md`](corpus-rulings.md).)*

**GC074: reject.** R-G1 direct: guard on a sealed field of the entity
itself.
```
entity privateDocs {
  owner -> members
  label: text = ""   sealed
  body:  text
  access read   owner == $caller or label == "public"
  access write  open
}
```
→ reject: `guard_on_sealed`

**GC075: reject.** R-G1 through a walk: `orders.memo` is sealed in the base
`market` fixture.
```
entity orderFlags {
  order   -> orders  owned
  handler -> members
  note: text
  access read   handler == $caller or order.memo == "gift"
  access write  open
}
```
→ reject: `guard_on_sealed`

**GC076: reject.** No `$caller` in any disjunct: a caller-blind guard is a
hidden filter and no authorization (R-G3), even when built from legal atoms.
```
entity banners {
  project -> projects  owned
  live: bool = false
  access read   live
  access write  open
}
```
→ reject: `invalid_guard_walk`

**GC077: reject.** Functions/operators outside the closed guard grammar.
```
entity notices {
  project -> projects  owned
  title: text
  access read   project.owner == $caller or length(title) > 3
  access write  open
}
```
→ reject: `invalid_guard_walk`

**GC078: reject.** Walk must follow declared refs: `boardsAccess` declares
no ref to `memos`, so the reverse step is unreachable from here.
```
entity memos {
  author -> members
  body: text
  access read   boardsAccess(member == $caller)
  access write  open
}
```
→ reject: `invalid_guard_walk`

**GC079: reject.** `access` takes exactly `read` or `write`; `admin` exists
only as an `acl` level.
```
entity wikis {
  owner -> members
  title: text
  access admin  owner == $caller
}
```
→ reject: `parse_error`

### J. Metadata and cost (GC080–GC083)

**GC080: compile.** Guard semi-joins classify `sql`: `openTasks` meta shows
cost class `sql`; scope manifest = `tasks.{status, title, project}` +
`projectMembers.{project, member}` (guard reads; `canWrite` is absent because the
*read* guard never touches it, the post-pruning discipline of composition-R11).

**GC081: compile.** Write op metadata: `closeTask` mode label `converges`
(unchanged by guards); write guard listed as a precondition:
`project.projectMembers(member == $caller and canWrite)`; guard reads
`projectMembers.{project, member, canWrite}`; cost class `sql`.

**GC082: compile.** Depth-2 rebased guard is still one indexed semi-join
chain: `noteFeed` (walk `task.project.projectMembers`) classifies `sql`;
manifest `taskNotes.{body, task}` + `tasks.{project}` +
`projectMembers.{project, member}`.

**GC083: compile.** Derived admin op metadata: `grantBoardsAccess` mode
label `creates-or-converges` (converge-or-create on `unique(board, member)`,
R-G17: regranting an existing holder converges the level);
`revokeBoardsAccess` is an addressed delete; both manifests carry
`gatedBy: boardsAccess.level == "admin"` and classify `sql`.

### K. Behavioral tier (GC084–GC087)

**GC084: behavioral.** A membership-row write invalidates a `?live` guarded
query (R-G15: guard entities are inlined base entities). Subscribe
`openTasks?live` `as: dov` → initial result 0 rows. Then `as: bram`:
```
addMember($p: proj_2, $m: mem_4)
```
→ invalidation fires on Dov's subscription (`projectMembers` is inlined via
the guard); refetch `as: dov` → 2 rows: (build site, proj_2) ·
(ship site, proj_2). Execution-tier.

**GC085: behavioral.** No false invalidation: with `openTasks?live`
subscribed `as: ana`, run `as: ana`:
```
setCardDone($id: card_1, $d: true)
```
→ no invalidation (cards/boards/boardsAccess are not inlined by `openTasks`).

**GC086: behavioral.** ACL grant invalidates acl-guarded live queries.
Subscribe `cardList?live` `as: dov` → 0 rows. Then `as: ana`:
```
grantBoardsAccess($board: board_1, $member: mem_4, $level: "read")
```
→ invalidation (boardsAccess inlined via the derived guard); refetch
`as: dov` → 2 rows: (q3 goals, false, board_1) · (q4 goals, true, board_1).

**GC087: behavioral.** Live revocation: the guest window closes without a
poll. Subscribe `boardList?live` `as: guest:gil` → 1 row: (roadmap). Then
`as: ana`:
```
revokeBoardsAccess($board: board_1, $member: mem_g)
```
→ invalidation; refetch `as: guest:gil` → 0 rows. (Access stops; the pin
that a guest's mirror may retain already-fetched data is platform-tier and
out of scope here.)

### L. Platform `members`, non-enumerability, acl bootstrap (GC088–GC095)

**GC088: reject + exec.** `members` is platform-provided and **undeclarable**;
its field list is fixed and `name` is not in it.
```
entity members {
  name: text
}
```
→ reject: `parse_error` (the name is reserved in the entity namespace; a
domain refs `members` and never declares it)

The projection every roster screen actually wants is `displayName`:
```
op teamNames($p: id) unbounded = projectMembers(project == $p)[member.displayName]
```
- `as: ana`, params `$p = proj_1` → 2 rows: (Ana) · (Bram)
- `as: dov`, params `$p = proj_1` → 0 rows (the link entity's own guard is
  `project.projectMembers(member == $caller)`; Dov holds none)

```
op teamNamesBad($p: id) unbounded = projectMembers(project == $p)[member.name]
```
→ reject: `projection_unknown_field`; there is no `members.name`, and a domain
may not add one (`displayName` is platform-maintained and must not be copied
into domain entities, because a copy drifts the moment somebody renames)

**GC089: write + behavioral.** **Non-enumerability (R-G10).** The four answers
below must be indistinguishable in pairs. `task_1` exists in proj_1; Dov holds
no membership anywhere; `task_404` and `note_404` do not exist.
```
renameTask
```
- `as: dov`, params `$id = task_1, $t = "x"` → **not_found**
- `as: dov`, params `$id = task_404, $t = "x"` → **not_found**, asserted
  **byte-identical** to the line above: same status, same code, same message,
  same (absent) headers. The harness compares the serialized envelopes, not
  just the class.
- `as: bram`, params `$id = task_1, $t = "x"` → **access_guard**, the third
  answer and the only informative one: Bram may read task_1 (pm_2), so telling
  him it exists discloses nothing he did not have.
- **Timing assertion.** The guard semi-join runs regardless of outcome, so the
  forbidden and absent invocations must be cost-close: the harness asserts the
  two are within the same band and that neither short-circuits ahead of the
  other. This is a conformance assertion and no observation left to luck: a
  runtime that answered absent rows faster would rebuild the oracle the ruling
  removed.

Read side, same rule:
```
op taskOne($id: id) = tasks(id == $id)[title]
```
- `as: dov`, params `$id = task_1` → 0 rows (a *filtered* read is empty, not an
  error; R-G10 governs **addressed** reads and writes; the empty result is the
  same shape a caller gets for a row that does not exist)

**GC090: write.** **The unguarded exception (R-G10).** `products` is a base
`market` entity with no `access` clause and no inherited `acl`, so there is no
read view to be outside of and `not_found` means only "absent".
```
op renameProduct($id: id, $n: text) = set products { id: $id, name: $n }
```
- `as: dov`, params `$id = prod_1, $n = "x"` → applied (no guard to fail)
- `as: dov`, params `$id = prod_404, $n = "x"` → **not_found**, and here the
  answer *is* informative: the entity is unguarded, so its row set is
  not a confidentiality boundary. Non-enumerability is a property of guarded
  data, and the suite asserts that distinction without claiming the blur
  everywhere.

**GC091: write + compile.** **acl bootstrap: the create-exemption (R-G11a)
and the same-group auto-grant.** `boards` is an acl root; its derived write
guard is `boardsAccess(member == $caller and level == "admin")`. Every path
from it to `$caller` runs through link rows that cannot exist before the board
does, so the guard is **create-exempt** and is not evaluated on the create.
```
op openBoard($n: text) = set boards { name: $n }
```
Each invocation starts from the pristine seed (§3.5), so the new board is
`board_3` in every one of them.
- `as: cleo` → **applied** (Cleo holds no acl row anywhere on `boards`; without
  G10a the derived guard would refuse the very create the same-group grant
  exists to make reachable). Readback `aclRoster` `as: cleo` gains
  (board_3, mem_3, **admin**). The creator receives the top level, minted in
  the **same** command group as the create, so there is no instant at which
  board_3 exists unreachable, and a group revert takes the grant with it. The
  harness asserts the group as well as the end state: both facts carry one group
  id.
- `as: dov` → **applied** likewise, with (board_3, mem_4, admin). Dov holds
  nothing anywhere; what bounds this create is the **door** (may he call
  `openBoard` at all) plus the op body, and no guard.

Compile side, the disclosure that makes the above reviewable:
- `boards`' compiled artifact carries `create: guard-exempt (self-rooted
  guard)`; the shape summary reads
  `acl (read, write, admin): creator receives admin at create`. Both are
  **derived and disclosed**, and neither is declared in config.
- `cards` carries no such mark: its guard is inherited from `boards` through
  the owned ref and is therefore **parent-rooted**, so it is evaluated
  normally on a create (GC041 stands unchanged: Cleo still cannot
  `addCard` into board_1 at read level).

**GC092: write.** **A guest creator gets no auto-grant.** Auto-grant follows
**space membership and not authorship** (spec §3 bootstrap rule 4): Gil holds a
members row with a null `role`, so he is a guest.
```
openBoard
```
- `as: guest:gil` → **applied** (the create-exemption is about the guard, not
  about who the caller is), **and zero access rows are minted** for board_3;
  readback `aclRoster` `as: guest:gil` → unchanged, 4 rows (the board_1 rows
  from GC046). Readback `boardList` `as: guest:gil` → 1 row: (roadmap),
  **not** the board he just made, which he can no longer reach at all.
- Without this rule a guest invited into one corner of a space could
  manufacture rows they exclusively administer and then extend access to
  principals of their choosing, inside a space they are a visitor in. The
  space owner administers the orphaned board (GC093).

**GC093: write.** **Space-owner acl administration is a door capability.**
Ora is the space owner and holds no membership row and no acl row anywhere, so
nothing she reaches comes from data.
```
boardList
```
- `as: ora` → 0 rows; the capability is *acl administration*, and on its own
  it hands her no data reach
```
grantBoardsAccess
```
- `as: ora`, params `$board = board_1, $member = mem_o, $level = "admin"` →
  **applied**, checked at the **door**, once per call, before planning, where
  the space is already bound. It is no extra disjunct inside `boards`' derived
  guard, because a per-row disjunct could deadlock on the very rows the capability
  exists to repair.
```
boardList
```
- `as: ora` → 1 row: (roadmap), after the grant and only after it

This is the disclosed property, and the suite asserts it: **no acl row is
private from the space owner**, because an owner who can administer acl rows
can grant themselves any level on any row. Two obligations ride with it, and
the suite asserts the first: the grant above is an ordinary attributed command
in the space's chain (`actor: mem_o`, visible in `callersOf`), so the owner
cannot look without leaving a record. The second (the consent page for joining
a space you do not own must say so plainly) is `bootstrap.md`'s to assert.

**GC094: write.** **`let x = set …` (R-G19): create and seed in one op.** The
create is guard-exempt; the dependent write is **not**.
```
op openBoardWith($n: text, $co: id) =
  let b = set boards { name: $n }
  set boardsAccess { board: b.id, member: $co, level: "write" }
```
- `as: cleo`, params `$n = "q6", $co = mem_4` → applied; **one atomic command
  group**, three facts in it: board_3 exists, Cleo holds `admin` on it (the
  auto-grant of GC091), Dov holds `write`. Readback `boardList` `as: dov` → 1
  row: (q6). Dov reaches this board and no other.
- The second statement is evaluated in the ordinary way and passes only because
  the auto-grant from the first statement is already in the group: statements
  in a command group execute in order. Ordering is not new machinery here:
  it is the ordering command groups already had.
- `b` is a **row descriptor**: `b.id`, `b.rev`, and `b.name` (the field this
  statement wrote) are in scope for later statements. `b` is **not** usable in
  the statement that produced it, nor in that statement's condition.

**GC095: reject.** `let` binds a create and only a create (R-G19).
```
op renameBoardBad($id: id, $n: text) =
  let b = set boards { id: $id, name: $n }
  set boardsAccess { board: b.id, member: $caller, level: "admin" }
```
→ reject: `unbindable_write`. The derived mode is CONVERGE, which addresses a row
whose id the caller already holds; there is nothing to learn. The same class is
raised for a CAS write (`id` + `rev`) and for a bulk write (an unknown number of
rows, so no single `.id` to name).

### M. Routines: the bound follows the trigger (GC096–GC098)

Fixture addition: `tasks` is source-fed in this suite's extension, so
`market.tasks.ingested` is a real event. The harness plays it with a **source
principal** (no members row), which is the point: the outsider chooses *when*
these routines wake and *on which rows*.

**GC096: write.** **Listener-woken: bounded to `$trigger.ids` (R-G20).**
```
routine seedTaskNotes in market {
  trigger when market.tasks.ingested
  run():
    tasks(id in $trigger.ids)[id, title]
    each -> set taskNotes { task: id, body: "ingested" }
}
```
Ingest one row (task_6, proj_1) as the source principal:
- → one note created, on task_6 only. Readback `noteFeed` `as: ana` gains
  (ingested, task_6).
- Now the probe: replace the routine body's source with `tasks[id, title]`
  (unfiltered, so the run header takes `run() unbounded:`) and re-run the same
  ingest. → **still one note, on task_6 only.** Every row outside
  `$trigger.ids` is invisible to the run exactly as a guard would make it. The
  routine cannot be steered into work the outsider who fired the trigger could
  never have done themselves. Note which marker does which job: `unbounded` is
  an acknowledgment about *work*, and it does **not** widen the listener bound.
  Only `unguarded` does (GC098).
- **Rows the routine creates get no auto-grant.** Repeat with an acl root:
  a routine-created `boards` row mints **zero** `boardsAccess` rows, because
  the attributed principal is `routine:seedTaskNotes@<version>` and has no
  members row. There is no human to grant to, and inventing one would be the
  platform declaring consent (spec §3 bootstrap rule 5).
- *The reason this bound exists is provenance.* The rationale
  that a routine "holds no members row, so every guard evaluates false" is
  **wrong** and no case may cite it: that describes R-G12's anonymous caller,
  and it would make every routine useless.

**GC097: write.** **Clock-woken: guards pass (R-G20).** Nobody outside chose
the moment, so there is no confused deputy to be.
```
routine nightlyStale in market {
  trigger every day 02:00Z
  run() unbounded:
    tasks(status == "open")[id]
    each -> set tasks { id: id, status: "stale" }
}
```
- Fire the clock → **all four** open tasks (task_1, task_3, task_4, task_5;
  task_2 is already `done`) are updated, across proj_1, proj_2 **and** proj_3
  (including task_5, which **no principal in the fixture may write**, since no
  proj_3 member holds `canWrite`, as GC029 asserts). That is the point: a
  clock-driven rule can never silently do nothing, and it is guard-exempt
  because its wake-up is the platform's own.

**GC098: write + compile.** **A listener that must range wider says so.**
```
routine reconcileAll in market {
  trigger when market.tasks.ingested
  run() unguarded unbounded:
    tasks(status == "open")[id]
    each -> set taskNotes { task: id, body: "swept" }
}
```
- Ingest task_6 → notes created on **every** open task, not only task_6. The
  `$trigger.ids` bound is lifted because one greppable word says a person
  decided to lift it.
- Compile: the routine's manifest carries `unguarded: true` alongside the
  bound it bypassed (`listener bound: $trigger.ids`), the same disclosure
  discipline `unguarded` ops get on the op surface (R-G8). **Two markers, two
  decisions**: `unguarded` is about authority, `unbounded` is about work, and
  neither implies the other. GC096's probe carries `unbounded` alone and stays
  bounded to `$trigger.ids`.

### N. The explicit-open spelling (GC099–GC100)

**GC099: compile + reject.** `access write open` is the explicit-open clause
(R-G21). The `project` ref is what makes the read walk reachable (R-G3, GC078):
```
entity announcements {
  project -> projects  owned
  body:    text
  access read   project.projectMembers(member == $caller)
  access write  open                  # deliberate: anyone who may call the op
}

op postAnnouncement($p: id, $b: text) =
  set announcements { project: $p, body: $b }
```
→ accepted. Assertions:
- the shape summary and every manifest inlining `announcements` render the word
  `open`. It is greppable by design, one word with one meaning, and the write
  guard is listed as `open` and not omitted;
- the entity is **not guard-bearing** on its write verb, so
```
share to guest:gil { postAnnouncement }
```
  → reject: `guest_exposure_unguarded` under R-G18. A keyword, where a
  literal would not do, is what makes that check undefeatable in one word: a
  guard-bearing test that accepted `access read <literal>` could be defeated
  silently, by the author most likely to be reaching for it.

**GC100: reject.** The withdrawn spelling.
```
entity announcements2 {
  project -> projects  owned
  body:    text
  access read   project.projectMembers(member == $caller)
  access write  true
}
```
→ reject: `parse_error`. `open` is a keyword and no expression: it is the
entire clause or nothing. R-G3 admits literals only as an `==` right-hand side
and requires a `$caller`-referencing disjunct, so a bare `true` was never a
legal walk. The rule that used to be written this way prescribed a form the
grammar rejects. The symmetric case (`access write` present, `access read`
absent) is the same error and the same fix: `access read open`.

Partial-open is also unspellable, and that is deliberate:
```
entity announcements3 {
  project -> projects  owned
  body:    text
  access read   project.projectMembers(member == $caller) or open
  access write  open
}
```
→ reject: `parse_error`. `open` cannot appear inside a walk: not as an
operand, not as an `==` right-hand side, nowhere.

### O. The delegated caller (GC101)

**GC101: exec + write.** **An agent acting for Ana is Ana to the guards
(R-G22).** Ana (`mem_1`) delegates to the agent `agt_ellanor`, which holds
no seat; the agent's token lists `myProjects`, `taskBoard` and `renameTask`.
```
myProjects
taskBoard
renameTask($task: "task_1", $title: "by the agent")
renameTask($task: "task_5", $title: "not allowed")
```
- `as: agt_ellanor for mem_1` → `myProjects` and `taskBoard` return **exactly
  Ana's rows**, the same tuples as `as: mem_1`.
- `as: agt_ellanor for mem_2` → Bram's rows, which differ from Ana's.
- `as: agt_ellanor` (no delegation) → empty: no seat, no rows (R-G12's shape).
- `renameTask` on task_1 (proj_1, Ana holds `canWrite`) → applied, recorded
  as `agent:agt_ellanor on behalf of user:mem_1`.
- `renameTask` on task_5 (proj_3, Ana is a member without `canWrite`) →
  `access_guard`, Ana's own answer.

Runs in `ordo-host` (`tests/delegation.rs`), because the delegation is the
door's and this crate has none.

---

## 6. Coverage map

101 cases. Components: (a) guard grammar · (b) read filtering · (c) write
guards · (d) inheritance/override · (e) unguarded · (f) acl derivation ·
(g) composition/refinement · (h) guests · (i) `$caller` lifecycle ·
(j) metadata/cost · (k) behavioral · (l) routines.

| Component | Cases |
|---|---|
| (a) guard grammar | GC069, GC074–GC079, GC088, GC099, GC100 |
| (b) read filtering | GC001–GC010, GC037, GC038, GC046, GC059, GC060, GC088, GC089 |
| (c) write guards | GC011–GC022, GC024–GC026, GC029, GC039–GC045, GC047, GC061, GC062, GC070, GC089–GC095 |
| (d) inheritance/override | GC023–GC029, GC038, GC041, GC048, GC082, GC091 |
| (e) unguarded | GC030–GC035, GC064, GC090, GC098, GC099 |
| (f) acl derivation | GC036–GC048, GC083, GC086, GC091–GC094 |
| (g) composition/refinement | GC049–GC058 |
| (h) guests | GC040, GC059–GC066, GC087, GC092 |
| (i) `$caller` lifecycle | GC006, GC016, GC067–GC073, GC101 |
| (j) metadata/cost | GC007, GC027, GC031, GC035, GC048, GC052, GC057, GC073, GC080–GC083, GC091, GC098, GC099 |
| (k) behavioral | GC067, GC068, GC084–GC087, GC089 |
| (l) routines | GC096–GC098 |

Feature-to-case index: per-caller divergence on one op GC001–GC004, GC023,
GC037, GC038, GC049 · empty-view-not-error GC001, GC002, GC008–GC010, GC050,
GC060 · guard + condition conjunction GC005, GC006, GC050, GC071 ·
atomic-group abort GC017 · bulk scoping GC019, GC020 · owner/member guard
divergence GC003, GC004, GC021, GC028, GC029 · depth-2 inheritance GC023–
GC027, GC082 · override-replaces GC028, GC029 · bypass family GC030–GC035 ·
level ordering GC037–GC040 · admin gating GC042–GC045, GC047, GC062 ·
no-laundering GC006, GC033, GC049, GC051, GC058 · guest window GC059–GC066,
GC087, GC092 · `$caller` placement rules GC067–GC070 · anonymous GC016,
GC030, GC072 · sealed × guards GC074, GC075 · live invalidation GC084–GC087 ·
**non-enumerability** (`not_found` for forbidden ≡ absent, R-G10) GC013,
GC017, GC026, GC029, GC072, GC089, GC090 · **acl bootstrap** (G10a
create-exemption, same-group auto-grant, membership-not-authorship, door-level
owner capability) GC091–GC093 · **`let x = set …`** GC094, GC095 ·
**trigger provenance** GC096–GC098 · **platform `members`** GC088 ·
**explicit-open** GC099, GC100.

### 6.1 What changed in this revision, and why

Three rulings landed in the specs after this suite was first written, and each
one had at least one case asserting its opposite. They are corrected in place
and never deleted, because a corpus that quietly loses coverage is worse than one
that is wrong loudly.

| Was | Is | Cases |
|---|---|---|
| addressed write outside the read view is `access_guard`; "the suite does not require the runtime to blur the two" | `not_found`, byte-identical to absent, for **guarded** entities (R-G10; `api-engine-design.md` §4.1 resolves in favour of non-enumerability and names this corpus as the losing side) | GC013, GC017, GC026, GC029, GC072 re-expected; GC089, GC090 added |
| R-G11 alone: creating writes always evaluate the write guard | R-G11 **plus R-G11a**: a self-rooted guard is not evaluated on a create; the auto-grant follows space membership; owner override is a door capability | GC091–GC093 added; GC015, GC016, GC041 confirmed unchanged (parent-rooted guards are still evaluated) |
| `let x = set …` unsettled; create-and-seed unspellable | settled: `let` binds CREATE mode only, yielding a row descriptor (R-G19) | GC094, GC095 added |

Plus three sweeps: the fixture no longer declares `entity members` (it is
platform-provided and undeclarable, GC088); routines gained the trigger-
provenance bound they never had a case for (R-G20, GC096–GC098); and the
explicit-open spelling is pinned to `access write open` with the withdrawn
`access write true` asserted as a `parse_error` (R-G21, GC099, GC100).

### 6.2 Corpus texts that did not compile as written (2026-08-30, 10g)

Two families were **stale rather than wrong**: their expected classes stand,
but their source carried a second, unrelated defect that a gate answered
first, so the case never reached what it was written to assert. Both are
corrected in place; neither expected outcome moved. Ruled and reasoned in
[`corpus-rulings.md`](corpus-rulings.md).

| Was | Is | Cases |
|---|---|---|
| the composing op omits `unbounded` while its callee (`openTasks`, `allTasks`) is declared `unbounded` in §3.4, so `unacknowledged_unbounded` is raised at the registration gate, before the bypass gate is reached | the acknowledgment is on the caller where composition-R9 puts it; the bypass expectations (`unguarded_superfluous`, `unacknowledged_unguarded`, and GC035's accepted compile) are unchanged | GC033, **GC034, GC035** re-spelled |
| the entity declares `access read` alone, so `incomplete_access_clause` (spec G9) is raised before the walk is examined | both verbs, the write one `open`, which is R-G21's own prescribed repair and adds nothing to what the case asserts | GC074–GC078 re-spelled |

GC034 and GC035 carried GC033's defect and had never been written down; the
suite had quietly supplied the missing marker for them. GC079 needs no repair
because `access admin` fails at the verb, before completeness is asked.

