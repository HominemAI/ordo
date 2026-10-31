# Auditing Deployment History

A deployment's command log is tamper-evident. This guide explains what the platform provides to detect history rewrites, what clients must preserve to make that detection work, and how to verify claims about the past.

---

## The Command Log

Every write appends an attributed group to the per-space chain. Each group is one database transaction, one or more commands, and lives in `$commandlog` as an immutable row.

A group carries:
- **Correlation ID**: unique per space, stable across retries, and the handle by which it can be reverted
- **Actor attribution**: who ran the commands, in canonical form (e.g., `user:alice` or `system:bootstrap`)
- **Count and sequence**: how many commands and their order
- **Index**: its position in the chain (0-based)
- **Prev hash**: the SHA-256 hash of the group that came before it (32 zero bytes for genesis)
- **Owner administration**: set when the space owner used their bootstrap rule-6 capability to grant, revoke, or change a level on an acl row *without holding an access row*; absent otherwise
- **Body**: canonical byte encoding of all above, for hashing
- **Hash**: SHA-256(prev || body)

The owner-administration flag is the one that answers a question attribution alone cannot. A space owner may administer acl rows two ways: by the rule-6 capability, or by holding an admin level like any other member. Both produce the identical command, on the identical row, with the identical actor. The flag says which. It is inside the body, so it is inside the hash: it cannot be added to or cleared from a stored group without verification saying so.

The canonical encoding is frozen: integers big-endian, strings length-prefixed UTF-8, image field order stable (BTreeMap). A byte flipped in storage is caught by re-hashing. The flag is encoded by presence: one trailing byte when set, nothing when not, so groups written before it existed hash exactly where they always did.

Verification reconstructs the chain from stored content: recompute each hash as SHA-256(prev || stored bytes), and check it matches the stored hash. Verification finds both broken links (stored prev ≠ prior hash) and content tampering (recomputed hash ≠ stored hash).

---

## Layer-1 Client Pinning

The platform does not require storage or polling to detect rewrites: every response carries a signed attestation of the chain's current head, and a client holding one 100-byte pin is a witness.

### What the client receives

Five response headers on every call:
- `x-ordo-position`: the chain position (how many groups have been appended, 0 for empty)
- `x-ordo-head`: the SHA-256 hash at that position, as hex
- `x-ordo-head-key`: which key ID signed this attestation
- `x-ordo-head-sig`: Ed25519 signature over (space, position, head), as hex
- `x-ordo-verifiable-from`: the earliest position still available for recomputation (unsigned; covered by pruning section below)

### What a client stores and checks

Store the tuple `(position, head, signature)` from any response. On the next call, compare the new values against the stored ones:

- If `position` moved forward but `head` changed differently than the chain alone would (or stayed constant when new groups were appended), or
- If the signature does not verify against the deployment's published public key,

then the deployment rewrote history. The moment a single client detects this, the rewrite is detected. No audit is required, no polling, and detection latency is one request.

### What verification entails

Verification has three steps:

1. **Obtain the public key** for the key ID named in `x-ordo-head-key`. The deployment publishes this key (e.g., in `/.well-known/ordo-keys`).
2. **Recompute the signed bytes** (the platform's canonical encoding of `(space, position, head)`) using the position and head from the response.
3. **Verify the signature** using Ed25519 and the public key. The signature must verify exactly.

A signature that does not verify means either the position or the head was altered in flight, or the deployment recomputed without moving; all indicate tampering.

### The important limit

Layer-1 pinning **detects tampering; it does not prevent it**. No consistency guarantee backs the headers. A deployment can serve a different head for the same position on consecutive calls, or serve a head whose signature does not verify. The client learns this happened one request later, not before. The platform's job is to make detection cheap and latency-free; the client's job is to keep a pin and notice when it changes.

---

## Checkpoints

Checkpoints are signed chain positions recorded in the `$checkpoints` table. They anchor verification: once a checkpoint at position N is verified, the chain from genesis to N is locked, and pruning can be performed safely downstream of that point.

A checkpoint carries:
- **Space**: which space
- **Position**: which chain position (after this many groups)
- **Head**: the chain hash at that position
- **Timestamp**: when it was signed
- **Key ID**: which key signed it
- **Signature**: Ed25519 signature over `(space, position, head)`, with timestamp = 0 (not part of the signature)

A checkpoint is verified exactly like a layer-1 pin: obtain the public key, recompute the signed bytes, and verify the signature. The signature covers only `(space, position, head)`, making it time-independent: a checkpoint from last month verifies with this month's public key the same way.

Checkpoint distribution (webhook, WORM storage, OpenTimestamps, etc.) is layer-2 and not part of this core contract. Checkpoints exist to let a client that stopped calling resume verification, or to feed an auditor who never called the platform at all.

---

## Reverting a Write

The platform cannot rewrite history. It can only append. To undo a group, it appends the inverse as a new group.

`revert_group(space, correlation_id, actor, now)` finds the named group and inverts it:

- A **Create** becomes a Delete: the after-image is discarded, the before-image (nothing) is null
- A **Delete** becomes a Create: the before-image is discarded, the after-image is taken
- A **Converge** stays a Converge: before-image and after-image swap

The inverse is a new group with:
- **Correlation ID** = `rev-` + original correlation ID
- **Actor** = the principal calling revert (not the original actor)
- **Commands** = inverted sequence, with each command ID prefixed `rev-`
- **Timestamp** = now (not the original timestamp)

This new group is appended to the chain like any other. A client verifying the chain sees both the original group and the revert; the history shows what was done and what was undone.

Reverting an already-reverted group is refused: the chain's unique index on `(space, correlation_id)` prevents a second append with the same ID. Reverting a revert is allowed (it produces `rev-rev-` IDs), which is a redo and is distinct history.

---

## Pruning

The chain can be trimmed. Once a verified checkpoint exists at position N, positions 0 through N-1 can be deleted. Deletion is not tampering; it is policy. But it makes old pins unusable.

A client holding a pin at position M cannot verify past the earliest retained position. The platform reports this in the `x-ordo-verifiable-from` header.

### What the client must do

On every response, compare the stored pin's position against `x-ordo-verifiable-from`:

- If **stored position ≤ verifiable_from**, recomputation is still possible: forward to the new head.
- If **stored position > verifiable_from**, the deployment deleted the span between them. The client cannot catch up to the head by recomputation. This is where the client learns the history it was away for is gone.

`verifiable_from` is unsigned and not covered by the signature. It is an instruction about feasibility, not evidence. A deployment cannot use it to make a client accept a forged history: recomputation against a signed checkpoint is the only way to move forward, and the signed checkpoint must exist and be correct.

### The cost

Making every pin a registered consumer (so the deployment will never prune past it) would let one abandoned browser tab hold a space's entire history forever. Instead, the platform accepts the loss and makes it visible: a client that stops calling and then returns learns immediately whether the span it missed is still available.

---

## The Security Stream, and why it is verified separately

There are **two** tamper-evident artifacts, not one. The command log records what
was done; the security-event stream records every authorization decision (denials, step-ups, token failures) and the platform's own custody acts. They are
hash-chained the same way and checkpointed the same way, and they are signed
**under different keys on purpose**. If one key signed both, compromising the key
that signs routine domain attestations would also be the ability to forge the
record of the compromise.

So `ordo verify` walks both and reports them apart:

```
ordo verify --space spc_7kq --db ordo \
  --key-id chain-2026 --public-key <hex> \
  --security-key-id sec-2026 --security-public-key <hex>
```

```
space   spc_7kq
anchor  checkpoint at position 400
VERIFIED 112 groups · head index 511 = 357b69…

security  spc_7kq
anchor    genesis
VERIFIED  1204 entries · head index 1203 = 6acc2f…
```

**Two verdicts, never merged.** A single summary line would be reporting whichever
artifact is longer, and which one moved is the entire diagnosis. Either verdict
failing exits non-zero, and **neither stops the other being checked**. A broken
command log is precisely the moment the security stream's verdict is worth the
most, because it is the artifact you can still believe when the domain record is
in doubt.

The deployment's own stream belongs to no space and has no command log beside it,
so it has its own mode. It is the only place a **cross-space sweep** is visible: a
principal probing a hundred spaces appears in none of those spaces' streams, by
design.

```
ordo verify --deployment --db ordo --security-key-id sec-2026 --security-public-key <hex>
```

### What a run without the security key does *not* tell you

The key is optional and its absence narrows the claim rather than blocking the
run. Without it the entries are still recomputed against each other, so a
doctored entry is still caught, but **nothing proves none was removed from the
end of the stream.** Deleting the last N entries leaves a perfectly
self-consistent chain; only a signed marker at a higher position knows they were
ever there. A keyless run says so in its output and names the checkpoint it
declined to use, so a green line is never mistaken for the stronger claim.

Publish the verifying key separately from the signing seed:
`ORDO_SECURITY_CHECKPOINT_KEY` holds the seed and signs;
`ORDO_SECURITY_CHECKPOINT_KEY_ID` and `ORDO_SECURITY_CHECKPOINT_PUBKEY` are what
a verifier needs and are safe to hand to an auditor.

When `ORDO_SECURITY_CHECKPOINT_KEY` is unset, the first boot makes a key, seals
it under the master key and keeps it in `$securitykey` in the control database.
Every later boot and every replica uses that one. The boot log prints the pair a
verifier needs:

```
  security checkpoint key (made at first boot); verify with ORDO_SECURITY_CHECKPOINT_KEY_ID=sec-… ORDO_SECURITY_CHECKPOINT_PUBKEY=…
```

`/health/deep` carries the same pair under `security_stream.checkpoint_key`. A
set key always wins over a made one. Losing the master key loses the made key
too; checkpoints signed by it still verify with its public half.

### Rotating the security key

**Pass both keys during the overlap.** The flags repeat and pair up in order:

```
ordo verify --space spc_7kq   --security-key-id sec-2026 --security-public-key <old hex>   --security-key-id sec-2027 --security-public-key <new hex>
```

A marker is checked against **the key it names**, not against whichever key
happens to work: key discovery is part of verification, and "some key we hold
verifies this" is a weaker statement than "the key it claims verifies this".

**A marker signed by a key you did not bring is reported as unreadable, not as
failed.** That distinction is the point. Before it existed, the day after a
rotation `ordo verify` reported the security stream as **FAILED**, because
the end-of-stream check verifies the *newest* marker, and every marker signed
before the rotation fails under the new key. You rotate a security key because
you suspect a compromise, so the moment the record most needs to be readable
was the moment it started crying wolf, and an operator who had just rotated
could not tell that finding from the real thing.

**Old markers are never re-signed under the new key**, and this is deliberate
rather than unimplemented. A rotation that could restate the past would make
the new key retroactive authority over the old record, exactly the property
the separate key exists to deny. Keep the old public key as long as you keep
the markers it signed; it verifies history and can sign nothing.

### What signs the marker

A deployment with `ORDO_SECURITY_CHECKPOINT_KEY` set signs a marker at the head
of every security stream **once an hour**, on the `secevent.checkpoint`
schedule. That is its own cadence and its own schedule row, separate from
retention's, and the separation is the point: how often a marker is signed
bounds how long entries can sit at the end of a stream where nothing signed
knows they exist, while how much history to keep is a different question with a
different answer. A deployment that keeps every denial forever still gets its
head signed.

The pass **verifies before it signs**, and refuses rather than anchor a stream
already in a state `ordo verify` would fail. It will not sign over history that
does not recompute, will not countersign a stream entries have already been
removed from the end of, and will not extend a retained tail nothing signed
vouches for. Each refusal is a line on `/health/deep` and leaves
`ordo_secevent_checkpoints_total` flat, which is the durable signal: a stream
with traffic and a flat count is a stream the end-of-stream check has nothing
to work with.

To sign one by hand (before a backup, before handing a stream to an auditor,
or where the security key is kept outside the server's environment):

```
ordo checkpoint --security --space <spc>
ordo checkpoint --security --deployment
```

It prints the position, the head, and the verifying key to publish.
`--security` *selects* the artifact rather than adding one: `ordo checkpoint
--space <spc>` on its own still signs the command log and nothing else, because
signing is an act under a specific key and the two keys are deliberately held
for different reasons. A refusal exits **3**, the verification-failed code, not
1: you asked to sign and were handed a verification finding.

With no security key set, nothing signs any stream. The startup census says so
on its own line, naming what is lost rather than only which variable is missing.

### Why verify-before-sign is not belt and braces

This is worth the paragraph, because the code did the opposite until
2026-09-26 and the consequence was not "a weaker guarantee".

**A checkpoint is the only lawful cut point.** Retention drops whole prefixes
ending at a verified marker, and `check_prune` is handed the head *at the cut
point*: it never walks the prefix it is about to destroy. So a marker signed
over a head nobody verified licenses the next retention pass to delete the very
entries that prove the stream was edited. Doctor an entry, wait for the pass:
the prefix goes, the survivors chain cleanly from a marker carrying the
deployment's own signature, and `ordo verify` reports **VERIFIED**. Retention
laundering a tamper, on schedule, as designed.

That is why the refusal lives in the signer and not only in the prune, and why
a refusal stops the cut as well as the signature.

---

## Layer 2: the attestation webhook

Layer 1 is always on and asks nothing of anyone. Layer 2 is **opt-in per
space** and is what makes the record exist somewhere the platform cannot
reach. A deployment that never enables it has chosen the default posture, not
a broken one.

### Setting a destination

The destination is **owner-signed**, so the platform stores it but cannot
silently point it elsewhere. That means the signing happens off the platform,
with a key it never sees, in three steps:

```
# 1. Learn the exact bytes to sign. The URL must be the canonical form:
#    lowercase host, :443 omitted, path present and beginning with "/".
ordo-verify-proof consent --space spc_acme --url https://hooks.example.com/attest

# 2. Sign those bytes with your Ed25519 key. They are the raw message,
#    not a digest.

# 3. Hand the platform the URL, your key, its id, and the signature.
POST /std.space.attestTo
{ "destination": "https://hooks.example.com/attest",
  "ownerKeyId":  "acme-audit-2026",
  "ownerKey":    "<64 hex>",
  "signature":   "<128 hex>" }
```

The op is **owner-only and step-up gated**, which has a consequence worth
knowing: a capability token can never nominate a destination, because step-up
denies outright when one is present.

The response carries the per-sink `secret` **once**. There is no op that reads
it back (`std.space.attestation` shows the destination, never the secret); losing it means running `attestTo` again, which is a fresh secret and
a fresh consent. It also reports `watchingFrom`, the chain position your
series starts at. A new sink starts at the *current* attestation, never at
genesis, because signed heads for a window you were not watching anchor
nothing.

### Reading it back, and switching it off

Owner ruling 2026-10-05. All three ops are the tenant owner's, step-up gated.

- `std.space.attestation` shows the destination that is set, its key id, when
  it was set, `watchingFrom`, and how many deliveries gave up. It never shows
  the secret. With no destination, every field but `deadLetters` is null.
- `std.space.attestTo` with no `destination` removes it. Deliveries still
  waiting for it are dropped, and the removal is recorded in the tenant's
  security stream as `attestation_destination_removed`. With nothing set, the
  answer is `unchanged` and nothing is recorded.
- `std.space.attestationDeadLetters` lists the deliveries that ran out of
  attempts: the position, the destination, the attempts, when, and a `kind`
  such as `timed_out` or `denied_address`. The detailed reason describes the
  deployment's network, so it is shown to the deployment owner only, on
  `/health/deep` under `supervisor.attestation_dead_letters`, with each
  destination cut to its host. Nothing retries a dead letter yet.

### Authenticating a delivery

Each POST carries two independent proofs, answering different questions.

**At ingest: is this really the platform?** `X-Ordo-Signature` is
`v1=<hex>`, HMAC-SHA256 under your per-sink secret over:

```
signed := str("v1") ‖ str(timestamp) ‖ str(canonical-destination) ‖ str(body)
str    := u64-big-endian byte length ‖ bytes
```

`X-Ordo-Timestamp` carries the same timestamp, so you can reject stale
deliveries without parsing the body. Choose your own replay window; 300
seconds is a reasonable default. Compare in constant time.

**Later: was this head really signed?** The payload's own Ed25519 signature,
which is the one that still means something months after the request is gone.
`ordo-verify-proof attestation` does it:

```
ordo-verify-proof attestation --file delivery.json \
  --key <deployment public key> --key-id <its id>
```

### The payload, `ordo.attestation/1`

```json
{
  "schema": "ordo.attestation/1",
  "space": "spc_acme",
  "upto": 4242,
  "head": "<64 hex>",
  "at": 1757000000,
  "keyId": "cp-2026",
  "signature": "<128 hex>",
  "signedBytes": "<hex>",
  "destinationKeyId": "acme-audit-2026",
  "cadence": { "everyCommands": 1000, "orSeconds": 3600 },
  "verifiableFrom": 40
}
```

Four fields deserve a note:

- **`signedBytes`** is the exact preimage the signature covers, and it is
  redundant on purpose. Reconstructing it means getting a u64 big-endian
  length prefix right, and a receiver who gets it wrong sees a *failed
  signature*: indistinguishable from tampering, and the one error nobody
  debugs correctly. Compare against this first.
- **`destinationKeyId`** is the owner key's fingerprint. **Record it on the
  first delivery and compare it on every one after.** The platform cannot
  redirect a delivery without the owner's private key, but a platform that
  rewrote the destination *and* the key together would produce a valid
  signature under a key it chose, and this field is how you see that. Same
  trust-on-first-use shape as layer 1.
- **`cadence`** is why silence is a signal. You now know the next delivery is
  due within `orSeconds`, without reading any documentation. **Alarm on the
  gap.** Omission is the only move a hostile platform has left, and it is the
  one you have to detect yourself.
- **`verifiableFrom`** is the chain's first retained index. It is **outside**
  the signature, deliberately, exactly as `x-ordo-verifiable-from` is: an
  unsigned floor cannot make you *accept* anything, so it is an instruction
  about feasibility rather than a claim.

### Where to point it

§7's documented targets, cheapest first. Only the first is implemented today;
the others are named so "optional" does not quietly mean "unsupported":

| Target | Status | What it buys |
|---|---|---|
| **Your own HTTPS endpoint** | **shipped** | A copy outside the platform's reach at a time it cannot retroactively influence, which is the whole property. Cheapest, and enough. |
| Object store with **object-lock/WORM** | owed | Unrewritable *even by you*, which is the strongest claim in the list |
| Sigstore **Rekor** | owed | A free public append-only log; verification needs Merkle inclusion proofs |
| **OpenTimestamps** | owed | Free and effectively zero per attestation; honest verification wants a Bitcoin node |
| **git** over HTTPS | owed | Commit history is itself a hash chain |
| Archived mailbox | works today | Point the webhook at a mail gateway you control |

---

## Layer 4: `chainProof`, and verifying without the deployment

`std.space.chainProof($from, $to)` exports a segment as
`ordo.chainproof/1`: positions, per-group `prev`/`hash` digests, the anchor,
the head, and every signed checkpoint or attestation inside the range. It is
**manager-and-owner** and step-up gated.

**It carries digests, not command bodies.** That is the design: your question
is whether the head you pinned is still what the chain says, which is a
question about links and signatures. It also means a proof discloses **no
space content**: no rows, no images, no actors, no correlation ids, which is
what makes it safe to hand to a third party. To check content, verify an
export bundle instead.

Save the `proof` field verbatim to a file (it is a string precisely so that
no client library re-serialises it) and run:

```
ordo-verify-proof proof --file proof.json \
  --key <public key> --key-id <its id> \
  --head <a head you pinned earlier>
```

`--head` is the flag that matters. Without it you learn the segment is
internally consistent; with it you learn it still ends where you last saw it,
which is the check that detects a rewrite.

**Without `--key` the tool prints `UNVERIFIED` and means it.** The key inside
a proof is the platform's claim about its own key, and checking a signature
against a key the signer chose proves nothing. Obtain the key the way you
obtain it for layer 1, and keep it.

The verifier is a separate binary from `ordo` on purpose: it links
`ordo-commandlog` and a JSON parser and nothing else: no database driver, no
socket, no configuration. Confirming it cannot phone home is reading one
dependency list rather than auditing a call graph. `ordo verify --proof` runs
the identical routine for operators who want it beside the other verification
commands.

Exit codes: `0` verified, `1` could not read the input, **`3` verification
failed**. Keep those separate in any automation: a bad download and a broken
chain must page different people.

### What a verified proof does and does not establish

Establishes: the links are unbroken; the segment begins at the anchor it
names; every signed marker inside it agrees with the link at its position; and
those signatures verify under **your** key. Combined with a head you pinned
earlier, that is *the history I witnessed at time T is still what this chain
says*: rewriting becomes provable.

Does not establish: that any hash corresponds to any particular content
(digests, not bodies); or anything at all about history before you started
watching. No scheme fixes the second, and layers 2–4 exist to make "started
watching" as early as possible.

---

## Putting It Together

A compliance reviewer or an engineer building a client needs to:

1. **Collect layer-1 pins**: store responses as they arrive, keeping `(position, head, signature)` from each
2. **Verify pin continuity**: check that positions are monotonic and signatures verify against the public key
3. **Detect at-risk spans**: when `x-ordo-verifiable-from` jumps, note what was pruned
4. **Checkpoint and archive**: request signed checkpoints at intervals and store them outside the platform
5. **Verify checkpoints**: same routine as layer-1 pins, using the public key
6. **Reconstruct the chain**: stream `$commandlog` entries from the earliest available position and re-hash from an anchoring checkpoint to the head

The result is proof: this deployment's history was not rewritten, or exactly when and how it was (via reverts), and exactly when data was pruned.
