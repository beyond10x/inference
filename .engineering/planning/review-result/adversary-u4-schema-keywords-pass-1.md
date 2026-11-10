---
format: aep.planning-md/3
id: review-result:adversary-u4-schema-keywords-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on the tool authority schema keywords: two malformed-form cases, one mutant gap closed'
relations:
- reviews: story:tool-authority-schema-keywords
revision: 1
---
unit: story:tool-authority-schema-keywords, branch impl/tool-authority-schema-keywords; findings cover 07cd8dd0 (my test commit on top is b83f2957)
verdict: NEEDS-CHANGE (2 red cases)
cases: executed 40→44, red 2
origin: introduced 3 / pre-existing 1 / undecided 0
wrote-outside-worktree: 1 path (scratch dir adv1, listed in part 6)
needs-coordinator: whether the two red cases (only hand-declared schemas reach them) hold the unit or go to the backlog

**1. Diff stat** (`git --no-pager diff --stat 07cd8dd0..HEAD`): only a test file changed. The working tree is clean.
```
 .../tests/adversary_u4_schema_keywords.rs          | 394 +++++++++++++++++++++
 1 file changed, 394 insertions(+)
```

**2. Cases added** in `crates/inference/tests/adversary_u4_schema_keywords.rs`. They copy the harness from `tool_macro_schema_keywords.rs`. I first ran them alone with `cargo test -p inference --test adversary_u4_schema_keywords`, and that exited 101.
- `an_empty_prefix_items_is_not_in_json_schemas_form`: is **RED now**. A hand-declared schema with `"prefixItems": []` should be refused at prepare.
  ```
  panicked at crates/inference/tests/adversary_u4_schema_keywords.rs:343:5:
  the authority prepares with the malformed schema {"properties":{"pair":{"prefixItems":[],"type":"array"}},"required":["pair"],"type":"object"}
  ```
- `a_length_written_with_a_zero_fraction_is_an_integer`: is **RED now**. A schema with `minLength`/`maxLength` 1.0 and `minItems`/`maxItems` 2.0 should prepare, admit `"a"`/`[1,2]` and refuse `"ab"` and `[1]`.
  ```
  panicked at crates/inference/tests/adversary_u4_schema_keywords.rs:314:5:
  assertion `left == right` failed: {"properties":{"c":{"maxLength":1.0,"minLength":1.0,"type":"string"},"values":{"maxItems":2.0,"minItems":2.0,"type":"array"}},"required":["c","values"],"type":"object"}
    left: Err("the schema of tool probe (10e93941-d764-49db-81ef-9c8ad788043b) uses \"maxLength\" in the form 1.0, which the tool authority does not validate")
   right: Ok(())
  ```
- `an_option_of_a_char_is_one_code_point_or_null`: is **green now**. It catches the mutant at :1083.
- `an_option_of_a_fixed_array_is_its_length_or_null`: is **green now**. It catches the mutant at :1090.
- I tested the mutant on a copy of the tree in scratch, never on the worktree. The change is `_ => true` → `_ => false` at `tool_authority.rs:1083` and `:1090`.
  - The unit's suite stays green under it: tool_macro_schema_keywords 7, adversary_w25_tool_macro 6, adversary_w25_tool_macro_pass2 4, tool_macro 11, and the in-module `nodes::tool_authority` tests 12.
  - My two tests go red under it:
  ```
  {"c":null} is admitted by {"properties":{"c":{"maxLength":1,"minLength":1,"type":["string","null"]}},"type":"object"}, got Rejected(ToolCallRejected { .. reason: SchemaInvalid })
  {"values":null} is admitted by {"properties":{"values":{"items":{...},"maxItems":3,"minItems":3,"type":["array","null"]}},"type":"object"}, got Rejected(ToolCallRejected { .. reason: SchemaInvalid })
  ```

**3. Suite run, after the cases existed.**
- With my file: `cargo test -j 8 --no-fail-fast -p inference --test tool_macro_schema_keywords --test adversary_w25_tool_macro --test adversary_w25_tool_macro_pass2 --test tool_macro --test adversary_u4_schema_keywords` exited 101.
  - adversary_u4_schema_keywords: `FAILED. 2 passed; 2 failed`
  - adversary_w25_tool_macro: `ok. 6 passed`
  - adversary_w25_tool_macro_pass2: `ok. 4 passed`
  - tool_macro: `ok. 11 passed`
  - tool_macro_schema_keywords: `ok. 7 passed`
- `cargo test -j 8 -p inference-pipeline --lib nodes::tool_authority` exited 0: `ok. 12 passed; 18 filtered out`.
- Before count: the same `-p inference` command without `adversary_u4_schema_keywords` exited 0 with 6+4+11+7 = 28. Adding the 12 in-module tests gives 40 before and 44 after.

**4. Findings**

| file:line | what is wrong | verdict | origin |
|---|---|---|---|
| crates/inference-pipeline/src/nodes/tool_authority.rs:972 | `prefixItems: []` is accepted. JSON Schema 2020-12 says its value must be a non-empty array, the same rule the unit already enforces for `oneOf`/`anyOf`. This breaks `supported`'s own doc promise ("each in JSON Schema's form"). The fix is to drop the `keyword == "prefixItems" \|\|` exemption. Only a hand-declared `Tool::declare` schema reaches this; schemars never emits it and nothing in the repo writes one. | INFEASIBLE | introduced |
| crates/inference-pipeline/src/nodes/tool_authority.rs:978 | A length keyword written as `1.0` or `2.0` is refused, because `is_u64` is used. In 2020-12 those are integers (the official test suite has "validation with a decimal" cases), and the authority's own `typed("integer")` accepts 1.0. Fixing only :978 is not enough: `admits` at :1081/:1088 uses `limit.as_u64()` and would then refuse every value. Same reach as the row above. At the base the keyword was refused in every form, so the case is red there too, but for a different reason; the form check is the unit's code. | INFEASIBLE | introduced |
| crates/inference-pipeline/src/nodes/tool_authority.rs:1083 | The unit's suite misses the mutant that makes `minLength`/`maxLength` refuse non-strings and `minItems`/`maxItems` (:1090) refuse non-arrays. Real macro arguments reach this: `Option<char>`, `Option<[T; N]>` and `Option<(..)>` passed `null`. The code is correct; my two green cases close the gap. | CONFIRMED | introduced |
| crates/inference-pipeline/src/nodes/tool_authority.rs:987 | The story title says "every argument type schemars describes", but the listed types are covered and other common types are still refused at prepare, without a compile error. By reading schemars 1.2.3: `HashSet`/`BTreeSet` (`uniqueItems`), `NonZero*` (`not`), `#[serde(flatten)]` (`allOf`/`unevaluatedProperties`), chrono durations (`exclusiveMaximum`), `Decimal`/semver (`pattern`). The acceptance statement only covers the listed types, so this does not hold the unit. | INFEASIBLE | pre-existing |

Commit or tree covered: 07cd8dd0.

**5. Attacked and could not break**
- Lengths count code points: emoji and é are admitted as a `char`.
- `prefixItems` together with `items`, and the skip offset.
- `prefixItems` without `items`.
- `minItems`/`maxItems` on values that are not arrays.
- `oneOf` admits exactly one match: none and two both refused.
- `anyOf` admits at least one match.
- `const` treats 1 and 1.0 as equal, and keeps null, false and 0 apart.
- Nested subschemas inside `oneOf`/`anyOf`, and malformed `oneOf: []`, `anyOf: {..}` and `anyOf: [3]`.
- `$ref` nested in `oneOf` is still refused.
- schemars output for internally tagged, adjacently tagged and untagged enums (by reading): it uses only keywords the unit supports.
- Every other mutant I tried on the new lines is caught: comparison flips, `chars()`→`len()`, `==1`→`>=1`, `any`→`all`, a dropped prefix check, a dropped skip, and a dropped `!any_of_holds`.

**6. Paths written outside the worktree**
- the unit's scratch directory `adv1/` (logs, `mutant.diff`, `commit-msg.txt`, and `mutant/`, a mutated copy of the tree with its own `target/`, about 1.2 GB, to delete when the wave is done).

```findings
[
  {"file": "crates/inference-pipeline/src/nodes/tool_authority.rs", "line": 972, "category": "boundary", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "supported accepts the malformed \"prefixItems\": [] although JSON Schema 2020-12 requires a non-empty array, reachable only through a hand-declared Tool schema"},
  {"file": "crates/inference-pipeline/src/nodes/tool_authority.rs", "line": 978, "category": "contract-drift", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "length keywords written with a zero fraction (1.0) are refused at prepare though 2020-12 reads them as integers, and admits' as_u64 at 1081/1088 would refuse every value if only supported were fixed"},
  {"file": "crates/inference-pipeline/src/nodes/tool_authority.rs", "line": 1083, "category": "mutant", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "the unit's suite stays green when the length and item-count checks refuse values of other types, which Option<char> and Option<[T; N]> with null reach; the adversary file's two green cases now catch it"},
  {"file": "crates/inference-pipeline/src/nodes/tool_authority.rs", "line": 987, "category": "judgement", "severity": "note", "verdict": "INFEASIBLE", "origin": "pre-existing", "message": "argument types outside the story's list (HashSet, NonZero, flatten, chrono, Decimal) still neither prepare nor fail to compile, against the story title's \"every argument type schemars describes\""}
]
```
