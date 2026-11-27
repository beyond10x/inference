---
format: aep.planning-md/3
id: review-result:local-tts-adapter-adversary-3
kind: review-result
status: active
title: Adversary pass on the admitted licences and the quality rule for a new adapter crate (2026-11-27)
relations:
- reviews: story:local-tts-adapter-under-licence-policy
revision: 1
---
## Adversary pass on the admitted licences and the quality rule for a new adapter crate (2026-11-27)

Target: branch `impl/local-tts-adapter` at 7892589cd (the licence bullet of `AGENTS.md`, the
licence tests of both sherpa-onnx crates, and the quality rule for an adapter crate no adopted
record holds yet). Red tests: commit 108cabb52 (taken into the branch as e37943056);
`inference-docs --test workflows`: 177 run, 175 passed, 2 failed.

Findings, all introduced:

- F1, warning (`crates/inference-docs/tests/quality/adapters.rs`, `awaiting_a_record`): a crate
  counted as awaiting its record when the record held no results for it, without reading the
  record's own `inputs.packages`; a hand-edited record whose run planned the crate passed. Test
  `a_record_whose_own_run_planned_the_crate_and_holds_no_result_of_it_is_refused_with_the_sentence`.
  Fixed in 7c0625a0e: awaiting only when the record's inputs do not name the crate.
- F2, note (`quality/README.md` against `check_crate_rows`): a section with the sentence passed with
  a table whose first cells were not backticked measures. Test
  `the_sentence_with_a_table_whose_rows_name_no_measure_in_a_code_span_is_refused`. Fixed in
  7c0625a0e: any table row in a section with the sentence is refused.
- J1, note: the admitted licence lists are copies of the rule in `AGENTS.md`. Fixed in 7c0625a0e:
  both licence tests read the bullet and check it names every admitted entry; the recogniser's
  stricter list stays, with its reason in a doc comment.
- J2, note (`src/assess/adopt.rs`, `record_rules`): its doc claimed the rules of
  `record_problems`, which now exempts awaiting crates. Fixed in 7c0625a0e: the doc says why
  `adopt-records` still refuses a run lacking a planned package; a test pins the difference, and
  the parity test gains a plant that removes a whole package.

Attacked without a finding: licence expressions (`WITH`, parentheses, case, spaces, a typo); ucd-tools
not admitted in the recogniser's crate; the sentence on a crate the record holds or in another
crate's section; a near-miss spelling; a crate dropped from the plan but kept on the page; a record
package the plan drops; `adopt-records` of a run lacking a planned package; the twelve-crate count;
the fixture changes against 24531c00e.

After the fixes: the local gate at 7c0625a0e, check-source, test-build, test-run (3503 passed, 101
skipped) and quality-records, all exit 0.
