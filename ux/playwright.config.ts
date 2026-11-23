import {defineConfig} from '@playwright/test';
import {adapter} from './adapter.ts';

export default defineConfig({
  testDir: './tests',
  timeout: 60_000,
  globalTimeout: process.env.UX_ROUND ? 900_000 : 120_000,
  fullyParallel: true,
  workers: 2,
  retries: 0,
  forbidOnly: true,
  globalSetup: './tools/global-setup.ts',
  reporter: [['list'], ['./tools/slowest-reporter.ts']],
  use: {
    browserName: 'chromium',
    reducedMotion: 'reduce',
    trace: 'off',
  },
  webServer: {
    command: 'node tools/serve.ts',
    url: `http://127.0.0.1:${adapter.port}${adapter.basePath}`,
    reuseExistingServer: false,
    timeout: 15_000,
  },
});
