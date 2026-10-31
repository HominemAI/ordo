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

import { and, asc, cond, daily, desc, domain, durationLit, each, entity, f, lit, op, or, routine, src, synth, t } from '../../src/index.js';

const plants = entity('plants', {
  name: t.text(),
  room: t.text().optional(),
  everyDays: t.duration('ms').default(durationLit(7, 'd')),
  notes: t.text().sealed().default(lit('')),
});

const waterings = entity('waterings', {
  plant: t.ref('plants').owned(),
  at: t.date(),
  by: t.text().default(lit('')),
});

const alerts = entity('alerts', {
  key: t.text().unique(),
  severity: t.text().default(lit('warning')),
  title: t.text(),
  status: t.text().default(lit('active')),
  raised: t.date(),
});

const thirsty = op('thirsty', { today: t.date() }, (b, p) =>
  b
    .source(src('plants').pick('name', 'room', 'everyDays'), src('waterings').pick('at'))
    .sort(desc(f.waterings.at))
    .take([f.plants.id], 1)
    .where(or(f.at.le(p.today.sub(f.everyDays)), f.at.isNull()))
    .sort(asc(cond([[f.at.isNull(), lit(0)]], lit(1))), asc(f.at)))
  .unbounded();

const history = op('history', { plant: t.id() }, (b, p) =>
  b
    .source(src('waterings').args(f.plant.eq(p.plant)).pick('at', 'by'))
    .sort(desc(f.at))
    .take(30));

const water = op('water', { plant: t.id(), by: t.text(), today: t.date() }, (b, p) =>
  [
    b.set('waterings', { plant: p.plant, by: p.by, at: p.today }),
    b.set('alerts', { status: lit('resolved') }).where(and(f.key.eq(lit('thirsty:').add(p.plant)), f.status.eq(lit('active')))),
  ]);

const addPlant = op('addPlant', { name: t.text(), room: t.text(), everyDays: t.duration('ms') }, (b, p) =>
  [
    b.set('plants', { name: p.name, room: p.room, everyDays: p.everyDays }),
  ]);

const pruneLogs = op('pruneLogs', { before: t.date() }, (b, p) =>
  [
    b.del('waterings', f.at.le(p.before)),
  ]);

const thirstCheck = routine('thirstCheck', { in: 'plantcare', trigger: daily('09:00Z'), params: { today: t.date() } }, (b, p) =>
  each(b
    .source(src('plants').pick('id', 'name', 'everyDays'), src('waterings').pick('at'))
    .sort(desc(f.waterings.at))
    .take([f.plants.id], 1)
    .where(or(f.at.le(p.today.sub(f.everyDays)), f.at.isNull())), [
    b.set('alerts', { key: lit('thirsty:').add(f.plants.id), title: f.name.add(lit(' needs water')), raised: p.today, status: lit('active') }),
  ]));

export const ordo: string = synth(
  domain('plantcare', '0.1.0'),
  plants,
  waterings,
  alerts,
  thirsty,
  history,
  water,
  addPlant,
  pruneLogs,
  thirstCheck,
);
