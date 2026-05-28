import { test, expect } from '@playwright/test';

test.describe('Assistant page', () => {
  test.beforeEach(async ({ page }) => {
    await page.route('**/api/health', async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ auth_required: false, auth_mode: 'disabled' }),
      });
    });
  });

  test('is a top-level navigation destination', async ({ page }) => {
    await page.goto('/');

    const sidebar = page.locator('aside');
    await sidebar.getByRole('link', { name: 'Assistant', exact: true }).click();

    await expect(page).toHaveURL(/\/assistant/);
    await expect(page.getByRole('heading', { name: 'Assistant', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: /Add Gateway/i }).first()).toBeVisible();
  });

  test('keeps the old Telegram settings route as a redirect', async ({ page }) => {
    await page.goto('/settings/telegram');

    await expect(page).toHaveURL(/\/assistant/);
    await expect(page.getByRole('heading', { name: 'Assistant', exact: true })).toBeVisible();
  });
});
