import { DeviceRegistryProvider, OrdoProvider, useSpaceDevice } from '@hominemai/ordo-react';
import { useEffect, useState } from 'react';
import { message } from '../hooks.js';
import { FilesProvider } from './Files.js';
import { readableOf, SharedList, useSharedNotes, type Readable } from './ReadNotes.js';
import type { Session } from '../session.js';
import { guestSpaces, leave, type SharedDevice, type sharedDevices } from '../shared.js';

interface Where {
  readonly space: string;
  readonly name: string | null;
}

interface Props {
  readonly session: Session;
  /** Tenants just accepted, which the door's list may not show for a moment. */
  readonly accepted: readonly Where[];
  /** The devices of the shared tenants, opened here and closed when this view goes. */
  readonly devices: ReturnType<typeof sharedDevices>;
}

/** Notes other people shared with this person. Read-only, and closable by the sharer. */
export function SharedView({ session, accepted, devices }: Props) {
  const [spaces, setSpaces] = useState<Where[]>([...accepted]);
  const [error, setError] = useState<string | null>(null);
  const [gone, setGone] = useState<ReadonlySet<string>>(new Set());

  useEffect(() => {
    let alive = true;
    guestSpaces(session).then(
      (listed) => {
        if (!alive) return;
        const merged = new Map<string, Where>();
        for (const w of [...accepted, ...listed.map((l) => ({ space: l.space, name: l.name }))]) {
          merged.set(w.space, w);
        }
        setSpaces([...merged.values()]);
      },
      (e: unknown) => alive && setError(message(e)),
    );
    return () => {
      alive = false;
    };
  }, [session, accepted]);

  const visible = spaces.filter((s) => !gone.has(s.space));

  return (
    <section aria-label="Shared with you">
      <h2>Shared with you</h2>
      {error !== null && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      {visible.length === 0 && <p className="hint">Nobody has shared a note with you yet.</p>}
      <DeviceRegistryProvider registry={devices.registry}>
        {visible.map((w) => (
          <SharedSpace
            key={w.space}
            session={session}
            where={w}
            onLeft={() => setGone((g) => new Set([...g, w.space]))}
          />
        ))}
      </DeviceRegistryProvider>
    </section>
  );
}

function SharedSpace({
  session,
  where,
  onLeft,
}: {
  readonly session: Session;
  readonly where: Where;
  readonly onLeft: () => void;
}) {
  // One device per shared tenant, from the registry: opened by the first
  // screen to ask, closed when none does.
  const { device: open, error: openError } = useSpaceDevice<SharedDevice>(where.space, session.principal);
  const [closedByHook, setClosed] = useState(false);
  const [thrown, setError] = useState<string | null>(null);
  // A window that cannot be opened is a share that was closed.
  const closed = closedByHook || openError !== null;
  const error = thrown ?? (openError === null ? null : message(openError));

  const stop = async () => {
    try {
      await leave(session, where.space);
      await open?.device.purge();
      onLeft();
    } catch (e) {
      setError(message(e));
    }
  };

  return (
    <div className="card">
      <h3>From {where.name ?? 'someone'}</h3>
      {closed ? (
        <p className="hint">This share was closed. What it showed has been removed from this device.</p>
      ) : open === null ? (
        <p className="hint">Loading.</p>
      ) : (
        <OrdoProvider client={open.hooks} device={open.device}>
          <FilesProvider>
            <SharedNotes readable={readableOf(open.ops())} onClosed={() => setClosed(true)} />
          </FilesProvider>
        </OrdoProvider>
      )}
      <div className="row">
        <button onClick={() => void stop()}>Stop seeing these notes</button>
      </div>
      {error !== null && !closed && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
    </div>
  );
}

/** The notes of one window, read from the guest's device, pinned first, with their files. */
function SharedNotes({ readable, onClosed }: { readonly readable: Readable; readonly onClosed: () => void }) {
  const { query, notes } = useSharedNotes(readable);
  // The sharer closing the window shows up as a feed the door refuses. The
  // hook says it was revoked, the copy has poisoned the replica, and so this
  // screen holds nothing.
  useEffect(() => {
    if (query.revoked) onClosed();
  }, [query.revoked, onClosed]);

  if (notes.length === 0) {
    return <p className="hint">{query.complete ? 'No note is shared with you here now.' : 'Loading.'}</p>;
  }
  return <SharedList notes={notes} readable={readable} />;
}
