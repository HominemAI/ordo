import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react';
import { ConsoleCallError, explain, SIGNED_OUT } from '../api';
import { listTenants, size, type Tenant } from '../tenants';
import {
  count,
  duration,
  listQuotas,
  period,
  QUANTITIES,
  readUsage,
  setQuota,
  slowOps,
  type Quantity,
  type Quota,
  type SlowOp,
  type SlowOps,
  type UsageReading,
} from '../usage';
import { useDeploymentTouch } from './useDeploymentTouch';
import './usage.css';

const RANGES = [
  { secs: 86_400, name: 'Last 24 hours' },
  { secs: 7 * 86_400, name: 'Last 7 days' },
  { secs: 30 * 86_400, name: 'Last 30 days' },
] as const;

const PERIODS = [
  { secs: 3600, name: 'An hour' },
  { secs: 86_400, name: 'A day' },
  { secs: 30 * 86_400, name: '30 days' },
] as const;

const QUANTITY_WORDS: Record<Quantity, string> = {
  requests: 'Requests',
  rows_returned: 'Rows returned',
};

function when(secs: number): string {
  return new Date(secs * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

/** Requests per period as bars, with the numbers in words beside it. */
function Bars(props: { reading: UsageReading }) {
  const ps = props.reading.periods;
  if (ps.length === 0) return <p className="muted">No usage was counted in this range.</p>;
  const max = Math.max(1, ...ps.map((p) => p.requests));
  const peak = ps.reduce((a, b) => (b.requests > a.requests ? b : a));
  const w = 600;
  const h = 120;
  const step = w / ps.length;
  const gap = Math.min(2, step / 4);
  return (
    <figure className="usage-chart">
      <svg viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" role="img" aria-labelledby="usage-chart-caption">
        {ps.map((p, i) => {
          const bh = (p.requests / max) * (h - 4);
          return (
            <rect key={p.start} x={i * step + gap / 2} y={h - bh} width={Math.max(1, step - gap)} height={bh}>
              <title>
                {when(p.start)}: {count(p.requests)} requests
              </title>
            </rect>
          );
        })}
      </svg>
      <figcaption id="usage-chart-caption" className="muted small">
        Requests per period, {ps.length} periods from {when(ps[0]!.start)}. The busiest had {count(peak.requests)}, at{' '}
        {when(peak.start)}.
      </figcaption>
    </figure>
  );
}

type SortKey = 'op' | 'tier' | 'meanMicros' | 'maxMicros' | 'calls' | 'errors';
const COLUMNS: { key: SortKey; name: string }[] = [
  { key: 'op', name: 'Op' },
  { key: 'tier', name: 'Tier' },
  { key: 'meanMicros', name: 'Mean time' },
  { key: 'maxMicros', name: 'Longest' },
  { key: 'calls', name: 'Calls' },
  { key: 'errors', name: 'Errors' },
];

function sorted(ops: SlowOp[], key: SortKey, up: boolean): SlowOp[] {
  const out = [...ops].sort((a, b) => {
    const x = a[key] ?? '';
    const y = b[key] ?? '';
    return typeof x === 'number' && typeof y === 'number' ? x - y : String(x).localeCompare(String(y));
  });
  return up ? out : out.reverse();
}

/** The quota the edit dialog is open for. */
type Editing = { space: string; quantity: Quantity; limit: string; grace: string; periodSecs: number; isNew: boolean };

/**
 * Usage and limits (docs/plan-console.md 4.9 and 4.11): what each tenant and
 * the whole deployment used over a range, the quotas with an edit, and the
 * slow ops table from the node that answered. A quota change asks for the
 * owner's touch at the deployment, relayed by the console.
 */
export function Usage(props: { onSignedOut: () => void }) {
  const [tenants, setTenants] = useState<Tenant[]>([]);
  const [home, setHome] = useState<string>('');
  const [whose, setWhose] = useState('');
  const [range, setRange] = useState<number>(RANGES[0].secs);
  const [reading, setReading] = useState<UsageReading | null>(null);
  const [quotas, setQuotas] = useState<Quota[] | null>(null);
  const [ops, setOps] = useState<SlowOps | null>(null);
  const [sort, setSort] = useState<{ key: SortKey; up: boolean }>({ key: 'meanMicros', up: false });
  const [editing, setEditing] = useState<Editing | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const editDialog = useRef<HTMLDialogElement>(null);
  const { atDeployment, prompt } = useDeploymentTouch();

  const act = useCallback(
    async (f: () => Promise<void>) => {
      setBusy(true);
      setError(null);
      try {
        await f();
      } catch (e) {
        if (e instanceof ConsoleCallError && SIGNED_OUT.has(e.code)) {
          props.onSignedOut();
          return;
        }
        if (e instanceof DOMException && e.name === 'AbortError') return;
        setError(explain(e));
      } finally {
        setBusy(false);
      }
    },
    [props],
  );

  const loadUsage = useCallback(async (secs: number, tenant: string) => {
    const to = Math.floor(Date.now() / 1000);
    setReading(await readUsage(to - secs, to, tenant));
  }, []);
  const loadQuotas = useCallback(async () => setQuotas((await listQuotas()).quotas), []);

  useEffect(() => {
    // Once, on open.
    void act(async () => {
      const [t, , s] = await Promise.all([listTenants(), loadQuotas(), slowOps()]);
      setTenants(t.tenants.filter((x) => x.status === 'active' || x.status === 'paused'));
      setOps(s);
      await loadUsage(RANGES[0].secs, '');
    });
  }, []);

  useEffect(() => {
    // The deployment's own space, named by a reading of the whole deployment.
    if (reading?.home) setHome(reading.home);
  }, [reading]);

  useEffect(() => {
    const d = editDialog.current;
    if (!d) return;
    if (editing && !d.open) d.showModal();
    if (!editing && d.open) d.close();
  }, [editing]);

  const nameOf = (id: string) =>
    id === home ? "The deployment's own space" : (tenants.find((t) => t.id === id)?.name ?? id);

  const pick = (secs: number, tenant: string) => {
    setRange(secs);
    setWhose(tenant);
    void act(() => loadUsage(secs, tenant));
  };

  const save = (e: FormEvent) => {
    e.preventDefault();
    if (!editing) return;
    const limit = Number(editing.limit);
    const grace = Number(editing.grace);
    if (!Number.isInteger(limit) || limit < 0 || !Number.isInteger(grace) || grace < 0) {
      setError('The limit and the grace are whole numbers, 0 or more.');
      return;
    }
    setDone(null);
    void act(async () => {
      const r = await atDeployment(() =>
        setQuota({ space: editing.space, quantity: editing.quantity, limit, grace, periodSecs: editing.periodSecs }),
      );
      setEditing(null);
      setDone(
        `Saved: ${nameOf(editing.space)}, ${QUANTITY_WORDS[editing.quantity].toLowerCase()} up to ${count(limit)} in ${period(editing.periodSecs)}.` +
          (r.quota.note ? ` ${r.quota.note}` : ''),
      );
      await loadQuotas();
    });
  };

  const notEnforced = (quotas ?? []).some((q) => !q.enforced);
  const shown = ops ? sorted(ops.ops, sort.key, sort.up) : [];

  return (
    <section className="card">
      <h1>Usage</h1>
      <p className="muted">
        What each tenant and the whole deployment used, the quotas that limit it, and which ops cost the most.
      </p>
      {done && (
        <p role="status" className="ok">
          {done}
        </p>
      )}
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}

      <h2>Over time</h2>
      <div className="usage-controls">
        <div>
          <label htmlFor="usage-whose">Whose</label>
          <select id="usage-whose" value={whose} disabled={busy} onChange={(e) => pick(range, e.target.value)}>
            <option value="">The whole deployment</option>
            {tenants.map((t) => (
              <option key={t.id} value={t.id}>
                {t.name}
              </option>
            ))}
          </select>
        </div>
        <div>
          <label htmlFor="usage-range">Range</label>
          <select
            id="usage-range"
            value={range}
            disabled={busy}
            onChange={(e) => pick(Number(e.target.value), whose)}
          >
            {RANGES.map((r) => (
              <option key={r.secs} value={r.secs}>
                {r.name}
              </option>
            ))}
          </select>
        </div>
      </div>
      {reading === null && !error && <p className="muted">Loading…</p>}
      {reading && (
        <>
          <dl className="facts usage-totals">
            <dt>Requests</dt>
            <dd>{count(reading.total.requests)}</dd>
            <dt>Rows returned</dt>
            <dd>{count(reading.total.rowsReturned)}</dd>
            <dt>Writes</dt>
            <dd>{count(reading.total.writes)}</dd>
            <dt>Stored, largest</dt>
            <dd>{size(reading.total.bytes)}</dd>
          </dl>
          <Bars reading={reading} />
          {reading.gaps.length > 0 && (
            <p className="note" role="note">
              {reading.gaps.length === 1 ? '1 period was' : `${reading.gaps.length} periods were`} not counted:{' '}
              {reading.gaps.map((g) => `${nameOf(g.space)} at ${when(g.start)} (${g.reason})`).join('; ')}.
            </p>
          )}
          {reading.missing.length > 0 && (
            <p className="note" role="note">
              Not in these numbers, because the deployment did not answer for them:{' '}
              {reading.missing.map((m) => `${m.name} (${m.message})`).join('; ')}.
            </p>
          )}
          {reading.leftOut > 0 && (
            <p className="note" role="note">
              {reading.leftOut} more tenants are not in these numbers: a reading covers at most 200.
            </p>
          )}
          {reading.scope === 'deployment' && reading.byTenant.length > 0 && (
            <div className="usage-table" role="region" aria-labelledby="usage-by-caption" tabIndex={0}>
              <table>
                <caption id="usage-by-caption">Usage by tenant</caption>
                <thead>
                  <tr>
                    <th scope="col">Tenant</th>
                    <th scope="col">Requests</th>
                    <th scope="col">Rows returned</th>
                    <th scope="col">Writes</th>
                  </tr>
                </thead>
                <tbody>
                  {[...reading.byTenant]
                    .sort((a, b) => b.requests - a.requests)
                    .map((t) => (
                      <tr key={t.id}>
                        <th scope="row">
                          <span className="usage-name">{t.name}</span>
                          <span className="mono small muted">{t.id}</span>
                        </th>
                        <td data-label="Requests">{count(t.requests)}</td>
                        <td data-label="Rows returned">{count(t.rowsReturned)}</td>
                        <td data-label="Writes">{count(t.writes)}</td>
                      </tr>
                    ))}
                </tbody>
              </table>
            </div>
          )}
        </>
      )}

      <h2>Quotas</h2>
      <p className="muted">
        Past its limit a tenant is warned; past the limit and the grace its requests are refused until the period
        starts again.
      </p>
      {notEnforced && (
        <p className="note" role="note">
          These quotas are stored and limit nothing yet. They count from the usage ledger, which is off: set
          ORDO_USAGE_LEDGER=true on the deployment.
        </p>
      )}
      {quotas && quotas.length === 0 && <p>No quotas are set. Every tenant may use as much as it asks for.</p>}
      {quotas && quotas.length > 0 && (
        <div className="usage-table" role="region" aria-labelledby="quotas-caption" tabIndex={0}>
          <table>
            <caption id="quotas-caption">Quotas</caption>
            <thead>
              <tr>
                <th scope="col">Tenant</th>
                <th scope="col">Counts</th>
                <th scope="col">Limit</th>
                <th scope="col">Grace</th>
                <th scope="col">Per</th>
                <th scope="col">Used this period</th>
                <th scope="col">Edit</th>
              </tr>
            </thead>
            <tbody>
              {quotas.map((q) => (
                <tr key={`${q.space}/${q.quantity}`}>
                  <th scope="row">
                    <span className="usage-name">{nameOf(q.space)}</span>
                    <span className="mono small muted">{q.space}</span>
                  </th>
                  <td data-label="Counts">{QUANTITY_WORDS[q.quantity] ?? q.quantity}</td>
                  <td data-label="Limit">{count(q.limit)}</td>
                  <td data-label="Grace">{count(q.grace)}</td>
                  <td data-label="Per">{period(q.periodSecs)}</td>
                  <td data-label="Used this period">
                    {count(q.used)}
                    {q.used >= q.limit + q.grace ? (
                      <span className="tag usage-over">refusing</span>
                    ) : q.used >= q.limit ? (
                      <span className="tag usage-over">over the limit</span>
                    ) : null}
                  </td>
                  <td data-label="Edit">
                    <button
                      type="button"
                      disabled={busy}
                      aria-label={`Edit the ${QUANTITY_WORDS[q.quantity]?.toLowerCase() ?? q.quantity} quota of ${nameOf(q.space)}`}
                      onClick={() =>
                        setEditing({
                          space: q.space,
                          quantity: q.quantity,
                          limit: String(q.limit),
                          grace: String(q.grace),
                          periodSecs: q.periodSecs,
                          isNew: false,
                        })
                      }
                    >
                      Edit
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <div className="actions">
        <button
          type="button"
          disabled={busy || tenants.length + (home ? 1 : 0) === 0}
          onClick={() =>
            setEditing({
              space: tenants[0]?.id ?? home,
              quantity: 'requests',
              limit: '',
              grace: '0',
              periodSecs: 86_400,
              isNew: true,
            })
          }
        >
          Set a quota
        </button>
      </div>

      <h2>Slow ops</h2>
      {ops === null && !error && <p className="muted">Loading…</p>}
      {ops && ops.ops.length === 0 && <p>This node has served no ops since it started.</p>}
      {ops && ops.ops.length > 0 && (
        <>
          <p className="muted small">
            From one node{ops.node ? `, ${ops.node}` : ''}
            {ops.since ? `, since it started at ${when(ops.since)}` : ''}. Each node counts its own, and starts again
            when it restarts.
          </p>
          <div className="usage-table" role="region" aria-labelledby="slow-caption" tabIndex={0}>
            <table>
              <caption id="slow-caption">Slow ops</caption>
              <thead>
                <tr>
                  {COLUMNS.map((c) => (
                    <th
                      key={c.key}
                      scope="col"
                      aria-sort={sort.key === c.key ? (sort.up ? 'ascending' : 'descending') : undefined}
                    >
                      <button
                        type="button"
                        className="sort"
                        onClick={() =>
                          setSort((s) => ({ key: c.key, up: s.key === c.key ? !s.up : c.key === 'op' || c.key === 'tier' }))
                        }
                      >
                        {c.name}
                        <span aria-hidden="true">{sort.key === c.key ? (sort.up ? ' ▲' : ' ▼') : ''}</span>
                      </button>
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {shown.map((o) => (
                  <tr key={o.op}>
                    <th scope="row" className="mono">
                      {o.op}
                    </th>
                    <td data-label="Tier">{o.tier ?? o.registeredClass}</td>
                    <td data-label="Mean time">{duration(o.meanMicros)}</td>
                    <td data-label="Longest">{duration(o.maxMicros)}</td>
                    <td data-label="Calls">{count(o.calls)}</td>
                    <td data-label="Errors">
                      {count(o.errors)}
                      {o.calls > 0 && o.errors > 0 ? ` (${((o.errors / o.calls) * 100).toFixed(1)}%)` : ''}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}

      <dialog ref={editDialog} aria-labelledby="quota-title" onCancel={() => setEditing(null)}>
        {editing && (
          <form onSubmit={save}>
            <h2 id="quota-title">{editing.isNew ? 'Set a quota' : `Quota for ${nameOf(editing.space)}`}</h2>
            {editing.isNew && (
              <>
                <label htmlFor="quota-space">Tenant</label>
                <select
                  id="quota-space"
                  value={editing.space}
                  onChange={(e) => setEditing({ ...editing, space: e.target.value })}
                >
                  {home && <option value={home}>The deployment's own space</option>}
                  {tenants.map((t) => (
                    <option key={t.id} value={t.id}>
                      {t.name}
                    </option>
                  ))}
                </select>
                <label htmlFor="quota-quantity">Counts</label>
                <select
                  id="quota-quantity"
                  value={editing.quantity}
                  onChange={(e) => setEditing({ ...editing, quantity: e.target.value as Quantity })}
                >
                  {QUANTITIES.map((q) => (
                    <option key={q} value={q}>
                      {QUANTITY_WORDS[q]}
                    </option>
                  ))}
                </select>
              </>
            )}
            {!editing.isNew && <p>Counts {QUANTITY_WORDS[editing.quantity].toLowerCase()}.</p>}
            <label htmlFor="quota-limit">Limit</label>
            <input
              id="quota-limit"
              type="number"
              inputMode="numeric"
              min={0}
              step={1}
              required
              value={editing.limit}
              aria-describedby="quota-limit-hint"
              onChange={(e) => setEditing({ ...editing, limit: e.target.value })}
            />
            <p id="quota-limit-hint" className="muted small">
              Past this, the tenant is warned and still served.
            </p>
            <label htmlFor="quota-grace">Grace</label>
            <input
              id="quota-grace"
              type="number"
              inputMode="numeric"
              min={0}
              step={1}
              required
              value={editing.grace}
              aria-describedby="quota-grace-hint"
              onChange={(e) => setEditing({ ...editing, grace: e.target.value })}
            />
            <p id="quota-grace-hint" className="muted small">
              How far past the limit it may go before it is refused.
            </p>
            <label htmlFor="quota-period">Per</label>
            <select
              id="quota-period"
              value={editing.periodSecs}
              onChange={(e) => setEditing({ ...editing, periodSecs: Number(e.target.value) })}
            >
              {!PERIODS.some((p) => p.secs === editing.periodSecs) && (
                <option value={editing.periodSecs}>{period(editing.periodSecs)}</option>
              )}
              {PERIODS.map((p) => (
                <option key={p.secs} value={p.secs}>
                  {p.name}
                </option>
              ))}
            </select>
            <div className="actions">
              <button type="submit" className="primary" disabled={busy}>
                Save quota
              </button>
              <button type="button" className="link" onClick={() => setEditing(null)}>
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
