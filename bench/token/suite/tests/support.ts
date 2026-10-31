/**
 * What both test files share: fresh addresses, and signing in through the
 * page exactly as a person does.
 */

import { expect, type Page } from '@playwright/test';
import { adapter } from '../adapters/index.js';
import { nextCode, type Outcome } from '../adapters/types.js';

const run = Date.now().toString(36);
let n = 0;

/** A new address no earlier test or run has used. */
export function fresh(who: string): string {
  return `${who}-${run}-${++n}@example.com`;
}

/** Open the page (or `path` on it) and sign in by emailed code. */
export async function signInOnPage(page: Page, email: string, path = '/'): Promise<void> {
  const a = adapter();
  await page.goto(new URL(path, a.page).toString());
  await page.getByLabel('Email', { exact: true }).fill(email);
  const before = (await a.mailbox(email)).length;
  await page.getByRole('button', { name: 'Email me a code' }).click();
  const code = await nextCode(a, email, before);
  await page.getByLabel('Code', { exact: true }).fill(code);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByText(`Signed in as ${email}`)).toBeVisible({ timeout: 60_000 });
}

export const tasksRegion = (page: Page) => page.getByRole('region', { name: 'Tasks' });
export const peopleRegion = (page: Page) => page.getByRole('region', { name: 'People' });

/** The list item in the Tasks region that holds `title`. */
export const taskItem = (page: Page, title: string) =>
  tasksRegion(page).getByRole('listitem').filter({ hasText: title });

/** The list item in the People region for `email`. */
export const personItem = (page: Page, email: string) =>
  peopleRegion(page).getByRole('listitem').filter({ hasText: email });

/** Add a task through the page and wait for it to show. */
export async function addTaskOnPage(page: Page, title: string): Promise<void> {
  await tasksRegion(page).getByLabel('New task', { exact: true }).fill(title);
  await tasksRegion(page).getByRole('button', { name: 'Add task' }).click();
  await expect(taskItem(page, title)).toBeVisible();
}

/** Unwrap an outcome the test needs to succeed. */
export function must<T>(o: Outcome<T>, what: string): T {
  if (!o.ok) throw new Error(`${what} was refused: ${o.why}`);
  return o.value;
}

/** A read that shows nothing: refused outright, or answered empty. */
export function seesNothing<T>(o: Outcome<readonly T[]>): boolean {
  return !o.ok || o.value.length === 0;
}
