import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ConsoleCallError, explain, SIGNED_OUT } from '../api';
import { link, type Link } from '../deployment';
import { listTenants, type Tenant } from '../tenants';
import {
  deleteSkip,
  deleteWindow,
  lengthWords,
  listWindows,
  localToUtc,
  ms,
  setWindow,
  skipWindow,
  utcToLocal,
  type Listing,
  type Occurrence,
  type Target,
  type Window,
} from '../windows';
import { useDeploymentTouch } from './useDeploymentTouch';
import './windows.css';

/** How many times each list shows. */
const SHOWN = 50;

/** The form's fields, as typed. Times are the viewer's local time. */
interface Draft {
  id: string | null;
  kind: 'range' | 'cron';
  target: Target;
  tenants: string[];
  starts: string;
  ends: string;
  cron: string;
  lengthMins: string;
  message: string;
}

const EMPTY: Draft = {
  id: null,
  kind: 'range',
  target: 'all',
  tenants: [],
  starts: '',
  ends: '',
  cron: '',
  lengthMins: '60',
  message: '',
};

function draftOf(w: Window): Draft {
  return {
    id: w.id,
    kind: w.cron ? 'cron' : 'range',
    target: w.target,
    tenants: [...w.tenants],
    starts: utcToLocal(w.starts),
    ends: utcToLocal(w.ends),
    cron: w.cron ?? '',
    lengthMins: w.lengthSecs ? String(Math.round(w.lengthSecs / 60)) : '60',
    message: w.message,
  };
}

/** `2026-11-08 03:00 UTC`. */
function utcWords(utc: string): string {
  return `${utc.slice(0, 10)} ${utc.slice(11, 16)} UTC`;
}

function localWords(utc: string): string {
  return new Date(utc).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

function when(secs: number): string {
  return new Date(secs * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

const TARGET_WORDS: Record<Target, string> = {
  all: 'All tenants',
  only: 'Only these tenants',
  except: 'All tenants except these',
};

function targetWords(t: Target, tenants: string[]): string {
  if (t === 'all') return TARGET_WORDS.all;
  return `${t === 'only' ? 'Only' : 'All except'}: ${tenants.join(', ')}`;
}

/**
 * Maintenance windows (owner, 2026-11-04): blocks of time the deployment
 * owner sets. A window does nothing by itself. It covers all tenants, only
 * some, or all but some, once (a start and an end) or on a cron schedule in
 * UTC for a length. Its message is any text, shown as plain text. Every
 * change asks for the owner's touch at the deployment.
 */
export function Windows(props: { onSignedOut: () => void }) {
  const [state, setState] = useState<Link | null>(null);
  const [list, setList] = useState<Listing | null>(null);
  const [tenants, setTenants] = useState<Tenant[]>([]);
  const [tenantsError, setTenantsError] = useState<string | null>(null);
  const [utc, setUtc] = useState(false);
  const [draft, setDraft] = useState<Draft>(EMPTY);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [skipping, setSkipping] = useState<string | null>(null);
  const [skip, setSkip] = useState({ starts: '', ends: '', reason: '' });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const formHeading = useRef<HTMLHeadingElement>(null);
  const { atDeployment, prompt } = useDeploymentTouch();

  const load = useCallback(async () => {
    const s = await link();
    setState(s);
    if (!s.linked) return;
    setList(await listWindows());
    try {
      setTenants((await listTenants()).tenants.filter((t) => t.status !== 'reaped'));
      setTenantsError(null);
    } catch (e) {
      setTenantsError(explain(e));
    }
  }, []);

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
        setError(explain(e));
      } finally {
        setBusy(false);
      }
    },
    [props],
  );

  useEffect(() => {
    // Once, on open.
    void act(load);
  }, []);

  /** A door time, in the viewer's local time or in UTC, with the other on hover. */
  const time = (t: string) => (
    <time dateTime={t} title={utc ? localWords(t) : utcWords(t)}>
      {utc ? utcWords(t) : localWords(t)}
    </time>
  );

  const byId = useMemo(() => new Map((list?.windows ?? []).map((w) => [w.id, w])), [list]);
  const skipsById = useMemo(() => new Map((list?.skips ?? []).map((s) => [s.id, s])), [list]);
  const now = Date.now();
  const upcoming = (list?.occurrences ?? []).filter((o) => ms(o.ends) >= now);
  const past = (list?.occurrences ?? []).filter((o) => ms(o.ends) < now).reverse();

  const save = () =>
    act(async () => {
      const lengthMins = Number(draft.lengthMins);
      const w = await atDeployment(() =>
        setWindow({
          ...(draft.id ? { id: draft.id } : {}),
          target: draft.target,
          tenants: draft.tenants,
          message: draft.message,
          schedule:
            draft.kind === 'range'
              ? { kind: 'range', starts: localToUtc(draft.starts), ends: localToUtc(draft.ends) }
              : { kind: 'cron', cron: draft.cron.trim(), lengthSecs: Math.round(lengthMins * 60) },
        }),
      );
      setDone(draft.id ? `Saved window ${w.id}.` : `Added window ${w.id}.`);
      setDraft(EMPTY);
      await load();
    });

  const remove = (id: string) =>
    act(async () => {
      await atDeployment(() => deleteWindow(id));
      setDeleting(null);
      if (draft.id === id) setDraft(EMPTY);
      setDone(`Deleted window ${id}.`);
      await load();
    });

  const addSkip = (window: string) =>
    act(async () => {
      const s = await atDeployment(() =>
        skipWindow(window, localToUtc(skip.starts), localToUtc(skip.ends), skip.reason.trim()),
      );
      setSkipping(null);
      setSkip({ starts: '', ends: '', reason: '' });
      setDone(`Skipping window ${window} from ${utcWords(s.starts)} to ${utcWords(s.ends)}.`);
      await load();
    });

  const removeSkip = (id: string) =>
    act(async () => {
      await atDeployment(() => deleteSkip(id));
      setDone(`Removed skip ${id}.`);
      await load();
    });

  const edit = (w: Window) => {
    setDraft(draftOf(w));
    setDone(null);
    setError(null);
    formHeading.current?.scrollIntoView({ block: 'start' });
    formHeading.current?.focus();
  };

  const toggleTenant = (id: string, on: boolean) =>
    setDraft((d) => ({ ...d, tenants: on ? [...d.tenants, id] : d.tenants.filter((t) => t !== id) }));

  /** The tenants to pick from: the deployment's, plus any the window names that are gone. */
  const choices = [
    ...tenants.map((t) => ({ id: t.id, name: t.name })),
    ...draft.tenants.filter((id) => !tenants.some((t) => t.id === id)).map((id) => ({ id, name: '' })),
  ];

  const occurrenceRow = (o: Occurrence, i: number) => {
    const w = byId.get(o.window);
    const s = o.skippedBy ? skipsById.get(o.skippedBy) : undefined;
    return (
      <li key={`${o.window}-${o.starts}-${i}`} className={o.skippedBy ? 'skipped' : ''}>
        <div className="rev-head">
          <strong>
            {time(o.starts)} to {time(o.ends)}
          </strong>
          {o.skippedBy && <span className="tag">skipped</span>}
        </div>
        {w && <p className="window-message">{w.message}</p>}
        <div className="muted small">
          {w ? targetWords(w.target, w.tenants) : TARGET_WORDS[o.target]}. Window <span className="mono">{o.window}</span>
          {s?.reason ? `. Skipped: ${s.reason}` : ''}
        </div>
      </li>
    );
  };

  return (
    <section className="card windows">
      <h1>Maintenance windows</h1>
      <p className="muted small">
        A window is a block of time you set. It does nothing by itself. The list runs from 12 months back to 12 months
        ahead.
      </p>
      {state === null && !error && <p className="muted">Loading…</p>}

      {state && !state.linked && (
        <p className="note">The console is not linked to the deployment yet. Link it on the Connection page first.</p>
      )}

      <div role="status">{done && <p className="ok">{done}</p>}</div>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}

      {state?.linked && list && (
        <>
          <div className="check">
            <input type="checkbox" id="times-utc" checked={utc} onChange={(e) => setUtc(e.target.checked)} />
            <label htmlFor="times-utc">Show times in UTC (otherwise your local time)</label>
          </div>

          <h2>Upcoming</h2>
          {list.truncated && (
            <p className="note small">The list reached its limit and stops early. Some later times are not shown.</p>
          )}
          {upcoming.length === 0 ? (
            <p className="muted">No window is coming up.</p>
          ) : (
            <ol className="revisions" aria-label="Upcoming windows">
              {upcoming.slice(0, SHOWN).map(occurrenceRow)}
            </ol>
          )}
          {upcoming.length > SHOWN && (
            <p className="muted small">
              The next {SHOWN} of {upcoming.length} are shown.
            </p>
          )}
          {past.length > 0 && (
            <details className="logs">
              <summary>Past ({past.length})</summary>
              <ol className="revisions" aria-label="Past windows">
                {past.slice(0, SHOWN).map(occurrenceRow)}
              </ol>
              {past.length > SHOWN && <p className="muted small">The latest {SHOWN} are shown.</p>}
            </details>
          )}

          <h2>Windows</h2>
          {list.windows.length === 0 ? (
            <p className="muted">No window is set.</p>
          ) : (
            <ol className="revisions" aria-label="Windows">
              {list.windows.map((w) => {
                const skips = list.skips.filter((s) => s.window === w.id);
                return (
                  <li key={w.id}>
                    <div className="rev-head">
                      <strong>
                        {w.cron ? (
                          <>
                            Repeats <span className="mono">{w.cron}</span> (UTC), {lengthWords(w.lengthSecs ?? 0)} each
                          </>
                        ) : (
                          <>
                            {time(w.starts ?? '')} to {time(w.ends ?? '')}
                          </>
                        )}
                      </strong>
                      <span className="tag">{w.cron ? 'repeats' : 'once'}</span>
                    </div>
                    <p className="window-message">{w.message}</p>
                    <div className="muted small">
                      {targetWords(w.target, w.tenants)}. Set by {w.setBy}, {when(w.setAt)}.{' '}
                      <span className="mono">{w.id}</span>
                    </div>

                    {skips.length > 0 && (
                      <ul className="kv" aria-label={`Skips of ${w.id}`}>
                        {skips.map((s) => (
                          <li key={s.id}>
                            <span>
                              Skipped {time(s.starts)} to {time(s.ends)}
                              {s.reason ? `: ${s.reason}` : ''}
                            </span>{' '}
                            <button
                              type="button"
                              className="link"
                              disabled={busy}
                              onClick={() => void removeSkip(s.id)}
                            >
                              Remove skip
                            </button>
                          </li>
                        ))}
                      </ul>
                    )}

                    {deleting !== w.id && skipping !== w.id && (
                      <div className="row-actions">
                        <button type="button" disabled={busy} onClick={() => edit(w)}>
                          Edit
                        </button>
                        {w.cron && (
                          <button
                            type="button"
                            disabled={busy}
                            onClick={() => {
                              setSkipping(w.id);
                              setSkip({ starts: '', ends: '', reason: '' });
                            }}
                          >
                            Skip a range
                          </button>
                        )}
                        <button type="button" disabled={busy} onClick={() => setDeleting(w.id)}>
                          Delete
                        </button>
                      </div>
                    )}

                    {deleting === w.id && (
                      <div className="confirm" role="group" aria-label={`Confirm delete window ${w.id}`}>
                        <p>Delete this window and its skips? This cannot be undone.</p>
                        <div className="actions">
                          <button type="button" className="danger" disabled={busy} onClick={() => void remove(w.id)}>
                            Delete window
                          </button>
                          <button type="button" className="link" onClick={() => setDeleting(null)}>
                            Cancel
                          </button>
                        </div>
                      </div>
                    )}

                    {skipping === w.id && (
                      <form
                        className="confirm"
                        aria-label={`Skip a range of window ${w.id}`}
                        onSubmit={(e) => {
                          e.preventDefault();
                          void addSkip(w.id);
                        }}
                      >
                        <p className="small">Times in this range do not happen. Your local time.</p>
                        <div className="pair">
                          <div>
                            <label htmlFor={`skip-starts-${w.id}`}>From</label>
                            <input
                              id={`skip-starts-${w.id}`}
                              name="starts"
                              type="datetime-local"
                              required
                              value={skip.starts}
                              onChange={(e) => setSkip({ ...skip, starts: e.target.value })}
                            />
                          </div>
                          <div>
                            <label htmlFor={`skip-ends-${w.id}`}>To</label>
                            <input
                              id={`skip-ends-${w.id}`}
                              name="ends"
                              type="datetime-local"
                              required
                              value={skip.ends}
                              onChange={(e) => setSkip({ ...skip, ends: e.target.value })}
                            />
                          </div>
                        </div>
                        <label htmlFor={`skip-reason-${w.id}`}>Reason (optional)</label>
                        <input
                          id={`skip-reason-${w.id}`}
                          name="reason"
                          value={skip.reason}
                          onChange={(e) => setSkip({ ...skip, reason: e.target.value })}
                        />
                        <div className="actions">
                          <button type="submit" className="primary" disabled={busy}>
                            Skip these times
                          </button>
                          <button type="button" className="link" onClick={() => setSkipping(null)}>
                            Cancel
                          </button>
                        </div>
                      </form>
                    )}
                  </li>
                );
              })}
            </ol>
          )}

          <h2 id="window-form-title" ref={formHeading} tabIndex={-1}>
            {draft.id ? 'Edit window' : 'Add a window'}
          </h2>
          {draft.id && (
            <p className="muted small">
              Editing <span className="mono">{draft.id}</span>. Saving replaces it. Changing it to one time drops its
              skips.
            </p>
          )}
          <form
            className="window-form"
            aria-labelledby="window-form-title"
            onSubmit={(e) => {
              e.preventDefault();
              void save();
            }}
          >
            <fieldset>
              <legend>When</legend>
              {(['range', 'cron'] as const).map((k) => (
                <div className="check" key={k}>
                  <input
                    id={`kind-${k}`}
                    type="radio"
                    name="kind"
                    value={k}
                    checked={draft.kind === k}
                    onChange={() => setDraft({ ...draft, kind: k })}
                  />
                  <label htmlFor={`kind-${k}`}>{k === 'range' ? 'Once, from a start to an end' : 'Repeats on a schedule'}</label>
                </div>
              ))}
              {draft.kind === 'range' ? (
                <div className="pair">
                  <div>
                    <label htmlFor="window-starts">Starts (your local time)</label>
                    <input
                      id="window-starts"
                      name="starts"
                      type="datetime-local"
                      required
                      aria-describedby="window-starts-utc"
                      value={draft.starts}
                      onChange={(e) => setDraft({ ...draft, starts: e.target.value })}
                    />
                    <p id="window-starts-utc" className="muted small">
                      {draft.starts ? utcWords(localToUtc(draft.starts)) : 'UTC shows here.'}
                    </p>
                  </div>
                  <div>
                    <label htmlFor="window-ends">Ends (your local time)</label>
                    <input
                      id="window-ends"
                      name="ends"
                      type="datetime-local"
                      required
                      aria-describedby="window-ends-utc"
                      value={draft.ends}
                      onChange={(e) => setDraft({ ...draft, ends: e.target.value })}
                    />
                    <p id="window-ends-utc" className="muted small">
                      {draft.ends ? utcWords(localToUtc(draft.ends)) : 'UTC shows here.'}
                    </p>
                  </div>
                </div>
              ) : (
                <div className="pair">
                  <div>
                    <label htmlFor="window-cron">Schedule (cron, 5 fields, UTC)</label>
                    <input
                      id="window-cron"
                      name="cron"
                      required
                      className="mono"
                      autoCapitalize="off"
                      autoCorrect="off"
                      spellCheck={false}
                      aria-describedby="window-cron-help"
                      value={draft.cron}
                      onChange={(e) => setDraft({ ...draft, cron: e.target.value })}
                    />
                    <p id="window-cron-help" className="muted small">
                      Minute, hour, day, month, weekday. <span className="mono">0 3 * * 0</span> is Sundays at 03:00 UTC.
                    </p>
                  </div>
                  <div>
                    <label htmlFor="window-length">Length (minutes)</label>
                    <input
                      id="window-length"
                      name="lengthMins"
                      type="number"
                      inputMode="numeric"
                      min={1}
                      step={1}
                      required
                      value={draft.lengthMins}
                      onChange={(e) => setDraft({ ...draft, lengthMins: e.target.value })}
                    />
                  </div>
                </div>
              )}
            </fieldset>

            <fieldset>
              <legend>Which tenants</legend>
              {(['all', 'only', 'except'] as const).map((t) => (
                <div className="check" key={t}>
                  <input
                    id={`target-${t}`}
                    type="radio"
                    name="target"
                    value={t}
                    checked={draft.target === t}
                    onChange={() => setDraft({ ...draft, target: t })}
                  />
                  <label htmlFor={`target-${t}`}>{TARGET_WORDS[t]}</label>
                </div>
              ))}
              {draft.target !== 'all' && (
                <div className="tenant-picks" role="group" aria-label="Tenants">
                  {tenantsError && <p className="error small">The tenant list did not load: {tenantsError}</p>}
                  {choices.length === 0 && !tenantsError && <p className="muted small">The deployment has no tenants.</p>}
                  {choices.map((t) => (
                    <div className="check" key={t.id}>
                      <input
                        id={`tenant-${t.id}`}
                        type="checkbox"
                        name="tenants"
                        value={t.id}
                        checked={draft.tenants.includes(t.id)}
                        onChange={(e) => toggleTenant(t.id, e.target.checked)}
                      />
                      <label htmlFor={`tenant-${t.id}`}>
                        {t.name || t.id} <span className="mono muted small">{t.name ? t.id : 'gone'}</span>
                      </label>
                    </div>
                  ))}
                </div>
              )}
            </fieldset>

            <label htmlFor="window-message">Message</label>
            <textarea
              id="window-message"
              name="message"
              required
              rows={3}
              aria-describedby="window-message-help"
              value={draft.message}
              onChange={(e) => setDraft({ ...draft, message: e.target.value })}
            />
            <p id="window-message-help" className="muted small">
              Any text. It is shown as written, never as markdown.
            </p>

            <div className="actions">
              <button type="submit" className="primary" disabled={busy}>
                {draft.id ? 'Save window' : 'Add window'}
              </button>
              {draft.id && (
                <button type="button" className="link" disabled={busy} onClick={() => setDraft(EMPTY)}>
                  Cancel edit
                </button>
              )}
            </div>
          </form>
        </>
      )}
      {prompt}
    </section>
  );
}
