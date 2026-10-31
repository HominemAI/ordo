import { useState, type FormEvent } from 'react';
import {
  admission as readAdmission,
  approveRecovery,
  invitations as readInvitations,
  profile as readProfile,
  recoverMember,
  resetOpAccess,
  revokeInvitation,
  ROLES,
  setAdmission,
  setMemberAccess,
  setOpAccess,
  setRecoveryPolicy,
  type Admission,
  type Invitation,
  type Member,
  type OpAccess,
  type PeopleOf,
  type RecoveryChange,
} from '../people';

/** What the People screen lends these sections. */
export interface Lent {
  tenant: string;
  people: PeopleOf;
  busy: boolean;
  /** Run one call, showing its error the screen's way. */
  act: (f: () => Promise<void>) => Promise<void>;
  /** Run a call at the deployment, asking for the touch there when needed. */
  atDeployment: <T>(f: () => Promise<T>) => Promise<T>;
  /** Say what was done, in the screen's status line. */
  said: (words: string) => void;
  /** Read the members and groups again. */
  reload: () => Promise<void>;
}

function who(m: Member): string {
  return m.name || m.member;
}

function day(secs: number): string {
  return new Date(secs * 1000).toLocaleDateString(undefined, { dateStyle: 'medium' });
}

/** A yes, a no, or leave it as it is. */
type Tri = '' | 'on' | 'off';
const tri = (t: Tri): boolean | undefined => (t === '' ? undefined : t === 'on');

function TriField(props: { id: string; label: string; value: Tri; onChange: (t: Tri) => void }) {
  return (
    <div className="field">
      <label htmlFor={props.id}>{props.label}</label>
      <select id={props.id} value={props.value} onChange={(e) => props.onChange(e.target.value as Tri)}>
        <option value="">Leave as it is</option>
        <option value="on">On</option>
        <option value="off">Off</option>
      </select>
    </div>
  );
}

/** Open invitations, by id and note, each revocable. Never the ticket. */
export function Invitations(p: Lent) {
  const [list, setList] = useState<Invitation[] | null>(null);
  const load = async () => setList((await p.atDeployment(() => readInvitations(p.tenant))).invitations);
  return (
    <section aria-labelledby="invitations-h">
      <h2 id="invitations-h">Invitations</h2>
      <button type="button" disabled={p.busy} onClick={() => void p.act(load)}>
        Show invitations
      </button>
      {list && list.length === 0 && <p>No open invitations.</p>}
      {list && list.length > 0 && (
        <div className="people-table" role="region" aria-labelledby="invitations-caption" tabIndex={0}>
          <table>
            <caption id="invitations-caption">{list.length === 1 ? '1 invitation' : `${list.length} invitations`}</caption>
            <thead>
              <tr>
                <th scope="col">Note</th>
                <th scope="col">Role</th>
                <th scope="col">Status</th>
                <th scope="col">Ends</th>
                <th scope="col">Actions</th>
              </tr>
            </thead>
            <tbody>
              {list.map((i) => (
                <tr key={i.id}>
                  <th scope="row">
                    <span className="person-name">{i.label || 'No note'}</span>
                    <span className="mono small muted person-id">{i.id}</span>
                  </th>
                  <td data-label="Role">{i.role || "the tenant's default"}</td>
                  <td data-label="Status">{i.status}</td>
                  <td data-label="Ends">{day(i.expiresAt)}</td>
                  <td data-label="Actions">
                    {i.status === 'open' && (
                      <button
                        type="button"
                        className="danger"
                        disabled={p.busy}
                        aria-label={`Revoke the invitation ${i.label || i.id}`}
                        onClick={() =>
                          void p.act(async () => {
                            await p.atDeployment(() => revokeInvitation(p.tenant, i.id));
                            p.said(`Revoked the invitation ${i.label || i.id}. It can no longer be used.`);
                            await load();
                          })
                        }
                      >
                        Revoke
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

/** Who may call each op in this tenant, and one member's own allowance. */
export function Access(p: Lent) {
  const [ops, setOps] = useState<OpAccess[] | null>(null);
  const [op, setOp] = useState('');
  const [roles, setRoles] = useState<string[]>([]);
  const [groups, setGroups] = useState<string[]>([]);
  const [member, setMember] = useState('');
  const [memberOp, setMemberOp] = useState('');
  const [effect, setEffect] = useState<'allow' | 'deny' | 'default'>('allow');

  const load = async () => {
    const r = (await p.atDeployment(() => readProfile(p.tenant))).ops;
    setOps(r);
    const first = r.find((o) => o.op === op) ?? r[0];
    if (first) pick(first);
    setMemberOp((m) => m || r[0]?.op || '');
    setMember((m) => m || p.people.members[0]?.member || '');
  };
  const pick = (o: OpAccess) => {
    setOp(o.op);
    setRoles(o.roles);
    setGroups(o.groups);
  };
  const toggle = (list: string[], x: string) => (list.includes(x) ? list.filter((y) => y !== x) : [...list, x]);
  const named = (id: string) => {
    const m = p.people.members.find((x) => x.member === id);
    return m ? who(m) : id;
  };
  const groupName = (id: string) => p.people.groups.find((g) => g.group === id)?.name ?? id;

  const save = (e: FormEvent) => {
    e.preventDefault();
    void p.act(async () => {
      await p.atDeployment(() => setOpAccess(p.tenant, op, roles, groups));
      p.said(`Saved who may call ${op}.`);
      await load();
    });
  };

  return (
    <section aria-labelledby="access-h">
      <h2 id="access-h">Access to ops</h2>
      <p className="muted small">
        Which roles and groups may call each of the app's ops in this tenant. A change you make here replaces the app's
        default for this tenant until you put it back.
      </p>
      <button type="button" disabled={p.busy} onClick={() => void p.act(load)}>
        Show access
      </button>
      {ops && (
        <>
          <div className="people-table" role="region" aria-labelledby="access-caption" tabIndex={0}>
            <table>
              <caption id="access-caption">Access by op</caption>
              <thead>
                <tr>
                  <th scope="col">Op</th>
                  <th scope="col">Roles</th>
                  <th scope="col">Groups</th>
                  <th scope="col">Only these people</th>
                  <th scope="col">Set here</th>
                </tr>
              </thead>
              <tbody>
                {ops.map((o) => (
                  <tr key={o.op}>
                    <th scope="row" className="mono">
                      {o.op}
                    </th>
                    <td data-label="Roles">{o.roles.join(', ') || 'none'}</td>
                    <td data-label="Groups">{o.groups.map(groupName).join(', ') || 'none'}</td>
                    <td data-label="Only these people">
                      {[...o.allow.map((m) => `${named(m)} allowed`), ...o.deny.map((m) => `${named(m)} denied`)].join(
                        ', ',
                      ) || 'none'}
                    </td>
                    <td data-label="Set here">{o.changed ? 'yes' : "no, the app's default"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <form className="people-form" onSubmit={save}>
            <div className="field">
              <label htmlFor="access-op">Op to change</label>
              <select
                id="access-op"
                value={op}
                onChange={(e) => {
                  const o = ops.find((x) => x.op === e.target.value);
                  if (o) pick(o);
                }}
              >
                {ops.map((o) => (
                  <option key={o.op} value={o.op}>
                    {o.op}
                  </option>
                ))}
              </select>
            </div>
            <fieldset className="field">
              <legend>Roles that may call it</legend>
              {ROLES.map((r) => (
                <label key={r} className="check">
                  <input type="checkbox" checked={roles.includes(r)} onChange={() => setRoles(toggle(roles, r))} /> {r}
                </label>
              ))}
            </fieldset>
            {p.people.groups.length > 0 && (
              <fieldset className="field">
                <legend>Groups that may call it</legend>
                {p.people.groups.map((g) => (
                  <label key={g.group} className="check">
                    <input
                      type="checkbox"
                      checked={groups.includes(g.group)}
                      onChange={() => setGroups(toggle(groups, g.group))}
                    />{' '}
                    {g.name}
                  </label>
                ))}
              </fieldset>
            )}
            <button type="submit" className="primary" disabled={p.busy || !op}>
              Save access
            </button>
            <button
              type="button"
              disabled={p.busy || !op}
              onClick={() =>
                void p.act(async () => {
                  await p.atDeployment(() => resetOpAccess(p.tenant, op));
                  p.said(`${op} is back to the app's default.`);
                  await load();
                })
              }
            >
              Back to the app's default
            </button>
          </form>

          <h3>One person's own access</h3>
          <form
            className="people-form"
            onSubmit={(e) => {
              e.preventDefault();
              void p.act(async () => {
                await p.atDeployment(() => setMemberAccess(p.tenant, member, memberOp, effect));
                p.said(
                  effect === 'default'
                    ? `${named(member)} has no access of their own on ${memberOp} now.`
                    : `${named(member)} is ${effect === 'allow' ? 'allowed' : 'denied'} ${memberOp}.`,
                );
                await load();
              });
            }}
          >
            <div className="field">
              <label htmlFor="own-person">Person to allow or deny</label>
              <select id="own-person" value={member} onChange={(e) => setMember(e.target.value)}>
                {p.people.members.map((m) => (
                  <option key={m.member} value={m.member}>
                    {who(m)}
                  </option>
                ))}
              </select>
            </div>
            <div className="field">
              <label htmlFor="own-op">Op for this person</label>
              <select id="own-op" value={memberOp} onChange={(e) => setMemberOp(e.target.value)}>
                {ops.map((o) => (
                  <option key={o.op} value={o.op}>
                    {o.op}
                  </option>
                ))}
              </select>
            </div>
            <div className="field">
              <label htmlFor="own-effect">Access</label>
              <select
                id="own-effect"
                value={effect}
                onChange={(e) => setEffect(e.target.value as 'allow' | 'deny' | 'default')}
              >
                <option value="allow">Allow, whatever their role</option>
                <option value="deny">Deny, whatever their role</option>
                <option value="default">As their role and groups say</option>
              </select>
            </div>
            <button type="submit" disabled={p.busy || !member || !memberOp}>
              Save person's access
            </button>
          </form>
        </>
      )}
    </section>
  );
}

/** How a person who is not a member gets in: the four ways. */
export function AdmissionPanel(p: Lent) {
  const [now, setNow] = useState<Admission | null>(null);
  const [open, setOpen] = useState(false);
  const [role, setRole] = useState('viewer');
  const [providers, setProviders] = useState('');
  const [firstComer, setFirstComer] = useState(false);
  const show = (a: Admission) => {
    setNow(a);
    setOpen(a.open);
    setRole(a.role);
    setProviders(a.providers);
    setFirstComer(a.firstComer);
  };
  return (
    <section aria-labelledby="admission-h">
      <h2 id="admission-h">Who may join</h2>
      <p className="muted small">
        The ways a person who is not yet a member is let in. An invitation always works. The deployment sets how far a
        tenant may open, and refuses a way above that.
      </p>
      <button
        type="button"
        disabled={p.busy}
        onClick={() => void p.act(async () => show((await p.atDeployment(() => readAdmission(p.tenant))).admission))}
      >
        Show who may join
      </button>
      {now && (
        <form
          className="people-form"
          onSubmit={(e) => {
            e.preventDefault();
            void p.act(async () => {
              const r = await p.atDeployment(() =>
                setAdmission(p.tenant, { open, role, providers: providers.trim(), firstComer }),
              );
              show(r.admission);
              p.said('Saved who may join.');
            });
          }}
        >
          <label className="check">
            <input type="checkbox" checked={open} onChange={(e) => setOpen(e.target.checked)} /> Anyone who signs in
            may join
          </label>
          <div className="field">
            <label htmlFor="admission-role">Role a new person gets</label>
            <select id="admission-role" value={role} onChange={(e) => setRole(e.target.value)}>
              {ROLES.map((r) => (
                <option key={r} value={r}>
                  {r}
                </option>
              ))}
            </select>
          </div>
          <div className="field">
            <label htmlFor="admission-providers">Outside sign-in rules (optional)</label>
            <input
              id="admission-providers"
              value={providers}
              maxLength={2048}
              placeholder="google:example.com=editor"
              onChange={(e) => setProviders(e.target.value)}
            />
          </div>
          <label className="check">
            <input type="checkbox" checked={firstComer} onChange={(e) => setFirstComer(e.target.checked)} /> The first
            person in becomes its owner, while it has no members
          </label>
          <button type="submit" className="primary" disabled={p.busy}>
            Save who may join
          </button>
          <p className="muted small">The deployment allows: {now.ceiling || 'invitations only'}.</p>
        </form>
      )}
    </section>
  );
}

/** A member's recovery, its approval, and the tenant's recovery settings. */
export function Recovery(p: Lent) {
  const [member, setMember] = useState('');
  const [caseId, setCaseId] = useState('');
  const [ticket, setTicket] = useState<string | null>(null);
  const [hold, setHold] = useState<Tri>('');
  const [path, setPath] = useState<Tri>('');
  const [codes, setCodes] = useState<Tri>('');
  const [managers, setManagers] = useState<Tri>('');
  const [delay, setDelay] = useState('');
  const [approvals, setApprovals] = useState('');
  const chosen = member || p.people.members[0]?.member || '';

  const recover = (e: FormEvent) => {
    e.preventDefault();
    setTicket(null);
    void p.act(async () => {
      const r = (await p.atDeployment(() => recoverMember(p.tenant, chosen, caseId.trim()))).recovery;
      setCaseId(r.case);
      if (r.ticket) {
        setTicket(r.ticket);
        p.said('The recovery is ready. Give the person the ticket below.');
      } else {
        p.said(
          `Recovery case ${r.case} is open. It needs ${r.approvalsRequired ?? 1} approvals${
            r.maturesAt ? ` and is ready from ${day(r.maturesAt)}` : ''
          }; then collect the ticket here.`,
        );
      }
    });
  };

  const saveSettings = (e: FormEvent) => {
    e.preventDefault();
    const change: RecoveryChange = {
      hold: tri(hold),
      pathEnabled: tri(path),
      codesEnabled: tri(codes),
      managersMayRecover: tri(managers),
      ...(delay.trim() ? { delaySecs: Number(delay) * 3600 } : {}),
      ...(approvals.trim() ? { approvals: Number(approvals) } : {}),
    };
    void p.act(async () => {
      const r = (await p.atDeployment(() => setRecoveryPolicy(p.tenant, change))).policy;
      p.said(
        `Saved. Recovery waits ${r.delaySecs / 3600} hours, needs ${r.approvals} approvals, and is ${
          r.pathEnabled ? 'on' : 'off'
        } for this tenant's administrators.`,
      );
    });
  };

  return (
    <section aria-labelledby="recovery-h">
      <h2 id="recovery-h">Recovery</h2>
      <p className="muted small">
        A person this tenant created who lost every passkey can be given a way back in. Not an owner, and not a person
        who belongs to another tenant too: the deployment refuses those.
      </p>
      <form className="people-form" onSubmit={recover}>
        <div className="field">
          <label htmlFor="recover-person">Person to recover</label>
          <select id="recover-person" value={chosen} onChange={(e) => setMember(e.target.value)}>
            {p.people.members.map((m) => (
              <option key={m.member} value={m.member}>
                {who(m)}
              </option>
            ))}
          </select>
        </div>
        <div className="field">
          <label htmlFor="recover-case">Case (to collect a ticket)</label>
          <input id="recover-case" value={caseId} onChange={(e) => setCaseId(e.target.value)} />
        </div>
        <button type="submit" disabled={p.busy || !chosen}>
          {caseId.trim() ? 'Collect the ticket' : 'Start a recovery'}
        </button>
        <button
          type="button"
          disabled={p.busy || !chosen || !caseId.trim()}
          onClick={() =>
            void p.act(async () => {
              const r = (await p.atDeployment(() => approveRecovery(p.tenant, chosen, caseId.trim()))).approval;
              p.said(`Approved: ${r.approvals} of ${r.approvalsRequired}.`);
            })
          }
        >
          Approve this case
        </button>
      </form>
      {ticket && (
        <div className="panel">
          <p>Give this to the person. It is shown once and Ordo does not send it.</p>
          <p className="mono ticket" aria-label="Recovery ticket">
            {ticket}
          </p>
        </div>
      )}

      <h3>This tenant's recovery settings</h3>
      <p className="muted small">A tenant may only be stricter than the deployment; the deployment says so if not.</p>
      <form className="people-form" onSubmit={saveSettings}>
        <TriField id="rec-path" label="Administrators may recover people" value={path} onChange={setPath} />
        <TriField id="rec-codes" label="People may use recovery codes" value={codes} onChange={setCodes} />
        <TriField id="rec-hold" label="Hold a recovered person until an owner lets them in" value={hold} onChange={setHold} />
        <TriField id="rec-managers" label="Managers may recover people" value={managers} onChange={setManagers} />
        <div className="field">
          <label htmlFor="rec-delay">Wait, in hours</label>
          <input id="rec-delay" inputMode="numeric" pattern="[0-9]*" value={delay} onChange={(e) => setDelay(e.target.value)} />
        </div>
        <div className="field">
          <label htmlFor="rec-approvals">Approvals needed</label>
          <input
            id="rec-approvals"
            inputMode="numeric"
            pattern="[0-9]*"
            value={approvals}
            onChange={(e) => setApprovals(e.target.value)}
          />
        </div>
        <button type="submit" className="primary" disabled={p.busy}>
          Save recovery settings
        </button>
      </form>
    </section>
  );
}
