import type {Page} from '@playwright/test';
import {adapter, type AlwaysVisible, type Theme, type Viewport} from '../adapter.ts';

export type Overflow = {horizontalScroll: boolean; scrollWidth: number; offenders: string[]};

export async function setState(page: Page, viewport: Viewport, theme: Theme): Promise<void> {
  await page.setViewportSize({width: viewport.width, height: viewport.height});
  await page.evaluate((t) => {
    localStorage.setItem('theme', t);
    document.documentElement.setAttribute('data-theme', t);
    document.documentElement.setAttribute('data-theme-choice', t);
    window.scrollTo(0, 0);
  }, theme);
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
}

export async function overflow(page: Page): Promise<Overflow> {
  return page.evaluate(() => {
    const doc = document.scrollingElement ?? document.documentElement;
    const vw = document.documentElement.clientWidth;
    const blockish = new Set(['block', 'list-item', 'table-cell', 'flex', 'grid', 'flow-root']);
    const describe = (el: Element) => {
      const cls = typeof el.className === 'string' && el.className.trim() ? `.${el.className.trim().split(/\s+/).slice(0, 2).join('.')}` : '';
      const text = (el.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 48);
      return `${el.tagName.toLowerCase()}${cls} "${text}"`;
    };
    const all = Array.from(document.body.querySelectorAll('*'));
    const styles = new Map<Element, CSSStyleDeclaration>();
    const scrolls = new WeakMap<Element, boolean>();
    const under = new WeakMap<Element, boolean>();
    for (const el of all) {
      const s = getComputedStyle(el);
      styles.set(el, s);
      const parent = el.parentElement;
      const inScroll = parent ? scrolls.get(parent) ?? false : false;
      under.set(el, inScroll);
      scrolls.set(el, inScroll || s.overflowX === 'auto' || s.overflowX === 'scroll' || s.display === 'none');
    }
    const flagged: Element[] = [];
    for (const el of all) {
      const s = styles.get(el)!;
      if (under.get(el)) continue;
      if (s.display === 'none' || s.visibility === 'hidden' || s.position === 'fixed') continue;
      const r = el.getBoundingClientRect();
      if (r.width < 2 || r.height < 2) continue;
      const pastPage = r.right > vw + 1 || r.left < -1;
      let spill = false;
      if (blockish.has(s.display) && s.overflowX === 'visible' && el.clientWidth > 0 && el.scrollWidth > el.clientWidth + 1) {
        const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
        const range = document.createRange();
        for (let node = walker.nextNode(); node && !spill; node = walker.nextNode()) {
          if (!node.textContent?.trim() || (node.parentElement && scrolls.get(node.parentElement))) continue;
          range.selectNodeContents(node);
          spill = Array.from(range.getClientRects()).some((t) => t.right > r.right + 1 || t.left < r.left - 1);
        }
      }
      if (pastPage || spill) flagged.push(el);
    }
    const deepest = flagged.filter((el) => !flagged.some((other) => other !== el && el.contains(other)));
    return {
      horizontalScroll: doc.scrollWidth > vw + 1,
      scrollWidth: doc.scrollWidth,
      offenders: deepest.slice(0, 20).map(describe).concat(deepest.length > 20 ? [`… ${deepest.length - 20} more`] : []),
    };
  });
}

export async function alwaysVisibleMisses(page: Page, viewport: Viewport, path: string): Promise<string[]> {
  const items = adapter.alwaysVisible.filter(
    (item: AlwaysVisible) => (item.minWidth === undefined || viewport.width >= item.minWidth) && (!item.docsOnly || path.startsWith('docs')),
  );
  return page.evaluate((list) => {
    const vw = document.documentElement.clientWidth;
    const vh = window.innerHeight;
    const misses: string[] = [];
    for (const item of list) {
      const el = Array.from(document.querySelectorAll(item.selector)).find((e) => {
        const r = e.getBoundingClientRect();
        return r.width > 0 && r.height > 0 && getComputedStyle(e).visibility !== 'hidden';
      });
      if (!el) {
        misses.push(`${item.name}: not on the page`);
        continue;
      }
      const r = el.getBoundingClientRect();
      if (r.left < -0.5 || r.top < -0.5 || r.right > vw + 0.5 || r.bottom > vh + 0.5) {
        misses.push(`${item.name}: outside the viewport (${Math.round(r.left)},${Math.round(r.top)} to ${Math.round(r.right)},${Math.round(r.bottom)})`);
        continue;
      }
      const hit = document.elementFromPoint(r.left + r.width / 2, r.top + Math.min(r.height / 2, 20));
      if (!hit || !(el.contains(hit) || hit.contains(el))) misses.push(`${item.name}: covered`);
    }
    return misses;
  }, items);
}

export async function scrollToReading(page: Page): Promise<void> {
  await page.evaluate(() => {
    const doc = document.scrollingElement ?? document.documentElement;
    const content = document.querySelector("article") ?? document.querySelector("main");
    let y = doc.scrollHeight / 2;
    if (content) y = Math.min(y, content.getBoundingClientRect().bottom + window.scrollY - window.innerHeight);
    window.scrollTo(0, Math.max(0, Math.floor(y)));
  });
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
}

export async function keyboardUnreachable(page: Page): Promise<string[]> {
  const targets = await page.evaluate(() => {
    const visible = (e: Element) => {
      const r = e.getBoundingClientRect();
      return r.width > 0 && r.height > 0 && getComputedStyle(e).visibility !== 'hidden';
    };
    const nodes = Array.from(document.querySelectorAll('nav.navbar a[href], nav.navbar button, nav.navbar input, [role="search"] input, [role="search"] button'));
    return nodes.filter(visible).map((e, i) => {
      e.setAttribute('data-ux-kb', String(i));
      return {id: String(i), name: (e.getAttribute('aria-label') ?? e.textContent ?? e.getAttribute('title') ?? '').trim().slice(0, 40) || e.tagName.toLowerCase()};
    });
  });
  const reached = new Set<string>();
  await page.evaluate(() => {
    window.scrollTo(0, 0);
    document.body.setAttribute('tabindex', '-1');
    document.body.focus();
    document.body.removeAttribute('tabindex');
  });
  for (let i = 0; i < adapter.keyboardMaxTabs && reached.size < targets.length; i++) {
    await page.keyboard.press('Tab');
    const id = await page.evaluate(() => document.activeElement?.closest('[data-ux-kb]')?.getAttribute('data-ux-kb') ?? null);
    if (id !== null) reached.add(id);
  }
  return targets.filter((t) => !reached.has(t.id)).map((t) => t.name);
}
