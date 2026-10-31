-- Seed for the `wide` fixture.
--
-- Written BEFORE any want or any query, and frozen. Everything downstream
-- reasons from this table, so it is the one artefact that must not move: a
-- seed edited to make a query pass would invalidate every want at once.
--
-- Shaped to make edge cases reachable rather than to look realistic:
--
--   nulls          every nullable column has at least one null AND at least
--                  one non-null row, so `is null` and `is not null` both
--                  select a proper subset
--   parallel edges prj_3 has lead == sponsor (the SAME row through two refs);
--                  prj_2 has a null sponsor
--   tree           teams is a 3-deep chain (team_1 < team_2 < team_3) plus an
--                  unrelated root in another org
--   orphans        ppl_4 and ppl_6 have no team; evt_3 has no task
--   ties           ppl_2 and ppl_4 share no salary, but tsk_1/tsk_5 share
--                  priority 1 with tsk_4 — three-way tie for ordering tests
--   empty group    team_1 has no people, so grouping people by team must
--                  decide whether team_1 appears with a zero or vanishes
--   zero vs null   ppl_6.salary is 0; ppl_2.age is null. Not the same thing.
--
-- Sealed columns (people.email, people.notes) hold placeholders, not the
-- values they would hold under F0. Nothing can encrypt yet and nothing can
-- read them: `ordo-sema` refuses at compile time.

INSERT INTO "orgs" ("$space", "id", "name", "region", "active", "founded", "seats") VALUES
  (@SPACE@, 'org_1', 'Acme', 'east', true,  DATE '2020-01-15', 50),
  (@SPACE@, 'org_2', 'Bolt', 'west', false, DATE '2022-06-01', 10);

INSERT INTO "teams" ("$space", "id", "org", "parent", "name", "budget", "archived", "depth", "$path") VALUES
  (@SPACE@, 'team_1', 'org_1', NULL,     'Platform', 100000, false, 0, 'team_1/'),
  (@SPACE@, 'team_2', 'org_1', 'team_1', 'Core',      50000, false, 1, 'team_1/team_2/'),
  (@SPACE@, 'team_3', 'org_1', 'team_2', 'Infra',     25000, true,  2, 'team_1/team_2/team_3/'),
  (@SPACE@, 'team_4', 'org_2', NULL,     'Ops',           0, false, 0, 'team_4/');

INSERT INTO "people"
  ("$space", "id", "org", "team", "name", "nickname", "email", "email$bidx", "notes",
   "age", "salary", "active", "hired", "lastSeen", "cadence", "shift", "externalId")
VALUES
-- `people.email` is `sealed indexed unique normalize(lower)`, so its
-- `$bidx` column is NOT NULL and carries a digest for every row (8l).
-- FIVE rows carry a real value's digest and exactly ONE (ppl_6) carries
-- the reserved null sentinel — because a nullable unique field admits at
-- most one null row (DL, uniqueness: `null == null` is true). Six nulls
-- was legal here only while the column stored SQL NULL and the unique
-- constraint therefore constrained nothing. One null PER SPACE, now that
-- the unique index leads with the space column.
--
-- The digests are `@BIDX(entity,field,value)@` placeholders resolved at
-- seed time by `Fixture::rendered_seed`; a blind index derives under the
-- SPACE's own key, so the same address digests differently in two spaces
-- and hex in this file could only be right for one of them. An empty
-- value is the reserved null sentinel.
  (@SPACE@, 'ppl_1', 'org_1', 'team_2', 'Ada', NULL,  '\x00'::bytea, @BIDX(people,email,ada@example.com)@, '\x'::bytea,
     34,   120000, true,  DATE '2021-03-01', TIMESTAMPTZ '2026-05-01T09:00:00Z', 604800000, 8, 'x-1'),
  (@SPACE@, 'ppl_2', 'org_1', 'team_2', 'Bo',  'B',   '\x00'::bytea, @BIDX(people,email,bo@example.com)@, '\x'::bytea,
     NULL,  90000, true,  DATE '2022-07-15', NULL,                               604800000, 8, NULL),
  (@SPACE@, 'ppl_3', 'org_1', 'team_3', 'Cy',  NULL,  '\x00'::bytea, @BIDX(people,email,cy@example.com)@, '\x'::bytea,
     45,   150000, false, DATE '2019-01-10', TIMESTAMPTZ '2026-04-20T13:30:00Z', 604800000, 8, 'x-3'),
  (@SPACE@, 'ppl_4', 'org_1', NULL,     'Di',  'Dee', '\x00'::bytea, @BIDX(people,email,di@example.com)@, '\x'::bytea,
     29,    80000, true,  DATE '2023-02-01', NULL,                               604800000, 4, NULL),
  (@SPACE@, 'ppl_5', 'org_2', 'team_4', 'Eli', NULL,  '\x00'::bytea, @BIDX(people,email,eli@example.com)@, '\x'::bytea,
     51,   200000, true,  DATE '2018-11-05', TIMESTAMPTZ '2026-05-02T08:15:00Z', 604800000, 8, 'x-5'),
  (@SPACE@, 'ppl_6', 'org_2', NULL,     'Fay', NULL,  NULL,          @BIDX(people,email,)@, '\x'::bytea,
     NULL,      0, false, DATE '2024-09-09', NULL,                               604800000, 8, NULL);

INSERT INTO "projects"
  ("$space", "id", "org", "lead", "sponsor", "name", "code", "budget", "startsOn", "ends", "shipped")
VALUES
  (@SPACE@, 'prj_1', 'org_1', 'ppl_1', 'ppl_3', 'Apollo',   'APL', 500000, DATE '2026-01-01', DATE '2026-12-31', false),
  (@SPACE@, 'prj_2', 'org_1', 'ppl_2', NULL,    'Borealis', 'BOR', 250000, DATE '2026-02-15', NULL,             false),
  -- lead and sponsor are the same row, reached through two different refs.
  (@SPACE@, 'prj_3', 'org_2', 'ppl_5', 'ppl_5', 'Cinder',   'CIN', 100000, DATE '2025-06-01', DATE '2026-03-01', true);

INSERT INTO "tasks"
  ("$space", "id", "project", "assignee", "title", "priority", "estimate", "due", "done", "points")
VALUES
  (@SPACE@, 'tsk_1', 'prj_1', 'ppl_1', 'Design', 1,  8,    DATE '2026-03-01', true,  500),
  (@SPACE@, 'tsk_2', 'prj_1', 'ppl_2', 'Build',  2,  40,   DATE '2026-06-01', false, 2000),
  (@SPACE@, 'tsk_3', 'prj_1', NULL,    'Review', 3,  NULL, NULL,              false, 100),
  (@SPACE@, 'tsk_4', 'prj_2', 'ppl_4', 'Spike',  1,  16,   DATE '2026-04-01', false, 300),
  (@SPACE@, 'tsk_5', 'prj_3', 'ppl_5', 'Ship',   1,  4,    DATE '2026-02-01', true,  1000),
  (@SPACE@, 'tsk_6', 'prj_3', NULL,    'Retro',  5,  NULL, NULL,              false, 0);

INSERT INTO "tags" ("$space", "id", "label") VALUES
  (@SPACE@, 'tag_1', 'urgent'),
  (@SPACE@, 'tag_2', 'backend'),
  (@SPACE@, 'tag_3', 'design');

INSERT INTO "taskTags" ("$space", "id", "task", "tag") VALUES
  (@SPACE@, 'ttg_1', 'tsk_1', 'tag_3'),
  (@SPACE@, 'ttg_2', 'tsk_2', 'tag_2'),
  (@SPACE@, 'ttg_3', 'tsk_2', 'tag_1'),
  (@SPACE@, 'ttg_4', 'tsk_4', 'tag_1');

INSERT INTO "events" ("$space", "id", "task", "kind", "at", "took", "billable") VALUES
  (@SPACE@, 'evt_1', 'tsk_1', 'start', TIMESTAMPTZ '2026-01-05T10:00:00Z', 3600000, true),
  (@SPACE@, 'evt_2', 'tsk_2', 'note',  TIMESTAMPTZ '2026-02-10T14:30:00Z', NULL,    false),
  (@SPACE@, 'evt_3', NULL,    'audit', TIMESTAMPTZ '2026-03-01T00:00:00Z', 60000,   false);
