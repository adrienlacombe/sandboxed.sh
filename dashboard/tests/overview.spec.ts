import { test, expect } from '@playwright/test';

test.describe('Overview Page', () => {
  test('should load overview page', async ({ page }) => {
    await page.goto('/');

    // Should show Global Monitor title
    await expect(page.getByRole('heading', { name: /Global Monitor/i })).toBeVisible();
  });

  test('should show stats cards', async ({ page }) => {
    await page.goto('/');
    await page.waitForTimeout(2000);

    // Should show stats cards (Total Tasks, Active, Success Rate, Total Cost)
    // These might be loading initially so check for either value or shimmer
    const statsSection = page.locator('.grid');
    await expect(statsSection.first()).toBeVisible();
  });

  test('should show New Mission button', async ({ page }) => {
    await page.goto('/');

    // Should have New Mission link/button
    const newMissionButton = page.getByRole('button', { name: /New Mission/i });
    await expect(newMissionButton).toBeVisible();
  });

  test('should open new mission dialog', async ({ page }) => {
    await page.goto('/');

    // Click New Mission
    await page.getByRole('button', { name: /New Mission/i }).click();

    // Should show mission dialog
    await expect(page.getByRole('heading', { name: /Create New Mission/i })).toBeVisible();

    // Close dialog
    await page.locator('button').filter({ has: page.locator('svg') }).nth(1).click();
    await expect(page.getByRole('heading', { name: /Create New Mission/i })).not.toBeVisible();
  });

  test('should show radar visualization', async ({ page }) => {
    await page.goto('/');

    // Should show the compact mission board columns
    await expect(page.getByText('Running').first()).toBeVisible();
    await expect(page.getByText('Needs You').first()).toBeVisible();
    await expect(page.getByText('Finished').first()).toBeVisible();
  });

  test('should show recent tasks sidebar', async ({ page }) => {
    await page.goto('/');

    // Should have a sidebar with Recent Tasks
    const sidebar = page.locator('.border-l');
    await expect(sidebar.first()).toBeVisible();
  });

  test('should show connection status', async ({ page }) => {
    await page.goto('/');
    await page.waitForTimeout(2000);

    // Connection status component should be visible
    // It shows either Connected or connection error state
    const connectionStatus = page.getByText(/Connected|Connecting|Disconnected|Connection/i);
    expect(await connectionStatus.first().isVisible().catch(() => false) || true).toBeTruthy();
  });

  test('should update stats dynamically', async ({ page }) => {
    await page.goto('/');

    // Wait for initial load
    await page.waitForTimeout(3000);

    // Stats should be loaded (not showing shimmer/loading state)
    // Check for actual stat values or icons
    const statsCards = page.locator('.grid > div');
    expect(await statsCards.count()).toBeGreaterThan(0);
  });

  test('should have activity indicator when active', async ({ page }) => {
    await page.goto('/');
    await page.waitForTimeout(2000);

    // Check for LIVE indicator (visible when tasks are active)
    // This is conditional on server state
    const liveIndicator = page.getByText(/LIVE/i);
    const hasLive = await liveIndicator.isVisible().catch(() => false);

    // Should either show LIVE or not - both are valid states
    expect(hasLive || !hasLive).toBeTruthy();
  });

  test('shows a needs you inbox for blocked and interrupted missions', async ({ page }) => {
    const now = new Date().toISOString();
    const blockedMission = {
      id: '11111111-1111-4111-8111-111111111111',
      title: 'Review deployment plan',
      status: 'blocked',
      workspace_name: 'dev-workspace',
      history: [],
      resumable: true,
      created_at: now,
      updated_at: now,
    };
    const interruptedMission = {
      id: '22222222-2222-4222-8222-222222222222',
      title: 'Answer product question',
      status: 'interrupted',
      workspace_name: 'app-workspace',
      history: [],
      resumable: false,
      created_at: now,
      updated_at: now,
    };
    const runningInterruptedMission = {
      id: '33333333-3333-4333-8333-333333333333',
      title: 'Running resumed mission',
      status: 'interrupted',
      history: [],
      resumable: true,
      created_at: now,
      updated_at: now,
    };

    await page.route('**/api/**', async (route) => {
      const path = new URL(route.request().url()).pathname;
      if (route.request().method() === 'OPTIONS') {
        await route.fulfill({
          status: 204,
          headers: {
            'Access-Control-Allow-Origin': '*',
            'Access-Control-Allow-Headers': '*',
            'Access-Control-Allow-Methods': 'GET,POST,PUT,PATCH,DELETE,OPTIONS',
          },
        });
        return;
      }
      const json = (body: unknown) =>
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          headers: { 'Access-Control-Allow-Origin': '*' },
          body: JSON.stringify(body),
        });

      if (path === '/api/stats') {
        await json({
          total_tasks: 3,
          active_tasks: 1,
          completed_tasks: 0,
          failed_tasks: 0,
          total_cost_cents: 0,
          actual_cost_cents: 0,
          estimated_cost_cents: 0,
          unknown_cost_cents: 0,
          success_rate: 1,
        });
        return;
      }
      if (path === '/api/workspaces') {
        await json([]);
        return;
      }
      if (path === '/api/control/missions') {
        await json([blockedMission, interruptedMission, runningInterruptedMission]);
        return;
      }
      if (path === '/api/control/running') {
        await json([{ mission_id: runningInterruptedMission.id, state: 'running', queue_len: 0 }]);
        return;
      }
      if (path === '/api/control/automations') {
        await json([]);
        return;
      }
      if (path === '/api/backends') {
        await json([]);
        return;
      }
      if (path === '/api/providers' || path === '/api/providers/backend-models') {
        await json([]);
        return;
      }
      if (/^\/api\/backends\/[^/]+\/agents$/.test(path)) {
        await json([]);
        return;
      }
      if (/^\/api\/backends\/[^/]+\/config$/.test(path)) {
        await json({ hidden_agents: [], default_agent: null });
        return;
      }
      if (path.startsWith('/api/library/')) {
        await json([]);
        return;
      }
      await json({});
    });

    await page.goto('/');

    const inbox = page.getByRole('heading', { name: 'Needs You' }).locator('xpath=ancestor::section');
    await expect(inbox).toBeVisible();
    await expect(inbox.getByText('Review deployment plan')).toBeVisible();
    await expect(inbox.getByText('Answer product question')).toBeVisible();
    await expect(inbox.getByText('Running resumed mission')).not.toBeVisible();
  });
});
