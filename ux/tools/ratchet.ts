import {readFileSync, writeFileSync, existsSync} from 'node:fs';
import {execFileSync} from 'node:child_process';
import {join} from 'node:path';
import {uxDir, repoDir} from '../lib/scenarios.ts';
import {aggregate, readRows, type Measured} from '../lib/record.ts';

export type Entry = {value: number; tolerance: number; decision?: string};
export type Baseline = {
  about: string;
  scenarios: Record<string, Record<string, Entry>>;
  tolerances: {counts: number; steps: number; layout_shift: number};
};

export const baselineFile = join(uxDir, 'baseline.json');

export function tolerance(key: string, value: number, t: Baseline['tolerances']): number {
  if (key.includes('steps')) return Math.round(value * t.steps * 1000) / 1000;
  if (key.includes('layout_shift')) return t.layout_shift;
  return t.counts;
}

function mainBaseline(): Baseline | undefined {
  for (const ref of ['origin/main', 'main']) {
    try {
      return JSON.parse(execFileSync('git', ['show', `${ref}:ux/baseline.json`], {cwd: repoDir, stdio: ['ignore', 'pipe', 'ignore']}).toString());
    } catch {
      continue;
    }
  }
  return undefined;
}

export function compare(baseline: Baseline, measured: Measured): string[] {
  const problems: string[] = [];
  for (const [scenario, keys] of Object.entries(baseline.scenarios)) {
    for (const [key, entry] of Object.entries(keys)) {
      const value = measured[scenario]?.[key];
      if (value === undefined) problems.push(`missing value: ${scenario} ${key}`);
      else if (value > entry.value + entry.tolerance) problems.push(`worse: ${scenario} ${key} ${value} > baseline ${entry.value} (tolerance ${entry.tolerance})`);
      else if (value < entry.value - entry.tolerance) problems.push(`improvement not taken: ${scenario} ${key} ${value} < baseline ${entry.value}: run the ratchet with --update in this change`);
    }
  }
  for (const [scenario, keys] of Object.entries(measured)) {
    for (const key of Object.keys(keys)) {
      if (baseline.scenarios[scenario]?.[key] === undefined) problems.push(`missing value: ${scenario} ${key} is measured and not in the baseline`);
    }
  }
  return problems;
}

export function againstMain(baseline: Baseline, main: Baseline | undefined): string[] {
  if (!main) return [];
  const problems: string[] = [];
  for (const [scenario, keys] of Object.entries(baseline.scenarios)) {
    for (const [key, entry] of Object.entries(keys)) {
      const before = main.scenarios[scenario]?.[key];
      if (before && entry.value > before.value + before.tolerance && !entry.decision) {
        problems.push(`baseline made worse without a decision id: ${scenario} ${key} ${before.value} -> ${entry.value}`);
      }
    }
  }
  return problems;
}

export function update(baseline: Baseline, measured: Measured, decision: string | undefined): Baseline {
  const next: Baseline = {...baseline, scenarios: {}};
  for (const [scenario, keys] of Object.entries(measured)) {
    for (const [key, value] of Object.entries(keys)) {
      const old = baseline.scenarios[scenario]?.[key];
      const entry: Entry = {value, tolerance: tolerance(key, value, baseline.tolerances)};
      if (old && value <= old.value + old.tolerance && value >= old.value - old.tolerance) {
        (next.scenarios[scenario] ??= {})[key] = old;
        continue;
      }
      if (old && value > old.value + old.tolerance) {
        if (!decision) throw new Error(`refusing to make ${scenario} ${key} worse (${old.value} -> ${value}) without --decision <id>`);
        entry.decision = decision;
      }
      (next.scenarios[scenario] ??= {})[key] = entry;
    }
  }
  return next;
}

function main(): number {
  const args = process.argv.slice(2);
  const doUpdate = args.includes('--update');
  const decisionAt = args.indexOf('--decision');
  const decision = decisionAt >= 0 ? args[decisionAt + 1] : undefined;
  const rows = readRows();
  if (rows.length === 0) {
    process.stderr.write('ratchet: no measurement in ux/.out/rows: run the tests first\n');
    return 1;
  }
  const measured = aggregate(rows);
  if (!existsSync(baselineFile)) {
    process.stderr.write('ratchet: ux/baseline.json is missing\n');
    return 1;
  }
  let baseline: Baseline = JSON.parse(readFileSync(baselineFile, 'utf8'));
  if (doUpdate) {
    baseline = update(baseline, measured, decision);
    writeFileSync(baselineFile, `${JSON.stringify(baseline, null, 2)}\n`);
    process.stdout.write('ratchet: baseline updated\n');
  }
  const problems = [...compare(baseline, measured), ...againstMain(baseline, mainBaseline())];
  for (const p of problems) process.stdout.write(`ratchet: ${p}\n`);
  const count = Object.values(baseline.scenarios).reduce((n, keys) => n + Object.keys(keys).length, 0);
  process.stdout.write(`ratchet: ${count} values, ${problems.length} problems\n`);
  return problems.length === 0 ? 0 : 1;
}

if (import.meta.filename === process.argv[1]) process.exit(main());
