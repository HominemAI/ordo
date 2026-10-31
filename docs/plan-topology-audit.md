# Auditing the topology against the market

**A plan for finding what the settled topology cannot do that buyers expect, and separating what it forbids from what nobody has built yet.**

The topology under audit is what this session settled: spaces as the tenancy boundary, per-space routing onto a sparse set of database targets, a control plane beside the tenant databases, a system space for platform-owned content, sealed fields with per-space keys, guards compiled into every query, a per-space hash-chained command log, and windows and feeds as the only cross-space paths.

This is a plan for the audit, not the audit.

---

## 0 · Why this needs a method rather than a feature list

Three failure modes make this kind of review worthless, and the method exists to avoid them.

**Feature-list envy.** Enumerating what Supabase ships and marking the difference produces a list of things to build that nobody asked for. The unit of comparison has to be **a capability a buyer asks for**, phrased as their question rather than as a product's feature name: "can each of my customers have their own encryption key", not "do you have TDE".

**Category confusion.** Ordo is not Convex, Jazz, CipherStash or Neon, and comparing against all four at once produces contradictory conclusions, because each made a different trade. The comparison set must be chosen per capability, naming who is being compared and why they are the relevant benchmark for that one question.

**Conflating three very different gaps.** The audit's whole value is in this distinction:

- **Structural**: the topology forbids it. Cross-space joins are structural: the planner returns a table and its predicate as one value and refuses an unscoped source, so a query reaching two spaces is not a missing feature.
- **Unbuilt**: the topology permits it and nobody wrote it. Per-tenant backup is unbuilt, not forbidden.
- **Refused**: it was considered and declined, with a recorded reason. Order-revealing encryption is refused, not missing.

A finding with no verdict on that axis is not actionable. A structural gap costs a redesign; an unbuilt one costs a sprint; a refused one costs a conversation about whether the reason still holds.

---

## 1 · The dimensions

Eight, each with the buyer question that defines it. The audit answers the question, not the category.

**1. Tenancy operations.** "Can you restore one customer to last Tuesday without touching the others?" Per-tenant backup and restore, point-in-time recovery, cloning a tenant, moving a tenant between targets, deleting a tenant completely, exporting a tenant on exit.

**2. Isolation under load.** "What happens to my other customers when one of them runs a bad query?" Noisy neighbours, per-tenant rate and resource limits, the per-space append lock as a write ceiling, connection fairness, QoS.

**3. Data platform expectations.** "Do I get search, analytics and time-series, or do I need three more systems?" Full-text search, vector search, aggregate and OLAP queries, time-series and telemetry, read replicas, caching.

**4. Schema and change management.** "How do I ship a breaking change to ten thousand tenants?" Migrations, zero-downtime change, per-tenant schema variation, versioned apps, rollback, preview and branch environments.

**5. Identity and access.** "Does this fit our SSO, and can I give a contractor access to one project?" SSO and SAML, SCIM provisioning, service accounts and API keys, delegated administration, scoped tokens, and the expectations RBAC buyers arrive with.

**6. Compliance and data governance.** "Where does my data live, and can you prove what happened to it?" Data residency per tenant, BYOK and HYOK, audit export to a SIEM, retention and legal hold, DSAR and right-to-erasure, evidence for SOC 2 and ISO 27001.

**7. Realtime and offline.** "Does it work on a plane, and what happens when two people edit at once?" Live queries, presence, conflict resolution, offline reads and writes, local cache and its revocation.

**8. Integration.** "How does this talk to the rest of our stack?" Webhooks, change data capture, event streams, connectors, import and export, and what a third party can build against.

---

## 2 · The comparison set, chosen per dimension

Naming the benchmark per capability rather than once, because each of these is the right comparison for different questions and the wrong one for the rest.

- **Convex** for developer velocity, realtime queries and the shape of a reactive backend. The right benchmark for dimension 7 and for 4's ergonomics. Explicitly the wrong benchmark for tenancy, since it has no tenant concept.
- **Supabase** for the Postgres-native expectation set: RLS, connection pooling, search, storage, the edge-function model. The right benchmark for 3 and 5.
- **Jazz** for local-first, cryptographic access control and offline. The right benchmark for 7.
- **CipherStash** for searchable encryption and the documented-leakage posture. The right benchmark for the encrypted half of 3 and 6.
- **Neon and PlanetScale** for branching, preview environments and per-tenant database economics. The right benchmark for 1 and 4.
- **Nile, Clerk and WorkOS** for how B2B multi-tenancy is packaged and what buyers have been taught to expect. The right benchmark for 1 and 5.
- **Firebase** for what a large installed base treats as table stakes in 7 and 8.

Where a dimension has no good comparator, say so rather than inventing one.

---

## 3 · The method

For each capability, in order:

1. **Write the buyer's question**, in their words.
2. **Answer it against the current tree**, with `file:line` evidence, in the discipline the plans in this repo already use. "It exists" and "it is wired" are different answers, and this session found several things that were the first without being the second.
3. **Assign the verdict**: structural, unbuilt, or refused. A refused verdict must cite where the refusal is recorded.
4. **For structural**, state what specifically forbids it and what changing it would cost. This is the section a reader will act on.
5. **For unbuilt**, estimate honestly and name what it depends on.
6. **Name the workaround** a customer would actually be told to use, and whether it is acceptable.

Two rules that keep it honest. **No verdict without evidence**, because the value of this exercise is that it is checkable rather than an opinion. And **record what we do that the comparison set does not**, since an audit that only finds gaps produces a plan to become a worse version of somebody else.

---

## 4 · What I already expect to find, written down now so the audit can disconfirm it

Recording these as predictions makes the audit falsifiable rather than a search for agreement.

**Structural, and the interesting ones.** No cross-space joins, so anything wanting analytics across tenants needs a derived projection. The per-space append lock caps a single tenant's write rate and no amount of hardware raises it. One schema per deployment, so per-tenant app versions need per-space activation. The WAL is cluster-wide, so per-tenant at-rest keys cover tables and indexes and never the write stream. Sealed fields cannot be sorted, ranged or aggregated, and guards cannot read them at all.

**Unbuilt, and probably expected by buyers.** Per-tenant backup and restore. Data residency per tenant, which routing makes reachable and nothing implements. SSO, SCIM and service accounts. Search of any kind. Webhooks and change data capture. The client and its local cache. A reader for either security stream.

**Refused, with reasons on record.** Order-revealing encryption. Public read as a property of data rather than a grant. A second storage engine. Cross-space principal lookup.

**And the ones I am least sure about, which is where the audit earns its keep.** Whether the append-lock ceiling is a real constraint at a real tenant's write rate, which is an unmeasured release gate. Whether buyers in this market actually ask for per-tenant keys or merely for the phrase "encrypted at rest". And whether the no-extension property is worth what it costs, given that it rules out `pg_tde`, `pgvector` and full-text extensions in one stroke.

---

## 5 · Output

One document, `audit-topology-gaps.md`, in the shape of the audits already in this repo: a table of capability, verdict, evidence and cost, followed by the structural findings written out at length, because those are the ones that change plans rather than schedules.

Plus two things the table cannot carry. A short list of **capabilities to refuse deliberately**, so that not building them becomes a position rather than an omission. And the **three questions the audit could not answer from the code**, which are the ones needing a customer conversation rather than a grep.

---

## 6 · What this audit is not

Not a competitive analysis, not a pricing exercise, and not a roadmap. It produces findings with verdicts and costs; what to do about them is a separate decision, and mixing the two is how audits become advocacy.
