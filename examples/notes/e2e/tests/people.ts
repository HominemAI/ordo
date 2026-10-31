/**
 * People for the flows that run in every browser. In Chromium a person comes
 * in with a virtual passkey. The virtual authenticator is Chrome DevTools
 * only, so Firefox and Safari (WebKit) people come in by an emailed code,
 * which every browser can do. Both land on the same signed-in screen.
 */

import { chromium, expect, type Browser, type BrowserContext, type BrowserContextOptions, type Page } from '@playwright/test';
import {
  codeFor,
  noteInList,
  passkeyDevice,
  person,
  signIn,
  signUp,
  signedIn,
  stack,
  unique,
} from './support.js';

/** Sign in with an emailed code. `n` is how many codes this address has had. */
export async function signInByEmail(page: Page, address: string, n: number): Promise<void> {
  await page.getByLabel('Email').fill(address);
  await page.getByRole('button', { name: 'Email me a code' }).click();
  await expect(page.getByText(`If ${address} can sign in here, a code is on its way.`)).toBeVisible();
  await page.getByLabel('Code').fill(await codeFor(address, n));
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await signedIn(page);
}

export interface Joined {
  readonly context: BrowserContext;
  readonly page: Page;
  /** Sign in again after a reload or a sign-out, the way this person came in. */
  readonly signInAgain: () => Promise<void>;
}

/** A new person, signed up and in, by passkey in Chromium and by emailed code elsewhere. */
export async function join(browser: Browser, display: string): Promise<Joined> {
  if (browser.browserType().name() === 'chromium') {
    const { context, page } = await person(browser);
    await signUp(page, display);
    return { context, page, signInAgain: () => signIn(page) };
  }
  const context = await browser.newContext();
  const page = await context.newPage();
  const address = `${unique(display.toLowerCase())}@example.org`;
  let sent = 0;
  await page.goto('/');
  await signInByEmail(page, address, sent++);
  return { context, page, signInAgain: () => signInByEmail(page, address, sent++) };
}

/**
 * A person who signs in by emailed code in every browser, for a flow that
 * needs the same person in two tabs (a virtual passkey belongs to one tab).
 */
export async function joinByEmail(browser: Browser, display: string) {
  const context = await browser.newContext();
  const page = await context.newPage();
  const address = `${unique(display.toLowerCase())}@example.org`;
  await page.goto('/');
  await signInByEmail(page, address, 0);
  /**
   * Another tab of the same person. It shares the first tab's renewal
   * cookie, so it opens signed in with no code: every tab of one person
   * stays signed in (owner ruling 2026-10-21). A sign-in screen is a failure.
   */
  const anotherTab = async (): Promise<Page> => {
    const tab = await context.newPage();
    await tab.goto('/');
    await signedIn(tab);
    await expect(tab.getByLabel('Email')).toHaveCount(0);
    return tab;
  };
  return { context, page, anotherTab };
}

/**
 * The person who owns a note and shares it. Sharing asks for a fresh passkey
 * touch, so the owner is always a Chromium person with a virtual passkey,
 * whatever browser the project runs. The reader is the project's browser.
 */
export async function owner(display: string, device: BrowserContextOptions = {}) {
  const browser = await chromium.launch();
  const context = await browser.newContext({ baseURL: stack().page, ignoreHTTPSErrors: true, ...device });
  const page = await context.newPage();
  await passkeyDevice(page);
  await signUp(page, display);
  return {
    context,
    page,
    close: async () => {
      await context.close();
      await browser.close();
    },
  };
}

/** The owner opens the note, makes an invitation, and gives back the link. */
export async function invitationFor(page: Page, title: string): Promise<string> {
  await noteInList(page, title).click();
  await page.getByRole('button', { name: 'Share', exact: true }).click();
  await page.getByRole('button', { name: 'Make an invitation for a person' }).click();
  const invitation = page.getByLabel('Invitation to send');
  await expect(invitation).toHaveValue(/#\/accept\//);
  return invitation.inputValue();
}

/** The "Shared with you" list. */
export function sharedList(page: Page) {
  return page.getByRole('region', { name: 'Shared with you' });
}

/** The reader opens the invitation and agrees. */
export async function acceptInvitation(page: Page, link: string, name: string, title: string): Promise<void> {
  await page.goto(link);
  await expect(page.getByRole('heading', { name: 'Someone shared a note with you' })).toBeVisible();
  await page.getByLabel('The name to show them').fill(name);
  await page.getByRole('button', { name: 'Agree and read it' }).click();
  await expect(sharedList(page).getByText(title)).toBeVisible({ timeout: 30_000 });
}
