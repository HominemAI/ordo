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

import { agg, and, asc, bare, dateLit, desc, domain, entity, f, fn, g, lit, named, nul, op, param, src, synth, t } from '../../src/index.js';

const vendors = entity('vendors', {
  name: t.text(),
  region: t.text(),
  active: t.bool().default(lit(true)),
});

const categories = entity('categories', {
  name: t.text().unique(),
  parent: t.ref('categories').optional().tree(),
}, { key: 'cat' });

const products = entity('products', {
  vendor: t.ref('vendors'),
  category: t.ref('categories'),
  name: t.text(),
  price: t.money(),
  active: t.bool().default(lit(true)),
});

const customers = entity('customers', {
  name: t.text(),
  email: t.text().optional().unique().sealed().indexed().normalize('lower'),
  tier: t.text().default(lit('basic')),
});

const orders = entity('orders', {
  customer: t.ref('customers'),
  placed: t.date(),
  status: t.text().default(lit('open')),
  total: t.money(),
  memo: t.text().sealed().default(lit('')),
}, { key: 'ord' });

const orderItems = entity('orderItems', {
  order: t.ref('orders').owned(),
  product: t.ref('products'),
  qty: t.int(),
  amount: t.money(),
}, { key: 'item' });

const projects = entity('projects', {
  name: t.text(),
  owner: t.ref('members'),
})
  .accessRead(g.walk(g.any(g.all(g.eq('owner')), g.all(g.step('projectMembers', g.any(g.all(g.eq('member'))))))))
  .accessWrite(g.walk(g.any(g.all(g.eq('owner')))));

const projectMembers = entity('projectMembers', {
  project: t.ref('projects').owned(),
  member: t.ref('members').owned(),
  canWrite: t.bool().default(lit(false)),
})
  .accessRead(g.walk(g.any(g.all(g.step('project.projectMembers', g.any(g.all(g.eq('member'))))))))
  .accessWrite(g.walk(g.any(g.all(g.eq('project.owner')))));

const tasks = entity('tasks', {
  project: t.ref('projects').owned(),
  assignee: t.ref('members').optional(),
  title: t.text(),
  status: t.text().default(lit('open')),
}, { key: 'task' })
  .accessRead(g.walk(g.any(g.all(g.step('project.projectMembers', g.any(g.all(g.eq('member'))))))))
  .accessWrite(g.walk(g.any(g.all(g.step('project.projectMembers', g.any(g.all(g.eq('member'), g.bare('canWrite'))))))));

const taskNotes = entity('taskNotes', {
  task: t.ref('tasks').owned(),
  body: t.text(),
}, { key: 'note' });

const boards = entity('boards', {
  name: t.text(),
}, { key: 'board' })
  .acl('read', 'write', 'admin');

const cards = entity('cards', {
  board: t.ref('boards').owned(),
  title: t.text(),
  done: t.bool().default(lit(false)),
}, { key: 'card' });

const folders = entity('folders', {
  owner: t.ref('members'),
  name: t.text(),
  parent: t.ref('folders').optional().tree(),
}, { key: 'fold' })
  .accessRead(g.OPEN)
  .accessWrite(g.walk(g.any(g.all(g.eq('owner')))));

const ledgerEntries = entity('ledgerEntries', {
  externalId: t.text().unique(),
  owner: t.ref('members'),
  amount: t.money(),
  note: t.text().default(lit('')),
}, { key: 'led' })
  .accessRead(g.walk(g.any(g.all(g.eq('owner')), g.all(g.eq('note', lit('shared'))))))
  .accessWrite(g.walk(g.any(g.all(g.eq('owner')))));

const coupons = entity('coupons', {
  code: t.text().unique(),
  slug: t.text().unique(),
  value: t.money(),
});

const activeProducts = op('activeProducts', {}, (b) =>
  b
    .source(src('products').args(f.active.eq(lit(true))).pick('name', 'price', 'vendor', 'category')))
  .unbounded();

const cheapActive = op('cheapActive', { cap: t.money() }, (b, p) =>
  b
    .source(src('activeProducts').args(f.price.lt(p.cap)).pick('name', 'price', 'category')))
  .unbounded();

const toolDeals = op('toolDeals', { cap: t.money(), root: t.id() }, (b, p) =>
  b
    .source(src('cheapActive').args(named('cap', p.cap), f.category.under(p.root)).pick('name', 'price'))
    .sort(asc(f.price)))
  .unbounded();

const recentOrders = op('recentOrders', { since: t.date().default(dateLit('2026-03-01')) }, (b, p) =>
  b
    .source(src('orders').args(f.placed.ge(p.since)).pick('placed', 'status', 'total'))
    .sort(asc(f.placed)))
  .unbounded();

const doneOrders = op('doneOrders', { from: t.date() }, (b, p) =>
  b
    .source(src('orders').args(and(f.status.eq(lit('done')), f.placed.ge(p.from))).pick('placed', 'total', 'customer')))
  .unbounded();

const spendByCustomer = op('spendByCustomer', { from: t.date() }, (b, p) =>
  b
    .source(src('doneOrders').args(named('from', p.from)).pick('total', 'customer'))
    .group([f.customer], { spent: agg.sum(f.total), n: agg.count() }))
  .unbounded();

const monthlyRevenue = op('monthlyRevenue', { from: t.date() }, (b, p) =>
  b
    .source(src('doneOrders').args(named('from', p.from)).pick('placed', 'total'))
    .group([fn.month(f.placed)], { revenue: agg.sum(f.total) }))
  .unbounded();

const productRevenue = op('productRevenue', {}, (b) =>
  b
    .source(src('orderItems').pick('amount', 'product'))
    .group([f.product], { sold: agg.count(), revenue: agg.sum(f.amount) }))
  .unbounded();

const top3Orders = op('top3Orders', {}, (b) =>
  b
    .source(src('orders').pick('placed', 'total', 'customer'))
    .sort(desc(f.total))
    .take(3));

const catInfo = op('catInfo', {}, (b) =>
  b
    .source(src('categories').pick('name', 'parent', 'depth')))
  .unbounded();

const customerByEmail = op('customerByEmail', { e: t.text() }, (b, p) =>
  b
    .source(src('customers').args(f.email.eq(p.e)).pick('name', 'tier')));

const giftOrders = op('giftOrders', {}, (b) =>
  b
    .source(src('orders').args(f.memo.eq(lit('gift'))).pick('placed', 'total', 'customer')))
  .unbounded();

const memoDigest = op('memoDigest', {}, (b) =>
  b
    .source(src('orders').pick('placed', 'memo'))
    .sort(asc(f.placed))
    .take(3));

const orderMemos = op('orderMemos', {}, (b) =>
  b
    .source(src('orders').pick('placed', 'total', 'memo')))
  .unbounded();

const myProjects = op('myProjects', {}, (b) =>
  b
    .source(src('projects').pick('name', 'owner'))
    .sort(asc(f.name)))
  .unbounded();

const taskBoard = op('taskBoard', {}, (b) =>
  b
    .source(src('tasks').pick('title', 'status', 'project'))
    .sort(asc(f.title)))
  .unbounded();

const openTasks = op('openTasks', {}, (b) =>
  b
    .source(src('tasks').args(f.status.eq(lit('open'))).pick('title', 'project'))
    .sort(asc(f.title)))
  .unbounded();

const myAssignedTasks = op('myAssignedTasks', {}, (b) =>
  b
    .source(src('tasks').args(f.assignee.eq(param('caller'))).pick('title'))
    .sort(asc(f.title)))
  .unbounded();

const noteFeed = op('noteFeed', {}, (b) =>
  b
    .source(src('taskNotes').pick('body', 'task'))
    .sort(asc(f.body)))
  .unbounded();

const teamRoster = op('teamRoster', {}, (b) =>
  b
    .source(src('projectMembers').pick('project', 'member', 'canWrite')))
  .unbounded();

const allTasks = op('allTasks', {}, (b) =>
  b
    .source(src('tasks').pick('title', 'status'))
    .sort(asc(f.title)))
  .unbounded()
  .unguarded();

const boardList = op('boardList', {}, (b) =>
  b
    .source(src('boards').pick('name'))
    .sort(asc(f.name)))
  .unbounded();

const cardList = op('cardList', {}, (b) =>
  b
    .source(src('cards').pick('title', 'done', 'board'))
    .sort(asc(f.title)))
  .unbounded();

const firstTask = op('firstTask', {}, (b) =>
  b
    .source(src('tasks').pick('title'))
    .sort(asc(f.title))
    .take(1));

const specTasks = op('specTasks', {}, (b) =>
  b
    .source(src('openTasks').args(f.title.contains(lit('spec'))).pick('title')))
  .unbounded();

const specCount = op('specCount', {}, (b) =>
  b
    .source(src('specTasks').pick('title'))
    .reduce({ n: agg.count() }));

const addTask = op('addTask', { project: t.id(), title: t.text() }, (b, p) =>
  [
    b.set('tasks', { project: p.project, title: p.title }),
  ]);

const claimTask = op('claimTask', { task: t.id() }, (b, p) =>
  [
    b.set('tasks', { assignee: param('caller') }).where(f.id.eq(p.task)),
  ]);

const renameTask = op('renameTask', { task: t.id(), title: t.text() }, (b, p) =>
  [
    b.set('tasks', { title: p.title }).where(f.id.eq(p.task)),
  ]);

const closeTask = op('closeTask', { task: t.id() }, (b, p) =>
  [
    b.set('tasks', { status: lit('done') }).where(f.id.eq(p.task)),
  ]);

const purgeDone = op('purgeDone', { project: t.id() }, (b, p) =>
  [
    b.del('tasks', and(f.project.eq(p.project), f.status.eq(lit('done')))),
  ]);

const addNote = op('addNote', { task: t.id(), body: t.text() }, (b, p) =>
  [
    b.set('taskNotes', { task: p.task, body: p.body }),
  ]);

const addBoard = op('addBoard', { name: t.text() }, (b, p) =>
  [
    b.set('boards', { name: p.name }),
  ]);

const addCard = op('addCard', { board: t.id(), title: t.text() }, (b, p) =>
  [
    b.set('cards', { board: p.board, title: p.title }),
  ]);

const finishCard = op('finishCard', { card: t.id() }, (b, p) =>
  [
    b.set('cards', { done: lit(true) }).where(f.id.eq(p.card)),
  ]);

const addCustomer = op('addCustomer', { name: t.text(), email: t.text() }, (b, p) =>
  [
    b.set('customers', { name: p.name, email: p.email }),
  ]);

const addAnonCustomer = op('addAnonCustomer', { name: t.text() }, (b, p) =>
  [
    b.set('customers', { name: p.name }),
  ]);

const clearCustomerEmail = op('clearCustomerEmail', { c: t.id() }, (b, p) =>
  [
    b.set('customers', { email: nul() }).where(f.id.eq(p.c)),
  ]);

const anonCustomers = op('anonCustomers', {}, (b) =>
  b
    .source(src('customers').args(f.email.eq(nul())).pick('name')));

const movePM = op('movePM', { pm: t.id(), p: t.id() }, (b, p) =>
  [
    b.set('projectMembers', { project: p.p }).where(f.id.eq(p.pm)),
  ]);

const moveBA = op('moveBA', { ba: t.id(), b: t.id() }, (b, p) =>
  [
    b.set('boardsAccess', { board: p.b }).where(f.id.eq(p.ba)),
  ]);

const renameTaskIf = op('renameTaskIf', { task: t.id(), title: t.text(), rev: t.int() }, (b, p) =>
  [
    b.set('tasks', { title: p.title, rev: p.rev }).where(f.id.eq(p.task)),
  ]);

const reassign = op('reassign', { task: t.id(), who: t.id() }, (b, p) =>
  [
    b.set('tasks', { assignee: p.who }).where(f.id.eq(p.task)),
  ]);

const addCategory = op('addCategory', { name: t.text(), parent: t.id() }, (b, p) =>
  [
    b.set('categories', { name: p.name, parent: p.parent }),
  ]);

const addRootCategory = op('addRootCategory', { name: t.text() }, (b, p) =>
  [
    b.set('categories', { name: p.name }),
  ]);

const moveCategory = op('moveCategory', { cat: t.id(), parent: t.id() }, (b, p) =>
  [
    b.set('categories', { parent: p.parent }).where(f.id.eq(p.cat)),
  ]);

const dropCategory = op('dropCategory', { cat: t.id() }, (b, p) =>
  [
    b.del('categories', f.id.eq(p.cat)),
  ]);

const setMemo = op('setMemo', { order: t.id(), memo: t.text() }, (b, p) =>
  [
    b.set('orders', { memo: p.memo }).where(f.id.eq(p.order)),
  ]);

const dropFolder = op('dropFolder', { f: t.id() }, (b, p) =>
  [
    b.del('folders', f.id.eq(p.f)),
  ]);

const dropFolderTree = op('dropFolderTree', { f: t.id() }, (b, p) =>
  [
    b.del('folders', f.id.eq(p.f)).subtree(),
  ]);

const dropCategoryTree = op('dropCategoryTree', { cat: t.id() }, (b, p) =>
  [
    b.del('categories', f.id.eq(p.cat)).subtree(),
  ]);

const purgeNotes = op('purgeNotes', {}, (b) =>
  [
    b.del('taskNotes', bare(lit(true))),
  ]);

const entryList = op('entryList', {}, (b) =>
  b
    .source(src('ledgerEntries').pick('externalId', 'amount', 'note', 'owner'))
    .sort(asc(f.externalId)))
  .unbounded();

const recordEntry = op('recordEntry', { ext: t.text(), amount: t.money(), note: t.text().default(lit('')) }, (b, p) =>
  [
    b.set('ledgerEntries', { externalId: p.ext, owner: param('caller'), amount: p.amount, note: p.note }),
  ]);

const recordEntryFor = op('recordEntryFor', { ext: t.text(), owner: t.id(), amount: t.money() }, (b, p) =>
  [
    b.set('ledgerEntries', { externalId: p.ext, owner: p.owner, amount: p.amount }),
  ]);

const recordEntryOnce = op('recordEntryOnce', { ext: t.text(), amount: t.money() }, (b, p) =>
  [
    b.set('ledgerEntries', { externalId: p.ext, owner: param('caller'), amount: p.amount, rev: nul() }),
  ]);

const reviseEntry = op('reviseEntry', { ext: t.text(), amount: t.money(), rev: t.int() }, (b, p) =>
  [
    b.set('ledgerEntries', { externalId: p.ext, owner: param('caller'), amount: p.amount, rev: p.rev }),
  ]);

const renumberEntry = op('renumberEntry', { entry: t.id(), ext: t.text() }, (b, p) =>
  [
    b.set('ledgerEntries', { id: p.entry, externalId: p.ext }),
  ]);

const renumberEntryOnce = op('renumberEntryOnce', { entry: t.id(), ext: t.text() }, (b, p) =>
  [
    b.set('ledgerEntries', { id: p.entry, externalId: p.ext, rev: nul() }),
  ]);

const claimCoupon = op('claimCoupon', { code: t.text(), slug: t.text(), value: t.money() }, (b, p) =>
  [
    b.set('coupons', { code: p.code, slug: p.slug, value: p.value }),
  ]);

const upsertCustomer = op('upsertCustomer', { name: t.text(), email: t.text(), tier: t.text().default(lit('basic')) }, (b, p) =>
  [
    b.set('customers', { email: p.email, name: p.name, tier: p.tier }),
  ]);

export const ordo: string = synth(
  domain('market', '1.1.0'),
  vendors,
  categories,
  products,
  customers,
  orders,
  orderItems,
  projects,
  projectMembers,
  tasks,
  taskNotes,
  boards,
  cards,
  folders,
  ledgerEntries,
  coupons,
  activeProducts,
  cheapActive,
  toolDeals,
  recentOrders,
  doneOrders,
  spendByCustomer,
  monthlyRevenue,
  productRevenue,
  top3Orders,
  catInfo,
  customerByEmail,
  giftOrders,
  memoDigest,
  orderMemos,
  myProjects,
  taskBoard,
  openTasks,
  myAssignedTasks,
  noteFeed,
  teamRoster,
  allTasks,
  boardList,
  cardList,
  firstTask,
  specTasks,
  specCount,
  addTask,
  claimTask,
  renameTask,
  closeTask,
  purgeDone,
  addNote,
  addBoard,
  addCard,
  finishCard,
  addCustomer,
  addAnonCustomer,
  clearCustomerEmail,
  anonCustomers,
  movePM,
  moveBA,
  renameTaskIf,
  reassign,
  addCategory,
  addRootCategory,
  moveCategory,
  dropCategory,
  setMemo,
  dropFolder,
  dropFolderTree,
  dropCategoryTree,
  purgeNotes,
  entryList,
  recordEntry,
  recordEntryFor,
  recordEntryOnce,
  reviseEntry,
  renumberEntry,
  renumberEntryOnce,
  claimCoupon,
  upsertCustomer,
);
