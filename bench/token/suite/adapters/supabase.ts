/**
 * The Supabase adapter: neutral calls onto supabase-js, as
 * spec/bindings/supabase.md binds them.
 *
 * - A team is a `teams` row; people are `team_members` rows keyed by the
 *   `auth.users` id.
 * - A person signs in by emailed code (`signInWithOtp`, then `verifyOtp`),
 *   read from Mailpit.
 * - Invitations are the two functions `create_invite` and `accept_invite`.
 *
 * Row level security refuses a write by changing nothing, not by an error.
 * So an update or delete that touched no row is a refusal here, and the
 * probe reads back as the owner as well.
 */

import { createClient, type SupabaseClient } from '@supabase/supabase-js';
import type { StackInfo } from './index.js';
import { nextCode, ok, refused, type Adapter, type Caller, type Member, type Outcome, type Task, type TeamRef } from './types.js';

type Row = Record<string, unknown>;

export function supabase(info: StackInfo): Adapter {
  const url = info.url ?? 'http://127.0.0.1:54321';
  const key = info.anonKey ?? '';
  const fresh = (): SupabaseClient =>
    createClient(url, key, { auth: { persistSession: false, autoRefreshToken: true, detectSessionInUrl: false } });

  const adapter: Adapter = {
    stack: 'supabase',
    page: info.page,

    async mailbox(email) {
      const q = encodeURIComponent(`to:"${email.toLowerCase()}"`);
      const r = await fetch(`${info.mail}/api/v1/search?query=${q}&limit=50`);
      if (!r.ok) return [];
      const found = (await r.json()) as { messages?: { ID: string; Created: string }[] };
      const list = [...(found.messages ?? [])].sort((a, b) => a.Created.localeCompare(b.Created));
      const texts: string[] = [];
      for (const m of list) {
        const one = (await (await fetch(`${info.mail}/api/v1/message/${m.ID}`)).json()) as { Text?: string; HTML?: string };
        texts.push(one.Text || one.HTML || '');
      }
      return texts;
    },

    codeIn(text) {
      return /\b(\d{6})\b/.exec(text)?.[1] ?? null;
    },

    async signIn(email) {
      const db = fresh();
      const before = (await adapter.mailbox(email)).length;
      const sent = await db.auth.signInWithOtp({ email, options: { shouldCreateUser: true } });
      if (sent.error) throw new Error(`signInWithOtp: ${sent.error.message}`);
      const code = await nextCode(adapter, email, before);
      const done = await db.auth.verifyOtp({ email, token: code, type: 'email' });
      if (done.error || !done.data.user) throw new Error(`verifyOtp: ${done.error?.message ?? 'no user'}`);
      return caller(email, db, done.data.user.id);
    },

    signedOut() {
      return caller('', fresh(), null);
    },
  };

  function caller(email: string, db: SupabaseClient, me: string | null): Caller {
    const fail = (e: { message: string } | null, what: string): Outcome<never> => refused(e ? e.message : what);

    /** An update or delete: refused when it errs or touches no row. */
    const touched = (r: { error: { message: string } | null; data: unknown[] | null }): Outcome =>
      r.error ? fail(r.error, '') : (r.data?.length ?? 0) > 0 ? ok(undefined) : refused('no row changed');

    const teams = async (): Promise<Outcome<TeamRef[]>> => {
      if (!me) return refused('signed out');
      const r = await db.from('team_members').select('team_id, role').eq('user_id', me);
      if (r.error) return fail(r.error, '');
      return ok((r.data as Row[]).map((x) => ({ id: String(x['team_id']), role: x['role'] === 'owner' ? 'owner' : 'member' })));
    };

    return {
      email,
      async selfId() {
        if (!me) throw new Error('signed out');
        return me;
      },
      async ownTeam() {
        // The team is made on the backend at sign-up; give it a moment.
        for (let i = 0; i < 40; i++) {
          const t = await teams();
          const own = t.ok ? t.value.find((x) => x.role === 'owner') : undefined;
          if (own) return own.id;
          await new Promise((res) => setTimeout(res, 250));
        }
        throw new Error(`${email} owns no team`);
      },
      teams,
      async listTasks(team) {
        const r = await db.from('tasks').select('id, title, done, assignee').eq('team_id', team);
        if (r.error) return fail(r.error, '');
        return ok(
          (r.data as Row[]).map(
            (x): Task => ({
              id: String(x['id']),
              title: String(x['title']),
              done: x['done'] === true,
              assignee: x['assignee'] == null ? null : String(x['assignee']),
            }),
          ),
        );
      },
      async addTask(team, title) {
        const r = await db.from('tasks').insert({ team_id: team, title }).select('id');
        return touched(r);
      },
      async updateTask(_team, task, patch) {
        const change: Row = {};
        if (patch.title !== undefined) change['title'] = patch.title;
        if (patch.done !== undefined) change['done'] = patch.done;
        if (patch.assignee !== undefined) change['assignee'] = patch.assignee;
        return touched(await db.from('tasks').update(change).eq('id', task.id).select('id'));
      },
      async deleteTask(_team, task) {
        return touched(await db.from('tasks').delete().eq('id', task.id).select('id'));
      },
      async members(team) {
        const r = await db.from('team_members').select('user_id, email, role').eq('team_id', team);
        if (r.error) return fail(r.error, '');
        return ok(
          (r.data as Row[]).map(
            (x): Member => ({ id: String(x['user_id']), email: String(x['email']), role: x['role'] === 'owner' ? 'owner' : 'member' }),
          ),
        );
      },
      async invite(team) {
        const r = await db.rpc('create_invite', { p_team_id: team });
        if (r.error) return fail(r.error, '');
        return typeof r.data === 'string' && r.data ? ok({ team, token: r.data }) : refused('no token in the answer');
      },
      async join(invite) {
        const r = await db.rpc('accept_invite', { p_token: invite.token });
        return r.error ? fail(r.error, '') : ok(undefined);
      },
      async removeMember(team, member) {
        return touched(await db.from('team_members').delete().eq('team_id', team).eq('user_id', member.id).select('user_id'));
      },
      async makeOwner(team, member) {
        return touched(
          await db.from('team_members').update({ role: 'owner' }).eq('team_id', team).eq('user_id', member.id).select('user_id'),
        );
      },
      async admitSelf(team) {
        const r = await db
          .from('team_members')
          .insert({ team_id: team, user_id: me ?? '00000000-0000-0000-0000-000000000000', email: email || 'nobody@example.com', role: 'member' })
          .select('user_id');
        return touched(r);
      },
    };
  }

  return adapter;
}
