# R-01 triage

Formula: priority = severity weight × frequency × confidence ÷ cost; S1 = 9, S2 = 3, S3 = 1;
frequency = pages (or tasks) affected ÷ pages (or tasks) in the round; confidence 2 when two
sources agree, else 1; cost S = 1, M = 2, L = 4.

## Caveats

- No people, recordings, questionnaire or paid levels in R-01 (L4 to L6, L8). Every finding comes
  from L2 or L3, so confidence is 1 for all of them.
- The heuristic and report reviewers (L7) did not run in R-01.
- First round: there is no earlier change set whose metrics could have moved.

## Did the last change sets move their metrics?

| round | change | target metric | moved |
|---|---|---|---|
| none | | | |

## Findings

| finding | severity | frequency | confidence | cost | priority | in the change set |
|---|---|---|---|---|---|---|
| UXF-003 links told apart by colour alone | S2 | 21/27 pages = 0.78 | 1 | S | 2.33 | yes, 1 |
| UXF-001 no search | S2 | 27/27 = 1.00 | 1 | M | 1.50 | yes, 2 |
| UXF-002 theme toggle hidden below 997 px | S3 | 27/27 = 1.00 | 1 | S | 1.00 | yes, 3 |
| UXF-004 sidebar entries below the fold | S3 | 3/5 tasks = 0.60 | 1 | M | 0.30 | no: waits for `decision.md` |

No S1 finding. The change set holds 3 changes.

## Change set

1. UXF-003: underline links inside paragraphs, list items, table cells and quotes
   (`website/src/css/custom.css`). Target: axe.serious_critical at 1440x900 and 390x844, 134 and
   130 now, 0 expected.
2. UXF-001: local search, `@easyops-cn/docusaurus-search-local` 0.55.3 in
   `website/docusaurus.config.ts`. Target: always_visible.misses at 1440x900 and 390x844 (the
   search share: 108 and 108).
3. UXF-002: the colour mode toggle stays in the navbar below 997 px (`website/src/css/custom.css`).
   Target: always_visible.misses at 390x844 (the toggle share: 108).

Each change is checked by the gate that found it, which fails on the R-01 measurement
(`report.md`) and passes after the change (`report-after.md`).
