import { useEffect, useState } from 'react';
import { explain } from '../api';
import { setVersionSetting, versionSettings, type RolloutSettingName, type VersionSettings } from '../deploys';
import { secsWords } from '../rollouts';

/** One setting: its full name, where its value answers, and whether it is a length. */
interface Row {
  name: RolloutSettingName;
  group: 'rollout' | 'backfill';
  key: string;
  length?: true;
  hint: string;
}

const ROWS: Row[] = [
  { name: 'rollout.strategy', group: 'rollout', key: 'strategy', hint: 'rolling, all, canary, group or query' },
  { name: 'rollout.batch_interval', group: 'rollout', key: 'batchInterval', length: true, hint: 'like 5m' },
  { name: 'rollout.window', group: 'rollout', key: 'window', length: true, hint: 'like 24h' },
  { name: 'rollout.failure_threshold', group: 'rollout', key: 'failureThreshold', hint: '1 or more' },
  { name: 'rollout.deployment_space', group: 'rollout', key: 'deploymentSpace', hint: 'first or last' },
  { name: 'rollout.canary_count', group: 'rollout', key: 'canaryCount', hint: '1 or more' },
  { name: 'rollout.canary_wait', group: 'rollout', key: 'canaryWait', length: true, hint: 'like 30m' },
  { name: 'rollout.health_error_rate', group: 'rollout', key: 'healthErrorRate', hint: 'like 1%' },
  { name: 'rollout.alert_channel', group: 'rollout', key: 'alertChannel', hint: 'stream, or a channel' },
  { name: 'rollout.share_partners', group: 'rollout', key: 'sharePartners', hint: 'on or off' },
  { name: 'rollout.group', group: 'rollout', key: 'group', hint: 'tenant ids, comma separated' },
  { name: 'rollout.query', group: 'rollout', key: 'query', hint: 'like members < 5' },
  { name: 'rollout.order', group: 'rollout', key: 'order', hint: 'an order over the tenant facts' },
  { name: 'rollout.limit', group: 'rollout', key: 'limit', hint: '1 or more' },
  { name: 'backfill.chunk_rows', group: 'backfill', key: 'chunkRows', hint: 'rows per chunk' },
  { name: 'backfill.chunk_timeout', group: 'backfill', key: 'chunkTimeout', length: true, hint: 'like 30s' },
  { name: 'backfill.max_attempts', group: 'backfill', key: 'maxAttempts', hint: 'tries per chunk' },
  { name: 'backfill.budget', group: 'backfill', key: 'budget', hint: 'chunks per run' },
  { name: 'backfill.concurrency', group: 'backfill', key: 'concurrency', hint: 'moves at once' },
  { name: 'backfill.per_database', group: 'backfill', key: 'perDatabase', hint: 'moves at once in one database' },
];

function shown(r: Row, s: VersionSettings): string {
  const v = s[r.group]?.[r.key];
  if (v === null || v === undefined || v === '') return 'not set';
  if (Array.isArray(v)) return v.length ? v.join(', ') : 'none';
  if (typeof v === 'boolean') return v ? 'on' : 'off';
  if (r.length && typeof v === 'number') return secsWords(v);
  return String(v);
}

/** The id part of a setting's name, for element ids. */
const idOf = (name: string) => name.replace(/\./g, '-');

/**
 * The rollout and backfill settings (ruled 2026-11-23, Q15): stored in Ordo
 * beside the version settings and changed through the same op as `ordo
 * rollout set` and `ordo versions set backfill.<name>`, one at a time with a
 * reason. The door checks each value; its refusal, which names the setting,
 * shows as given. A cleared setting takes its default.
 */
export function RolloutSettings(props: {
  atDeployment: <T>(f: () => Promise<T>) => Promise<T>;
  /** The screen's own runner: it shows a refusal in the screen's alert. */
  act: (f: () => Promise<void>) => Promise<void>;
  busy: boolean;
  /** Say what was done in the screen's status line. */
  onDone: (words: string) => void;
}) {
  const { act, busy } = props;
  const [settings, setSettings] = useState<VersionSettings | null>(null);
  const [editing, setEditing] = useState<RolloutSettingName | null>(null);
  const [value, setValue] = useState('');
  const [reason, setReason] = useState('');
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    // Once, on open. A failed read stays shown here: the screen's own
    // alert clears on its next action.
    void act(async () => {
      try {
        setSettings(await versionSettings());
        setLoadError(null);
      } catch (e) {
        setLoadError(explain(e));
        throw e;
      }
    });
  }, []);

  const save = (name: RolloutSettingName, clear: boolean) =>
    act(async () => {
      const r = await props.atDeployment(() => setVersionSetting(name, clear ? null : value.trim(), reason.trim()));
      setSettings(await versionSettings());
      setEditing(null);
      props.onDone(r.changed ? (clear ? `Cleared ${name}: it takes its default.` : `Set ${name}.`) : `${name} was already that.`);
    });

  return (
    <div className="rollout-settings" role="region" aria-label="Rollout settings">
      {settings === null ? (
        <p className={loadError ? 'error small' : 'muted'}>
          {loadError ? `The settings did not load: ${loadError}` : 'Loading…'}
        </p>
      ) : (
        <ul className="kv" aria-label="Settings">
          {ROWS.map((r) => (
            <li key={r.name}>
              <span className="mono">{r.name}</span>
              <span>{shown(r, settings)}</span>
              {editing !== r.name ? (
                <button
                  type="button"
                  className="link"
                  disabled={busy}
                  aria-label={`Change ${r.name}`}
                  onClick={() => {
                    setEditing(r.name);
                    setValue('');
                    setReason('');
                  }}
                >
                  Change
                </button>
              ) : (
                <form
                  className="confirm"
                  aria-label={`Change ${r.name}`}
                  onSubmit={(e) => {
                    e.preventDefault();
                    void save(r.name, false);
                  }}
                >
                  <label htmlFor={`setting-value-${idOf(r.name)}`}>New value</label>
                  <input
                    id={`setting-value-${idOf(r.name)}`}
                    name="value"
                    required
                    autoCapitalize="none"
                    autoCorrect="off"
                    spellCheck={false}
                    aria-describedby={`setting-hint-${idOf(r.name)}`}
                    value={value}
                    onChange={(e) => setValue(e.target.value)}
                  />
                  <p id={`setting-hint-${idOf(r.name)}`} className="muted small">
                    {r.hint}
                  </p>
                  <label htmlFor={`setting-reason-${idOf(r.name)}`}>Reason</label>
                  <input
                    id={`setting-reason-${idOf(r.name)}`}
                    name="reason"
                    required
                    value={reason}
                    onChange={(e) => setReason(e.target.value)}
                  />
                  <div className="actions">
                    <button type="submit" className="primary" disabled={busy}>
                      Save
                    </button>
                    <button
                      type="button"
                      disabled={busy || !reason.trim()}
                      title="A reason is needed to clear it too"
                      onClick={() => void save(r.name, true)}
                    >
                      Clear to the default
                    </button>
                    <button type="button" className="link" onClick={() => setEditing(null)}>
                      Cancel
                    </button>
                  </div>
                </form>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
