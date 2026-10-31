# Contributing to Ordo

Thanks for looking. Read this page before opening anything. Ordo is spec-first
and pre-alpha, which changes what a useful contribution looks like.

## The state of things

The specification is ahead of the software. The engine is at the parser stage:
there is no planner, no executor, no `ordo apply`, no SDK, no release. Most of
what the docs describe does not execute yet.

**The spec moves.** Sections marked as leaning, and open items marked in place,
will change, sometimes in ways that invalidate work built on them. If you are
about to spend a weekend on something, open an issue first and get the design
question settled before you write the code. Nobody enjoys a rejected pull
request that was correct against last month's grammar.

## What a good first contribution looks like

Because the project is spec-first, the highest-value contributions are not code:

- **Conformance cases.** A case that pins behaviour the spec describes but does
  not yet test, especially a case that *should* fail and says why. Follow the
  existing numbering and structure in `docs/composition-conformance.md` and
  `docs/guards-conformance.md`.
- **Corpus additions.** Translation pairs in `docs/sql-translation-corpus.md`
  and `-2.md`: a definition, the SQL it should produce, and the reason. Real
  shapes from real applications are more useful than synthetic ones.
- **Documentation fixes.** Contradictions between documents, terms used before
  they are defined, examples that no longer parse against the current grammar.
  These are found by reading, and reading is what the project needs most.
- **Traps.** If the grammar surprised you, that surprise belongs in
  `docs/traps.md` in the existing format: the trap, why it exists in one line,
  the fix.
- **Engine work.** Welcome, but coordinate first. The crate boundaries are
  still moving and the parser is the only settled surface.

## How to contribute

1. Open an issue describing the problem or the case before writing anything
   substantial. For typo-scale fixes, skip straight to the pull request.
2. Branch, make the change, keep the diff focused on one thing.
3. `cargo fmt`, `cargo clippy` and `cargo test` must be clean for code changes.
   `unsafe` is forbidden workspace-wide and that is not negotiable.
4. Documentation changes follow the surrounding voice: terse and declarative.
   Say what is true and what is not yet true.
5. Open the pull request. Explain the *why*. The diff already shows the what.

## The CLA, in thirty seconds

**Every contribution requires a signed Contributor License Agreement.** It is
the [Apache Individual Contributor License Agreement v2.0][icla-src]. It is the
standard one, unchanged in substance, and it lives in [CLA.md](CLA.md).

**What signing does:** it gives this project a licence to use, ship and
sublicense the code you submitted. It is a permission slip on
top of the rights you already have.

**What signing does not do:**

- **You do not assign ownership.** You keep the copyright in your
  contribution. Sign the CLA and you can still use, publish, sell and
  relicense your own work however you like, to anyone, forever. The CLA grants
  a licence alongside yours and does not take yours away.
- **It does not touch anything else you have written.** Only what you
  deliberately submit here.
- **It cannot be used to take the project private.** Every line already
  released under AGPL-3.0 or Apache-2.0 stays released under it, in every copy
  already distributed. That grant is irrevocable and no future decision can
  claw it back. If you dislike where this goes, the fork is always there, by
  design.

**Why it exists.** There are two reasons:

1. **So the project can relicense more permissively later.** The engine is
   AGPL-3.0 today. If a more open licence turns out to serve it better, the
   CLA makes that a decision instead of an archaeology project requiring every
   past contributor's consent. The intent is to become more open, but either
   direction is impossible without it.
2. **So commercial licences can be offered.** Some organizations cannot accept
   AGPL in their dependency tree. Selling them the same code under different
   terms requires the right to license the whole work, your part included.

**How to sign:** fill in the form at the bottom of [CLA.md](CLA.md) and email
it to **secobs@hominem.ai**. A typed name is fine. You do this once, on your
first pull request, and never again. It covers everything you contribute,
before and after.

**If you do not sign:** your pull request cannot be merged. Nothing else
happens: no hard feelings, and your issue reports, design arguments, bug
reproductions and reviews are all still welcome and need no paperwork at all.
Only merged code needs the signature.

**If your employer owns what you write** (check your employment contract,
most do), see section 4 of the CLA and write to `secobs@hominem.ai` for a
Corporate CLA.

[icla-src]: https://www.apache.org/licenses/icla.pdf

## Which licence covers what you are writing

Your contribution is licensed under the licence covering the path you touch:

- `crates/` and the server: **AGPL-3.0-only** (`LICENSE`)
- `docs/`, the specification and conformance corpora: **Apache-2.0**
  (`LICENSE-APACHE`)
- future `sdk/` and `builder/` paths: **Apache-2.0** (`LICENSE-APACHE`)

The full map is in `NOTICE`; the reasoning is in `docs/licensing.md`.

**Nothing you contribute creates any claim over what Ordo's users generate with
it.** Their SQL, DDL, plans, generated code and exports are theirs; see the
output disclaimer in `NOTICE`.

## Conduct

Be decent. Argue about designs, not about people; assume the other person read
the same document differently and acted in good faith; do not make the issue
tracker a worse place to be than it was yesterday. Harassment, personal attacks
and deliberate bad-faith participation get you removed, and the maintainer is
the one who decides.

That is the summary. [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) is the whole of
it, and it adopts the [Contributor Covenant v2.1][cc] for everything the
summary does not cover.

[cc]: https://www.contributor-covenant.org/version/2/1/code_of_conduct/

## Contact

Conduct concerns, CLA questions, security reports and commercial-licensing
enquiries: **secobs@hominem.ai**.

Do not report security issues in a public issue.
