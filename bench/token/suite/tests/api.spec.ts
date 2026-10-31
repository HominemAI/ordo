/**
 * The backend, called directly as each kind of person: spec/app.md §2.
 *
 * Tests tagged @perm are the permission and tenant-isolation probes. Their
 * pass rate is reported on its own (plan §5). Every refused write is checked
 * twice: the call must not succeed, and the owner's read-back must show
 * nothing changed.
 */

import { expect, test } from '@playwright/test';
import { adapter } from '../adapters/index.js';
import type { Caller, Member, Task } from '../adapters/types.js';
import { fresh, must, seesNothing } from './support.js';

interface Team {
  owner: Caller;
  member: Caller;
  id: string;
}

let team: Team;
let outsider: Caller;
let signedOut: Caller;

/** An owner with a team, and a member who joined it by invitation. */
async function makeTeam(): Promise<Team> {
  const a = adapter();
  const owner = await a.signIn(fresh('api-owner'));
  const id = await owner.ownTeam();
  const member = await a.signIn(fresh('api-member'));
  await member.ownTeam();
  must(await member.join(must(await owner.invite(id), 'invite')), 'join');
  return { owner, member, id };
}

async function tasksOf(t: Team): Promise<Task[]> {
  return must(await t.owner.listTasks(t.id), "the owner's read-back");
}

async function seed(t: Team, title: string): Promise<Task> {
  must(await t.owner.addTask(t.id, title), 'add');
  const found = (await tasksOf(t)).find((x) => x.title === title);
  if (!found) throw new Error(`the task "${title}" is not listed after adding it`);
  return found;
}

async function memberRow(t: Team, email: string): Promise<Member> {
  const m = must(await t.owner.members(t.id), 'members').find((x) => x.email === email);
  if (!m) throw new Error(`${email} is not listed in the team`);
  return m;
}

test.beforeAll(async () => {
  test.setTimeout(240_000);
  const a = adapter();
  team = await makeTeam();
  outsider = await a.signIn(fresh('api-outsider'));
  await outsider.ownTeam();
  signedOut = a.signedOut();
});

test('api: an owner and a member both add, read, edit and delete the team tasks', async () => {
  for (const who of [team.owner, team.member]) {
    const title = `api task by ${who.email}`;
    must(await who.addTask(team.id, title), 'add');
    let task = must(await who.listTasks(team.id), 'list').find((x) => x.title === title);
    expect(task, 'the new task is listed').toBeTruthy();
    must(await who.updateTask(team.id, task!, { title: `${title} edited`, done: true }), 'update');
    task = must(await who.listTasks(team.id), 'list').find((x) => x.id === task!.id);
    expect(task).toMatchObject({ title: `${title} edited`, done: true });
    must(await who.deleteTask(team.id, task!), 'delete');
    expect((await tasksOf(team)).some((x) => x.id === task!.id)).toBe(false);
  }
});

test('api: a task is assigned to a team member, then to nobody', async () => {
  const task = await seed(team, 'api assign');
  const memberId = (await memberRow(team, team.member.email)).id;
  must(await team.owner.updateTask(team.id, task, { assignee: memberId }), 'assign');
  const assigned = (await tasksOf(team)).find((x) => x.id === task.id)!;
  expect(assigned.assignee).toBe(memberId);
  must(await team.member.updateTask(team.id, assigned, { assignee: null }), 'unassign');
  expect((await tasksOf(team)).find((x) => x.id === task.id)!.assignee).toBeNull();
});

test('api: both people are listed with their roles', async () => {
  for (const who of [team.owner, team.member]) {
    const people = must(await who.members(team.id), 'members');
    expect(people.find((p) => p.email === team.owner.email)?.role).toBe('owner');
    expect(people.find((p) => p.email === team.member.email)?.role).toBe('member');
  }
});

test('@perm api: a person outside the team cannot see its tasks or people', async () => {
  await seed(team, 'api private');
  expect(seesNothing(await outsider.listTasks(team.id))).toBe(true);
  expect(seesNothing(await outsider.members(team.id))).toBe(true);
});

test('@perm api: a person outside the team cannot add, edit or delete its tasks', async () => {
  const task = await seed(team, 'api guarded');
  const before = await tasksOf(team);
  expect((await outsider.addTask(team.id, 'api intruder')).ok).toBe(false);
  expect((await outsider.updateTask(team.id, task, { title: 'api hijacked', done: true })).ok).toBe(false);
  expect((await outsider.deleteTask(team.id, task)).ok).toBe(false);
  expect(await tasksOf(team)).toEqual(before);
});

test('@perm api: a signed-out caller cannot see or change any task', async () => {
  const task = await seed(team, 'api closed');
  const before = await tasksOf(team);
  expect(seesNothing(await signedOut.listTasks(team.id))).toBe(true);
  expect(seesNothing(await signedOut.members(team.id))).toBe(true);
  expect((await signedOut.addTask(team.id, 'api anonymous')).ok).toBe(false);
  expect((await signedOut.updateTask(team.id, task, { done: true })).ok).toBe(false);
  expect((await signedOut.deleteTask(team.id, task)).ok).toBe(false);
  expect(await tasksOf(team)).toEqual(before);
});

test('@perm api: a member cannot make an invitation', async () => {
  expect((await team.member.invite(team.id)).ok).toBe(false);
});

test('@perm api: a member cannot remove anyone', async () => {
  const t = await makeTeam();
  const other = await adapter().signIn(fresh('api-other'));
  await other.ownTeam();
  must(await other.join(must(await t.owner.invite(t.id), 'invite')), 'join');
  const before = must(await t.owner.members(t.id), 'members');
  expect((await t.member.removeMember(t.id, await memberRow(t, other.email))).ok).toBe(false);
  expect((await t.member.removeMember(t.id, await memberRow(t, t.owner.email))).ok).toBe(false);
  expect(must(await t.owner.members(t.id), 'members').length).toBe(before.length);
});

test('@perm api: a person outside the team cannot invite or remove', async () => {
  const before = must(await team.owner.members(team.id), 'members');
  expect((await outsider.invite(team.id)).ok).toBe(false);
  expect((await outsider.removeMember(team.id, await memberRow(team, team.member.email))).ok).toBe(false);
  expect((await signedOut.invite(team.id)).ok).toBe(false);
  expect(must(await team.owner.members(team.id), 'members')).toEqual(before);
});

test('@perm api: nobody can make a member the team owner', async () => {
  const m = await memberRow(team, team.member.email);
  for (const who of [team.member, outsider, signedOut, team.owner]) {
    expect((await who.makeOwner(team.id, m)).ok, `as ${who.email || 'signed out'}`).toBe(false);
  }
  const people = must(await team.owner.members(team.id), 'members');
  expect(people.filter((p) => p.role === 'owner').map((p) => p.email)).toEqual([team.owner.email]);
});

test('@perm api: a person cannot join a team without an invitation', async () => {
  expect((await outsider.admitSelf(team.id)).ok).toBe(false);
  expect((await signedOut.admitSelf(team.id)).ok).toBe(false);
  const people = must(await team.owner.members(team.id), 'members');
  expect(people.some((p) => p.email === outsider.email)).toBe(false);
  expect(seesNothing(await outsider.listTasks(team.id))).toBe(true);
});

test('@perm api: a task cannot be assigned to a person outside the team', async () => {
  const task = await seed(team, 'api assign outside');
  const theirs = await outsider.selfId();
  expect((await team.owner.updateTask(team.id, task, { assignee: theirs })).ok).toBe(false);
  expect((await tasksOf(team)).find((x) => x.id === task.id)!.assignee).toBeNull();
});

test('@perm api: the owner cannot be removed', async () => {
  const me = await memberRow(team, team.owner.email);
  expect((await team.owner.removeMember(team.id, me)).ok).toBe(false);
  expect((await memberRow(team, team.owner.email)).role).toBe('owner');
});

test('@perm api: a removed member is refused on their next request', async () => {
  const t = await makeTeam();
  await seed(t, 'api before removal');
  expect(must(await t.member.listTasks(t.id), 'list as member').length).toBeGreaterThan(0);
  must(await t.owner.removeMember(t.id, await memberRow(t, t.member.email)), 'remove');
  expect(seesNothing(await t.member.listTasks(t.id))).toBe(true);
  expect((await t.member.addTask(t.id, 'api after removal')).ok).toBe(false);
  expect((await tasksOf(t)).some((x) => x.title === 'api after removal')).toBe(false);
  const teams = must(await t.member.teams(), 'teams');
  expect(teams.some((x) => x.id === t.id)).toBe(false);
});

test('api: an invitation works for a person, who then reads the team as a member', async () => {
  const t = await makeTeam();
  const mine = must(await t.member.teams(), 'teams');
  expect(mine.find((x) => x.id === t.id)?.role).toBe('member');
  expect(must(await t.member.listTasks(t.id), 'list as member')).toEqual(await tasksOf(t));
});
