# The deployment operational security log

**Roadmap 11-22. A design for the third artifact of `security-logging-design.md` §6.1's three-artifact split, and the increments that build it.**

Companion documents: `security-logging-design.md` §6.1 (`[settled]`, the per-space stream, which this artifact borrows almost all of its machinery from), `bootstrap.md` §8 (the deployment control-plane audit stream, `[proposed]`), `dashboard-plan.md` §3 and §5b (the surfaces), `implementation.md` Observability (`[proposed]`, the population inventory), `corpus-rulings.md` 2026-09-07 (own artifact by default, with a toggle) and 2026-09-09 (the unverified restore goes to the ops log).

Two things are ruled and are not re-opened here: the artifact ships as its own hash-chained stream by default with a deployment-config toggle to turn it off, and §6.1's rules for the per-space stream stand. Where those rules carry over, §1 says so and says why. Where this artifact has to differ, §1 argues the difference rather than inheriting it silently.

---

> **Topology note, 2026-09-12.** The owner has since ruled one tenant,
> one key, with a database per tenant reached by per-space routing
> (checklist 18c/18d, planned in `plan-space-routing.md`). Two things in
> this document are affected and are marked where they appear: the
> deployment artifact stays in the **control** database and cannot move
> into a tenant's, since it holds what belongs to no space and must be
> writable when a caller names a space that does not exist; and
> `$secevents` becomes two physical tables in two databases, which makes
> the read-versus-prune custody split cleaner rather than harder, because
> the two streams can then have different signers.
>
> **Built, 2026-09-14** (`plan-space-routing.md` S4, S6): the deployment
> stream is written to the control database without consulting the
> resolver, each space's stream to the space's own database, and
> retention walks every database (`secdrain::retain_everywhere`). Both
> streams are signed by the one deployment signer today; giving them
> different signers is possible and is not built.
>
> **Also built, 7d 2026-09-26:** the **checkpoint** sweep walks the same
> list through the same helper (`secdrain::checkpoint_everywhere`), on its
> own hourly schedule row rather than retention's. A database the
> checkpoint pass misses has streams nothing signs, so `ordo verify` on any
> of its tenants reports a green walk it cannot back with an end-of-stream
> check — which is a silent weakness in the answer rather than in the
> storage, and is why the two sweeps share one definition of where the
> streams live.

## 0 · The starting state, verified

Every claim below was read in the file named. The headline finding is that the checklist row and `bootstrap.md` §8 are both stale: this is not a whole missing subsystem, it is a built subsystem with three specific holes, one of which destroys the property the artifact exists for.

### 0.1 The deployment stream exists, is routed to, is stored, checkpointed and retained [settled]

`Scope` has two variants and the deployment one is not a stub (`crates/ordo-secevent/src/event.rs:36-51`). Its storage key is the reserved `$deployment` (`event.rs:51`), chosen because space ids are `spc_…` so no caller can name it and no space can collide with it.

The routing rule that decides between the two is `route` (`crates/ordo-secevent/src/recorder.rs:192-200`), and it is §6.1's members-only rule exactly: an unverified subject always goes to the deployment, a subject with a relationship to the bound space goes to that space, everything else goes to the deployment.

The door already calls it. An unverifiable token records a deployment-scope denial with `Subject::Unverified` and `Relationship::None` (`crates/ordo-host/src/lib.rs:1236-1250`); a verified principal's denial routes on the relationship the engine just computed (`lib.rs:1262-1276`, with `relationship` at `recorder.rs:144-156`); a not-live delegation records at `lib.rs:1192-1204`.

Durable storage is scope-keyed, not space-keyed: `$secevents` has a `scope text` column carrying either a space id or `DEPLOYMENT_KEY` (`crates/ordo-exec/src/secevent.rs:83-108`), `try_scopes` maps the reserved key back to `Scope::Deployment` (`secevent.rs:530-546`), and the retention pass walks the deployment scope exactly like a space's (`retain_scope`, `secevent.rs:627`, driven from `crates/ordo-host/src/secdrain.rs:426-473`). Checkpoints are signed per scope under the security key (`crates/ordo-secevent/src/store.rs:399-416`), the deployment stream is its own chain from its own genesis (asserted in `crates/ordo-secevent/src/tests.rs:206-226`), and a durable end-to-end test appends to both and verifies both independently (`crates/ordo-exec/tests/secevent.rs:522-573`).

So the machinery question the item asks ("its own artifact or the app log") was answered in code before it was answered by ruling, and the crate says so out loud: "deployment-scope events go into the same chained structure as space-scope ones, because the machinery was already built and an unchained operator log is the one an attacker can edit" (`crates/ordo-secevent/src/lib.rs:106-113`). The 2026-09-07 ruling matches what is already there. What is missing is the toggle, the target space, the collapse key, and every reader.

### 0.2 The deployment stream never records which space was probed [open]

This is the defect that matters. `Observation` carries the bound space (`recorder.rs:177-189`), `route` consumes it to pick a scope, and then `observe` drops it: for a denial it builds a `CollapseKey { scope, subject, op }` (`recorder.rs:328-341`, `collapse.rs:38-44`), and for everything else a `StreamEntry::single(scope, subject, op, kind, at, correlation)` (`recorder.rs:342-349`). `StreamEntry` has no field for it (`event.rs:562-580`), and the canonical encoding has no place to put one (`crates/ordo-secevent/src/chain.rs:135-150`).

The consequence is exact. A stranger probing a hundred spaces produces deployment entries that say a principal was denied on op `X`, one hundred times, and say nothing at all about which hundred spaces. §6.1's stated entry contents are "space, principal, principal kind, attempted op, the decision, the instant", and for this artifact the first field is absent. The one question the artifact exists to answer, *which spaces did this principal sweep*, is not answerable from it today.

### 0.3 The collapse key merges a whole sweep into one entry [open]

Worse than 0.2 and caused by it. §6.1 pins the key as `(space, principal, op)` and gives the reason: "a principal firing one op at ten thousand spaces produces ten thousand keys and cannot merge them, which was the second defect in the `(principal, op)` version". `collapse.rs:12-18` restates this and believes it has implemented it, with `scope` "standing in for space so the deployment stream collapses by the same rule".

For space scope that is true, because `Scope::Space(id)` carries the space. For deployment scope it is false: every entry in the deployment stream has the same `scope` value, so the key degenerates to `(principal, op)`, which is precisely the rejected version. A ten-thousand-space sweep at one op is one open window, one closed entry, and `count: 10000`. The artifact whose reason for existing is that a sweep is visible nowhere else records the sweep as a single line that could equally be ten thousand retries against one space.

### 0.4 Key releases lose their space too, and `implementation.md` says they carry it [open]

`MeteredKeys::release` records a deployment-scope `KeyReleased { purpose }` with `space: None` (`crates/ordo-host/src/metrics.rs:893-912`, the `None` at `:902`). The comment argues correctly that the *scope* is the deployment, since a space owner cannot act on the platform's custody decisions. It then drops the space from the record as well, which is a separate act and is not argued.

`implementation.md:488-492` specifies the deployment security log as carrying "key releases (space, purpose, correlation)". The code contradicts the document, and the same missing field causes it. The metric keeps the space (`ordo_key_release_total{space,purpose}`), so the fact survives in the substrate that has no integrity guarantee and is lost from the one that does.

### 0.5 Nothing reads either stream, and `verify` covers only the command chain [settled]

`ordo-secevent`'s own module doc says it: "No wire surface. A space owner reading their stream, `verify()` over both artifacts from one command, and the dashboard's explorer/burst view are all reads over this crate's types and none of them is built" (`lib.rs:115-120`).

Confirmed at the edges. `ordo verify --space` chooses an anchor, verifies the command chain, and touches `$secevents` nowhere (`crates/ordo-cli/src/operator.rs:278-354`). The only production readers of `PgSecurityStore::try_entries` are the retention pass and verification itself (`crates/ordo-exec/src/secevent.rs:688`, `:724`, `:750`); every other caller is a test. `IdentityAccess::take_audit` and `take_events` (`crates/ordo-host/src/lib.rs:1077-1097`) are destructive drains documented as unsafe to use beside a real drainer, and nothing in `src/` calls them.

This is why §6.1's retention floor is allowed to be simple, and `store.rs:60-95` spells out the three bounds that have no counterpart here (registered consumers, undo horizon, `x-ordo-verifiable-from`) with the reason for each. The floor is "age, anchored, with a tail", and `verifiable_from` is computed and deliberately unpublished because there is nobody to publish it to (`store.rs:494-504`).

### 0.6 `/security/alerts` is mounted and its op is deployed nowhere [open]

The route exists (`crates/ordo-host/src/serve.rs:153`), its handler authorizes against the reserved op name `platform.security.alerts` (`serve.rs:67-75`, `:380-405`), and that op name is never passed to `deploy_op` and never granted to any role: the fixture builder deploys and grants exactly `platform.health.deep` and `platform.metrics` (`crates/ordo-host/src/lib.rs:1421-1438`). An unregistered op name denies at the door, so `/security/alerts` answers 403 to everyone in every deployment built through that path. There is no test over the route in `crates/ordo-host/tests/`.

The mandatory deny-volume alert, which `implementation.md` says a deployment that silences is misconfigured, is therefore reachable by nobody through HTTP today.

### 0.7 The platform surfaces authorize against a caller-named space [open]

`authorize` resolves the space with `space_of`, which reads `x-ordo-space`, then `$space`, then falls back to the deployment's default space (`serve.rs:519-547`). The access check then runs against whatever the caller named.

With the built-in profile, `platform.metrics` is granted to `Role::Owner` (`lib.rs:1421-1423`). In a deployment with more than one space built through that builder, the owner of any space can name their own space and receive a deployment-wide disclosure that carries principal ids, op names and spaces across every tenant this process served (`metrics.rs:107-112` states exactly what the labels carry). I did not find a deployment in-tree that provisions more than one space through this builder, so I state the property of the code rather than a confirmed exploit. It is directly load-bearing for §2: a deployment-wide reader must not resolve its authorization space from the caller.

### 0.8 There is no toggle, and no deployment-config seam to hang one on [settled]

The workspace has no configuration file and no settings struct. Deployment configuration is environment variables read at the point of use, and the complete inventory of `std::env::var` call sites outside tests is `ORDO_CHECKPOINT_KEY`, `ORDO_CHECKPOINT_KEY_ID`, `ORDO_CHECKPOINT_PUBKEY`, `ORDO_EGRESS_DENY_CIDRS`, `ORDO_KEY_ABBREVIATIONS`, `ORDO_MASTER_KEY`, `ORDO_METRICS_PER_SPACE`, plus `ORDO_SECURITY_CHECKPOINT_KEY` / `_KEY_ID` (`store.rs:114-116`) and `ORDO_DENY_ALERT_THRESHOLD` / `_WINDOW_SECS` (`alert.rs:88-91`).

Two existing precedents matter for the toggle's shape. `ORDO_METRICS_PER_SPACE` is off by default with the argument written next to it: "an alert a missing variable silences is not a control, while a label a missing variable adds is a cardinality bill nobody agreed to" (`metrics.rs:274-283`). And `DrainConfig::retain` is a boolean whose *off* state is reported in prose on `/health/deep` rather than by an absent field (`secdrain.rs:210-284`, rendered at `serve.rs:307-338`, with `wired: false` argued at `serve.rs:52-63` as a first-class answer).

### 0.9 There is no ops log [open]

`grep` finds "ops log" and "operational log" in six documents and in four Rust comments that describe a place to write things. There is no logging framework anywhere in the workspace (no `tracing`, no `log`, in any `Cargo.toml`), and no `eprintln!` or `println!` in `crates/ordo-exec/src` or `crates/ordo-host/src`. `implementation.md:480` marks structured logs `[proposed]`.

This is the material fact behind §3. The 2026-09-09 ruling sent the unverified-restore record to the ops log, and the ops log is a name rather than a facility. C6 of `plan-blob-chain.md` is in flight and its `Done when` includes `an_image_without_a_digest_restores_without_an_event_and_logs_the_skip`, which cannot pass against a facility that does not exist without inventing one.

### 0.10 Both encodings are frozen, and both have a lawful way to grow [settled]

The security stream's layout is frozen and unversioned; the one lawful change is appending a new `EventKind` tag, and the cost is stated: a newer entry read by an older binary is `BadTag`, so a tag addition is a forward step the whole fleet takes before the first entry using it is written (`chain.rs:42-54`). Tag 13, `EventsDropped`, is the first one added this way.

The command chain has the same freeze and a second technique the security stream does not yet use: an **optional trailer**, encoded by presence rather than by value, so an unflagged group renders byte-identical bytes to what it rendered before the field existed (`crates/ordo-commandlog/src/chain.rs:40-71`, encoder at `:221-240`, decoder at `:355-372`). The proof is a golden hash and a golden length computed *before* the field was added and pinned as constants (`crates/ordo-commandlog/src/tests.rs:637-690`). Unknown trailer tags are refused rather than skipped, and the doc anticipates a second tag: "A future flag would take a new tag byte".

The one existing trailer is `CommandGroup::owner_administration` (`crates/ordo-commandlog/src/command.rs:157-183`), a group-level audit flag that "exists for exactly one reader, an auditor, and for exactly one question", is hashed into the chain, and is explicitly not an authorization input.

### 0.11 The crate graph is not touched by any of this [settled]

`ordo-wasm` compiles `ordo-conformance` for `wasm32-unknown-unknown` (`crates/ordo-wasm/Cargo.toml`, the `ordo-conformance` dependency with its comment). `ordo-conformance`'s dependencies are `ordo-syntax`, `ordo-schema`, `ordo-sema`, `ordo-plan`, `ordo-commandlog`, `ordo-crypto`, `serde_json`, `unicode-normalization`. It does not depend on `ordo-secevent`, `ordo-exec` or `ordo-host`, and `ordo-secevent` itself depends only on `ordo-commandlog`, `ordo-identity` and `sha2`, with no tokio at all.

Two consequences for this plan. Everything in §1 and §2 lands in `ordo-secevent`, `ordo-exec`, `ordo-host` and `ordo-cli`, none of which is reachable from the wasm build. And if §3's recommendation is taken, the change lands in `ordo-commandlog`, which *is* compiled for wasm through `ordo-conformance`: a trailer byte adds no dependency, so the constraint is satisfied, and `scripts/build-wasm.sh` is the gate that says so (the same gate C2 of `plan-blob-chain.md` ran).

---

## 1 · What carries over from §6.1, and what must differ

### 1.1 Carried over unchanged, with the reason each survives the scope change [settled]

- **Its own hash chain, its own head, its own checkpoints, its own key.** All three arguments in `store.rs:4-34` are about the *record* rather than about the space: lifecycles differ, blast radius differs, audiences differ. They apply with more force here, because the audience for this artifact is the deployment's security function alone.
- **Appends off the request path, through the drainer.** `recorder.rs:49-57`. A deployment-scope event is by definition produced by a caller with no authority anywhere, which is exactly the caller who must not be able to make the platform do durable work synchronously.
- **The bounded outbox and the `EventsDropped` marker.** `recorder.rs:59-103`. Unchanged, and the "oldest entries survive, arrivals are refused" rule is more important here: the onset of a sweep is the evidence.
- **Retention that cuts only at a verified checkpoint, never an interior entry, never the last entry.** `store.rs:36-58` and `StreamError::RetainedTail` at `store.rs:156-176`. The tail rule is the one that must not be relaxed for the deployment scope, and the reason is mechanical rather than aesthetic: the append path reads the last stored row to get `(idx, hash)` and starts from `ChainHash::GENESIS` when there is none, so an emptied stream makes the next denial fork a second chain over the freed indices and orphans every checkpoint. On a deployment stream the freed indices are the record of every stranger who ever probed the deployment, and the fork would be invisible to the only party who could notice.
- **The decision, never the wire code.** `event.rs:207-272`. Unchanged.
- **Attribution taxonomy is §7's**, and `Subject::Unverified` carries nothing: no claimed subject, no token fingerprint (`event.rs:107-136`). This is *the* rule for this artifact, because most of its subjects are unverified. A claimed name would let a stranger make the deployment's own record say a name.

### 1.2 Must differ: the entry names the space it was about [proposed]

§6.1 says an entry carries the space. In the per-space stream the scope *is* the space, so the field is implied. In the deployment stream the scope is the deployment, and the space the caller named is a different thing: it is the **object** of the event, not its home.

So `StreamEntry` gains `about: Option<SpaceId>`, with a stated invariant: `Some` only when `scope` is `Scope::Deployment`. Duplicating the space into a space-scope entry would create two fields that can disagree, and a record whose two spellings of one fact can disagree is a record that will eventually be read the wrong way.

**The value is caller-supplied text and is admitted the way an op name is.** `sanitize_op` exists because `/op/{name}` takes any string and a denied caller must not write chosen bytes into a record others read (`event.rs:518-546`). The space is exactly the same class of input: `bind_space` bounds it at the HTTP door (non-empty, at most 128 bytes, `[A-Za-z0-9_-]`, `crates/ordo-api/src/router.rs:673-687`), and there is no space-id validator anywhere in `ordo-identity` or `ordo-secevent` for the leaf crate to lean on. The leaf crate does not trust its callers with the op name and must not start trusting them with the space name, so `sanitize_space` is written beside `sanitize_op`, all-or-nothing, with the same argument against partial sanitizing: a stripped string looks like the caller's input, so a reader trusts it, and the stripping is what lets a hostile name masquerade as a legitimate one.

**This is not the "attempted arguments" rule being relaxed.** §6.1 forbids the arguments because params may carry the very values the caller was refused, and because a stranger must not choose plaintext that a space owner reads. The space id is envelope-tier by the same taxonomy `implementation.md:476-479` applies to telemetry: it is the addressing of the request, it is already in the metric labels, and it is the only field that makes the artifact answer its question. What stays forbidden is everything inside `$refine`, `$page`, the body, and the bearer.

**Encoded as a trailer, so no stored entry moves.** The security stream's frozen layout ends with the correlation list. `about` is appended as `0x01 ‖ str space`, present only when set, absent otherwise, exactly the technique `ordo-commandlog` uses for `owner_administration` (`chain.rs:40-71` there). An entry with no target renders the bytes it rendered before the field existed and hashes where it hashed, so every stored chain and every signed checkpoint over it survives. Canonicity survives too, on the same argument: silence is the one spelling of "no target", so an explicit empty trailer is `TrailingBytes`. The forward cost is the one the crate already accepts for a new tag: a newer entry read by an older binary refuses, which for an audit spine is the correct failure.

The alternative considered and rejected: a second `EventKind` tag, `DeniedAt { about, decisions }`, alongside `Denied`. It is lawful under the existing rule and it is worse. The target applies to key releases and custody actions as well as denials, so one tag would not cover it; and it would make every reader branch on which of two variants it is holding, which is the shape `event.rs:556-561` argues against when it refuses a separate "summary" type.

### 1.3 Must differ: the collapse key, without letting the target become a cap dimension [proposed]

`CollapseKey` gains `about: Option<SpaceId>`, so the deployment stream's key is `(deployment, target space, principal, op)`, which is §6.1's `(space, principal, op)` with the deployment's own name in front. A hundred-space sweep becomes a hundred keys and a hundred entries, which is the whole point of the artifact.

**The per-window cap stays keyed on `Scope` alone** (`collapse.rs:86` `open_per_scope`, checked at `:140-144`). This is the load-bearing detail and it is the opposite of the change above. If the cap counted per target, a sweep across ten thousand caller-chosen space ids would buy ten thousand independent 256-entry budgets and the attacker would size the record. Keeping the cap on the scope means one sweep produces at most `DEFAULT_CAP` distinct entries per window plus one `Suppressed { further }` marker, so the record degrades before it grows, exactly as §6.1 requires, while the shape of the degradation still says "many distinct targets, and more were suppressed".

The consequence is worth stating plainly rather than discovering: under a sweep the deployment stream grows up to 256 entries per window where it previously grew one. That is a deliberate 256-fold increase in the volume of the artifact under attack, bounded, declared, and cut by retention at the next checkpoint.

### 1.4 Must differ: the retention *number*, and only the number [proposed]

`Retention` is a single `keep_secs` applied to every scope by the retention pass (`secdrain.rs:439-473` walks `try_scopes` and passes `l.cfg.retention` to each). The deployment scope is retained under the same policy as every tenant's stream.

They should be two numbers. A tenant's denial stream is evidence about that tenant's members and its default of ninety days is argued from incident-discovery lag (`store.rs:428-435`). The deployment stream is the deployment's abuse record, it is the only artifact where a slow sweep is legible, and slow sweeps are slow on purpose. Recommend `Retention` gains a second value used for `Scope::Deployment`, defaulting to the same ninety days so nothing changes on the day it lands, with its own environment variable. Everything else about retention stays: cut only at a verified checkpoint, whole prefixes, never the last entry.

### 1.5 Unchanged, and worth restating because it is the item's whole premise

A deployment-scope entry never touches a space's stream and never takes the command chain's append lock (`SECEVENT_LOCK_NS` at `crates/ordo-exec/src/secevent.rs:118`, asserted behaviourally in `crates/ordo-exec/tests/secevent.rs`). Nothing in this plan puts a stranger's byte into a tenant's record.

---

## 2 · Who may read it [proposed]

This is the item's second question. The artifact names principals across tenancy boundaries: with §1.2 it names, in one place, which principal probed which tenant's space and when. It is the most sensitive artifact in the system and it is the only one whose natural readers are also the people who can prune it.

### 2.1 The candidates, with their consequences

**R0 · Nobody, which is today.** The record accumulates; whoever holds `psql` reads it by hand; the sweep is recorded and observed by no one. §6.1's floor stays simple by construction. This is the status quo and it fails the item: `dashboard-plan.md` §5b already promises that the pane "links across" to where sweeps live, and there is nowhere to link.

**R1 · The offline operator, through the CLI, cursorless.** Authorization is possession of the database credentials plus the security checkpoint public key, which is the same authority that already runs `ordo prune` and `ordo checkpoint`. No HTTP surface, no new op, no new authorization concept, and no reader that holds a position.

**R2 · An operator HTTP surface**, a fourth reserved platform op beside `platform.health.deep`, `platform.metrics` and `platform.security.alerts`. Feeds the dashboard. Requires fixing 0.7 first, because a deployment-wide artifact authorized against a caller-named space is a cross-tenant disclosure with a header for a key.

**R3 · Operator plus space owners, each filtered to entries about their own space.** Superficially attractive: the owner of a probed space learns they were probed. Three consequences. It names a **third party** to a tenant: the principal in the entry is a stranger to that space and may be another tenant's user, so the filter discloses one tenant's principal ids to another. It reintroduces the attacker's choice of victim one level up: the stranger picks whose filtered view fills with entries, which is denial of attention rather than denial of storage, but it is the same shape §6.1's members-only rule exists to remove. And it makes every filter bug a tenancy breach in the one artifact where a breach is worst.

**R4 · A distinct security role**, able to read the artifact and unable to prune or configure it, separated from the operator. This is the right long-run shape (the reader and the pruner should not be one grant), and there is no role concept to build it on today: the access model resolves roles per space, and the 2026-09-07 ruling closed the deployment-scoped op set at four kinds with "anything new must argue its way in".

**R5 · The subject reading entries about themselves.** Named only to refuse it. It hands a prober a live oracle on what the platform saw and when, which tells them exactly how to pace the next sweep.

### 2.2 The recommendation

**R1 now, R2 next behind an owner ruling, R3 never as a raw feed, R4 as the shape R2's grant should grow into, R5 refused.**

The argument for starting offline is not timidity. R1 needs no new authorization concept, so it does not have to argue a fifth deployment-scoped op class past the 2026-09-07 ruling; it cannot be reached by a token at all, so no capability grant, no delegated agent and no stolen session opens it; and it is available to the only party who can act on what it says. The artifact's consumer today is an operator investigating an incident, and an operator investigating an incident has a terminal.

**What the space owner gets instead of R3.** The owner's real question is "is anyone attacking me", and the answer to it does not require naming anyone. A derived count, delivered as a notification or rendered in the platform-health pane ("N failed access attempts from principals outside this space in the last 24 hours"), answers the question, names no third party, needs no filtered projection of a cross-tenant artifact, and cannot leak by a bug in a predicate. `dashboard-plan.md` §5b's sentence that "the two surfaces answer different questions and the pane links across rather than pretending to be complete" is honoured by a count and a link, not by a feed.

### 2.3 Two rules that ride with any reader, and one that is §6.1's own

**Every read of this artifact is an entry in this artifact.** A new `EventKind::StreamRead { entries: u64 }` at deployment scope, subject the operator who read, appended through the recorder like everything else so the read path still does no synchronous append. The reason is the one property that distinguishes this artifact from the other two: its readers are its pruners. "Who looked at the cross-tenant probe record, and when" must be answerable, and the only record that can answer it is the record itself. The cost is one entry per read and no recursion, because the entry is produced after the read completes.

**The reader is cursorless, and that is a design constraint rather than an omission.** §6.1: "The command log's floor also bounds on registered consumers, the undo horizon and a published `x-ordo-verifiable-from`; none of the three has a counterpart here, because nothing reads this stream from a cursor... If a reader surface is ever built, the floor gains those bounds and the disclosure with them."

Read exactly, the trigger is a reader that **holds a position**, not a reader that exists. R1 and R2 as specified page by index and time and register nothing, so no prune can overtake anyone and the floor stays "age, anchored, with a tail". The moment anything registers a cursor over this stream (a SIEM forwarder, a scheduled export, a mirror), §6.1's sentence bites in full: `$logconsumers` gains a consumer kind for it, the floor takes the consumer bound, and `verifiable_from` (`store.rs:494-504`, computed and unpublished today) must be published to that consumer. The plan forbids a cursor-registering reader until that work is done, and names it as the trigger so it is not discovered by a prune that ate a forwarder's backlog.

**A reader never renders arguments, because there are none.** The stream carries no arguments by construction, which is what makes a reader over it safe to build at all. `dashboard-plan.md` §3 already says this of the per-space explorer ("sealed values never appear because the stream never carries arguments") and it carries over verbatim.

---

## 3 · What lands in it [proposed]

### 3.1 The population is already broader than the checklist row says, and the code proves it

The checklist describes 11-22 as where "non-member, anonymous and unrelated-space denials land". That sentence describes §6.1's **routing rule**, which is a rule about what may touch a *tenant's* record. It was never a definition of this artifact's contents.

`implementation.md:488-492` gives the contents, and they are wider: "denials from non-members and anonymous callers, plus step-up demands and outcomes, key releases (space, purpose, correlation), break-glass activation, grant mint/expiry, authenticator link/unlink, and first-login provisioning". Only the first is a denial. None of the rest is evidence of wrongdoing.

The code agrees with `implementation.md` and not with the checklist: `MeteredKeys::release` writes a `KeyReleased` to deployment scope on every successful lease (`metrics.rs:893-912`), and the `EventKind` vocabulary already contains `Authenticated`, `StepUpSatisfied`, `MembershipReactivated`, `CustodyAction`, `AuthenticatorLinked` and `PrincipalOnboarded` (`event.rs:376-446`), only one of which is a refusal.

So the first half of the answer is that **11-22 does not need to be broadened to admit non-denials, because it was never that narrow**. What it needs is a written admission rule, since it currently has none and is accumulating members by precedent.

### 3.2 The admission rule, and the limb this plan refuses to write

An entry belongs in the deployment artifact when it is a security-relevant fact and at least one of:

- **A · It has no per-space home**, under §6.1's routing rule: denials of non-members, of anonymous callers, of tokens naming a space they have no relationship with, and the record's own markers (`Suppressed`, `EventsDropped`).
- **B · Its subject is the platform itself**, acting on or about a space that is the object rather than the author of the act: key release, deployment-performed custody actions, provisioning, and (when built) break-glass activation. The space appears in `about` per §1.2; the subject is `Subject::platform(...)`, which the type system already distinguishes (`event.rs:178-184`).

The proposed third limb, *"or it is not evidence of wrongdoing"*, is refused, and the reason is that it has no closure. Every log line in the system is not evidence of wrongdoing. An admission test that admits on the absence of a property admits everything, and what it admits is charged the artifact's full price: a chained append, a scope lock, a checkpoint cadence, a retention policy, and (per §2.3) a reader whose every access is itself recorded. Turning the deployment artifact into the deployment's hash-chained stdout would make the operator stop reading it, which costs the sweep its only reader.

Test B does the work the third limb was reaching for, and it does it with a boundary: the platform is the subject, the space is the object, and the authoring principal is either absent or is the platform.

### 3.3 The unverified restore does not pass either test, and here is where it should go

C6's record is: a revert restored a blob handle without verifying it, because the image predates the digest and there was nothing to compare (`plan-blob-chain.md` §4.3, the fourth outcome, and §4.4's final paragraphs). Its properties: it is performed by a **member**, it has a **space**, it writes a **command group** (revert applies the inverse through the normal gate as new history), and it is not a refusal.

Under test A it fails: it has a per-space home, and §6.1's routing would send it to the space's stream. Under test B it fails: the subject is a member, and the space is where the act happened rather than what it was done to. Admitting it means writing the limb §3.2 refuses.

The four candidate homes, in order of how much they cost:

1. **The ops log, as ruled 2026-09-09.** Blocked by 0.9: there is no ops log, no logging framework, and `implementation.md`'s structured logs are `[proposed]`. Honouring the ruling literally means C6 either builds a logging subsystem (much the largest of these four options) or writes an `eprintln!` that is unstructured, unretained, unqueryable and findable by nobody, which is a worse outcome than not writing the record.
2. **The space's security-event stream**, as `CustodyAction { action: "blob_restored_unverified" }`. Refused by the owner on noise grounds. Not re-opened.
3. **This artifact**, broadened. Costs the third limb; puts a member's ordinary act on the operator's desk and hides it from the space whose data it concerns, which inverts tenancy; and the toggle can lose it, which is the interaction the item's framing already worries about.
4. **The revert's own command group, as a second command-chain trailer.** `CommandGroup` gains `restored_unverified: bool`, encoded as trailer tag `0x02`, present only when set, so every group already written renders and hashes exactly as before (§0.10's machinery, its golden test, and its explicit anticipation of "a future flag would take a new tag byte").

**Recommendation: option 4.** Six reasons, and the last two are the ones that decide it.

- It is in the same hash-chained artifact as the act it qualifies, so the statement and the thing it is about cannot be separated, cannot disagree, and cannot be retained differently.
- Its retention is automatically right: the statement "this restore was unverified" is interesting exactly as long as the group exists, and it ages out with it.
- Its reader is the right one: the space owner, in the command-log explorer, which is where `dashboard-plan.md` §3 already renders group detail and before/after images.
- It honours the ruling's substance. The owner refused a *security event* on the ground that it would be noise; a trailer bit on the group is not a security event, does not appear in the denial explorer, and adds no line to any stream.
- It costs no new subsystem, where option 1 costs an entire one.
- It is immune to the toggle. An operator who turns 11-22 off loses nothing about blob verification, which removes the awkward interaction entirely instead of mitigating it.

Two costs, stated rather than buried. The flag is **group-level**, so it says "at least one restore in this group was unverified" and not which field; the ruling's line named the entity, the field and the handle. The detail is derivable from the group itself, since the group carries the before-image it restored from and the absent `"{field}$digest"` key is exactly what the flag is about, so one bit plus the image is a complete answer. And a new trailer tag is a **fleet-wide forward step**: an older binary meeting tag `0x02` refuses the group as corrupt rather than silently dropping the flag (`chain.rs:66-71`), so the decode must ship everywhere before the first flagged group is written. That is the same cost `EventsDropped`'s tag 13 already paid on the other chain.

The precedent is exact and is in the tree: `owner_administration` exists "for exactly one reader, an auditor, and for exactly one question", is hashed into the chain so it cannot be edited later, and is explicitly not an authorization input (`command.rs:157-183`). "Did the restore this group performed have anything to verify against" is the same kind of question from the same reader.

**The fork, stated so the owner can take the other branch.** If he prefers to keep the ops-log answer, then 11-22 does not broaden, and the ops log becomes a tracked missing subsystem in its own right: named in six documents, implemented nowhere, and now a prerequisite of an in-flight increment. That is a defensible position and it should be filed as a row rather than left as a phrase.

### 3.4 The negative rule, written down

Nothing enters this artifact merely because it has no other home. An event with no home is a missing artifact, and the correct response is to name the missing artifact. This sentence exists because the deployment stream is the only chained, retained, verified place in the system that accepts anything, and that makes it the default destination for every homeless record unless it says otherwise.

---

## 4 · The toggle [settled as shape, proposed as mechanism]

Ruled 2026-09-07: its own artifact as the default, with a deployment-config toggle to turn it off. The mechanism, following 0.8's precedents:

**`ORDO_DEPLOYMENT_SECLOG`, default on.** Unset means on, in the manner of the deny-volume alert rather than of `ORDO_METRICS_PER_SPACE`, and for the reason `metrics.rs:274-283` gives: a control a missing variable silences is not a control. The value is read once at construction and surfaced as a field, with a code-level builder beside it for deployments that configure in code (the shape `Metrics::with_alert_rule` already has).

**What "off" turns off, precisely.** Deployment-scope observations are discarded at `Recorder::observe`, before the collapser and before the outbox, and counted. Space-scope routing is untouched: a tenant's stream is not the operator's to disable. The retention pass and the reader still work over what is already stored, since turning recording off is not an instruction to delete evidence.

**Off is loud in three places.** A counter (`ordo_secevent_disabled_total`), a `/health/deep` line in the `security_stream` block naming the state and the variable, and the artifact's own last entry.

**The artifact records its own disabling.** New tags `StreamDisabled` and `StreamEnabled`, appended at the transition and only at the transition. Without them the stream simply stops, and a stream that stops is indistinguishable from a deployment where nothing happened and from one where entries were removed, which are the same two ambiguities `StreamError::RetainedTail` exists to prevent (`store.rs:156-176`). Transition detection is a single read of the deployment scope's last entry at drainer start, so a restart while off does not append a second marker.

---

## 5 · Increments

Each is green on its own. D1 is a prerequisite for everything after it, because it changes what an entry can say.

**D0 · The platform surfaces stop authorizing against a caller-named space.** Independent of the rest of this plan and shippable first. `authorize` (`serve.rs:535-547`) resolves the check space from `state.space` for the reserved platform ops rather than from `space_of`, and `platform.security.alerts` is deployed and granted in the builder beside the other two (`lib.rs:1421`), since a mounted route whose op nobody registered denies everyone. *Done when* `a_platform_op_is_not_authorized_by_a_caller_named_space`, `metrics_denies_an_owner_of_another_space`, and `the_alerts_route_answers_the_deployment_owner` pass. *Size: small.*

**D1a · Pin the goldens before touching the encoder.** One test file, computed against the current encoder, holding the byte length and the chain hash of a representative deployment entry and a representative space entry, with the module doc recording that a golden derived from the changed code proves nothing (`crates/ordo-commandlog/src/tests.rs:646-649` states the rule). *Done when* `a_deployment_entry_hashes_where_it_hashed` and `a_space_entry_hashes_where_it_hashed` pass on unmodified code. *Size: small.*

**D1b · The `about` field.** `StreamEntry.about` and `CollapseKey.about`; `sanitize_space` beside `sanitize_op`; the trailer in `encode_entry` / `decode_entry`; `route` and `observe` carrying the bound space through instead of dropping it; the invariant that a space-scope entry carries no target; `Retention` gaining its deployment number; the denormalized `about` column added to `SECEVENT_DDL` with `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` (there is no migration machinery for platform tables, `crates/ordo-exec/src/lib.rs:290-320`), null on pre-existing rows, and a copy rather than the record in the manner `secevent.rs:76-82` already argues. *Done when* D1a's goldens still pass, plus `a_sweep_over_a_hundred_spaces_is_a_hundred_entries`, `the_window_cap_counts_per_scope_and_not_per_target`, `a_hostile_space_id_becomes_the_sentinel`, `a_space_scope_entry_carries_no_target`, `a_trailer_with_an_unknown_tag_refuses`, `an_explicit_empty_trailer_is_trailing_bytes`, and a durable round trip through `PgSecurityStore`. *Size: a day.*

**D2 · The producers stop losing the space.** The door's three `deny` sites already pass it (`lib.rs:1192`, `:1241`, `:1270`) and now keep it; `MeteredKeys::release` passes the released space (`metrics.rs:902`), which closes the contradiction with `implementation.md:490`. *Done when* `a_non_member_denial_names_the_space_it_probed`, `an_unverified_caller_s_denial_names_the_space_it_probed`, `a_key_release_names_the_space_it_touched`, and `the_deny_volume_monitor_is_unchanged` pass. *Size: small.*

**D3 · The toggle.** §4 in full, including the two transition tags and the `/health/deep` line. *Done when* `the_default_is_on_with_no_variable_set`, `off_discards_only_deployment_scope`, `a_space_stream_is_unaffected_by_the_toggle`, `the_artifact_records_its_own_disabling_once`, `a_restart_while_off_appends_no_second_marker`, and `health_deep_names_the_variable_when_off` pass. *Size: a day.*

**D4 · `ordo verify --deployment`.** The second artifact gets a verifier, using the security key rather than the command-chain key, reporting the anchor it chose and the head it reached, in the shape `verify` already uses for a space (`crates/ordo-cli/src/operator.rs:310-354`). Verification only, no content, so it discloses nothing an operator without database access could use. *Done when* `verify_deployment_reports_its_anchor_and_head`, `a_tampered_deployment_entry_exits_three`, and `verify_deployment_refuses_the_command_chain_key` pass. *Size: small.*

**D5 · The reader, offline and cursorless (R1).** `ordo seclog --deployment [--since] [--about] [--principal] [--limit]`, rendering through `StreamEntry::render` (`event.rs:607-644`), paging by index, registering nothing, and appending a `StreamRead` entry through the recorder. *Done when* `a_read_is_itself_recorded`, `the_reader_registers_no_consumer` (asserted against `$logconsumers`), `paging_is_by_index_and_not_by_a_token`, `a_filter_on_about_finds_the_sweep`, and `the_reader_prints_no_arguments_because_there_are_none` pass. *Size: a day.*

**D6 · The HTTP surface and the pane (R2). [proposed, gated on Q-2]** A fourth reserved platform op, authorized against the deployment's own space per D0 and step-up gated, feeding `dashboard-plan.md` §3's cross-space view, plus the derived per-space count that replaces R3. *Done when* `the_deployment_log_op_is_not_reachable_from_a_capability_token`, `a_space_owner_cannot_read_the_deployment_log`, `the_per_space_count_names_no_principal`, and an HTTP round trip pass. *Size: more than a day, and it should not start before Q-2 is answered.*

**D7 · The unverified restore's home.** *Done, 2026-09-10.* Q-1 took the
recommendation: `CommandGroup::restored_unverified`, trailer tag `0x02`, the golden-first sequence D1a used, `undo::apply` setting it on the fourth outcome, and `plan-blob-chain.md` §4.4's ops-log paragraph rewritten. *Done when* `an_unflagged_group_still_hashes_where_it_did`, `a_restore_without_a_digest_flags_its_group`, `a_verified_restore_leaves_the_group_unflagged`, `an_unknown_trailer_tag_still_refuses`, and `scripts/build-wasm.sh` pass. *Size: small, and it must land before C6 ships an `eprintln`.*

**D8 · The record.** §6.1 gains a paragraph naming the deployment artifact's differences by reference rather than by restatement; `implementation.md:488-492`'s "ordinary structured logging" is corrected to the hash-chained default with a toggle, which is what the code has done since 7a; `bootstrap.md` §8's "a deployment control-plane audit stream `[proposed]`" is resolved to this artifact; the checklist's 11-22 row records the readership ruling and the population rule; `corpus-rulings.md` gains the found-while-building section. *Done when* the documents say what the code does. *Size: small.*

---

## 6 · Questions for the owner

Each has a recommendation, so there is something to disagree with.

**Q-1 · Where the unverified restore goes. ANSWERED 2026-09-10, and built.**
The owner deferred the choice and it was taken as recommended: the revert's
own command group, trailer tag `0x02`, landed as D7. `ops_log_line()` is
deleted with it, since it was a rendering built as the seam for an artifact
this decision bypasses. See `corpus-rulings.md`. The original question
follows.

**Q-1 (original) · Where the unverified restore goes.** You ruled it to the ops log on 2026-09-09, not to the security stream, on the ground that a security event there would be noise. §0.9 verified that the ops log does not exist: no logging framework in the workspace, no `eprintln!` in `ordo-exec` or `ordo-host`, and `implementation.md`'s structured logs still `[proposed]`. So the record currently has no home and C6 ships in days.

**Recommendation: put it on the revert's own command group as a second trailer bit (§3.3, option 4), rather than on the ops log or in 11-22.** It honours both halves of your ruling (it is not a security event, and the fact is recorded), it costs one byte on flagged groups and nothing on every group already written, its reader is the space owner in the pane that already renders group detail, it is retained exactly as long as the thing it describes, and it is immune to 11-22's toggle. The precedent is `owner_administration`, which exists for one auditor and one question. If you prefer the ops log, then that log needs a checklist row of its own, because it is a prerequisite now rather than a convenience.

**Q-2 · Who may read the deployment artifact. ANSWERED 2026-09-10.**
The backend's own admin, and no one else. Space owners are refused, so no
per-space filtered feed; the subject is refused. The platform reading its
own artifact was considered and has no caller, since it acts on these events
as they occur rather than reading its record back. So R1 and R2 are both
approved and R3 and R5 are refused, which makes D0 a prerequisite rather
than a tidy-up. The two rules of §2.3 ride along: every read appends an
entry, and the reader holds no cursor.

**Residual, filed rather than solved**: the admin reads, prunes and can
toggle the artifact off, and append-on-read does not close that, since
whoever prunes can prune the read records. R4, a role that reads and cannot
prune, is the fix and has nothing to build on today.

**Q-2 (original) · Who may read the deployment artifact.** Candidates and consequences in §2.1.

**Recommendation: the offline operator first (R1), an operator HTTP surface next behind your ruling (R2), no per-space projection ever (R3 refused).** The reason to refuse R3 is not caution about filters: the entries name principals who are strangers to the space and may be another tenant's users, so a filtered per-space view discloses one tenant's people to another. Space owners get a derived count that names nobody. Two rules ride with any reader: every read appends an entry to the artifact it read, and the reader holds no cursor, which is what keeps §6.1's floor at "age, anchored, with a tail" until something genuinely reads from a position.

**Q-3 · Does the reader need a fifth deployment-scoped op?** The 2026-09-07 ruling closed the space-free op set at four kinds and said anything new must argue its way in. R1 needs nothing, because it is not an op. R2 would be a reserved platform op in the manner of `platform.metrics`, which is authorized by the ordinary access model rather than by the `scope deployment` class, so on a strict reading it does not touch the closed set.

**Recommendation: build R2 as a reserved platform op, and treat that as inside the existing pattern rather than as a fifth kind.** If you read the closed set as covering reserved platform ops too, then D6 waits for a ruling and D5 is the whole of the reader for now, which is a survivable outcome.

**Q-4 · The 256-fold growth under a sweep.** Fixing the collapse key (§1.3) means one sweeping principal produces up to `DEFAULT_CAP` (256) entries per sixty-second window instead of one, plus a `Suppressed` marker. That is the artifact doing its job, and it is also a real volume change on the deployment's disk during exactly the incident when the deployment is under load.

**Revisited 2026-09-12, after the tenancy ruling (checklist 18c/18d: one
tenant, one database, one key).** §1.3's cap logic is unchanged, since
there is still exactly one deployment scope, so the cap still gives a
sweep one budget rather than one per target. Three things do change.

The volume now lands in the **control database** rather than anywhere a
tenant can feel it, which is where that pressure belongs.

The arithmetic changes if `key-release-home` takes option (a) and routes
key releases into space streams. Key releases are the deployment
stream's dominant term today by orders of magnitude (roughly 850,000 a
day at a hundred tenants, against about 2.3 MB an hour for a capped
sweep). Move them out and the deployment stream's steady state falls to
near nothing, so this growth stops being a spike on a large baseline and
becomes nearly all of what the stream holds. That makes it a clearer
signal, and it makes the independent retention number D1b landed much
cheaper to set generously: a stream that is quiet at rest can afford a
year of sweeps where one carrying every key release cannot afford
ninety days.

And one property becomes load-bearing rather than incidental: **the
deployment stream must be writable without opening any tenant
database.** A caller can name a space that does not exist, and under
database-per-tenant there is no tenant database to open for that
request. It is true today by accident of the routing rule; it becomes a
requirement, and it is why this stream cannot be sharded by target space
even though every entry now names one.

**Recommendation: take the growth, keep the cap keyed on the scope, and set the deployment retention number independently (§1.4).** The alternative, capping per target, hands the attacker the ability to size the record by choosing space ids, which is the defect being fixed rather than a bound on it.

**Q-5 · Does D0 ship now? ANSWERED 2026-09-10: yes.** And it is now
load-bearing rather than hygienic, because Q-2 approved a reader reachable
over HTTP.

**Q-5 (original) · Does D0 ship now?** §0.7 is a property of code that exists today: `authorize` runs its check against a space the caller names in a header, and `platform.metrics` is granted to `Role::Owner`. I did not find an in-tree deployment that provisions more than one space through that builder, so I have not confirmed an exploitable path.

**Recommendation: ship D0 ahead of this plan.** It is small, it is a precondition for any deployment-wide reader, and "no multi-space deployment exists yet" is a fact about today that the fix should not depend on.
