/**
 * What the specs that move tenants through a rename share
 * (plan-versions.md increments 11a and 11b): the renamed program, the v2
 * page, the tenant a page calls, and a retitle with its check.
 */

import { cpSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, type Page } from '@playwright/test';
import { noteInList, signIn, signedIn, stack } from '../support.js';

/**
 * The program the move goes to: notes v3 at `version`, as `seal` and
 * `rehearsal` write it when `sealed` (notes.body sealed), always off the
 * reader line, with `title` renamed to `heading`. Made from notes v3's own
 * file so the two never drift, beside v3's `fn` code.
 */
export function renamedTitle(version: string, sealed: boolean): string {
  const v3 = readFileSync(join(stack().programV3, 'notes.ordo'), 'utf8');
  const swaps: [string, string][] = [
    ['domain notes @ 0.3.0', `domain notes @ ${version}`],
    ['reader note_viewer reads notes, notesAccess', 'reader note_viewer reads notesAccess'],
    ['  title: text\n', '  heading: text = "" prev(title)\n'],
    ['set notes { title: $title,', 'set notes { heading: $title,'],
    ['set notes { id: $id, rev: $rev, title: $title', 'set notes { id: $id, rev: $rev, heading: $title'],
    ['[id, rev, title,', '[id, rev, title: heading,'],
  ];
  if (sealed) swaps.push(['  body:  text = ""\n', '  body:  text sealed\n']);
  let source = v3;
  for (const [from, to] of swaps) {
    expect(source, `notes v3 still holds \`${from}\``).toContain(from);
    source = source.split(from).join(to);
  }
  expect(source).not.toMatch(/\btitle: \$title\b/);
  const dir = mkdtempSync(join(tmpdir(), `notes-${version}-`));
  writeFileSync(join(dir, 'notes.ordo'), source);
  cpSync(join(stack().programV3, 'fn'), join(dir, 'fn'), { recursive: true });
  return dir;
}

export async function openV2(page: Page): Promise<void> {
  await page.goto(stack().pageV2);
  const notes = page.getByLabel('New note', { exact: true });
  const settingUp = page.getByRole('heading', { name: 'Setting up your account' });
  const passkey = page.getByRole('button', { name: 'Sign in with a passkey' });
  await expect(notes.or(settingUp).or(passkey)).toBeVisible({ timeout: 30_000 });
  if (await passkey.isVisible()) await signIn(page);
  await signedIn(page);
}

/** The tenant the page sends to its door, as last seen. */
export function watchTenant(page: Page): () => string {
  let seen = '';
  page.on('request', (r) => {
    const space = r.headers()['x-ordo-space'] ?? '';
    const toDoor = new URL(r.url()).port === new URL(stack().door).port;
    if (toDoor && space && space !== 'system') seen = space;
  });
  return () => {
    expect(seen, 'the page has called its door').not.toBe('');
    return seen;
  };
}

/** Open a note, retitle it, save. A saved note closes its editor. */
export async function retitle(page: Page, title: string, next: string): Promise<void> {
  await noteInList(page, title).click();
  await page.getByRole('textbox', { name: 'Title', exact: true }).fill(next);
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByRole('textbox', { name: 'Title', exact: true })).toHaveCount(0);
  await expect(noteInList(page, next)).toBeVisible();
}

/** After a reload, a note with this title is listed. */
export async function holds(page: Page, title: string): Promise<void> {
  await expect(noteInList(page, title)).toBeVisible();
}

