import { useEffect, useState, type FormEvent, type ReactNode } from 'react';
import { OrdoProvider, useOpQuery, useOpPages, type DescriptorClient } from '@hominemai/ordo-react';
import { registerPasskey, signInWithPasskey } from '@hominemai/ordo-sdk/auth/passkey.js';
import { downloadFile, saveBlob, uploadFile } from '@hominemai/ordo-sdk/core/files.js';
import type { Params, Row } from '@hominemai/ordo-sdk/core/client.js';
import { DOOR } from './config.js';
import { Session, sessionStore } from './session.js';
import { commit, message, studioClient, workspaces, workspaceData, tenantConnections, type WorkspaceChoice } from './studio-client.js';

const columnLabel = (name: string) =>
  ({
    displayName: 'Name',
    expiresAt: 'Expires',
    at: 'Approved at',
    reviewer: 'Approved by',
    kind: 'File type',
    size: 'Bytes',
  })[name as 'displayName'] ?? name.charAt(0).toUpperCase() + name.slice(1);
const text = (value: unknown) => (value == null ? '' : String(value));
const selectedSpace = () => new URLSearchParams(location.search).get('space') || 'system';
const options = () => ({ key: 'lumen.studio', baseUrl: DOOR, space: selectedSpace(), store: sessionStore() });
type Context = { client: DescriptorClient; session: Session };
type Field = {
  name: string;
  label: string;
  kind?: 'long' | 'number' | 'money' | 'date' | 'datetime-local' | 'list' | 'checkbox';
  choices?: string[];
  optional?: boolean;
};
const field = (name: string, label: string, kind?: Field['kind']): Field => ({
  name,
  label,
  ...(kind ? { kind } : {}),
});
const productFields = [
  field('slug', 'URL name'),
  field('name', 'Name'),
  field('tagline', 'Tagline'),
  field('description', 'Description', 'long'),
  field('price', 'Price', 'money'),
  field('features', 'Features (one per line)', 'list'),
  field('rank', 'Display order', 'number'),
  field('internalNote', 'Private staff note', 'long'),
];
const planFields = [
  field('name', 'Name'),
  field('tagline', 'Tagline'),
  field('price', 'Price', 'money'),
  field('period', 'Billing period'),
  field('features', 'Features (one per line)', 'list'),
  field('highlighted', 'Recommended plan', 'checkbox'),
  field('rank', 'Display order', 'number'),
];
const reviewFields = [
  field('author', 'Author'),
  field('role', 'Role'),
  field('company', 'Company'),
  field('quote', 'Quote', 'long'),
  field('stars', 'Stars', 'number'),
  field('rank', 'Display order', 'number'),
];
const campaignFields: Field[] = [
  field('title', 'Title'),
  field('brief', 'Brief', 'long'),
  field('tags', 'Tags (one per line)', 'list'),
  field('budget', 'Budget', 'money'),
  { ...field('due', 'Due date', 'date'), optional: true },
];

export function Studio() {
  const [session, setSession] = useState(() => Session.restore(options()));
  const [client, setClient] = useState<DescriptorClient | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [retry, setRetry] = useState(0);
  const [invitation, setInvitation] = useState('');
  useEffect(
    () =>
      session?.onEnd(() => {
        setSession(null);
        setClient(null);
      }),
    [session],
  );
  useEffect(() => {
    let current = true;
    setClient(null);
    if (session)
      void studioClient(session).then(
        (c) => {
          if (current) setClient(c);
        },
        (e) => {
          if (current) setError(message(e));
        },
      );
    return () => {
      current = false;
    };
  }, [session, retry]);
  async function enter(register: boolean, display = '') {
    setBusy(true);
    setError('');
    try {
      const endpoint = { baseUrl: DOOR };
      const result = register
        ? await registerPasskey(endpoint, { display, space: selectedSpace(), ...(invitation ? { invitation } : {}) })
        : await signInWithPasskey(endpoint, { space: selectedSpace(), ...(invitation ? { invitation } : {}) });
      setSession(Session.from(options(), result));
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <main className="studio">
      <header className="studio-header">
        <div>
          <a href="/">Lumen Studio</a>
          <h1>Studio workspace</h1>
        </div>
        {session && (
          <button
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              try {
                await session.signOut();
              } catch (e) {
                setError(message(e));
              } finally {
                setBusy(false);
              }
            }}
          >
            Sign out
          </button>
        )}
      </header>
      {error && <p role="alert">{error}</p>}
      {!session ? (
        <section className="card">
          <h2>Sign in to the studio</h2>
          <p>Use your passkey. A studio owner must give your account access before you can manage this site.</p>
          <div className="form">
            <label htmlFor="invitation">Invitation ticket (optional)</label>
            <input
              id="invitation"
              value={invitation}
              onChange={(e) => setInvitation(e.target.value)}
              autoComplete="off"
            />
          </div>
          <button disabled={busy} onClick={() => void enter(false)}>
            Sign in with a passkey
          </button>
          <details>
            <summary>Create a passkey</summary>
            <form
              className="form"
              onSubmit={(e) => {
                e.preventDefault();
                void enter(true, text(new FormData(e.currentTarget).get('display')));
              }}
            >
              <label htmlFor="display">Your name (required)</label>
              <input id="display" name="display" autoComplete="name" required />
              <button disabled={busy}>Create account</button>
            </form>
          </details>
        </section>
      ) : !client ? (
        <p role="status">
          {error ? (
            <button
              onClick={() => {
                setError('');
                setRetry((x) => x + 1);
              }}
            >
              Retry workspace
            </button>
          ) : (
            'Opening workspace…'
          )}
        </p>
      ) : !client.iface.op('inbox') ? (
        <section>
          <h2>Studio access needed</h2>
          <p>Your account is signed in. Ask a deployment owner to grant it access to this studio.</p>
          <button onClick={() => setRetry((x) => x + 1)}>Check access again</button>
        </section>
      ) : (
        <OrdoProvider client={client} live stepUp={() => session.stepUp()}>
          <Workspace client={client} session={session} />
        </OrdoProvider>
      )}
    </main>
  );
}

function Workspace(ctx: Context) {
  const [tab, setTab] = useState('Inbox');
  const [error, setError] = useState('');
  const [exporting, setExporting] = useState(false);
  const [choices, setChoices] = useState<WorkspaceChoice[]>([]);
  useEffect(() => {
    let active = true;
    workspaces(ctx.session).then(value => { if (active) setChoices(value); }, e => { if (active) setError(message(e)); });
    return () => { active = false; };
  }, [ctx.session]);
  return (
    <>
      <form className="toolbar" aria-label="Choose workspace" onSubmit={e => {
        e.preventDefault();
        const space = String(new FormData(e.currentTarget).get('space'));
        location.assign(`/studio?space=${encodeURIComponent(space)}`);
      }}>
        <label htmlFor="workspace-space">Workspace</label>
        <select id="workspace-space" name="space" defaultValue={ctx.session.space}>
          <option value={ctx.session.space}>{ctx.session.space === 'system' ? 'Public site' : ctx.session.space}</option>
          {choices.filter(c => c.space !== ctx.session.space && c.status === 'active').map(c => (
            <option key={c.space} value={c.space}>{c.name || c.space}</option>
          ))}
        </select>
        <button type="submit">Open workspace</button>
      </form>
      {ctx.session.space === 'system' && <p>Scheduled reminders run in tenant workspaces. Choose your workspace for automation.</p>}
      <nav aria-label="Workspace">
        <div className="toolbar">
          {['Inbox', 'Products', 'Plans', 'Reviews', 'Campaigns', 'Team', 'Data'].map((name) => (
            <button key={name} aria-current={tab === name ? 'page' : undefined} onClick={() => setTab(name)}>
              {name}
            </button>
          ))}
          <button
            disabled={exporting}
            onClick={async () => {
              setExporting(true);
              setError('');
              try {
                await ctx.session.stepUp();
                const file = await ctx.client.downloadOwn();
                saveBlob(file.blob, file.name);
              } catch (e) {
                setError(message(e));
              } finally {
                setExporting(false);
              }
            }}
          >
            Download studio data
          </button>
        </div>
      </nav>
      {error && <p role="alert">{error}</p>}
      {tab === 'Inbox' && <Inbox {...ctx} />}
      {tab === 'Data' && <DataTools session={ctx.session} />}
      {tab === 'Products' && (
        <Records
          {...ctx}
          title="Products"
          read="studioProducts"
          create="addProduct"
          edit="editProduct"
          remove="deleteProduct"
          fields={productFields}
        />
      )}
      {tab === 'Plans' && (
        <Records
          {...ctx}
          title="Plans"
          read="studioPlans"
          create="addPlan"
          edit="editPlan"
          remove="deletePlan"
          fields={planFields}
        />
      )}
      {tab === 'Reviews' && (
        <Records
          {...ctx}
          title="Reviews"
          read="studioReviews"
          create="addTestimonial"
          edit="editReview"
          remove="deleteReview"
          fields={reviewFields}
        />
      )}
      {tab === 'Campaigns' && <Campaigns {...ctx} />}
      {tab === 'Team' && <Team {...ctx} />}
    </>
  );
}

/** Walk every keyset page, including a short or empty page with a cursor. */
function useListing(name: string, params?: Params) {
  const query = useOpPages(name, params ? { params } : {});
  useEffect(() => {
    if (!query.loading && query.hasMore && !query.refusal) query.more();
  }, [query.loading, query.hasMore, query.refusal, query.more]);
  return { ...query, refetch: query.restart };
}

function QueryStatus({ loading, refusal }: { loading: boolean; refusal: { message: string } | null }) {
  return refusal ? <p role="alert">{refusal.message}</p> : loading ? <p role="status">Loading…</p> : null;
}
function Table({
  title,
  columns,
  rows,
  actions,
}: {
  title: string;
  columns: string[];
  rows: readonly Row[];
  actions?: (row: Row) => ReactNode;
}) {
  return (
    <div className="table-wrapper" tabIndex={0} role="region" aria-label={title}>
      <table>
        <caption>{title}</caption>
        <thead>
          <tr>
            {columns.map((c) => (
              <th scope="col" key={c}>
                {columnLabel(c)}
              </th>
            ))}
            {actions && <th scope="col">Actions</th>}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, index) => (
            <tr key={text(row['id']) || index}>
              {columns.map((c, i) =>
                i === 0 ? (
                  <th scope="row" key={c}>
                    {text(row[c])}
                  </th>
                ) : (
                  <td key={c}>{text(row[c])}</td>
                ),
              )}
              {actions && (
                <td>
                  <div className="toolbar">{actions(row)}</div>
                </td>
              )}
            </tr>
          ))}
        </tbody>
      </table>
      {rows.length === 0 && <p>No records to show.</p>}
    </div>
  );
}
function Editor({
  ctx,
  op,
  fields,
  initial = {},
  onDone,
  onCancel,
  title,
}: {
  ctx: Context;
  op: string;
  fields: Field[];
  initial?: Row;
  onDone: () => void;
  onCancel: () => void;
  title: string;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  async function submit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    setBusy(true);
    setError('');
    const data = new FormData(e.currentTarget);
    const params: Record<string, Params[string]> = {};
    if (initial['id']) {
      params['id'] = text(initial['id']);
      params['rev'] = Number(initial['rev']);
    }
    for (const f of fields) {
      const value = text(data.get(f.name));
      params[f.name] =
        f.kind === 'checkbox'
          ? data.has(f.name)
          : f.kind === 'number'
            ? Number(value)
            : f.kind === 'list'
              ? value
                  .split('\n')
                  .map((s) => s.trim())
                  .filter(Boolean)
              : f.optional && !value
                ? null
                : f.kind === 'datetime-local'
                  ? new Date(value).toISOString().replace(/\.\d{3}Z$/, 'Z')
                  : value;
    }
    if (op === 'createCampaign') params['created'] = new Date().toISOString().replace(/\.\d{3}Z$/, 'Z');
    try {
      await commit(ctx.client, ctx.session, op, params);
      onDone();
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="card editor">
      <h3 id="editor-title">{title}</h3>
      <form className="form" aria-labelledby="editor-title" onSubmit={submit}>
        {fields.map((f) => {
          const id = `edit-${f.name}`;
          const value =
            initial[f.name] ??
            (f.kind === 'number' ? (f.name === 'stars' ? 5 : 100) : f.kind === 'money' ? '0.00' : '');
          const required = !f.optional && !['long', 'list', 'checkbox'].includes(f.kind ?? '');
          return (
            <div className="field" key={f.name}>
              {f.kind !== 'checkbox' && (
                <label htmlFor={id}>
                  {f.label}
                  {required ? ' (required)' : ''}
                </label>
              )}
              {f.choices ? (
                <select id={id} name={f.name} defaultValue={text(value)}>
                  {f.choices.map((v) => (
                    <option key={v}>{v}</option>
                  ))}
                </select>
              ) : f.kind === 'long' || f.kind === 'list' ? (
                <textarea
                  id={id}
                  name={f.name}
                  rows={4}
                  defaultValue={Array.isArray(value) ? value.join('\n') : text(value)}
                />
              ) : (
                <input
                  id={id}
                  name={f.name}
                  type={
                    f.kind === 'checkbox'
                      ? 'checkbox'
                      : f.kind === 'money' || f.kind === 'number'
                        ? 'number'
                        : (f.kind ?? 'text')
                  }
                  step={f.kind === 'money' ? '.01' : undefined}
                  min={f.kind === 'money' ? '0' : f.name === 'stars' ? '1' : undefined}
                  max={f.name === 'stars' ? '5' : undefined}
                  maxLength={f.name === 'title' ? 200 : undefined}
                  defaultChecked={f.kind === 'checkbox' ? value === true : undefined}
                  defaultValue={f.kind === 'checkbox' ? undefined : text(value)}
                  required={required}
                />
              )}
              {f.kind === 'checkbox' && <label htmlFor={id}>{f.label}</label>}
            </div>
          );
        })}
        <div className="toolbar">
          <button disabled={busy}>Save changes</button>
          <button type="button" disabled={busy} onClick={onCancel}>
            Cancel
          </button>
        </div>
        {error && <p role="alert">{error}</p>}
      </form>
    </section>
  );
}
function Remove({ ctx, op, row, done }: { ctx: Context; op: string; row: Row; done: () => void }) {
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  return (
    <>
      {!confirm ? (
        <button onClick={() => setConfirm(true)}>Delete</button>
      ) : (
        <>
          <span>Delete permanently?</span>
          <button
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              try {
                await commit(ctx.client, ctx.session, op, { id: text(row['id']) });
                done();
              } catch (e) {
                setError(message(e));
              } finally {
                setBusy(false);
              }
            }}
          >
            Confirm delete
          </button>
          <button disabled={busy} onClick={() => setConfirm(false)}>
            Keep record
          </button>
        </>
      )}
      {error && <span role="alert">{error}</span>}
    </>
  );
}
function Records(
  props: Context & { title: string; read: string; create: string; edit: string; remove: string; fields: Field[] },
) {
  const q = useListing(props.read);
  const [editing, setEditing] = useState<Row | null>(null);
  return (
    <section>
      <h2>{props.title}</h2>
      <p>Saved changes appear on the public site. Private staff notes stay in this workspace.</p>
      <button onClick={() => setEditing({})}>Add {props.title.toLowerCase().replace(/s$/, '')}</button>
      <QueryStatus {...q} />
      {editing && (
        <Editor
          key={text(editing['id']) || 'new'}
          ctx={props}
          title={editing['id'] ? `Edit ${props.title.toLowerCase()}` : `New ${props.title.toLowerCase()}`}
          op={editing['id'] ? props.edit : props.create}
          initial={editing}
          fields={props.fields.filter((f) => !(editing['id'] && f.name === 'slug'))}
          onCancel={() => setEditing(null)}
          onDone={() => {
            setEditing(null);
            q.refetch();
          }}
        />
      )}
      <Table
        title={props.title}
        columns={props.title === 'Reviews' ? ['author', 'quote', 'stars'] : ['name', 'tagline', 'price']}
        rows={q.rows}
        actions={(r) => (
          <>
            <button onClick={() => setEditing(r)}>Edit</button>
            <Remove ctx={props} op={props.remove} row={r} done={q.refetch} />
          </>
        )}
      />
    </section>
  );
}
function Inbox(ctx: Context) {
  const q = useListing('inbox');
  const report = useOpQuery('enquiryReport');
  const [query, setQuery] = useState('');
  const [editing, setEditing] = useState<Row | null>(null);
  const matches = q.rows.filter((r) =>
    ['name', 'email', 'company', 'message', 'status'].some((k) =>
      text(r[k]).toLowerCase().includes(query.toLowerCase()),
    ),
  );
  return (
    <section>
      <h2>Enquiry inbox</h2>
      <p>Follow up, keep private notes, and record the outcome.</p>
      <QueryStatus {...report} />
      <p>{report.rows.map((r) => `${text(r['status'])}: ${text(r['total'])}`).join(' · ')}</p>
      <form
        role="search"
        onSubmit={(e) => {
          e.preventDefault();
          setQuery(text(new FormData(e.currentTarget).get('q')));
        }}
        className="toolbar"
      >
        <label htmlFor="inbox-search">Search enquiries</label>
        <input id="inbox-search" name="q" type="search" />
        <button>Search</button>
      </form>
      <QueryStatus {...q} />
      {editing && (
        <>
          <blockquote>{text(editing['message'])}</blockquote>
          <Editor
            key={text(editing['id'])}
            ctx={ctx}
            title={`Follow up with ${text(editing['name'])}`}
            op="triageEnquiry"
            initial={editing}
            fields={[
              { name: 'status', label: 'Status', choices: ['new', 'responding', 'won', 'closed'] },
              field('notes', 'Private notes', 'long'),
            ]}
            onCancel={() => setEditing(null)}
            onDone={() => {
              setEditing(null);
              q.refetch();
              report.refetch();
            }}
          />
        </>
      )}
      <Table
        title="Enquiries"
        columns={['name', 'email', 'company', 'status']}
        rows={matches}
        actions={(r) => (
          <>
            <button onClick={() => setEditing(r)}>Follow up</button>
            <Remove
              ctx={ctx}
              op="deleteEnquiry"
              row={r}
              done={() => {
                q.refetch();
                report.refetch();
              }}
            />
          </>
        )}
      />
    </section>
  );
}
function Campaigns(ctx: Context) {
  const [tag, setTag] = useState('');
  const q = useListing(tag ? 'campaignsTagged' : 'campaignList', tag ? { tag } : undefined);
  const report = useOpQuery('campaignReport');
  const [editing, setEditing] = useState<Row | null>(null);
  const [selected, setSelected] = useState<Row | null>(null);
  return (
    <section>
      <h2>Campaigns</h2>
      <p>
        Draft briefs, set budgets and due dates, and share work with your team. Publishing a campaign records its
        approval; use Products, Plans or Reviews to change the public site.
      </p>
      <QueryStatus {...report} />
      <p>{report.rows.map((r) => `${text(r['status'])}: ${text(r['total'])}`).join(' · ')}</p>
      <div className="toolbar">
        <button onClick={() => setEditing({})}>New campaign</button>
        <form
          role="search"
          onSubmit={(e) => {
            e.preventDefault();
            setTag(text(new FormData(e.currentTarget).get('tag')));
          }}
        >
          <label htmlFor="tag">Filter by tag</label>
          <input id="tag" name="tag" type="search" />
          <button>Filter campaigns</button>
        </form>
      </div>
      <QueryStatus {...q} />
      {editing && (
        <Editor
          key={text(editing['id']) || 'new'}
          ctx={ctx}
          title={editing['id'] ? 'Edit campaign' : 'New campaign'}
          op={editing['id'] ? 'editCampaign' : 'createCampaign'}
          fields={
            editing['id']
              ? [
                  ...campaignFields,
                  { name: 'status', label: 'Status', choices: ['draft', 'review', 'published', 'archived'] },
                ]
              : campaignFields
          }
          initial={editing}
          onCancel={() => setEditing(null)}
          onDone={() => {
            setEditing(null);
            q.refetch();
            report.refetch();
          }}
        />
      )}
      <Table
        title="Campaigns"
        columns={['title', 'status', 'budget', 'due']}
        rows={q.rows}
        actions={(r) => (
          <>
            <button onClick={() => setEditing(r)}>Edit</button>
            <button onClick={() => setSelected(r)}>Files and access</button>
            <Remove
              ctx={ctx}
              op="deleteCampaign"
              row={r}
              done={() => {
                if (selected?.['id'] === r['id']) setSelected(null);
                q.refetch();
                report.refetch();
              }}
            />
          </>
        )}
      />
      {selected && (
        <CampaignDetail key={text(selected['id'])} {...ctx} row={selected} close={() => setSelected(null)} />
      )}
    </section>
  );
}
function CampaignDetail(ctx: Context & { row: Row; close: () => void }) {
  const campaign = text(ctx.row['id']);
  const files = useListing('campaignFiles', { campaign });
  const access = useListing('campaignAccess', { campaign });
  const approvals = useListing('approvalHistory', { campaign });
  const current = useListing('campaignList');
  const [review, setReview] = useState<{ words: number; checks: { label: string; passed: boolean }[] } | null>(null);
  const people = useListing('people');
  const reminders = useListing('campaignReminders');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  async function act(work: () => Promise<unknown>) {
    setBusy(true);
    setError('');
    setNotice('');
    try {
      await work();
      files.refetch();
      access.refetch();
      reminders.refetch();
      approvals.refetch();
      current.refetch();
      setNotice('Changes saved.');
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="card campaign-detail">
      <div className="toolbar">
        <h3>{text(ctx.row['title'])}</h3>
        <button onClick={ctx.close}>Close details</button>
      </div>
      <p>{text(ctx.row['brief'])}</p>
      {error && <p role="alert">{error}</p>}
      {notice && <p role="status">{notice}</p>}
      <h4 id="approval-title">Editorial review</h4>
      <button
        disabled={busy}
        onClick={() =>
          void act(async () => {
            const result = await ctx.client.callFunction<{
              words: number;
              checks: { label: string; passed: boolean }[];
              ready: boolean;
            }>('reviewBrief', { campaign });
            setReview(result.result);
          })
        }
      >
        Check brief
      </button>
      {review && (
        <div>
          <p>{review.words} words in this brief.</p>
          <ul>
            {review.checks.map((c) => (
              <li key={c.label}>
                {c.passed ? 'Complete' : 'Needs attention'}: {c.label}
              </li>
            ))}
          </ul>
        </div>
      )}
      <form
        className="form"
        aria-labelledby="approval-title"
        onSubmit={(e) => {
          e.preventDefault();
          const note = text(new FormData(e.currentTarget).get('note'));
          const latest = current.rows.find((r) => r['id'] === campaign);
          if (!latest) return;
          void act(() =>
            commit(ctx.client, ctx.session, 'approveCampaign', {
              id: campaign,
              rev: Number(latest['rev']),
              note,
              at: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
            }),
          );
        }}
      >
        <label htmlFor="approval-note">Approval note (required)</label>
        <input id="approval-note" name="note" required />
        <button disabled={busy}>Approve campaign</button>
      </form>
      <QueryStatus {...approvals} />
      <Table title="Approval history" columns={['at', 'note', 'reviewer']} rows={approvals.rows} />
      <h4 id="asset-title">Campaign files</h4>
      <QueryStatus {...files} />
      <form
        className="form"
        aria-labelledby="asset-title"
        onSubmit={(e) => {
          e.preventDefault();
          const form = e.currentTarget;
          const file = new FormData(form).get('file');
          if (!(file instanceof File) || !file.size) return;
          void act(async () => {
            const result = await commit(ctx.client, ctx.session, 'addAsset', {
              campaign,
              name: file.name,
              kind: file.type,
              size: file.size,
            });
            const id = text(result.rows[0]?.['created']);
            if (!id) throw new Error('The file record was not created.');
            try {
              await uploadFile(ctx.client, { entity: 'assets', field: 'file', id }, file);
            } catch (e) {
              await commit(ctx.client, ctx.session, 'deleteAsset', { id });
              throw e;
            }
            form.reset();
          });
        }}
      >
        <label htmlFor="asset-file">Choose a file (required)</label>
        <input id="asset-file" name="file" type="file" required />
        <button disabled={busy}>Upload file</button>
      </form>
      <Table
        title="Files"
        columns={['name', 'kind', 'size']}
        rows={files.rows}
        actions={(r) => (
          <>
            <button
              disabled={busy || !r['file']}
              onClick={() => void act(() => downloadFile(ctx.client, text(r['file']), text(r['name'])))}
            >
              Download
            </button>
            <Remove ctx={ctx} op="deleteAsset" row={r} done={files.refetch} />
          </>
        )}
      />
      <h4 id="access-title">Campaign access</h4>
      <QueryStatus {...access} />
      <QueryStatus {...people} />
      <form
        className="form"
        aria-labelledby="access-title"
        onSubmit={(e) => {
          e.preventDefault();
          const data = new FormData(e.currentTarget);
          void act(() =>
            commit(ctx.client, ctx.session, 'grantCampaignsAccess', {
              campaign,
              member: text(data.get('member')),
              level: text(data.get('level')),
            }),
          );
        }}
      >
        <label htmlFor="access-member">Team member</label>
        <select id="access-member" name="member" required>
          <option value="">Choose a member</option>
          {people.rows.map((r) => (
            <option key={text(r['id'])} value={text(r['id'])}>
              {text(r['displayName'])}
            </option>
          ))}
        </select>
        <label htmlFor="access-level">Access level</label>
        <select id="access-level" name="level">
          <option value="read">Read</option>
          <option value="write">Edit</option>
          <option value="admin">Manage access</option>
        </select>
        <button disabled={busy}>Grant campaign access</button>
      </form>
      <Table
        title="People with campaign access"
        columns={['member', 'level']}
        rows={access.rows.map((r) => ({
          ...r,
          member: text(people.rows.find((p) => p['id'] === r['member'])?.['displayName']) || r['member'],
          memberId: r['member'],
        }))}
        actions={(r) => (
          <button
            disabled={busy}
            onClick={() =>
              void act(() =>
                commit(ctx.client, ctx.session, 'unshareCampaign', { campaign, member: text(r['memberId']) }),
              )
            }
          >
            Revoke access
          </button>
        )}
      />
      <h4 id="reminder-title">Reminder</h4>
      <QueryStatus {...reminders} />
      {reminders.rows
        .filter((r) => r['campaign'] === campaign)
        .map((r) => (
          <p key={text(r['id'])}>
            {text(r['due'])} · {r['fired'] ? 'Fired' : 'Scheduled'}{' '}
            <button
              disabled={busy}
              onClick={() => void act(() => commit(ctx.client, ctx.session, 'clearReminder', { campaign }))}
            >
              Clear reminder
            </button>
          </p>
        ))}
      <form
        className="form"
        aria-labelledby="reminder-title"
        onSubmit={(e) => {
          e.preventDefault();
          const due = new Date(text(new FormData(e.currentTarget).get('due'))).toISOString().replace(/\.\d{3}Z$/, 'Z');
          void act(() => commit(ctx.client, ctx.session, 'setReminder', { campaign, due }));
        }}
      >
        <label htmlFor="reminder-due">Remind at (your local time)</label>
        <input id="reminder-due" name="due" type="datetime-local" required />
        <button disabled={busy}>Set reminder</button>
      </form>
    </section>
  );
}
function Team(ctx: Context) {
  const people = useListing('people');
  const invitations = useOpQuery('std.space.invitations', { live: false });
  const [error, setError] = useState('');
  const [ticket, setTicket] = useState('');
  const [busy, setBusy] = useState(false);
  const [leaving, setLeaving] = useState(false);
  async function act(name: string, params: Params) {
    setBusy(true);
    setError('');
    try {
      const r = await commit(ctx.client, ctx.session, name, params);
      if (name === 'std.space.leave') { location.assign('/studio'); return; }
      if (name === 'std.space.invite') setTicket(text(r.rows[0]?.['ticket']));
      people.refetch();
      invitations.refetch();
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section>
      <h2>Team access</h2>
      <p>Owners manage roles and invitations. Sensitive changes may ask for a fresh passkey touch.</p>
      {error && <p role="alert">{error}</p>}
      {ctx.session.space !== 'system' && (
        <section aria-label="Leave workspace">
          {!leaving ? <button type="button" onClick={() => setLeaving(true)}>Leave workspace</button> : <>
            <p>Leave this workspace? Your access ends. Shared records remain for your colleagues.</p>
            <button type="button" disabled={busy} onClick={() => void act('std.space.leave', {})}>Confirm leave</button>
            <button type="button" disabled={busy} onClick={() => setLeaving(false)}>Stay in workspace</button>
          </>}
        </section>
      )}
      <QueryStatus {...people} />
      <Table
        title="Studio members"
        columns={['displayName', 'role', 'status']}
        rows={people.rows}
        actions={(r) => (
          <>
            <form
              className="toolbar"
              onSubmit={(e) => {
                e.preventDefault();
                void act('std.space.setRole', {
                  member: text(r['id']),
                  role: text(new FormData(e.currentTarget).get('role')),
                });
              }}
            >
              <label htmlFor={`role-${text(r['id'])}`}>Role for {text(r['displayName'])}</label>
              <select id={`role-${text(r['id'])}`} name="role" defaultValue={text(r['role'])}>
                {['viewer', 'editor', 'manager', 'owner'].map((role) => (
                  <option key={role}>{role}</option>
                ))}
              </select>
              <button disabled={busy}>Save role</button>
            </form>
            <button
              disabled={busy}
              onClick={() =>
                void act(r['status'] === 'suspended' ? 'std.space.reinstate' : 'std.space.suspend', {
                  member: text(r['id']),
                })
              }
            >
              {r['status'] === 'suspended' ? 'Reinstate' : 'Suspend'}
            </button>
          </>
        )}
      />
      <Connections session={ctx.session} />
      <h3 id="invite-title">Invite a colleague</h3>
      <form
        className="form"
        aria-labelledby="invite-title"
        onSubmit={(e) => {
          e.preventDefault();
          const data = new FormData(e.currentTarget);
          void act('std.space.invite', { role: text(data.get('role')), label: text(data.get('label')), expires: '7d' });
        }}
      >
        <label htmlFor="invite-label">Invitation label (required)</label>
        <input id="invite-label" name="label" required />
        <label htmlFor="invite-role">Role</label>
        <select id="invite-role" name="role">
          <option>viewer</option>
          <option>editor</option>
          <option>manager</option>
        </select>
        <button disabled={busy}>Create invitation</button>
      </form>
      {ticket && (
        <section role="status">
          <p>Invitation created. Share this ticket privately with your colleague; it expires in 7 days.</p>
          <output className="ticket">{ticket}</output>
          <button onClick={() => setTicket('')}>Hide invitation</button>
        </section>
      )}
      <QueryStatus {...invitations} />
      <Table
        title="Invitations"
        columns={['label', 'role', 'status', 'expiresAt']}
        rows={invitations.rows}
        actions={(r) => (
          <button
            disabled={busy || r['status'] !== 'open'}
            onClick={() => void act('std.space.revokeInvitation', { id: text(r['id']) })}
          >
            Revoke invitation
          </button>
        )}
      />
    </section>
  );
}

function DataTools({ session }: { session: Session }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [done, setDone] = useState('');
  async function run(action: 'export' | 'import' | 'attach' | 'detach', params: Params) {
    setBusy(true); setError(''); setDone('');
    try {
      const result = await workspaceData(session, action, params);
      setDone(action === 'attach' || action === 'detach' ? text(result[0]?.['disclosure']) : action === 'export'
        ? 'Export prepared. Your deployment operator can transfer the bundle to its destination.'
        : `Import complete. ${text(result[0]?.['rows'])} rows processed. Open a workspace list to review the data.`);
    } catch (e) { setError(message(e)); }
    finally { setBusy(false); }
  }
  return <section className="card">
    <h2>Workspace data</h2>
    <p>Export this workspace with its members and data, or import a bundle your deployment operator has staged for this workspace.</p>
    <button disabled={busy} onClick={() => void run('export', { ciphertext: false })}>Prepare workspace export</button>
    <form className="form" aria-label="Import workspace bundle" onSubmit={e => {
      e.preventDefault();
      const data = new FormData(e.currentTarget);
      void run('import', { bundle: String(data.get('bundle')), name: String(data.get('name')) });
    }}>
      <label htmlFor="import-bundle">Staged bundle name (required)</label>
      <input id="import-bundle" name="bundle" required maxLength={128} autoComplete="off" spellCheck={false} />
      <label htmlFor="import-name">Import name (required)</label>
      <input id="import-name" name="name" required maxLength={64} autoComplete="off" spellCheck={false} aria-describedby="import-retry" />
      <p id="import-retry">Keep this name when retrying an interrupted import. A new name starts a separate import.</p>
      <button disabled={busy} type="submit">Import staged bundle</button>
    </form>
    <h3>File storage</h3>
    <p>Choose an operator-configured bucket for new files. Existing files stay in their original store, which must remain available. Files stay sealed under this workspace's keys.</p>
    <form className="form" aria-label="Attach file bucket" onSubmit={e => {
      e.preventDefault();
      const data = new FormData(e.currentTarget);
      void run('attach', { endpoint: String(data.get('endpoint')), region: String(data.get('region')),
        bucket: String(data.get('bucket')), credential: String(data.get('credential')), platformSealed: true });
    }}>
      <label htmlFor="bucket-endpoint">Storage endpoint (required)</label>
      <input id="bucket-endpoint" name="endpoint" type="url" required autoComplete="off" />
      <label htmlFor="bucket-region">Storage region (required)</label>
      <input id="bucket-region" name="region" required autoComplete="off" />
      <label htmlFor="bucket-name">Bucket name (required)</label>
      <input id="bucket-name" name="bucket" required autoComplete="off" />
      <label htmlFor="bucket-credential">Operator credential label (required)</label>
      <input id="bucket-credential" name="credential" required autoComplete="off" aria-describedby="bucket-secret-note" />
      <p id="bucket-secret-note">Use the label supplied by your operator. Never enter an access key or secret here.</p>
      <button type="submit" disabled={busy}>Attach bucket for new files</button>
    </form>
    <button type="button" disabled={busy} onClick={() => void run('detach', {})}>Use deployment storage for new files</button>
    {error && <p role="alert">{error}</p>}
    {done && <p role="status">{done}</p>}
  </section>;
}


function Connections({ session }: { session: Session }) {
  const [connections, setConnections] = useState<Row[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [done, setDone] = useState('');
  const [retiring, setRetiring] = useState('');
  async function run(action: 'list' | 'add' | 'update' | 'retire', params: Params = {}) {
    setBusy(true); setError(''); setDone('');
    try {
      if (action !== 'list') await tenantConnections(session, action, params);
      setConnections(await tenantConnections(session, 'list'));
      setDone(action === 'list' ? 'Connections loaded.' : 'Sign-in connection saved.');
      setRetiring('');
    } catch (e) { setError(message(e)); }
    finally { setBusy(false); }
  }
  return <details>
    <summary>Sign-in providers</summary>
    <p>Owners can connect an OpenID Connect provider for this workspace. Keep an owner who signs in independently of these providers.</p>
    <button type="button" disabled={busy} onClick={() => void run('list')}>Load sign-in providers</button>
    <form className="form" aria-label="Add sign-in provider" onSubmit={e => {
      e.preventDefault();
      const data = new FormData(e.currentTarget);
      const params = { name: String(data.get('name')), kind: 'oidc', issuer: String(data.get('issuer')),
        clientId: String(data.get('clientId')), clientSecret: String(data.get('clientSecret')), role: String(data.get('role')) };
      e.currentTarget.reset();
      void run('add', params);
    }}>
      <label htmlFor="connection-name">Connection name (required)</label>
      <input id="connection-name" name="name" required maxLength={32} autoComplete="off" />
      <label htmlFor="connection-issuer">Issuer URL (required)</label>
      <input id="connection-issuer" name="issuer" type="url" required autoComplete="off" />
      <label htmlFor="connection-client">Client ID (required)</label>
      <input id="connection-client" name="clientId" required autoComplete="off" />
      <label htmlFor="connection-secret">Client secret (required)</label>
      <input id="connection-secret" name="clientSecret" type="password" required autoComplete="new-password" />
      <label htmlFor="connection-role">New member role</label>
      <select id="connection-role" name="role" defaultValue="viewer"><option>viewer</option><option>editor</option></select>
      <button type="submit" disabled={busy}>Add sign-in provider</button>
    </form>
    <ul aria-label="Sign-in connections">{connections.map(row => <li key={text(row['key'])}>
      <span>{text(row['name'])} · {text(row['state'])}</span>
      {row['state'] === 'active' && <>
        <form aria-label={`Rotate secret for ${text(row['name'])}`} onSubmit={e => {
          e.preventDefault();
          const secret = String(new FormData(e.currentTarget).get('secret'));
          e.currentTarget.reset();
          void run('update', { name: text(row['name']), clientSecret: secret });
        }}>
          <label htmlFor={`rotate-${text(row['key'])}`}>New secret for {text(row['name'])}</label>
          <input id={`rotate-${text(row['key'])}`} name="secret" type="password" required autoComplete="new-password" />
          <button type="submit" disabled={busy}>Save new secret for {text(row['name'])}</button>
        </form>
        {retiring !== row['name'] ? <button type="button" onClick={() => setRetiring(text(row['name']))}>Retire {text(row['name'])}</button> : <>
          <p>Retire this provider? New sign-ins and pending callbacks through it will stop.</p>
          <button type="button" disabled={busy} onClick={() => void run('retire', { name: text(row['name']) })}>Confirm retire {text(row['name'])}</button>
          <button type="button" disabled={busy} onClick={() => setRetiring('')}>Keep provider</button>
        </>}
      </>}
    </li>)}</ul>
    {error && <p role="alert">{error}</p>}
    {done && <p role="status">{done}</p>}
  </details>;
}
