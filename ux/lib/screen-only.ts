import type {Page} from '@playwright/test';
import {adapter} from '../adapter.ts';
import type {TaskScenario} from './scenarios.ts';

export type Glance = 1 | 2 | 3;

export type Run = {
  reached: boolean;
  actions: number;
  looks: number;
  steers: number;
  steps: number;
  glances: Glance[];
  trail: string[];
};

type Candidate = {id: string; name: string; path: string; score: number; glance: Glance};

const entryWords = ['documentation', 'docs'];
const maxSteps = 10;

function pathOf(page: Page): string {
  const p = new URL(page.url()).pathname;
  return p.startsWith(adapter.basePath) ? p.slice(adapter.basePath.length).replace(/\/$/, '') : p;
}

async function candidates(page: Page, words: string[], visited: string[]): Promise<Candidate[]> {
  return page.evaluate(
    ({words, visited, base, primary}) => {
      const vw = document.documentElement.clientWidth;
      const vh = window.innerHeight;
      const out: Candidate[] = [];
      const links = Array.from(document.querySelectorAll('a[href]')) as HTMLAnchorElement[];
      links.forEach((a, i) => {
        const href = new URL(a.href, location.href);
        if (href.origin !== location.origin || !href.pathname.startsWith(base)) return;
        const path = href.pathname.slice(base.length).replace(/\/$/, '');
        if (visited.includes(path)) return;
        const s = getComputedStyle(a);
        if (s.visibility === 'hidden' || a.closest('[aria-hidden="true"]')) return;
        const r = a.getBoundingClientRect();
        if (r.width < 2 || r.height < 2) return;
        if (r.left < -0.5 || r.right > vw + 0.5) return;
        const name = (a.getAttribute('aria-label') ?? a.innerText ?? '').trim().replace(/\s+/g, ' ');
        const lower = name.toLowerCase();
        const score = words.findIndex((w) => lower.includes(w));
        if (score < 0) return;
        let glance: Glance = 3;
        if (r.top >= 0 && r.bottom <= vh) {
          const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
          if (!hit || !(a.contains(hit) || hit.contains(a))) return;
          glance = a.closest(primary) ? 1 : 2;
        }
        a.setAttribute('data-ux-pick', String(i));
        out.push({id: String(i), name, path, score, glance});
      });
      return out.sort((x, y) => x.score - y.score || x.glance - y.glance);
    },
    {words, visited, base: adapter.basePath, primary: adapter.primaryNavigation},
  );
}

async function settle(page: Page): Promise<void> {
  await page.waitForLoadState('load');
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
}

export async function atTruth(page: Page, scenario: TaskScenario): Promise<boolean> {
  if (pathOf(page) !== scenario.truth.path) return false;
  return page.evaluate((text) => (document.querySelector('main, article')?.textContent ?? '').includes(text), scenario.truth.text);
}

export async function play(page: Page, scenario: TaskScenario): Promise<Run> {
  const run: Run = {reached: false, actions: 0, looks: 0, steers: 0, steps: 0, glances: [], trail: [`/${pathOf(page)}`]};
  const visited = [pathOf(page)];
  let menuTried = false;
  for (let step = 0; step < maxSteps; step++) {
    if (await atTruth(page, scenario)) {
      run.reached = true;
      return run;
    }
    let pick = (await candidates(page, scenario.words, visited))[0];
    if (!pick || pick.glance === 3) {
      const menu = page.getByRole(adapter.menuButton.role, {name: adapter.menuButton.name});
      if (!menuTried && (await menu.isVisible())) {
        menuTried = true;
        await menu.click();
        run.actions++;
        run.glances.push(1);
        run.trail.push('open menu');
        await page.waitForTimeout(400);
        const inMenu = (await candidates(page, scenario.words, visited))[0];
        if (inMenu) pick = inMenu;
      }
    }
    if (!pick) pick = (await candidates(page, entryWords, visited))[0];
    if (!pick) {
      if (visited.length > 1 && run.steers < 2) {
        await page.goBack();
        await settle(page);
        run.steers++;
        run.actions++;
        run.trail.push('back');
        continue;
      }
      break;
    }
    const link = page.locator(`[data-ux-pick="${pick.id}"]`).first();
    if (pick.glance === 3) {
      await link.scrollIntoViewIfNeeded();
      run.looks++;
    }
    run.glances.push(pick.glance);
    const title = await page.title();
    await link.click();
    run.actions++;
    run.steps++;
    menuTried = false;
    await page.waitForURL((u) => u.pathname.replace(/\/$/, '').endsWith(pick.path) || pick.path === '', {timeout: 5000}).catch(() => undefined);
    await page.waitForFunction((before) => document.title !== before, title, {timeout: 5000}).catch(() => undefined);
    await settle(page);
    visited.push(pathOf(page));
    run.trail.push(`${pick.name} (glance ${pick.glance}) -> /${pathOf(page)}`);
  }
  run.reached = await atTruth(page, scenario);
  return run;
}
