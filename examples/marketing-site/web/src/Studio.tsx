import { useEffect, useState, type FormEvent, type ReactNode } from 'react';
import { OrdoProvider, useOpQuery, type DescriptorClient } from '@hominemai/ordo-react';
import { registerPasskey, signInWithPasskey } from '@hominemai/ordo-sdk/auth/passkey.js';
import { downloadFile, saveBlob, uploadFile } from '@hominemai/ordo-sdk/core/files.js';
import type { Params, Row } from '@hominemai/ordo-sdk/core/client.js';
import { DOOR } from './config.js';
import { Session, sessionStore } from './session.js';
import { commit, message, studioClient } from './studio-client.js';

const text = (value: unknown) => value == null ? '' : String(value);
const options = () => ({ key: 'lumen.studio', baseUrl: DOOR, space: 'system', store: sessionStore() });
type Context = { client: DescriptorClient; session: Session };
type Field = { name: string; label: string; kind?: 'long' | 'number' | 'money' | 'date' | 'datetime-local' | 'list' | 'checkbox'; choices?: string[]; optional?: boolean };
const field = (name: string, label: string, kind?: Field['kind']): Field => ({ name, label, ...(kind ? { kind } : {}) });
const productFields = [field('slug', 'URL name'), field('name', 'Name'), field('tagline', 'Tagline'), field('description', 'Description', 'long'), field('price', 'Price', 'money'), field('features', 'Features (one per line)', 'list'), field('rank', 'Display order', 'number'), field('internalNote', 'Private staff note', 'long')];
const planFields = [field('name', 'Name'), field('tagline', 'Tagline'), field('price', 'Price', 'money'), field('period', 'Billing period'), field('features', 'Features (one per line)', 'list'), field('highlighted', 'Recommended plan', 'checkbox'), field('rank', 'Display order', 'number')];
const reviewFields = [field('author', 'Author'), field('role', 'Role'), field('company', 'Company'), field('quote', 'Quote', 'long'), field('stars', 'Stars', 'number'), field('rank', 'Display order', 'number')];
const campaignFields: Field[] = [field('title', 'Title'), field('brief', 'Brief', 'long'), field('tags', 'Tags (one per line)', 'list'), field('budget', 'Budget', 'money'), { ...field('due', 'Due date', 'date'), optional: true }];

export function Studio() {
  const [session, setSession] = useState(() => Session.restore(options()));
  const [client, setClient] = useState<DescriptorClient | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [retry, setRetry] = useState(0);
  const [invitation, setInvitation] = useState('');
  useEffect(() => session?.onEnd(() => { setSession(null); setClient(null); }), [session]);
  useEffect(() => {
    let current = true;
    setClient(null);
    if (session) void studioClient(session).then(c => { if (current) setClient(c); }, e => { if (current) setError(message(e)); });
    return () => { current = false; };
  }, [session, retry]);
  async function enter(register: boolean, display = '') {
    setBusy(true); setError('');
    try {
      const endpoint = { baseUrl: DOOR };
      const result = register ? await registerPasskey(endpoint, { display, space: 'system', ...(invitation ? { invitation } : {}) }) : await signInWithPasskey(endpoint, { space: 'system', ...(invitation ? { invitation } : {}) });
      setSession(Session.from(options(), result));
    } catch (e) { setError(message(e)); } finally { setBusy(false); }
  }
  return <main className="studio">
    <header className="studio-header"><div><a href="/">Lumen Studio</a><h1>Studio workspace</h1></div>{session && <button disabled={busy} onClick={async () => { setBusy(true); try { await session.signOut(); } catch (e) { setError(message(e)); } finally { setBusy(false); } }}>Sign out</button>}</header>
    {error && <p role="alert">{error}</p>}
    {!session ? <section className="card"><h2>Sign in to the studio</h2><p>Use your passkey. A studio owner must give your account access before you can manage this site.</p><div className="form"><label htmlFor="invitation">Invitation ticket (optional)</label><input id="invitation" value={invitation} onChange={e => setInvitation(e.target.value)} autoComplete="off" /></div><button disabled={busy} onClick={() => void enter(false)}>Sign in with a passkey</button><details><summary>Create a passkey</summary><form className="form" onSubmit={e => { e.preventDefault(); void enter(true, text(new FormData(e.currentTarget).get('display'))); }}><label htmlFor="display">Your name (required)</label><input id="display" name="display" autoComplete="name" required /><button disabled={busy}>Create account</button></form></details></section>
      : !client ? <p role="status">{error ? <button onClick={() => { setError(''); setRetry(x => x + 1); }}>Retry workspace</button> : 'Opening workspace…'}</p>
      : !client.iface.op('inbox') ? <section><h2>Studio access needed</h2><p>Your account is signed in. Ask a deployment owner to grant it access to this studio.</p><button onClick={() => setRetry(x => x + 1)}>Check access again</button></section>
      : <OrdoProvider client={client} live stepUp={() => session.stepUp()}><Workspace client={client} session={session} /></OrdoProvider>}
  </main>;
}

function Workspace(ctx: Context) {
  const [tab, setTab] = useState('Inbox');
  const [error, setError] = useState('');
  const [exporting, setExporting] = useState(false);
  return <>
    <nav aria-label="Workspace"><div className="toolbar">{['Inbox', 'Products', 'Plans', 'Reviews', 'Campaigns', 'Team'].map(name => <button key={name} aria-current={tab === name ? 'page' : undefined} onClick={() => setTab(name)}>{name}</button>)}<button disabled={exporting} onClick={async () => { setExporting(true); setError(''); try { await ctx.session.stepUp(); const file = await ctx.client.downloadOwn(); saveBlob(file.blob, file.name); } catch(e) { setError(message(e)); } finally { setExporting(false); } }}>Download studio data</button></div></nav>
    {error && <p role="alert">{error}</p>}
    {tab === 'Inbox' && <Inbox {...ctx} />}
    {tab === 'Products' && <Records {...ctx} title="Products" read="studioProducts" create="addProduct" edit="editProduct" remove="deleteProduct" fields={productFields} />}
    {tab === 'Plans' && <Records {...ctx} title="Plans" read="studioPlans" create="addPlan" edit="editPlan" remove="deletePlan" fields={planFields} />}
    {tab === 'Reviews' && <Records {...ctx} title="Reviews" read="studioReviews" create="addTestimonial" edit="editReview" remove="deleteReview" fields={reviewFields} />}
    {tab === 'Campaigns' && <Campaigns {...ctx} />}
    {tab === 'Team' && <Team {...ctx} />}
  </>;
}

function QueryStatus({ loading, refusal }: { loading: boolean; refusal: { message: string } | null }) {
  return refusal ? <p role="alert">{refusal.message}</p> : loading ? <p role="status">Loading…</p> : null;
}
function Table({ title, columns, rows, actions }: { title: string; columns: string[]; rows: readonly Row[]; actions?: (row: Row) => ReactNode }) {
  return <div className="table-wrapper" tabIndex={0} role="region" aria-label={title}><table><caption>{title}</caption><thead><tr>{columns.map(c => <th scope="col" key={c}>{c}</th>)}{actions && <th scope="col">Actions</th>}</tr></thead><tbody>{rows.map((row, index) => <tr key={text(row['id']) || index}>{columns.map((c, i) => i === 0 ? <th scope="row" key={c}>{text(row[c])}</th> : <td key={c}>{text(row[c])}</td>)}{actions && <td><div className="toolbar">{actions(row)}</div></td>}</tr>)}</tbody></table>{rows.length === 0 && <p>No records to show.</p>}</div>;
}
function Editor({ ctx, op, fields, initial = {}, onDone, onCancel, title }: { ctx: Context; op: string; fields: Field[]; initial?: Row; onDone: () => void; onCancel: () => void; title: string }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  async function submit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault(); setBusy(true); setError('');
    const data = new FormData(e.currentTarget);
    const params: Record<string, Params[string]> = {};
    if (initial['id']) { params['id'] = text(initial['id']); params['rev'] = Number(initial['rev']); }
    for (const f of fields) {
      const value = text(data.get(f.name));
      params[f.name] = f.kind === 'checkbox' ? data.has(f.name) : f.kind === 'number' ? Number(value) : f.kind === 'list' ? value.split('\n').map(s => s.trim()).filter(Boolean) : f.optional && !value ? null : f.kind === 'datetime-local' ? new Date(value).toISOString() : value;
    }
    if (op === 'createCampaign') params['created'] = new Date().toISOString();
    try { await commit(ctx.client, ctx.session, op, params); onDone(); } catch(e) { setError(message(e)); } finally { setBusy(false); }
  }
  return <section className="card editor"><h3 id="editor-title">{title}</h3><form className="form" aria-labelledby="editor-title" onSubmit={submit}>{fields.map(f => {
    const id = `edit-${f.name}`; const value = initial[f.name] ?? (f.kind === 'number' ? 100 : f.kind === 'money' ? '0.00' : '');
    const required = !f.optional && !['long', 'list', 'checkbox'].includes(f.kind ?? '');
    return <div className="field" key={f.name}>{f.kind !== 'checkbox' && <label htmlFor={id}>{f.label}{required ? ' (required)' : ''}</label>}{f.choices ? <select id={id} name={f.name} defaultValue={text(value)}>{f.choices.map(v => <option key={v}>{v}</option>)}</select> : f.kind === 'long' || f.kind === 'list' ? <textarea id={id} name={f.name} rows={4} defaultValue={Array.isArray(value) ? value.join('\n') : text(value)} /> : <input id={id} name={f.name} type={f.kind === 'checkbox' ? 'checkbox' : f.kind === 'money' || f.kind === 'number' ? 'number' : f.kind ?? 'text'} step={f.kind === 'money' ? '.01' : undefined} min={f.kind === 'money' ? '0' : undefined} defaultChecked={f.kind === 'checkbox' ? value === true : undefined} defaultValue={f.kind === 'checkbox' ? undefined : text(value)} required={required} />}{f.kind === 'checkbox' && <label htmlFor={id}>{f.label}</label>}</div>;
  })}<div className="toolbar"><button disabled={busy}>Save changes</button><button type="button" disabled={busy} onClick={onCancel}>Cancel</button></div>{error && <p role="alert">{error}</p>}</form></section>;
}
function Remove({ ctx, op, row, done }: { ctx: Context; op: string; row: Row; done: () => void }) {
  const [confirm, setConfirm] = useState(false); const [busy, setBusy] = useState(false); const [error, setError] = useState('');
  return <>{!confirm ? <button onClick={() => setConfirm(true)}>Delete</button> : <><span>Delete permanently?</span><button disabled={busy} onClick={async () => { setBusy(true); try { await commit(ctx.client, ctx.session, op, { id: text(row['id']) }); done(); } catch(e) { setError(message(e)); } finally { setBusy(false); } }}>Confirm delete</button><button disabled={busy} onClick={() => setConfirm(false)}>Keep record</button></>}{error && <span role="alert">{error}</span>}</>;
}
function Records(props: Context & { title: string; read: string; create: string; edit: string; remove: string; fields: Field[] }) {
  const q = useOpQuery(props.read); const [editing, setEditing] = useState<Row | null>(null);
  return <section><h2>{props.title}</h2><p>Saved changes appear on the public site. Private staff notes stay in this workspace.</p><button onClick={() => setEditing({})}>Add {props.title.toLowerCase().replace(/s$/, '')}</button><QueryStatus {...q} />{editing && <Editor key={text(editing['id']) || 'new'} ctx={props} title={editing['id'] ? `Edit ${props.title.toLowerCase()}` : `New ${props.title.toLowerCase()}`} op={editing['id'] ? props.edit : props.create} initial={editing} fields={props.fields.filter(f => !(editing['id'] && f.name === 'slug'))} onCancel={() => setEditing(null)} onDone={() => { setEditing(null); q.refetch(); }} />}<Table title={props.title} columns={props.title === 'Reviews' ? ['author', 'quote', 'stars'] : ['name', 'tagline', 'price']} rows={q.rows} actions={r => <><button onClick={() => setEditing(r)}>Edit</button><Remove ctx={props} op={props.remove} row={r} done={q.refetch} /></>} /></section>;
}
function Inbox(ctx: Context) {
  const q = useOpQuery('inbox'); const report = useOpQuery('enquiryReport'); const [query, setQuery] = useState(''); const [editing, setEditing] = useState<Row | null>(null);
  const matches = q.rows.filter(r => ['name', 'email', 'company', 'message', 'status'].some(k => text(r[k]).toLowerCase().includes(query.toLowerCase())));
  return <section><h2>Enquiry inbox</h2><p>Follow up, keep private notes, and record the outcome.</p><QueryStatus {...report} /><p>{report.rows.map(r => `${text(r['status'])}: ${text(r['total'])}`).join(' · ')}</p><form role="search" onSubmit={e => { e.preventDefault(); setQuery(text(new FormData(e.currentTarget).get('q'))); }} className="toolbar"><label htmlFor="inbox-search">Search enquiries</label><input id="inbox-search" name="q" type="search" /><button>Search</button></form><QueryStatus {...q} />{editing && <><blockquote>{text(editing['message'])}</blockquote><Editor key={text(editing['id'])} ctx={ctx} title={`Follow up with ${text(editing['name'])}`} op="triageEnquiry" initial={editing} fields={[{ name: 'status', label: 'Status', choices: ['new', 'responding', 'won', 'closed'] }, field('notes', 'Private notes', 'long')]} onCancel={() => setEditing(null)} onDone={() => { setEditing(null); q.refetch(); report.refetch(); }} /></>}<Table title="Enquiries" columns={['name', 'email', 'company', 'status']} rows={matches} actions={r => <><button onClick={() => setEditing(r)}>Follow up</button><Remove ctx={ctx} op="deleteEnquiry" row={r} done={() => { q.refetch(); report.refetch(); }} /></>} /></section>;
}
function Campaigns(ctx: Context) {
  const [tag, setTag] = useState('');
  const q = useOpQuery(tag ? 'campaignsTagged' : 'campaigns', tag ? { params: { tag } } : {});
  const report = useOpQuery('campaignReport');
  const [editing, setEditing] = useState<Row | null>(null);
  const [selected, setSelected] = useState<Row | null>(null);
  return <section><h2>Campaigns</h2><p>Draft briefs, set budgets and due dates, and share work with your team. Publishing a campaign records its approval; use Products, Plans or Reviews to change the public site.</p><QueryStatus {...report} /><p>{report.rows.map(r => `${text(r['status'])}: ${text(r['total'])}`).join(' · ')}</p><div className="toolbar"><button onClick={() => setEditing({})}>New campaign</button><form role="search" onSubmit={e => { e.preventDefault(); setTag(text(new FormData(e.currentTarget).get('tag'))); }}><label htmlFor="tag">Filter by tag</label><input id="tag" name="tag" type="search" /><button>Filter campaigns</button></form></div><QueryStatus {...q} />{editing && <Editor key={text(editing['id']) || 'new'} ctx={ctx} title={editing['id'] ? 'Edit campaign' : 'New campaign'} op={editing['id'] ? 'editCampaign' : 'createCampaign'} fields={editing['id'] ? [...campaignFields, { name: 'status', label: 'Status', choices: ['draft', 'review', 'published', 'archived'] }] : campaignFields} initial={editing} onCancel={() => setEditing(null)} onDone={() => { setEditing(null); q.refetch(); report.refetch(); }} />}<Table title="Campaigns" columns={['title', 'status', 'budget', 'due']} rows={q.rows} actions={r => <><button onClick={() => setEditing(r)}>Edit</button><button onClick={() => setSelected(r)}>Files and access</button><Remove ctx={ctx} op="deleteCampaign" row={r} done={() => { if (selected?.['id'] === r['id']) setSelected(null); q.refetch(); report.refetch(); }} /></>} />{selected && <CampaignDetail key={text(selected['id'])} {...ctx} row={selected} close={() => setSelected(null)} />}</section>;
}
function CampaignDetail(ctx: Context & { row: Row; close: () => void }) {
  const campaign = text(ctx.row['id']);
  const files = useOpQuery('assets', { params: { campaign } });
  const access = useOpQuery('campaignAccess', { params: { campaign } });
  const people = useOpQuery('people'); const reminders = useOpQuery('reminders');
  const [busy, setBusy] = useState(false); const [error, setError] = useState(''); const [notice, setNotice] = useState('');
  async function act(work: () => Promise<unknown>) { setBusy(true); setError(''); setNotice(''); try { await work(); files.refetch(); access.refetch(); reminders.refetch(); setNotice('Changes saved.'); } catch(e) { setError(message(e)); } finally { setBusy(false); } }
  return <section className="card campaign-detail"><div className="toolbar"><h3>{text(ctx.row['title'])}</h3><button onClick={ctx.close}>Close details</button></div><p>{text(ctx.row['brief'])}</p>{error && <p role="alert">{error}</p>}{notice && <p role="status">{notice}</p>}
    <h4 id="asset-title">Campaign files</h4><QueryStatus {...files} /><form className="form" aria-labelledby="asset-title" onSubmit={e => { e.preventDefault(); const form = e.currentTarget; const file = new FormData(form).get('file'); if (!(file instanceof File) || !file.size) return; void act(async () => { const result = await commit(ctx.client, ctx.session, 'addAsset', { campaign, name: file.name, kind: file.type, size: file.size }); const id = text(result.rows[0]?.['created']); if (!id) throw new Error('The file record was not created.'); try { await uploadFile(ctx.client, { entity: 'assets', field: 'file', id }, file); } catch(e) { await commit(ctx.client, ctx.session, 'deleteAsset', { id }); throw e; } form.reset(); }); }}><label htmlFor="asset-file">Choose a file (required)</label><input id="asset-file" name="file" type="file" required /><button disabled={busy}>Upload file</button></form><Table title="Files" columns={['name', 'kind', 'size']} rows={files.rows} actions={r => <><button disabled={busy || !r['file']} onClick={() => void act(() => downloadFile(ctx.client, text(r['file']), text(r['name'])))}>Download</button><Remove ctx={ctx} op="deleteAsset" row={r} done={files.refetch} /></>} />
    <h4 id="access-title">Campaign access</h4><QueryStatus {...access} /><QueryStatus {...people} /><form className="form" aria-labelledby="access-title" onSubmit={e => { e.preventDefault(); const data = new FormData(e.currentTarget); void act(() => commit(ctx.client, ctx.session, 'grantCampaignsAccess', { campaign, member: text(data.get('member')), level: text(data.get('level')) })); }}><label htmlFor="access-member">Team member</label><select id="access-member" name="member" required><option value="">Choose a member</option>{people.rows.map(r => <option key={text(r['id'])} value={text(r['id'])}>{text(r['displayName'])}</option>)}</select><label htmlFor="access-level">Access level</label><select id="access-level" name="level"><option value="read">Read</option><option value="write">Edit</option><option value="admin">Manage access</option></select><button disabled={busy}>Grant campaign access</button></form><Table title="People with campaign access" columns={['member', 'level']} rows={access.rows.map(r => ({ ...r, member: text(people.rows.find(p => p['id'] === r['member'])?.['displayName']) || r['member'], memberId: r['member'] }))} actions={r => <button disabled={busy} onClick={() => void act(() => commit(ctx.client, ctx.session, 'unshareCampaign', { campaign, member: text(r['memberId']) }))}>Revoke access</button>} />
    <h4 id="reminder-title">Reminder</h4><QueryStatus {...reminders} />{reminders.rows.filter(r => r['campaign'] === campaign).map(r => <p key={text(r['id'])}>{text(r['due'])} · {r['fired'] ? 'Fired' : 'Scheduled'} <button disabled={busy} onClick={() => void act(() => commit(ctx.client, ctx.session, 'clearReminder', { campaign }))}>Clear reminder</button></p>)}<form className="form" aria-labelledby="reminder-title" onSubmit={e => { e.preventDefault(); const due = new Date(text(new FormData(e.currentTarget).get('due'))).toISOString(); void act(() => commit(ctx.client, ctx.session, 'setReminder', { campaign, due })); }}><label htmlFor="reminder-due">Remind at (your local time)</label><input id="reminder-due" name="due" type="datetime-local" required /><button disabled={busy}>Set reminder</button></form>
  </section>;
}
function Team(ctx: Context) {
  const people = useOpQuery('people'); const invitations = useOpQuery('std.space.invitations', { live: false });
  const [error, setError] = useState(''); const [ticket, setTicket] = useState(''); const [busy, setBusy] = useState(false);
  async function act(name: string, params: Params) { setBusy(true); setError(''); try { const r = await commit(ctx.client, ctx.session, name, params); if (name === 'std.space.invite') setTicket(text(r.rows[0]?.['ticket'])); people.refetch(); invitations.refetch(); } catch(e) { setError(message(e)); } finally { setBusy(false); } }
  return <section><h2>Team access</h2><p>Owners manage roles and invitations. Sensitive changes may ask for a fresh passkey touch.</p>{error && <p role="alert">{error}</p>}<QueryStatus {...people} /><Table title="Studio members" columns={['displayName', 'role', 'status']} rows={people.rows} actions={r => <><form className="toolbar" onSubmit={e => { e.preventDefault(); void act('std.space.setRole', { member: text(r['id']), role: text(new FormData(e.currentTarget).get('role')) }); }}><label htmlFor={`role-${text(r['id'])}`}>Role for {text(r['displayName'])}</label><select id={`role-${text(r['id'])}`} name="role" defaultValue={text(r['role'])}>{['viewer', 'editor', 'manager', 'owner'].map(role => <option key={role}>{role}</option>)}</select><button disabled={busy}>Save role</button></form><button disabled={busy} onClick={() => void act(r['status'] === 'suspended' ? 'std.space.reinstate' : 'std.space.suspend', { member: text(r['id']) })}>{r['status'] === 'suspended' ? 'Reinstate' : 'Suspend'}</button></>} />
    <h3 id="invite-title">Invite a colleague</h3><form className="form" aria-labelledby="invite-title" onSubmit={e => { e.preventDefault(); const data = new FormData(e.currentTarget); void act('std.space.invite', { role: text(data.get('role')), label: text(data.get('label')), expires: '7d' }); }}><label htmlFor="invite-label">Invitation label (required)</label><input id="invite-label" name="label" required /><label htmlFor="invite-role">Role</label><select id="invite-role" name="role"><option>viewer</option><option>editor</option><option>manager</option></select><button disabled={busy}>Create invitation</button></form>{ticket && <section role="status"><p>Invitation created. Share this ticket privately with your colleague; it expires in 7 days.</p><output className="ticket">{ticket}</output><button onClick={() => setTicket('')}>Hide invitation</button></section>}<QueryStatus {...invitations} /><Table title="Invitations" columns={['label', 'role', 'status', 'expiresAt']} rows={invitations.rows} actions={r => <button disabled={busy || r['status'] === 'revoked' || r['status'] === 'accepted'} onClick={() => void act('std.space.revokeInvitation', { id: text(r['id']) })}>Revoke invitation</button>} /></section>;
}
