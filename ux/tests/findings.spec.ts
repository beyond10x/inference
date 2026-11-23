import {test, expect} from '@playwright/test';
import {existsSync, readdirSync, readFileSync} from 'node:fs';
import {join} from 'node:path';
import {uxDir, repoDir} from '../lib/scenarios.ts';

const fields = ['id', 'title', 'class', 'severity', 'metric', 'status', 'found_by', 'evidence_recording', 'evidence_screenshot', 'evidence_lines', 'regression_asset'];

function header(text: string): Record<string, string> {
  const match = text.match(/^---\n([\s\S]*?)\n---/);
  if (!match) return {};
  return Object.fromEntries(
    match[1].split('\n').map((line) => {
      const at = line.indexOf(':');
      return [line.slice(0, at).trim(), line.slice(at + 1).trim()];
    }),
  );
}

test('findings: every record has its header and no closed S1 or S2 lacks a regression asset', () => {
  const dir = join(uxDir, 'findings');
  const problems: string[] = [];
  if (!existsSync(dir)) return;
  for (const file of readdirSync(dir).filter((f) => /^UXF-\d{3}\.md$/.test(f))) {
    const h = header(readFileSync(join(dir, file), 'utf8'));
    for (const field of fields) if (!(field in h)) problems.push(`${file}: no ${field}`);
    if (`${h.id}.md` !== file) problems.push(`${file}: id ${h.id} does not match the file name`);
    if (!/^S[123]$/.test(h.severity ?? '')) problems.push(`${file}: severity ${h.severity}`);
    const closed = h.status === 'closed';
    if (closed && (h.severity === 'S1' || h.severity === 'S2')) {
      const asset = (h.regression_asset ?? '').split(' ')[0];
      if (!asset || asset === 'none') problems.push(`${file}: closed ${h.severity} without a regression asset`);
      else if (!existsSync(join(repoDir, asset))) problems.push(`${file}: regression asset ${asset} does not exist`);
    }
  }
  expect(problems).toEqual([]);
});
