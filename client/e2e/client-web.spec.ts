// Playwright E2E tests for the Agent Code Flutter web client.
//
// These test the compiled WASM build in a real browser — verifying that
// assets load, the UI renders, and user interactions work as expected.
//
// Run:
//   cd client/e2e && npm install && npx playwright install chromium && npm test
//
// With pre-built assets (skip Flutter build):
//   SKIP_BUILD=1 npm test

import { test, expect } from '@playwright/test';

// ── WASM Loading & Boot ─────────────────────────────────────────

test.describe('WASM Loading', () => {
  test('index.html loads and contains Flutter bootstrap', async ({ page }) => {
    const response = await page.goto('/');
    expect(response?.status()).toBe(200);

    // The Flutter bootstrap script must be present.
    const bootstrapScript = await page.locator('script[src="flutter_bootstrap.js"]');
    await expect(bootstrapScript).toBeAttached();
  });

  test('WASM module loads without errors', async ({ page }) => {
    const consoleErrors: string[] = [];
    page.on('console', (msg) => {
      if (msg.type() === 'error') consoleErrors.push(msg.text());
    });

    await page.goto('/');
    // Wait for Flutter to finish loading — it renders content into the body.
    await page.waitForFunction(
      () => document.querySelector('flt-glass-pane') !== null,
      { timeout: 20_000 },
    );

    // Filter out known non-fatal warnings.
    const realErrors = consoleErrors.filter(
      (e) => !e.includes('service_worker') && !e.includes('favicon'),
    );
    expect(realErrors).toHaveLength(0);
  });

  test('Flutter renders within 15 seconds', async ({ page }) => {
    const startTime = Date.now();
    await page.goto('/');

    // Wait for the Flutter glass pane (the root rendering surface).
    await page.waitForFunction(
      () => document.querySelector('flt-glass-pane') !== null,
      { timeout: 15_000 },
    );

    const loadTime = Date.now() - startTime;
    console.log(`Flutter WASM loaded in ${loadTime}ms`);

    // Should load in under 15s even on slow CI.
    expect(loadTime).toBeLessThan(15_000);
  });
});

// ── UI Rendering ────────────────────────────────────────────────

test.describe('UI Rendering', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    // Wait for Flutter to render.
    await page.waitForFunction(
      () => document.querySelector('flt-glass-pane') !== null,
      { timeout: 20_000 },
    );
    // Give Flutter an extra moment to paint widgets.
    await page.waitForTimeout(2000);
  });

  test('app title "Agent Code" is visible', async ({ page }) => {
    // Flutter renders to canvas/semantics tree. Use accessibility labels.
    // The text "Agent Code" should be in the semantics tree.
    const agentCodeText = page.getByText('Agent Code');
    await expect(agentCodeText.first()).toBeVisible({ timeout: 10_000 });
  });

  test('SESSIONS header is visible in sidebar', async ({ page }) => {
    const sessions = page.getByText('SESSIONS');
    await expect(sessions).toBeVisible({ timeout: 10_000 });
  });

  test('+ New button is visible', async ({ page }) => {
    const newButton = page.getByText('+ New');
    await expect(newButton).toBeVisible({ timeout: 10_000 });
  });

  test('empty state message is shown', async ({ page }) => {
    const emptyMsg = page.getByText('Create a new session to get started');
    await expect(emptyMsg).toBeVisible({ timeout: 10_000 });
  });

  test('no sessions yet message appears', async ({ page }) => {
    const noSessions = page.getByText('No sessions yet');
    await expect(noSessions).toBeVisible({ timeout: 10_000 });
  });
});

// ── Interactions ────────────────────────────────────────────────

test.describe('Interactions', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await page.waitForFunction(
      () => document.querySelector('flt-glass-pane') !== null,
      { timeout: 20_000 },
    );
    await page.waitForTimeout(2000);
  });

  test('clicking + New shows web error message', async ({ page }) => {
    const newButton = page.getByText('+ New');
    await expect(newButton).toBeVisible({ timeout: 10_000 });
    await newButton.click();

    // On web, spawning processes is impossible — should show error.
    const errorMsg = page.getByText('Cannot spawn');
    await expect(errorMsg).toBeVisible({ timeout: 5_000 });
  });

  test('error message disappears area does not crash the app', async ({ page }) => {
    // Click + New to trigger error.
    const newButton = page.getByText('+ New');
    await newButton.click();
    await page.waitForTimeout(1000);

    // App should still be functional — SESSIONS header still visible.
    await expect(page.getByText('SESSIONS')).toBeVisible();
    await expect(page.getByText('Agent Code')).toBeVisible();
  });
});

// ── Responsive Layout ───────────────────────────────────────────

test.describe('Responsive Layout', () => {
  test('renders at 1280x720 without overflow', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 720 });
    await page.goto('/');
    await page.waitForFunction(
      () => document.querySelector('flt-glass-pane') !== null,
      { timeout: 20_000 },
    );
    await page.waitForTimeout(2000);

    // Should render without errors.
    await expect(page.getByText('SESSIONS')).toBeVisible({ timeout: 10_000 });
  });

  test('renders at 800x600 minimum', async ({ page }) => {
    await page.setViewportSize({ width: 800, height: 600 });
    await page.goto('/');
    await page.waitForFunction(
      () => document.querySelector('flt-glass-pane') !== null,
      { timeout: 20_000 },
    );
    await page.waitForTimeout(2000);

    await expect(page.getByText('SESSIONS')).toBeVisible({ timeout: 10_000 });
    await expect(page.getByText('+ New')).toBeVisible({ timeout: 10_000 });
  });
});

// ── Assets & Headers ────────────────────────────────────────────

test.describe('Assets & Headers', () => {
  test('main.dart.wasm served with correct MIME type', async ({ request }) => {
    const response = await request.get('/main.dart.wasm');
    expect(response.status()).toBe(200);
    expect(response.headers()['content-type']).toContain('application/wasm');
  });

  test('flutter_bootstrap.js is accessible', async ({ request }) => {
    const response = await request.get('/flutter_bootstrap.js');
    expect(response.status()).toBe(200);
  });

  test('flutter.js is accessible', async ({ request }) => {
    const response = await request.get('/flutter.js');
    expect(response.status()).toBe(200);
  });

  test('manifest.json is accessible', async ({ request }) => {
    const response = await request.get('/manifest.json');
    expect(response.status()).toBe(200);
  });

  test('cross-origin headers are set for WASM', async ({ request }) => {
    const response = await request.get('/');
    const headers = response.headers();
    // These are required for SharedArrayBuffer which WASM may need.
    expect(headers['cross-origin-opener-policy']).toBe('same-origin');
    expect(headers['cross-origin-embedder-policy']).toBe('require-corp');
  });
});

// ── Screenshots ─────────────────────────────────────────────────

test.describe('Visual Snapshots', () => {
  test('capture initial state screenshot', async ({ page }) => {
    await page.goto('/');
    await page.waitForFunction(
      () => document.querySelector('flt-glass-pane') !== null,
      { timeout: 20_000 },
    );
    await page.waitForTimeout(3000);

    await page.screenshot({ path: 'screenshots/initial-state.png', fullPage: true });
  });

  test('capture error state screenshot', async ({ page }) => {
    await page.goto('/');
    await page.waitForFunction(
      () => document.querySelector('flt-glass-pane') !== null,
      { timeout: 20_000 },
    );
    await page.waitForTimeout(2000);

    // Trigger error.
    const newButton = page.getByText('+ New');
    await newButton.click();
    await page.waitForTimeout(1000);

    await page.screenshot({ path: 'screenshots/error-state.png', fullPage: true });
  });
});
