# [proposed] Ordo Security Hardening Review 3 — architecture and trust boundaries

**[proposed] Status.** Everything in this document is `[proposed]`. Nothing here is a settled Ordo ruling. Quoted design text is evidence for a proposed finding, not an attempt to change the quoted text by implication.

**[proposed] Scope.** This is a cold architecture review of `README.md`, the glossary, the prior consolidated and security reviews, the language/platform/API/migration/modeling/std/traps documents, and the current `ordo-api`, `ordo-identity`, and `ordo-commandlog` crates. It deliberately does not repeat the deterministic-gate findings already tracked for keyword parity, plan coverage, unreferenced conformance rulings, status markers, or absolute registrations.

## [proposed] Executive summary

**[proposed] Hardest pushback.** The decision I would push back on hardest is presenting client-first mode as the same authorization model with only the execution venue moved. `implementation.md` says both that the local store is authoritative and that the same compiled artifacts run on the client, while one per-space key is wrapped to enrolled devices. A malicious or stolen client is not a trustworthy policy-enforcement point. Once a device holds the space key and ciphertext, it can bypass the local engine, decrypt rows outside its guards, fabricate ciphertext that did not pass checks/CAS/guards, and submit an opaque proposal the server cannot semantically validate. Without per-authority cryptographic compartments, trusted hardware, or proofs of correct execution, client-first can provide server blindness or hostile-member row isolation, but not both. This is SH3-001 and SH3-007.

**[proposed] Disagreement with review 2.** Review 2 calls SH-27's per-op hash plus ordered output-field comparison semantically correct. I disagree. Output fields are not an authority lattice. A write can add a second mutation, remove a guard, begin egress, or widen a routine trigger while returning the identical row shape. The documented comparison classifies the unchanged output set as narrowing (or leaves equality-under-hash-change undefined) and can silently retain consent after an authority expansion. This is SH3-002.

**[proposed] Disagreement with the current bootstrap closure.** X-005's ACL deadlock is closed operationally, but the universal owner escape relocates the defect into confidentiality: an owner can grant themselves top-level access to any ACL row and then read it. That may be an acceptable product choice, but it means ACLs never protect data from a malicious space owner and the claim must be structural and prominent, not inferred from an administration rule. This is SH3-004.

**[proposed] Strengths not to break.** The fixes below must preserve the closed runtime surface, one access-checked door, per-space custody dial, no grants in configuration, derived disclosure, open-skeleton std defaults, typed failures, single-space command groups, deterministic canonical encoding, export/audit/exit, client-first/offline use, and the honest statement that the envelope remains visible at every custody position. The design is strongest where authority is computed from artifacts and changes fail loudly; the proposed pins extend that method rather than introduce prose-only policy.

## [proposed] Exact threat actors

- **[proposed] Curious operator.** Holds platform database, bucket, logs, telemetry, deployment metadata, and ordinary operator access; must not gain sealed values or silently rewrite tenant authority/history.
- **[proposed] Compromised worker.** Holds the network and whatever resolver, execution, import, migration, key-release, or chain-append capability its current job needs; must not turn temporary work into cross-space or durable authority.
- **[proposed] Malicious tenant.** Holds a valid human membership and possibly the owner role in one space; must not cross guards, manufacture stronger authority than they hold, or use tenant-controlled configuration to weaken a platform invariant.
- **[proposed] Malicious guest.** Holds accepted guest binding and only the window/ACL authority explicitly granted in a foreign space; must not enumerate or retain anything beyond the disclosed window semantics.
- **[proposed] Stolen device with local mirror.** Holds an imaged or unlocked local database, cached tokens, and possibly an enrolled client-first device key; must not turn past local possession into new server authority or decrypt beyond its cryptographic grant.
- **[proposed] Supply chain.** Controls a package, package update, dependency, provider implementation, or compiled artifact input; must not expand read/write/egress/routine authority under a change classified as narrowing.
- **[proposed] Network adversary.** Can observe, replay, redirect, or steal bearer/session material and choose among public clients and endpoints; must not convert token theft into durable account or key enrollment.

## [proposed] Findings

### [proposed] SH3-001 — Critical — client-first collapses row authorization into possession of the space key

**[proposed] Exact design claim.** `implementation.md`, “Client-first mode,” says: “the client's local store is the authoritative working copy,” “Data rows and command images are sealed client-side under client-held keys,” queries run on the client using the “same compiled artifacts,” and multi-device operation uses key wrapping. “Device keys, enrollment, loss & revocation” says the space key is rewrapped for remaining devices. `definition-language.md` §3 and §8 separately promise row guards, ACLs, roles, profiles, and per-member overrides inside the same space.

**[proposed] Concrete attack path.** A viewer, editor, or co-tenant enrolls a device legitimately, or an attacker steals an enrolled device. The device receives material capable of opening the per-space data epochs. The attacker ignores the generated client and local engine, reads the local database or synced ciphertext directly, unwraps it with the device-held key, and decrypts rows the server-side guard would have filtered. Mirror scope is a retention/volume control, not a security boundary: a hostile client can request or retain all ciphertext made available to the space sync protocol. Revoking membership rotates only future epochs; the design correctly admits that old segments remain readable. Therefore ACL/profile distinctions inside a client-first space are UI/runtime conventions against a key holder, not confidentiality controls.

**[proposed] Affected custody positions.** Client-first is directly affected. Managed and BYO-DB/BYO-bucket positions are not affected while key release remains server-side. Guest windows are already declared unavailable from client-first, but same-space roles and per-row ACLs remain affected. Client-first feeds inherit the sender-device trust assumption.

**[proposed] Proposed pin.** Choose and expose one of two honest models. Model A: client-first spaces are cryptographic trust domains; every enrolled key holder may read every sealed value in the space, and guards constrain supported operations but not a hostile device. The compiler and custody consent page must reject or loudly flag client-first conversion where members/ACLs imply mutually distrustful row views. Model B: preserve hostile-member isolation with cryptographic compartments—per-principal or per-ACL content keys, envelopes for each authorized set, rekey on membership/ACL changes, and a sync protocol that never sends decryptable row material outside that set. Model B needs an explicit scalability/revocation analysis; “one SMK per space” is insufficient.

**[proposed] Fix must not break.** Client-first/offline operation, server blindness, deterministic artifacts, custody conversion disclosure, revocation honesty, and ordinary managed-custody guards must remain intact.

**[proposed] Prior X/SH closure state.** SH-8 is marked closed after device enrollment, removal, rotation, and escrow were pinned. SH3-001 is a relocated consequence of that closure: enrollment now has a ceremony, but the authority conveyed by the wrapped space key is broader than the row authority the design continues to advertise. No prior X/SH item closes hostile-key-holder row confidentiality; state: **relocated/open**.

### [proposed] SH3-002 — High — manifest “narrowing” ignores write, guard, trigger, and egress authority

**[proposed] Exact design claim.** `security-logging-design.md` §5 and `definition-language.md` §8 say a capability grant pins each op's manifest hash plus its ordered output field set; when the hash differs, an incoming field-set subset is “narrowing” and applies silently, while a superset or disjoint set is expansion and kills the grant. `ordo-identity` has not implemented that richer form yet: `CapabilityGrant` holds one combined `ManifestHash`, currently FNV-1a 64, but the finding is against the documented target semantics, not only the unfinished crate.

**[proposed] Concrete attack path.** A supply-chain actor publishes an update to an allowlisted write op. Version 1 updates `invoice.status` and returns `[id, status]`. Version 2 performs the same update, also writes `payments.approved = true` or emits an outbound intent, and still returns `[id, status]`. Its manifest hash changes but its ordered output field set is equal. Equality is a subset under the stated rule, so the change is treated as narrowing; if “subset” meant strict subset, the equal-set/hash-changed case is undefined. The same attack removes an access guard, adds `unguarded`, broadens a routine trigger, changes create/converge/delete mode, or adds sealed-field key release without changing output fields. A valid old token silently authorizes new effects.

**[proposed] Affected custody positions.** Managed, BYO-DB, BYO-bucket, and client-first are affected because grants pin logical artifacts rather than storage placement. Guest/feed exposure is affected when the op or routine backs the shared surface. Package and tier-2 integrations are the highest-risk callers.

**[proposed] Proposed pin.** Define a componentwise authority manifest, not an output projection comparison. At minimum it contains: read entity/field scope; sealed key-release scope; write entity/field scope per statement; mode and delete/subtree power; effective guards and every bypass marker; input bounds; output fields; refinement surface; egress sinks/payload class; routine trigger provenance and row anchor; and callable composition closure. Silent narrowing is allowed only when every authority component is monotonically no broader and no new component appears. Equality of output fields says nothing about other components. Hashes authenticate the canonical manifest; the manifest lattice classifies it. Production hashes are collision-resistant and per op.

**[proposed] Fix must not break.** Silent application of genuine narrowing, deterministic deploy diffs, per-op grant isolation, derived consent pages, package updates, and client-first artifact parity must remain intact.

**[proposed] Prior X/SH closure state.** SH-27 is marked partial, with review 2 explicitly calling the field-set semantics correct and only the FNV primitive open. This review disputes that closure: the primitive remains open and the semantic classifier is also open. State: **open; prior partial closure rejected**.

### [proposed] SH3-003 — High — step-up is attached to the client, so callers can shop for a weaker client

**[proposed] Exact design claim.** `definition-language.md` §8 places `stepUp [transfer.approve, space.delete]` inside an identity `client` block. `security-logging-design.md` §5 says step-up is “demanded by client config.” The same spec says full export and deletion “always exist” and are owner-only and step-up gated. `ordo-identity::AccessEngine` stores a flat `step_up_ops` set, but its comment says the set is resolved “from the calling client's `stepUp […]` config.”

**[proposed] Concrete attack path.** A deployment exposes the same op through two registered relying parties: the main PWA lists `space.delete` and `space.export` in `stepUp`, while an older mobile client, CLI, newly added app, or compromised client definition omits them. A malicious tenant or network adversary holding a token for the weaker client calls the op through the common `/op/{name}` route. If the required set is selected from the caller's client configuration, the op no longer demands fresh presence. The supposedly invariant exit-op gate becomes a client preference. A stolen access/refresh token chooses the weakest client rather than defeating WebAuthn.

**[proposed] Affected custody positions.** All custody positions are affected, including BYO targets and client-first, because deletion/export/device enrollment/custody conversion are control-plane operations. Every app/client sharing the issuer creates an alternate path.

**[proposed] Proposed pin.** Required step-up is intrinsic to the operation artifact or immutable platform-op definition. `space.delete`, full export, custody conversion, device enrollment/removal, recovery-mode changes, authenticator linking, and break-glass cannot be weakened by any client. A client may add stronger step-up requirements for its own use but never subtract the op minimum. Access tokens bind `iss`, `aud`, and authorized-party/client id; the door checks them, while the op requirement is looked up independently of the chosen client.

**[proposed] Fix must not break.** One issuer/many apps, configurable client UX, capability tokens being structurally unable to satisfy human presence, typed door errors, and owner-controlled exit must remain intact.

**[proposed] Prior X/SH closure state.** SH-18 remains open for bearer binding and boolean presence freshness. SH3-003 is a distinct policy-location failure adjacent to SH-18; even perfect proof-of-possession does not help if a weaker client removes the requirement. State: **open**.

### [proposed] SH3-004 — High — the ACL bootstrap fix gives every space owner unilateral access to every ACL-protected row

**[proposed] Exact design claim.** `definition-language.md` §3 closes ACL bootstrap with: “Space owners can always administer any acl row in their space, regardless of level.” The access model repeats that owners “may administer any acl row.” `traps.md` presents this as the escape hatch for departed admins. Derived admin operations can grant the owner the top ACL level, and std/authored reads then pass the row guard normally.

**[proposed] Concrete attack path.** A malicious tenant with the owner role encounters an ACL-protected HR case, medical note, whistleblower report, private board, or member journal for which they hold no row. They invoke the universal admin path to grant their own members row the top level, then call the ordinary guarded read or sealed-inclusive authored op. Every step is authorized and command-logged; there is no denial or anomaly. The ACL did not protect the row from the most powerful tenant actor—it only made the access a two-command sequence. Export is even broader and reaches the same result for the whole space.

**[proposed] Affected custody positions.** Managed, BYO-DB, and BYO-bucket are affected at the authorization layer. Client-first is already weaker under SH3-001. Guest windows can expose rows after the owner self-grants and re-shares. Feeds can make the copy durable elsewhere.

**[proposed] Proposed pin.** First decide the product contract. If owners are sovereign over all space data, state structurally that no ACL is private from an owner: render the owner override in every ACL-derived manifest, share/custody page, `callersOf`, and generated privacy copy. If private-from-owner rows are required, replace immediate universal administration with a recovery capability that does not itself decrypt: predeclared recovery quorum or row-holder approval, time delay, mandatory notifications to current holders, reason, and an independently visible break-glass event. System/routine-created orphan rows can instead require an explicit owner-assignment policy at creation.

**[proposed] Fix must not break.** Day-one ACL bootstrap, recovery when all row admins leave, owner exit rights, deterministic derived guards, command attribution, and a state from which data is administrable must remain intact.

**[proposed] Prior X/SH closure state.** X-005 was fatal and is now treated as closed by creator auto-grant plus universal owner administration. The creator auto-grant closes day one; the universal fallback relocates the failure into owner-confidentiality semantics. State: **closed for availability, relocated/open for confidentiality**.

### [proposed] SH3-005 — High — suspension is modeled as display state, not an access-check input

**[proposed] Exact design claim.** `definition-language.md` §3 defines the platform `members` entity with `status: active / suspended` and says suspension is a command, not a delete. `security-logging-design.md` §6 defines the human door as membership role plus groups plus profile, without a suspension step. The current `ordo-identity::AccessEngine` stores membership as `(space, principal) -> Role`; `check` treats its presence as active membership and has no status or session-revocation input.

**[proposed] Concrete attack path.** An operator or owner suspends a compromised user after a stolen-device report. The suspension command changes the members row and may update the UI, but an already minted access or refresh token still resolves to a membership role in the door's stated pipeline. The attacker continues calling allowed ops, holding live streams, and minting ordinary command history until token/session expiry. In client-first, the same omission delays key-epoch rotation because the design pins rotation to membership/device removal, not clearly to suspension.

**[proposed] Affected custody positions.** All server-mediated custody positions are affected by continued calls. Client-first is additionally affected for new-epoch access and sync acceptance. Guest suspension/revocation needs the same state transition. Feeds and source principals are not human members but need equivalent enabled/revoked status in their own grant path.

**[proposed] Proposed pin.** Membership state is an authorization input checked before role/profile resolution: only `active` continues. Suspension atomically revokes or marks all sessions/refresh-token families for that principal+space, closes live subscriptions, blocks sync proposal acceptance, and—where the suspended device held client-first keys—opens a new epoch for remaining devices. Define reactivation as a separate step-up-gated command; it must not silently revive old refresh tokens or old wrapped keys.

**[proposed] Fix must not break.** Audited suspension as a reversible command, no membership deletion requirement, fast door checks, offline honesty for already-copied plaintext, and per-space scoping must remain intact.

**[proposed] Prior X/SH closure state.** No prior X/SH finding directly closes suspension enforcement. SH-30 covers live re-check after revocation and remains open; SH-8 covers device removal, not suspension. State: **open**, with dependencies on open SH-30 and closed-but-narrow SH-8.

### [proposed] SH3-006 — High — authenticator linking can turn a transient session theft into permanent account takeover

**[proposed] Exact design claim.** `security-logging-design.md` §3 says link/unlink are audited commands and refuses unlinking the last authenticator. It does not require fresh presence, proof of control of the existing authenticator, proof of possession of the new authenticator in the same ceremony, notification, or session-family rotation. The current `Directory::link(principal, authenticator, at)` checks only that the principal exists and the authenticator is not already linked; the caller is responsible for logging the returned event.

**[proposed] Concrete attack path.** A network adversary steals a short-lived bearer token, or a thief opens an unlocked mirrored device. Before the session expires, the attacker links their passkey/OIDC subject to the victim principal. The link is logged but succeeds without a pinned reauthentication ceremony. After the original token expires or the device is recovered, the attacker signs in through the newly linked authenticator indefinitely. They can then enroll a client-first device, receive future key wraps, and turn a transient compromise into durable data access.

**[proposed] Affected custody positions.** Identity compromise reaches every custody position. Client-first has the additional consequence of durable device-key enrollment and future epoch access. BYO storage does not help because the attacker enters through the authorized API identity.

**[proposed] Proposed pin.** Linking requires a transaction-bound dual proof: fresh step-up with an already linked authenticator plus successful possession ceremony for the new authenticator, both bound to the same principal, client, nonce, and short expiry. Recovery when the old authenticator is unavailable is a separate, explicitly chosen recovery path with delay/notification. Link/unlink sends out-of-band notification to all existing authenticators, rotates refresh-token families, and offers immediate rollback that cannot unlink the last known-good factor. The command log records the ceremony id and method classes, never secrets.

**[proposed] Fix must not break.** Multiple authenticators per human, pluggable providers, anti-enumeration, invite/auto-onboard, the last-authenticator lockout guard, auditability, and explicit recovery choice must remain intact.

**[proposed] Prior X/SH closure state.** SH-18 remains open for bearer-token proof of possession; this finding describes what a stolen bearer can make durable. SH-8 closes device enrollment ceremony but does not stop account-authenticator persistence before enrollment. State: **open**.

### [proposed] SH3-007 — Critical — opaque proposal sequencing cannot enforce CAS, guards, checks, or truthful history

**[proposed] Exact design claim.** `implementation.md`, “The merge rule,” says the server sequences sealed device proposals in arrival order; proposals become history only when sequenced; conflicting later proposals are replayed by their device through ordinary CAS; “A rejected command never becomes history; only the accepted replay does.” The same document says the server cannot read values. `definition-language.md` promises write guards, `check` invariants, mode derivation, before/after images, and command-log truth for every write. `ordo-commandlog::CommandGroup` validates only non-empty/single-space/image shape; semantic validation belongs above it.

**[proposed] Concrete attack path.** Two honest offline devices submit proposals against the same revision. The server cannot decrypt either payload, evaluate the row revision, run a write guard/check, or know whether the claimed before/after image is truthful. If it sequences both first, the losing command has already become history, contradicting “a rejected command never becomes history.” If it waits for device replay before sequencing, arrival order is not yet canonical and a malicious/stolen device can withhold the verdict forever. More directly, a hostile enrolled device submits ciphertext claiming a valid converge with a fabricated `rev`, bypassed guard, invalid check, or false before-image. The blind server can authenticate the device and envelope but cannot establish semantic validity; accepting it corrupts state/history, while asking that same hostile device to validate itself establishes nothing.

**[proposed] Affected custody positions.** Client-first is directly affected. Managed and BYO positions retain server-side semantic enforcement. Client-first feeds can carry state derived from unvalidated local history into a receiving space.

**[proposed] Proposed pin.** Separate availability ordering from semantic acceptance. A server may append an immutable receipt for an opaque proposal, but it must not call that receipt an accepted command or include it in authoritative state history until a verifier independent of the submitting device proves validity. Viable choices are: trusted execution/attested client verifier; cryptographic proofs over committed state (substantial new machinery); multi-device/authorized-quorum countersignature for protected transitions; or an honest scope reduction in which client-first guarantees confidentiality and tamper-evident transport but not server-enforced guards/checks/CAS against an enrolled malicious device. Rejected proposal receipts remain auditable without pretending they changed state. Bind every accepted content signature to space, principal, device, op/version, base state commitment, params/effects commitment, and nonce.

**[proposed] Fix must not break.** Offline proposals, deterministic canonical order, no CRDT requirement, visible conflicts, server blindness, per-space audit/exit, content signatures, and append-only history must remain intact. Rejection must remain history of an attempted proposal, never history of a state mutation.

**[proposed] Prior X/SH closure state.** Consolidated question Q2/X-058 was treated as resolved by server arrival order, and SH-13's owner-verifiable proof remains open. The ordering decision closes forks among honest clients but relocates the unsolved question to semantic acceptance by hostile clients. State: **relocated/open**.

## [proposed] Cross-finding priorities

- **[proposed] P0 before claiming client-first parity:** resolve SH3-001 and SH3-007 together. The same trust-boundary decision governs confidentiality, authorization, invariants, sync, and audit truth.
- **[proposed] P0 before third-party packages or capability tokens carry valuable writes:** replace output-field narrowing with the componentwise authority manifest in SH3-002.
- **[proposed] P0 before exit/device/recovery operations ship:** make step-up intrinsic to ops (SH3-003), suspension enforceable (SH3-005), and authenticator linking a dual-proof ceremony (SH3-006).
- **[proposed] P1 before marketing ACLs as privacy inside a tenant:** decide and disclose whether owners are universal readers, or replace the immediate owner escape with constrained recovery (SH3-004).

## [proposed] Closure test

**[proposed] A future review should not close these findings from prose alone.** Closure requires executable architecture tests: a hostile client that bypasses the SDK; an unchanged-output write whose effects expand; two clients with different step-up declarations; an owner attempting to self-grant a private row; a suspended principal with live/access/refresh/sync paths; a stolen session attempting authenticator linking; and conflicting opaque proposals where neither submitting device is trusted. The expected result must be derived from the same artifact/state the runtime enforces.
