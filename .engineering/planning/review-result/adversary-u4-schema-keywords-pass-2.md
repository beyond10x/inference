---
format: aep.planning-md/3
id: review-result:adversary-u4-schema-keywords-pass-2
kind: review-result
status: active
title: 'Adversary pass 2 on the tool authority schema keywords: a length limit past 1.8e19 is refused'
relations:
- reviews: story:tool-authority-schema-keywords
revision: 1
---
unit: story:tool-authority-schema-keywords, tree at 4c2fc211 (code under attack is 421de232)
verdict: CONFIRMED
cases: executed 44→50, red 1
origin: introduced 3 / pre-existing 0 / undecided 0
wrote-outside-worktree: 8 paths (part 6)
needs-coordinator: none

Pass 2 found one real failing case, and it is small. `count()` refuses a length limit of 1.8e19 or more at prepare, even though its own doc comment calls any number with no fractional part a valid length. Only a hand-written tool schema can get there, so it does not block the unit. Two more cases pass now but catch mutants that the existing suite let through.

**One deviation you should know about.** I tested the two mutants by editing `tool_authority.rs` in the worktree, as your message allowed. The adversary procedure says to never do this, even briefly. The script restored the file with `git checkout -- <file>` and also had an exit trap to do it. The file was changed for about 1 minute (01:51). `git status` was clean before the commit.

**1. Diff since 421de232** (test file only, committed as 4c2fc211 through the bot route)
```
 .../tests/adversary_u4_schema_keywords_pass2.rs    | 480 +++++++++++++++++++++
 1 file changed, 480 insertions(+)
```

**2. New cases** in `crates/inference/tests/adversary_u4_schema_keywords_pass2.rs`

| Case | What it asserts | Now |
|---|---|---|
| `a_length_beyond_u64_is_still_a_non_negative_integer` (:366) | a hand-declared schema with `maxLength: 1e20` and `maxItems: 18446744073709551616` prepares and admits | red |
| `a_zero_written_with_a_fraction_is_the_count_zero` (:390) | `maxItems: 0.0` and `minLength: 0.0` count as 0 | green; catches mutant `>= 0.0` → `> 0.0` |
| `an_empty_any_of_or_a_nested_empty_prefix_items_is_refused` (:412) | `anyOf: []`, `prefixItems: []` nested inside `oneOf`, and `anyOf: []` under `items` all fail prepare | green; catches mutant `keyword == "anyOf" \|\| !schemas.is_empty()` |
| `an_adjacently_tagged_enum_matches_exactly_one_variant` (:436) | macro tool with a `tag`/`content` enum | green |
| `an_untagged_enum_is_any_of_its_variants` (:454) | macro tool with an untagged enum | green |
| `an_option_of_a_tuple_with_a_char_is_the_tuple_or_null` (:469) | macro tool with `Option<(u8, char)>` | green |

Red output from the first run of this file alone (`cargo test -p inference --test adversary_u4_schema_keywords_pass2`, exit 101):
```
thread 'a_length_beyond_u64_is_still_a_non_negative_integer' panicked at crates/inference/tests/adversary_u4_schema_keywords_pass2.rs:342:5:
assertion `left == right` failed: {"properties":{"c":{"maxLength":1e+20,"type":"string"},"values":{"maxItems":1.8446744073709552e+19,"type":"array"}},"required":["c","values"],"type":"object"}
  left: Err("the schema of tool probe (...) uses \"maxLength\" in the form 1e+20, which the tool authority does not validate")
 right: Ok(())
test result: FAILED. 5 passed; 1 failed
```

Mutant run, both mutants applied together (`adv2/mutant.diff`):
- **Existing suite stayed green:** pipeline `nodes::tool_authority` 12 ok, `tool_macro_schema_keywords` 7, `adversary_u4_schema_keywords` 4, `adversary_w25_tool_macro` 6, `adversary_w25_tool_macro_pass2` 4. `tool_macro` (the compile-fail tests) was not run against the mutants.
- **Caught by the new cases:**
  - `a_zero_written…`: `Err("… uses \"minLength\" in the form 0.0 …")`
  - `an_empty_any_of…`: panicked at :423 (line before formatting)

**3. Suite run** (after the cases existed and the file was restored): `adv2/suite.sh`
- `cargo test -p inference-pipeline --lib nodes::tool_authority`: 12 passed, exit 0.
- `cargo test -p inference` on its six schema-keyword and tool-macro test targets, `--no-fail-fast`, exit 101:
  - `adversary_u4_schema_keywords` 4 ok
  - `adversary_u4_schema_keywords_pass2` 5 passed, 1 failed (`a_length_beyond_u64_is_still_a_non_negative_integer`)
  - `adversary_w25_tool_macro` 6 ok
  - `adversary_w25_tool_macro_pass2` 4 ok
  - `tool_macro` 11 ok
  - `tool_macro_schema_keywords` 7 ok
- The before count of 44 is this same run without the pass-2 file.

**4. Findings** (cover 4c2fc211)

| file:line | What | Verdict | Origin |
|---|---|---|---|
| `crates/inference-pipeline/src/nodes/tool_authority.rs:1002` | The `*n < 1.8e19` cutoff makes prepare refuse valid integer length limits at or above 1.8e19, which the doc comment at :996 says are accepted. Measured: red case at test :366, exit 101. What reaches it: only a hand-declared `Tool` schema, since schemars writes lengths as `usize` integers. Suggested fix: drop the cutoff, because `n as u64` saturates and a limit that large means the same thing. | INFEASIBLE | introduced |
| `tool_authority.rs:1002` | The `>= 0.0` check could become `> 0.0` with the suite still green. It is now covered by the test at :390. | CONFIRMED | introduced |
| `tool_authority.rs:971` | The non-empty check could exempt `anyOf` again (the same shape as the bug 421de232 fixed) with the suite still green. Nothing tested `anyOf: []` before. It is now covered by the test at :412. | CONFIRMED | introduced |

**5. Attacked and could not break**
- schemars 1.2.3 never writes `prefixItems: []`; a tuple struct with no fields gets only `maxItems` (`schema_exprs.rs:668`). So the non-empty rule refuses nothing the macro generates.
- `count` handles `-0.0`, negative integers, and integers that serde_json stores as floats correctly.
- `supported` and `admits` both read limits through `count`, so they cannot disagree.
- Adjacently tagged enums, untagged enums, and `Option` of a tuple holding a `char` all prepare and decide calls correctly.

**6. Paths written outside the worktree** (all in the unit's scratch directory `adv2/`): `cases.log`, `commit-msg.txt`, `mutant.sh`, `mutant.diff`, `mutant-pipeline.log`, `mutant-inference.log`, `mutant-status.txt`, `suite.sh`; the suite logs `suite-pipeline.log` and `suite-inference.log` are in the same directory. The lease `inference-u4-adv2` is released.

**7.**
```findings
[
  {"file": "crates/inference-pipeline/src/nodes/tool_authority.rs", "line": 1002, "category": "boundary", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "the 1.8e19 cutoff in count refuses length limits such as 1e20 that its own doc comment and JSON Schema 2020-12 call integers; only a hand-declared schema reaches it, and saturating n as u64 without the cutoff gives the same meaning"},
  {"file": "crates/inference-pipeline/src/nodes/tool_authority.rs", "line": 1002, "category": "mutant", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "the mutant >= 0.0 to > 0.0 in count, which refuses a limit written 0.0, survived the unit's suite and is now caught by a_zero_written_with_a_fraction_is_the_count_zero"},
  {"file": "crates/inference-pipeline/src/nodes/tool_authority.rs", "line": 971, "category": "mutant", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "exempting anyOf from the non-empty rule, the shape of the bug 421de232 fixed for prefixItems, survived the unit's suite and is now caught by an_empty_any_of_or_a_nested_empty_prefix_items_is_refused"}
]
```
