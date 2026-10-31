/**
 * Attaching a file: a failed upload keeps its row with "Try again" and
 * "Remove", a file over the limit creates no row, and a row whose bytes never
 * arrived (a reload) shows the same two choices. The door is a stub.
 */

import { OrdoProvider } from '@hominemai/ordo-react';
import { act, create, type ReactTestInstance, type ReactTestRenderer } from 'react-test-renderer';
import { afterEach, describe, expect, it } from 'vitest';
import { FilesProvider, NoteFiles } from '../src/components/Files.js';
import { FILE_LIMIT_BYTES } from '../src/files.js';
import { door, hooksOver, type Reply, type Sent } from './support.js';

(globalThis as Record<string, unknown>)['IS_REACT_ACT_ENVIRONMENT'] = true;

const path = (s: Sent) => new URL(s.url).pathname;
const row = (file: string | null) => ({ id: 'att_1', rev: 1, note: 'note_1', name: 'a.txt', kind: 'text/plain', size: 3, created: '2026-10-27T10:00:00Z', file });

let tree: ReactTestRenderer | null = null;
afterEach(() => {
  act(() => tree?.unmount());
  tree = null;
});
const settle = () => act(async () => void (await new Promise((r) => setTimeout(r, 25))));

async function show(rows: () => unknown[], blob: (s: Sent) => Reply) {
  const d = door((s) => {
    const p = path(s);
    if (p === '/op/myFiles') return { json: { rows: rows() } };
    if (p === '/op/addAttachment') return { json: { rows: [{ entity: 'attachments', created: 'att_1' }] } };
    if (p === '/op/removeAttachment') return { json: { rows: [] } };
    if (p.startsWith('/blob/')) return blob(s);
    return { json: { rows: [] } };
  });
  await act(async () => {
    tree = create(
      <OrdoProvider client={hooksOver(d)}>
        <FilesProvider>
          <NoteFiles note="note_1" editable online />
        </FilesProvider>
      </OrdoProvider>,
    );
  });
  await settle();
  const root = (): ReactTestInstance => (tree as ReactTestRenderer).root;
  const flat = (n: unknown): string =>
    n === null || n === undefined ? '' : typeof n === 'string' ? n : Array.isArray(n) ? n.map(flat).join('') : flat((n as { children?: unknown }).children);
  const text = () => flat((tree as ReactTestRenderer).toJSON());
  const press = async (label: string) => {
    const b = root().findAll((n) => n.type === 'button' && n.children.join('') === label)[0];
    if (!b) throw new Error(`no button ${label}`);
    await act(async () => void (await (b.props as { onClick: () => unknown }).onClick()));
    await settle();
  };
  const pick = async (...files: Array<{ name: string; type: string; size: number }>) => {
    const i = root().findByProps({ id: 'attach-note_1' });
    await act(async () => void (await (i.props as { onChange: (e: unknown) => unknown }).onChange({ target: { files } })));
    await settle();
  };
  return { d, text, press, pick };
}

const small = { name: 'a.txt', type: 'text/plain', size: 3 };

describe('attaching a file', () => {
  it('shows the limit in the hint', async () => {
    const ui = await show(() => [], () => ({ status: 204 }));
    expect(ui.text()).toContain('up to 100.0 MB');
  });

  it('keeps a failed upload as a row with Try again and Remove; retry sends the bytes to the same row', async () => {
    let fail = true;
    const ui = await show(
      () => [row(null)],
      () => (fail ? { status: 500, json: { error: { code: 'internal', message: 'boom' } } } : { status: 204 }),
    );
    await ui.pick(small);
    expect(ui.d.sent.some((s) => path(s) === '/op/addAttachment')).toBe(true);
    expect(ui.text()).toContain('was not sent');
    expect(ui.text()).toContain('Try again');
    expect(ui.text()).toContain('Remove a.txt');
    expect(ui.text()).not.toContain('still being sent');
    fail = false;
    const adds = ui.d.sent.filter((s) => path(s) === '/op/addAttachment').length;
    await ui.press('Try again');
    const blobs = ui.d.sent.filter((s) => path(s).startsWith('/blob/'));
    expect(blobs.length).toBe(2);
    expect(blobs.every((s) => path(s) === '/blob/attachments/file/att_1')).toBe(true);
    expect(ui.d.sent.filter((s) => path(s) === '/op/addAttachment').length).toBe(adds);
  });

  it('removes the row on Remove', async () => {
    const ui = await show(() => [row(null)], () => ({ status: 204 }));
    await ui.press('Remove a.txt');
    const rm = ui.d.sent.find((s) => path(s) === '/op/removeAttachment');
    expect(JSON.stringify(rm)).toContain('att_1');
  });

  it('refuses a file over the limit in the page and creates no row', async () => {
    const ui = await show(() => [], () => ({ status: 204 }));
    await ui.pick({ name: 'big.bin', type: 'application/octet-stream', size: FILE_LIMIT_BYTES + 1 });
    expect(ui.text()).toContain('big.bin');
    expect(ui.text()).toContain('The limit is');
    expect(ui.d.sent.some((s) => path(s) === '/op/addAttachment')).toBe(false);
    expect(ui.d.sent.some((s) => path(s).startsWith('/blob/'))).toBe(false);
  });

  it('after a reload, a row with no bytes offers Try again and Remove', async () => {
    const ui = await show(() => [row(null)], () => ({ status: 204 }));
    expect(ui.text()).toContain('a.txt was not sent.');
    expect(ui.text()).toContain('Try again');
    expect(ui.text()).toContain('Remove a.txt');
    expect(ui.text()).not.toContain('still being sent');
  });
});
