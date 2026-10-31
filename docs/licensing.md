# Ordo Licensing

Why Ordo is licensed the way it is, and what it means for you. The operative
files are `LICENSE` (AGPL-3.0-only), `LICENSE-APACHE` (Apache-2.0) and `NOTICE`
(the path map). This document is the reasoning; where it disagrees with those
files, those files win.

**It is also not legal advice.** It is one project's reading of its own licence,
written to be useful and not exhaustive. Some questions turn on facts about
*your* deployment. The sharpest is who counts as a "user interacting over a
network" under AGPL §13. For those, this document says so and stops, because it
is not in a position to give you a confident answer.

---

## Three objects, three answers

Licensing arguments go in circles because people are arguing about different
objects. There are exactly three here, and each gets a different answer.

| Object | What it is | Licence | Because |
|---|---|---|---|
| **The engine** | `crates/`, the server that runs | **AGPL-3.0-only** | It is a service. Modify it, offer it to others as a service, publish your modifications. |
| **The spec & the surfaces** | `docs/` (specification, conformance corpora), future `sdk/`, future `builder/` | **Apache-2.0** | Developers link these into their own code. Copyleft here is a non-starter. |
| **Your output** | The SQL, DDL, plans, generated code and exports Ordo produces from your definitions | **No licence at all. It is yours.** | A compiler has no claim on what it compiles. |

### The engine: AGPL-3.0

Ordo is a backend platform. The realistic way to take it without giving
anything back is to host it as a service, which is the case AGPL §13 covers and
GPL does not.

### The spec, the corpora, the SDKs, the builders: Apache-2.0

**A copyleft client SDK is unusable.** The SDK is generated code that gets
compiled into a proprietary application. If that carried a copyleft obligation,
no company would adopt Ordo and none should. The licence would be a trap
disguised as a library. The same holds for the CDK/builder frontends, which are
TypeScript that lives in the user's own repository.

The specification and the conformance corpora are Apache-2.0 for a different
reason: **an encumbered specification is not a specification.** A second
implementation of Ordo must be legally possible, including a commercial closed
one. Without that, the "specification" is documentation of one vendor's product.
The conformance corpora exist to make that second implementation *checkable*,
which only works if anyone may run them.

Apache-2.0 was chosen over MIT for the explicit patent grant and
patent-retaliation clause. It is the more careful permissive licence, at the
cost of one extra paragraph in a NOTICE file.

### Your output: nothing

**Everything Ordo produces from your definitions (SQL, DDL, migration and
deployment plans, registry and interface artifacts, generated SDK and UI code,
and exports) belongs to you, and this project claims no licence over it.
Running Ordo, or compiling your definitions with it, places no licence
obligation of any kind on those outputs, on your definitions, or on the
applications you build with them.**

This is the same intent as the GCC runtime library exception, stated in plain
language. A copyleft compiler must never make the compiled program copyleft.
Ordo generates DDL, an entire API surface, a UI and SDK bindings, and
generation must not become contamination.
Your `.ordo` definitions are yours; every artifact derived from them is yours;
the fact that AGPL code did the deriving is irrelevant to your rights in the
result.

The verbatim wording lives in `NOTICE`, which is the file to cite.

---

## What AGPL actually requires of a self-hoster

The common fear is bigger than the obligation. Concretely:

**First, a disclaimer that has to come before the table, because the row it
qualifies used to be stated as a flat "nothing":** this section is the
project's reading of its own licence and is not legal advice. One row below
turns on a question that only your counsel can answer for your deployment. The
question is *who counts as a "user interacting with the software over a
network"*.

| You are… | You owe |
|---|---|
| Running stock Ordo internally for your own company | **Nothing.** No source to publish, no notice to file, no one to tell. |
| Running stock Ordo as a service for your customers | **Nothing.** It is unmodified; the source is already public. |
| Running *modified* Ordo internally | **Probably an offer of source to whoever interacts with it. Get advice.** See the note below; the old "nothing" here was wrong. |
| Running *modified* Ordo as a service others interact with over a network | **Publish your modifications** to those users, under AGPL-3.0. Your patches, not your data, not your `.ordo` definitions, not your application code. |
| Building an application whose backend is an Ordo deployment | **Nothing.** Your application talks to Ordo over the wire; it is a user of a service, not a derivative of it. |
| Linking the engine crates directly into your binary (embedded mode) | **This one is real.** Linking AGPL crates into your program makes your program AGPL. If you need embedded mode in a closed application, that is what the commercial licence is for. |

**The modified-internal-use row, corrected.** An earlier version of this table
said you owe nothing for running a modified Ordo internally, on the reasoning
that internal use is not distribution. That reasoning is right about the GPL and
insufficient for the AGPL. **AGPL §13 does not turn on distribution at all.**
It triggers when a modified version is *"modif[ied] to be remotely interacted
with through a computer network"* and requires an offer of the Corresponding
Source **to all users interacting with it remotely**. The FSF's own GPL FAQ
takes the same line for modified AGPL software running on a website. So the
only honest statement is conditional:

- If genuinely **nobody interacts with it over a network** (a modified build
  you run locally, or a batch job with no remote users), §13 has nothing to
  fire on, and you owe nothing.
- If your **employees, contractors, or subsidiaries** use it over the network,
  whether they are "users" for §13 purposes is a real legal question with no
  categorical answer this document can give. Note that the obligation, if it
  applies, is narrow: offer *those* users the Corresponding Source of *your
  modifications*. It is not publication to the world, and it never reaches your
  data, your `.ordo` definitions, or your application code.
- **The safe default is cheap:** put your patches in an internal repository your
  users can reach, and the question stops mattering.

The embedded-mode row is the other honest edge. Linking is the case where AGPL
reaches into your code. Client mode, which is the overwhelmingly common one,
does not.

Not owed under any row: your `.ordo` definitions, your data, your schema, your
generated SDK, your UI, your business logic, your customer list.

---

## What the commercial licence is for

One thing: organizations whose policy forbids AGPL in their dependency tree, or
who need embedded mode inside a closed product. They buy the same code under
different terms. Alongside it, there is a hosted Ordo platform for people who
would prefer not to run it.

The commercial licence does not give a better version. There is no feature
held back from the open-source engine, no enterprise-only module, no licence
key checking for paid capabilities. The open engine is the whole engine. If
that ever changes, this paragraph is where you should expect to see it.

The AGPL grant does not depend on any of this. It stands on its own, in every
copy already distributed, permanently.

---

## Why there is no gate and no "source available" tier

No click-through, no registration, no email wall, no redistribution
restriction, no BSL, no "open core," no SSPL. `git clone` is the entire
onboarding.

This is partly ideology. It is also a product argument, and it is load-bearing:

**Ordo asks for an unusually large commitment.** You learn a closed grammar with
no Stack Overflow corpus, you let a compiler derive your schema, your
permissions and your audit trail, and you put your production data behind it.
Nobody sane makes that bet against a vendor who can revoke the terms.

So the exit story has to be real, and it has three legs:

1. **Export.** Full space export yields plain DDL, per-entity JSONL and a
   chain-verifiable command-log archive. It is a documented op that needs no
   support ticket. (`migration.md` §6, "Migration out".)
2. **Self-host.** The engine is the whole engine, under a licence nobody can
   take back.
3. **Fork.** If this project stalls, dies, or makes a decision you cannot live
   with, you take the code and continue. That is insurance, and insurance that
   requires the insurer's permission is not insurance.

A registration gate or a source-available licence would break leg 3 and hollow
out leg 2, in exchange for lead-gen. That is a bad trade for a project asking
for this much trust. The CLA (see `CONTRIBUTING.md`) exists to keep the *option*
of relicensing more permissively open. It cannot close the project, because
what is already published cannot be unpublished.

---

## How the hosted platform relates

The hosted platform runs ahead of the public release. New capability lands
there first and reaches the open repository on a lag.

This is a **release cadence and carries no licence restriction**:

- Nothing is withheld permanently. There is no capability that only the hosted
  version will ever have.
- The lag is a fact about when code is published. It places no limit on what
  the published code may do. Nothing in AGPL-3.0 obliges anyone to publish work
  in progress, and nothing here restricts what you do with what *is* published.
- What you self-host is the full engine. There is no "community edition."

If you need to be on the newest work the day it exists, use the hosted
platform. If you need control, self-host, at the cost of being some months
behind. Both are legitimate. Pick one.

---

## Deployment identity: the apex domain is configuration

This section covers the one piece of "who is Ordo" that is **not** a property of
the project. It belongs to your deployment, and it is the only such value that
is expensive to change later.

**The apex domain of an Ordo deployment is supplied by the environment.** It is
not hardcoded in the engine, not compiled in, and not a property of this
repository. Nothing in the licence, the specification or the code names a
domain. The hosted platform run by Hominem will start on `ordo.hominem.ai` and
may later move. That is a fact about one deployment and does not describe Ordo.
Your deployment's apex is whatever you configure it to be.

> **⚠️ Changing the apex domain strands every existing passkey.**
>
> The passkey relying-party id is the deployment's registrable apex domain.
> That is a WebAuthn requirement and is not an Ordo choice. Credentials are bound by
> the browser and the authenticator to the RP id they were registered under. If
> the apex changes, every passkey registered against the old one stops
> resolving, for every user, permanently. There is no migration, no re-binding
> ceremony, and no server-side fix: the credentials live in hardware you do not
> control. Affected users must re-register from a surviving factor.
>
> **This is a one-time decision per deployment.** Pick the apex before you
> onboard the first user, and treat it as immutable afterwards.

Consequences:

- **Do not put a deployment on a domain you are renting a decision about.** If
  there is any chance the project or product name changes, settle that first.
- **A subdomain move can cost you.** The RP id is the *registrable* apex,
  so `a.example.com` → `b.example.com` is survivable, but
  `ordo.hominem.ai` → `ordo.dev` is not.
- **Keep a second factor enrolled.** Email OTP or a federated issuer is the
  recovery path if something outside your control forces an apex move, such as
  a lost domain, a trademark dispute or an acquisition.
- **Ops docs must never bake the domain into an example as if it were real.**
  `implementation.md` states the same requirement from the deployment side.

### The variable

Nothing in the corpus names this variable yet. **Proposed:
`ORDO_APEX_DOMAIN`**. It is marked as proposed and is not settled. It carries
the registrable apex only (`example.com`, not `https://example.com/` and not
`app.example.com`), and the engine derives the passkey RP id, cookie domain and
issued issuer identity from it. The engine should **refuse to start** if it is
unset, and should default to `localhost` only in an explicit development mode.
A deployment that silently picks its own RP id silently strands passkeys the
first time the default changes.

If the engine work lands a different name, that name wins and this section
should be updated to match it.

---

## FAQ

**Can I fork it?**
Yes. AGPL-3.0 on the engine, Apache-2.0 on the spec and surfaces, no CLA
required to fork, no permission to ask. If you offer your modified fork as a
service, publish your modifications to its users. If your fork is only ever run
where **nothing interacts with it over a network**, §13 has nothing to fire on
and you owe nothing further. Read the modified-internal-use note above
before assuming your case is that one, because "internal" and "no remote users"
are different statements. Rename it. The name is the only thing that is not
yours.

**Can I run it for my customers?**
Yes, including commercially, including competing with the hosted platform. If
you modify the engine, your customers get your modifications under AGPL. If you
run it stock, you owe nothing at all.

**Do I owe you anything for what it generates?**
No. Not a licence, not attribution, not a fee, not a notice file. The SQL, DDL,
plans, generated SDK and UI code, and exports are yours outright. See the
disclaimer above and in `NOTICE`.

**What if you abandon it?**
The code stays licensed exactly as it is, and that grant is irrevocable. The
spec and the conformance corpora are Apache-2.0, so a successor implementation
is legally clean and *verifiable* against the same cases. Your data comes out
through export ops in plain formats. Abandonment costs you a maintainer. It does
not cost you your application or your data. This is deliberate: it is the only
honest answer a pre-alpha project can give to "what if you disappear," and the
licensing is arranged so the answer stays true.

**Why AGPL and not MIT?**
The engine is a hosted service, and MIT invites a large cloud provider to host
it, contribute nothing, and end the project. AGPL is the narrowest licence that
closes that specific hole. It is not aimed at users, self-hosters, or anyone
building an application. Permissive terms were chosen for everything you link
into your own code (spec, corpora, SDKs, builders). That is Apache-2.0, because
the copyleft argument does not apply there.

**Is the CLA a trap?**
It is a real ask, and it lets the project relicense later and offer commercial
licences. Owner intent is that any relicensing runs toward
*more* open. It does not transfer your copyright, does not touch your other
work, and cannot retract anything already published. `CONTRIBUTING.md` has the
full statement.

**Do I have to register, sign up, or accept anything to use Ordo?**
No. There is no gate of any kind, and there is not going to be one.

**Is Ordo tied to a particular domain?**
No. The apex domain is per-deployment configuration supplied by the
environment. See "Deployment identity" above. Read it before you
deploy, because the passkey relying-party id is derived from it and changing it
afterwards strands every registered passkey.

**Who holds the copyright?**
Hominem. See `NOTICE`. Contributors retain copyright in their own
contributions; the CLA grants a licence, not ownership.

---

## Contact

Licensing questions, commercial-licence enquiries, CLA questions, conduct
reports and security reports: **secobs@hominem.ai**.

Do not report security issues in a public issue tracker.
