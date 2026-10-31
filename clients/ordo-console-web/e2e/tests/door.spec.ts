/**
 * The link to the deployment in Chromium (docs/plan-console.md 8): sign in
 * the stack's door with a passkey, then a write at the deployment that asks
 * for the owner's own touch there, relayed by the console, then unlink.
 * The door is `ordo-host`'s own sign-in routes and console key ops
 * (crates/ordo-console/examples/e2e_door.rs).
 */

import { expect, test, type Page } from '@playwright/test';
import { person, recoverCode, savedAndContinue, shownCodes, linkAsNamedOwner } from './support';

const V1 = `entity note {
  title: text
}

op listNotes unbounded =
  note[title]
`;

const V2 = `entity note {
  title: text
  body:  text?
}

op listNotes unbounded =
  note[title, body]
`;

/** Answer the deployment's touch dialog when it is shown; it is not when a touch still counts. */
async function touchIfAsked(page: Page): Promise<void> {
  const prompt = page.getByRole('dialog', { name: 'Touch your passkey for the deployment' });
  const use = prompt.getByRole('button', { name: 'Use my passkey' });
  try {
    await use.waitFor({ state: 'visible', timeout: 3000 });
  } catch {
    return;
  }
  await use.click();
  await expect(prompt).toBeHidden();
}

test('link the deployment by sign-in, write with a relayed touch, deploy and roll back the app, unlink', async ({ browser }) => {
  const { context, page, device, problems } = await person(browser);
  // The console owner, back in with `ordo console recover`.
  await page.goto('/');
  await page.getByRole('button', { name: 'Have an invite code?' }).click();
  await page.getByLabel('Code').fill(recoverCode());
  await page.getByRole('button', { name: 'Continue with a passkey' }).click();
  await expect(page.getByRole('heading', { name: 'Your new passkey is ready' })).toBeVisible();
  await shownCodes(page);
  await savedAndContinue(page);

  // Health says the console is not linked yet.
  await expect(page.getByRole('heading', { name: "The deployment's door" })).toBeVisible();
  await expect(page.getByText('The console is not linked to the deployment yet.')).toBeVisible();

  // The owner's new passkey at the deployment, made in this page and named
  // the owner there; then the console links by sign-in.
  await page.getByRole('link', { name: 'Deployment' }).click();
  await expect(page.getByRole('heading', { name: 'Deployment', level: 1 })).toBeVisible();
  await page.getByLabel(/Name for this console's key/).fill('e2e console');
  await linkAsNamedOwner(page, device);
  const keys = page.getByRole('list', { name: 'Console keys' });
  await expect(keys.getByRole('listitem')).toHaveCount(1);
  await expect(keys.getByText('this console')).toBeVisible();

  // A write at the deployment: it asks for the owner's touch there.
  await page.getByRole('button', { name: 'Rotate the key now' }).click();
  const prompt = page.getByRole('dialog', { name: 'Touch your passkey for the deployment' });
  await expect(prompt).toBeVisible();
  await prompt.getByRole('button', { name: 'Use my passkey' }).click();
  await expect(prompt).toBeHidden();
  await expect(page.getByRole('status')).toContainText('Rotated.');
  await expect(keys.getByRole('listitem')).toHaveCount(2);
  await expect(keys.getByText('revoked')).toBeVisible();

  // Health from the door.
  await page.getByRole('link', { name: 'Health' }).click();
  const door = page.getByRole('definition').filter({ hasText: 'ready' });
  await expect(door.first()).toBeVisible();

  // Deploys (step 4): preview, apply, apply a second version, roll the app back.
  await page.getByRole('link', { name: 'Deploys' }).click();
  await expect(page.getByRole('heading', { name: 'Deploys', level: 1 })).toBeVisible();
  await expect(page.getByText('No program is stored yet.')).toBeVisible();
  const program = (name: string, text: string) =>
    page.getByLabel('Program (.ordo file)').setInputFiles({ name, mimeType: 'text/plain', buffer: Buffer.from(text) });

  await program('notes.ordo', V1);
  await page.getByRole('button', { name: 'Preview' }).click();
  // The rotation's touch has lapsed for this session or not; either way the
  // preview answers, asking for the deployment's touch when it needs one.
  await touchIfAsked(page);
  const plan = page.getByLabel('Preview');
  await expect(plan.getByText('The database is empty.')).toBeVisible();
  await expect(page.getByText('No program is stored yet.')).toBeVisible();
  await plan.getByRole('button', { name: 'Apply' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Every node serves it (1).');
  const versions = page.getByRole('list', { name: 'Stored versions' });
  await expect(versions.getByRole('listitem')).toHaveCount(1);

  await program('notes.ordo', V2);
  await page.getByRole('button', { name: 'Preview' }).click();
  await touchIfAsked(page);
  await expect(plan.getByText('safe changes only')).toBeVisible();
  await expect(plan.getByRole('list', { name: 'Schema changes' })).toContainText('body');
  await plan.getByRole('button', { name: 'Apply' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Every node serves it (1).');
  await expect(versions.getByRole('listitem')).toHaveCount(2);

  // Roll the app back to the first version.
  await versions.getByRole('button', { name: 'Roll back the app to this' }).click();
  await page.getByRole('button', { name: 'Roll back the app', exact: true }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Rolled the app back to');
  await expect(versions.getByRole('listitem').last().getByText('serving')).toBeVisible();
  await page.getByText(/Activation history/).click();
  await expect(page.getByRole('list', { name: 'Activations' }).getByRole('listitem')).toHaveCount(3);

  // Unlink. The touch given for the deploys may still count; if not, it asks.
  await page.getByRole('link', { name: 'Deployment' }).click();
  await page.getByRole('button', { name: 'Unlink' }).click();
  await page.getByRole('button', { name: 'Yes, unlink' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Unlinked.');
  await expect(page.getByRole('button', { name: 'Link with your passkey' })).toBeVisible();

  expect(problems).toEqual([]);
  await context.close();
});
