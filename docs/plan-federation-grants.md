# Federation through the door: grants between Ordo deployments

Status: **design agreed with the owner in conversation, 2026-11-25 and 2026-11-26.**
Not built. Board task `federation-ordo-deployments-share-data-through`.
Replaces the shared-database peer path (`peer`, `reader`, Postgres grants)
described in `definition-language.md` "`peer`" and `plan-evolution-federation.md`.

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
  No key moves. The `seal_read_by_reader` refusal goes.
- A tenant kept in a database of its own reaches a peer like any other.
- `$ordorelease` grants to readers (PR 106) go with the shared-database path;
  the release check reads a consumer's release through the door.

## 7 · Open, to settle while building

- How a consumer proves itself to the service. Clients are public today
  (PKCE, no secret; `ordo-identity-service/src/oidc.rs`). Recommendation:
  a key derived from the deployment's master key, registered at the service
  when the deployment is registered, so no person handles a key (owner on
  Q11: having to give a key is a nuisance). Built in G2 this way
  (`plan-identity-service.md` 11.5). Still open: who carries the public
  half to the service. A provisioner registers it with the deployment;
  the route also takes the deployment's own admin token, so the host or
  console can register it unasked.
- How the call chain is carried for the loop check (a header listing the
  deployments a call has passed through, signed with the token).

## 8 · Build order

| Step | What | Size |
|---|---|---|
| G1 | `grant` blocks: parser, expansion at apply, plan output, refusals | M |
| G2 | The identity service: consumer credentials, token exchange with the actor claim. **Built** on `fed-g2-token-exchange`: consumer key derived from the master key, public half registered (`consumerKey`, `deployment consumer-key`); RFC 7523 assertions, each `jti` once; RFC 8693 exchange (`typ` `ordo-peer+jwt`, `act.sub`) and `client_credentials` (`typ` `ordo-client+jwt`, `kind` `client`), `aud` `ordo-peer:<holder>`; `ORDO_IDENTITY_PEER_TOKEN_SECS` 300, `ORDO_IDENTITY_CLIENT_ASSERTION_SECS` 120; audit `oidc.exchange`, `oidc.client-token`. Tests: `ordo-identity-service/tests/federation_exchange.rs`, 7. The host deriving and registering its key is G4 | M |
| G3 | The holder's door: grant check on every call, on behalf or as itself, the loop check, audit | M |
| G4 | The consumer: `peer` block checked against the holder at apply; the holder records uses and names breaks | M |
| G5 | Peer entities served by door calls: pushdown, batched joins, `$caller` through the exchanged token | L |
| G6 | `grant provider` | S |
| G7 | Sealed fields through the holder; tenants in their own database | S |
| G8 | note-viewer and the federation e2e move over; the shared-database path is removed | M |
