# Attack catalog

Every attack the security round of 2026-10-24 ran, one JSON object per line,
held or broke. Each file is one tester: Opus pen tests (`pentest-*`), Fable
pen tests (`fable-*`) and the deployment audit (`audit-deploy`).

Fields: `id`, `surface`, `technique`, `category` (OWASP or CWE), `actor`,
`setup`, `request` (exact, with placeholders such as `{TOKEN_A}`),
`expected_safe`, `observed`, `result` (`held`, `broke`, `unclear`), `finding`,
`replayable` (`http`, `browser`, `needs-code`).

The `http` entries are replayed as rows of the authorization matrix
(`crates/ordo-host/tests/authz_matrix.rs`, `scripts/authz-matrix.sh`), each
row carrying its catalog id. Reports and verification evidence are not kept
here; the confirmed findings are the board's Reported fixes.
