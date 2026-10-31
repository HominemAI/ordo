/**
 * What the screenshot specs share. A picture is only compared in the CI
 * runner's container (docs/howto-e2e.md, "Screenshots"). Everywhere else these
 * specs skip, because fonts and text drawing differ from machine to machine.
 *
 * The measures against false flags live here and in playwright.config.ts:
 * fixed note titles and names, the mouse parked off the page, focus removed,
 * the notes written a clear second apart so their order cannot tie, and a
 * mask over everything that changes per run.
 */

import { expect, test, type Locator, type Page } from '@playwright/test';
import { addNote, noteInList } from '../support.js';

/** Local runs skip the pictures unless E2E_SCREENSHOTS is set. */
export function onlyWhereThePicturesAreMade(): void {
  test.skip(
    !process.env['E2E_SCREENSHOTS'],
    'Pictures are made and checked in the CI runner only. Set E2E_SCREENSHOTS=1 to run them anyway.',
  );
}

/** Where a person's account id sits: the footer. */
export function perRun(page: Page): Locator[] {
  return [page.locator('footer')];
}

/**
 * Put the page in a rest state, then compare it with its picture. The picture
 * is named by the screen, and the project adds its own folder.
 */
export async function matches(page: Page, name: string, mask: Locator[] = []): Promise<void> {
  // No hover style from where the last click left the mouse, no focus ring.
  await page.mouse.move(0, 0);
  await page.evaluate(() => {
    (document.activeElement as HTMLElement | null)?.blur();
    window.scrollTo(0, 0);
  });
  await page.evaluate(() => document.fonts.ready);
  await expect(page).toHaveScreenshot(`${name}.png`, {
    fullPage: true,
    animations: 'disabled',
    caret: 'hide',
    mask: [...mask, ...perRun(page)],
    maskColor: '#ff00ff',
  });
}

/**
 * Notes with fixed titles. Each is written a clear second after the last: a
 * note is stamped to the second, and two with the same stamp could swap places.
 */
export const NOTES = [
  { title: 'Groceries', body: 'Bread, eggs, coffee.', tags: 'home' },
  { title: 'Meeting notes', body: 'Agree the plan.\nSend the summary.', tags: 'work, ideas' },
  { title: 'Reading list', body: 'Three books for the trip.', tags: 'home, ideas' },
] as const;

export async function writeNotes(page: Page, notes: readonly (typeof NOTES)[number][] = NOTES): Promise<void> {
  for (const n of notes) {
    await addNote(page, n.title, n.body, n.tags);
    await page.waitForTimeout(1_100);
  }
  await expect(noteInList(page, notes[0]!.title)).toBeVisible();
}
