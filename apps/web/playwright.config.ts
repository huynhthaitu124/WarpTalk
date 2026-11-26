import { defineConfig, devices } from '@playwright/test';

// Ports can be moved (E2E_WEB_PORT / E2E_API_PORT) to run next to another stack on the defaults.
const WEB_PORT = Number(process.env.E2E_WEB_PORT ?? 3199);
const API_PORT = Number(process.env.E2E_API_PORT ?? 18080);
const API = `http://127.0.0.1:${API_PORT}`;
const baseURL = `http://127.0.0.1:${WEB_PORT}`;
// Static export smoke test: `next build` -> out/ served by scripts/serve-out.mjs.
const EXPORT_PORT = Number(process.env.E2E_EXPORT_PORT ?? 3198);
const exportURL = `http://127.0.0.1:${EXPORT_PORT}`;

// Optional override when the bundled browser build does not match the installed Playwright:
//   PW_CHROMIUM=/opt/pw-browsers/chromium npx playwright test
const executablePath = process.env.PW_CHROMIUM || undefined;
const launchOptions = executablePath ? { executablePath } : {};

export default defineConfig({
  testDir: './e2e/tests',
  outputDir: './test-results',
  snapshotPathTemplate: '{testDir}/../__screenshots__/{projectName}/{testFileName}/{arg}{ext}',
  fullyParallel: true,
  workers: process.env.CI ? 2 : 3,
  retries: process.env.CI ? 1 : 0,
  timeout: 45_000,
  expect: { timeout: 10_000, toHaveScreenshot: { maxDiffPixelRatio: 0.01, animations: 'disabled', caret: 'hide' } },
  reporter: [['list'], ['html', { open: 'never', outputFolder: 'playwright-report' }]],
  use: {
    baseURL,
    trace: 'off',
    screenshot: 'only-on-failure',
    video: 'off',
    reducedMotion: 'reduce',
    locale: 'en-US',
    timezoneId: 'UTC',
  },
  projects: [
    {
      name: 'chromium',
      testIgnore: /export\.spec\.ts/,
      use: { ...devices['Desktop Chrome'], launchOptions },
    },
    {
      // Pre-join camera/mic preview: Chromium's fake devices + auto-granted permission prompt.
      name: 'meeting-media',
      testMatch: /(meeting-media|voice-media)\.spec\.ts/,
      use: {
        ...devices['Desktop Chrome'],
        launchOptions: { ...launchOptions, args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream'] },
      },
    },
    {
      // The bundled macOS build: deep links on the static export (own server, see webServer below).
      name: 'export',
      testMatch: /export\.spec\.ts/,
      use: { ...devices['Desktop Chrome'], baseURL: exportURL, launchOptions },
    },
    {
      // The macOS app embeds this UI in a WKWebView at 1280x800; run the shell-level specs there.
      name: 'macos-webview',
      testMatch: /(webview|visual|a11y)\.spec\.ts/,
      use: {
        ...devices['Desktop Chrome'],
        viewport: { width: 1280, height: 800 },
        userAgent: `${devices['Desktop Chrome'].userAgent} WarpTalkMac/1.0 WKWebView`,
        launchOptions,
      },
    },
  ],
  webServer: [
    {
      // Real Rust backend: dev services + migrator + warptalk-api on :18080 (LiveKit unset).
      command: `E2E_API_BIND=127.0.0.1:${API_PORT} bash ../../scripts/e2e-backend.sh`,
      url: `${API}/health`,
      timeout: 15 * 60_000,
      reuseExistingServer: !process.env.CI,
      stdout: 'ignore',
      stderr: 'pipe',
      gracefulShutdown: { signal: 'SIGTERM', timeout: 5000 },
    },
    {
      // `next dev` with the test harness bridge on; /api is rewritten to the e2e backend.
      command: `NEXT_PUBLIC_E2E_HARNESS=1 NEXT_DIST_DIR=.next-e2e API_PROXY_TARGET=${API} npx next dev -H 127.0.0.1 -p ${WEB_PORT}`,
      url: baseURL,
      timeout: 120_000,
      reuseExistingServer: !process.env.CI,
      stdout: 'ignore',
      stderr: 'pipe',
    },
    {
      command: `NEXT_DIST_DIR=.next-export npx next build && API_PROXY_TARGET=${API} node scripts/serve-out.mjs --port ${EXPORT_PORT}`,
      url: exportURL,
      timeout: 10 * 60_000, // a full `next build` of the ported legacy UI takes ~5 min on hosted runners
      reuseExistingServer: !process.env.CI,
      stdout: 'ignore',
      stderr: 'pipe',
    },
  ],
});
