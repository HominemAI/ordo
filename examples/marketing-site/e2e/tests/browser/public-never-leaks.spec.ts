import { test, expect } from '@playwright/test';

test('the landing page shows the public content and never the private field', async ({ page }) => {
  const bodies: Promise<string>[] = [];
  page.on('response', response => { bodies.push(response.text()); });
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Lumen Studio' })).toBeVisible();
  await expect(page.locator('#products article')).toHaveCount(10);
  await expect(page.getByRole('heading', { name: 'Aurora Lamp' })).toBeVisible();
  await expect(page.locator('#pricing article')).toHaveCount(3);
  await expect(page.getByRole('heading', { name: 'Studio', exact: true })).toBeVisible();
  await expect(page.locator('#testimonials blockquote')).toHaveCount(8);
  await expect(page.getByText('I stopped fiddling with settings the first day.')).toBeVisible();
  // All content reads have completed once their expected rows appear.
  for (const body of await Promise.all(bodies)) expect(body).not.toContain('INTERNAL-');
  await expect(page.locator('body')).not.toContainText('INTERNAL-');
});
