/**
 * The page, as a person uses it: spec/app.md §3 and §4. Every locator is a
 * label, a role or an exact text the spec names, so one suite fits every
 * stack.
 */

import { expect, test, type Browser, type Page } from '@playwright/test';
import { adapter } from '../adapters/index.js';
import { addTaskOnPage, fresh, peopleRegion, personItem, signInOnPage, taskItem, tasksRegion } from './support.js';

const picker = (page: Page) => page.getByLabel('Team', { exact: true });
const chosenTeam = (page: Page) => picker(page).locator('option:checked');

/** An owner and a member who joined through the page. Each on their own page. */
async function teamWithMember(browser: Browser) {
  const ownerEmail = fresh('owner');
  const memberEmail = fresh('member');
  const owner = await (await browser.newContext()).newPage();
  await signInOnPage(owner, ownerEmail);
  await peopleRegion(owner).getByRole('button', { name: 'Invite member' }).click();
  const linkField = peopleRegion(owner).getByLabel('Invite link', { exact: true });
  await expect(linkField).toHaveValue(/^https?:\/\//);
  const link = await linkField.inputValue();
  const member = await (await browser.newContext()).newPage();
  await member.goto(link);
  await expect(member.getByText('Sign in to join the team.')).toBeVisible();
  await signInOnPage(member, memberEmail, link);
  return { owner, member, ownerEmail, memberEmail, link };
}

test('sign-in: a new person signs in by emailed code and owns a team of their own', async ({ page }) => {
  const email = fresh('new');
  await signInOnPage(page, email);
  await expect(picker(page)).toBeVisible();
  await expect(chosenTeam(page)).toHaveText(email);
  await expect(page.getByText('Your role: owner')).toBeVisible();
  await expect(tasksRegion(page)).toBeVisible();
  await expect(personItem(page, email)).toContainText('owner');
});

test('sign-in: a wrong code is refused and the person stays signed out', async ({ page }) => {
  const email = fresh('wrong');
  await page.goto(adapter().page);
  await page.getByLabel('Email', { exact: true }).fill(email);
  await page.getByRole('button', { name: 'Email me a code' }).click();
  await page.getByLabel('Code', { exact: true }).fill('000000');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.waitForTimeout(2_000);
  await expect(page.getByText(`Signed in as ${email}`)).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Sign in' })).toBeVisible();
});

test('sign-in: a reload keeps the person signed in, on the same team', async ({ browser }) => {
  const { member, ownerEmail, memberEmail } = await teamWithMember(browser);
  await expect(chosenTeam(member)).toHaveText(ownerEmail);
  await member.reload();
  await expect(member.getByText(`Signed in as ${memberEmail}`)).toBeVisible({ timeout: 30_000 });
  await expect(chosenTeam(member)).toHaveText(ownerEmail);
  await expect(member.getByText('Your role: member')).toBeVisible();
});

test('sign-out: the signed-out screen shows, and a reload stays signed out', async ({ page }) => {
  const email = fresh('leaver');
  await signInOnPage(page, email);
  await page.getByRole('button', { name: 'Sign out' }).click();
  await expect(page.getByLabel('Email', { exact: true })).toBeVisible();
  await page.reload();
  await expect(page.getByLabel('Email', { exact: true })).toBeVisible({ timeout: 30_000 });
  await expect(page.getByText(`Signed in as ${email}`)).toHaveCount(0);
});

test('tasks: add, rename, mark done and delete a task, each kept after a reload', async ({ page }) => {
  await signInOnPage(page, fresh('tasker'));
  const first = `Water the ferns ${Date.now()}`;
  const renamed = `${first} again`;
  await addTaskOnPage(page, first);
  await page.reload();
  await expect(taskItem(page, first)).toBeVisible({ timeout: 30_000 });

  await taskItem(page, first).getByRole('button', { name: 'Edit' }).click();
  await tasksRegion(page).getByLabel('Title', { exact: true }).fill(renamed);
  await tasksRegion(page).getByRole('button', { name: 'Save' }).click();
  await expect(taskItem(page, renamed)).toBeVisible();
  await page.reload();
  await expect(taskItem(page, renamed)).toBeVisible({ timeout: 30_000 });

  const done = taskItem(page, renamed).getByRole('checkbox', { name: 'Done' });
  await expect(done).not.toBeChecked();
  // A click, not check(): the page may tick the box only once the change is saved.
  await done.click();
  await expect(done).toBeChecked();
  await page.reload();
  await expect(taskItem(page, renamed).getByRole('checkbox', { name: 'Done' })).toBeChecked({ timeout: 30_000 });

  await taskItem(page, renamed).getByRole('button', { name: 'Delete' }).click();
  await expect(taskItem(page, renamed)).toHaveCount(0);
  await page.reload();
  await expect(tasksRegion(page)).toBeVisible({ timeout: 30_000 });
  await expect(taskItem(page, renamed)).toHaveCount(0);
});

test('tasks: assign a task to a team member and back to nobody, kept after a reload', async ({ browser }) => {
  const { owner, memberEmail } = await teamWithMember(browser);
  await owner.reload();
  await expect(personItem(owner, memberEmail)).toBeVisible({ timeout: 30_000 });
  const title = `Book the room ${Date.now()}`;
  await addTaskOnPage(owner, title);
  const who = () => taskItem(owner, title).getByLabel('Assignee', { exact: true });
  await expect(who().locator('option:checked')).toHaveText('Nobody');
  await who().selectOption({ label: memberEmail });
  await expect(who().locator('option:checked')).toHaveText(memberEmail);
  await owner.reload();
  await expect(who().locator('option:checked')).toHaveText(memberEmail, { timeout: 30_000 });
  await who().selectOption({ label: 'Nobody' });
  await expect(who().locator('option:checked')).toHaveText('Nobody');
  await owner.reload();
  await expect(who().locator('option:checked')).toHaveText('Nobody', { timeout: 30_000 });
});

test('people: the owner is listed as owner and sees Invite member, and no Remove for themselves', async ({ page }) => {
  const email = fresh('boss');
  await signInOnPage(page, email);
  await expect(personItem(page, email)).toContainText('owner');
  await expect(peopleRegion(page).getByRole('button', { name: 'Invite member' })).toBeVisible();
  await expect(personItem(page, email).getByRole('button', { name: 'Remove' })).toHaveCount(0);
});

test('joining: an invitation link brings a new person in as a member of the inviting team', async ({ browser }) => {
  const { member, ownerEmail, memberEmail } = await teamWithMember(browser);
  await expect(chosenTeam(member)).toHaveText(ownerEmail);
  await expect(member.getByText('Your role: member')).toBeVisible();
  // A new person also owns a team of their own.
  await expect
    .poll(async () => (await picker(member).locator('option').allTextContents()).map((t) => t.trim()).sort())
    .toEqual([ownerEmail, memberEmail].sort());
  await expect(personItem(member, ownerEmail)).toContainText('owner');
  await expect(personItem(member, memberEmail)).toContainText('member');
});

test("joining: a member sees the team's tasks, and neither Invite member nor Remove", async ({ browser }) => {
  const { owner, member, ownerEmail } = await teamWithMember(browser);
  const title = `Owner's task ${Date.now()}`;
  await addTaskOnPage(owner, title);
  await member.reload();
  await expect(chosenTeam(member)).toHaveText(ownerEmail, { timeout: 30_000 });
  await expect(taskItem(member, title)).toBeVisible();
  await expect(peopleRegion(member).getByRole('button', { name: 'Invite member' })).toHaveCount(0);
  await expect(peopleRegion(member).getByRole('button', { name: 'Remove' })).toHaveCount(0);
});

test('joining: a task a member adds is seen by the owner after a reload', async ({ browser }) => {
  const { owner, member } = await teamWithMember(browser);
  const title = `Member's task ${Date.now()}`;
  await addTaskOnPage(member, title);
  await owner.reload();
  await expect(taskItem(owner, title)).toBeVisible({ timeout: 30_000 });
});

test('removing: the owner removes a member, and after a reload the team is gone from their picker', async ({ browser }) => {
  const { owner, member, ownerEmail, memberEmail } = await teamWithMember(browser);
  const title = `Secret plan ${Date.now()}`;
  await addTaskOnPage(owner, title);
  await owner.reload();
  await personItem(owner, memberEmail).getByRole('button', { name: 'Remove' }).click();
  await expect(personItem(owner, memberEmail)).toHaveCount(0);
  await owner.reload();
  await expect(personItem(owner, ownerEmail)).toBeVisible({ timeout: 30_000 });
  await expect(personItem(owner, memberEmail)).toHaveCount(0);

  await member.reload();
  await expect(member.getByText(`Signed in as ${memberEmail}`)).toBeVisible({ timeout: 30_000 });
  await expect(picker(member).locator('option', { hasText: ownerEmail })).toHaveCount(0);
  await expect(taskItem(member, title)).toHaveCount(0);
});
