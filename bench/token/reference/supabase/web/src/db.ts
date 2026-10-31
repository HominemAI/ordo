/**
 * The page's one way to Supabase: a client, and the reads and writes the
 * page makes. Every failure becomes an Error with a message a person can act
 * on.
 *
 * The session lives in localStorage (supabase-js's default), so a reload
 * stays signed in.
 */

import { createClient, type PostgrestError } from '@supabase/supabase-js';

const url: string = import.meta.env.VITE_SUPABASE_URL ?? 'http://localhost:54321';
const key: string = import.meta.env.VITE_SUPABASE_ANON_KEY ?? '';

export const db = createClient(url, key);

export interface Team {
  readonly id: string;
  /** The owner's email. */
  readonly owner: string;
  readonly role: 'owner' | 'member';
}

export interface Person {
  readonly id: string;
  readonly email: string;
  readonly role: 'owner' | 'member';
}

export interface Task {
  readonly id: string;
  readonly title: string;
  readonly done: boolean;
  readonly assignee: string | null;
}

function check<T>(r: { data: T | null; error: PostgrestError | null }): T {
  if (r.error) throw new Error(r.error.message);
  return r.data as T;
}

/** The invitation token in `#/join/<token>`, or null. */
export function joinOf(hash: string): string | null {
  return /^#\/join\/([A-Za-z0-9_-]+)$/.exec(hash)?.[1] ?? null;
}

export function inviteLink(token: string): string {
  return `${window.location.origin}/#/join/${token}`;
}

export async function sendCode(email: string): Promise<void> {
  const { error } = await db.auth.signInWithOtp({ email, options: { shouldCreateUser: true } });
  if (error) throw new Error(error.message);
}

export async function verifyCode(email: string, code: string): Promise<void> {
  const { error } = await db.auth.verifyOtp({ email, token: code.trim(), type: 'email' });
  if (error) throw new Error(error.message);
}

/** The person's teams, each with its owner's email and the person's role. */
export async function myTeams(me: string): Promise<Team[]> {
  // Row level security shows the people of every team the caller is in.
  const rows = check(await db.from('team_members').select('team_id, user_id, email, role'));
  const owners = new Map(rows.filter((r) => r.role === 'owner').map((r) => [r.team_id as string, r.email as string]));
  return rows
    .filter((r) => r.user_id === me)
    .map((r) => ({ id: r.team_id, owner: owners.get(r.team_id) ?? r.team_id, role: r.role === 'owner' ? 'owner' : 'member' }));
}

export async function people(team: string): Promise<Person[]> {
  const rows = check(
    await db.from('team_members').select('user_id, email, role').eq('team_id', team).order('joined_at'),
  );
  return rows.map((r) => ({ id: r.user_id, email: r.email, role: r.role === 'owner' ? 'owner' : 'member' }));
}

export async function tasks(team: string): Promise<Task[]> {
  return check(await db.from('tasks').select('id, title, done, assignee').eq('team_id', team).order('created_at'));
}

export async function addTask(team: string, title: string): Promise<void> {
  check(await db.from('tasks').insert({ team_id: team, title }));
}

export async function changeTask(id: string, change: Partial<Omit<Task, 'id'>>): Promise<void> {
  const rows = check(await db.from('tasks').update(change).eq('id', id).select('id'));
  if (rows.length === 0) throw new Error('That task is gone or no longer yours to change. The list has been reloaded.');
}

export async function deleteTask(id: string): Promise<void> {
  check(await db.from('tasks').delete().eq('id', id));
}

export async function invite(team: string): Promise<string> {
  return check(await db.rpc('create_invite', { p_team_id: team })) as string;
}

export async function acceptInvite(token: string): Promise<string> {
  return check(await db.rpc('accept_invite', { p_token: token })) as string;
}

export async function removePerson(team: string, user: string): Promise<void> {
  const rows = check(await db.from('team_members').delete().eq('team_id', team).eq('user_id', user).select('user_id'));
  if (rows.length === 0) throw new Error('That person could not be removed.');
}
