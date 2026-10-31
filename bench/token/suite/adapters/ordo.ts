/**
 * The Ordo adapter: neutral calls onto the door, as spec/bindings/ordo.md
 * binds them.
 *
 * - A team is a tenant; calls name it in `X-Ordo-Space`.
 * - A person signs in by emailed code (`/auth/begin`, `/auth/complete`,
 *   with `display` set to their email).
 * - Program ops: `listTasks`, `addTask`, `updateTask`, `deleteTask`,
 *   `listMembers`. Platform ops: `std.space.invite`, `remove`, `setRole`,
 *   `admit`.
 * - Joining is a fresh sign-in that carries the invitation.
 *
 * Any answer other than 200 is a refusal. The probe reads back as the owner
 * as well, so a refusal that is really a silent no-op is caught either way.
 */

import type { StackInfo } from './index.js';
import { nextCode, ok, refused, type Adapter, type Caller, type Invite, type Member, type Outcome, type Task, type TeamRef } from './types.js';

type Wire = Record<string, unknown>;

interface Held {
  token: string;
  expiresAt: number;
  principal: string;
}

export function ordo(info: StackInfo): Adapter {
  const door = info.door ?? 'http://127.0.0.1:4710';
  const origin = info.page;
  let seq = 0;

  const post = (path: string, body: Wire, token?: string, space?: string) =>
    fetch(`${door}${path}`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Accept: 'application/json',
        Origin: origin,
        ...(token ? { Authorization: `Bearer ${token}` } : {}),
        ...(space ? { 'X-Ordo-Space': space, 'Idempotency-Key': `bench-${Date.now()}-${++seq}` } : {}),
      },
      body: JSON.stringify(body),
    });

  const get = (path: string, token?: string, space?: string) =>
    fetch(`${door}${path}`, {
      headers: {
        Accept: 'application/json',
        Origin: origin,
        ...(token ? { Authorization: `Bearer ${token}` } : {}),
        ...(space ? { 'X-Ordo-Space': space } : {}),
      },
    });

  async function answer(r: Response): Promise<Outcome<Wire[]>> {
    const text = await r.text();
    let body: Wire = {};
    try {
      body = JSON.parse(text) as Wire;
    } catch {
      // Not JSON: the status says it all.
    }
    if (r.status !== 200) {
      const err = body['error'] as Wire | undefined;
      return refused(`${r.status} ${String(err?.['class'] ?? err?.['code'] ?? '')} ${String(err?.['message'] ?? text.slice(0, 200))}`.trim());
    }
    return ok(Array.isArray(body['rows']) ? (body['rows'] as Wire[]) : []);
  }

  const adapter: Adapter = {
    stack: 'ordo',
    page: info.page,

    async mailbox(email) {
      const r = await fetch(`${info.mail}/messages?to=${encodeURIComponent(email.toLowerCase())}`);
      const list = (await r.json()) as { data: string }[];
      return list.map((m) => m.data);
    },

    codeIn(text) {
      const plain = text.replace(/=\r?\n/g, '');
      return (/code is (\d+)/.exec(plain) ?? /\b(\d{6,8})\b/.exec(plain))?.[1] ?? null;
    },

    async signIn(email) {
      const held = await emailSignIn(email);
      return caller(email, held);
    },

    signedOut() {
      return caller('', null);
    },
  };

  /** One emailed-code sign-in. With an invitation, it joins that team too. */
  async function emailSignIn(email: string, invite?: Invite): Promise<Held> {
    const extra: Wire = invite ? { space: invite.team, invitation: invite.token } : {};
    const before = (await adapter.mailbox(email)).length;
    const begun = await post('/auth/begin', { method: 'email', identifier: email, ...extra });
    if (begun.status !== 200) throw new Error(`/auth/begin answered ${begun.status}: ${await begun.text()}`);
    const { challenge, binder } = (await begun.json()) as { challenge: string; binder: string };
    const code = await nextCode(adapter, email, before);
    const done = await post('/auth/complete', {
      method: 'email',
      kind: 'email_code',
      challenge,
      binder,
      code,
      display: email,
      ...extra,
    });
    if (done.status !== 200) throw new Error(`/auth/complete answered ${done.status}: ${await done.text()}`);
    const s = (await done.json()) as Wire;
    const token = String(s['token']);
    if (s['provisioning'] === 'in progress') await provisioned(token);
    if (invite) {
      const outcome = (s['admission'] as Wire | undefined)?.['outcome'];
      if (outcome !== 'admitted' && outcome !== 'already-member') {
        throw new Error(`the sign-in did not join the team: admission ${JSON.stringify(s['admission'] ?? null)}`);
      }
    }
    return { token, expiresAt: Number(s['expires_at'] ?? 0), principal: String(s['principal'] ?? s['sub'] ?? 'unknown') };
  }

  async function provisioned(token: string): Promise<void> {
    const until = Date.now() + 120_000;
    while (Date.now() < until) {
      const r = await get('/auth/provisioning', token);
      const s = (await r.json().catch(() => ({}))) as Wire;
      if (s['state'] === 'ready') return;
      if (s['state'] === 'failed') throw new Error(`the new team failed: ${JSON.stringify(s)}`);
      await new Promise((res) => setTimeout(res, 250));
    }
    throw new Error('the new team was not ready in 2 minutes');
  }

  /** Wait until the door serves a team made or joined a moment ago. */
  async function served(held: Held, space: string): Promise<void> {
    const until = Date.now() + 15_000;
    while (Date.now() < until) {
      if ((await get('/op/listTasks', held.token, space)).status === 200) return;
      await new Promise((res) => setTimeout(res, 150));
    }
  }

  function caller(email: string, start: Held | null): Caller {
    let held = start;

    const token = async (): Promise<string | undefined> => {
      if (!held) return undefined;
      if (held.expiresAt - Date.now() / 1000 < 60) {
        const r = await post('/auth/refresh', {}, held.token);
        if (r.status === 200) {
          const b = (await r.json()) as { token: string; expires_at: number };
          held = { ...held, token: b.token, expiresAt: b.expires_at };
        }
      }
      return held.token;
    };

    const read = async (op: string, team: string) => answer(await get(`/op/${op}`, await token(), team));
    const write = async (op: string, team: string, body: Wire) => answer(await post(`/op/${op}`, body, await token(), team));
    const done = (o: Outcome<Wire[]>): Outcome => (o.ok ? ok(undefined) : o);

    const members = async (team: string): Promise<Outcome<Member[]>> => {
      const o = await read('listMembers', team);
      if (!o.ok) return o;
      return ok(
        o.value.map((r) => ({
          id: String(r['id']),
          email: String(r['displayName'] ?? ''),
          role: r['role'] === 'owner' ? 'owner' : 'member',
        })),
      );
    };

    const teams = async (): Promise<Outcome<TeamRef[]>> => {
      const t = await token();
      if (!t) return refused('signed out');
      const r = await get('/auth/spaces', t);
      if (r.status !== 200) return refused(`${r.status}`);
      const list = (await r.json()) as Wire[];
      return ok(
        list
          .filter((s) => s['via'] !== 'guest' && typeof s['role'] === 'string')
          .map((s) => ({ id: String(s['space']), role: s['role'] === 'owner' ? 'owner' : 'member' })),
      );
    };

    return {
      email,
      async selfId() {
        const own = await this.ownTeam();
        const m = await members(own);
        const me = m.ok ? m.value.find((x) => x.email === email) : undefined;
        if (!me) throw new Error(`${email} is not listed in their own team`);
        return me.id;
      },
      async ownTeam() {
        const t = await teams();
        const own = t.ok ? t.value.find((x) => x.role === 'owner') : undefined;
        if (!own) throw new Error(`${email} owns no team`);
        if (held) await served(held, own.id);
        return own.id;
      },
      teams,
      async listTasks(team) {
        const o = await read('listTasks', team);
        if (!o.ok) return o;
        return ok(
          o.value.map((r) => ({
            id: String(r['id']),
            rev: Number(r['rev']),
            title: String(r['title']),
            done: r['done'] === true,
            assignee: r['assignee'] == null ? null : String(r['assignee']),
          })),
        );
      },
      async addTask(team, title) {
        return done(await write('addTask', team, { title }));
      },
      async updateTask(team, task, patch) {
        const assignee = patch.assignee === undefined ? task.assignee : patch.assignee;
        return done(
          await write('updateTask', team, {
            id: task.id,
            rev: task.rev ?? 0,
            title: patch.title ?? task.title,
            done: patch.done ?? task.done,
            assignee,
          }),
        );
      },
      async deleteTask(team, task) {
        // A delete op is called with DELETE, its params in the query.
        const t = await token();
        const r = await fetch(`${door}/op/deleteTask?id=${encodeURIComponent(task.id)}`, {
          method: 'DELETE',
          headers: {
            Accept: 'application/json',
            Origin: origin,
            'X-Ordo-Space': team,
            'Idempotency-Key': `bench-${Date.now()}-${++seq}`,
            ...(t ? { Authorization: `Bearer ${t}` } : {}),
          },
        });
        return done(await answer(r));
      },
      members,
      async invite(team) {
        const o = await write('std.space.invite', team, { role: 'editor' });
        if (!o.ok) return o;
        const ticket = o.value[0]?.['ticket'];
        return typeof ticket === 'string' ? ok({ team, token: ticket }) : refused('no ticket in the answer');
      },
      async join(invite) {
        if (!held) return refused('signed out');
        try {
          held = await emailSignIn(email, invite);
        } catch (e) {
          return refused(e instanceof Error ? e.message : String(e));
        }
        await served(held, invite.team);
        return ok(undefined);
      },
      async removeMember(team, member) {
        return done(await write('std.space.remove', team, { member: member.id }));
      },
      async makeOwner(team, member) {
        return done(await write('std.space.setRole', team, { member: member.id, role: 'owner' }));
      },
      async admitSelf(team) {
        return done(await write('std.space.admit', team, { principal: held?.principal ?? 'nobody', role: 'editor' }));
      },
    };
  }

  return adapter;
}
