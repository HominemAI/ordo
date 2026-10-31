import {
  OrdoRefusal,
  useCommit,
  useFile,
  useFileDownload,
  useFileUpload,
  useOpQuery,
} from '@hominemai/ordo-react';
import { createContext, useContext, useMemo, useRef, useState, type ReactNode } from 'react';
import { refusalText } from '../errors.js';
import { attachmentOf, attachParams, createdId, FILE_LIMIT_BYTES, filesOf, isImage, sizeText, tooLargeText, type Attachment } from '../files.js';
import { message } from '../hooks.js';

const FilesContext = createContext(false);

/**
 * Marks the screens that read a tenant's files. The bytes are read with the
 * client of the `OrdoProvider` above, so the token and the tenant are that
 * client's. A screen with no marker (one that reads no files) renders no
 * file section and reads nothing.
 */
export function FilesProvider({ children }: { readonly children: ReactNode }) {
  return <FilesContext.Provider value={true}>{children}</FilesContext.Provider>;
}

export function useFilesOn(): boolean {
  return useContext(FilesContext);
}

/** What to show for a failed file call: the door's refusal in the page's words, else the error's text. */
export function fileError(e: unknown): string {
  return e instanceof OrdoRefusal ? refusalText(e.refusal) : message(e);
}

/**
 * Every file the caller may read, from `myFiles`. A share made under v1 does
 * not list `myFiles`, so its guest is refused it: that reads as no files.
 */
export function useFiles(): { readonly files: readonly Attachment[]; readonly loading: boolean } {
  const query = useOpQuery('myFiles', { live: true });
  const files = useMemo(() => (query.refusal ? [] : query.rows.map((r) => attachmentOf({ ...r }))), [query.rows, query.refusal]);
  return { files, loading: !query.complete && !query.refusal };
}

/** An image shown in the page. Its bytes are fetched with the caller's token and shown from an object URL. */
function FileImage({ file }: { readonly file: Attachment }) {
  const shown = useFile(file.file);
  if (shown.phase === 'failed') return <span className="hint">{fileError(shown.error)}</span>;
  if (shown.url === null) return <span className="hint">Loading {file.name}.</span>;
  return <img className="attachment" src={shown.url} alt={file.name} loading="lazy" decoding="async" />;
}

/** A file offered as a download. The bytes are fetched on the click, with the caller's token. */
function FileDownload({ file }: { readonly file: Attachment }) {
  const dl = useFileDownload();
  const handle = file.file;
  return (
    <>
      <button
        disabled={dl.phase === 'working' || handle === null}
        onClick={() => {
          if (handle !== null) dl.download(handle, file.name).catch(() => {});
        }}
      >
        Download {file.name}
      </button>
      {dl.phase === 'failed' && (
        <span role="alert" className="error">
          {fileError(dl.error)}
        </span>
      )}
    </>
  );
}

interface Props {
  readonly note: string;
  /** The owner's editor: files can be added and removed. A reader's view cannot. */
  readonly editable?: boolean;
  readonly online?: boolean;
}

/**
 * The files on one note: images shown, other files as downloads. With no
 * files marker above it (a screen that reads no files), it renders nothing and
 * reads nothing.
 */
export function NoteFiles(props: Props) {
  return useFilesOn() ? <Files {...props} /> : null;
}

function Files({ note, editable = false, online = true }: Props) {
  const { files } = useFiles();
  const mine = useMemo(() => filesOf(files, note), [files, note]);
  const add = useCommit('addAttachment');
  const remove = useCommit('removeAttachment');
  const [sending, setSending] = useState(0);
  // Rows whose bytes are on their way now. A row with no bytes that is not in
  // this set was never sent, or its send failed.
  const [uploading, setUploading] = useState<ReadonlySet<string>>(new Set());
  // The files of rows that failed in this page, so "Try again" can re-send them.
  const unsent = useRef(new Map<string, File>());
  const retryFor = useRef<string | null>(null);
  const up = useFileUpload();
  const [thrown, setThrown] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const retryInput = useRef<HTMLInputElement>(null);

  if (!editable && mine.length === 0) return null;

  const mark = (id: string, on: boolean) =>
    setUploading((s) => {
      const n = new Set(s);
      if (on) n.add(id);
      else n.delete(id);
      return n;
    });

  /** Send the bytes to a row that exists. A failure keeps the row and the file for "Try again". */
  const send = async (id: string, file: File): Promise<boolean> => {
    mark(id, true);
    try {
      await up.upload({ entity: 'attachments', field: 'file', id }, file);
      unsent.current.delete(id);
      return true;
    } catch (e) {
      unsent.current.set(id, file);
      setThrown(`${file.name} was not sent. ${fileError(e)}`);
      return false;
    } finally {
      mark(id, false);
    }
  };

  const attach = async (picked: FileList | null) => {
    if (!picked || picked.length === 0) return;
    setThrown(null);
    const all = [...picked];
    // Checked before any row exists: a file over the limit creates nothing.
    const refused = all.map(tooLargeText).filter((t): t is string => t !== null);
    const list = all.filter((f) => tooLargeText(f) === null);
    if (refused.length > 0) setThrown(refused.join(' '));
    if (list.length === 0) {
      if (input.current) input.current.value = '';
      return;
    }
    setSending((n) => n + list.length);
    for (const file of list) {
      try {
        // The row first, then the bytes to it.
        const out = await add.commit(attachParams(note, file));
        if (out.status !== 'committed') continue;
        await send(createdId(out.response.rows as readonly Record<string, unknown>[]), file);
        add.reset();
      } catch (e) {
        setThrown(fileError(e));
      } finally {
        setSending((n) => n - 1);
      }
    }
    if (input.current) input.current.value = '';
  };

  const retry = (f: Attachment) => {
    setThrown(null);
    const kept = unsent.current.get(f.id);
    if (kept) {
      void send(f.id, kept);
      return;
    }
    // After a reload the page no longer holds the bytes: ask for the file again.
    retryFor.current = f.id;
    retryInput.current?.click();
  };

  const retryPicked = (picked: FileList | null) => {
    const file = picked?.[0];
    const id = retryFor.current;
    if (retryInput.current) retryInput.current.value = '';
    retryFor.current = null;
    if (!file || id === null) return;
    const big = tooLargeText(file);
    if (big !== null) setThrown(big);
    else void send(id, file);
  };

  const drop = async (file: Attachment) => {
    setThrown(null);
    try {
      await remove.commit({ id: file.id });
      unsent.current.delete(file.id);
    } catch (e) {
      setThrown(message(e));
    }
  };

  const refusal = add.refusal ?? remove.refusal;
  const error = thrown ?? (refusal ? refusalText(refusal) : null);

  return (
    <section className="files" aria-label="Files">
      {mine.length > 0 && (
        <ul className="files">
          {mine.map((f) => (
            <li key={f.id}>
              {f.file === null ? (
                uploading.has(f.id) || !editable ? (
                  <span className="hint">{f.name} is still being sent.</span>
                ) : (
                  <>
                    <span className="hint">{f.name} was not sent.</span>
                    <button disabled={!online} onClick={() => retry(f)}>
                      Try again
                    </button>
                  </>
                )
              ) : isImage(f) ? (
                <FileImage file={f} />
              ) : (
                <FileDownload file={f} />
              )}
              <span className="hint">
                {f.name}, {sizeText(f.size)}
              </span>
              {editable && (
                <button disabled={!online || remove.committing} onClick={() => void drop(f)}>
                  Remove {f.name}
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
      {editable && (
        <>
          <label htmlFor={`attach-${note}`}>Attach images or files</label>
          <p className="hint" id={`attach-hint-${note}`}>
            Each file may be up to {sizeText(FILE_LIMIT_BYTES)}.
          </p>
          <input ref={retryInput} type="file" hidden aria-label="Choose the file again" onChange={(e) => retryPicked(e.target.files)} />
          <input
            ref={input}
            id={`attach-${note}`}
            type="file"
            multiple
            aria-describedby={`attach-hint-${note}`}
            disabled={!online || sending > 0}
            onChange={(e) => void attach(e.target.files)}
          />
          {sending > 0 && (
            <p role="status" className="hint">
              Sending {sending} file{sending === 1 ? '' : 's'}.
              {up.progress?.fraction != null && (
                <>
                  {' '}
                  <progress max={1} value={up.progress.fraction} aria-label="Upload progress" />
                </>
              )}
            </p>
          )}
          {!online && <p className="hint">Adding a file needs the network.</p>}
        </>
      )}
      {error !== null && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
    </section>
  );
}
