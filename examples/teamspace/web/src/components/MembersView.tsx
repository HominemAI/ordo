import { useCommit, useOpQuery } from '@hominemai/ordo-react';
import { useMemo, useState, type FormEvent } from 'react';
import { refusalText } from '../errors.js';
import { message } from '../hooks.js';
import { inviteLink, invitableRoles, invitationOf, personOf, when, type Me } from '../teamspace.js';

interface Props {
  readonly me: Me;
  /** The team being worked in, which the invitation link names. */
  readonly space: string;
}

interface Made {
  readonly url: string;
  readonly expiresAt: number;
}

const ROLES = ['owner', 'manager', 'editor', 'viewer'] as const;

/**
 * Admins only: who is in the team, who has been invited, and the form that
 * invites someone. Only an owner changes a role.
 *
 * Invitations and role changes are platform ops. They want a fresh passkey
 * touch, which the provider's `stepUp` asks for.
 */
export function MembersView({ me, space }: Props) {
  const people = useOpQuery('people');
  const invitations = useOpQuery('std.space.invitations');
  const invite = useCommit('std.space.invite');
  const revoke = useCommit('std.space.revokeInvitation');
  const setRole = useCommit('std.space.setRole');
  const everyone = useMemo(() => people.rows.map((r) => personOf({ ...r })), [people.rows]);
  const open = useMemo(
    () => invitations.rows.map((r) => invitationOf({ ...r })).filter((i) => i.status === 'open'),
    [invitations.rows],
  );
  const roles = invitableRoles(me.role);
  const [role, setInviteRole] = useState<string>('editor');
  const [label, setLabel] = useState('');
  const [made, setMade] = useState<Made | null>(null);
  const [thrown, setThrown] = useState<string | null>(null);

  const isOwner = me.role === 'owner';
  const refusal = invite.refusal ?? revoke.refusal ?? setRole.refusal ?? people.refusal ?? invitations.refusal;
  const shown = thrown ?? (refusal ? refusalText(refusal) : null);
  const busy = invite.committing || revoke.committing || setRole.committing;

  const run = async (act: () => Promise<void>) => {
    for (const hook of [invite, revoke, setRole]) hook.reset();
    setThrown(null);
    try {
      await act();
    } catch (e) {
      setThrown(message(e));
    }
  };

  const send = (e: FormEvent) => {
    e.preventDefault();
    void run(async () => {
      const out = await invite.commit({ role, expires: '7d', ...(label.trim() !== '' ? { label: label.trim() } : {}) });
      if (out.status !== 'committed') return;
      const row = out.response.rows[0];
      const ticket = typeof row?.['ticket'] === 'string' ? row['ticket'] : '';
      const expiresAt = typeof row?.['expiresAt'] === 'number' ? row['expiresAt'] : 0;
      if (ticket === '') return;
      setMade({ url: inviteLink(window.location, space, ticket), expiresAt });
      setLabel('');
      invitations.refetch();
    });
  };

  return (
    <section aria-label="Members">
      <div className="card">
        <h2>People</h2>
        <ul className="plain">
          {everyone.map((p) => (
            <li key={p.id} className="between">
              <span>
                {p.displayName}
                {p.id === me.id && <span className="hint"> (you)</span>}
              </span>
              {isOwner && p.id !== me.id ? (
                <select
                  aria-label={`Role of ${p.displayName}`}
                  value={p.role}
                  disabled={busy}
                  onChange={(e) =>
                    void run(async () => {
                      const out = await setRole.commit({ member: p.id, role: e.target.value });
                      if (out.status === 'committed') people.refetch();
                    })
                  }
                >
                  {ROLES.map((r) => (
                    <option key={r} value={r}>
                      {r}
                    </option>
                  ))}
                </select>
              ) : (
                <span className="hint">{p.role}</span>
              )}
            </li>
          ))}
        </ul>
      </div>

      <form className="card" onSubmit={send}>
        <h2>Invite someone</h2>
        <p className="hint">You get a link to send them. It works for 7 days.</p>
        <label htmlFor="invite-role">Role</label>
        <select id="invite-role" value={role} onChange={(e) => setInviteRole(e.target.value)}>
          {roles.map((r) => (
            <option key={r} value={r}>
              {r}
            </option>
          ))}
        </select>
        <label htmlFor="invite-label">Note for yourself</label>
        <input id="invite-label" value={label} onChange={(e) => setLabel(e.target.value)} placeholder="Who is this for?" />
        <div className="row">
          <button type="submit" className="primary" disabled={busy}>
            Make an invitation link
          </button>
        </div>
        {made !== null && (
          <div className="link">
            <input readOnly value={made.url} aria-label="Invitation link" onFocus={(e) => e.currentTarget.select()} />
            <button type="button" onClick={() => void navigator.clipboard?.writeText(made.url)}>
              Copy
            </button>
            <p className="hint">
              This is the only time the link is shown. It works until {when(made.expiresAt)}. Send it the way you would
              send a password.
            </p>
          </div>
        )}
      </form>

      {shown !== null && (
        <p role="alert" className="error">
          {shown}
        </p>
      )}

      <div className="card">
        <h2>Open invitations</h2>
        {open.length === 0 && <p className="hint">None.</p>}
        <ul className="plain" aria-label="Open invitations">
          {open.map((i) => (
            <li key={i.id} className="between">
              <span>
                {i.role}
                {i.label !== '' && ` for ${i.label}`}
                <span className="hint">{` · until ${when(i.expiresAt)}`}</span>
              </span>
              <button
                disabled={busy}
                onClick={() =>
                  void run(async () => {
                    const out = await revoke.commit({ id: i.id });
                    if (out.status === 'committed') invitations.refetch();
                  })
                }
              >
                Revoke
              </button>
            </li>
          ))}
        </ul>
      </div>
    </section>
  );
}
