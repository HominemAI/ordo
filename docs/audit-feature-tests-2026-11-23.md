# Audit: is each conceptual feature tested? 2026-11-23

What Ordo promises, stated as guarantees a person relies on, and the strongest test that proves each one. Asked by the owner 2026-11-23: *"Promises ordo makes on functionality as a concept as opposed to individual mechanisms."* Board: `close-the-coverage-gap-through-the`.

## How it was made

- 7 agents read the guides, specs, skills and public site, one area each plus one for guarantees that cut across areas, and listed every promise with the line that makes it.
- For each, they found and read the strongest test. Nothing was run.
- A guarantee with several paths takes the level of its weakest path. A "never" guarantee needs a test that tries the forbidden thing; a behaviour guarantee needs a test that observes it.
- Areas overlap at the edges, so a few guarantees appear twice.
- Levels rest partly on test names and short reads; the agents flagged where.

| Level | Means |
|---|---|
| E | Through an example app's end-to-end test |
| E* | Through another browser suite: the console's or the identity service's own, not an example app |
| I | Integration: a test drives a real server or database as a client would |
| U | Unit only, or against a mock |
| N | No test found |

## Totals

413 guarantees graded; 82 more are promised but marked unbuilt.

| Area | Graded | E | E* | I | U | N | Unbuilt |
|---|---|---|---|---|---|---|---|
| System guarantees, across areas | 40 | 6 | 0 | 25 | 5 | 4 | 12 |
| Language: data model, values, writes | 38 | 1 | 0 | 25 | 8 | 4 | 9 |
| Access, sharing, cost, loading | 77 | 5 | 2 | 57 | 11 | 2 | 11 |
| Custody, files, audit trail, leaving | 56 | 1 | 0 | 51 | 3 | 1 | 13 |
| Identity, sign-in, people, delegation | 54 | 6 | 5 | 38 | 4 | 1 | 13 |
| Deploying, operating, console, CLI, routines, functions | 82 | 2 | 7 | 68 | 3 | 2 | 13 |
| Clients, live data, offline, public path, federation | 66 | 11 | 0 | 36 | 17 | 2 | 11 |
| **All** | **413** | **32** | **14** | **300** | **51** | **16** | **82** |

## What the audit found

- **8% of guarantees are proved through an example app** (32 of 413). 73% are proved by integration tests, 12% by unit tests or mocks only, 4% not at all.
- **The owner's 5 examples:** live updates are E (2 tabs) and I across 2 nodes. Tenant data never on a public page: I. The public cache used: I, and the test does not prove the database went unread on a hit. A tenant op reading public data: the general form is unbuilt; the closest path is E through note-viewer. Proof verification: I, built by a direct function call, never fetched through the door; the `ordo-verify-proof` binary has never run in a test.
- **Many "never" guarantees are proved for one path of several.** Tenant isolation is E for ops and search, I for files, feeds and export, and only a mock on the device. No test writes in tenant B and checks that a feed in tenant A stays silent.
- **The client is often tested against a mock door.** Device copy wiped on sign-out, revoked feeds emptying the screen, reconnect after an upgrade, never showing half a write: all mock-only.
- **Every console browser test runs against a stand-in door and a fake child**, not `ordo-host`. They are E* here.
- **Nothing starts the real `ordo dev` binary**, and the first-deployment path (make-secrets, compose, invite, apply) is never run whole.
- **`crates/ordo-host/tests/two_instances.rs` is `#[ignore]`**, so a session ended on one server and refused on another never runs in CI.
- **Few example tests reach the harder language.** Shopfront declares money and a multi-statement op (`examples/shopfront/shopfront.ordo`), but has no browser tests; no example declares a tree. No test checks that a failed second write undoes the first.

## The 16 guarantees with no test

1. A receipt (account deletion, later tenant erasure) can be checked by someone outside the deployment.
2. A visitor with no token cannot open a live stream (`?live=1` answers `not_live`).
3. A write on one node drops the cached public answers on every other node.
4. A parameter value, sealed value or token never appears in a log line, span or metric label (checked for backend functions only).
5. Changing a money field's places never silently changes amounts.
6. Durations compare and sort only within one unit class.
7. A null parameter equals an omitted one, so null never overrides a default.
8. A write op cannot be a source or be composed into another op.
9. A tenant's own op choices survive a deploy; new ops arrive at the template default.
10. A chain proof discloses no space content.
11. The identity service refuses a database URL naming the console's or the deployment's database.
12. The first-deployment path works end to end.
13. A database on the wrong host or port is refused with a message naming host:port.
14. Public requests are counted by op, kind and outcome, and a probe for an unknown op mints no series.
15. A tenant in its own database does not reach a peer (a documented gap).
16. Nothing warns on an unindexed open filter (a documented gap, not a promise).

## System guarantees, across areas


### Verifiable history

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | Anyone outside the deployment can check a tenant's history offline, and the check fails when one link is moved or the key is not the one they trust | site-animations/ordo-guide.html:1045; README.md:30 | crates/ordo-host/tests/attest_webhook.rs:615 a_chain_proof_verifies_offline_and_stops_when_a_link_moves; :677 a_signature_from_a_key_you_do_not_trust_is_not_evidence; :551 the_payload_verifies_under_the_published_verifier | The proof is built by calling attest_ops::build_proof on the pool. No test fetches std.space.chainProof through the door and hands those bytes to the verifier. No SDK verifier exists (implementation.md wants one shipped with the SDKs). No example app verifies anything. |
| N | A receipt (account deletion, and later tenant erasure) can be checked by someone outside the deployment | docs/corpus-rulings.md 2026-09-21 erasure proof (cited at docs/audit-board-2026-09-29.md:87) |  | crates/ordo-host/tests/member_release.rs:951 asserts the account-deletion receipt's fields (act, reason, correlation). Nothing checks the receipt against the chained security stream or offline. The tenant erasure receipt is unbuilt (listed below). |
| I | History cannot be changed behind the platform's back: an edited entry or a whole-database restore is caught, by index, and the tenant closed | site-animations/ordo-guide.html:1045; docs/implementation.md:420 | crates/ordo-exec/tests/commandlog.rs:335 tampering_with_a_stored_group_is_caught_at_its_index; crates/ordo-cli/tests/operator.rs:313 verify_names_the_first_bad_index_and_its_kind; crates/ordo-host/tests/spaces.rs:1339 a_rewound_tenant_is_refused_at_resolve_and_served_after_the_record; crates/ordo-cli/tests/operator.rs:3159 | No example's e2e runs `ordo verify` after its clicks (upgrade_continuity.rs does, I). The backup and restore drill itself is unbuilt. |
| I | Every answered request carries a signed head a client can pin, and a pin never goes backwards | docs/deploying.md:1606 (x-ordo-verifiable-from, x-ordo-head-sig) | crates/ordo-host/tests/deployment.rs:2081 every_answered_request_carries_a_verifiable_pin; crates/ordo-host/tests/attest.rs:80 the_pin_follows_the_chain_and_never_goes_backwards | The SDK reads the pin only against a mock door (clients/ordo-sdk/test/generated.test.ts:334, U). No client checks the head signature end to end. |
| I | Every refusal at the door is recorded in the space's security stream, and a stranger can never write into a space's record | docs/security-logging-design.md:429 | crates/ordo-host/tests/http.rs:1543 refusals_before_the_check_are_recorded; crates/ordo-exec/tests/spaces.rs:905 a_denial_does_not_reach_the_twins_stream | 'Every' rests on crates/ordo-api/tests/door_refusal_scan.rs:70, a source scan, not a call per refusal. |

### Public data

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| E | A visitor with no token sees the app's public data | examples/syntax/21-public.ordo:3; docs/howto-public.md:10 | examples/note-viewer/e2e/tests/browser/federation.spec.ts:25 a person's public note is shown to anyone...; examples/note-viewer/e2e/tests/api/grants.spec.ts:41 a visitor gets the notes tagged public and no other |  |
| E | A signed-in person sees the app's public data beside their own (a tenant call reads public data) | docs/plan-public-data.md:164 (ask 23) | examples/note-viewer/e2e/tests/browser/federation.spec.ts:64 another person sees their own notes and the public ones... | Proven only as the page calling the public op with no token (examples/note-viewer/web/src/App.tsx:94). A tenant op that reads the deployment's public rows needs the `deployment` entity line, marked unbuilt (howto-public.md:413). crates/ordo-host/tests/public_path.rs:238 has a member call a public op with a token and asserts only 200, not which rows. |
| I | Tenant data can never be reached from a public page: no private field, private op, tenant space, refinement, tokenless write or private file | docs/howto-public.md:102; site-animations/ordo-guide.html:868 | crates/ordo-host/tests/public_path.rs:238 the_anonymous_door; crates/ordo-host/tests/blob_door.rs:3439 a_public_rows_file_is_served_and_a_private_rows_is_not; crates/ordo-host/tests/public_writes.rs:354 a_public_write_cannot_reach_outside_what_its_op_names (with :491 human check on by default, :546 hourly cap); crates/ordo-host/tests/public_name_collision.rs:209 | The op path is E (grants.spec.ts:41 and federation.spec.ts:25 try myNotes signed out and see 403 and no private note). Naming a tenant space, a private field, a private file and refine are I only. The tokenless live path is untested (separate row). |
| N | A visitor cannot open a live stream (a tokenless ?live=1 answers not_live) | docs/howto-public.md:107 |  | crates/ordo-api/tests/api.rs:2226 tests not_live only for a token holder on a non-live op. No test opens ?live=1 with no token. |
| I | The public call cache is used, and on one node a write, a deploy or a lost listener drops what it holds, so a visitor never sees stale data | docs/howto-public.md:119; docs/howto-public.md:256 | crates/ordo-host/tests/public_path.rs:365 the_cache_and_its_invalidation (miss then hit with the same body; a write to a read entity makes the next read a miss and fresh); crates/ordo-host/tests/public_writes.rs:611 a_public_write_drops_the_public_answers_that_read_its_entity | A hit is seen only in the X-Ordo-Cache header; no test counts that no query ran, and no e2e checks the header. Across nodes is the next row. |
| I | The public cache never serves one caller's private answer to another | docs/howto-public.md:109; docs/howto-public.md:253 | crates/ordo-host/tests/public_path.rs:238 (a member with a token gets no X-Ordo-Cache: members never use the cache); crates/ordo-api/tests/api.rs:3271 member_answers_and_streams_are_never_stored_by_the_browser_cache | Safe by construction because a public op cannot read $caller (compile refusal public_caller). No test has two members call the same op and compare. |
| N | A write on one node drops the cached public answers on every other node | docs/howto-public.md:253 (each node keeps answers; dropped exactly when a write commits) |  | public_path.rs boots one runtime. deployment.rs:1644 proves the Postgres bus crosses nodes for feeds, not for the public cache. A test would boot two nodes, warm the cache on B, write on A, and see a miss on B. |

### Live data

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| E | A change shows on every open client of the same data with no reload | README.md:31; README.md:227 | examples/notes/e2e/tests/ui/notes.spec.ts:54 a note written in another tab ... shows in the first tab with no reload; examples/notes/e2e/tests/api/door.spec.ts:157 a tenant made after the door booted gets live changes | Same person in two tabs. No e2e has a second person see a shared note change live. |
| I | A change written through one server reaches a live client held by another server | site-animations/ordo-guide.html:391; docs/deploying.md:1392 | crates/ordo-host/tests/deployment.rs:1644 a_write_on_one_node_reaches_a_live_subscriber_on_another | Two nodes in one test process. crates/ordo-host/tests/two_instances.rs:420 (two real processes) is #[ignore] and not run in CI. |
| U | A page keeps working across change: it is told when its op drifted, catches up after the network returns, and reopens its feed and restarts a stale list across an upgrade | site-animations/ordo-guide.html:863; docs/deploying.md:1556 | examples/notes/e2e/tests/ui/notes.spec.ts:106 after the network returns, the page catches up by itself (E); crates/ordo-api/tests/api.rs:2243 version_skew_header_flags_stale_pin (I); crates/ordo-host/tests/upgrade_continuity.rs:1995 (server side: feed carries on, cursor refused version_skew, I) | Weakest: the SDK's recovery from an upgrade (reopen, restart on version_skew) is proven only against a mock door. No e2e keeps a page open across an upgrade or loads an old page against a new version. |
| U | Two parts of one screen never show a write half applied (reads held to one log position) | docs/implementation.md:1179 | clients/ordo-sdk/test/manager.test.ts:207 advances screens only at a position every op has reached | Mock door only. An e2e would show two lists over one entity and assert neither shows the write before both do. |
| I | A routine fires once for the deployment, however many servers run | docs/deploying.md:1417 | crates/ordo-host/tests/notes_app.rs:1737 a_due_reminder_fires_once_and_mails_only_the_owner_with_two_servers_running | The e2e (examples/notes/e2e/tests/upgrade/v1-to-v2.spec.ts:252, one notice and one email) runs one server. |

### Tenant isolation

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| U | One tenant's data is never visible to another, through any op, feed, file, export, search or device copy | site-animations/ordo-guide.html:802; site-animations/ordo-guide.html:914 | examples/notes/e2e/tests/browser/passkey.spec.ts:41 a second person ... cannot see the first person's note (E); examples/notes/e2e/tests/v2/search-and-links.spec.ts:49 search ... never one that was not shared (E); crates/ordo-host/tests/blob_door.rs:761 a_handle_in_another_space_is_not_found_like_an_absent_one (I); crates/ordo-api/tests/api.rs:3454 (adding a feed read for another space is refused, I); crates/ordo-host/tests/exit_ops.rs:373 (only the owner exports, I); clients/ordo-sdk/test/device.test.ts:66 (device copy per person, U) | Weakest: the device copy. One person per space is proven only against a mock door; no e2e signs a second person into the same browser and checks the first person's cached rows are gone. Feed: no test writes in tenant B and checks a feed in tenant A stays silent. |
| I | A write in one tenant can never touch, or point at, another tenant's rows | site-animations/ordo-guide.html:802 | crates/ordo-host/tests/spaces.rs:515 an_upload_cannot_name_another_space; crates/ordo-host/tests/spaces.rs:676 each_tenants_writes_join_its_own_chain; crates/ordo-conformance/tests/spaces.rs:206 a_tree_walk_does_not_climb_into_another_space | No e2e. The cross_space_ref refusal on a write the guide shows is not tested through the door by name. |
| I | Every guard holds on every read path: an op, a refinement, a file, a person's own download | docs/writing-data.md (cited README.md:317); site-animations/ordo-guide.html:866 | crates/ordo-host/tests/refine_audit.rs:448 a_refinement_never_reaches_past_the_guard; crates/ordo-host/tests/blob_door.rs:535 the_row_guard_refuses_a_caller_the_door_would_otherwise_admit; crates/ordo-host/tests/own_download.rs:365 | E only for the op and search paths (passkey.spec.ts:41, search-and-links.spec.ts:49). |
| I | One busy tenant cannot take the whole pool or hold a statement for ever | docs/deploying.md:554; docs/deploying.md:587 | crates/ordo-exec/tests/noisy_neighbour.rs:94 a_capped_space_is_served_again_once_it_lets_go; crates/ordo-host/tests/noisy_neighbour.rs:23 | The statement timeout reaching the caller is a plain fn (crates/ordo-host/tests/noisy_neighbour.rs:128), not a timed-out call. A fair queue across many tenants is unbuilt. |
| I | Nothing the app developer did not declare is reachable, and a forbidden call looks exactly like one to an op that does not exist | README.md:44; site-animations/ordo-guide.html:868 | crates/ordo-api/tests/api.rs:1205 tokenless_real_and_unknown_ops_answer_alike; crates/ordo-api/tests/api.rs:2389 calling_another_space_is_denied_at_the_door; crates/ordo-host/tests/public_path.rs:238 (byte-identical denial); crates/ordo-host/tests/interface_surface.rs:491 a_token_for_another_space_describes_nothing | No e2e asks an undeclared op or route. |
| E | Between two deployments, Postgres itself refuses every table the reader line does not name | examples/note-viewer/viewer.ordo:5 | examples/note-viewer/e2e/tests/api/grants.spec.ts:25 Postgres refuses the viewer's role every notes v2 table...; crates/ordo-host/tests/reader_grants.rs:271 removing_a_table_or_the_line_revokes | The e2e asks Postgres with psql, not through a page; that is the right level for this promise. |
| E | One person is the same person in two deployments that share sign-in, and sees only what they hold in each | examples/note-viewer/viewer.ordo:9 | examples/note-viewer/e2e/tests/browser/federation.spec.ts:25; :87 a person who signs up at note_viewer first ... sees no private notes; crates/ordo-host/tests/peer_entities.rs:406 a_person_who_reaches_two_accounts_reads_nothing |  |

### Access ends

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | Taking back a share, withdrawing a delegation, or signing out ends access everywhere: ops, open feeds, the device copy and every server | docs/deploying.md:1247 | examples/notes/e2e/tests/browser/passkey.spec.ts:59 a shared note ... disappears when the share is taken back (E); crates/ordo-host/tests/feed_door.rs:315, :371, :405 logout_ends_every_feed_kind_within_one_idle_tick_with_no_write (I); crates/ordo-host/tests/notes_app.rs:1083 the_page_keeps_notes_on_the_device_and_erases_a_closed_share (I) | Weakest: another server. crates/ordo-host/tests/two_instances.rs:1342 an_ended_session_is_refused_on_the_other_instance is #[ignore], not in CI; member_release.rs:623 asserts only the reply text 'announced to every replica'. |
| I | Widening what an op reads or discloses kills every grant over it until a person consents again | docs/deploying.md:1270 | crates/ordo-host/tests/http.rs:1219 a_capability_token_is_confined_and_dies_when_its_scope_changes; crates/ordo-host/tests/sharing.rs:1568 a_window_token_carries_the_consented_pins_so_a_widened_op_refuses_the_window | No test changes the program by `ordo apply` and then calls with the old grant; the e2e upgrade does not hold a grant. |
| I | An agent acting for a person can call only the approved ops, never steps up, and stops at the next call when withdrawn | README.md:214; site-animations/ordo-guide.html:371 | crates/ordo-host/tests/delegation_consent.rs:628 ordo_consent_the_token_works_only_for_the_approved_ops; :738 ordo_consent_revoke_stops_the_next_call; crates/ordo-host/tests/feed_door.rs:315 a_withdrawn_delegation_ends_the_agents_feeds | No example app uses delegation (examples/assistant has no e2e). |
| I | A person who leaves a tenant loses access at once and their access rows go | docs/definition-language.md:1338 | crates/ordo-host/tests/member_release.rs:346 a_left_seat_is_refused_at_the_door; :367 leaving_deletes_access_rows_and_keeps_referring_rows | No e2e has a person leave a tenant. |

### Erasure

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| U | Erased data is gone everywhere it lived: a deleted tenant's rows, files, keys and people, a deleted account's sign-in, and the copies on devices | site-animations/ordo-guide.html:1001; README.md:201; docs/audit-one-way-gaps.md:16 | crates/ordo-host/tests/exit_ops.rs:617 deletion_needs_the_spaces_own_id_and_then_ends_the_space; :855 erasing_a_space_deletes_its_objects_too; :1067 deleting_a_tenant_settles_the_people_it_made_and_says_so; crates/ordo-host/tests/member_release.rs:951, :996 a_deleted_account_cannot_sign_in_and_its_data_stays_by_default | Rows, files, people and accounts are I. Weakest: copies on devices (clients/ordo-sdk/test/device.test.ts:91 purge(), mock door only); nothing tells a device the space is gone. Backups are not built. No e2e deletes a tenant or an account. |

### Upgrades and versions

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | An upgrade (program or Ordo itself) keeps every person, session, row, file, share, link, delegation and chain | docs/deploying.md:1534 | examples/notes/e2e/tests/upgrade/v1-to-v2.spec.ts:88 two people with notes, tags and a share under v1 find all of it under v2 (E, program upgrade); crates/ordo-host/tests/upgrade_continuity.rs:1995 a_compatible_program_update_keeps_everyone_and_everything; :2126 the_previous_ordo_hands_over_to_this_one (CI `upgrade` job) | Weakest: an Ordo upgrade is I only; no e2e runs the previous release's binary and then this one under the browser. |
| I | A change the data cannot take, or a failed deploy, never loses data: it is refused or held, by name, and the old version keeps serving | docs/implementation.md:360; docs/deploying.md:1400 | crates/ordo-cli/tests/plan_against.rs:275 dropping_a_field_the_serving_artifact_reads_is_refused_by_name; crates/ordo-host/tests/notes_app.rs:989 a_change_one_tenants_data_cannot_take_closes_that_tenant_alone; crates/ordo-host/tests/apply_everywhere.rs:174 a_target_that_fails_holds_back_the_activation; crates/ordo-host/tests/deploy.rs:87 a_rollback_is_a_re_point_and_leaves_a_record | A death mid-backfill (implementation.md:369) is marked proposed and not tested. |
| I | No write is lost or applied twice: a retried write lands once across restarts and upgrades, and two people editing the same thing keep both texts | docs/deploying.md:230; docs/deploying.md:1550; docs/implementation.md:282 | examples/notes/e2e/tests/ui/notes.spec.ts:137 two tabs edit the same note: the second save is refused, nothing is lost (E); crates/ordo-host/tests/replay_durable.rs:97 a_stored_response_survives_the_process_that_stored_it; :129 exactly_one_of_two_connections_reserves; crates/ordo-host/tests/upgrade_continuity.rs:1995 (I) | Weakest: idempotent retry is I only; no e2e retries a write. The two-server case (two_instances.rs:420) is #[ignore], not in CI. |

### Client and server agree

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| U | The client and the server agree: the device filters and sorts as the server does, and refuses what the server refuses | docs/implementation.md:1179; clients/ordo-react/test/refine-agreement.test.ts:1 | crates/ordo-conformance/tests/refine_parity.rs + clients/ordo-sdk/test/refine-parity.test.ts over one case table crates/ordo-conformance/fixtures/refine-parity.json (results, I); clients/ordo-react/test/refine-agreement.test.ts (refusals, U) | Results are I. Weakest: refusals. Only the SDK and React copies are compared with each other; nothing compares them with the server's invalid_refinement answer. No e2e. |
| I | Signed in stays signed in: across reload, server restart and upgrade | README.md:212; docs/deploying.md:1545 | examples/notes/e2e/tests/browser/passkey.spec.ts:18 ... reloads, and is still signed in (E); crates/ordo-host/tests/auth_durability.rs:114 a_credential_survives_a_restart; crates/ordo-host/tests/upgrade_continuity.rs:1995 | Weakest: a server restart under an open page is I only. |
| E | Notes stay on screen with the network off, and a write waits for it | README.md:228 | examples/notes/e2e/tests/ui/notes.spec.ts:322; examples/notes/e2e/tests/browser/passkey.spec.ts:103 | Offline writes are planned (README.md:207). |

### Refusals and secrets

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A refusal always names what to change: the setting and value, or the rule | site-animations/ordo-guide.html:739; memory rule 'refusals name the setting' | examples/notes/e2e/tests/v2/search-and-links.spec.ts:133 ... shows the refusal that names the setting (E); crates/ordo-host/tests/own_download.rs:336 off_by_default_and_the_refusal_names_the_switch; crates/ordo-host/tests/delegation_consent.rs:867 ordo_consent_refusals_name_the_value_to_set | Proven one refusal at a time. No sweep checks every refusal class carries a fix, so a new refusal without one passes CI. |
| I | An error never hands the caller the database's, store's or provider's internals | AGENTS.md rule 9; docs/deploying.md:584 | crates/ordo-host/tests/blob_door.rs:476 a_store_error_is_never_returned_to_the_caller; crates/ordo-host/tests/spaces.rs:1783 a_dial_error_is_never_returned_to_the_caller; crates/ordo-host/tests/db_error_text_scan.rs (scan) |  |
| I | A sealed value never sits in the clear in the database, the command log or a stored file | site-animations/ordo-guide.html:961; site-animations/sovereignty.html:419 | crates/ordo-exec/tests/sealed.rs:302 a_sealed_image_resolves_to_the_plaintext_that_was_written (asserts no plaintext in the chain bytes); crates/ordo-blob/tests/pipeline.rs:128 the_plaintext_never_appears_in_the_stored_object | Log lines are untested (next row). |
| N | A parameter value, sealed value or token never appears in a log line, span or metric label | docs/implementation.md:509 |  | Only backend functions are checked (crates/ordo-host/tests/functions.rs:360 'the input is never logged'). A test would call ops with a marker value and grep the captured tracing output and /metrics. |
| I | Outbound calls never reach inside the deployment's own network and never follow a redirect | docs/implementation.md:223; docs/implementation.md:231 | crates/ordo-host/tests/attest_webhook.rs:429 a_destination_inside_the_perimeter_is_refused_without_a_socket; crates/ordo-egress/tests/function_fetch.rs:238 a_redirect_is_returned_and_never_followed |  |

**Promised but marked unbuilt, not graded:**

- A tenant op reads the deployment's public rows: the `deployment` entity line and `$tenant` (docs/howto-public.md:413, docs/roadmap-checklist.md:13376)
- Shared live public streams (docs/howto-public.md:409)
- A public write landing in a tenant's own space by grant (docs/howto-public.md:140)
- A row condition on `public` (docs/howto-public.md:414)
- Public cache counters ordo_public_cache_entries / ordo_public_cache_bytes (docs/howto-public.md:418)
- A tenant erasure receipt (docs/audit-board-2026-09-29.md:87, board erasure-receipt)
- Client-held keys: client-first spaces, keys only on devices (README.md:206; site-animations/sovereignty.html:422)
- Offline writes (README.md:207)
- The backup and restore drill (README.md:236, README.md:254; docs/implementation.md:420 proposed)
- Migration death mid-backfill recovery (docs/implementation.md:354, proposed)
- Device key loss and revocation (docs/implementation.md:1102, proposed)
- A fair queue across many busy tenants on one database (docs/deploying.md:572)

## Language: data model, values, writes


### Ids and identity

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| U | Every id says what it is (prefix from the entity), is server-minted, and two entities can never share a prefix | docs/typed-id-keys.md:1-36 | crates/ordo-schema/tests/graph.rs:532 explicit_key_overrides_and_collisions_fail_naming_both; :589 general_english_abbreviations_apply | Schema-level only. No test mints ids through a door and reads the prefix back; no e2e asserts an id shape. A client can never choose its own id: not tested at the door. |
| U | A deployment owner's abbreviation table wins over built-ins and a malformed line is a loud error, never dropped | docs/typed-id-keys.md:50-62 | crates/ordo-schema/tests/graph.rs:~573 user_key_table_overrides_builtins | KeyTable::parse only; the ORDO_KEY_ABBREVIATIONS env var is never exercised on a booted host. |
| I | id and rev cannot be declared or written by app code; rev starts at 1 and moves on every converge | docs/definition-language.md:782-790; docs/writing-data.md:102,164 | crates/ordo-host/tests/http.rs and crates/ordo-api/tests/api.rs (reserved_derived_name); crates/ordo-schema/tests/graph.rs | rev-starts-at-1 asserted only indirectly through CAS tests. |

### Text and ordering

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| U | The same op returns text in codepoint order (Z before a) on every deployment, and no locale ordering can be requested | docs/howto-values.md:42-60; definition-language.md:149-159 | crates/ordo-plan/tests/emit.rs (COLLATE in SQL); crates/ordo-playground/tests/differential.rs:637 | Asserts the SQL carries COLLATE "C"; no test sorts Z/a/Aa rows through a real server and sees the order. Not in any example. |
| I | One text is one value: two Unicode spellings of the same text can never both be stored, compared apart, or both fit a unique field | docs/howto-values.md:64-82 | crates/ordo-exec/tests/nfc.rs:66,83 (real Postgres); crates/ordo-api/tests/api.rs:3800 refinement_text_reaches_the_executor_in_nfc | The nfc.rs unique test builds its own table by hand and normalizes by calling nfc() itself; it does not drive a registered op write end to end. normalize(lower) uniqueness has no test. |
| I | hash(x) gives the same bucket forever, in SQL and in the host, and is non-negative | docs/howto-values.md:190-210 | crates/ordo-conformance/tests/corpus.rs:1004 hash_is_bit_stable_and_agrees_across_venues | Frozen vectors plus Postgres parity. Good. |
| I | A bounded text(n) never stores a longer value: a long parameter is bound_violation, a long field value is check_violation, counted in characters | definition-language.md:217-227,641-643 | crates/ordo-conformance/tests/text_bounds.rs:81,126,154 | Through the SDK/web form only in clients/ordo-react/test/form.test.ts (unit). Not through an example e2e. |

### Money and numbers

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | Money is never silently rounded: a computed decimal cannot land in a money or int field without an explicit round/floor/ceil, and the refusal names implicit_rounding at deploy | howto-values.md:86-155; definition-language.md:245-254; traps.md:480- | crates/ordo-conformance/tests/decimal.rs:305 a_decimal_may_not_land_in_a_typed_slot_unconverted; money_places.rs:202,278; computed_rhs.rs:761,782 | Compile-time refusal tested against real fixtures. No example app declares money, so nothing end to end in a client. |
| I | A person writes 29.99 and it is stored and compared as 2999 cents; different places never mix | howto-values.md:88-107 | crates/ordo-conformance/tests/money_places.rs:88 major_units_are_written_and_minor_units_are_stored; :125; ordo-sema/tests/money_places.rs (money_places_mismatch) | Strong at conformance level; no e2e shows a price form. |
| I | Arithmetic gives the same answer in SQL and in the host: division is fixed at 12 places, /0 is a loud typed error, no float anywhere | definition-language.md:281-325 | crates/ordo-conformance/tests/decimal.rs:99,125,206,272 | round is half-up asserted in decimal.rs:236 conversions; half-up tie case at digit 13 not separately seen. |
| I | Splitting an amount with allocate always sums exactly to the total, whichever party gets the odd cent | definition-language.md:907-956; howto-values.md:157-164 | crates/ordo-conformance/tests/allocate.rs:173,195,334,574 (agrees_with_postgres) | Tests the allocation function. The only legal site (each over a list param) does not parse, so no op can use it: see unbuilt. Scored I for the arithmetic, N for any user-reachable use. |
| N | Changing a money field's places never silently changes amounts: migrate demands rescale\|keep, and a revert past a rescale converts or refuses naming the row | howto-values.md:109-128 |  | No test found for places_revert_loses_digits, places_revert_overflows, or the --places flag refusal (rg over crates returned no test). An example test would need: apply v1 with money(2), write rows, apply v2 money(4) without --places and see refusal, with rescale then revert. |

### Dates and time

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A date that cannot exist (2026-02-31, 2025-02-29) never gets into a program or a request | definition-language.md:59-62 | crates/ordo-syntax/src/lexer.rs:670 nonexistent_gregorian_dates_are_rejected; crates/ordo-api/tests/api.rs:3712 (door refuses 2026-02-31) | Lexer test is in src (unit); door half is in ordo-api tests. |
| I | A window like since 30d moves with the clock but is fixed at request time, and an op marked reproducible refuses since and filled params | howto-values.md:170-186; definition-language.md:645-666 | crates/ordo-conformance/tests/clock.rs:33,51; dl_spec_gaps.rs:41,50,64,78,123,141 | since over a month window (clamped day) has no test; month arithmetic clamping (2026-01-31 + 1mo = 02-28, since 1mo) has only the shift_date code in ordo-sema/src/lib.rs:1154 and no test: N for the clamp. |
| N | Durations compare and sort only inside one unit class; mixing is refused at deploy, and a months value cannot enter an exact-class field | definition-language.md:338-380; traps.md:107-150 |  | Promised classes incomparable_duration and duration_unit_class have no test and the doc says bare duration is not built. Only unit conversion of defaults is tested (ordo-schema/tests/graph.rs:440-475). Listed also under unbuilt. |
| U | today/now/newid are refused as defaults and in write bodies, so time always arrives as a declared parameter | definition-language.md:713-717; traps.md:24-70 | crates/ordo-schema/tests/graph.rs and crates/ordo-cli/tests/cli.rs (unsupported_default) | Refusal of today in a write RHS / condition not located as a test. |

### Null and comparison

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | Null never makes a row silently vanish: != keeps null rows, == null finds them, negated orderings keep them, in every venue (filter, guard, check, join) | definition-language.md:2393-2445; howto-values.md:7-38 | crates/ordo-conformance/tests/null_logic.rs:48,63,77,91,107,120,142; equality_parity.rs:386,399,449 (null-saturated parity over real corpora); check_invariants.rs:168 | Not seen in an example e2e. |
| U | Nulls sort last whichever way the sort goes | definition-language.md:2457; traps.md:328 | crates/ordo-plan/tests/emit.rs:~112 null_placement_is_explicit_in_both_directions; conformance corpus.rs:547 | Asserts SQL text contains NULLS LAST; no test reads rows back in order. |
| N | A null param equals an omitted param: the default applies, so null never overrides a default | skills/ordo/language.md:159-160 |  | No test named for it found. Needs: op with $lead: id = null default, send explicit null on a converge, assert stored value. |

### Values: json and files

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A json field stores any document whole, refuses documents over the byte/depth limits, and can be filtered by path without a float ever involved | definition-language.md:561-630 | crates/ordo-conformance/tests/json_filter.rs:135-251; json_field.rs; crates/ordo-host/tests/json_field.rs; E: examples/notes/e2e/tests/ui/notes.spec.ts:244,284 (tags via contains) | Weakest part: the over-limit refusal (ORDO_JSON_MAX_BYTES) is at I only (write_literal_gaps.rs:28); e2e covers contains on arrays only. |
| I | A json document can never be sorted, grouped, summed, indexed, put in a check, or read through a sealed path: each is refused by name at deploy | definition-language.md:610-617 | crates/ordo-conformance/tests/json_field.rs:47,57,65,94; json_filter.rs:268-299 | Compile-time refusals only. |
| U | A blob or uri field can't be declared in a way that promises what files can't do (index, default, required, normalize, op param, private on non-blob): each refused by name | definition-language.md:437-451,491 | crates/ordo-schema/tests/content_types.rs; graph.rs (blob_not_indexable) | Schema compile only; blob_parameter checked in ordo-host/tests/upload_seam.rs. Blob upload/serve itself belongs to another agent. |
| I | A client-sealed (private) blob is refused on upload and read with the reason, never served as ciphertext labelled as a file | definition-language.md:452-474 | crates/ordo-host/tests/blob_door.rs:682 a_sealed_blob_field_is_refused_by_name; :720 ...also_refused_on_read | Open export of a private object as stored: no test seen. |

### Shapes and integrity

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A row can never break its own invariant: a write that violates check aborts the whole group, names the check, and leaves nothing changed (the seat never oversells) | definition-language.md:719-736; writing-data.md:69-86 | crates/ordo-conformance/tests/check_invariants.rs:112; computed_rhs.rs:175 dls_own_increment_counts_and_the_check_caps_it | Concurrent last-seat race (two callers at once, one seat each) is not tested; both tests are sequential. Multi-statement abort (check fires on statement 2, statement 1 rolled back) not seen. Ref-walking check is refused (unbuilt). |
| I | A unique value is unique per tenant only, never across tenants; a nullable unique admits one null row; sealed unique still holds | definition-language.md:803-840; traps.md:72-94 | crates/ordo-conformance/tests/spaces.rs:387 a_unique_key_is_claimable_once_per_space; entity_clauses.rs:217; corpus_stage5.rs:1736 | Two tenants through the real stack at the door (ordo-host) not seen; conformance fixture only. |
| I | A row that others still point at can't be deleted (has_referrers names the referrer); a row's owned children go with it and come back on revert; deleting what you may not fully delete deletes nothing | definition-language.md:796-799,1045-1063; writing-data.md:236-271 | crates/ordo-conformance/tests/owned_delete.rs:60,90,105,124,136,148; crates/ordo-exec/tests/owned_delete_revert.rs:185,258; E: examples/notes/e2e/tests/ui/notes.spec.ts:202 (delete note, access rows go) | E covers the delete-cascade of acl rows only; has_referrers refusal and revert-restores are I. |
| I | A tree can't be corrupted: cycles, over-depth moves, ranged reparents and deleting a node with children are refused; subtree delete is all-or-nothing and per-row revertible | definition-language.md:1117-1149; writing-data.md:199-234 | crates/ordo-conformance/tests/corpus_stage5.rs:638 tree_writes_maintain_the_path_and_refuse_what_they_should; :936; :1146; crates/ordo-host/tests/http.rs (tree_cycle, tree_bulk_reparent, tree_has_children) | tree_depth_exceeded found only in generated specs, no test asserting the cap error through a door. No example app uses a tree. |
| U | Every ref and declared index is served by an index scoped to the tenant; wrong index spellings on sealed/open fields are refused | definition-language.md:1993-2007 | crates/ordo-conformance/tests/entity_clauses.rs:252; ordo-schema/tests/graph.rs (index_on_sealed, indexed_without_sealed) | DDL/catalog presence, no query plan check. |

### Writes

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | The mode of a write is derived from its braces and printed, so a reviewer sees creates / converges / creates-or-converges / bulk on every op | definition-language.md:3083-3123; writing-data.md:13-46 | crates/ordo-conformance/tests/corpus_stage5.rs (create/converge/CAS/upsert cases); crates/ordo-cli/tests/cli.rs | Per-statement label list on multi-statement ops not located as a test. |
| I | A write that could fail on its create branch never deploys: missing required fields (insert_incomplete), identity in a bulk write, two unique keys (ambiguous) are all refused at deploy | definition-language.md:3124-3158; writing-data.md:310-350 | crates/ordo-conformance/tests/corpus_stage5.rs (insert_incomplete, bulk_identity_field, ambiguous_identity); routine_each.rs | Compile-time. |
| E | Two people editing one row never lose each other's work: a stale rev is refused (cas_miss), the writer keeps their text and may take the stored one | definition-language.md:3159-3183,3199 | examples/notes/e2e/tests/ui/notes.spec.ts:137 and :177; examples/notes-v2/web/test/notes.test.ts | expected_absent (create-once, rev: null) and conflicting_identity have I only (corpus_stage5.rs, ordo-api tests). |
| I | Replaying an ingestion record or a retried request never duplicates a row (converge-or-create on a unique key; idempotency key replays the stored answer) | definition-language.md:3197-3214 | crates/ordo-api/tests/api.rs:1662 idempotent_write_replays_without_reexecution; spaces.rs:387 (repeat converges); corpus_stage5.rs (null_key_address) | SDK retry loop reusing the key against a dropped connection: not seen at E. |
| I | A converge changes only the fields you describe, and an addressed counter increments atomically from the row's current value (create reads the default) | definition-language.md:3184-3186,2464-2469; writing-data.md:69-104 | crates/ordo-conformance/tests/computed_rhs.rs:290,341,625,664,601,714 | Concurrent increments not tested. |
| I | A multi-statement op lands whole or not at all, and one revert undoes the whole group | definition-language.md:3188-3195; writing-data.md:148-165 | crates/ordo-conformance/tests/write_let.rs:152 the_create_and_the_dependent_write_are_one_command_group; ordo-exec/tests/owned_delete_revert.rs | Failure of a later statement rolling back an earlier one is only shown through a check abort on a single row; no test makes statement 2 fail and checks statement 1 absent. e2e: none. |
| I | A person cannot learn a row exists by writing to it: an invisible row answers not_found byte-identical to an absent one, a ranged write silently skips rows they cannot write | writing-data.md:106-132; traps.md:209 | crates/ordo-conformance/tests/guards_corpus.rs (not_found parity); crates/ordo-host/tests/http.rs | Overlaps the authorization agent. Not at E. |
| U | A refused write never answers 500: every data or statement refusal has a named class and the right 4xx family | writing-data.md:273-308 | crates/ordo-host/tests/refine_door.rs:360 (400 never 500); ordo-conformance/tests/class_inventory.rs | No test sweeps every write refusal in the table for status; class_inventory checks names. |
| N | A write op can't be a source for a read or composed into another op | writing-data.md:44-46 |  | No test found for this refusal. |

### Defaults and params

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A caller can never supply $caller or another person's identity; bounds on params are enforced at the door before planning | definition-language.md:641-643,668-671; writing-data.md:64-67 | crates/ordo-api/tests/api.rs:1631 a_client_supplied_caller_is_refused_before_anything_runs; :1149 bound_violations | Only the notes example uses $caller via acl; no e2e tries to send $caller. |

**Promised but marked unbuilt, not graded:**

- enum(...) field type (refused: closed type set)
- list of {...} params and set ... each $list (no parse arm); so allocate has no legal site and every allocate is invalid_allocate
- check that walks refs (check_beyond_row); parent-side re-check on referenced writes
- bare duration / calendar class: incomparable_duration and duration_unit_class not built (definition-language.md:217-227)
- now, today, newid as defaults (refused as unsupported_default)
- f.mime / f.size on blob fields do not resolve (not_a_ref)
- blob private upload/serve (key ceremony owed, refused by name)
- ordo revert rounding policy has no setting (RevertPolicy only reachable from Rust)
- index serving a path inside a json field; schema change text<->json on sealed fields not detected

## Access, sharing, cost, loading


### Who can see which rows

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A row a person may not read is never shown to them: a read lists only rows their guard admits, and an addressed read of one answers not_found, the same as a missing row (no existence oracle) | skills/ordo/guards-and-roles.md:64; docs/std-library.md:248 | crates/ordo-conformance/tests/guards_corpus.rs:118 gc001_to_gc004; std_library.rs:420 not_found_and_access_guard_stay_indistinguishable_where_they_must; guards_canary.rs:230 | Real Postgres, executor level. Not through an example app guard for a non-acl entity. |
| I | A person who can read a row but not write it is refused with access_guard, and a read-level share never allows a write | skills/ordo/guards-and-roles.md:64 | guards_corpus.rs:1036 gc040_a_read_level_cannot_write; :322 gc011_to_gc022_the_write_matrix |  |
| E | One person's note is never visible to another person in the same deployment (own tenant), through ops, feeds or search | docs/howto-sharing.md:17 | examples/notes/e2e/tests/browser/passkey.spec.ts:41 a second person signs up in another browser and cannot see the first person's note; v2/search-and-links.spec.ts:49 search never finds an unshared note | Feed and cache paths covered only by the tenant-isolation crates tests, not named in the e2e |
| I | A guard on a child ref walks to the parent (owned children inherit; an access line on a child replaces the inherited guard) | skills/ordo/guards-and-roles.md:54-60 | guards_corpus.rs:592 gc023_and_gc028; guards_multihop.rs:89 a_grandchild_inherits_the_root_guard_across_two_hops |  |
| I | Guards cannot be switched off by accident: stripping them changes every guarded answer (canary), and a bypass must be written `unguarded` and is disclosed | skills/ordo/guards-and-roles.md:66 | guards_canary.rs:230 every_guarded_read_answers_differently_without_guards; guards_corpus.rs:735 gc030_to_gc035_the_bypass_family |  |
| I | `== $caller` matches the person's own row only, never a group they belong to | skills/ordo/guards-and-roles.md:66 | guards_corpus.rs:1706 gc071_and_gc072_caller_conditions; guard_groups.rs:299 |  |
| U | A guard cannot be written wrongly: an entity with only one of read/write is refused (incomplete_access_clause) | skills/ordo/guards-and-roles.md:53,68 | crates/ordo-schema/tests/graph.rs:697 | Schema-level refusal; no test through ordo check or deploy |
| I | A refinement can never reach a row the op itself could not (filters cannot widen what a guard allows) | docs/howto-cost.md:149 | crates/ordo-host/tests/refine_audit.rs:448 a_refinement_never_reaches_past_the_guard |  |
| I | Per-row sharing through acl: creator gets top level, grant/revoke/change-level ops, tenant owner may administer | skills/ordo/guards-and-roles.md:55 | guards_corpus.rs:851 gc036; :1159 gc042_to_gc045; owner_admin.rs:59; examples/notes e2e share-by-invitation exercises it | Owner-admin in e2e not exercised as owner overriding |
| I | Standard (std) ops obey the same guards and never bypass them; std.E.* over an acl link cannot undercut the grant op | docs/std-library.md:437,129 | crates/ordo-conformance/tests/std_library.rs:350 a_derived_write_refuses_the_callers_the_authored_write_refuses; :458 std_over_an_acl_link_entity_cannot_undercut_the_grant_op; :656 |  |
| I | An entity with sealed fields exposes only its open skeleton through std unless the shape opts in | docs/std-library.md:210-236 | std_library.rs:234 sealed_fields_are_absent_from_the_derived_read_surface |  |

### Roles and who may call an op

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | An op someone may not call answers access_denied, the same as an unknown op (no op-existence leak) | docs/howto-roles.md:192 | crates/ordo-host/tests/callers_of.rs:365 every_refusal_is_the_same_refusal; authz_matrix.rs:273 authorization_matrix (ignored, run by e2e.yml authz job) |  |
| I | Default roles hold: every role reads, editor and above write, manager and owner call std ops | docs/howto-roles.md:19; skills/ordo/guards-and-roles.md:79 | access_profiles.rs:349 the_template_is_the_default_in_every_tenant; :1138 no_template_keeps_the_built_in_default | Writes and std default not asserted individually in the first test |
| U | The app developer's template sets the default per op and a bad template refuses boot (platform op, pattern, unknown role, duplicate) | docs/howto-roles.md:27-45 | crates/ordo-host/src/access_template.rs:227 a_platform_op_a_pattern_and_an_unknown_role_refuse | Parser unit test; no test boots the binary with a bad value. teamspace_app.rs:269 boots with a good one |
| I | A tenant owner can narrow or widen one op for their tenant only, and another tenant is never changed | docs/howto-roles.md:9-11 | access_profiles.rs:386; :496 two_tenants_profiles_are_independent |  |
| N | A tenant's own choice survives a deploy; an unset op follows the template; a new op arrives at its template default | docs/howto-roles.md:79-83 |  | No test found that deploys over a tenant profile |
| I | A manager can never hand out more than a manager holds (no self-escalation, every widening path) | docs/howto-roles.md:85-92 | access_profiles.rs:714 a_manager_hands_out_only_ops_the_manager_holds; :843 a_manager_cannot_raise_their_own_access |  |
| I | A deny override beats every allow, for one member | docs/howto-roles.md:144-155 | access_profiles.rs:443 a_member_override_allows_and_denies_one_person |  |
| I | A group a profile line or an access grant still names cannot be deleted (group_in_use); groups cannot nest | docs/howto-roles.md:99-109 | access_profiles.rs:938; :1069; group_membership.rs:375 groups_may_not_contain_groups |  |
| I | A tenant defines its own roles over resources: a group member opens a granted resource, a non-member does not, and removal takes effect on the next request | docs/howto-roles.md:111-140 | group_membership.rs:238 a_group_member_opens_a_granted_resource_and_a_non_member_does_not; :284 | Example app 08-groups program is not driven by Playwright |
| I | Platform ops, owner-only capabilities, delegated-token-closed deletes and step-up cannot be loosened by a tenant | docs/howto-roles.md:157-162 | access_profiles.rs:627 a_refusal_names_what_to_change; :1155 a_delete_closed_to_delegation_stays_closed_whatever_a_tenant_opens | Platform-op listing refusal only partly shown |
| I | Every access change is one command in the tenant's own chain and shows who did it | docs/howto-roles.md:164-170 | access_profiles.rs:557 a_change_is_one_command_in_the_tenant_chain |  |
| I | An exported tenant brings its profile (op lines, overrides, groups) to the destination, never to the wrong person and never merged | docs/howto-roles.md:172-188 | crates/ordo-host/tests/import_op.rs:791 an_exported_profile_is_restored_exactly_in_a_fresh_tenant; :849; :892; :910 |  |

### Membership and joining

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A removed or suspended member is refused on their next request | skills/ordo/guards-and-roles.md:101 | membership_commands.rs:257 a_suspension_through_the_command_is_refused_on_the_next_request; :294 | No browser test of a removed person's open page |
| I | The last active owner can never be removed, suspended or demoted | skills/ordo/guards-and-roles.md:106 | membership_commands.rs:354 the_last_owner_cannot_be_removed_suspended_or_demoted |  |
| I | Only owners change roles/remove; a manager cannot invite an owner | skills/ordo/guards-and-roles.md:95-103 | membership_commands.rs:409; invitations.rs:255 |  |
| I | An invitation works once, expires, can be revoked or declined, and a named invitation is spent only by that person; no role means viewer | skills/ordo/guards-and-roles.md:97; docs/howto-sharing.md:119-125 | invitations.rs:177,198,209,227,286,343,453,557 | Invitation e2e is share-only; tenant-join invite not in a notes spec |
| I | A stranger gets no tenant under invite-only; auto-onboard and self-serve give one as owner | skills/ordo/guards-and-roles.md:118-122 | crates/ordo-host/tests/auth_guest.rs:483 under_invite_only_a_new_guest_is_refused_like_any_newcomer; provision_own.rs; admission.rs | Not named per setting; e2e notes uses on-signup (door.spec.ts:92) only |
| I | A person can leave a tenant themselves | skills/ordo/guards-and-roles.md:104 | crates/ordo-host/tests/membership_commands.rs (leave not found by name); sharing.rs:768 covers a guest leaving | Member leave by std.space.leave: no named test found |
| I | A platform op that changes something is refused without a fresh reauth; the deployment owner's exempt list works; delegated tokens never get it | skills/ordo/guards-and-roles.md:108-114 | crates/ordo-host/tests/presence.rs:212,296,428 | These drive the table and door (sync fns); e2e passkey.spec.ts:59 shows the touch for share |

### Sharing with a guest

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| E | A guest reads only the rows and ops the sharer named, read-only, and nothing else in the tenant | docs/howto-sharing.md:17-21 | examples/notes/e2e/tests/browser/passkey.spec.ts:59 a shared note is read-only for the reader; sharing.rs:1502 a_window_token_reads_exactly_the_shared_rows_and_is_refused_in_any_other_space |  |
| I | A share is refused before a ticket exists if it lists a write, a platform op, an unguarded op, an unguarded-exposure op or an op beyond the sharer | docs/howto-sharing.md:46-60 | sharing.rs:482 sharing_an_unguarded_op_a_platform_op_or_an_op_beyond_the_sharer_is_refused_before_a_ticket_exists; :877 | Write-op refusal not an e2e |
| E* | When the sharer takes a share back, the guest's next call is refused and open screens lose the data at once, with no reload | docs/howto-sharing.md:68-95,164-175 | passkey.spec.ts:59 (Stop sharing, Bo's feed empties); sharing.rs:648; feed_door.rs:371 a_revoked_share_ends_a_members_window_feed | Also when the sharer deletes the note: ui/notes.spec.ts:221 |
| I | A ticket works for the right person once; a wrong holder cannot spend it; a ticket can be declined; every refusal is one answer share_refused | docs/howto-sharing.md:112-160 | sharing.rs:1331; :1872; :420 | Same-answer rule not asserted across all causes in one place |
| I | A guest can leave a share and is refused after | docs/howto-sharing.md:93 | sharing.rs:768; :1749 |  |
| I | The consent the guest sees is shown (what accepting discloses, owner can reach everything) and recorded in both logs | docs/howto-sharing.md:127-142 | sharing.rs:1097 an_accepted_share_binds_a_seat_records_the_grant_and_writes_the_acl_rows_as_the_sharer | Disclosure wording not asserted in e2e; screenshot only |
| I | If the grant registry is down, shares are refused and members are not | docs/howto-sharing.md:89 | sharing.rs:836 an_unreachable_grant_registry_refuses_window_calls_and_serves_members |  |
| I | A window token cannot write, step up, open a window, accept a share, or reach another tenant | docs/howto-sharing.md:151-157 | sharing.rs:1612; :1502 |  |

### Share links (no account)

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A link opens exactly the shared rows, read-only, and never writes, calls platform ops or reaches other rows or tenants | docs/howto-sharing.md:410-417 | share_links.rs:834 an_opened_link_reads_exactly_the_shared_rows_and_nothing_else; :974 | Open without password in e2e not found (only the password case) |
| I | An unknown, expired, revoked or switched-off link is one answer, and revocation bites on the next call | docs/howto-sharing.md:382-386,402 | share_links.rs:897; :1099; :539 |  |
| E | A link with a password asks for it, refuses a wrong one and opens with the right one | docs/howto-sharing.md:276-300 | examples/notes/e2e/tests/v2/search-and-links.spec.ts:95 |  |
| I | Wrong link passwords are slowed (always) and may lock where a layer turns it on; limits are layered and refused out of range | docs/howto-sharing.md:288-330 | share_links.rs:2333; :1565; :1751; :2417; :2498 |  |
| I | A link can be limited to N opens per device, reloads and renewals do not count, and a race cannot pass the limit | docs/howto-sharing.md:248-270 | share_links.rs:2105; :2181; :2215 | Reader-side device id in browser not e2e |
| I | A link serves the file of a shared row and no other row's file | docs/howto-sharing.md:224 | share_links.rs:1329 a_link_serves_the_file_of_a_shared_row_and_not_another |  |
| I | The link secret is shown once and stored only as a hash; lists never show it | docs/howto-sharing.md:215,407 | share_links.rs:330; :1174 |  |
| I | The lifetime has a default, tenant choice only where exposed, and a tenant bound holds | docs/howto-sharing.md:229-244 | share_links.rs:563; :614 |  |

### Reads cannot be accidentally expensive

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | An op whose work grows with the space cannot deploy unless it says `unbounded`, and the word is viral through composition | docs/howto-cost.md:82-125 | crates/ordo-conformance/tests/corpus_p0.rs:620; guards_corpus.rs; ordo-host/tests/cost_class.rs:406,509 |  |
| I | Open-field refinements never trip the cost gate and never get refused for cost | docs/howto-cost.md:2,218 | cost_class.rs:547; refine_door.rs:47 every_parity_case_answers_through_the_door |  |
| I | A sealed condition that would examine unbounded rows is refused (refinement_unbounded) rather than served | docs/howto-cost.md:313-323 | cost_class.rs:406 a_sealed_refinement_cannot_walk_around_the_unbounded_gate; refine_door.rs:353 |  |
| I | A sealed condition the host would answer is refused with its class (sealed_unsupported 501) naming the field and the fix | docs/howto-cost.md:325-340 | refine_door.rs:312 a_sealed_unindexed_condition_is_refused_at_the_door |  |
| I | Sealed refinement is equality only, opt-in per field, and never under a count (no oracle) | docs/howto-cost.md:182-190,253-255 | refine_audit.rs:521 a_count_refuses_a_sealed_condition_and_a_list_serves_equality_only; check_refinable.rs:106 |  |
| I | A malformed refinement is a typed 400 with its field and message, never a 500, whatever the operator or value | docs/howto-cost.md:231-268 | refine_audit.rs:197 every_operator_on_every_field_against_hostile_values_is_never_a_500; :333; refine_door.rs:360 |  |
| I | The size limits (16 conditions, 200 in-values) hold at the door and name the setting | docs/howto-cost.md:285-303 | refine_audit.rs:664 the_size_limits_hold_at_the_boundary_through_the_real_door; refine_door.rs:546 |  |
| I | A refined list pages full and count and list agree under one refinement | docs/howto-cost.md:400-415 | refine_paging.rs:47,85,144 |  |
| I | Reading a bounded sealed page opens one key release per page, and a page that opens nothing releases nothing; anonymous callers never open sealed values | docs/howto-cost.md:50-62 | sealed_reads.rs:59 a_sealed_list_is_served_page_by_page_with_one_release_per_page; :212 |  |
| I | `ordo check` prints the class and what each read may be refined by, and agrees with the descriptor | docs/howto-cost.md:65-79,172-205 | crates/ordo-cli/tests/check_refinable.rs:55,142 |  |
| I | A refinement is refused on a write or a platform/public read, and a POST on a read is method_mismatch | docs/howto-cost.md:152-156,251-252 | crates/ordo-api/tests/api.rs:1239 post_on_read_op_is_method_mismatch | Write-refinement refusal message itself not found asserted |
| I | A refined live read stays refined, and a sync takes where and refuses pick, where rows entering and leaving arrive as changes | docs/howto-cost.md:419-445 | api.rs:3940,3963,3986,4014 | Router with fake executor, not a real database; the device-side refusal is clients/ordo-sdk/test/sync.test.ts:342 (U) |
| U | The device answers the same refinement the server would, with the same rules (parity) | docs/howto-cost.md:449-459 | clients/ordo-sdk/test/refine-parity.test.ts:135; crates/ordo-conformance/tests/refine_parity.rs:174 the server side | Server and device each pass the same table, but no run puts a real client against a real server |
| N | Indexing and slow-open-field warnings: nothing warns on an unindexed open filter (documented gap) | docs/howto-cost.md:371-381 |  | Doc says nothing logs the effective class today; promise is only advice |

### Public pages and tenant data (cross-cutting concepts)

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A public call (no sign-in) can read only fields marked public, never a private or sealed field, and cannot compose a private op | docs/std-library.md:8 (public ops; see docs/howto-public.md); crates/ordo-plan/tests/public_ops.rs | crates/ordo-conformance/tests/public_catalogue.rs:49 a_private_field_cannot_be_reached_by_a_public_op; ordo-host/tests/public_path.rs:238 the_anonymous_door | Public docs are outside this area; counted here because the owner named it. No Playwright run of a public page |
| I | Tenant (member-only) data is never served on a public page or from the public cache | docs/std-library.md:(owner addendum) | public_path.rs:365 the_cache_and_its_invalidation (members do not use the public cache, line 358); public_name_collision.rs:209,241 | Cache-leak across tenants not asserted with two tenants |
| I | The public read cache is used: a second identical read is a hit and a write invalidates it | docs/std-library.md:(owner addendum) | public_path.rs:365 the_cache_and_its_invalidation (x-ordo-cache miss/hit); :438 the_cdn_headers | Counts cache header, a real observation |
| I | A public op can read public data for a signed-in tenant caller too (same rows the op keeps) | docs/std-library.md:(owner addendum) | public_name_collision.rs:241 a_signed_in_caller_gets_only_the_rows_the_op_keeps |  |

### Loading and staying current

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| U | A page draws from what it has at once, says whether it is complete, and never pretends to be done | docs/howto-loading.md:29-33,98-102 | clients/ordo-react/test/device.test.ts:621 complete_is_never_true_with_a_partial_row_set | Mocked server; no e2e asserts the Still loading hint |
| E | Live updates reach another client with no reload: a change in one tab or by another person shows up | docs/howto-loading.md:23-27,53-58 | examples/notes/e2e/tests/ui/notes.spec.ts:54 a note written in another tab ... shows in the first tab with no reload; passkey.spec.ts:59; api/door.spec.ts:157 |  |
| E* | After the network returns the page catches up by itself; offline the notes stay on screen and writes wait | docs/howto-loading.md:53-58,242 | notes.spec.ts:106,322; passkey.spec.ts:103 |  |
| U | A live read refetches at most once per pace and keeps old rows on screen during the refetch | docs/howto-loading.md:23-27,211-214 | clients/ordo-react/test/live.test.ts:206,221,585 | Mocked stream |
| I | Live read stays correct as data changes: an op's feed is re-asked on every pass and ends on logout, removal, revoked share or withdrawn delegation | docs/howto-sharing.md:170-175 | crates/ordo-host/tests/feed_door.rs:315,371,405; api tests/feed_door_guard.rs:50 (source scan) |  |
| U | A device copy loads in frames, resumes on a hash, and releases rows a reload leaves out; a row held by two syncs stays until neither holds it | docs/howto-loading.md:288-303 | clients/ordo-sdk/test/sync.test.ts:89,110,146,256 | Server half: api.rs:2984 (fake executor); browser half via notes e2e offline test |
| U | A device copy never leaves rows after the person signs out or the access is revoked (purge, poison) | docs/howto-loading.md:246-247 | clients/ordo-sdk/test/device.test.ts:91; ordo-react/test/device-gaps.test.ts:134,159 | Sign-out purge across a real IndexedDB not e2e; passkey.spec.ts:127 only re-signs in |
| U | A sync nobody uses closes after 15 s and rows unused for 7 days are erased | docs/howto-loading.md:252-265 | clients/ordo-sdk/test/retention.test.ts:26,45,129 | Fake clock and memory backend |
| U | Tabs share one set of feeds with one leader and the others follow | docs/howto-loading.md:266-275 | clients/ordo-sdk/test/tabs.test.ts:265 (only the no-locks fallback found by name) | Leader election in a real browser with several tabs: not found |
| E | A call with no answer fails as blocked, unreachable or timeout, each naming its fix | docs/howto-loading.md:59-61 | examples/notes/e2e/tests/ui/notes.spec.ts:65 the SDK can tell a blocked origin from a dead door; ordo-sdk/test/network.test.ts:59-136 |  |
| I | Server settings for loading (frame rows, wait, live idle close, max reads) are honoured and a bad value refuses boot | docs/howto-loading.md:329-359 | crates/ordo-api/tests/api.rs:3040,3060 (sync wait, frame rows) | Live idle close and max reads and boot refusal: no test found |
| U | No polling: the SDK never reloads on focus and runs no timer | docs/howto-loading.md:53-58,249 | clients/ordo-sdk/test/device.test.ts:101 an online event nudges every feed; there is no focus reload and no timer | Absence of timers in a real page not observed |
| U | Long lists page: more() appends, a refinement rides every page, and stale cursors restart cleanly | docs/howto-loading.md:165-193 | clients/ordo-react/test/pages.test.ts:10,32,153 | Against a mock; server side refine_paging.rs is I |

**Promised but marked unbuilt, not graded:**

- A group that holds a role (howto-roles.md:196)
- Packages that carry the access template (howto-roles.md:198)
- A guest that writes (howto-sharing.md:424)
- blob private content in a link or share (howto-sharing.md:422)
- Taking back acl rows a share wrote for a role-holding member (howto-sharing.md:425)
- Removal of expired and revoked link rows (howto-sharing.md:423)
- Host-side filtering of sealed fields and sealed aggregates (howto-cost.md:47,325)
- Logging a request's effective cost class (howto-cost.md:470)
- std.E.list/whole/under/rollup/history derivation (std-library.md:30-33)
- Consented feeds between tenants and the anonymous door for a paywall (sharing-options.html, access-model-explained.html: design discussion, superseded)
- Per-space activation / system space (access-model-explained.html: proposed)

## Custody, files, audit trail, leaving


### Being able to verify

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | Anyone with the database can prove a space's history was not rewritten; a doctored or deleted group is named by index and kind, exit 3 not 1 | docs/howto-audit.md:25 | crates/ordo-cli/tests/operator.rs:313 verify_names_the_first_bad_index_and_its_kind; :371 a_deleted_entry_breaks_the_link; :2847 the_binary_exits_0_..._3; ordo-exec/tests/commandlog.rs:335 | No example app or e2e workflow runs ordo verify against a live stack. An example test would run a notes e2e, then ordo verify on its DB, then doctor a row and see exit 3. |
| I | Every answered request carries a signed head a client can check offline; a refusal carries none (never leaks a space's size) | docs/howto-audit.md:31 | crates/ordo-host/tests/deployment.rs:2091 every_answered_request_carries_a_verifiable_pin (refusal case at :2144); ordo-host/tests/attest.rs:80 the_pin_follows_the_chain_and_never_goes_backwards | Server half only. No test shows a client (SDK or React) storing a pin and flagging a rewind or a bad signature, which is the detection the doc promises. x-ordo-verifiable-from jump is at deployment.rs:2227 (server side). |
| I | Removing the last entries of a chain or security stream is caught, not only edits in the middle | docs/howto-audit.md:171 | crates/ordo-cli/tests/operator.rs:495 a_truncated_tail_is_caught_by_the_checkpoint; :3907 security_entries_removed_from_the_end_are_caught_only_by_the_signed_checkpoint; :4018 | A keyless run says it proves less: covered at :3907. |
| I | The command log and the security stream are verified apart; a broken one never hides, or is hidden by, the other | docs/howto-audit.md:137 | crates/ordo-cli/tests/operator.rs:3824 a_broken_security_stream_does_not_hide_behind_a_green_chain; :3866; ordo-exec/tests/secevent.rs:305 the_two_artifacts_verify_independently_and_under_different_keys |  |
| I | Rotating the security key never makes the old record read as tampered (unreadable, not failed); old markers are never re-signed | docs/howto-audit.md:211 | crates/ordo-cli/tests/operator.rs:4244 a_rotated_security_key_reads_as_unreadable_not_as_tampering; :4330 |  |
| I | Pruning or retention can never be used to launder a tamper: a checkpoint is refused over history that does not verify, and a tampered prefix is never deleted | docs/howto-audit.md:264 | crates/ordo-cli/tests/operator.rs:2333 a_checkpoint_is_refused_over_history_that_does_not_verify; :4094; ordo-exec/tests/cadence_prune.rs:574, :607 a_prune_refuses_to_destroy_a_tampered_prefix, :316 |  |
| I | A whole-database restore from backup is detected as a rewind unless it was recorded; two restores are two discontinuities | docs/howto-audit.md:29 | crates/ordo-cli/tests/operator.rs:1005 a_later_unannounced_rewind_is_rewound_not_explained; :599; :3159; ordo-host/tests/restore_witness.rs:111; ordo-host/tests/restore_drill.rs:413 | Doc coverage is indirect (howto-audit does not describe restore); the promise comes from the pin and checkpoint sections. |
| I | A third party can verify a chain proof without the deployment; no key means UNVERIFIED, a key they do not trust is no evidence, a pinned head turns it into a rewrite check | docs/howto-audit.md:422 | crates/ordo-host/tests/attest_webhook.rs:615 a_chain_proof_verifies_offline_and_stops_when_a_link_moves; :663 a_proof_without_a_key_says_unverified; :677; :719 below the floor; :741 platform refuses to publish a proof over its broken chain | Drives ordo_verify_proof::verify_proof as a library over a real database. The ordo-verify-proof binary (arg parsing, exit codes 0/1/3 as the process returns them) is never run; v.code()==3 only is asserted. |
| I | The offline verifier cannot reach a deployment (no socket, no database driver, no environment) | docs/howto-audit.md:454 | crates/ordo-verify-proof/tests/offline.rs:42 the_verifier_cannot_reach_a_deployment; :72 the_dependency_set; :85 nothing_in_the_source_opens_a_socket | Static dependency and source scan, which is the doc's own argument. |
| N | A chain proof discloses no space content (no rows, images, actors, correlation ids) | docs/howto-audit.md:429 |  | No test asserts the proof JSON lacks content fields. An example test would build a proof over a space with known row text and actor names and assert none appears in the string. |
| I | A delivered attestation authenticates twice (HMAC at ingest, Ed25519 later); the platform cannot redirect it by editing the row; a destination inside the perimeter is refused | docs/howto-audit.md:283 | crates/ordo-host/tests/attest_webhook.rs:291 a_delivery_round_trip_reaches_the_receiver; :380 the_platform_cannot_redirect_a_delivery_by_editing_the_row; :429; :360; :551 | Receiver is a test listener, not an example app. |
| I | Silence is detectable: delivery is at least once and never re-delivers a landed position; gaps follow a declared cadence; failures dead-letter with a fixed word, not network detail | docs/howto-audit.md:397 | crates/ordo-host/tests/attest_webhook.rs:462, :498, :1091, :1135; ordo-exec/tests/cadence_prune.rs:205 attestation_fires_on_volume_and_on_time_independently |  |
| I | Only the tenant owner with fresh presence can set, read, switch off a destination; a capability token can never nominate one | docs/howto-audit.md:312 | crates/ordo-host/tests/presence.rs:191 (chainProof listed); ordo-host/tests/authz_support/policy.rs:384,441; attest_webhook.rs:983 the_owner_switches_the_destination_off_and_reads_it_back | Authz matrix covers refusal by role; confirm attestTo with a capability token is in presence.rs list (attestTo appears in policy.rs, chainProof in presence.rs). |
| I | Revert only appends: history shows both the write and the undo; a revert needs the right to have made the write; a second revert of the same group is refused | docs/howto-audit.md:85 | crates/ordo-cli/tests/operator.rs:2368 a_revert_round_trips_a_write_and_lands_in_the_chain_as_new_history; :2486; :2600; :2659; :2730 |  |
| I | Imported or exported history keeps its integrity: the exported command log carries hashes that verify, a flipped byte in a bundle member is caught | docs/migration.md:393 | crates/ordo-exec/tests/export.rs:1105 the_command_log_travels_with_the_hashes_that_verify_it; :457 a_flipped_byte_in_any_member_is_caught_by_the_manifest |  |
| I | A blob upload is part of the verifiable history (one converge group, rev moves, revert restores the old file, a tampered or absent object refuses or is recorded) | docs/howto-files.md:35 | crates/ordo-host/tests/blob_door.rs:1301 an_upload_appends_one_converge_group; :1385; :1415; :1451; :1971; :2066 | docs/blob-chain-explained.html:2 and §2 still say an upload appends no group and is an open ruling. The tests show it is built; the page is stale. |
| U | The security stream stays bounded under a denial sweep, and a sweep across spaces is still visible as separate entries | docs/opslog-growth-explained.html:3 | crates/ordo-host/tests/opslog_d2.rs:52 a_non_member_denial_names_the_space_it_probed; ordo-secevent/src/tests.rs (collapser); ordo-exec/tests/secevent.rs:596 | The 256 per window cap and Suppressed marker are tested at unit level. No test runs a many-space probe through the door and counts entries. Key releases are not collapsed by design. |
| I | Every key release for a sealed read or write is counted and recorded on the deployment stream | docs/opslog-growth-explained.html:4 | crates/ordo-host/tests/key_release_metered.rs:301 a_served_sealed_write_and_read_are_counted_and_recorded; :365 | The page notes the stream wiring was demonstrated only in unit tests at the time; this test now wires it. |

### Sealed values stay unreadable without the key

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| U | Someone who holds the database and no key learns no sealed value, and the command log never holds plaintext | docs/howto-custody.md:18 | crates/ordo-exec/tests/sealed.rs:204 one_value_written_twice_is_two_handles_over_two_rows; :302 | Tests check the image carries handles and ciphertext differs per write. No test scans every raw column, the log and the payload table for the plaintext after a write through the door (blob_door.rs has a scan_column helper for files). An example test would write a known secret and grep a pg_dump. |
| I | A sealed envelope moved to another row, field or entity never opens as that row's value | docs/howto-custody.md:28 | crates/ordo-exec/tests/sealed_reads.rs:488 a_moved_envelope_refuses_rather_than_open_as_another_rows_value; sealed_through_join.rs:74 |  |
| I | A read returns the plaintext only to someone the guard admits; anonymous callers open nothing; a page costs one key release | docs/howto-custody.md:22 | crates/ordo-exec/tests/sealed_reads.rs:411 the_guard_decides_what_opens_and_anonymous_opens_nothing; :253; ordo-host/tests/sealed_reads.rs:59, :212 |  |
| I | A sealed field is never queried wrongly: filter, sort, range, group and unbounded reads are refused by name (sealed_unsupported) rather than answered from ciphertext | docs/howto-custody.md:24 | crates/ordo-api/tests/api.rs:2694; ordo-host/tests/cost_class.rs:528, :570; ordo-conformance/tests/corpus_p0.rs:674; corpus.rs:375; ordo-exec/tests/sealed_reads.rs:460 |  |
| I | Blind index: equality, in, unique, and upsert by a sealed key work; null admits exactly one row; normalize(lower) makes case variants one value | docs/howto-custody.md:58 | crates/ordo-conformance/tests/corpus_stage5.rs:1562 upsertCustomer; :1722 (8l null sentinel, real Postgres); ordo-host/tests/sealed_reads.rs:134; ordo-exec/tests/sealed.rs:496 | Digests differing between tenants is not asserted anywhere I found. |
| I | A ranged write to a sealed field seals once per row, so every row's seal is bound to its own id | docs/howto-custody.md:30 | crates/ordo-conformance/tests/row_bound_seals.rs:173 |  |
| I | Key rotation re-encrypts everything off the old epoch with no write barrier, a row written during the pass is not clobbered, digests never move, a killed pass resumes | docs/howto-custody.md:128 | crates/ordo-exec/tests/reseal.rs:403, :526, :586, :636; ordo-host/tests/reseal.rs:282 a_rotation_reseals_under_the_supervisor; ordo-cli/tests/keys.rs:379 | No example app test rotates under live traffic. An example test would rotate during the notes e2e and assert notes still open. |
| I | An epoch cannot be destroyed while anything still uses it (refused with the count); after destruction a captured envelope can never open | docs/howto-custody.md:167 | crates/ordo-exec/tests/reseal.rs:692; ordo-cli/tests/keys.rs:379 (refused: 1 payloads still under epoch 1; --confirm token); ordo-exec/tests/erase.rs:200 erasure_makes_a_surviving_envelope_unopenable |  |
| I | A value that will not open says why: destroyed, unreadable (possibly tampered), or store busy; a busy store is never read as destroyed | docs/howto-custody.md:189 | crates/ordo-exec/tests/sealed.rs:578, :752; ordo-crypto/tests/key_error.rs:8, :30; ordo-exec/tests/sealed_reads.rs:538 | The alert half (ordo_sealed_unreadable_total, sealed-unreadable event once per 5 minutes) is unit only: ordo-host/src/metrics.rs:1667. The /health/deep sealed_decrypts persistence is tested at ordo-host/tests/decrypt_seen.rs:36, not the live endpoint. |
| I | The development master key is refused outside development; a sealed master opens only with its passphrase | docs/howto-custody.md:132 | crates/ordo-cli/tests/keys.rs:33; ordo-crypto/tests/master.rs:24, :58 |  |
| I | A revert of a sealed write restores the sealed value, and the restored row is findable by its blind index again | docs/howto-custody.md:243 | crates/ordo-exec/tests/sealed.rs:372, :496 |  |

### Bring your own storage, safely

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | Only the tenant owner with fresh presence can attach, change or detach a bucket; an app's capability token cannot | docs/howto-byo-custody.md:92 | crates/ordo-host/tests/attach_bucket.rs:336, :372, :1208 |  |
| I | A bucket attach that would expose data is refused: off-allowlist host (one reason), plaintext endpoint, the deployment's own bucket, another tenant's own-perimeter bucket, a credential the bucket rejects | docs/howto-byo-custody.md:117 | crates/ordo-host/tests/attach_bucket.rs:711, :754, :848, :911, :955 |  |
| I | A credential label or secret never appears in an answer or on a request | docs/howto-byo-custody.md:196 | crates/ordo-host/tests/attach_bucket.rs:464 the_attach_response_never_names_the_credential_label | Boot refusal of group- or world-readable credential files and empty key halves: no test found (attached_tenant.rs:214 only builds a 0o600 dir). Part N. |
| I | Changing or detaching a bucket that holds files loses nothing: old files still read, are swept, erased, exported and verified in the bucket they were written to | docs/howto-byo-custody.md:150 | crates/ordo-host/tests/blob_door.rs:3128, :3167, :3211, :3259; :2885 an_upload_that_lands_during_a_detach_stays_readable | The negative (door answers an error, never not-found, when the old credential is gone) is not asserted. |
| I | Deleting a tenant deletes platform-held files, leaves files in the tenant's own bucket, makes them unopenable, and reports them (digest, count, epochs) | docs/howto-byo-custody.md:198 | crates/ordo-host/tests/exit_ops.rs:855, :951 erasing_a_space_leaves_files_in_its_own_bucket_and_records_them; ordo-exec/tests/blobgc.rs:900, :939, :966 |  |
| I | Custody changes are recorded on the deployment's stream and never on the tenant's | docs/howto-byo-custody.md:191 | crates/ordo-host/tests/attach_bucket.rs:1071, :1137, :1239; ordo-cli/tests/custody_ops.rs:12 |  |
| I | A tenant's database is never moved or detached once attached; a database another tenant uses or the control plane is refused | docs/howto-byo-custody.md:67 | crates/ordo-exec/tests/spacedb.rs:90 a_repeat_is_unchanged_and_a_different_target_is_refused; :147; ordo-host/tests/attached_tenant.rs:650; ordo-host/tests/tenant_create.rs | Exact refusals for control-plane DB and another tenant's DB were not confirmed by name. Treat that part as U. |
| I | A tenant database built on a cluster of its own serves the tenant from there and not from the deployment's | docs/howto-byo-custody.md:49 | crates/ordo-host/tests/attached_tenant.rs:650 an_attached_tenant_is_served_from_its_own_cluster |  |

### Files

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A person who may not see a row cannot read its file or its type and size; a handle from another space is not found | docs/howto-files.md:32 | crates/ordo-host/tests/blob_door.rs:535, :587, :761; ordo-api/tests/blob_door.rs:300 |  |
| I | An uploaded file can never run script on the door's origin (sandbox CSP, nosniff, active types saved not shown) | docs/howto-files.md:51 | crates/ordo-api/tests/blob_door.rs:352, :505, :569, :601, :632; ordo-host/tests/stored_bytes_scan.rs | Headers asserted. No browser test opens an uploaded HTML or SVG in the notes app and sees no script run. That is the real outcome. |
| E | Upload and download work in a real app: attach an image and a file, the image draws, the file saves with its bytes, a shared reader sees it, removing it hides it | docs/howto-files.md:101 | examples/notes/e2e/tests/upgrade/v1-to-v2.spec.ts:88 two people with notes, tags and a share under v1 ... attach and download (:168-:230) | Collection of the removed file's bytes is not asserted in the browser (blob worker side is I in blobgc.rs). |
| I | An upload is a write: it moves rev, fires events, is revertible, and is refused (and writes nothing) when the guard refuses | docs/howto-files.md:35 | crates/ordo-host/tests/blob_door.rs:1385, :1415, :1451, :1706, :829 |  |
| I | When file storage is down or busy the door answers retryable server_busy with nothing changed; a permanent refusal is never leaked in the answer | docs/howto-files.md:38 | crates/ordo-host/tests/blob_door.rs:476, :510; ordo-blob/tests/s3_retry.rs:105 | Retry-After on the blob route specifically and 408/425/429 mapping not found; the generic 503 Retry-After is at ordo-api/tests/api.rs:2026. |
| I | Oversize or undeclared-length uploads are refused before storing; private and sealed blob fields are refused by name | docs/howto-files.md:28 | crates/ordo-api/tests/blob_door.rs:256, :283; ordo-host/tests/blob_door.rs:636, :682, :720 |  |
| I | A failed or abandoned upload leaves no orphan, and a file's bytes outlive its last reference until the undo horizon passes | docs/howto-files.md:19 | crates/ordo-host/tests/blob_door.rs:995, :1065, :1196; ordo-exec/tests/blobgc.rs:296, :347 |  |
| I | A function reads a file as its caller and the row guard still holds; a public row's file is served and a private row's is not | docs/howto-files.md:32 | crates/ordo-host/tests/blob_door.rs:3439, :3546 |  |
| U | SDK and React file helpers (progress, release, abort, error classes) | docs/howto-files.md:101 | clients/ordo-sdk/test/files.test.ts; clients/ordo-react/test/files.test.ts | Mock fetch. The notes e2e exercises the happy path only; cancel, abort and progress are not run against a server. |

### Leaving

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | With no release rule nobody can leave; the app's rule decides, a refusal names the held condition and never a row; the last owner and an own tenant cannot leave | docs/howto-leave.md:8 | crates/ordo-host/tests/member_release.rs:312, :528, :548, :575; ordo-cli/tests/release_check.rs:24 |  |
| I | After leaving, the person has no access: seat refused at the door, sessions end on every replica, rows they wrote stay unless the app says remove or reassign | docs/howto-leave.md:51 | crates/ordo-host/tests/member_release.rs:346, :367, :623, :636, :703 | No browser test: a person leaves and then reloads and is refused. An example test would use notes with a release line and a second browser context. |
| I | A leave is one reversible command group attributed to the leaver; reverting restores the seat | docs/howto-leave.md:125 | crates/ordo-host/tests/member_release.rs:1279, :1371 |  |
| I | A person's share grants are revoked, people they made become independent, a guest can leave | docs/howto-leave.md:136 | crates/ordo-host/tests/member_release.rs:730, :763; ordo-host/tests/sharing.rs:765 (share/leave) |  |
| I | An account cannot be deleted while any tenant holds the person; the refusal lists each tenant and what stands in the way; it needs presence and the id typed back | docs/howto-leave.md:156 | crates/ordo-host/tests/member_release.rs:898, :935, :1208 |  |
| I | A deleted account can never sign in again and its data stays by default; removal follows ORDO_ACCOUNT_REMOVAL (never, now, n days), including a changed setting applying to those already deleted; history keeps only an opaque id | docs/howto-leave.md:183 | crates/ordo-host/tests/member_release.rs:996, :951, :1065, :1131, :1149, :1180 | Boot refusal of a bad ORDO_ACCOUNT_REMOVAL value and ORDO_MEMBER_LEFT_NAME over 256 characters: only the default is tested (:461). Not found. |
| I | A person can download their own data only if the app marked ops; they get only what they may read; delegated tokens and guests are refused by name; bounded per hour and by bytes | docs/howto-leave.md:241 | crates/ordo-host/tests/own_download.rs:336, :365, :449, :546, :571 | The zip content is E in notes v1-to-v2.spec.ts:193-223. Refusals, quota and size bound are not run through an example. The five download_* check refusals are unit only: ordo-plan/tests/download_ops.rs:61, :79. |
| I | An owner's export is complete, repeatable and loads into a fresh database; sealed values open for the owner, an unopenable one refuses instead of exporting null | docs/migration.md:393 | crates/ordo-exec/tests/export.rs:374, :677, :771, :932; ordo-host/tests/exit_ops.rs:373, :518 | Export, an exit promise, is not run from an example. Delivery to a hosted caller is unbuilt. |
| I | Deleting a tenant removes its rows, files, keys and platform tables, and the owner's own record survives; only the owner with the space id typed can do it | docs/migration.md:389 | crates/ordo-host/tests/exit_ops.rs:373, :617, :776, :1067; ordo-exec/tests/erase.rs:200, :578, :607 |  |
| I | Import arrives through the write protocol: sealed fields re-sealed under the destination, resumes without duplicates, refuses rather than guesses, never creates a membership, refuses people the map does not place | docs/migration.md:29 | crates/ordo-exec/tests/import.rs:294, :618, :749; ordo-host/tests/import_op.rs:411, :542, :570, :598; import_blobs.rs:309 |  |

**Promised but marked unbuilt, not graded:**

- ordo introspect and the mapping file (migration.md:52, ruled not wanted before first deployment)
- Genesis import mode with summarised batch groups, a signed genesis checkpoint and the didn't-migrate manifest (migration.md section 3 and 4, marked proposed)
- Dual-write sync bridge and cutover patterns as tooling (migration.md section 5, proposed)
- Delivering an export or import bundle to a hosted deployment (migration.md:38, :423)
- A tenant's own key, KMS binding, client-first vault space (howto-byo-custody.md:240; howto-custody.md:314)
- Moving a live tenant database (checklist 18e, after first deployment; howto-byo-custody.md:70)
- Moving old files into a new bucket, and the one-for-one attach exception (howto-byo-custody.md:180)
- Signed erasure receipt (17ab, howto-byo-custody.md:225)
- Host-side filtering on sealed fields (F0, howto-custody.md:26)
- Attestation targets WORM, Rekor, OpenTimestamps, git; retry of dead letters (howto-audit.md:411, :339)
- Resumable uploads, per-field inline/saved choice, reading type and size inside an op (howto-files.md:157, :66)
- Release mode as a layered setting member.release (24a); deployment owner deleting an account for someone who cannot sign in; scrubbing a name on removal (howto-leave.md:321)
- On-demand blobgc command (howto-custody.md:155 says none exists)

## Identity, sign-in, people, delegation


### Signing in and staying in

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| E | A person signs up with a passkey, gets a tenant of their own, writes data, and signing out and back in lands in the same tenant with the same data | docs/howto-sign-in.md:459 | examples/notes/e2e/tests/browser/passkey.spec.ts:127 signing out and in again with the same passkey lands in the same tenant | Real browser, virtual passkey. |
| E | A reload keeps a person signed in; the renewal cookie is out of script reach; after sign-out a reload stays signed out | docs/howto-sign-in.md:253 | examples/notes/e2e/tests/browser/passkey.spec.ts:18 signs up, reloads, still signed in | Positive path and cookie-hidden are E. The negatives (a spent cookie used again ends the family; only a listed origin redeems it; idle and tenant-off expiry) are I: crates/ordo-host/tests/auth_door.rs:934, :1049, :1128, :1384. |
| E | A second person never sees the first person's data | docs/howto-sign-in.md:32 | examples/notes/e2e/tests/browser/passkey.spec.ts:41 a second person signs up in another browser and cannot see the first person's note |  |
| I | A signed-out or logged-out token is dead even if copied; a session ends at its ceiling; ending every session kills every earlier token | docs/howto-sign-in.md:240 | crates/ordo-host/tests/auth_door.rs:267 a_logged_out_token_is_refused_by_the_door; :425 a_session_ends_at_its_ceiling; :678 ending_every_session_refuses_each_token_minted_before_it | No browser test replays a copied token after sign-out. |
| I | Session length is layered (tenant, app developer, deployment); a tenant can never exceed the app developer's range, and the refusal names the range | docs/howto-sign-in.md:301 | crates/ordo-host/tests/auth_door.rs:1247, :1303, :1327, :1473 (tenant shorter, fallback, outside range refused, overall limit ends a used sign-in) |  |
| I | Wrong passwords are slowed per account and never lock the rightful person out; an unknown account is slowed the same, so accounts are not revealed | docs/howto-sign-in.md:341 | crates/ordo-host/tests/guess_limits.rs:182, :232, :250, :274 |  |
| I | A read of platform state needs no reauth for a signed-in person; a change does; the level and per-op lists move that | docs/howto-sign-in.md:382 | crates/ordo-host/tests/presence.rs:212 at_high_a_read_needs_no_presence_and_a_change_does; :243 a_read_freed_by_default_is_not_delegable; :484 each_level_asks_where_the_table_says | Browser level only for the identity-service/console touch: clients/ordo-console-web/e2e/tests/identity.spec.ts:48. |

### Other ways in

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| E | A person signs in with an emailed code, signs out, and a later code returns the same account and data | docs/howto-sign-in.md:1130 | examples/notes/e2e/tests/browser/other-ways-in.spec.ts:26 | Happy path E. |
| I | An emailed code can never be abused: works once, only with the asking browser, wrong/late is one refusal, begin never says whether an address is known, per-address/device/deployment limits are invisible, a deleted account is never mailed | docs/howto-sign-in.md:1185 | crates/ordo-host/tests/auth_email.rs:248, :297, :317, :352, :372, :418, :574 | No browser test tries a reused or foreign code. |
| E | Discord/GitHub sign-in returns the same account; a denied sign-in leaves the person signed out; the hand-off works once and only with its binder; an unlisted return address is refused before any redirect | docs/howto-sign-in.md:914 | examples/notes/e2e/tests/browser/other-ways-in.spec.ts:42 and :61 (stand-in Discord); crates/ordo-host/tests/auth_oauth2.rs:325, :360 | E uses a stand-in provider; binder and return-address refusals are I only. |
| U | A forged, foreign or stale OIDC ID token signs nobody in (wrong issuer, audience, nonce, algorithm, time, another connection's code, a discovery doc naming another issuer); a rotated key works with no restart | docs/howto-sign-in.md:1543 | crates/ordo-identity/src/oidc_tests.rs:273 nonce, :303 audience/azp, :326 expiry/skew (unit); crates/ordo-host/tests/auth_oidc.rs:267 code from one connection refused at another, :295 rotated key, :343 discovery naming another issuer | Weakest part: no end-to-end test sends a token with a bad aud, nonce, exp or algorithm through the door (alg none/HS* refusal at oidc level not found). |
| I | OIDC keys and discovery stay usable when the provider is down for a bounded grace and the use is recorded; a provider outage reads as try-again, not sign-in failure | docs/howto-sign-in.md:1558 | crates/ordo-host/tests/door_stale_grace.rs; crates/ordo-host/tests/auth_oauth2.rs:487 a_provider_outage_or_rate_limit_is_try_again | Stale-grace test is for the token door; the OIDC JWKS 24h grace itself not confirmed by name. |
| E | Guest sign-in: off refuses; a guest resumes only with its secret; claiming Discord/email/passkey keeps its id and data; a deleted guest cannot resume; counted against the sign-up ceiling | docs/howto-sign-in.md:1391 | examples/notes/e2e/tests/browser/other-ways-in.spec.ts:71 a guest tries the app and writes a note; crates/ordo-host/tests/auth_guest.rs:262, :316, :404, :425 | E covers only start-and-write. Resume, claim, delete, off are I. |
| I | One account can hold several ways in, and an account is never joined to another by matching email; a stolen token alone cannot link or add a passkey | docs/howto-sign-in.md:1316 | crates/ordo-host/tests/auth_linking.rs:334, :378, :415, :494, :751 |  |
| I | The human check blocks scripted sign-ins and sign-ups with the one ordinary refusal, sends no address to the provider, and fails closed when the provider is down | docs/howto-sign-in.md:979 | crates/ordo-host/tests/auth_human.rs:222, :267, :283, :334; crates/ordo-host/tests/auth_email.rs:702 |  |
| I | One sign-in across subdomains: the person lands signed in without a shared cookie or a token in the URL; the code works once and only with the page's binder; another origin is refused; the new session never outlives the old one | docs/howto-sign-in.md:1264 | crates/ordo-host/tests/auth_handoff.rs:217, :298, :323, :337, :349; clients/ordo-sdk/test/handoff.test.ts | No browser test crosses two subdomains. |
| I | A page learns the ways in from the door without any secret, and never learns which tenants exist | docs/howto-sign-in.md:416 | crates/ordo-host/tests/auth_methods.rs:193, :218, :244; crates/ordo-host/tests/auth_tenant_providers.rs:1124 | notes e2e door.spec.ts:68 'the door lists the ways in' is also E at the API level (examples/notes/e2e/tests/api/door.spec.ts:68). |

### Joining and tenants

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A newcomer under invite-only gets nothing without a valid invitation (nothing stored, one refusal); an invitation works once, with every method, even when two race | docs/howto-sign-in.md:683 | crates/ordo-host/tests/auth_invitation.rs:416-596 (passkey, password, email, Discord, spent/expired/foreign refused, race spends once); crates/ordo-host/tests/auth_passkey.rs:492 | E exists only for share-by-invitation (examples/notes/e2e/tests/ui/notes.spec.ts:221), not for joining a tenant as a member. |
| I | A new person's tenant is made after sign-up answers, one tenant per person even with simultaneous sign-ins, finished after a restart, retried without ever being marked failed, and stuck setups are reported to the person and the owner | docs/howto-sign-in.md:459-545 | crates/ordo-host/tests/provision_own.rs:1906, :1941, :1959, :2077, :2119, :2197 | E only waits through 'Setting up your account' (examples/notes/e2e/tests/support.ts:88) without asserting it. The /metrics gauges ordo_tenant_setup_stuck are not asserted by any test found. |
| U | A call into a tenant that is not ready runs nothing, is refused tenant_not_ready, and the client retries by itself with the same idempotency key | docs/howto-sign-in.md:510 | clients/ordo-sdk/test/busy.test.ts:215 retrying tenant_not_ready | Client retry is unit-tested against a fake; no server test found that a real not-ready tenant answers 503 and runs nothing. |
| I | The sign-up ceiling per hour refuses the next sign-up and mints nobody; the per-address limit does not refuse an innocent address | docs/howto-sign-in.md:603 | crates/ordo-host/tests/provision_own.rs:1707, :1741, :1227, :1310 |  |
| I | A tenant's own provider admits newcomers into that tenant only, cannot sign in to another tenant, retiring it stops sign-ins at once, and a tenant always keeps one owner who needs no tenant provider (concurrent changes cannot remove them) | docs/howto-sign-in.md:1687 | crates/ordo-host/tests/auth_tenant_providers.rs:490, :572, :609, :660, :926, :1055, :1089 | Secret never shown/answered: :532. |
| I | A company login's two-step claim counts as presence and as the second step only where allowed, never reaches deployment acts, and a stale or single-factor claim mints none | docs/howto-sign-in.md:1605 | crates/ordo-host/tests/auth_oidc.rs:630, :674, :707, :729, :845, :867, :892 | The 400 presence_not_available answer for a non-OIDC method has no test. |

### Reauth and presence

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A reauth cannot be forged or borrowed: another person's passkey, a replayed or wrong-origin/rp/counter-regressed assertion, a cleared presence flag, a delegated or capability token, or a stale window all fail to step up | docs/security-logging-design.md:123-145 | crates/ordo-host/tests/passkey_attacks.rs:357-1054 (about 30 named attacks); auth_passkey.rs:723, :810 | A person with no passkey reauths by a fresh sign-in: second_factor.rs:2410. |
| I | Unlinking a person's last passkey is refused, and linking a new factor needs a fresh assertion from one already linked, naming both in the audit | docs/security-logging-design.md:218, :566 | crates/ordo-host/tests/auth_passkey.rs:1483, :1292, :1361 |  |
| I | A suspended member is denied at the door at once with a valid token, and the record says suspended | docs/security-logging-design.md:551 | crates/ordo-host/tests/http.rs:1187 suspended member same token 403 and audit says Suspended | Weakest part: the doc also promises revoking refresh family, closing live subscriptions and refusing pending sync; no test of those found. Reason code principal_suspended is not asserted. |
| I | A tenant can lower or raise its own touches only within what the app developer exposes; deployment acts can never be exposed | docs/security-logging-design.md:189 | crates/ordo-host/tests/tenant_touches.rs:98, :143, :251, :316 |  |

### Account email

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | An address counts only after its emailed code, a confirmation code never signs anyone in, changing or removing needs a reauth and mails the old address, and a required address refuses a covered op | docs/howto-sign-in.md:1829 | crates/ordo-host/tests/contact_email.rs:277, :316, :357, :401, :563, :633 | Reminder email e2e at examples/notes/e2e/tests/upgrade/v1-to-v2.spec.ts:252 touches mail, not the confirm flow. |

### Mail

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | Mail is queued, survives a restart, retried with growing delay, never falls back to plaintext, and private relays are refused unless named | docs/howto-sign-in.md:1025-1105 | crates/ordo-host/tests/mail_outbox.rs; crates/ordo-host/tests/smtp.rs; crates/ordo-host/tests/mail_api.rs | Not read line by line; another area may own this. |

### Deployment people (identity service)

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| E* | A first admin joins by the one first invite, once; a spent, expired or revoked link does not work and says what to do | docs/howto-identity.md:82 | crates/ordo-identity-service/e2e/tests/identity.spec.ts:138 and :235; flows.rs:66, :119 | Playwright against a throwaway identity stack, not an example app. |
| E* | A person manages passkeys (cannot remove the last), makes new recovery codes, and a recovery code on a new device makes a new passkey and nothing else; the old passkey then stops working | docs/howto-identity.md:231 | crates/ordo-identity-service/e2e/tests/identity.spec.ts:138; flows.rs:452; page.rs:295 |  |
| I | Ending a person refuses new sign-ins and renewals at once, ends their sessions and recovery codes; a held token dies within its life; the last admin cannot be ended | docs/howto-identity.md:118 | crates/ordo-identity-service/tests/flows.rs:842; service.rs:335, :119; clients/ordo-console-web/e2e/tests/identity.spec.ts:48 (invite and end an admin, E) | The 'token works until it expires' bound is only checked through the token-life test, not after an end. |
| I | A machine key signs CI in with nobody present, never asks for a reauth, is refused once ended, holds only its grants, mints only within its minting grant, and a key without the grant is refused grant_needed | docs/howto-identity.md:135 | crates/ordo-identity-service/tests/flows.rs:619, :717, :148; crates/ordo-cli/tests/login.rs:294 | No test runs 'ordo apply' with ORDO_MACHINE_KEY in CI; --public-key registration not found; 'machine-key end' via CLI not found. |
| E* | The CLI signs in through the browser with a passkey (code to loopback only; deny sends nothing), whoami names credential, session file is 0600, a script gets told to run login --touch | docs/howto-identity.md:178 | crates/ordo-identity-service/e2e/tests/identity.spec.ts:251; crates/ordo-cli/tests/login.rs:294, :446, :497 | 'ordo people invite' and 'ordo people end' from the CLI have no test found; only list and machine-key add/list. |
| I | Host break-glass works with no sign-in: invite link, 15-minute admin token, key rotation, audit verify, each audited | docs/howto-identity.md:240 | crates/ordo-identity-service/tests/flows.rs:863 |  |
| I | The signing key rotates without breaking verification, a retired key verifies nothing, and a tampered audit row is named; the audit chain checkpoints | docs/howto-identity.md:251 | crates/ordo-identity-service/tests/service.rs:168, :248, :499, :549; crates/ordo-host/tests/identity_tokens.rs:501 |  |
| I | A token for one deployment, an OIDC ID token or an app token is never accepted where it does not belong (hal's door refuses ordo's token; an ID token is no deployment token) | docs/howto-identity.md:421, :655 | crates/ordo-identity-service/tests/shared.rs:71; oidc_provider.rs:382; crates/ordo-host/tests/identity_tokens.rs:525, :570 |  |
| I | One service for many deployments: a person gets a deployment's token only while a member, a deployment's console sees only its own members, an ended deployment stops new tokens and brings no member back | docs/howto-identity.md:411 | crates/ordo-identity-service/tests/shared.rs:180, :526, :604 |  |
| E* | Open sign-up makes an account for that app only, with no token for the deployment (app_member_only); invite-mode answers invite_only; one passkey joins a second app; counted per address | docs/howto-identity.md:521 | crates/ordo-identity-service/e2e/tests/identity.spec.ts:343; open_sign_up.rs:114, :179, :249, :436 | Not through an app's own page. |
| I | An app signs its users in through the service as an OIDC provider: code works once for 60 s with PKCE, only the exact registered redirect, an unknown client is shown a refusal and never followed, one person is one subject across apps, an app's page cannot spend the session cookie | docs/howto-identity.md:572 | crates/ordo-identity-service/tests/oidc_provider.rs:306, :411, :504, :597, :644, :756; crates/ordo-host/tests/identity_service_oidc.rs:413, :529 | No example app's browser flow through the service found. |
| I | Guessing recovery or sign-up codes is limited per address and for everyone together, counted once across two instances, and the visitor address is read from a header only behind a trusted proxy | docs/howto-identity.md:400 | crates/ordo-identity-service/tests/flows.rs:497, :930, :964, :1022 |  |
| U | The passkey relying party stays the same: a start under another RP id warns and old passkeys do not sign in; a service address outside the RP id refuses boot naming both settings | docs/howto-identity.md:277 | crates/ordo-identity-service/tests/import.rs:334; crates/ordo-identity-service/src/config.rs:835 | Weakest part: the boot refusal is a unit test of config parsing. |
| I | An upgrade moves console owners in with passkeys and codes still working, never writes to the console database, runs once, and voids open console invites | docs/howto-identity.md:301 | crates/ordo-identity-service/tests/import.rs:229, :379, :464 | The Playwright upgrade case was retired 2026-10-31 (identity-e2e.yml). Never-writes-to-console is not seen asserted directly (fingerprint helper at :184 suggests it). |
| I | A deployment with no identity database set keeps console sign-in and shows the banner; the supervisor does not start the service | docs/howto-identity.md:60 | crates/ordo-console/tests/runtime.rs:498 without_the_identity_service_the_old_sign_in_stays_with_a_banner; crates/ordo-supervisor/tests/supervisor.rs |  |
| N | The service refuses a database URL that names the console's or the deployment's database | docs/howto-identity.md:56 |  | No test found for this refusal. |
| E* | The console on the identity service: a person's own token reaches the door, a stale touch asks for step-up, a token from another origin is refused, and it will not boot holding a console key | docs/howto-identity.md:36 | clients/ordo-console-web/e2e/tests/identity.spec.ts:48; crates/ordo-console/tests/identity_signin.rs:745, :822, :886 |  |

### Delegation

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A person lends an agent part of their rights through a consent ceremony: the agent asks, the person reads an accurate plain-words disclosure computed from the program, approves with a reauth, and the agent redeems a token; the agent never sees the session | docs/howto-delegation.md:7 | crates/ordo-host/tests/delegation_consent.rs:370, :441, :556, :594; clients/ordo-sdk/test/delegation.test.ts | examples/assistant scripts run the ceremony but no CI workflow runs them; assistant_app.rs:406 is Rust I. |
| I | A delegated agent never exceeds the person: it reads only the person's rows, writes are refused where the person cannot write, calls outside the allowlist are refused, deletes stay closed until opened, platform and presence ops are closed | docs/howto-delegation.md:96-109 | crates/ordo-host/tests/delegation.rs:963, :1012, :1053, :1083, :456, :1277; presence.rs:428; assistant_app.rs:571, :592 |  |
| I | An agent can never approve its own request; only the named person can read, approve or decline it; a stranger's request conveys nothing | docs/howto-delegation.md:124 | crates/ordo-host/tests/delegation_consent.rs:671 only_the_named_person_can_approve; :594; delegation.rs:456 | No test with a delegated token calling std.delegation.approve by name found. |
| I | Revoking or expiry stops the agent's very next call, and a live feed it holds ends with delegation_required | docs/howto-delegation.md:176 | crates/ordo-host/tests/delegation.rs:300, :362, :1137; delegation_consent.rs:738; crates/ordo-host/tests/feed_door.rs | The 30 s recheck with no write was not confirmed. |
| I | A request id seen on a screen is useless without its binder; a request gives one token; a decline grants nothing; a second approval replaces the first | docs/howto-delegation.md:158-160 | crates/ordo-host/tests/delegation_consent.rs:689, :793, :997, :717 |  |
| I | The open request route is bounded: unlisted agents refused, per-agent and per-person caps answer 429 delegation_limit naming the setting, the human check applies, and refusals name the value to set | docs/howto-delegation.md:53-70 | crates/ordo-host/tests/delegation_consent.rs:824, :867, :894, :934 |  |
| I | Every delegation step and every delegated write is recorded naming both the agent and the person | docs/howto-delegation.md:207 | crates/ordo-host/tests/delegation_consent.rs:960; delegation.rs:614, :1168; assistant_app.rs:489 |  |

### Tokens

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| U | A capability grant dies when an op's output expands, applies silently when it narrows | docs/security-logging-design.md:257-272 | crates/ordo-host/tests/pinning.rs:229, :296 (classification of a change) | Pinning classifies changes; no test found that a grant is actually killed by an expansion at the door. |

**Promised but marked unbuilt, not graded:**

- MCP binding for delegation (docs/howto-delegation.md:222)
- Agent credentials of their own; the request route stays open to any caller within caps (docs/howto-delegation.md:223)
- Rate and volume budgets per delegation, SH-20 (docs/howto-delegation.md:226)
- Roles beyond admin for deployment people (docs/howto-identity.md:661)
- Refresh tokens, userinfo endpoint, sign out of every app at once (docs/howto-identity.md:663)
- Sign-in through outside providers for deployment people (docs/howto-identity.md:665)
- Invite links sent by mail (docs/howto-identity.md:666)
- More than one copy of the identity service (docs/howto-identity.md:667)
- Machine keys for a registered deployment (docs/howto-identity.md:668)
- Reauth in a console on another domain than the shared service's RP id (docs/howto-identity.md:514)
- Mail APIs for Mailgun, Postmark, Brevo, Mailjet, SES (docs/howto-sign-in.md:1111)
- Password reset by mail, 'in a later step' (docs/howto-sign-in.md:1027)
- Suspension also revoking refresh family, closing live subscriptions, DEK rotation; notification, rollback window for linking (docs/security-logging-design.md:559, :580), described as obligations, status unclear

## Deploying, operating, console, CLI, routines, functions


### Deploying and upgrading never lose or break live data

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | An upgrade of the app or of Ordo itself keeps every person's sign-in, rows, files, shares, links, delegations, idempotency keys, and the command chain still verifies | deploying.md:1532-1565 | crates/ordo-host/tests/upgrade_continuity.rs:1995 a_compatible_program_update_keeps_everyone_and_everything; :2126 the_previous_ordo_hands_over_to_this_one | Real servers and real sign-ins, but no Playwright browser; the e2e upgrade (notes v1-to-v2.spec.ts:88) covers the program update only, not an Ordo binary upgrade |
| I | A page never calls an op the new version added before every node serves it: apply waits, names a slow node, and names a tenant left behind | deploying.md:1385-1450 | crates/ordo-cli/tests/apply_wait.rs:107 apply_returns_only_after_the_node_serves_and_caught_up; :173 a_timeout_names_the_node_and_what_it_is_doing; :155 no_wait; :204 stopped node not waited for | Tenant 'held' line: crates/ordo-host/tests/apply_everywhere.rs:317 a_refused_tenant_is_held_and_the_others_are_brought_forward. No browser polling /health/deploy test |
| I | A failed or refused deploy leaves the old version serving, and a program that does not compile stores nothing | howto-operating.md:5-148; deploying.md:1356 | crates/ordo-host/tests/deploy_apply.rs:635 a_program_that_does_not_compile_is_refused_and_stores_nothing; deploy.rs:372 a_swap_cannot_install_a_version_the_gate_refused; deploy_apply.rs:584 |  |
| I | Two applies at once never interleave; the second waits then refuses apply_in_progress | howto-operating.md:~90 | crates/ordo-host/tests/deploy_apply.rs:509 two_applies_serialize_rather_than_interleave |  |
| I | A schema change that could lose data is refused by name and writes nothing; additive changes apply with rows kept | quickstart.md:435-470; howto-operating.md:148-186 | crates/ordo-cli/tests/schema_apply.rs:462 a_transforming_change_is_refused_by_name_and_writes_nothing; :190 first_apply_creates_and_a_later_apply_keeps_the_rows | Migration is a CLI/DB test; no example-app e2e performs migrate (v1-to-v2 e2e applies through apply) |
| I | A tenant database not yet at the program's shape is closed, never served half-migrated, and reopens once brought forward | howto-operating.md:299-312 | crates/ordo-host/tests/schema_behind.rs a_target_behind_the_artifact_quarantines_its_tenants; versions_serving.rs a_database_not_in_its_versions_shape_is_closed_naming_the_fix |  |
| I | Rolling back is an ordinary activation and leaves a record; rolled-back version serves again with no restart | howto-operating.md:186-268; deploying.md:1362 | crates/ordo-host/tests/deploy.rs:87 a_rollback_is_a_re_point_and_leaves_a_record; :271 a_hot_swap_moves_the_whole_version_and_a_rollback_moves_it_back; console-web e2e door.spec.ts:42 rolls back the app | Console e2e uses a stand-in door, not ordo-host |
| I | A revoked version can never be activated, applied, pinned or given to new tenants; revoking the active one needs somewhere to go | howto-operating.md:~240 | crates/ordo-host/tests/versions_revoke.rs a_version_not_active_is_only_marked_and_then_refused_everywhere; the_active_version_is_revoked_only_with_somewhere_to_go; e2e windows.spec.ts:35 revoke and lift |  |
| I | A tenant on a revoked or too-old version moves only in its maintenance window or on demand, never by surprise; each tenant gets its own version's ops | howto-operating.md:250-345 | crates/ordo-host/tests/versions_moves.rs a_tenant_moves_in_its_window_and_not_before; versions_serving.rs two_tenants_on_two_versions_are_each_served_their_own |  |
| E* | Maintenance windows set, skip and refuse bad input | howto-operating.md:268-298 | clients/ordo-console-web/e2e/tests/windows.spec.ts:35; crates/ordo-host/tests/windows.rs bad_windows_are_refused_saying_what_to_change | Console e2e uses a stand-in door |
| I | The deployment's own program version only changes by the deployment owner: tenant owners, managers, editors and capability/delegated tokens are refused (no agent can deploy) | howto-operating.md:~70-95 | crates/ordo-host/tests/deploy_apply.rs:394,406,419,445 manager/editor/space-owner/capability_token refused |  |
| I | Every tenant's own database follows an apply, 8 at a time, and a tenant that was down is caught up later | howto-operating.md:299-312 | crates/ordo-host/tests/apply_everywhere.rs:114 an_apply_reaches_every_target_before_the_activation; :385 a_reachable_fleet_activates |  |
| I | The first program can be applied --direct, and direct use is recorded in the security log; with no --host and no --direct apply refuses naming both | quickstart.md:339-400; howto-operating.md:~30 | crates/ordo-cli/tests/wire_apply.rs:315 with_direct_the_command_talks_to_the_database_and_leaves_a_record; :870 apply_and_rollback_name_both_flags_when_given_neither; notes e2e stack.sh uses apply --direct | The notes e2e stack applies --direct (E for the happy path); the 'reaches the security log at next boot' part is I only |

### The CLI does what the docs tell a developer to run

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | ordo dev: one command brings up its own Postgres, serves, recompiles on every save with no restart, additive schema saves keep rows | quickstart.md:122-190; deploying.md:455-520 | crates/ordo-host/tests/dev_loop.rs:215 d2_owned_cluster_lifecycle_and_delta; :506 d3_hot_swap_serves_a_new_op_through_the_same_door; :171 real watcher | No e2e runs the real `ordo dev` binary end to end; the tests drive the library path |
| I | The token ordo dev prints really calls an op as the seeded mem_dev member; a call with no token is refused identically to an unknown op | quickstart.md:191-260 | crates/ordo-cli/src/dev.rs:1204 the_printed_token_calls_a_real_op_over_real_http | In-crate test; the no-token refusal text is not asserted here (door tests cover access_denied) |
| U | seed.sql and --fresh reseed the dev database | quickstart.md:200-225 | crates/ordo-host/tests/dev_loop.rs:329 (reset line says no seed.sql) | The seeded branch ('(seeded)') and --fresh are not asserted |
| I | A broken save prints a located error and the loop keeps serving the old program | deploying.md:1b | crates/ordo-host/tests/dev_loop.rs:140 d1_broken_save_prints_a_spanned_frame | Keeps-serving part not asserted |
| I | ordo gen ts writes both typed packages deterministically and writes nothing for a program that does not compile | quickstart.md:268-300 | crates/ordo-cli/tests/gen_ts.rs:24,55,206; clients/ordo-gen tests/ratchet.rs | The generated client is used by every example e2e page (indirect E) |
| I | ordo check exits 1 on a non-compiling op, 0 otherwise, and lists routines, nopause and notify marks | tutorial.md; howto-routines.md:~100 | crates/ordo-cli/tests/check_exit.rs:30,81; each_check.rs:66; nopause_check.rs; function_check.rs |  |
| I | ordo login / whoami / touch / machine key / logout, and a stale touch is retried once | howto-console.md:~60; first-deployment.md:196 | crates/ordo-cli/tests/login.rs:294,446,497 |  |
| I | Every door command (versions, windows, fleet, quota, plan, fn, secret) goes through the door and answers the same as the direct path | howto-operating.md:~30-60 | crates/ordo-cli/tests/wire_apply.rs:125 the_wire_and_the_database_print_the_same_report; :561; :706; wire_secret.rs; wire_fn_runs.rs |  |

### The console never stands in for the owner and never shows what it must not

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| E* | First run: the first invite link signs the first admin up once, with a passkey and recovery codes shown once | howto-console.md:17-37; first-deployment.md:143 | clients/ordo-console-web/e2e/tests/identity.spec.ts:48; crates/ordo-identity-service/tests/flows.rs:66 first_start_mints_one_invite_that_signs_the_first_admin_up_once | Compose boot from `docker compose up` and reading the log for the invite is not exercised; the e2e starts binaries via stack.sh |
| E* | A recovery code signs in once and only to make a new passkey; every lockout has a way back | howto-console.md:38-66 | clients/ordo-console-web/e2e/tests/console.spec.ts:103; flows.rs:452 |  |
| E* | The console can never satisfy a touch itself: platform writes ask the owner to touch at the deployment (tenant delete, quota, create) and one touch covers 5 minutes | howto-console.md:87-102 | clients/ordo-console-web/e2e/tests/identity.spec.ts:83-89 (touch counted); tenants.spec.ts:42; usage.spec.ts:50 | A stand-in door; the 5-minute window and 'a machine key is never asked' are not asserted in a browser (crates/ordo-console/tests/door_link.rs is I) |
| E* | A managed secret is set and no page or API answer ever shows its value | howto-console.md:125; deploying.md | clients/ordo-console-web/e2e/tests/runtime.spec.ts:132 |  |
| E* | A setting change brings up a new revision; a refused setting touches nothing running; a crashing revision rolls back; a killed deployment is restarted | howto-console.md:68-86,167-197 | runtime.spec.ts:80,92,108,119,159; crates/ordo-supervisor/tests/supervisor.rs:392,428,530 | Runs against a fake child process, not a real ordo-host |
| I | In door-only mode every runtime write is refused naming ORDO_CONSOLE_SUPERVISOR_SOCKET | howto-console.md:208-227 | crates/ordo-console/tests/door_only.rs | No browser check |
| E* | The console lists tenants, creates, pauses and resumes them, shows a held-closed tenant, and runs owner succession | howto-console.md:103-124 | tenants.spec.ts:14; people.spec.ts:16; shares.spec.ts:13; usage.spec.ts:12; settings.spec.ts:16 | Tenant delete, export and attach-bucket buttons have no e2e (crates/ordo-console/tests/tenants.rs is I) |
| I | A bad console setting refuses the boot and the message names the setting; the console refuses the deployment's own database | howto-console.md:125-166,318-330 | crates/ordo-console/tests/settings.rs; compose_hardening.rs |  |
| I | The one image runs three children as separate unix users with only the capabilities listed; a child crash never stops the other two | howto-console.md:167-197 | crates/ordo-supervisor/tests/supervisor.rs:291,392,428,455; .github/workflows/images.yml:349-413 docker smoke (users, caps) | The compose file itself (read-only root, tmpfs, named volumes) is not brought up by any test |
| I | Upgrading from the pre-identity-service image changes nothing for owners (carry-over, keep own sign-in) | howto-console.md:228-290 | crates/ordo-supervisor/tests/supervisor.rs:614 the_old_console_key_is_forgotten_before_the_console_starts; :764 with_no_identity_database_the_identity_service_is_not_started | No test with the old compose file |

### A paused tenant is held still, and nothing new happens inside it

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | Every op on a paused tenant is refused space_paused 423 (or the owner's chosen status), the data is kept, resume restores everything; a non-member learns nothing of the pause | howto-pause.md:11-90 | crates/ordo-host/tests/tenant_pause.rs:302,340,360,418 | Console e2e pauses and resumes (E, tenants.spec.ts:49) but no example-app e2e shows a user hitting a paused tenant |
| I | Only the deployment owner pauses by default; a tenant pauses itself only if a role is granted; a tenant cannot lift the deployment's pause | howto-pause.md:25-60 | tenant_pause.rs:545,556,585,597 |  |
| I | nopause ops keep answering while paused; derived std ops never do | howto-pause.md:110-135 | tenant_pause.rs:405 | Derived-std-op refusal not separately asserted |
| I | People can look, leave and take access away while paused; nothing new is admitted, shared or uploaded | howto-pause.md:136-165 | tenant_pause.rs:427,497 | Blob door read and upload refused while paused: no test found in tenant_pause.rs or blob_door tests (N for that part) |
| I | Routines and live feeds hold during a pause: clock ticks skipped and recorded, change listeners keep position, nopause routines still run, live feed closes with space_paused | howto-pause.md:166-200 | crates/ordo-host/tests/routines.rs:371,1117,1195; tenant_pause.rs:851 live_closes_with_space_paused_and_reconnects_on_resume |  |
| I | The SDK holds every synced op on a pause (view.paused, no stop, device keeps rows) and reloads first on resumed() | howto-pause.md:201-250 | clients/ordo-sdk/test/manager.test.ts:1375 | SDK against a scripted door, not a real paused server in a browser |
| I | Records keep being secured during a pause (checkpoint, attest, prune, reseal); a person's own tenant stays theirs | howto-pause.md:180,250 | tenant_pause.rs:915 checkpoint_attest_prune_and_reseal_still_run; :728 |  |
| I | A pause reaches other replicas within the refresh interval | howto-pause.md:215 | tenant_pause.rs:779 |  |

### Routines run once, as themselves, and cannot reach beyond what they changed

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A scheduled routine fires once per period for the whole deployment however many servers run, and a change routine runs once per change | howto-routines.md:~40-70 | crates/ordo-host/tests/routines.rs:261,324; notes_app.rs:1737 two servers |  |
| I | A routine's writes land as one command group attributed routine:name@version and are revertible as a whole | howto-routines.md:~20,115 | crates/ordo-conformance/tests/routine_each.rs:243; routines.rs:523 |  |
| I | A change-woken routine can never read or write rows beyond the changed rows and one declared ref away unless unguarded; a routine is not an anonymous caller | howto-routines.md:~120-140 | crates/ordo-exec/tests/routine_provenance.rs:252,305,331,221; routine_each.rs:349 |  |
| U | A bad routine never compiles (duplicate name, $caller, unknown field, bad parameter, UTC time without Z, unit mo/y/ms, unknown event kind) and the refusal names its class | howto-routines.md:~45-80,230 | crates/ordo-syntax/src/declparse.rs:5203-5246; crates/ordo-conformance/tests/routine_each.rs:338,349; ordo-cli/tests/each_check.rs:86 | Parser unit tests for the Z rule and units; unknown_event_kind and $caller refusal have no integration test found |
| I | A run handles at most ORDO_ROUTINE_EACH_MAX_ROWS rows and the next run finishes the rest | howto-routines.md:~95 | routine_each.rs:197 |  |
| I | A routine added, retimed or removed by a hot deploy takes effect with no restart; a tenant runs the routine of its own pinned version; a renamed trigger hands over its cursor and never replays history | howto-routines.md:~210-250 | routines.rs:656,725,871,1005; crates/ordo-cli/tests/triggers.rs:153 deregister |  |
| I | A failing routine retries with backoff, is dead-lettered after 5, and never takes the server down | howto-routines.md:~200 | routines.rs:459; crates/ordo-exec/tests/triggers.rs:311 |  |
| E | notify emails only the people holding exactly the level, once, with nothing from the row, only after the commit | howto-routines.md:~140-180 | examples/notes/e2e/tests/upgrade/v1-to-v2.spec.ts:252 a reminder fires once: a notice and one email; crates/ordo-host/tests/notes_app.rs:1737 | Rollback-sends-nothing, 'no email sign-in means no mail' and the refusal list (empty text, no acl) are not asserted |
| I | An idle tenant is visited less often and a change restores the pace | howto-routines.md:~255 | routines.rs:815 |  |

### A function acts as the person, never as more than the person, and never holds a secret

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A function can never do more than the person calling: ops it reaches are in uses intersected with what the person may do; a stranger and a guarded row get nothing | howto-functions.md:535-590 | crates/ordo-host/tests/functions.rs:1574 a_person_who_lacks_access_gets_no_further_through_a_function; :1677; :366 |  |
| I | A function cannot delete unless the app developer opened that op, and the refusal names ORDO_DELEGABLE_OPS; platform ops and function-to-function calls are impossible | howto-functions.md:560-570,615 | functions.rs:1677 an_op_not_in_uses_and_a_closed_delete_are_refused_and_name_the_fix; ordo-fn/tests/ops.rs:147 |  |
| I | The chain names both: agent:function:name@version on behalf of user:person | howto-functions.md:555 | functions.rs:1518 |  |
| I | A secret value is never visible to code, logs, answers or run records; it is sent only to its bound hosts, refused for any other host before any lookup | howto-functions.md:240-400 | functions.rs:701,971,1030; crates/ordo-fn/tests/secret_hmac.rs:148,218; ordo-egress/tests/function_fetch.rs:198,254 |  |
| I | An outbound call is refused for a host off the egress list or inside the deployment; redirects are never followed; bodies and time are capped | howto-functions.md:366-396 | ordo-fn/tests/run.rs:125; ordo-egress/tests/function_fetch.rs:238,266,319,336 |  |
| I | A function is bounded: CPU, wall time, memory and concurrent calls stop it with function_timeout, function_oom or function_busy 429; no state survives a call | howto-functions.md:134-152 | ordo-fn/tests/run.rs:207,245,279,297,186; ordo-host/tests/functions.rs:522 |  |
| I | A function reads only the files named in its own call, as the caller; a file the caller may not read is not_found; the per-call cap names its setting | howto-functions.md:163-240 | functions.rs:1234,1287,1353; ordo-fn/tests/blob.rs:200,229,246 |  |
| I | A new program version gives new code to every tenant at once; rollback brings old code back; a removed function answers access_denied | howto-functions.md:431-455 | crates/ordo-cli/tests/wire_apply.rs:387,424 | Removed-function and rollback-restores-code parts not found |
| I | A function called with an Idempotency-Key does not run twice; a different body with the same key is idempotency_mismatch | howto-functions.md:~500 | functions.rs:462 |  |
| I | The owner lists runs and reads a log; the log never holds input or answer; runs older than retention are swept | howto-functions.md:629-670 | functions.rs:798,917 |  |
| E | A page calls a function and sees its refusal naming the missing secret setting | howto-functions.md:455-520 | examples/notes/e2e/tests/v2/search-and-links.spec.ts:133 Suggest title with no llmKey | Only the refusal path in a browser; a successful function call needs a model API, only I (functions.rs:309) |

### A worker acts only for the person who consented, and stops when they revoke

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A worker sees exactly the rows its person sees, writes with their answers, and calls only ops in both lists; without a live delegation it is only itself | howto-workers.md:17-35 | crates/ordo-host/tests/delegation.rs; delegation_consent.rs |  |
| I | A worker can never delete (unless opened), write platform ops, carry a fresh touch, or name somebody else | howto-workers.md:36-56 | crates/ordo-host/tests/delegation.rs; functions.rs:1677 | Confirmed by name only for function side; delegation.rs not read line by line |
| I | Revoking the delegation stops the worker's next call with delegation_required on every surface; replays and cursors are never shared between workers | howto-workers.md:70-100 | crates/ordo-host/tests/delegation.rs | As above |

### Audit, tamper and restore are detectable

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A tampered, deleted or truncated command chain is found and the first bad index named; a clean chain exits 0 and a broken one 3 | howto-operating.md:1213-1317 | crates/ordo-cli/tests/operator.rs:313,371,495,2847 |  |
| I | A database restore nobody recorded is never served silently: the space is quarantined, other tenants keep serving, and recording the restore lifts it with no restart | howto-operating.md:1084-1213 | crates/ordo-host/tests/restore_drill.rs:413; ordo-cli/tests/operator.rs:1629,3159; ordo-host/tests/schema_behind.rs | Console shows the held-closed tenant in e2e (tenants.spec.ts:36) from a stand-in |
| I | The master key can never travel inside a backup: boot and keys init refuse when the database holds it | howto-operating.md:1060 | crates/ordo-host/tests/runtime.rs:1091 a_master_stored_in_the_database_refuses_the_boot |  |
| I | A prune never destroys history past a signed checkpoint, a listener or the slowest consumer and --dry-run shows the floor | deploying.md:1592-1620 | operator.rs:3039,915,1141 |  |
| I | The PITR plus per-space bundle both exist and a bundle verifies standalone | howto-operating.md:1028-1084 | crates/ordo-exec/tests/export.rs; restore_drill.rs:413 | Doc advice (take both backups) is not testable; the drill covers the restore |

### Operating a live deployment shows the truth and tells outsiders nothing

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | /health/live never consults a dependency; /health/ready is ready with no program, not ready (503) when the database or a credential is missing, and names the check not the host | howto-operating.md:346-412 | crates/ordo-host/tests/health.rs:129,192,229,258 |  |
| I | An anonymous health caller learns status only, nothing naming a node or a version | howto-operating.md:~395; deploying.md:1440 | health.rs:276 anonymous_health_answers_status_only |  |
| I | One tenant cannot hold the whole connection pool; a full pool answers 503 server_busy with Retry-After and nothing ran; a runaway statement answers statement_timeout 503 | deploying.md:521-606; pooling-explained.html | crates/ordo-exec/tests/noisy_neighbour.rs:72,94; ordo-host/tests/noisy_neighbour.rs:128; store_busy.rs | Metric naming the consuming tenant: ordo-host noisy_neighbour.rs:23 |
| I | A page cursor is bound to space, op and principal: another caller's or space's cursor is invalid_cursor, a stale one version_skew | deploying.md:652-672 | crates/ordo-host/tests/refine_paging.rs; api tests | Names not read; paging is covered more fully in another area |
| I | Usage is counted by class and rows, two replicas sum, a crashed period is marked open, and the owner reads it; Ordo meters and never invoices | howto-operating.md:431-561; deploying.md:1681-1775 | crates/ordo-host/tests/usage.rs; ordo-cli/tests/operator.rs:3460,3511; console e2e usage.spec.ts:12 |  |
| I | A quota over limit plus grace refuses quota_exceeded 429, within grace warns and serves, and a non-member of an over-quota space still gets access_denied | howto-operating.md:466-520 | crates/ordo-host/tests/quotas.rs over_quota_plus_grace_is_refused_at_the_door_and_a_non_member_still_gets_access_denied; within_grace_warns_and_serves | Console edits a quota in e2e (usage.spec.ts:44) |
| I | The deployment owner's reads of usage, quota and versions answer nobody else | howto-operating.md:466-560 | deployment_quotas.rs quotas_answer_only_the_deployment_owner; usage.rs the_usage_route_answers_the_operator_and_refuses_a_tenant_owner |  |
| I | The deploy is attributed to the signed-in principal; --as is refused with --host | howto-operating.md:~55 | deploy_apply.rs:459; wire_apply.rs:265 |  |
| U | Outbound delivery from the deployment never reaches loopback, link-local, private or metadata addresses; a malformed ORDO_EGRESS_DENY_CIDRS refuses the boot | deploying.md:1622-1680 | crates/ordo-egress/tests/fetch.rs; function_fetch.rs:336 loopback_is_denied | Built-in categories tested; no test found that the CIDR setting itself parses, refuses a host-bit CIDR or denies a configured range |
| I | The function engine cache loads from disk on restart | howto-functions.md:153-162 | crates/ordo-fn/tests/engine_cache.rs:82 |  |

### First deployment from nothing

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| N | make-secrets plus compose bring up console, identity and deployment; the log shows the invite; Deploys applies notes v2 and lists it active | first-deployment.md:17-220 |  | No test starts docker/compose.one.yml. Pieces are tested (supervisor.rs, identity flows, console e2e door apply) separately. An e2e would run make-secrets.sh then compose up and click through |
| I | scripts/make-secrets.sh makes the secret files with the right owners and modes | first-deployment.md:38-62 | scripts/make-secrets-test.sh | Shell test; whether CI runs it not checked |
| N | A database on the wrong host or port refuses with a message naming host:port and what to check | first-deployment.md:133-142 |  | No test found for the 'could not reach ORDO_PG_HOST:ORDO_PG_PORT' message in the supervisor/console path |
| I | sealed master key: opens with its passphrase and nothing else; sealing over an existing file is refused | quickstart.md:346-360 | crates/ordo-crypto/tests/master.rs:24,89 |  |
| I | A system space is created at boot and a deployment with no program serves /health/live | quickstart.md:368-380 | health.rs:129 |  |

**Promised but marked unbuilt, not graded:**

- Other notify channels (push, SMS), template, link in mail, CDK spelling (howto-routines.md:Not built)
- Top-level `when ... -> run x` line, wildcard entity in a change trigger, $trigger in a body, routine rate caps (howto-routines.md)
- Function schedules, change triggers and webhooks (howto-functions.md:13)
- A tenant reading its own function runs (howto-functions.md:665)
- Rate and volume budgets per delegation, worker acting for a worker, one delegation across two spaces (howto-workers.md)
- Schema evolution increments 3-5: backfills, drops, blind-index backfill (quickstart.md thin section, howto-operating.md)
- Write quotas, only requests and rows_returned gated (howto-operating.md:~490)
- OpenTelemetry traces, structured logs, metrics exporter (howto-operating.md:412-430)
- Fair queue across k tenants in the pool (deploying.md:2a)
- Rollouts that move tenants in batches (howto-operating.md:Versions and pins)
- Separate deployment-only and console-only images (owner ruling, howto-console.md)
- seed.sql placeholder rendering for sealed indexed fields (deploying.md:1b)
- Local tenant provisioning inside ordo dev (deploying.md:1b)

## Clients, live data, offline, public path, federation


### Concept guarantees (public path)

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A visitor with no token reads public data, and never a private field or another tenant's row, through any public op | howto-public.md:13-16,94-107 | crates/ordo-host/tests/public_path.rs:238 the_anonymous_door (asserts no 'margin', no 'tenant-only') | Native entity tested at I. E only through note_viewer's external-backed publicNotes (examples/note-viewer/e2e/tests/api/grants.spec.ts:41) and the viewer page (browser/federation.spec.ts:25). No e2e on a native public entity; a page-level test would load the marketing-site or shopfront page signed out and assert the private field and a tenant row are absent. |
| I | Tenant data can never be called from a public page: a tokenless call to a non-public op or to a tenant's space gets the same byte-identical access_denied | howto-public.md:15,108 | crates/ordo-host/tests/public_path.rs:238 the_anonymous_door (unknown vs private op bytes identical; other space refused) | Strong negative at I. No e2e: a Playwright page with no session should call a private op and a tenant space and see 403. |
| U | A signed-in member calling a public op takes the ordinary path in their own space (no public cache, tenant rows not mixed with deployment rows) | howto-public.md:109 | crates/ordo-host/tests/public_path.rs:238 the_anonymous_door (member status 200, no x-ordo-cache header) | Weakest part: the test checks status and no cache header, not that the member sees the tenant's rows and not the deployment's public rows (or the reverse). |
| I | The public call cache is really used: a repeated call is a hit and reads no database | howto-public.md:119,251-262 | crates/ordo-host/tests/public_path.rs:365 the_cache_and_its_invalidation (x-ordo-cache miss then hit); :473 public_misses_past_the_cap (5 hits do not count as misses) | Observes hit/miss header. Does not observe the database was not read (the miss-cap test indirectly shows hits are not counted). Not E: a page test would load the public page twice and read X-Ordo-Cache and ordo_public_requests_total. |
| I | A write to an entity a public op reads drops the cached answer at once, so the next visitor sees the new row | howto-public.md:256-258,401-404 | crates/ordo-host/tests/public_path.rs:365 the_cache_and_its_invalidation; crates/ordo-host/tests/public_writes.rs:611 a_public_write_drops_the_public_answers_that_read_its_entity | Unrelated-entity write keeps the hit; deploy and lost listener drop all (asserted at :396-418). |
| I | A visitor can never see a stale answer served as cacheable while the database listener is lost | howto-public.md:262 | crates/ordo-host/tests/public_path.rs:365 (a_lost_listener_drops_everything_on_reconnect, asserts s-maxage=0) |  |
| I | CDN headers: Cache-Control, ETag with 304, Cache-Tag and Surrogate-Key name the op and entities | howto-public.md:265-281 | crates/ordo-host/tests/public_path.rs:438 the_cdn_headers |  |
| U | Dropped answers are purged at the CDN by tag (ORDO_PUBLIC_CDN_PURGE_URL), through egress, with a private endpoint refused naming the setting | howto-public.md:279-281 | crates/ordo-host/src/public.rs:553 a_purge_to_a_private_address_names_the_private_hosts_list; :568 purge timeout | Only purge_body, refusal text and settings parse tested. No test where a drop makes a real POST reach a stand-in CDN. An I test needs a local https stand-in and a write that asserts {tags:[...]} arrives. |
| I | A public read past the per-minute miss cap is refused 429 quota_exceeded naming the setting; hits still served | howto-public.md:120-127 | crates/ordo-host/tests/public_path.rs:473 public_misses_past_the_cap_are_refused_by_name |  |
| U | Public reads reject $refine (invalid_refinement) and tokenless ?live=1 (not_live) | howto-public.md:102-107 | crates/ordo-host/tests/public_path.rs:238 (invalid_refinement asserted); SDK clients/ordo-sdk/test/public.test.ts:57 refuses locally | Weakest part: tokenless ?live=1 answering not_live on a public op has no test (the not_live tests at crates/ordo-api/tests/api.rs:2229 are for a non-live-capable private op). |
| I | A file on a public row is served to a visitor, and a private row's file is not | howto-public.md:34,100 | crates/ordo-host/tests/blob_door.rs:3430 a_public_rows_file_is_served_and_a_private_rows_is_not |  |
| I | A public op can only be written over public fields: private, sealed, guarded, private-blob and private-composing reads are refused at compile time with the class | howto-public.md:39-56 | crates/ordo-plan/tests/public_ops.rs:39-272 (public_sealed, public_reads_private, public_composes_private, public_caller, public_guarded, public_private); crates/ordo-conformance/tests/public_catalogue.rs:49 | Real compile. public_write_ref at :237. op_named_like_entity: crates/ordo-host/tests/public_name_collision.rs:187; duplicate_op_name: crates/ordo-plan/tests/duplicate_op_names.rs. No 'ordo check' CLI test for the '(public: anyone may call it)' line. |
| I | public* cascades to owned children only; plain public and plain refs do not carry it | howto-public.md:30-32 | crates/ordo-plan/tests/public_ops.rs:107 public_star_cascades_to_owned_children; :135; :145 |  |

### Public writes

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A public write creates a row as $anonymous and can reach nothing but what its op names (no update, delete, other entity, other space) | howto-public.md:137-165 | crates/ordo-host/tests/public_writes.rs:309,354 a_public_write_cannot_reach_outside_what_its_op_names | Also real socket in marketing_site_app.rs:579 the_contact_form_writes_an_enquiry_that_no_visitor_reads_back. Not E: a form submit on the marketing-site page is not in Playwright. |
| I | Compiler refuses public writes that update/delete, write guarded entities, use $caller, or aim a ref at a private row | howto-public.md:49-51 | crates/ordo-plan/tests/public_ops.rs:195-256 |  |
| I | A keyed public create answers identically for a new and an existing email, and changes nothing for a duplicate | howto-public.md:176-208 | crates/ordo-host/tests/public_writes.rs:835 a_keyed_public_create_answers_alike_for_a_new_and_an_existing_email |  |
| I | A public write needs a human-check token: on by default, refused 403 human_check_failed naming the settings, one use per token, fails closed | howto-public.md:210-243 | crates/ordo-host/tests/public_writes.rs:491 the_human_check_is_on_by_default_and_names_its_settings; :899 a_flood_of_bad_tokens_leaves_the_hourly_cap; marketing_site_app.rs:623 | Provider stand-in used. Real Turnstile never tried (expected). |
| I | Public writes past the hourly cap are refused 429 naming the setting | howto-public.md:245-249 | crates/ordo-host/tests/public_writes.rs:546 |  |
| I | Per-tenant CORS: off until the deployment owner allows it, owner-only, exact origins, union on preflight, public answers carry * | howto-public.md:317-346 | crates/ordo-host/tests/public_writes.rs:715 a_tenant_lists_its_own_origins_when_the_deployment_allows_it; crates/ordo-api/tests/cors_layers.rs | Public answers carrying ACAO * on success and refusal not seen asserted; check cors_layers.rs. |
| I | Proxy mode slows a busy tokenless visitor and never refuses; off until both settings are set; address never logged | howto-public.md:348-375 | crates/ordo-host/tests/public_writes.rs:652; marketing_site_app.rs:645,689 (real socket) | Boot refusal for one of the two proxy variables not found by me. |
| N | Public requests are counted in ordo_public_requests_total by op, kind, outcome; a probe for an unknown op cannot mint series | howto-public.md:377-387 |  | Search for the counter name in tests found nothing. A test would scrape /metrics after a hit, miss, limit and unknown-op probe. |
| I | Public read settings out of range refuse the boot naming variable, range and default | howto-public.md:285-287 | crates/ordo-host/src/public.rs:569 (unit) and crates/ordo-host/tests/deployment_settings.rs | Boot path checked at unit level for the timeout; confirm others. |

### Clients: live data

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| E | Live reads update when data changes: a write in one tab or by another person appears on a screen with no reload | react.md:37-40; ui-gen.md:113-121 | examples/notes/e2e/tests/ui/notes.spec.ts:54 a note written in another tab of a new person shows in the first tab with no reload | Live in notes: <OrdoProvider live>. Server side: crates/ordo-api/tests/api.rs:2200 (feed gives invalidate on bus signal), crates/ordo-host/tests/invalidation_routing.rs. Only a tab of the same person; a second person seeing a shared note change live is not an e2e. |
| E | A feed that drops reconnects and catches up by itself when the network returns, without waiting for a retry timer | ui-gen.md:113-116 | examples/notes/e2e/tests/ui/notes.spec.ts:106 after the network returns, the page catches up by itself |  |
| U | Live is opt-in; a burst of invalidations is one refetch, at least 250 ms apart; one stream per op and params | react.md:37-39; ui-gen.md:117-121 | clients/ordo-react/test/live.test.ts:182,206,221,585 | Mock door. Fine at this level; no e2e of a burst. |
| U | A live stream never leaks another person's rows: a revoked or refused feed empties the hook and stops | ui-gen.md:130-132 | clients/ordo-react/test/live.test.ts:519 a revoking refusal empties the hook; clients/ordo-react/test/device-gaps.test.ts:134 | Mock door only. Server side feed guard: crates/ordo-api/tests/feed_door_guard.rs (I). No e2e where a share is revoked and the reader's open screen empties; notes passkey.spec.ts:59 covers disappearance after the share is taken back (E, probably after reload; read not verified). |
| I | A live op over an external table is refused not_live (the other system's writes reach no Ordo log) | definition-language.md:2105-2107 | crates/ordo-host/tests/external_entities.rs:267 a_read_of_an_external_is_never_live |  |

### Clients: offline and device copy

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| E | Notes stay on screen with the network off, and a write is refused (not queued) until it returns | ui-gen.md:123-129; react.md | examples/notes/e2e/tests/ui/notes.spec.ts:322; examples/notes/e2e/tests/browser/passkey.spec.ts:103 | Client-level: clients/ordo-react/test/device.test.ts:87,417. |
| U | The device copy answers only syncable reads; aggregates, grouped reads, std.space.* and writes go to the network and say why | ui-gen.md:123-129 | clients/ordo-react/test/device.test.ts:292,305,337,347 | Mock; the sync spec comes from a golden descriptor, not a real door. |
| U | The device copy is kept current from the server feed and survives a restart | ui-gen.md:123-125 | clients/ordo-react/test/device.test.ts:114,139; clients/ordo-sdk/test/sync.test.ts, manager.test.ts, replica.test.ts | Restart across a real browser reload with the net off not found as an e2e; the offline e2e is after a normal load. |
| U | Signing out or switching person erases what the device held, so the next person never sees the first person's rows | (notes app purge; device-gaps; device.test.ts:765) | clients/ordo-react/test/device.test.ts:765 a new sign-in drops every row in memory; examples/notes/web/src/App.tsx:66 purge | Not documented in the named docs as a promise beyond device.test. No e2e: sign in as A offline-cache, sign out, sign in as B, assert none of A's notes in the page or IndexedDB. |

### Clients: writes, conflicts, retries, refusals

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| E | A write on a stale row is refused cas_miss, nothing is lost, and the person keeps their text or takes the stored one | react.md:96-118; client.md:34-37; ui-gen.md:56-60 | examples/notes/e2e/tests/ui/notes.spec.ts:137 and :177 (two tabs edit the same note) | Hook-level: clients/ordo-react/test/cas.test.ts, commit-gaps.test.ts. |
| U | A CAS op sent without rev is refused before the wire (missing_param); no last-write-wins path | react.md:117-118; client.md:53-54 | clients/ordo-react/test/cas.test.ts:16 a CAS op committed without a rev is refused before the wire | Client-side refusal; the door also refuses (host tests). |
| U | server_busy, serialization_conflict and tenant_not_ready are retried by the client with the same idempotency key, bounded, and surface when exhausted | client.md:65-66,128 | clients/ordo-react/test/busy.test.ts:18-158; conflict.test.ts:32-71; clients/ordo-sdk/test/busy.test.ts, conflict.test.ts | Mock door. Door-side idempotent replay is tested in crates (replay_durable.rs, I). No SDK-against-real-door test of a retried write landing once. |
| U | Every failure is a typed OrdoRefusal with class, message and param; a call with no answer is OrdoNetworkError (timeout, unreachable, blocked) | client.md:111-131; sdk-signatures.md:193-206 | clients/ordo-sdk/test/door.test.ts, network.test.ts, deadline.test.ts; e2e probe examples/notes/e2e/tests/ui/notes.spec.ts:65 | Weakest part: the e2e only proves the browser facts the classification rests on, not the SDK's own kind. |
| U | Refusal classes map to page behaviour (access_denied, not_found, access_guard, check/bound/invalid/missing param, duplicate) | client.md:120-131 | clients/ordo-sdk/test/door.test.ts; crates/ordo-api/tests/client_classes.rs (I, server side) | The wire shape is I on the server; client mapping U. |
| U | A listing ends only when `next` is absent: short or empty pages do not end it; invalid_cursor and version_skew restart | ui-gen.md:91-97; react.md:68-69 | clients/ordo-react/test/pages.test.ts:45,62,153,173 | Server side paging: crates/ordo-host/tests/refine_paging.rs (I). No example app pages past one page in e2e. |
| I | refine and pick narrow rows at the door, agree between device and server, and a sealed field refines by equality only | react.md:66-67; sdk-signatures.md:186 | crates/ordo-host/tests/refine_door.rs; crates/ordo-conformance/tests/refine_parity.rs; clients/ordo-sdk/test/refine-parity.test.ts; clients/ordo-react/test/refine-agreement.test.ts | No example app uses refine/pick in e2e (grep found no use in example sources by me; check not complete). |
| U | Forms derived from op params validate in wire types (money string, bounds in characters, id prefix, enum) and send the door's own classes | react.md:120-171 | clients/ordo-react/test/form.test.ts:198-299 | No e2e: the notes app hand-writes its forms; no example uses useOpForm that I verified. |
| U | Generated typed functions per op: branded ids, ReadCall/WriteCall, optional param null clears | client.md:6-66 | clients/ordo-sdk/test/generated.test.ts, refine-types.test.ts | Notes web uses them in e2e indirectly (E for the common read/write path), not the null-clear path. |
| I | Backend functions called from a client: result, or function_error/timeout/oom/busy/egress_refused/space_paused refusals; files by handle | client.md:68-91 | examples/notes/e2e/tests/v2/search-and-links.spec.ts:133 (Suggest title with no llmKey shows the refusal naming the setting; E for the refusal path); crates/ordo-host/tests/functions.rs | Success result, timeout, oom, 1 MiB body bound_violation: host tests only; check each class has a case. |
| E | Platform ops by name (invite, setRole, members, share, remove, leave...) work through the hooks; a missing reauth is asked once and retried | react.md:173-206 | examples/notes/e2e/tests/ui/notes.spec.ts:221 (invite, accept, read, lose on delete); clients/ordo-react/test/platform.test.ts:105-142 (reauth retry, mock) | Reauth retry (stepUp) is U only; no e2e of the 'Confirm it's you' touch on a platform write. |
| E | A new person's tenant is set up with a status the page shows (setting up, stuck with reason, failed with Try again, ready) | react.md:214; sdk-signatures.md:140-149 | every notes e2e join (examples/notes/e2e/tests/people.ts); clients/ordo-react/test/tenant.test.ts:100-193 for stuck/failed | Stuck and failed phases U only. |
| E | Files: upload to a blob field, view and download by handle, a person's own data as a zip | react.md:216-217 | examples/notes/e2e/tests/upgrade/v1-to-v2.spec.ts:88 (attach and download); crates/ordo-host/tests/own_download.rs | Own-data zip not in e2e. |
| I | Generated UI surfaces are catalog-scoped: a token receives descriptors only for ops it may call (module is not access) | ui-gen.md:62-68 | crates/ordo-host/tests/interface_surface.rs | Not verified in depth; no e2e. |

### Federation: external tables

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| I | A foreign table in the same database reads as an entity, joined in Ordo's SQL, in key order, with the foreign index serving paging | definition-language.md:2011-2015,2076-2090 | crates/ordo-host/tests/external_entities.rs:90 std_list_and_get_read_the_foreign_table_in_key_order; :199 a_nineteen_digit_key_is_found_everywhere; :237 a_developer_op_joins; crates/ordo-plan/tests/external.rs:145 |  |
| I | An external can never be written by Ordo; write ops and routine writes are refused read_only | definition-language.md:2088,2178 | crates/ordo-plan/tests/external.rs:189; crates/ordo-schema/tests/external.rs:235 |  |
| I | Ordo never changes the other system's schema: apply, reset, erase, export and removal plan no DDL for an external | definition-language.md:2094-2095 | crates/ordo-host/tests/external_entities.rs:283 the_apply_leaves_the_foreign_schema_as_the_other_system_made_it; crates/ordo-schema/tests/external.rs:138,165 |  |
| I | A ref to a vanished external row reads as missing, never a cast error; a foreign id of another prefix matches nothing | definition-language.md:2078-2091 | crates/ordo-plan/tests/external.rs:76,120 (SQL shape); crates/ordo-host/tests/external_entities.rs:149 (ref through the write path) | The raw-SQL level is U; the door-level 'no cast error' for a malformed id is not clearly asserted. |
| I | The external's read guard applies; shared without access read is flagged at plan | definition-language.md:2040-2044,2064 | crates/ordo-plan/tests/external.rs:171; crates/ordo-cli/tests/external_notes.rs:25 |  |
| I | A public external serves only its public fields to a visitor; a private external is refused; the origin cache holds at most ORDO_PUBLIC_CDN_S_MAXAGE | definition-language.md:2045-2054 | crates/ordo-host/tests/external_entities.rs:466 a_visitor_is_served_a_public_external; :494; crates/ordo-api/..; E via examples/note-viewer/e2e/tests/api/grants.spec.ts:41 | E for the guarded rows only (tagged public). |
| I | ordo introspect --external prints a reviewable block (types mapped, unmappable columns as comments) and never writes to the database | definition-language.md:2114-2138 | crates/ordo-cli/tests/external_introspect.rs:60-357 |  |
| I | Plan and apply compare each external with the live table and refuse external_missing, external_no_grant, external_changed; added columns are noted | definition-language.md:2139-2153 | crates/ordo-host/tests/external_drift.rs:102-262; crates/ordo-cli/tests/external_present.rs:84 | Tenant's own database apply not separately tested here. |
| I | The watcher holds an external's ops with external_changed (HTTP 500, naming table, column, fix), lets other ops and ref-only writes through, and lifts when the table returns | definition-language.md:2161-2170 | crates/ordo-host/tests/external_drift.rs:323 the_watcher_holds_an_entity_and_lifts_it_when_the_table_is_back | watch_once is called by the test, not the timer ORDO_EXTERNAL_WATCH_SECS. |
| I | ordo plan lists tables present but not Ordo's, capped at 20, changing nothing | definition-language.md:2155-2159 | crates/ordo-cli/tests/external_present.rs:48 |  |
| U | Compile refusals for externals name the fix (not_shared, composite_key, owned, acl, sealed, inherited_guard, ref_to_native, live, unsupported) | definition-language.md:2172-2189 | crates/ordo-syntax/tests/external.rs:92,101,109,222-245; crates/ordo-schema/tests/external.rs:196,210,235 | Parse/schema level only. external_acl, external_sealed, external_inherited_guard have no test I found by class name (the 'every write-shaped clause' loop at ordo-syntax/tests/external.rs:109 may cover acl/sealed; not read in full). |
| U | The schema of an external can be swapped per environment by ORDO_EXTERNAL_<SCHEMA>_SCHEMA; a bad value refuses the boot naming it | definition-language.md:2067-2074 | crates/ordo-host/tests/peer_entities.rs:566 a_plain_external_may_swap_its_schema | Checks the resolved name; no test that a swapped schema is the one a door read goes to. |

### Federation: peers and readers

| Level | Guarantee | Promised in | Strongest test | Weakest part, or what an example test needs |
|---|---|---|---|---|
| E | A person signed in to two deployments reads their own rows in the peer, and only theirs ($caller on a peer is the peer's member for the same person) | definition-language.md:2224-2250 | examples/note-viewer/e2e/tests/browser/federation.spec.ts:25,64 (private note seen by owner, never by another person); crates/ordo-host/tests/peer_entities.rs:278 | Real passkey, real identity service, two deployments. |
| E | A person with no member in the peer, or with sign-ins reaching two peer accounts, reads no guarded rows and gets no error | definition-language.md:2244-2246,2267-2269 | examples/note-viewer/e2e/tests/browser/federation.spec.ts:87 (Cy holds nothing); crates/ordo-host/tests/peer_entities.rs:406 a_person_who_reaches_two_accounts_reads_nothing | Two-account case is I only. |
| I | People match by provider fingerprint, not connection name; same name at another issuer matches nothing; changeable subject claims (email, login...) never link | definition-language.md:2227-2265 | crates/ordo-host/tests/peer_entities.rs:345,360,489 | SAML/password/passkey/tenant-provider never-match cases and the 60 s map re-read have no test I found. |
| I | Start refuses when the peer has no connection table, no shared provider with $caller used, or a connection name means two providers; peer setting unset or bad | definition-language.md:2215-2218,2271-2281 | crates/ordo-host/tests/peer_entities.rs:444,471,530,607; crates/ordo-cli/tests/peer_apply.rs:53 | peer_not_ordo and peer_caller_compare classes: no test found by name. |
| E | A reader sees only the tables its reader line names, by Postgres grant; the sign-in columns granted by column; the peer cannot read the reader back | definition-language.md:2289-2327 | examples/note-viewer/e2e/tests/api/grants.spec.ts:20,25,59; crates/ordo-host/tests/reader_grants.rs:172 | Real roles in e2e. |
| I | Removing a table or the whole reader line revokes the grant; a table a migration creates is granted by that apply | definition-language.md:2324-2327 | crates/ordo-host/tests/reader_grants.rs:271,313 |  |
| I | A deployment refuses to start without its grant (quoting the reader line to add) or on a shared/owner role unless ORDO_PEER_SHARED_ROLE=on | definition-language.md:2329-2353 | crates/ordo-host/tests/reader_grants.rs:229,353,394 |  |
| E | Each side of two deployments sharing one sign-in joins at once, one passkey serves both, and a reload stays signed in | definition-language.md:2224-2250 | examples/note-viewer/e2e/tests/browser/federation.spec.ts:25 |  |
| N | Tenant in its own database does not reach a peer (documented gap) | definition-language.md:2286-2287 |  | Documented limitation; no refusal test found. |

**Promised but marked unbuilt, not graded:**

- Page-level snapshot coordinator (ui-gen.md:133-137)
- Default skins OrdoTable/Detail/Form/Metric (ui-gen.md:150)
- ordo eject surface (ui-gen.md:162-175)
- App manifest compile checks, deref linking (ui-gen.md:177-340)
- Static renderers @ordo/render (ui-gen.md:156-160)
- filled param lexing residual (ui-gen.md:303-308)
- reserved annotations label/desc/order/choices/danger (ui-gen.md:389-400)
- Site labels; separate read layer; single-flight misses; shared live public streams; API keys; derived public index (howto-public.md:406-420)
- deployment entity line / public_tenant_entity; row condition on public; public.cache.min_fresh_ms; ordo public drop; public uploads; public write into a tenant's space; counters ordo_public_cache_entries and _bytes (howto-public.md:410-420)
- Sign-up per-address and share-link miss limits are ignored (howto-public.md:374)
- Offline write queue (refused, by design, ui-gen.md:129)
