import type {Reporter, TestCase, TestResult, FullResult} from '@playwright/test/reporter';

export default class SlowestReporter implements Reporter {
  private durations: {title: string; ms: number}[] = [];
  private started = Date.now();

  onTestEnd(test: TestCase, result: TestResult): void {
    this.durations.push({title: test.title, ms: result.duration});
  }

  onEnd(result: FullResult): void {
    const wall = Date.now() - this.started;
    const slowest = [...this.durations].sort((a, b) => b.ms - a.ms).slice(0, 10);
    const lines = ['', `ux: ${this.durations.length} tests, wall ${(wall / 1000).toFixed(1)} s, status ${result.status}`, 'ux: 10 slowest tests'];
    for (const t of slowest) lines.push(`  ${(t.ms / 1000).toFixed(1).padStart(6)} s  ${t.title}`);
    process.stdout.write(`${lines.join('\n')}\n`);
  }
}
