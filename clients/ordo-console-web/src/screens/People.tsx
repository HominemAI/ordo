import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react';
import { ConsoleCallError, explain, SIGNED_OUT } from '../api';
import {
  addToGroup,
  callableBy,
  callersOf,
  createGroup,
  deleteGroup,
  groupMembers,
  invite,
  listPeople,
  reinstate,
  removeFromGroup,
  removeMember,
  ROLES,
  setRole,
  suspend,
  viaWords,
  type Member,
  type PeopleOf,
  type Via,
} from '../people';
import { listTenants, type Tenant } from '../tenants';
import { Access, AdmissionPanel, Invitations, Recovery, type Lent } from './PeopleAdmin';
import { useDeploymentTouch } from './useDeploymentTouch';
import './people.css';

/** The one action a dialog is open for. */
type Open =
  | { kind: 'role'; m: Member }
  | { kind: 'remove'; m: Member }
  | { kind: 'suspend'; m: Member }
  | { kind: 'delete-group'; group: string; name: string }
  | null;

const EXPIRES = [
  { value: '1d', label: '1 day' },
  { value: '7d', label: '7 days' },
  { value: '30d', label: '30 days' },
];

function who(m: Member): string {
  return m.name || m.member;
}

function day(secs: number): string {
  return new Date(secs * 1000).toLocaleDateString(undefined, { dateStyle: 'medium' });
}

/**
 * The people of one tenant where the deployment owner holds a role
 * (docs/plan-console.md 4.5): members and their roles, groups and who is in
 * each, the grants given to apps, what one member can call and who can call
 * one op. Invite, change a role, suspend, remove; make and delete groups and
 * change who is in one; who may call each op; invitations; who may join;
 * a member's recovery and the tenant's recovery settings. Everything here
 * asks for the owner's touch at the deployment; the deployment decides what
 * the owner's role there may see and do.
 */
export function People(props: { onSignedOut: () => void }) {
  const [tenants, setTenants] = useState<Tenant[] | null>(null);
  const [tenant, setTenant] = useState('');
  const [people, setPeople] = useState<PeopleOf | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [ticket, setTicket] = useState<string | null>(null);
  const [inviteRole, setInviteRole] = useState('');
  const [inviteExpires, setInviteExpires] = useState('7d');
  const [inviteLabel, setInviteLabel] = useState('');
  const [can, setCan] = useState<{ m: Member; ops: Via[] } | null>(null);
  const [inGroup, setInGroup] = useState<{ group: string; members: Member[] } | null>(null);
  const [groupPick, setGroupPick] = useState('');
  const [memberPick, setMemberPick] = useState('');
  const [op, setOp] = useState('');
  const [callers, setCallers] = useState<{ op: string; callers: Via[] } | null>(null);
  const [open, setOpen] = useState<Open>(null);
  const [newRole, setNewRole] = useState('viewer');
  const [groupName2, setGroupName2] = useState('');
  const actionDialog = useRef<HTMLDialogElement>(null);
  const { atDeployment, prompt } = useDeploymentTouch();

  const act = useCallback(
    async (f: () => Promise<void>) => {
      setBusy(true);
      setError(null);
      setDone(null);
      try {
        await f();
      } catch (e) {
        if (e instanceof ConsoleCallError && SIGNED_OUT.has(e.code)) {
          props.onSignedOut();
          return;
        }
        if (e instanceof DOMException && e.name === 'AbortError') return;
        if (e instanceof ConsoleCallError && e.extra.doorError === 'access_denied') {
          setError(
            'The deployment refused: your role in this tenant may not do that, or the touch is more than 5 minutes old. Try again to touch once more.',
          );
          return;
        }
        setError(explain(e));
      } finally {
        setBusy(false);
      }
    },
    [props],
  );

  useEffect(() => {
    // Once, on open: the tenants where the owner holds a role that may read people.
    void act(async () => {
      const r = await listTenants();
      const mine = r.tenants.filter((t) => t.yourRole === 'owner' || t.yourRole === 'manager');
      setTenants(mine);
      if (mine[0]) setTenant(mine[0].id);
    });
  }, []);

  useEffect(() => {
    const d = actionDialog.current;
    if (!d) return;
    if (open && !d.open) d.showModal();
    if (!open && d.open) d.close();
  }, [open]);

  const load = async (id: string) => {
    const r = await atDeployment(() => listPeople(id));
    setPeople(r);
    setGroupPick((g) => g || r.groups[0]?.group || '');
    setMemberPick((m) => m || r.members[0]?.member || '');
  };

  const show = (e: FormEvent) => {
    e.preventDefault();
    if (!tenant) return;
    setCan(null);
    setInGroup(null);
    setCallers(null);
    setTicket(null);
    void act(() => load(tenant));
  };

  const change = (f: () => Promise<unknown>, said: string) =>
    act(async () => {
      await atDeployment(f);
      setOpen(null);
      setDone(said);
      await load(tenant);
    });

  const sendInvite = (e: FormEvent) => {
    e.preventDefault();
    void act(async () => {
      const r = await atDeployment(() => invite(tenant, inviteRole, inviteExpires, inviteLabel.trim()));
      setTicket(r.invited.ticket);
      setInviteLabel('');
      setDone(`Invitation made${r.invited.role ? ` for the role ${r.invited.role}` : ''}. It ends ${day(r.invited.expiresAt)}.`);
    });
  };

  const named = (id: string) => {
    const m = people?.members.find((x) => x.member === id);
    return m ? who(m) : id;
  };
  const groupName = (id: string) => people?.groups.find((g) => g.group === id)?.name ?? id;
  const tenantName = tenants?.find((t) => t.id === tenant)?.name ?? tenant;
  const lent: Lent | null = people && {
    tenant,
    people,
    busy,
    act,
    atDeployment,
    said: setDone,
    reload: () => load(tenant),
  };

  return (
    <section className="card">
      <h1>People</h1>
      <p className="muted">
        Who is in a tenant where you hold a role, and what each person may do. Reading and changing both ask for your
        passkey at the deployment; one touch covers the next 5 minutes. What you see is what your role there may see.
      </p>

      {tenants !== null && tenants.length === 0 && (
        <p>You hold no owner or manager role in any tenant, so there are no people for you to see here.</p>
      )}
      {tenants !== null && tenants.length > 0 && (
        <form className="people-pick" onSubmit={show}>
          <div className="field">
            <label htmlFor="people-tenant">Tenant</label>
            <select id="people-tenant" value={tenant} onChange={(e) => setTenant(e.target.value)}>
              {tenants.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.name} (you: {t.yourRole})
                </option>
              ))}
            </select>
          </div>
          <button type="submit" className="primary" disabled={busy}>
            Show people
          </button>
        </form>
      )}

      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      <p role="status" className="ok">
        {done ?? ''}
      </p>

      {people && (
        <>
          <section aria-labelledby="members-h">
            <h2 id="members-h">Members of {tenantName}</h2>
            <div className="people-table" role="region" aria-labelledby="members-caption" tabIndex={0}>
              <table>
                <caption id="members-caption">{people.members.length} members</caption>
                <thead>
                  <tr>
                    <th scope="col">Person</th>
                    <th scope="col">Role</th>
                    <th scope="col">Status</th>
                    <th scope="col">Joined</th>
                    <th scope="col">Groups</th>
                    <th scope="col">Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {people.members.map((m) => (
                    <tr key={m.member}>
                      <th scope="row">
                        <span className="person-name">{who(m)}</span>
                        <span className="mono small muted person-id">{m.member}</span>
                      </th>
                      <td data-label="Role">{m.role ?? 'guest'}</td>
                      <td data-label="Status">{m.status}</td>
                      <td data-label="Joined">{m.joined ?? ''}</td>
                      <td data-label="Groups">{m.groups.map(groupName).join(', ') || 'none'}</td>
                      <td data-label="Actions">
                        <div className="row-actions">
                          <button
                            type="button"
                            disabled={busy}
                            aria-label={`What ${who(m)} can call`}
                            onClick={() =>
                              void act(async () => {
                                const r = await atDeployment(() => callableBy(tenant, m.member));
                                setCan({ m, ops: r.ops });
                              })
                            }
                          >
                            Can call
                          </button>
                          <button
                            type="button"
                            disabled={busy}
                            aria-label={`Change the role of ${who(m)}`}
                            onClick={() => {
                              setNewRole(m.role ?? 'viewer');
                              setOpen({ kind: 'role', m });
                            }}
                          >
                            Role
                          </button>
                          {m.status === 'suspended' ? (
                            <button
                              type="button"
                              disabled={busy}
                              aria-label={`Reinstate ${who(m)}`}
                              onClick={() => void change(() => reinstate(tenant, m.member), `${who(m)} is active again.`)}
                            >
                              Reinstate
                            </button>
                          ) : (
                            <button
                              type="button"
                              disabled={busy}
                              aria-label={`Suspend ${who(m)}`}
                              onClick={() => setOpen({ kind: 'suspend', m })}
                            >
                              Suspend
                            </button>
                          )}
                          <button
                            type="button"
                            className="danger"
                            disabled={busy}
                            aria-label={`Remove ${who(m)}`}
                            onClick={() => setOpen({ kind: 'remove', m })}
                          >
                            Remove
                          </button>
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            {can && (
              <div className="panel" role="region" aria-label={`What ${who(can.m)} can call`}>
                <h3>What {who(can.m)} can call</h3>
                {can.ops.length === 0 ? (
                  <p>Nothing you can also call.</p>
                ) : (
                  <ul className="via-list">
                    {can.ops.map((v, i) => (
                      <li key={i}>
                        <span className="mono">{v.op}</span> {viaWords(v)}
                      </li>
                    ))}
                  </ul>
                )}
                <p className="muted small">Only ops you may call yourself are listed.</p>
              </div>
            )}
          </section>

          <section aria-labelledby="invite-h">
            <h2 id="invite-h">Invite someone</h2>
            <form className="people-form" onSubmit={sendInvite}>
              <div className="field">
                <label htmlFor="invite-role">Role</label>
                <select id="invite-role" value={inviteRole} onChange={(e) => setInviteRole(e.target.value)}>
                  <option value="">The tenant's default</option>
                  {ROLES.map((r) => (
                    <option key={r} value={r}>
                      {r}
                    </option>
                  ))}
                </select>
              </div>
              <div className="field">
                <label htmlFor="invite-lasts">Lasts</label>
                <select id="invite-lasts" value={inviteExpires} onChange={(e) => setInviteExpires(e.target.value)}>
                  {EXPIRES.map((x) => (
                    <option key={x.value} value={x.value}>
                      {x.label}
                    </option>
                  ))}
                </select>
              </div>
              <div className="field">
                <label htmlFor="invite-note">Note for the list (optional)</label>
                <input id="invite-note" value={inviteLabel} maxLength={256} onChange={(e) => setInviteLabel(e.target.value)} />
              </div>
              <button type="submit" className="primary" disabled={busy}>
                Make invitation
              </button>
            </form>
            {ticket && (
              <div className="panel">
                <p>
                  Give this to the person. It is shown once and Ordo does not send it.
                </p>
                <p className="mono ticket" aria-label="Invitation">
                  {ticket}
                </p>
              </div>
            )}
          </section>

          <section aria-labelledby="groups-h">
            <h2 id="groups-h">Groups</h2>
            {people.groups.length === 0 ? (
              <p>This tenant has no groups.</p>
            ) : (
              <ul className="group-list">
                {people.groups.map((g) => (
                  <li key={g.group}>
                    <span className="person-name">{g.name}</span>{' '}
                    <span className="muted small">
                      {g.members.length === 1 ? '1 member' : `${g.members.length} members`}
                    </span>{' '}
                    <button
                      type="button"
                      disabled={busy}
                      aria-label={`Who is in ${g.name}`}
                      onClick={() =>
                        void act(async () => {
                          setInGroup(await atDeployment(() => groupMembers(tenant, g.group)));
                        })
                      }
                    >
                      Who is in it
                    </button>{' '}
                    <button
                      type="button"
                      className="danger"
                      disabled={busy}
                      aria-label={`Delete the group ${g.name}`}
                      onClick={() => setOpen({ kind: 'delete-group', group: g.group, name: g.name })}
                    >
                      Delete
                    </button>
                  </li>
                ))}
              </ul>
            )}
            {inGroup && (
              <div className="panel" role="region" aria-label={`In ${groupName(inGroup.group)}`}>
                <h3>In {groupName(inGroup.group)}</h3>
                {inGroup.members.length === 0 ? (
                  <p>Nobody yet.</p>
                ) : (
                  <ul>
                    {inGroup.members.map((m) => (
                      <li key={m.member}>
                        {who(m)} <span className="muted small">({m.role ?? 'guest'})</span>{' '}
                        <button
                          type="button"
                          disabled={busy}
                          aria-label={`Take ${who(m)} out of ${groupName(inGroup.group)}`}
                          onClick={() =>
                            void act(async () => {
                              const g = inGroup.group;
                              await atDeployment(() => removeFromGroup(tenant, g, m.member));
                              setDone(`Took ${who(m)} out of ${groupName(g)}.`);
                              setInGroup(await atDeployment(() => groupMembers(tenant, g)));
                              await load(tenant);
                            })
                          }
                        >
                          Take out
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            )}
            {people.groups.length > 0 && (
              <form
                className="people-form"
                onSubmit={(e) => {
                  e.preventDefault();
                  if (!groupPick || !memberPick) return;
                  void change(
                    () => addToGroup(tenant, groupPick, memberPick),
                    `Added ${named(memberPick)} to ${groupName(groupPick)}.`,
                  );
                }}
              >
                <div className="field">
                  <label htmlFor="group-pick">Group</label>
                  <select id="group-pick" value={groupPick} onChange={(e) => setGroupPick(e.target.value)}>
                    {people.groups.map((g) => (
                      <option key={g.group} value={g.group}>
                        {g.name}
                      </option>
                    ))}
                  </select>
                </div>
                <div className="field">
                  <label htmlFor="member-pick">Person</label>
                  <select id="member-pick" value={memberPick} onChange={(e) => setMemberPick(e.target.value)}>
                    {people.members.map((m) => (
                      <option key={m.member} value={m.member}>
                        {who(m)}
                      </option>
                    ))}
                  </select>
                </div>
                <button type="submit" disabled={busy}>
                  Add to group
                </button>
              </form>
            )}
          </section>

          <section aria-labelledby="new-group-h">
            <h3 id="new-group-h">Make a group</h3>
            <form
              className="people-form"
              onSubmit={(e) => {
                e.preventDefault();
                const name = groupName2.trim();
                if (!name) return;
                void act(async () => {
                  await atDeployment(() => createGroup(tenant, name));
                  setGroupName2('');
                  setDone(`Made the group ${name}.`);
                  await load(tenant);
                });
              }}
            >
              <div className="field">
                <label htmlFor="new-group">Group name</label>
                <input id="new-group" value={groupName2} maxLength={120} onChange={(e) => setGroupName2(e.target.value)} />
              </div>
              <button type="submit" disabled={busy || !groupName2.trim()}>
                Make group
              </button>
            </form>
          </section>

          <section aria-labelledby="grants-h">
            <h2 id="grants-h">Grants given to apps</h2>
            {people.grants.length === 0 ? (
              <p>Nobody in this tenant has given an app a grant.</p>
            ) : (
              <div className="people-table" role="region" aria-labelledby="grants-caption" tabIndex={0}>
                <table>
                  <caption id="grants-caption">Live grants</caption>
                  <thead>
                    <tr>
                      <th scope="col">Whose</th>
                      <th scope="col">App</th>
                      <th scope="col">Ops</th>
                      <th scope="col">Ends</th>
                    </tr>
                  </thead>
                  <tbody>
                    {people.grants.map((g) => (
                      <tr key={g.grant}>
                        <th scope="row">
                          <span className="person-name">{g.member ? named(g.member) : g.principal}</span>
                          <span className="mono small muted person-id">{g.grant}</span>
                        </th>
                        <td data-label="App">{g.client ?? (g.kind === 'share' ? 'a share' : 'unnamed')}</td>
                        <td data-label="Ops" className="mono">
                          {g.ops.join(', ')}
                        </td>
                        <td data-label="Ends">{day(g.expires)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </section>

          <section aria-labelledby="callers-h">
            <h2 id="callers-h">Who can call an op</h2>
            <form
              className="people-form"
              onSubmit={(e) => {
                e.preventDefault();
                const o = op.trim();
                if (!o) return;
                void act(async () => {
                  setCallers(await atDeployment(() => callersOf(tenant, o)));
                });
              }}
            >
              <div className="field">
                <label htmlFor="op-name">Op name</label>
                <input id="op-name" value={op} placeholder="notes.list" onChange={(e) => setOp(e.target.value)} />
              </div>
              <button type="submit" disabled={busy}>
                Find callers
              </button>
            </form>
            {callers && (
              <div className="panel" role="region" aria-label={`Who can call ${callers.op}`}>
                <h3>
                  Who can call <span className="mono">{callers.op}</span>
                </h3>
                {callers.callers.length === 0 ? (
                  <p>Nobody.</p>
                ) : (
                  <ul className="via-list">
                    {callers.callers.map((v, i) => (
                      <li key={i}>
                        {people.members.find((m) => m.principal === v.principal)?.name ?? v.principal} {viaWords(v)}
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            )}
          </section>

          {lent && (
            <>
              <Invitations key={`i-${tenant}`} {...lent} />
              <Access key={`a-${tenant}`} {...lent} />
              <AdmissionPanel key={`j-${tenant}`} {...lent} />
              <Recovery key={`r-${tenant}`} {...lent} />
            </>
          )}
        </>
      )}

      <dialog
        ref={actionDialog}
        aria-labelledby="people-action-title"
        onClose={() => setOpen(null)}
      >
        {open?.kind === 'role' && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void change(
                () => setRole(tenant, open.m.member, newRole),
                `${who(open.m)} is now ${newRole}.`,
              );
            }}
          >
            <h2 id="people-action-title">Role of {who(open.m)}</h2>
            <div className="field">
              <label htmlFor="new-role">Role</label>
              <select id="new-role" value={newRole} onChange={(e) => setNewRole(e.target.value)}>
                {ROLES.map((r) => (
                  <option key={r} value={r}>
                    {r}
                  </option>
                ))}
              </select>
            </div>
            <div className="actions">
              <button type="submit" className="primary" disabled={busy}>
                Save role
              </button>
              <button type="button" className="link" onClick={() => setOpen(null)}>
                Cancel
              </button>
            </div>
          </form>
        )}
        {open?.kind === 'suspend' && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void change(() => suspend(tenant, open.m.member), `${who(open.m)} is suspended.`);
            }}
          >
            <h2 id="people-action-title">Suspend {who(open.m)}</h2>
            <p>
              {who(open.m)} cannot get into {tenantName} until you reinstate them. They stay a member, with their role
              and groups, and what they made stays.
            </p>
            <div className="actions">
              <button type="submit" className="danger" disabled={busy}>
                Yes, suspend
              </button>
              <button type="button" className="link" onClick={() => setOpen(null)}>
                Cancel
              </button>
            </div>
          </form>
        )}
        {open?.kind === 'delete-group' && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void change(() => deleteGroup(tenant, open.group), `Deleted the group ${open.name}.`);
            }}
          >
            <h2 id="people-action-title">Delete the group {open.name}</h2>
            <p>
              Its members stay in {tenantName}; they lose only what the group let them call.
            </p>
            <div className="actions">
              <button type="submit" className="danger" disabled={busy}>
                Yes, delete
              </button>
              <button type="button" className="link" onClick={() => setOpen(null)}>
                Cancel
              </button>
            </div>
          </form>
        )}
        {open?.kind === 'remove' && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void change(() => removeMember(tenant, open.m.member), `Removed ${who(open.m)}.`);
            }}
          >
            <h2 id="people-action-title">Remove {who(open.m)}</h2>
            <p>
              {who(open.m)} loses every way into {tenantName} at once. What they made stays. A member others' rows
              still name cannot be removed; the deployment says so.
            </p>
            <div className="actions">
              <button type="submit" className="danger" disabled={busy}>
                Yes, remove
              </button>
              <button type="button" className="link" onClick={() => setOpen(null)}>
                Cancel
              </button>
            </div>
          </form>
        )}
      </dialog>
      {prompt}
    </section>
  );
}
