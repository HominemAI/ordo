# Security findings: a traceability audit, not a re-review

Owner's instruction, 2026-09-27: *"in terms of leaning — can we just audit to
confirm / trace."*

So this is a **trace**, deliberately. It takes the 31 `SH-n` findings from the
four security hardening reviews and asks one question of each: *what in the
tree answers this, and can I point at it?* It does not re-read the system
adversarially. Nothing here is a new finding; where the answer is "nothing
answers this", that is recorded as untraced rather than dressed up as a
discovery.

> **Corrected 2026-09-28 by `security-review-6.md`, the review this trace
> was the argument for.** Three things in this document were wrong as first
> written, and they are fixed in place below rather than left standing:
>
> 1. **The counts.** The summary said 8 homed and 10 untraced. The tables
>    list **9 homed and 9 untraced**; SH-30 was counted in both. The table was
>    right and the summary was not.
> 2. **"No contradiction between the reviews and the code was found."** True
>    of what a trace looks at and false as a statement about the system. The
>    review found fifteen places where Ordo's own documents disagree with its
>    code, two of them Critical. A trace asks whether a mechanism exists; it
>    does not ask whether the mechanism is right, and this sentence read as
>    though it had.
> 3. **Two guesses, both marked "probably", one of them wrong.** SH-14 was
>    guessed closed and is closed. **SH-18 was guessed closed by the durable
>    credential work and is still open by design**: a `client` claim exists on
>    the token and nothing binds to it.
>
> Dispositions now carried from the review: SH-24 closed (the catalog is
> filtered by the door's own access check, verified live), SH-29 closed in
> the engine and unwired at the edge, SH-22 moot for this build, SH-11
> narrowed, SH-7 and SH-12 open as designed. **SH-2 is still untraced by
> anyone.**

**Why it was needed.** The four reviews were last touched **2026-08-10**,
before custody, the door, guard execution, sealing, blobs, the import path and
the security event stream existed. And only **two** of the 31 ids —
`SH-17`, `SH-20` — appear anywhere in `roadmap-checklist.md`. The X-series
usability findings had rounds 3 and 4 tracking each one to a disposition; the
SH series had no trail at all, so nobody could say which of the 31 were live.

---

## 1 · Result

| Disposition | Count | Meaning |
|---|---|---|
| **Traced, closed** | 13 | A mechanism exists and the file is named below |
| **Traced, homed, unbuilt** | 9 | A checklist item owns it; it is not built yet |
| **Untraced** | 9 | Nothing found by this pass; needs a real look |

**No contradiction between the *prior findings* and the code was found by
this trace**, with one near-miss worth recording (§4). That is a statement
about 31 old findings and nothing wider: `security-review-6.md` §3 lists
fifteen contradictions between the system's documents and its code.

---

## 2 · Traced, closed

| | Finding | Answered by |
|---|---|---|
| **SH-1** | Per-space salt pinned nowhere | **Ruled the other way and recorded.** `custody.rs:37`: blind-index keys stay SMK-derived and **epoch-independent**, because a rotation that moved digests would let `sealed indexed unique` admit duplicates across the rotation boundary — during exactly the revocations that force a rotation. `implementation.md:576` carries the replacement as settled; `glossary.md:549` names the old reading as "a common and dangerous" misconception |
| **SH-3** | `normalize(lower)` + blind index as a dictionary accelerant | `custody.rs:66` pins what a blind index digests (normalized text, or the reserved null sentinel). The keying is per space; the *cost* argument against a dictionary is not restated in code, so this is closed on mechanism and thin on rationale |
| **SH-4** | Tier-0 conditions on sealed fields are an equality oracle | `ordo_api::refine.rs:116` — *"sealed field accepts equality only"*, refused at declaration time rather than served |
| **SH-5** | `not_found` vs `access_guard` non-uniform | `ordo_api::error.rs:135` — `not_found` covers a row that exists *outside the caller's read view*, "which answers the same way, because you learn a row exists only if you may read it" |
| **SH-6** | Page cursors unauthenticated, carrying row values | `ordo_api::page.rs` — HMAC-SHA256 under a key derived from `ORDO_PAGE_KEY`, compared with `ct_eq` |
| **SH-9** | No DEK rotation story anywhere (*"the largest single gap"*) | `KeyProvider::rotate(space, reason) -> Epoch` (`custody.rs:526`), three implementations, and the `keys.reseal` worker registered on the supervisor |
| **SH-13** | Owners cannot verify their own chain | `std.space.chainProof` (`attest_ops.rs`) and `ordo verify --proof`, which checks an exported proof with no database at all |
| **SH-19** | The access check has no space dimension | `MemberAccess::check` resolves per space through `in_space`, against **the space the request named**. Its own comment records the earlier single-space version as the defect |
| **SH-21** | A signed index does not say "signed by whom" | `commandlog/sign.rs` carries `key_id`, and its module doc records the FNV-style fold with no key input as the **past** state |
| **SH-25** | Feed copies survive crypto-erasure, undocumented | `exec/erase.rs:81` — `RETAINED` is the named list of what an erasure deliberately keeps |
| **SH-27** | `ManifestHash` is FNV-1a 64 | `identity/principal.rs:377` — `ManifestHash([u8; 32])` over SHA-256, **length-prefixed**, so concatenation is unambiguous. The remaining FNV in the tree is a Postgres advisory-lock id derived from a constant table name at compile time |
| **SH-28** | Denials land in the operational log, not the command log | Marked `[settled]` by the review itself. `ordo-secevent` is that log, and it is now verified, signed on its own cadence and readable by the owner (7d) |
| **SH-31** | `resolve_or_onboard` mints a principal from any federated login | `identity/session.rs:442` resolves through a `Provisioning` policy, configured by `ORDO_AUTH_PROVISIONING` rather than defaulting open |

## 3 · Traced, homed, unbuilt

Each has an owner on the checklist. None is a surprise; all are listed so that
"unbuilt" is a position rather than an omission.

| | Finding | Home |
|---|---|---|
| **SH-8** | No key-loss story; multi-device wrapping has no revocation | `18h` vault-sync, `f7-vault-plan.md` |
| **SH-10** | Crypto-erasure claimed while backups are unaddressed | Named in 11-10's remainder: a customer's own database backups are outside any op's reach, and that belongs on the BYO-DB consent page **and is not there yet** |
| **SH-15** | Feed re-share laundering has no consent-chain semantics | `11-2` / `11-3` sharing |
| **SH-16** | Guest windows leak schema and membership graph | `11-2` / `11-3` sharing |
| **SH-17** | Replay cache keyed on (op, header) with no principal | Checklist line 4054 — the resolved principal is one of four arguments each owed its own work |
| **SH-20** | A delegation grant is unbounded in volume and breadth | `11-4d`, "SH-20's budgets, the buildable half" |
| **SH-23** | Break-glass is well-designed and unmonitored | `11-26` break-glass, unbuilt, so there is nothing yet to monitor |
| **SH-26** | Payload minimization for outbound sinks asserted, never enforced | `18a` the full egress rail |
| **SH-30** | Live SSE subscriptions access-checked once, at subscribe | No SSE route exists; `live` on the board |

*(Nine rows, and nine is the count.)*

## 4 · The near-miss, recorded because it is the shape to watch for

`custody.rs` says blind-index keys are epoch-independent. `SH-1`'s own text,
and `critique-consolidated.md:694`, say they derive per epoch so that a
rotation re-salts. Those are opposite claims about a security property, and
for about ten minutes this pass treated it as a live contradiction.

It is not. The per-epoch reading was **proposed, ruled against, and
replaced**: `implementation.md:576` carries the replacement marked settled,
`glossary.md:549` names the old reading as a dangerous misconception, and
`engine-plan.md:1697` records that a stale paragraph still asserting it was
found and corrected. The surviving occurrences are inside the review
documents themselves, which are records of what was thought at the time and
must not be edited — the same rule that applies to `X-076`'s hyphenated names.

Worth writing down because a trace that reads only the finding and only the
code, and not the ruling in between, produces a false positive exactly here.

## 5 · Untraced — the actual output of this pass

Nothing in this pass found a mechanism for these. That is not the same as
"open": it means no ID-level trail exists and somebody has to look.

**SH-2** feed copies re-index under the receiver's salt · **SH-7** the mirror
has no storage pins · **SH-11** KeyProvider compromise blast radius unbounded
and unmeasured · **SH-12** the envelope as a behavioural side channel that
survives every custody level · **SH-14** correlation ids resolve across spaces
and revert planning is unscoped · **SH-18** every token is a bearer token,
nothing binds to a client or device · **SH-22** profile templates are the one
permission-shaped thing and nothing bounds them · **SH-24** entity and field
names are sensitive and the catalog treats them as public-to-the-token ·
**SH-29** `AccessProfile` has no per-member overrides

Several of these are probably answered by work done since August —
**SH-18** by 20b's durable credentials, **SH-14** by per-space chains and the
scoped `revert --group` — and that is the point: *probably* is not a trace.
Each needs the same treatment the thirteen above got.

Filed as checklist **17ag**.

## Related

- `security-hardening-review.md` and `-2`, `-3`, `-4` — the findings
- `critique-consolidated.md` §3c — the security section of the merged reviews,
  **not** covered by round 5 and not covered here
- `critique-consolidated-5.md` — the usability re-run
