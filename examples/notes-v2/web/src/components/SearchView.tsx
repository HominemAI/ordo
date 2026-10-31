import { DeviceRegistryProvider, OrdoProvider, useOpQuery, useSpaceDevice } from '@hominemai/ordo-react';
import { useCallback, useDeferredValue, useEffect, useMemo, useState } from 'react';
import { message, useOnline } from '../hooks.js';
import { noteOf, type Note } from '../notes.js';
import { countText, search, searchWords } from '../search.js';
import type { Session } from '../session.js';
import { guestSpaces, type SharedDevice, type sharedDevices } from '../shared.js';
import { FilesProvider } from './Files.js';
import { NoteEditor } from './NoteEditor.js';
import { readableOf, SharedList, useSharedNotes, type Readable } from './ReadNotes.js';

interface Where {
  readonly space: string;
  readonly name: string | null;
}

interface Props {
  readonly session: Session;
  /** Tenants just accepted, which the door's list may not show for a moment. */
  readonly accepted: readonly Where[];
  /** The devices of the shared tenants, the same registry the shared view uses. */
  readonly devices: ReturnType<typeof sharedDevices>;
}

/**
 * Search the person's own notes and the notes shared with them: title, body
 * and tags. It runs on the device copies (`search.ts`), so results change on
 * every key and work offline for what the device holds. Each share's copy
 * holds only the rows the sharer's row guard served to it, so a search shows
 * nothing a share does not grant.
 *
 * The provider around it is the person's own, with their device: `allNotes`
 * is answered from it. Each share gets its own provider and device, as in the
 * shared view.
 */
export function SearchView({ session, accepted, devices }: Props) {
  const online = useOnline();
  const [query, setQuery] = useState('');
  // Typing stays quick on a long list: the results follow a moment behind.
  const typed = useDeferredValue(query);
  const words = searchWords(typed);
  const own = useOpQuery('allNotes', { live: true });
  const notes = useMemo(() => own.rows.map((r) => noteOf({ ...r })), [own.rows]);
  const found = useMemo(() => search(notes, typed), [notes, typed]);
  const [open, setOpen] = useState<Note | null>(null);
  const [counts, setCounts] = useState<Readonly<Record<string, number>>>({});
  const spaces = useGuestSpaces(session, accepted);

  const counted = useCallback((space: string, n: number) => {
    setCounts((c) => (c[space] === n ? c : { ...c, [space]: n }));
  }, []);
  const shared = spaces.list.reduce((sum, w) => sum + (counts[w.space] ?? 0), 0);

  if (open !== null) {
    return <NoteEditor key={open.id} note={open} online={online} onClose={() => setOpen(null)} />;
  }

  return (
    <section aria-label="Search">
      <h2>Search</h2>
      <label htmlFor="search">Search your notes and notes shared with you</label>
      <input
        id="search"
        type="search"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        placeholder="Words in a title, the text or a tag"
        enterKeyHint="search"
        autoComplete="off"
        autoCorrect="off"
        autoCapitalize="off"
        spellCheck={false}
        aria-describedby="search-count"
      />
      <p id="search-count" role="status" aria-live="polite" className="hint">
        {words.length === 0 ? 'Type a word to search.' : countText(found.length + shared, 'here')}
      </p>
      {!online && (
        <p className="banner">You are offline. This searches the notes this device holds.</p>
      )}

      {words.length > 0 && (
        <div className="card">
          <h3>Your notes</h3>
          {!own.complete && notes.length === 0 ? (
            <p className="hint">Loading your notes.</p>
          ) : (
            <OwnResults notes={found} onOpen={setOpen} />
          )}
        </div>
      )}

      {spaces.error !== null && (
        <p className="hint">The notes shared with you could not be listed: {spaces.error}</p>
      )}
      <DeviceRegistryProvider registry={devices.registry}>
        {spaces.list.map((w) => (
          <SharedResults key={w.space} session={session} where={w} query={typed} onCount={counted} />
        ))}
      </DeviceRegistryProvider>
    </section>
  );
}

/** The tenants this person is a guest in, with those just accepted. */
function useGuestSpaces(session: Session, accepted: readonly Where[]) {
  const [list, setList] = useState<Where[]>([...accepted]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    guestSpaces(session).then(
      (listed) => {
        if (!alive) return;
        const merged = new Map<string, Where>();
        for (const w of [...accepted, ...listed.map((l) => ({ space: l.space, name: l.name }))]) merged.set(w.space, w);
        setList([...merged.values()]);
        setError(null);
      },
      (e: unknown) => alive && setError(message(e)),
    );
    return () => {
      alive = false;
    };
  }, [session, accepted]);
  return { list, error };
}

function OwnResults({ notes, onOpen }: { readonly notes: readonly Note[]; readonly onOpen: (n: Note) => void }) {
  if (notes.length === 0) return <p className="hint">{countText(0, 'of yours')}</p>;
  return (
    <ul className="notes" aria-label="Your notes that match">
      {notes.map((n) => (
        <li key={n.id}>
          <button className="note" onClick={() => onOpen(n)}>
            <strong>
              {n.pinned && <span className="pin">Pinned: </span>}
              {n.title || 'Untitled'}
              {n.archived !== null && <span className="hint"> (archived)</span>}
            </strong>
            <span>{n.body.split('\n')[0]}</span>
          </button>
          {n.tags.length > 0 && <span className="tags">{n.tags.join(', ')}</span>}
        </li>
      ))}
    </ul>
  );
}

/** One share's matches, from the guest's device for that tenant. */
function SharedResults({
  session,
  where,
  query,
  onCount,
}: {
  readonly session: Session;
  readonly where: Where;
  readonly query: string;
  readonly onCount: (space: string, n: number) => void;
}) {
  const { device: open } = useSpaceDevice<SharedDevice>(where.space, session.principal);
  // A window that cannot be opened, or is closed, matches nothing.
  if (open === null) return <Counted space={where.space} n={0} onCount={onCount} />;
  return (
    <OrdoProvider client={open.hooks} device={open.device}>
      <FilesProvider>
        <SharedMatches
          readable={readableOf(open.ops())}
          where={where}
          query={query}
          onCount={onCount}
        />
      </FilesProvider>
    </OrdoProvider>
  );
}

function SharedMatches({
  readable,
  where,
  query,
  onCount,
}: {
  readonly readable: Readable;
  readonly where: Where;
  readonly query: string;
  readonly onCount: (space: string, n: number) => void;
}) {
  const { query: read, notes } = useSharedNotes(readable);
  // A share the sharer closed holds nothing, whatever the copy had.
  const found = useMemo(() => (read.revoked ? [] : search(notes, query)), [read.revoked, notes, query]);
  useEffect(() => onCount(where.space, found.length), [onCount, where.space, found.length]);
  if (found.length === 0) return null;
  return (
    <section className="card" aria-label={`From ${where.name ?? 'someone'}, matching`}>
      <h3>From {where.name ?? 'someone'}</h3>
      <SharedList notes={found} readable={readable} />
    </section>
  );
}

function Counted({ space, n, onCount }: { readonly space: string; readonly n: number; readonly onCount: (space: string, n: number) => void }) {
  useEffect(() => onCount(space, n), [onCount, space, n]);
  return null;
}
