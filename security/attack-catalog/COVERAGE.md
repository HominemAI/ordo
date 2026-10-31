# Where each attack is guarded

Every id in the attack catalog (335 entries, 7 testers) once, with the guard
that keeps it from coming back. Written for the security round of 2026-10-24.

- **matrix row**: a row of the authorization matrix
  (`crates/ordo-host/tests/authz_matrix.rs`, rows in
  `tests/authz_support/catalog.rs`, run by `scripts/authz-matrix.sh`). The
  row's name starts with the id. Its answer is what is safe on main today,
  which is the ruled behaviour where a defect was since fixed.
- **skipped**: an `http` entry the matrix world cannot express, with the
  reason, and the test that guards it instead where one exists
  (`SKIPPED` in `catalog.rs`).
- **console test**: an `http` entry aimed at the separate console server,
  replayed against a real console in
  `crates/ordo-console/tests/catalog_attacks.rs`, one test per id (the test
  is named by the id, `-` written `_`). The guard checks that the test exists.
- **existing**: a named test or CI job that already guards it.
- **not covered**: no automated test guards it; the reason is given, and a
  finding is on the board (Reported fixes) where there is one.
- The guard test `every_catalog_http_id_has_a_row_or_a_reason`
  (`catalog.rs`) fails when an `http` id is in neither the rows nor
  `SKIPPED`, and when an id is missing from this file. It runs in the
  ordinary `cargo test -p ordo-host --test authz_matrix`.

Guessing limits for passwords and link passwords (`DEPLOY-016`, `DATA-032`)
belong to the guessing-limits rows.

| Id | Surface | Result then | Replay | Guard |
|---|---|---|---|---|
| `DEPLOY-001` | ORDO_HOST_ADDR default | held | needs-code | not covered: a default read from `ORDO_HOST_ADDR` in `runtime.rs` (127.0.0.1:8080); no test asserts the default |
| `DEPLOY-002` | ORDO_CORS_ORIGINS | held | needs-code | not covered by a named test; `crates/ordo-host/tests/auth_door.rs` sets the list. Held at boot when tested |
| `DEPLOY-003` | OPTIONS /op/{name} | held | http | matrix row (`catalog.rs`) |
| `DEPLOY-004` | POST /op/{name} | held | http | matrix row (`catalog.rs`) |
| `DEPLOY-005` | GET /metrics | held | http | matrix row (`catalog.rs`) |
| `DEPLOY-006` | GET /health/deep | held | http | matrix row (`catalog.rs`) |
| `DEPLOY-007` | GET /usage | held | http | matrix row (`catalog.rs`) |
| `DEPLOY-008` | GET /security/alerts | held | http | matrix row (`catalog.rs`) |
| `DEPLOY-009` | GET /security/events | held | http | matrix row (`catalog.rs`) |
| `DEPLOY-010` | GET /health/ready | broke | http | matrix row (`catalog.rs`) |
| `DEPLOY-011` | GET /health/deploy | broke | http | matrix row (`catalog.rs`) |
| `DEPLOY-012` | GET /live | held | http | matrix row (`catalog.rs`) |
| `DEPLOY-013` | GET /auth/methods | held | http | skipped (needs a minimal boot). Existing: `crates/ordo-host/tests/auth_methods.rs` |
| `DEPLOY-014` | /auth/* with ORDO_AUTH_METHODS unset | held | http | matrix row (`catalog.rs`) |
| `DEPLOY-015` | ORDO_AUTH_PROVISIONING default | held | http | skipped (needs the password method and a closed door). Existing: `crates/ordo-host/tests/registrations.rs` |
| `DEPLOY-016` | POST /auth/begin + /auth/complete (password) | broke | http | skipped: covered by the guessing-limits rows |
| `DEPLOY-017` | password challenge id | broke | http | skipped (needs the password method). Fixed in 5158a6de (random challenge ids); no test of the id shape |
| `DEPLOY-018` | $secevents | held | needs-code | existing: `crates/ordo-host/tests/deployment_security.rs` `a_row_is_the_recorded_act_and_never_more_than_the_record` |
| `DEPLOY-019` | boot refusal | held | needs-code | existing: `crates/ordo-env/src/lib.rs` `each_refusal_names_the_variable_to_change`, `every_sealed_secret_hides_its_value_and_so_do_codes_and_credentials` |
| `DEPLOY-020` | boot refusal | held | needs-code | existing: `ordo-env` `every_sealed_secret_hides_its_value_and_so_do_codes_and_credentials` |
| `DEPLOY-021` | boot refusal | held | needs-code | existing: `ordo-env` `the_value_form_and_neither` |
| `DEPLOY-022` | boot refusal | held | needs-code | existing: `crates/ordo-host/tests/db_error_text_scan.rs` |
| `DEPLOY-023` | boot refusal | held | needs-code | existing: `ordo-env` `every_sealed_secret_hides_its_value_and_so_do_codes_and_credentials` |
| `DEPLOY-024` | boot record | held | needs-code | existing: `crates/ordo-host/src/boot_record.rs` `a_secret_value_never_moves_the_fingerprint`, `a_claim_code_or_a_credential_never_moves_the_fingerprint` |
| `DEPLOY-025` | error bodies with DB down | held | http | skipped (needs Postgres stopped). Existing: `crates/ordo-host/tests/db_error_text_scan.rs`, `health.rs` `a_deployment_whose_database_is_unreachable_is_not_ready` |
| `DEPLOY-026` | bootstrap codes | broke | needs-code | not covered: finding F13 (codes printed to logs); on the board as a Reported fix |
| `DEPLOY-027` | ordo_env::SECRET_SETTINGS | held | needs-code | existing: `ordo-env` `every_sealed_secret_hides_its_value_and_so_do_codes_and_credentials` |
| `DEPLOY-028` | Postgres connection | broke | needs-code | not covered: finding F3 (TLS to the database); on the board |
| `DEPLOY-029` | ordo-console launcher | broke | needs-code | existing: `crates/ordo-console/tests/launcher.rs` (`a_forged_parent_is_refused`, `root_as_the_target_is_refused`, `the_consoles_own_identity_as_the_target_is_refused`); fixed by 1edd86b9. Also the images workflow smoke (`.github/workflows/images.yml`) for the one-image child user |
| `DEPLOY-030` | docker console target | held | needs-code | the images workflow smoke (`.github/workflows/images.yml`) and `crates/ordo-console/tests/compose_hardening.rs` `split_services_are_locked_down` |
| `DEPLOY-031` | docker images USER | held | needs-code | the images workflow smoke (`.github/workflows/images.yml`) (non-root user check) |
| `DEPLOY-032` | docker base images | broke | needs-code | not covered: finding F10 (base image digests not pinned); on the board |
| `DEPLOY-033` | compose ports | broke | needs-code | existing: `compose_hardening.rs` `published_ports_bind_loopback` |
| `DEPLOY-034` | compose secrets | broke | needs-code | existing: `compose_hardening.rs` `no_secret_is_made_world_readable` |
| `DEPLOY-035` | compose hardening | broke | needs-code | existing: `compose_hardening.rs` `split_services_are_locked_down`, `one_image_is_read_only_but_keeps_its_privileges` |
| `DEPLOY-036` | cargo audit | broke | needs-code | CI audit job (`cargo audit`, ignores in `.cargo/audit.toml`) |
| `DEPLOY-037` | cargo audit | held | needs-code | CI audit job (`cargo audit`) |
| `DEPLOY-038` | npm audit | held | needs-code | CI audit job (`npm audit --omit=dev`) |
| `DEPLOY-039` | npm audit | broke | needs-code | CI audit job (`npm audit`, dev dependencies) |
| `DEPLOY-040` | lockfiles | held | needs-code | CI audit job (runs against the committed lockfiles) |
| `DEPLOY-041` | CI dependency audit | broke | needs-code | CI audit job (the gate itself, added after finding F5) |
| `DEPLOY-042` | workflow permissions | held | needs-code | not covered: workflow `permissions:` blocks are read by eye; no test |
| `DEPLOY-043` | workflow triggers | held | needs-code | not covered: no test forbids `pull_request_target` |
| `DEPLOY-044` | third-party actions | broke | needs-code | not covered: finding F5 (actions not pinned by sha); on the board |
| `DEPLOY-045` | argon2id parameters | held | needs-code | not covered: argon2id cost is read from code, no test |
| `DEPLOY-046` | password timing | held | needs-code | not covered: password timing by unknown identifier; no test |
| `DEPLOY-047` | test hashers | held | needs-code | not covered: no test asserts the plain hasher is unreachable in a deployment |
| `DEPLOY-048` | randomness | held | needs-code | not covered: no test; code review only |
| `DEPLOY-049` | token signing | held | needs-code | existing: `crates/ordo-identity/src/token.rs` `forgery_of_every_shape_is_the_same_refusal`; matrix rows `ID-001` to `ID-006` |
| `DEPLOY-050` | token key rotation | broke | needs-code | not covered: finding F11 (token key rotation overlap); on the board |
| `DEPLOY-051` | JOSE algorithms | held | needs-code | existing: `crates/ordo-identity/tests/jose_rfc7515.rs` `rfc7515_a5_unsecured_is_refused_by_name`, `rfc7515_a1_hs256_is_refused_by_name` |
| `DEPLOY-052` | code compares | held | needs-code | existing: `auth_email.rs` `an_expired_code_and_a_wrong_code_are_the_one_answer`; constant-time compares are not asserted elsewhere |
| `DEPLOY-053` | recovery codes | held | needs-code | not covered: recovery code entropy and storage; `crates/ordo-host/tests/recovery.rs` runs the flow |
| `DEPLOY-054` | renewal cookie | held | needs-code | matrix row `FID-COOKIE-001` covers the http attributes; the https `__Secure-` form is not covered |
| `DEPLOY-055` | ORDO_REMEMBER_MAX_SECS default | broke | needs-code | not covered: finding F14 (remembered sign-in never ends by default); `crates/ordo-host/src/session_policy.rs` tests the policy layers |
| `DEPLOY-056` | ORDO_CONSOLE_ORIGIN | broke | needs-code | not covered: finding F15 (http console origin on a non-loopback host); on the board |
| `DEPLOY-057` | dev-only keys | held | needs-code | not covered: no test asserts `ORDO_DEV_UNSAFE_TEST_KEYS` is off by default |
| `DEPLOY-058` | guest, self-serve, step-up exemptions, delegation defaults | held | needs-code | existing: the matrix world unsets guest, presence-on-reads, step-up exemptions and delegable ops in `authz_support/mod.rs` `spawn`, so every matrix cell runs against the secure defaults |
| `DEPLOY-059` | crypto-bigint / ctutils | broke | needs-code | CI audit job (`cargo audit`, advisories on crypto-bigint); finding F8 on the board |
| `FDATA-001` | GET /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `FDATA-002` | POST /op/editNote | held | http | matrix row (`catalog.rs`) |
| `FDATA-003` | POST /op/editNote | held | http | matrix row (`catalog.rs`) |
| `FDATA-004` | DELETE /op/deleteNote | held | http | matrix row (`catalog.rs`) |
| `FDATA-005` | GET /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `FDATA-006` | DELETE /op/deleteNote?id= | held | http | matrix row (`catalog.rs`) |
| `FDATA-007` | DELETE /op/deleteNote?id= | held | http | matrix row (`catalog.rs`) |
| `FDATA-008` | GET /op/notesTagged?tag= | held | http | matrix row (`catalog.rs`) |
| `FDATA-009` | GET /op/notesTagged?tag= | held | http | matrix row (`catalog.rs`) |
| `FDATA-010` | GET /op/notesTagged?tag= | held | http | matrix row (`catalog.rs`) |
| `FDATA-011` | POST /op/editNote | held | http | matrix row (`catalog.rs`) |
| `FDATA-012` | GET /download | held | http | matrix row (`catalog.rs`) |
| `FDATA-013` | GET /download | held | http | matrix row (`catalog.rs`) |
| `FDATA-014` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `FDATA-015` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `FDATA-016` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `FDATA-017` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `FDATA-018` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `FDATA-019` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `FDATA-020` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `FDATA-021` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `FDATA-022` | GET /blob/{handle}/meta | held | http | matrix row (`catalog.rs`) |
| `FDATA-023` | POST /op/addAttachment | held | http | matrix row (`catalog.rs`) |
| `FDATA-024` | PUT /blob/attachments/file/{id} | held | http | matrix row (`catalog.rs`) |
| `FDATA-025` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `FDATA-026` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `FDATA-027` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `FDATA-028` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `FDATA-035` | GET /metrics | held | http | matrix row (`catalog.rs`) |
| `FDATA-036` | GET /metrics | held | http | matrix row (`catalog.rs`) |
| `FDATA-037` | GET /security/alerts | held | http | matrix row (`catalog.rs`) |
| `FDATA-038` | GET /security/alerts | held | http | matrix row (`catalog.rs`) |
| `FDATA-039` | GET /security/events | held | http | matrix row (`catalog.rs`) |
| `FDATA-040` | GET /security/events | held | http | matrix row (`catalog.rs`) |
| `FDATA-041` | GET /security/events | held | http | matrix row (`catalog.rs`) |
| `FDATA-042` | GET /usage | held | http | matrix row (`catalog.rs`) |
| `FDATA-043` | GET /usage | held | http | matrix row (`catalog.rs`) |
| `FDATA-044` | GET /usage | held | http | matrix row (`catalog.rs`) |
| `FDATA-045` | GET /op/allNotes?live=1&sync=1 | held | http | matrix row (`catalog.rs`) |
| `FDATA-046` | GET /op/allNotes?live=1 | held | http | matrix row (`catalog.rs`) |
| `FDATA-047` | GET /op/allNotes?$refine= | held | http | matrix row (`catalog.rs`) |
| `FDATA-048` | GET /op/allNotes?$refine= | held | http | matrix row (`catalog.rs`) |
| `FDATA-049` | GET /op/allNotes?$refine= | held | http | matrix row (`catalog.rs`) |
| `FDATA-050` | GET /op/allNotes?$refine= | held | http | matrix row (`catalog.rs`) |
| `FDATA-051` | GET /op/allNotes?$refine= | held | http | matrix row (`catalog.rs`) |
| `FDATA-052` | GET /op/allNotes?$page= | held | http | matrix row (`catalog.rs`) |
| `FDATA-053` | POST /op/createNote (Idempotency-Key) | held | http | matrix row (`catalog.rs`) |
| `FDATA-054` | GET /download | held | http | matrix row (`catalog.rs`) |
| `FDATA-055` | GET /op/std.notesAccess.list | held | http | matrix row (`catalog.rs`) |
| `FDATA-056` | DELETE /op/unshareNote | held | http | matrix row (`catalog.rs`) |
| `FDATA-057` | POST /op/grantNotesAccess | held | http | matrix row (`catalog.rs`) |
| `FDATA-058` | POST /auth/link | held | http | matrix row (`catalog.rs`) |
| `FDATA-059` | POST /auth/link | held | http | matrix row (`catalog.rs`) |
| `FDATA-060` | POST /auth/link | held | http | matrix row (`catalog.rs`) |
| `FDATA-061` | POST /auth/link | held | http | matrix row (`catalog.rs`) |
| `FID-TOKEN-001` | POST /auth/refresh | held | http | matrix row (`catalog.rs`) |
| `FID-TOKEN-002` | POST /auth/refresh | held | http | matrix row (`catalog.rs`) |
| `FID-TOKEN-003` | token format (ordo_identity::token) | held | needs-code | existing: `crates/ordo-identity/src/token.rs` `forgery_of_every_shape_is_the_same_refusal`, `the_second_step_claim_round_trips_and_a_v3_token_is_refused_outright` |
| `FID-CSRF-001` | POST /auth/refresh | held | http | matrix row (`catalog.rs`) |
| `FID-CSRF-002` | POST /auth/refresh | held | http | matrix row (`catalog.rs`) |
| `FID-SESSION-001` | POST /auth/refresh (cookie resume) | held | http | matrix row (`catalog.rs`) |
| `FID-SESSION-002` | POST /auth/refresh | held | http | matrix row (`catalog.rs`) |
| `FID-SESSION-003` | POST /auth/refresh | held | http | matrix row (`catalog.rs`) |
| `FID-SESSION-004` | POST /auth/logout + /op | held | http | matrix row (`catalog.rs`) |
| `FID-SESSION-005` | POST /auth/refresh after logout | held | http | matrix row (`catalog.rs`) |
| `FID-SESSION-006` | POST /auth/refresh after logout | held | http | matrix row (`catalog.rs`) (a short script) |
| `FID-BOLA-001` | GET /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `FID-BOLA-002` | POST /op/createNote; GET /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `FID-OAUTH-001` | POST /auth/begin (discord) | held | http | matrix row (`catalog.rs`): the world has a `discord` OAuth2 connection; `begin` builds the redirect without calling the provider, so no stand-in server is needed. Also `auth_oidc.rs`, `auth_oauth2.rs` |
| `FID-OAUTH-002` | POST /auth/begin (discord) | held | http | matrix row (`catalog.rs`): the eight look-alike return addresses, each refused with no redirect, and the exact origin passing. Also `auth_oauth2.rs`, `auth_handoff.rs` |
| `FID-OAUTH-003` | federated callback handoff | held | needs-code | existing: `crates/ordo-host/tests/auth_handoff.rs` |
| `FID-AUTHN-001` | POST /auth/begin (email) | held | http | matrix row (`catalog.rs`): emailed codes are on in the world, through the mail relay stand-in; the row compares status, fields and lengths (timing is not asserted) |
| `FID-AUTHN-002` | POST /auth/complete (email_code) | held | http | matrix row (`catalog.rs`): 7 wrong codes then the real one, all the one refusal. Also `auth_email.rs` `five_wrong_codes_end_the_ceremony_and_leave_the_address_usable` |
| `FID-AUTHN-003` | POST /op/{unknown} | held | http | matrix row (`catalog.rs`) |
| `FID-AUTHN-004` | POST /auth/begin + complete (email) | held | http | matrix row (`catalog.rs`): another ceremony's binder with the attacker's code and with the victim's, both refused. Also `auth_email.rs` `a_code_without_the_asking_browser_is_refused` |
| `FID-CONSOLE-001` | GET /d/main/api/{status,session,owners,runtime} | held | http | console test (`crates/ordo-console/tests/catalog_attacks.rs` `FID_CONSOLE_001`) |
| `FID-CONSOLE-002` | POST /d/main/api/setup/begin | held | http | console test (`crates/ordo-console/tests/catalog_attacks.rs` `FID_CONSOLE_002`) |
| `FID-CONSOLE-003` | POST /d/main/api/setup/begin | held | http | console test (`crates/ordo-console/tests/catalog_attacks.rs` `FID_CONSOLE_003`) |
| `FID-CONSOLE-004` | GET /assets/* and /d/main/api/* | held | http | console test (`crates/ordo-console/tests/catalog_attacks.rs` `FID_CONSOLE_004`) |
| `FID-CONSOLE-005` | GET /d/main/ (console page) | held | http | console test (`crates/ordo-console/tests/catalog_attacks.rs` `FID_CONSOLE_005`) |
| `FID-CONSOLE-006` | POST /d/main/api/recovery | held | needs-code | existing: `crates/ordo-console/tests/console.rs` `a_recovery_code_works_once_and_forces_a_new_passkey_and_set`; brute force is not asserted: not covered |
| `FID-CONSOLE-007` | POST /d/main/api/signin/finish | held | needs-code | existing: `console.rs` `an_owner_signs_in_with_a_passkey`; a foreign passkey is not asserted: not covered |
| `FID-CONSOLE-008` | console session gate (Need levels) | held | needs-code | not covered: no test calls runtime/apply or door/rotate with a must_enrol session |
| `FID-CONSOLE-009` | GET/POST /d/{handle}/api/* | held | needs-code | not covered: cross-deployment handle confusion; `crates/ordo-console/tests/tenants.rs` is the nearest |
| `FID-EMAIL-001` | POST /auth/email/complete | unclear | http | matrix row (`catalog.rs`) expects today's answer: a first contact address is added without a touch (`replaced:false`). Unclear finding F1; see the board |
| `FID-EMAIL-002` | POST /auth/email/complete | held | http | matrix row (`catalog.rs`): `403 presence_required`, and the address is unchanged |
| `FID-EMAIL-003` | POST /auth/begin+complete (email) vs contact address | held | http | matrix row (`catalog.rs`): signing in as the added contact address is another principal, and the victim's own address still reaches the victim |
| `FID-EMAIL-004` | account recovery channel | held | needs-code | existing: `crates/ordo-host/tests/recovery.rs`, `contact_email.rs`; the contact-address recovery channel is not asserted against an attacker address: not covered |
| `FID-HEADERS-001` | door JSON API (/auth/*, /op/*) | held | http | matrix row (`catalog.rs`) |
| `FID-COOKIE-001` | Set-Cookie on /auth/complete | held | http | matrix row (`catalog.rs`) (a short script) |
| `FID-CONSOLE-010` | console.key / setup-code files | held | http | skipped (file modes, not an HTTP answer). `compose_hardening.rs` `no_secret_is_made_world_readable` covers the compose files, not the data directory: not covered |
| `DATA-001` | GET /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `DATA-002` | POST /op/editNote | held | http | matrix row (`catalog.rs`) |
| `DATA-003` | DELETE /op/deleteNote | held | http | matrix row (`catalog.rs`) |
| `DATA-004` | POST /op/editNote | held | http | matrix row (`catalog.rs`) |
| `DATA-005` | GET /op/std.notes.get | held | http | matrix row (`catalog.rs`) |
| `DATA-006` | GET /op/std.notes.get | held | http | matrix row (`catalog.rs`) |
| `DATA-007` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `DATA-008` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `DATA-009` | GET /blob/{handle}/meta | held | http | matrix row (`catalog.rs`) |
| `DATA-010` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `DATA-011` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `DATA-012` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `DATA-013` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `DATA-014` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `DATA-015` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `DATA-016` | GET /blob/{handle} Range | held | http | matrix row (`catalog.rs`) |
| `DATA-017` | GET /blob/{handle} Range | held | http | matrix row (`catalog.rs`) |
| `DATA-018` | GET /blob/{handle} Range | held | http | matrix row (`catalog.rs`) |
| `DATA-019` | GET /blob/{handle} Range | held | http | matrix row (`catalog.rs`) |
| `DATA-020` | GET /blob/{handle} Range | held | http | matrix row (`catalog.rs`) |
| `DATA-021` | PUT /blob/attachments/file/{id} | held | http | matrix row (`catalog.rs`) |
| `DATA-022` | PUT /blob/attachments/file/{id} | held | http | matrix row (`catalog.rs`) |
| `DATA-023` | PUT /blob/attachments/file/{id} | held | http | matrix row (`catalog.rs`) (a short script) |
| `DATA-024` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `DATA-025` | POST /op/{name} | held | http | matrix row (`catalog.rs`) |
| `DATA-026` | PUT /blob/{entity}/{field}/{id} | held | http | matrix row (`catalog.rs`) |
| `DATA-027` | POST /op/addAttachment | held | http | matrix row (`catalog.rs`) |
| `DATA-028` | GET /blob/{handle} | held | http | matrix row (`catalog.rs`) |
| `DATA-029` | POST /auth/link | held | http | matrix row (`catalog.rs`) |
| `DATA-030` | POST /auth/link | held | http | matrix row (`catalog.rs`) |
| `DATA-031` | POST /auth/link | held | http | matrix row (`catalog.rs`) |
| `DATA-032` | POST /auth/link | broke | http | skipped: covered by the guessing-limits rows |
| `DATA-033` | GET /op/allNotes | held | http | matrix row (`catalog.rs`) |
| `DATA-034` | GET /op/std.notes.get | held | http | matrix row (`catalog.rs`) |
| `DATA-035` | GET /op/myNotes,notesTagged | held | http | matrix row (`catalog.rs`) |
| `DATA-036` | POST/DELETE /op/* | held | http | matrix row (`catalog.rs`) |
| `DATA-037` | GET /download | held | http | matrix row (`catalog.rs`) |
| `DATA-038` | POST /auth/link + GET /op/allNotes | held | browser | browser: a guest token after a link revoke. The matrix actor `link_revoked` runs every op and route as a dead actor, and `crates/ordo-host/tests/share_links.rs` has `a_revoked_link_is_refused_at_the_next_call`; the browser path itself is not replayed |
| `DATA-039` | GET /op/myNotes?live=1&sync=1 | held | http | matrix row (`catalog.rs`) |
| `DATA-040` | GET /live ; POST /live/{stream} | held | http | matrix row (`catalog.rs`, a held-open `/live` stream; the add is refused) |
| `DATA-041` | POST /live/{stream} | held | http | matrix row (`catalog.rs`) |
| `DATA-042` | GET /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `DATA-043` | GET /metrics,/security/events,/usage | held | http | matrix row (`catalog.rs`) |
| `DATA-044` | GET /op/notesTagged | held | http | matrix row (`catalog.rs`) |
| `DATA-045` | POST /op/createNote | held | http | matrix row (`catalog.rs`) |
| `DATA-046` | GET /blob/{h}/meta & /op/std.notes.get | held | http | skipped (timing comparison, not an outcome). Not covered |
| `ID-001` | GET /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `ID-002` | GET /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `ID-003` | GET /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `ID-004` | GET /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `ID-005` | GET /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `ID-006` | GET /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `ID-007` | GET /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `ID-008` | GET /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `ID-009` | GET /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `ID-010` | GET /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `ID-011` | GET /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `ID-012` | POST /auth/complete | held | needs-code | existing: `auth_email.rs` `five_wrong_codes_end_the_ceremony_and_leave_the_address_usable` |
| `ID-013` | POST /auth/complete | held | needs-code | existing: `auth_email.rs` `a_code_signs_in_once` |
| `ID-014` | POST /auth/complete | held | needs-code | existing: `auth_email.rs` `a_code_without_the_asking_browser_is_refused` |
| `ID-015` | POST /auth/begin | held | http | matrix row (`catalog.rs`): same check as `FID-AUTHN-001`, on a second address pair |
| `ID-016` | POST /auth/begin | held | needs-code | existing: `auth_email.rs` `the_sixth_message_in_an_hour_to_one_address_is_not_sent` |
| `ID-017` | POST /auth/refresh | held | http | matrix row (`catalog.rs`) |
| `ID-018` | POST /auth/refresh | held | http | matrix row (`catalog.rs`) |
| `ID-019` | POST /auth/refresh | held | http | matrix row (`catalog.rs`) |
| `ID-020` | POST /auth/refresh | held | http | matrix row (`catalog.rs`) |
| `ID-021` | POST /auth/refresh | held | http | matrix row (`catalog.rs`) |
| `ID-022` | POST /auth/logout | held | http | matrix row (`catalog.rs`) (a short script) |
| `ID-023` | POST /auth/refresh | held | http | matrix row (`catalog.rs`) |
| `ID-024` | OPTIONS /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `ID-025` | OPTIONS /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `ID-026` | GET /op/myNotes | held | http | matrix row (`catalog.rs`) |
| `ID-027` | OPTIONS /op/myNotes | broke | http | matrix row (`catalog.rs`) |
| `ID-028` | POST /auth/begin | held | http | matrix row (`catalog.rs`) |
| `ID-029` | POST /auth/begin | held | http | matrix row (`catalog.rs`) |
| `ID-030` | POST /auth/begin | held | http | matrix row (`catalog.rs`) |
| `ID-031` | POST /auth/begin | held | http | matrix row (`catalog.rs`) |
| `ID-032` | POST /auth/step-up/begin | held | http | matrix row (`catalog.rs`) |
| `ID-033` | POST /auth/account/delete | held | http | matrix row (`catalog.rs`) |
| `ID-034` | POST /auth/delegation/redeem | held | http | matrix row (`catalog.rs`) |
| `ID-035` | POST /auth/recover/begin | held | http | matrix row (`catalog.rs`) |
| `ID-036` | POST /auth/logout | held | http | matrix row (`catalog.rs`) |
| `ID-037` | POST /op/std.space.share | held | http | matrix row (`catalog.rs`) |
| `ID-038` | GET /op/std.deployment.settings | held | http | matrix row (`catalog.rs`) |
| `ID-039` | GET /health/live and others | broke | http | matrix row (`catalog.rs`) |
| `ID-040` | POST /d/main/api/setup/begin | held | http | console test (`crates/ordo-console/tests/catalog_attacks.rs` `ID_040`) |
| `ID-041` | POST /d/main/api/setup/begin | held | http | console test (`crates/ordo-console/tests/catalog_attacks.rs` `ID_041`) |
| `ID-042` | POST /d/main/api/recovery | held | http | console test (`crates/ordo-console/tests/catalog_attacks.rs` `ID_042`) |
| `ID-043` | POST /d/main/api/setup/begin | held | http | console test (`crates/ordo-console/tests/catalog_attacks.rs` `ID_043`) |
| `ID-044` | POST /d/main/api/setup/begin | held | http | console test (`crates/ordo-console/tests/catalog_attacks.rs` `ID_044`) |
| `ID-045` | OPTIONS /d/main/api/setup/begin | held | http | console test (`crates/ordo-console/tests/catalog_attacks.rs` `ID_045`) |
| `ID-046` | POST /d/main/api/signin/finish | held | http | console test (`crates/ordo-console/tests/catalog_attacks.rs` `ID_046`) |
| `ID-047` | GET /d/nonexistent/api/session | held | http | console test (`crates/ordo-console/tests/catalog_attacks.rs` `ID_047`) |
| `ID-048` | GET /d/main/../../../etc/passwd | held | http | console test (`crates/ordo-console/tests/catalog_attacks.rs` `ID_048`) |
| `ID-049` | GET /d/main/api/session, /api/door/keys, /api/owners/invite | held | http | console test (`crates/ordo-console/tests/catalog_attacks.rs` `ID_049`) |
| `ID-050` | GET /d/main/ | held | http | console test (`crates/ordo-console/tests/catalog_attacks.rs` `ID_050`) |
| `INJ-SESSION-001` | GET /op/search?text= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-002` | GET /op/search?text= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-003` | GET /op/search?text= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-004` | GET /op/search?text= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-005` | GET /op/search?text= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-006` | GET /op/search?text= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-007` | GET /op/search?text= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-008` | GET /op/byPattern?p= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-009` | GET /op/byPattern?p= | broke | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-010` | GET /op/byPattern?p= | unclear | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-011` | GET /op/byPattern?p= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-012` | GET /op/byPattern?p= | broke | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-013` | GET /op/search?text= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-014` | GET /op/search?text= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-015` | POST /op/createItem | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-016` | GET /op/search?text= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-017` | GET /op/qtyAtLeast?min= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-018` | GET /op/qtyAtLeast?min= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-019` | GET /op/qtyAtLeast?min= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-020` | GET /op/topN?n= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-021` | GET /op/topN?n= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-022` | POST /op/createItem | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-023` | POST /op/createItem | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-024` | POST /op/createItem | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-025` | POST /op/createItem | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-026` | POST /op/createItem | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-027` | POST /op/createItem | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-028` | POST /op/createItem | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-029` | POST /op/createItem | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-030` | POST /op/createItem | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-031` | POST /op/createItem | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-032` | POST /op/createItem | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-033` | POST /op/createItem | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-034` | POST /op/createItem | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-035` | POST /op/createItem | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-036` | GET /op/priceAtMost?cap= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-037` | GET /op/priceAtMost?cap= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-038` | GET /op/dueBy?d= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-039` | GET /op/dueBy?d= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-040` | POST /op/createItem | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-041` | POST /op/createItem | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-042` | GET /op/oneItem?id= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-043` | GET /op/oneItem?id= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-044` | GET /op/search | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-045` | GET /op/search | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-046` | GET /op/../op/allItems | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-047` | GET /op/%2e%2e%2fallItems | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-048` | GET /op/allItems%00 | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-049` | GET /op/ALLITEMS | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-050` | GET /op/noSuchOp | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-051` | GET /op/allItems (X-Ordo-Space) | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-052` | GET /op/allItems (X-Ordo-Space) | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-053` | GET /op/allItems (X-Ordo-Space) | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-054` | POST /op/search | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-055` | GET /op/allItems | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-056` | GET /op/allItems | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-057` | GET /op/allItems (X-Ordo-Space) | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-058` | GET /op/allItems?$refine= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-059` | GET /op/allItems?$refine= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-060` | GET /op/allItems?$refine= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-061` | GET /op/allItems?$refine= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-062` | GET /op/allItems?$refine= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-063` | GET /op/allItems?$refine= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-064` | GET /op/allItems?$refine= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-065` | GET /op/allItems?$refine= | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-066` | GET /op/allItems (X-Ordo-Page) | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-067` | GET /op/allItems (X-Ordo-Page) | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-068` | GET /op/allItems (X-Ordo-Page) | held | http | matrix row (`catalog.rs`) |
| `INJ-SESSION-069` | GET /op/allItems (X-Ordo-Page) | held | needs-code | existing: `crates/ordo-host/tests/refine_paging.rs` |
| `INJ-SESSION-070` | GET /op/allItems (X-Ordo-Page) | held | needs-code | existing: `refine_paging.rs` |
| `INJ-SESSION-071` | GET /op/recent (X-Ordo-Page) | held | needs-code | existing: `refine_paging.rs` |
| `INJ-SESSION-072` | GET /op/allItems (X-Ordo-Page) | held | needs-code | existing: `refine_paging.rs` |
| `INJ-SESSION-073` | GET /op/allItems (X-Ordo-Page) | held | needs-code | existing: `refine_paging.rs` |
| `INJ-SESSION-074` | POST /op/search | held | http | matrix row (`catalog.rs`) |
| `SYNC-001` | GET /op/{name}?live=1&sync=1 | broke | http | matrix row (`catalog.rs`, held-open feed; a revoked link ends it with an error frame). Also `feed_door.rs` `a_revoked_share_ends_a_members_window_feed` |
| `SYNC-002` | GET /op/{name}?live=1 | broke | http | matrix row (`catalog.rs`, held-open feed; no invalidate after the revoke) |
| `SYNC-003` | GET /op/{name}?live=1&sync=1 | broke | http | matrix row (`catalog.rs`, held-open feed; a withdrawn delegation ends it). Also `feed_door.rs` `a_withdrawn_delegation_ends_the_agents_feeds` |
| `SYNC-004` | GET /op/{name}?live=1&sync=1 | held | http | matrix row (`catalog.rs`) |
| `SYNC-005` | GET /op/{name}?live=1&sync=1 | broke | http | matrix row (`catalog.rs`, held-open feed; a logout ends it within one tick, no write). Also `feed_door.rs` `logout_ends_every_feed_kind_within_one_idle_tick_with_no_write` |
| `SYNC-006` | GET /live + POST /live/{stream} | broke | http | matrix row (`catalog.rs`, held-open `/live` stream; a removed member's stream ends with an error) |
| `SYNC-007` | GET /op/{name}?live=1 | held | http | matrix row (`catalog.rs`, held-open feed and a second writer; the signal is per op by design, `live.rs`, and carries no row) |
| `SYNC-008` | GET /op/{name}?live=1&sync=1 (x-ordo-resume) | held | http | matrix row (`catalog.rs`, held-open sync feed; a forged resume hash gets the normal load) |
| `SYNC-009` | GET /op/{name}?live=1&sync=1 ($page) | held | http | matrix row (`catalog.rs`) |
| `SYNC-010` | POST /auth/link (device) | held | http | matrix row (`catalog.rs`) |
| `SYNC-011` | clients/ordo-sdk local mirror | held | needs-code | not covered: SDK local mirror after sign-out (browser storage); the SDK tests under `clients/` are the place |
| `SYNC-012` | clients/ordo-sdk local mirror | held | needs-code | not covered: SDK in-memory key (browser); the SDK tests under `clients/` are the place |
| `SYNC-013` | GET /op/{name}?live=1&sync=1 (flooding) | held | http | skipped (pool-pressure timing). Not covered; `crates/ordo-host/tests/store_pool_stress.rs` exercises the pool |
| `SYNC-014` | offline write replay | held | needs-code | not covered: offline write replay after revocation (SDK) |
| `SYNC-015` | shared note write (conflict) | held | needs-code | not covered: second-writer conflict through the replica (SDK) |

## Passkey ceremonies (2026-10-25)

Full WebAuthn ceremonies replayed through the real `/auth` routes with the software authenticator, one test per attack, in `crates/ordo-host/tests/passkey_attacks.rs`. All 36 held; none is ignored.

| Test | Result |
|---|---|
| `replays_a_sign_in_assertion_after_it_was_spent` | held |
| `replays_a_registration_after_its_challenge_was_spent` | held |
| `answers_a_challenge_the_server_never_issued` | held |
| `signs_a_challenge_issued_to_another_ceremony_inside_the_client_data` | held |
| `answers_another_sessions_step_up_challenge_on_my_session` | held |
| `answers_a_step_up_challenge_at_the_sign_in_route` | held |
| `answers_a_sign_in_challenge_at_the_step_up_route` | held |
| `answers_a_sign_in_challenge_past_its_expiry` | held |
| `answers_a_registration_challenge_past_its_expiry` | held |
| `answers_a_step_up_challenge_past_its_expiry` | held |
| `asserts_from_a_wrong_origin` | held |
| `registers_from_a_wrong_origin` | held |
| `asserts_in_a_cross_origin_frame` | held |
| `asserts_for_a_wrong_rp_id_hash` | held |
| `registers_for_a_wrong_rp_id_hash` | held |
| `asserts_with_user_presence_cleared` | held |
| `asserts_with_user_verification_cleared_where_it_is_required` | held |
| `registers_with_user_presence_or_verification_cleared` | held |
| `steps_up_with_user_verification_cleared_where_it_is_required` | held |
| `steps_up_with_user_presence_cleared` | held |
| `asserts_with_a_get_type_where_a_create_is_owed` | held |
| `asserts_with_a_create_type_where_a_get_is_owed` | held |
| `registers_with_an_unsupported_attestation_format` | held |
| `tampers_the_client_data_json_after_it_was_signed` | held |
| `tampers_the_authenticator_data_after_it_was_signed` | held |
| `sends_garbage_where_the_ceremony_bytes_belong` | held |
| `signs_with_another_credentials_key_under_the_victims_credential_id` | held |
| `asserts_for_a_credential_the_person_does_not_hold` | held |
| `runs_a_sign_count_backwards_as_a_cloned_authenticator` | held |
| `runs_a_sign_count_backwards_at_step_up` | held |
| `registers_another_persons_credential_id_under_a_new_key` | held |
| `replays_a_step_up_assertion_for_a_second_token` | held |
| `uses_a_stepped_up_token_after_the_freshness_window` | held |
| `uses_a_stepped_up_token_after_logout` | held |
| `presents_a_quarantined_passkey_at_a_presence_gated_act` | held |
| `asserts_with_a_preferred_deployment_but_presence_cleared` | held |
