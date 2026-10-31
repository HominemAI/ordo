// GENERATED — do not edit; regenerate with `ordo gen ts`.
//
// One corpus program, rebuilt through the builder.
//
// Source of truth: the program's own parsed AST, translated by `ordo_gen::emit`
//
// This file is the GENERATED half of this package. The hand-written
// half is under `src/core/` and is versioned by hand; the boundary
// between them is documented in the package README. An edit here is
// lost the next time the generator runs.

import { agg, desc, domain, durationLit, entity, f, lit, op, src, synth, t } from '../../src/index.js';

const orgs = entity('orgs', {
  name: t.text().unique(),
  region: t.text().default(lit('unset')),
  active: t.bool().default(lit(true)),
  founded: t.date(),
  seats: t.int().default(lit(0)),
});

const teams = entity('teams', {
  org: t.ref('orgs'),
  parent: t.ref('teams').optional().tree(),
  name: t.text(),
  budget: t.money().default(lit(0)),
  archived: t.bool().default(lit(false)),
});

const people = entity('people', {
  org: t.ref('orgs'),
  team: t.ref('teams').optional(),
  name: t.text(),
  nickname: t.text().optional(),
  email: t.text().optional().unique().sealed().indexed().normalize('lower'),
  notes: t.text().sealed().default(lit('')),
  age: t.int().optional(),
  salary: t.money().default(lit(0)),
  active: t.bool().default(lit(true)),
  hired: t.date(),
  lastSeen: t.datetime().optional(),
  cadence: t.duration('ms').default(durationLit(7, 'd')),
  shift: t.duration('h').default(durationLit(8, 'h')),
  externalId: t.id().optional(),
}, { key: 'ppl' });

const projects = entity('projects', {
  org: t.ref('orgs'),
  lead: t.ref('people'),
  sponsor: t.ref('people').optional(),
  name: t.text(),
  code: t.text().unique(),
  budget: t.money().default(lit(0)),
  startsOn: t.date(),
  ends: t.date().optional(),
  shipped: t.bool().default(lit(false)),
}, { key: 'prj' });

const tasks = entity('tasks', {
  project: t.ref('projects').owned(),
  assignee: t.ref('people').optional(),
  title: t.text(),
  priority: t.int().default(lit(3)),
  estimate: t.duration('h').optional(),
  due: t.date().optional(),
  done: t.bool().default(lit(false)),
  points: t.money().default(lit(0)),
}, { key: 'tsk' });

const tags = entity('tags', {
  label: t.text().unique(),
});

const taskTags = entity('taskTags', {
  task: t.ref('tasks').owned(),
  tag: t.ref('tags'),
}, { key: 'ttg' });

const events = entity('events', {
  task: t.ref('tasks').optional(),
  kind: t.text().default(lit('note')),
  at: t.datetime(),
  took: t.duration('ms').optional(),
  billable: t.bool().default(lit(false)),
}, { key: 'evt' });

const documents = entity('documents', {
  project: t.ref('projects').owned(),
  title: t.text(),
  scan: t.blob().optional(),
  contract: t.blob().optional().sealed(),
  avatar: t.blob().optional().inline(),
  source: t.uri().optional(),
  canonical: t.uri().optional().unique().sealed().indexed().normalize('lower'),
  vault: t.blob().optional().private(),
  filedOn: t.date(),
}, { key: 'doc' });

const activePeople = op('activePeople', {}, (b) =>
  b
    .source(src('people').args(f.active.eq(lit(true))).pick('name', 'org', 'team', 'salary', 'hired')))
  .unbounded();

const orgTeams = op('orgTeams', {}, (b) =>
  b
    .source(src('teams').pick('name', 'org', 'budget')))
  .unbounded();

const openTasks = op('openTasks', {}, (b) =>
  b
    .source(src('tasks').args(f.done.eq(lit(false))).pick('title', 'priority', 'project', 'assignee')))
  .unbounded();

const projectRoster = op('projectRoster', {}, (b) =>
  b
    .source(src('projects').pick('name', 'code', 'lead', 'sponsor', 'budget')))
  .unbounded();

const tasksByProject = op('tasksByProject', {}, (b) =>
  b
    .source(src('tasks').pick('points', 'project'))
    .group([f.project], { total: agg.sum(f.points), n: agg.count() }))
  .unbounded();

const topPaid = op('topPaid', {}, (b) =>
  b
    .source(src('people').pick('name', 'salary'))
    .sort(desc(f.salary))
    .take(5));

const eventLog = op('eventLog', {}, (b) =>
  b
    .source(src('events').pick('kind', 'at', 'task'))
    .sort(desc(f.at)))
  .unbounded();

const projectDocuments = op('projectDocuments', {}, (b) =>
  b
    .source(src('documents').pick('title', 'scan', 'source', 'filedOn', 'project'))
    .sort(desc(f.filedOn)))
  .unbounded();

export const ordo: string = synth(
  domain('wide', '1.0.0'),
  orgs,
  teams,
  people,
  projects,
  tasks,
  tags,
  taskTags,
  events,
  documents,
  activePeople,
  orgTeams,
  openTasks,
  projectRoster,
  tasksByProject,
  topPaid,
  eventLog,
  projectDocuments,
);
