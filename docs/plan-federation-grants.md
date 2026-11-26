# Federation through the door: grants between Ordo deployments

Status: **design agreed with the owner in conversation, 2026-11-25 and 2026-11-26.**
G1 to G8 and G5b built. Board task `federation-ordo-deployments-share-data-through`.
G8 removed the shared-database peer path (`peer` alone, `reader`, Postgres
grants); `definition-language.md` "`peer` alone and `reader`" lists what
went and what to write instead.

## 1 · Words

- **Deployment**: one instance of an app, with its own data, keys, door,
  definition file and settings. Its environment (dev, staging, production)
  is a property of it.
- **Node**: one running copy of the Ordo server. More than one node can hold
  the same deployment, as identical copies for high availability. Settings
  are one value per deployment, the same on every node holding it.
- **Holder**: the deployment that holds a piece of data.
- **Consumer**: a deployment that uses a holder's data through the holder's door.
- **External system**: anything not Ordo. Read through generated translations
  in Ordo's own database (`plan-external-entities.md`). Unchanged by this plan.
- **Grant**: what a holder allows one consumer, written in the holder's file.
- **On behalf of a person**: a call carrying the person's identity; the holder
  applies its own access rules for that person.
- **As itself**: a call carrying only the consumer's identity.

## 2 · The rules

1. A deployment's data lives only in that deployment. No copy of any kind,
   managed or not (owner: *"no copy. data lives within its deployment end of
   story"*). Where its database sits is not part of the design.
2. One Ordo deployment reaches another only through the holder's door, never
   its database (board, ask 1, *"Yes"*).
3. The shared-database path is removed once this works (ask 2, *"Remove"*).
4. The public image carries the minimum for separate deployments to share
   data or sign-in (ask 3).
5. The identity service says who is calling: the person and the consumer.
   The holder alone decides what they may do.
6. A consumer writes only by calling the holder's own ops. Reads are ops too
   (`std.<entity>.get`, `std.<entity>.list`).
7. On behalf of a person, a consumer can do nothing the person could not do
   in the holder directly. As itself, only what its grant opens to it.
8. A person is the same person in 2 deployments when the provider and the
   subject match. Never by email. Shared sign-in is needed only for calls on
   behalf of a person.
9. 2 deployments may grant each other. Only a call chain that comes back to a
   deployment already in it is refused.

## 3 · The grant, in the holder's file

```
grant note_viewer {
  std.notes.get
  std.notes.list
  /std\.notesAccess\..*/
  /add.*/
  !addAdmin
}

grant billing as itself {
  markPaid
}

grant provider hominem {
  std.notes.get
}
```

- A line is an exact op name, a `/regex/` that must match the whole name
  (Rust `regex` syntax, linear time), or `*`.
- `!` before a line negates it. Lines are read top to bottom and the last line
  that matches an op decides. No match means not granted.
- Patterns are expanded at apply. The door checks only the expanded list.
  The plan shows the list and every change to it ("note_viewer gains
  addReaction, matches /add.*/"). A later op that matches is granted by the
  pattern and shown in the plan.
- Refused at apply, by name: a pattern that matches nothing; a line that
  changes nothing (overridden by a later line).
- `as itself` in the header allows calls with no person. Without it, the
  grant is used only on behalf of a person.
- `grant provider <name>` covers every deployment registered at that
  identity service, on behalf of a person only. Allowed only for an Ordo
  identity service; refused for an outside provider, since everything
  registered with Google is anyone.
- The block holds op lines only. A keyword is added only when something
  other than ops must go there.
- The tie from the name `note_viewer` to that consumer's id at the identity
  service is a setting on the holder, `ORDO_CONSUMER_<NAME>_CLIENT`, never in
  the file (ids differ per environment; owner ruling 2026-11-14).

## 4 · The consumer's file

```
peer notes {
  std.notes.list
  addComment
}
```

- At the consumer's apply, Ordo reads the holder's expanded grant and op
  shapes from the holder's door and refuses, by name, anything the holder
  does not grant. The holder records what the consumer uses.
- The holder's plan names each consumer a change would break
  ("note_viewer calls archiveNote") before it applies.
- External entities from a peer become door calls, not SQL. A filter, sort,
  limit and field list are sent whole and run as the holder's own SQL; a join
  between a native entity and a peer entity is one batched call, never one
  call per row. One client inside the host, with one typed error (AGENTS.md
  rule 9).
- **Consistency.** There is no transaction across deployments. 2 reads of a
  holder in one op are 2 calls, not one snapshot, and the rows read from the
  holder are not of the same instant as this deployment's own. A write here
  never waits on, or undoes, anything in the holder.

## 5 · How a call proves who it is

1. Both deployments are registered at the identity service and trust its
   signature; the holder fetches its public keys once.
2. On behalf of a person: the consumer sends the service the person's token
   and its own credential and receives a token for the holder that says
   "this person, acting through this consumer" (OAuth token exchange, the
   actor claim). The person's browser talks only to the consumer.
3. As itself: the consumer gets a token for itself, addressed to the holder.
4. The holder checks the signature, the audience, the consumer's id against
   its grant, then its own access rules for the person. Its log records both.
5. A person with no member in the holder is handled by the holder's own
   sign-up rules: open sign-up makes a member with no data; invite-only refuses.

## 6 · What follows

- Sealed fields: the holder opens them in its own answers, under the grant.
  No key moves. The `seal_read_by_reader` refusal goes. **Built in G7**:
  a derived `std.<entity>.list` carries a sealed field only when the
  entity says `std sealed` (definition-language §3), the holder's own act.
  The refusal went with the `reader` line in G8.
- A tenant kept in a database of its own reaches a peer like any other.
  **Built in G7.**
- `$ordorelease` grants to readers (PR 106) went with the shared-database
  path in G8. **Built in G8:** the consumer sends its Ordo release with
  `std.peer.recordUse` and the holder answers its own; each keeps the
  other's (`$peeruses`, `$peerholders`), and `ordo release-check` names
  each peer and each consumer with it.

## 7 · Open, to settle while building

- How a consumer proves itself to the service. Clients are public today
  (PKCE, no secret; `ordo-identity-service/src/oidc.rs`). Recommendation:
  a key derived from the deployment's master key, registered at the service
  when the deployment is registered, so no person handles a key (owner on
  Q11: having to give a key is a nuisance). Built in G2 this way
  (`plan-identity-service.md` 11.5). Who carries the public half: G4
  built the host's side as the door op `std.peer.registerKey($token)`,
  which registers it with a token for the deployment's own admin. The
  host holds no admin token of its own, so a person (CLI or console)
  passes theirs once; the apply's refusal names the op until then.
  Whether the CLI or console should do this unasked is an owner question.
- How a `grant provider` tells Ordo's identity service from an outside
  provider. G1 uses the deployment's settings: the connection is an OIDC
  connection whose `ORDO_AUTH_<NAME>_ISSUER` is `ORDO_IDENTITY_ORIGIN`.
  G2 now exists; whether the service should mark itself (for example in
  its discovery document) is for G6. **G6 kept the settings test**, the
  conservative choice: the door already trusts only the service's issuer
  for peer tokens, so a mark in discovery would add a second source for
  the same fact. An owner question (G6 report).
- How the call chain is carried for the loop check. **Settled in G3**: a
  `chain` claim in the token itself, not a header, so it is under the
  service's signature and cannot be stripped. The consumer sends its own
  incoming chain as `ordo_chain` at the token endpoint; the service adds the
  consumer last (at most 8). The holder refuses a chain that names it. The
  consumer's host passing its incoming chain on is G4 and G5. **On the
  read path since G5b:** the door hands a peer call's chain to the
  executor (`$chain`), and every peer read and every guard's peer read in
  a write passes it on. 2 deployments whose guards read each other are
  refused as a loop by name at once.
- How a consumer keeps acting for a person past their 10-minute ID token
  (board `federation-build-5-choices-from-steps`, ask 2). **Built in G5,
  the simpler option, an owner question:** the consumer signs a session
  statement (`typ` `ordo-session+jwt`: `iss` the consumer, `sub` the
  person, `aud` the service, at most `ORDO_IDENTITY_CLIENT_ASSERTION_SECS`)
  with its consumer key and exchanges it (`subject_token_type`
  `urn:ietf:params:oauth:token-type:jwt`). The service takes it from that
  consumer only, for a person still its member, and the token lives its own
  `ORDO_IDENTITY_PEER_TOKEN_SECS`. Measured against refresh tokens: the
  service has no `refresh_token` grant and the host keeps no ID token after
  sign-in, so refresh tokens need a grant, a store, rotation and revocation
  at the service, and a sealed store per session on every consumer node.
  The statement needs about 60 lines at the service and 20 in the client,
  and nothing is held. What it gives up: the consumer, not the person's
  live token, vouches that the person is there, so a consumer can act for
  any of its members. The holder's rules still bound every call.

## 8 · Build order

| Step | What | Size |
|---|---|---|
| G1 | `grant` blocks: parser, expansion at apply, plan output, refusals. **Built** (branch `fed-g1-grant-blocks`): the parser and the CDK's `grant()` and `grantProvider()`; `ordo_host::consumer_grants` expands each block at plan and apply, stores the lists in `$consumergrant` by version, and shows each list, its changes and `ORDO_CONSUMER_<NAME>_CLIENT`; the 4 refusals are `grant_refused`. A provider is the identity service when its OIDC issuer is `ORDO_IDENTITY_ORIGIN`. Not enforced on the door (G3). Tests: `ordo-syntax/tests/grant.rs`, `ordo-host/tests/consumer_grants.rs` (notes v2 with a `grant note_viewer` block), `ordo-cdk/test/grant.test.ts`, the isomorphism corpus | M |
| G2 | The identity service: consumer credentials, token exchange with the actor claim. **Built** on `fed-g2-token-exchange`: consumer key derived from the master key, public half registered (`consumerKey`, `deployment consumer-key`); RFC 7523 assertions, each `jti` once; RFC 8693 exchange (`typ` `ordo-peer+jwt`, `act.sub`) and `client_credentials` (`typ` `ordo-client+jwt`, `kind` `client`), `aud` `ordo-peer:<holder>`; `ORDO_IDENTITY_PEER_TOKEN_SECS` 300, `ORDO_IDENTITY_CLIENT_ASSERTION_SECS` 120; audit `oidc.exchange`, `oidc.client-token`. Tests: `ordo-identity-service/tests/federation_exchange.rs`, 7. The host deriving and registering its key is G4 | M |
| G3 | The holder's door: grant check on every call, on behalf or as itself, the loop check, audit. **Built** on `fed-g3-holder-door` (c4b6cd8d): `ordo_host::peer_door`. The token gate verifies `ordo-peer+jwt` and `ordo-client+jwt` with the identity service's keys (`typ`, the issuer of the one sign-in connection to the service, `aud` `ordo-peer:<ORDO_IDENTITY_DEPLOYMENT>`, `exp`), refuses a chain that names this deployment, and resolves the person as a direct sign-in does (`oidc:<connection>:<sub>`, the sign-up rules and ceiling). `check` then needs the op in the consumer's grants for the serving version (`ORDO_CONSUMER_<NAME>_CLIENT`; `grant provider <connection>` on behalf of a person only); as itself, a grant marked `as itself`, in the deployment's own space only; on behalf, the access engine for that person. Refusals name the consumer, the op and the fix (`AccessChecker::peer_refused`, answered `access_denied`); a bad signature keeps the uniform denial. Recorded as `PeerTokenRefused`, `ConsumerNotGranted`, `PeerLoop`, and each allowed call with the person and the consumer (`peer-person`/`peer-client`); a write's command log names `consumer:<handle>` as the agent. G2 change: the service writes the `chain` claim from `ordo_chain`. Tests: `ordo-host/tests/peer_door.rs` (4, real service and door), `federation_exchange.rs` chain test, `peer_token` unit tests. No live stream for a peer token yet | M |
| G4 | The consumer: `peer` block checked against the holder at apply; the holder records uses and names breaks. **Built** on `fed-g4-consumer` (135b7166). `peer <name> { op … }` (`peer_block`); the one-line form stays until G8. One client, `ordo_host::peer_client` (`PeerClientError`: retry, refuse, fix input): consumer key from custody (`blind_index` seed, then `peer_token::consumer_key`), a client assertion per token, `client_credentials` or token exchange, the incoming chain as `ordo_chain`, the door by `GET`/`POST`/`DELETE`. Settings `ORDO_PEER_<NAME>_URL`, `_DEPLOYMENT` (default the nickname), `_SPACE`, `ORDO_PEER_PLAIN_HTTP`. Door ops (`ordo_host::peer_ops`): `std.peer.grant` (read, the consumer's expanded grants and each granted op's `op`, `kind`, `params`), `std.peer.recordUse` (`$peeruses`), both only as itself in the holder's own space; `std.peer.registerKey($token)`, owner only. Consumer apply and plan: an ungranted op is `peer_refused` / `peer_op_not_granted`, nothing stored; the use is recorded before the version is stored. Holder plan and apply: `warning     breaks note_viewer: note_viewer calls addEntry …`, never a refusal (owner question). Tests: `ordo-host/tests/peer_consumer.rs` (3, real service and 2 deployments), `ordo-syntax/tests/peer.rs`, `peer_ops` and `peer_client` unit tests, `ordo-cdk/test/peer.test.ts`, the isomorphism corpus. Not built: a CLI command for the key; the promotion gate's apply does not check peer blocks yet | M |
| G5 | Peer entities served by door calls: pushdown, batched joins, `$caller` through the exchanged token. **Built** on `fed-g5-peer-reads` (7953d8e0 to f92cc99c). A `peer` with `ORDO_PEER_<NAME>_URL` set is read through its door: `ordo_exec::peer::resolve` marks it (`PeerSource::door`), skips its schema setting and drops it from every shared-database check. `ordo_exec::peer_reads::serve` runs before the plan: each alias of a peer entity is one call to `std.<table>.list` with a `$refine` (its conditions, its fields; for the read's own rows also its sort, the id, and its `take` or page as `limit`); a ref hop from a native row is one batched `id in [...]` call, its refs from a probe (the read with that peer's rows empty); the answers are served to the one statement as typed literals (`ordo_plan::external_source`) and dropped with it. `field == $caller` on a peer entity goes as a `caller` condition the holder decides; here it reads `IS NOT NULL`; one under `or` or `not` is refused. Calls go for the person (`Acting::ForPerson`, their `sub` from `oidc:<connection>:<sub>`), or as itself when nobody is signed in. Holder side: `$refine` gains `sort`, `limit` (`ORDO_REFINE_MAX_LIMIT`, 200) and the `caller` op; a sorted read is never paged. The consumer apply checks and records `std.<table>.list` as if the block listed it. `ORDO_PEER_TIMEOUT_SECS` (10), `ORDO_PEER_READ_MAX_ROWS` (1000), `ORDO_PEER_BATCH_KEYS` (200); `peer_unavailable` (503), `peer_refused` (403). Ask 2 of `federation-build-5-choices-from-steps`: the session statement (§7). Tests: `ordo-host/tests/peer_reads.rs` (6: pushdown in 1 call, 100 joined rows in 1 call, `$caller` by the holder and past 20 minutes, ungranted refused, timeout refused, the shared path unchanged), `refine_door.rs` (`sort_limit_and_caller_…`), `federation_exchange.rs` (session statement and its 6 refusals). Not built: `read_as` fails with a 500 whose statement names the peer; live reads are not told of the holder's changes; no note-viewer e2e (G8 moves it over). The chain on reads and writes whose guards read a peer: G5b | L |
| G5b | 2 defects G5 left. **Built** on `fed-g5b-chain-guards` (f6834d06). (1) The chain on every peer call: `AccessChecker::peer_chain` reads the admitted token's `chain` (`PeerDoor::chain_of`), the door binds it as `$chain` (`ordo_api::CHAIN_PARAM`), and `PeerFetch::fetch` sends it as `ordo_chain`. Before, a read sent an empty chain, and 2 deployments whose guards read each other called each other until `ORDO_PEER_TIMEOUT_SECS` and answered `peer_unavailable`. (2) A write whose guard reads a peer through its door: the write protocol asks its connection to serve the guard's query (`WriteDb::serve_peers`) before it is planned, at the create check, the row check (pinned to the one row) and the ranged write. `BlockingTx` runs `peer_reads::serve` there: the holder is asked while the write runs, outside its transaction; the refs come from a probe in the transaction; the guard is decided on the holder's answer. A refusal is `access_guard`, a holder's refusal `peer_refused`; before, `internal` (500). Made to work rather than refused, because the write already reads native rows that may change after it, and a holder's rows are never in its transaction: the guard reads the holder as of the call, as a read does. The cost: the write's transaction stays open for the call, up to `ORDO_PEER_TIMEOUT_SECS`, and a retry asks again. Tests: `ordo-host/tests/peer_chain.rs` (3: guards that read each other refused as a loop in under 3 s; a create and an update allowed and refused on the holder's answer; a write whose guard read loops refused by name, nothing written) | M |
| G6 | `grant provider`. **Built** on `fed-g6-g7` (624f5554). G1 parsed and expanded it, G3 honoured it on the door, G4's `std.peer.grant` answered it. G6 checked it end to end and finished the plan: it shows `grant provider <name>`, never the bare name (a `grant ids` and a `grant provider ids` are 2 grants), and names the issuer it trusts and that a call as itself needs a grant of its own. Tests: `peer_consumer.rs` `a_provider_grant_covers_an_unnamed_consumer_for_a_person_and_never_as_itself` (real service and 2 deployments): `stranger`, registered at the service and in no grant, applies against `grant provider ids` (an op outside it refused by name, nothing stored; the use recorded), reads for Ada, is refused as itself naming `ORDO_CONSUMER_<NAME>_CLIENT=stranger`, and the holder's plan that drops the grant names it as broken. `peer_door.rs` keeps the door half | S |
| G7 | Sealed fields through the holder; tenants in their own database. **Built** on `fed-g6-g7` (624f5554). Sealed: the holder opens a sealed field in its own `std.<entity>.list` answer when the entity says `std sealed`; no key leaves it. A seal of a field a `grant` opens is not refused; `seal_read_by_reader` stays for a `reader` line (the shared path) and its fix now names the door. Tenants: `DoorReads` reads the member in the tenant's database and the sign-in link in the deployment's (`with_directory`), so a tenant in a database of its own reads through the door; before, its `$authenticator` join failed there. On the shared path such a tenant's op is refused `peer_refused`, line `peer_tenant_database`, naming the peer and `ORDO_PEER_<NAME>_URL` (`external_check::shared_peer_refusal`, checked in `FixtureExecutor` when the space's database is not the deployment's). Tests: `peer_reads.rs` `a_sealed_field_is_opened_by_the_holder_and_never_stored_plain`, `a_tenant_in_its_own_database_reaches_the_peer_through_the_door`, `a_tenant_in_its_own_database_is_refused_the_shared_database_path_by_name`; `sealed_backfill.rs` `a_seal_of_a_field_a_door_consumer_reads_is_not_refused`. Not built: the note-viewer e2e for plan-example-tests #15 (the viewer still reads through the shared database; it moves to the door in G8, which also removes the shared path the refusal guards) | S |
| G8 | note-viewer and the federation e2e move over; the shared-database path is removed. **Built** on `fed-g8-move-over`. **Language:** a one-line `peer` and a `reader` line are refused by name (`peer_name`, `reader_line`), each writing the block to use and the setting that replaces its old one. An external named like an op of its `peer` block reads that op (`PeerSource::op`), so a holder's own read, under its own conditions, is a consumer's table; `std.<table>.list` otherwise. **Removed:** `ordo_exec::peer` down to `door_url_env`, `external_schema_env` and `resolve` (a peer read by an external needs `ORDO_PEER_<NAME>_URL`, refused at boot by name); `$caller` through the peer's tables (`$peerlink`, `Planner::peer_caller`, now refused `peer_caller_unserved`); `reader`, `Schema::readers`, the Postgres grants (`$readergrant`), `ORDO_READER_<NAME>_ROLE`, `ORDO_PEER_<NAME>_SCHEMA`, `ORDO_PEER_SHARED_ROLE`; `ordo_exec::sign_in_contract` and `$ordorelease` (`grant_release_to_peers`); the renamed-column comment, `changing` and `/health/ready` `external.changing`; `seal_read_by_reader`; `peer_tenant_database`; the promotion's reader check; `$authconnection.subject_claim` writes; the CDK's `reader()` and one-line `peer()`. **Kept:** plain externals and `ORDO_EXTERNAL_<SCHEMA>_SCHEMA`; F1, now listing each `grant` that opens a changed table (`consumer_grants::tables_read`). **Release check:** each side keeps the other's release from the door (`std.peer.recordUse` sends and answers it), listed as `peer` and `consumer`. **`read_as`** (G5 left a 500) and a door read with no client refuse a peer read by name, `peer_refused`; the shape gate skips a read of a peer as undescribable. **Examples:** notes v2 0.2.1 (0.2.0 unchanged elsewhere) adds `publicNotes` and `grant note_viewer as itself { myNotes  publicNotes }`; notes v3 0.3.0 the same (an owner question: its content changed in place); note_viewer 0.2.0 reads through `peer notes { myNotes  publicNotes }`, authored ops rather than `std.notes.list`, which a notes editor may not call and whose fields follow every rename, its page shows each list's refusal in that list's place. **Tests:** `ordo-syntax/tests/peer.rs` (the 2 refusals), `ordo-exec` `peer::tests`, `peer_reads.rs` (`read_as_refuses_…`, `an_external_named_like_an_op_…`), `peer_consumer.rs` (releases both ways), `stored_plans.rs` (F1 on grants), the note-viewer e2e (projects `door`, `api`, `browser`, `v3`, `rename`, `orphans`, `seal`, `narrowed`), the notes e2e's seal without the reader step. Deleted: `reader_grants.rs`, `peer_entities.rs`, the reader tests of `promotion.rs`, `prechecks.rs`, `sealed_backfill.rs`, `identity_service_oidc.rs` | M |
