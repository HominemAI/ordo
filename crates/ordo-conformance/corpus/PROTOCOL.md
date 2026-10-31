# The corpus protocol

Four stages, in an order that may not be reversed, each with a machine gate.

1. **Seed** (`fixtures/*-seed.sql`) — written first, frozen. Every
   expectation downstream is derived by reasoning from it.
2. **Wants** (`wants.json`) — what we want to know and what would satisfy
   it, written before any query exists. Frozen: a want may be **retired
   with a reason, never edited**. Two additive layers — `expect`
   (mechanical; code decides) and `judge` (questions code cannot decide;
   a verdict without a `because` fails the build).
3. **Queries** (`sql.json`) — attempts to satisfy the wants. Postgres is
   the truth; a mismatch means the query is wrong. Failure diagnostics
   are phrased in rows, never in queries, so a retry is not anchored.
4. **Ordo** (`ordo.json`) — the same intents in Ordo, written from the
   want, never translated from the SQL. Four verdicts: `equivalent`
   (must match), `compile_error` (fails loudly for a human to attribute),
   `semantic_divergence` (fails loudly, most serious), `inexpressible`
   (pinned in the test; growing or shrinking the set is a deliberate act
   in two files). Inexpressibles are the corpus's product: each one is a
   language ruling waiting to happen — see `docs/corpus-rulings.md`.

## The blind gate

The corpus's founding failure: its author satisfied 37/37 of the original
wants while two isolated authors managed 26 and 27 — eleven wants existed
only in the author's head. Since then:

- **No mechanical want counts until a blind author satisfies it** from its
  wording alone. The author sees the redacted want (id, probes, intent,
  columns, order flag), the schema, and the seed — never the expected
  values, never anyone else's queries. `validation.json` is the ledger;
  `wants.rs` fails the build for any want not in it.
- Validate with `examples/blind_report.rs` over an isolated author's
  answers file. Ambiguity the author reports is a finding about the want,
  not the author — retire and restate.
