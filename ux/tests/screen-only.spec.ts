import {test, expect} from '@playwright/test';
import {mkdirSync} from 'node:fs';
import {join} from 'node:path';
import {adapter, url} from '../adapter.ts';
import {tasks, outDir} from '../lib/scenarios.ts';
import {record, median, p95, type Row} from '../lib/record.ts';
import {play} from '../lib/screen-only.ts';
import {setState} from '../lib/probes.ts';

for (const scenario of tasks()) {
  for (const viewport of adapter.gatingViewports) {
    test(`L3 screen-only user ${scenario.name} ${viewport.name}`, async ({page}) => {
      const rows: Row[] = [];
      const misses: string[] = [];
      for (const theme of adapter.themes) {
        await page.setViewportSize({width: viewport.width, height: viewport.height});
        await page.goto(url(scenario.start), {waitUntil: 'load'});
        await setState(page, viewport, theme);
        const run = await play(page, scenario);
        let shot: string | undefined;
        if (run.reached) {
          await page.waitForTimeout(750);
          const dir = join(outDir, 'shots');
          mkdirSync(dir, {recursive: true});
          shot = join(dir, `task-${scenario.name}-${viewport.name}-${theme}.png`);
          await page.screenshot({path: shot});
        } else {
          misses.push(`${theme}: ${run.trail.join(' | ')}`);
        }
        rows.push({
          scenario: scenario.name,
          page: scenario.truth.path,
          viewport: viewport.name,
          theme,
          gating: true,
          metrics: {
            completion_miss: run.reached ? 0 : 1,
            'screen_only.actions': run.actions,
            'screen_only.looks': run.looks,
            'screen_only.steers': run.steers,
            'screen_only.time_to_act_steps.max': run.steps,
            'glance_cost.median': median(run.glances),
            'glance_cost.p95': p95(run.glances),
          },
          details: {trail: run.trail, glances: run.glances},
          shot,
        });
      }
      record(`task-${scenario.name}-${viewport.name}`, rows);
      expect(misses, 'gates 3 and 4: the screen-only user reaches the truth page').toEqual([]);
    });
  }
}
