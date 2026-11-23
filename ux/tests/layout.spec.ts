import {test, expect} from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import {mkdirSync} from 'node:fs';
import {join} from 'node:path';
import {adapter, url} from '../adapter.ts';
import {sitemapPages, outDir} from '../lib/scenarios.ts';
import {record, type Row} from '../lib/record.ts';
import {alwaysVisibleMisses, keyboardUnreachable, overflow, scrollToReading, setState} from '../lib/probes.ts';

const keyboardPages = new Set(['', 'docs/getting-started']);

for (const path of sitemapPages()) {
  test(`L2 layout contract /${path}`, async ({page}) => {
    const rows: Row[] = [];
    const failures: string[] = [];
    await page.goto(url(path), {waitUntil: 'load'});
    for (const viewport of [...adapter.gatingViewports, ...(process.env.UX_ROUND ? adapter.measuredViewports : [])]) {
      const gating = adapter.gatingViewports.includes(viewport);
      for (const theme of adapter.themes) {
        await setState(page, viewport, theme);
        const layout = await overflow(page);
        const misses = await alwaysVisibleMisses(page, viewport, path);
        await scrollToReading(page);
        const missesScrolled = (await alwaysVisibleMisses(page, viewport, path)).map((m) => `after scroll: ${m}`);
        await page.evaluate(() => window.scrollTo(0, 0));
        const metrics: Record<string, number> = {
          'layout.h_scroll': layout.horizontalScroll ? 1 : 0,
          'layout.overflow_elements': layout.offenders.length,
          'always_visible.misses': misses.length + missesScrolled.length,
        };
        const details: Record<string, unknown> = {offenders: layout.offenders, misses: [...misses, ...missesScrolled]};
        if (gating) {
          if (keyboardPages.has(path)) {
            const unreachable = await keyboardUnreachable(page);
            metrics['keyboard.unreachable'] = unreachable.length;
            details.keyboard = unreachable;
          }
          const axe = await new AxeBuilder({page}).setLegacyMode(true).options({resultTypes: ['violations']}).withTags(adapter.axeTags).analyze();
          const serious = axe.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
          metrics['axe.serious_critical'] = serious.reduce((n, v) => n + v.nodes.length, 0);
          details.axe = serious.map((v) => ({id: v.id, impact: v.impact, nodes: v.nodes.length, target: v.nodes.slice(0, 3).map((n) => n.target.join(' '))}));
        }
        let shot: string | undefined;
        const bad = metrics['layout.h_scroll'] + metrics['layout.overflow_elements'] + metrics['always_visible.misses'] + (metrics['axe.serious_critical'] ?? 0) + (metrics['keyboard.unreachable'] ?? 0);
        if (gating && bad > 0) {
          const first = failures.length === 0;
          failures.push(`${viewport.name} ${theme}: ${JSON.stringify(details)}`);
          const dir = join(outDir, 'shots');
          mkdirSync(dir, {recursive: true});
          if (first) shot = join(dir, `layout-${(path || 'index').replace(/\//g, '_')}-${viewport.name}-${theme}.png`);
          if (shot) await page.screenshot({path: shot});
        }
        rows.push({scenario: 'sitemap', page: path || 'index', viewport: viewport.name, theme, gating, metrics, details, shot});
      }
    }
    record(`sitemap-${path || 'index'}`, rows);
    expect(failures, 'gates 1, 2 and 5 at the gating viewports').toEqual([]);
  });
}
