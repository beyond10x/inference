import {mkdirSync, readdirSync, readFileSync, writeFileSync, existsSync} from 'node:fs';
import {join} from 'node:path';
import {outDir} from './scenarios.ts';

export type Row = {
  scenario: string;
  page: string;
  viewport: string;
  theme: string;
  gating: boolean;
  metrics: Record<string, number>;
  details?: Record<string, unknown>;
  shot?: string;
};

export function record(id: string, rows: Row[]): void {
  const dir = join(outDir, 'rows');
  mkdirSync(dir, {recursive: true});
  writeFileSync(join(dir, `${id.replace(/[^a-z0-9._-]+/gi, '_')}.json`), JSON.stringify(rows, null, 1));
}

export function readRows(): Row[] {
  const dir = join(outDir, 'rows');
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((f) => f.endsWith('.json'))
    .sort()
    .flatMap((f) => JSON.parse(readFileSync(join(dir, f), 'utf8')) as Row[]);
}

function quantile(values: number[], q: number): number {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.ceil(q * sorted.length) - 1)];
}

export const median = (values: number[]) => quantile(values, 0.5);
export const p95 = (values: number[]) => quantile(values, 0.95);

export type Measured = Record<string, Record<string, number>>;

export function aggregate(rows: Row[]): Measured {
  const out: Measured = {};
  for (const row of rows.filter((r) => r.gating)) {
    const scenario = (out[row.scenario] ??= {});
    for (const [key, value] of Object.entries(row.metrics)) {
      const name = `${row.viewport}/${key}`;
      if (row.scenario === 'sitemap') {
        scenario[name] = (scenario[name] ?? 0) + value;
      } else {
        scenario[name] = Math.max(scenario[name] ?? 0, value);
      }
    }
  }
  return out;
}
