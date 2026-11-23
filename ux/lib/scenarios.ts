import {readdirSync, readFileSync, existsSync} from 'node:fs';
import {join, resolve} from 'node:path';
import {adapter} from '../adapter.ts';

export const uxDir = resolve(import.meta.dirname, '..');
export const repoDir = resolve(uxDir, '..');
export const outDir = join(uxDir, '.out');

export type TaskScenario = {
  name: string;
  kind: 'task';
  card: 'anchor' | 'rotating';
  task: string;
  start: string;
  words: string[];
  truth: {path: string; text: string};
};

export type SitemapScenario = {
  name: string;
  kind: 'sitemap';
  card: 'anchor' | 'rotating';
  about: string;
  source: string;
};

export type Scenario = TaskScenario | SitemapScenario;

export function scenarios(): Scenario[] {
  const dir = join(uxDir, 'scenarios');
  return readdirSync(dir, {withFileTypes: true})
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .sort()
    .map((name) => ({name, ...JSON.parse(readFileSync(join(dir, name, 'scenario.json'), 'utf8'))}));
}

export function tasks(): TaskScenario[] {
  return scenarios().filter((s): s is TaskScenario => s.kind === 'task');
}

export function sitemapPages(): string[] {
  const sitemap = scenarios().find((s): s is SitemapScenario => s.kind === 'sitemap');
  if (!sitemap) return [];
  const file = join(repoDir, sitemap.source);
  if (!existsSync(file)) throw new Error(`${sitemap.source} is missing: build the site first`);
  const xml = readFileSync(file, 'utf8');
  const pages = [...xml.matchAll(/<loc>([^<]+)<\/loc>/g)].map((m) => {
    const path = new URL(m[1]).pathname;
    return path.startsWith(adapter.basePath) ? path.slice(adapter.basePath.length) : path;
  });
  return [...new Set(pages)].sort();
}
