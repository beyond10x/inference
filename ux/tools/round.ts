import {existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync} from 'node:fs';
import {execFileSync} from 'node:child_process';
import {basename, join} from 'node:path';
import {chromium} from '@playwright/test';
import {adapter} from '../adapter.ts';
import {uxDir, repoDir, scenarios} from '../lib/scenarios.ts';
import {aggregate, readRows, type Measured, type Row} from '../lib/record.ts';
import type {Baseline} from './ratchet.ts';

type Gate = {id: number; name: string; value: number; pass: boolean; note: string};

function sum(rows: Row[], key: string): number {
  return rows.reduce((n, r) => n + (r.metrics[key] ?? 0), 0);
}

function gates(rows: Row[]): Gate[] {
  const g = rows.filter((r) => r.gating);
  const site = g.filter((r) => r.scenario === 'sitemap');
  const task = g.filter((r) => r.scenario !== 'sitemap');
  const layout = sum(site, 'layout.h_scroll') + sum(site, 'layout.overflow_elements');
  const visible = sum(site, 'always_visible.misses');
  const misses = sum(task, 'completion_miss');
  const a11y = sum(site, 'axe.serious_critical') + sum(site, 'keyboard.unreachable');
  return [
    {id: 1, name: 'no horizontal scroll, no element past its box', value: layout, pass: layout === 0, note: 'page states with horizontal scroll plus overflowing elements'},
    {id: 2, name: 'always visible: navbar, search, theme toggle, sidebar at desktop', value: visible, pass: visible === 0, note: 'misses at the top and at a reading position: half the page, at most to where the article ends'},
    {id: 3, name: 'unmissable completion', value: misses, pass: misses === 0, note: 'completion_miss over every task run; false_complete cannot occur, completion is the truth page'},
    {id: 4, name: 'screen-only user completes every task', value: misses, pass: misses === 0, note: 'runs that did not reach the truth page'},
    {id: 5, name: 'axe 0 serious or critical, keyboard reaches nav and search', value: a11y, pass: a11y === 0, note: 'serious or critical nodes plus navbar controls Tab does not reach'},
  ];
}

function previousRound(round: string): {name: string; measured: Measured} | undefined {
  const dir = join(uxDir, 'rounds');
  if (!existsSync(dir)) return undefined;
  const earlier = readdirSync(dir).filter((d) => /^R-\d\d$/.test(d) && d < round).sort().pop();
  if (!earlier) return undefined;
  const file = join(dir, earlier, 'round.json');
  if (!existsSync(file)) return undefined;
  return {name: earlier, measured: JSON.parse(readFileSync(file, 'utf8')).measured};
}

async function images(rows: Row[], shotsDir: string, suffix: string): Promise<{sheet?: string; worst: {file: string; caption: string}[]}> {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  const toDataUrl = (file: string) => `data:image/png;base64,${readFileSync(file).toString('base64')}`;
  const worstRows = rows
    .filter((r) => r.gating && r.shot && r.scenario === 'sitemap' && existsSync(r.shot))
    .map((r) => ({r, bad: Object.values(r.metrics).reduce((n, v) => n + v, 0)}))
    .sort((a, b) => b.bad - a.bad || a.r.page.localeCompare(b.r.page))
    .slice(0, 6);
  const worst: {file: string; caption: string}[] = [];
  for (const [i, {r}] of worstRows.entries()) {
    const vp = [...adapter.gatingViewports].find((v) => v.name === r.viewport)!;
    await page.setViewportSize({width: vp.width, height: vp.height});
    await page.setContent(`<body style="margin:0"><img src="${toDataUrl(r.shot!)}" style="display:block"></body>`);
    const file = join(shotsDir, `worst${suffix}-${String(i + 1).padStart(2, "0")}.jpg`);
    await page.screenshot({path: file, type: 'jpeg', quality: 60});
    const d = r.details ?? {};
    const what = [...((d.misses as string[]) ?? []), ...((d.axe as {id: string}[]) ?? []).map((a) => `axe ${a.id}`), ...((d.offenders as string[]) ?? []).slice(0, 2)];
    worst.push({file: basename(file), caption: `/${r.page === 'index' ? '' : r.page} at ${r.viewport}, ${r.theme}: ${what.join('; ')}`});
  }
  const moments = rows.filter((r) => r.scenario !== 'sitemap' && r.shot && existsSync(r.shot));
  let sheet: string | undefined;
  if (moments.length > 0) {
    const names = [...new Set(moments.map((r) => r.scenario))];
    const columns = adapter.gatingViewports.flatMap((v) => adapter.themes.map((t) => ({v: v.name, t})));
    const cells = names
      .map((n) => `<tr><th>${n}</th>${columns.map((c) => {
        const r = moments.find((m) => m.scenario === n && m.viewport === c.v && m.theme === c.t);
        return `<td>${r ? `<img src="${toDataUrl(r.shot!)}" style="height:220px">` : 'not reached'}</td>`;
      }).join('')}</tr>`)
      .join('');
    const head = `<tr><th></th>${columns.map((c) => `<th>${c.v} ${c.t}</th>`).join('')}</tr>`;
    await page.setViewportSize({width: 1600, height: 400});
    await page.setContent(`<body style="margin:8px;font:12px sans-serif"><table>${head}${cells}</table></body>`);
    await page.waitForLoadState('load');
    sheet = `contact-sheet${suffix}.jpg`;
    await page.screenshot({path: join(shotsDir, sheet), type: 'jpeg', quality: 60, fullPage: true});
  }
  await browser.close();
  return {sheet, worst};
}

async function main(): Promise<number> {
  const round = process.argv[2];
  if (!round || !/^R-\d\d$/.test(round)) {
    process.stderr.write('usage: node tools/round.ts R-nn [label]\n');
    return 2;
  }
  const label = process.argv[3] ?? '';
  const suffix = label && label !== 'measure' ? `-${label}` : '';
  const rows = readRows();
  if (rows.length === 0) {
    process.stderr.write('round: no measurement in ux/.out/rows: run the tests first\n');
    return 1;
  }
  const dir = join(uxDir, 'rounds', round);
  const shotsDir = join(dir, 'shots');
  mkdirSync(shotsDir, {recursive: true});
  const measured = aggregate(rows);
  const baseline: Baseline | undefined = existsSync(join(uxDir, 'baseline.json')) ? JSON.parse(readFileSync(join(uxDir, 'baseline.json'), 'utf8')) : undefined;
  const last = previousRound(round);
  const results = gates(rows);
  const dirty = execFileSync('git', ['status', '--porcelain', '--', 'website'], {cwd: repoDir}).toString().trim() !== '';
  const commit = execFileSync('git', ['rev-parse', '--short', 'HEAD'], {cwd: repoDir}).toString().trim() + (dirty ? ' with uncommitted website changes' : '');
  const date = new Date().toISOString().slice(0, 10);
  const {sheet, worst} = await images(rows, shotsDir, suffix);

  const measuredOnly = rows.filter((r) => !r.gating && r.scenario === 'sitemap');
  const byViewport = adapter.measuredViewports.map((v) => {
    const vr = measuredOnly.filter((r) => r.viewport === v.name);
    return `| ${v.name} | ${sum(vr, 'layout.h_scroll')} | ${sum(vr, 'layout.overflow_elements')} | ${sum(vr, 'always_visible.misses')} |`;
  });
  const cards = scenarios().map((s) => `| ${s.name} | ${s.card} | ${s.kind === 'task' ? `${s.task} Start /${s.start}, truth /${s.truth.path}` : s.about} |`);
  const metricLines: string[] = [];
  for (const [scenario, keys] of Object.entries(measured)) {
    for (const [key, value] of Object.entries(keys).sort()) {
      const b = baseline?.scenarios[scenario]?.[key]?.value;
      const l = last?.measured[scenario]?.[key];
      metricLines.push(`| ${scenario} | ${key} | ${value} | ${b ?? 'none'} | ${l ?? 'none'} |`);
    }
  }
  const report = [
    `# ${round}${label ? `: ${label}` : ''}`,
    '',
    `Site: ${adapter.site}, built from commit ${commit}, measured ${date}. Gating viewports ${adapter.gatingViewports.map((v) => v.name).join(', ')}; measured only ${adapter.measuredViewports.map((v) => v.name).join(', ')}; themes ${adapter.themes.join(', ')}.`,
    '',
    '## Gates',
    '',
    '| gate | check | value | result | counts |',
    '|---|---|---|---|---|',
    ...results.map((g) => `| ${g.id} | ${g.name} | ${g.value} | ${g.pass ? 'pass' : 'FAIL'} | ${g.note} |`),
    '| 6 | cue truth | n/a | dropped | the site has no guidance cue |',
    '',
    '## Scenario cards',
    '',
    '| scenario | card | task |',
    '|---|---|---|',
    ...cards,
    '',
    '## Metrics against the baseline and the last round',
    '',
    `Last round: ${last ? last.name : 'none'}. Lower is better.`,
    '',
    '| scenario | key | value | baseline | last round |',
    '|---|---|---|---|---|',
    ...metricLines,
    '',
    '## Measured-only viewports',
    '',
    '| viewport | horizontal scroll states | overflowing elements | always-visible misses |',
    '|---|---|---|---|',
    ...byViewport,
    '',
    '## Worst moments',
    '',
    ...(worst.length === 0 ? ['None: no gating state failed.'] : worst.flatMap((w) => [`- ${w.caption}`, '', `  ![${w.file}](shots/${w.file})`, ''])),
    '',
    '## Key moments',
    '',
    sheet ? `Each task's truth page at every gating viewport and theme, 0.75 s after arrival: ![contact sheet](shots/${sheet})` : 'No task reached its truth page.',
    '',
    '## Inputs not in this round',
    '',
    '- Recorded sessions of people (L6), the questionnaire and real-user sessions (L8): none.',
    '- Keyed replay (L4) and the discoverability probe (L5): not run, no paid levels in this round.',
    '- Guidance cue (L0, gate 6): the site has none.',
    '',
  ].join('\n');
  writeFileSync(join(dir, `report${suffix}.md`), report);
  writeFileSync(join(dir, `round${suffix}.json`), `${JSON.stringify({round, label, commit, date, gates: results, measured}, null, 2)}\n`);

  const trend = join(uxDir, 'rounds', 'trend.md');
  const headerLines = ['# Trend', '', 'One row per round. Gate values: lower is better, 0 passes.', '', '| round | date | commit | gate 1 | gate 2 | gate 3 | gate 4 | gate 5 | label |', '|---|---|---|---|---|---|---|---|---|'];
  const existing = existsSync(trend) ? readFileSync(trend, 'utf8').split('\n').filter((l) => l.startsWith('| R-')) : [];
  const row = `| ${round} | ${date} | ${commit} | ${results.map((g) => g.value).join(' | ')} | ${label} |`;
  const kept = existing.filter((l) => !l.startsWith(`| ${round} |`) || !l.endsWith(`| ${label} |`));
  writeFileSync(trend, `${[...headerLines, ...kept, row].join('\n')}\n`);
  process.stdout.write(`round: wrote ${join('ux', 'rounds', round)}/report${suffix}.md, round${suffix}.json, ${worst.length} worst moments, ${sheet ? 'a contact sheet' : 'no contact sheet'}\n`);
  return 0;
}

main().then((code) => process.exit(code));
