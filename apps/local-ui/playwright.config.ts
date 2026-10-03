import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: './tests',
  workers: 1,
  timeout: 60000,
  expect: { timeout: 20000 },
  use: {
    browserName: 'chromium',
    ...(process.platform === 'darwin' ? { channel: 'chrome' } : {}),
    viewport: { width: 1360, height: 900 },
    trace: 'retain-on-failure',
  },
  outputDir: '../../artifacts/browser-tests',
  reporter: 'list',
});
