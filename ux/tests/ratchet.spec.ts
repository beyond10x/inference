import {test, expect} from '@playwright/test';
import {compare, againstMain, update, type Baseline} from '../tools/ratchet.ts';

const tolerances = {counts: 0, steps: 0.05, layout_shift: 0.01};

function baseline(value: number, decision?: string): Baseline {
  return {about: 'test', tolerances, scenarios: {s: {'390x844/screen_only.actions': {value, tolerance: 0, ...(decision ? {decision} : {})}}}};
}

test('ratchet: equal values pass', () => {
  expect(compare(baseline(3), {s: {'390x844/screen_only.actions': 3}})).toEqual([]);
});

test('ratchet: a worse value fails', () => {
  expect(compare(baseline(3), {s: {'390x844/screen_only.actions': 4}})).toEqual([expect.stringMatching(/^worse:/)]);
});

test('ratchet: an improvement not taken fails', () => {
  expect(compare(baseline(3), {s: {'390x844/screen_only.actions': 2}})).toEqual([expect.stringMatching(/^improvement not taken:/)]);
});

test('ratchet: a missing value fails both ways', () => {
  expect(compare(baseline(3), {s: {}})).toEqual([expect.stringMatching(/^missing value:/)]);
  expect(compare(baseline(3), {s: {'390x844/screen_only.actions': 3, '390x844/screen_only.looks': 0}})).toEqual([expect.stringMatching(/^missing value:.*not in the baseline/)]);
});

test('ratchet: a baseline made worse than main needs a decision id', () => {
  expect(againstMain(baseline(4), baseline(3))).toEqual([expect.stringMatching(/^baseline made worse/)]);
  expect(againstMain(baseline(4, 'D-1'), baseline(3))).toEqual([]);
  expect(() => update(baseline(3), {s: {'390x844/screen_only.actions': 4}}, undefined)).toThrow(/without --decision/);
  expect(update(baseline(3), {s: {'390x844/screen_only.actions': 2}}, undefined).scenarios.s['390x844/screen_only.actions'].value).toBe(2);
});

test('ratchet: steps carry a 5 % tolerance', () => {
  const b: Baseline = {about: 'test', tolerances, scenarios: {s: {'k/screen_only.time_to_act_steps.max': {value: 100, tolerance: 5}}}};
  expect(compare(b, {s: {'k/screen_only.time_to_act_steps.max': 105}})).toEqual([]);
  expect(compare(b, {s: {'k/screen_only.time_to_act_steps.max': 106}})).toHaveLength(1);
});
