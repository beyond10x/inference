---
format: aep.planning-md/3
id: review-result:adversary-realtime-session-input-options-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on the realtime session input options: no findings'
relations:
- reviews: story:realtime-session-input-options
revision: 1
---
## Report

Adversary pass 1 on `impl/realtime-session-input-options` (diff `6cd4f0f..5488673`), cases
committed on top as `6dc3d54`,
`adapters/inference-openai/tests/adversary15_u1_session_input_options.rs`, 10 cases. The report
as it returned:

```
unit: U1 story:realtime-session-input-options
verdict: nothing found
cases: executed 65→75, red 0
origin: introduced 0, pre-existing 0, undecided 0
wrote-outside-worktree: 3 paths (own.log, suite.log, msg file under the u1/adv scratch root)
needs-coordinator: none
```

1. Touched one file, `adapters/inference-openai/tests/adversary15_u1_session_input_options.rs`,
   411 insertions; `src/` and `Cargo.toml` untouched. Commit `6dc3d54` through the bot route.
2. Cases added, run alone first: 10 passed, 0 failed, exit 0. They stay as regression cases:
   `a_full_0_9_0_server_vad_config_serialises_as_it_did`,
   `a_full_0_9_0_semantic_vad_config_serialises_as_it_did`,
   `every_input_option_together_is_the_exact_audio_input_object`,
   `noise_reduction_off_and_turn_detection_off_are_both_null`,
   `language_alone_and_prompt_alone_are_sent_alone`,
   `a_language_without_a_model_leaves_no_trace`,
   `a_function_tool_choice_is_sent_verbatim_without_tools`,
   `tool_choice_none_is_the_string_none`,
   `the_new_types_are_exported_with_their_derives`,
   `update_session_mid_session_carries_the_input_options`.
3. Suite run (`cargo test -p inference-openai --features duplex`, `--lib` and six named
   targets), exit 0: lib 0/0, adversary15_u1_session_input_options 10/0, adversary1_realtime
   12/0, adversary2_duplex 8/0, duplex 28/0, offline 20/0, session_input_options 5/0. fmt exit 0;
   clippy `--all-targets --features duplex,gpt-live -- -D warnings` exit 0.
4. Judgement findings: none.
5. Attacked and could not break: wire shape against the brief (key names, nesting, `null` versus
   absent, language and prompt dropped without a model); 0.9.0 byte identity for fully
   populated configs; unit tests that cannot fail (none); other `SessionConfig` literals (all use
   `..Default` or are `inference_duplex::SessionConfig`); doc comments and `lib.rs` exports; no
   new parsing path. `Some("")` for the language or prompt is sent as `""`; nothing in the crate
   passes an empty value, so not counted.

```findings
[]
```
