-- The seed dataset of docs/composition-conformance.md §2.2.
--
-- Ids are the doc's abbreviated forms (`vend_1`) rather than the typed
-- UUIDv7 the spec calls for. The corpus relies on "id order = suffix order"
-- for its tiebreaks, and for single-digit suffixes lexical text order gives
-- exactly that. A tenth row in any table would break the correspondence;
-- real UUIDv7 minting is F-track work and is not in this slice.
--
-- SEALED FIELDS ARE NOT SEEDED WITH THEIR §2.2 VALUES. `customers.email`
-- and `orders.memo` are sealed, nothing in the tree can encrypt (F0,
-- `ordo-crypto`, does not exist), and writing the plaintext into a column
-- typed for ciphertext would be the exact confusion the type is there to
-- prevent. They are seeded null / empty and every op that reads them is
-- refused at compile time with `sealed_unsupported`.

INSERT INTO "vendors" ("$space", "id", "name", "region", "active") VALUES
  (@SPACE@, 'vend_1', 'Acme',   'east', true),
  (@SPACE@, 'vend_2', 'Blythe', 'west', true),
  (@SPACE@, 'vend_3', 'Corvid', 'east', false);

INSERT INTO "categories" ("$space", "id", "name", "parent", "depth", "$path") VALUES
  (@SPACE@, 'cat_1', 'goods',     NULL,    0, 'cat_1/'),
  (@SPACE@, 'cat_2', 'tools',     'cat_1', 1, 'cat_1/cat_2/'),
  (@SPACE@, 'cat_3', 'handtools', 'cat_2', 2, 'cat_1/cat_2/cat_3/'),
  (@SPACE@, 'cat_4', 'services',  NULL,    0, 'cat_4/'),
  (@SPACE@, 'cat_5', 'repair',    'cat_4', 1, 'cat_4/cat_5/');

INSERT INTO "products" ("$space", "id", "vendor", "category", "name", "price", "active") VALUES
  (@SPACE@, 'prod_1', 'vend_1', 'cat_3', 'hammer',  1500,  true),
  (@SPACE@, 'prod_2', 'vend_1', 'cat_2', 'drill',   9500,  true),
  (@SPACE@, 'prod_3', 'vend_2', 'cat_3', 'saw',     3000,  true),
  (@SPACE@, 'prod_4', 'vend_2', 'cat_5', 'tune-up', 12000, true),
  (@SPACE@, 'prod_5', 'vend_3', 'cat_3', 'wrench',  2000,  false),
  (@SPACE@, 'prod_6', 'vend_3', 'cat_4', 'consult', 20000, true);

-- email carries a REAL blind-index digest now (F1): HMAC under the
-- TestProvider's fixed SMK, normalize(lower) applied first. The payload
-- bytes are still placeholders — nothing in the tree can open a sealed
-- payload, and no corpus case needs to.
-- The `email$bidx` values are `@BIDX(entity,field,value)@` PLACEHOLDERS,
-- resolved by `Fixture::rendered_seed` through `KeyProvider::blind_index`
-- at seed time. They used to be hex, with a standing instruction to
-- regenerate rather than hand-edit; the instruction is now unnecessary,
-- because there is nothing here to edit.
--
-- The reason is the space column (9h). A blind index derives under
-- `BIK = KDF(SMK(space), …)`, so ONE value digests to DIFFERENT bytes in
-- different spaces — and this file is seeded into more than one space.
-- Literal hex could only ever have been right for `conformance`.
-- An empty value is the reserved null sentinel.
--
-- cust_3 has a NULL email, and its digest is NOT null (8l): a nullable
-- `sealed indexed unique` field HMACs the reserved null sentinel
-- (`ordo_crypto::NULL_SENTINEL`), so `null == null` holds in the digest
-- column and `UNIQUE ("$space", "email$bidx")` admits at most ONE null
-- customer PER SPACE. While this column held SQL NULL the unique
-- constraint did not constrain nulls at all.
INSERT INTO "customers" ("$space", "id", "name", "email", "email$bidx", "tier") VALUES
  (@SPACE@, 'cust_1', 'Ada', '\x00'::bytea, @BIDX(customers,email,ada@example.com)@, 'pro'),
  (@SPACE@, 'cust_2', 'Bo',  '\x00'::bytea, @BIDX(customers,email,bo@example.com)@, 'basic'),
  (@SPACE@, 'cust_3', 'Cy',  NULL,          @BIDX(customers,email,)@, 'basic');

INSERT INTO "orders" ("$space", "id", "customer", "placed", "status", "total", "memo") VALUES
  (@SPACE@, 'ord_1', 'cust_1', DATE '2026-01-10', 'done', 4500,  '\x'::bytea),
  (@SPACE@, 'ord_2', 'cust_1', DATE '2026-02-05', 'done', 9500,  '\x'::bytea),
  (@SPACE@, 'ord_3', 'cust_1', DATE '2026-03-15', 'open', 1500,  '\x'::bytea),
  (@SPACE@, 'ord_4', 'cust_2', DATE '2026-02-20', 'done', 12000, '\x'::bytea),
  (@SPACE@, 'ord_5', 'cust_2', DATE '2026-03-01', 'void', 3000,  '\x'::bytea),
  (@SPACE@, 'ord_6', 'cust_3', DATE '2026-03-20', 'open', 20000, '\x'::bytea),
  (@SPACE@, 'ord_7', 'cust_1', DATE '2026-04-02', 'done', 2000,  '\x'::bytea);

INSERT INTO "orderItems" ("$space", "id", "order", "product", "qty", "amount") VALUES
  (@SPACE@, 'item_1', 'ord_1', 'prod_1', 1, 1500),
  (@SPACE@, 'item_2', 'ord_1', 'prod_3', 1, 3000),
  (@SPACE@, 'item_3', 'ord_2', 'prod_2', 1, 9500),
  (@SPACE@, 'item_4', 'ord_3', 'prod_1', 1, 1500),
  (@SPACE@, 'item_5', 'ord_4', 'prod_4', 1, 12000),
  (@SPACE@, 'item_6', 'ord_5', 'prod_3', 1, 3000),
  (@SPACE@, 'item_7', 'ord_6', 'prod_6', 1, 20000),
  (@SPACE@, 'item_8', 'ord_7', 'prod_5', 1, 2000);

-- ── guards-conformance.md §3.2 ──────────────────────────────────────────
-- `kind` is `principal` or `group` (definition-language.md §3, the `members`
-- table) and `memberCount` is "1 for a principal". The seed said `person`
-- and `0`, which no code read and no test asserted — so both were wrong for
-- as long as the fixture has existed, and the group work is the first thing
-- that has to tell a principal row from a group row. Corrected here rather
-- than worked around in the surface that reads it.
-- `principal` is the member's own id: the door resolves a token's principal
-- through this column (enrolment E1), and the harness mints tokens for the
-- member ids. A test that wants one person in two spaces writes its own rows.
INSERT INTO "members" ("$space", "id", "principal", "displayName", "kind", "memberCount", "role", "status", "joined") VALUES
  (@SPACE@, 'mem_1', 'mem_1', 'Ana',  'principal', 1, 'manager', 'active', DATE '2026-01-01'),
  (@SPACE@, 'mem_2', 'mem_2', 'Bram', 'principal', 1, 'editor',  'active', DATE '2026-01-01'),
  (@SPACE@, 'mem_3', 'mem_3', 'Cleo', 'principal', 1, 'editor',  'active', DATE '2026-01-01'),
  (@SPACE@, 'mem_4', 'mem_4', 'Dov',  'principal', 1, 'viewer',  'active', DATE '2026-01-01'),
  (@SPACE@, 'mem_g', 'mem_g', 'Gil',  'principal', 1, NULL,      'active', DATE '2026-01-01'),
  (@SPACE@, 'mem_o', 'mem_o', 'Ora',  'principal', 1, 'owner',   'active', DATE '2026-01-01');

INSERT INTO "projects" ("$space", "id", "name", "owner") VALUES
  (@SPACE@, 'proj_1', 'Alpha', 'mem_1'),
  (@SPACE@, 'proj_2', 'Beta',  'mem_2'),
  (@SPACE@, 'proj_3', 'Gamma', 'mem_3');

INSERT INTO "projectMembers" ("$space", "id", "project", "member", "canWrite") VALUES
  (@SPACE@, 'pm_1', 'proj_1', 'mem_1', true),
  (@SPACE@, 'pm_2', 'proj_1', 'mem_2', false),
  (@SPACE@, 'pm_3', 'proj_2', 'mem_2', true),
  (@SPACE@, 'pm_4', 'proj_2', 'mem_3', true),
  (@SPACE@, 'pm_5', 'proj_3', 'mem_1', false);

INSERT INTO "tasks" ("$space", "id", "project", "assignee", "title", "status") VALUES
  (@SPACE@, 'task_1', 'proj_1', 'mem_1', 'draft spec',  'open'),
  (@SPACE@, 'task_2', 'proj_1', NULL,    'review spec', 'done'),
  (@SPACE@, 'task_3', 'proj_2', 'mem_3', 'build site',  'open'),
  (@SPACE@, 'task_4', 'proj_2', NULL,    'ship site',   'open'),
  (@SPACE@, 'task_5', 'proj_3', 'mem_2', 'audit books', 'open');

INSERT INTO "taskNotes" ("$space", "id", "task", "body") VALUES
  (@SPACE@, 'note_1', 'task_1', 'outline ready'),
  (@SPACE@, 'note_2', 'task_3', 'site copy wip'),
  (@SPACE@, 'note_3', 'task_5', 'numbers pending');

INSERT INTO "boards" ("$space", "id", "name") VALUES
  (@SPACE@, 'board_1', 'roadmap'),
  (@SPACE@, 'board_2', 'icebox');

INSERT INTO "boardsAccess" ("$space", "id", "board", "member", "level") VALUES
  (@SPACE@, 'ba_1', 'board_1', 'mem_1', 'admin'),
  (@SPACE@, 'ba_2', 'board_1', 'mem_2', 'write'),
  (@SPACE@, 'ba_3', 'board_1', 'mem_3', 'read'),
  (@SPACE@, 'ba_4', 'board_2', 'mem_2', 'admin'),
  (@SPACE@, 'ba_5', 'board_1', 'mem_g', 'read');

INSERT INTO "cards" ("$space", "id", "board", "title", "done") VALUES
  (@SPACE@, 'card_1', 'board_1', 'q3 goals', false),
  (@SPACE@, 'card_2', 'board_1', 'q4 goals', true),
  (@SPACE@, 'card_3', 'board_2', 'someday',  false);

-- A guarded tree (`folders`, owner-writes). Ana owns everything except
-- fold_4, which sits one level under her root: her cascade from fold_1
-- therefore has to reach a row she may not delete, and a cascade from
-- fold_5 does not. The path/depth columns are seeded the way the platform
-- would have written them.
INSERT INTO "folders" ("$space", "id", "owner", "name", "parent", "depth", "$path") VALUES
  (@SPACE@, 'fold_1', 'mem_1', 'work',    NULL,     0, 'fold_1/'),
  (@SPACE@, 'fold_2', 'mem_1', 'specs',   'fold_1', 1, 'fold_1/fold_2/'),
  (@SPACE@, 'fold_3', 'mem_1', 'drafts',  'fold_2', 2, 'fold_1/fold_2/fold_3/'),
  (@SPACE@, 'fold_4', 'mem_2', 'bram',    'fold_1', 1, 'fold_1/fold_4/'),
  (@SPACE@, 'fold_5', 'mem_1', 'archive', NULL,     0, 'fold_5/'),
  (@SPACE@, 'fold_6', 'mem_1', '2025',    'fold_5', 1, 'fold_5/fold_6/'),
  (@SPACE@, 'fold_7', 'mem_1', 'q1',      'fold_6', 2, 'fold_5/fold_6/fold_7/');
