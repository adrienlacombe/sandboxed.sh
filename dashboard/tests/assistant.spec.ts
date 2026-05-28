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
    await page.route('**/api/system/components', async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          components: [
            {
              name: 'assistant_mcp',
              version: '0.1.0',
              installed: true,
              update_available: null,
              path: '/usr/local/bin/assistant-mcp',
              status: 'ok',
            },
          ],
        }),
      });
    });
  });

  test('is a top-level navigation destination', async ({ page }) => {
    await page.goto('/');

    const sidebar = page.locator('aside');
    await sidebar.getByRole('link', { name: 'Assistant', exact: true }).click();

    await expect(page).toHaveURL(/\/assistant/);
    await expect(page.getByRole('heading', { name: 'Assistant', exact: true })).toBeVisible();
    await expect(page.getByText('assistant-mcp 0.1.0')).toBeVisible();
    await expect(page.getByRole('button', { name: /Add Gateway/i }).first()).toBeVisible();
  });

  test('shows MCP handoff warning when assistant-mcp is unavailable', async ({ page }) => {
    await page.route('**/api/system/components', async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          components: [
            {
              name: 'assistant_mcp',
              version: null,
              installed: false,
              update_available: null,
              path: null,
              status: 'missing',
            },
          ],
        }),
      });
    });

    await page.goto('/assistant');

    await expect(page.getByText('assistant-mcp not ready')).toBeVisible();
    await expect(page.getByText('Install assistant-mcp before handing mission control to Hermes.')).toBeVisible();
  });

  test('keeps the old Telegram settings route as a redirect', async ({ page }) => {
    await page.goto('/settings/telegram');

    await expect(page).toHaveURL(/\/assistant/);
    await expect(page.getByRole('heading', { name: 'Assistant', exact: true })).toBeVisible();
  });
});
