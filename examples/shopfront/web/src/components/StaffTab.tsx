import type { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';
import { useState } from 'react';
import type { MembersId, ShopsId } from '../generated/ids.js';
import { grantShopsAccess, revokeShopsAccess, type StaffOfRow } from '../generated/ops.js';
import { useAct } from '../hooks.js';
import { Problem } from './Account.js';

/** Who works in the shop. Admins add and remove people by member id. */
export function StaffTab(props: {
  client: OrdoClient;
  shop: ShopsId;
  rows: readonly StaffOfRow[];
  error: string | null;
  isAdmin: boolean;
  self: string | null;
  reload: () => void;
}) {
  const { client, shop } = props;
  const [member, setMember] = useState('');
  const [level, setLevel] = useState<'staff' | 'admin'>('staff');
  const act = useAct();

  const grant = async () => {
    const ok = await act.run(() =>
      grantShopsAccess(client, { shop, member: member.trim() as MembersId, level }).commit(),
    );
    if (ok) {
      setMember('');
      props.reload();
    }
  };

  const remove = async (who: string) => {
    const ok = await act.run(() => revokeShopsAccess(client, { shop, member: who as MembersId }).commit());
    if (ok) props.reload();
  };

  return (
    <section aria-labelledby="staff-title">
      <h2 id="staff-title">Staff</h2>
      <Problem text={props.error} />
      {props.rows.length === 0 && props.error === null && <p className="hint">No one yet.</p>}
      <ul className="plain">
        {props.rows.map((s) => (
          <li key={s.id} className="card">
            <strong>{s.name}</strong> <span className="hint">{s.level}</span>
            <div className="hint">member {s.member}</div>
            {props.isAdmin && s.member !== props.self && (
              <div className="row">
                <button type="button" disabled={act.busy} onClick={() => void remove(s.member)}>
                  Remove
                </button>
              </div>
            )}
          </li>
        ))}
      </ul>
      {props.isAdmin ? (
        <form
          className="card"
          aria-labelledby="grant-title"
          onSubmit={(e) => {
            e.preventDefault();
            void grant();
          }}
        >
          <h3 id="grant-title">Add a person</h3>
          <label htmlFor="grant-member">Member id</label>
          <input id="grant-member" value={member} onChange={(e) => setMember(e.target.value)} />
          <label htmlFor="grant-level">Level</label>
          <select id="grant-level" value={level} onChange={(e) => setLevel(e.target.value as 'staff' | 'admin')}>
            <option value="staff">Staff</option>
            <option value="admin">Admin</option>
          </select>
          <div className="row">
            <button type="submit" className="primary" disabled={member.trim() === '' || act.busy}>
              Add
            </button>
          </div>
        </form>
      ) : (
        <p className="hint">Only an admin can add or remove staff.</p>
      )}
      <Problem text={act.error} />
    </section>
  );
}
