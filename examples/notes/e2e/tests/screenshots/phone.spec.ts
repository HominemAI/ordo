/** The sign-in screen and the notes list on a phone (Pixel 7). See screens.spec.ts. */

import { expect, test } from '@playwright/test';
import { passkeyDevice, signUp } from '../support.js';
import { matches, onlyWhereThePicturesAreMade, writeNotes } from './steady.js';

onlyWhereThePicturesAreMade();

test('screen on a phone: sign-in', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Sign up or sign in' })).toBeVisible();
  await expect(page.getByLabel('Your name')).toBeVisible();
  await matches(page, 'sign-in');
});

test('screen on a phone: notes list with tags', async ({ page }) => {
  await passkeyDevice(page);
  await signUp(page, 'Ada');
  await writeNotes(page);
  await matches(page, 'notes-list');
});
