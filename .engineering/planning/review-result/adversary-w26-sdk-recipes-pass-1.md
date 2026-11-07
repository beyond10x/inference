---
format: aep.planning-md/3
id: review-result:adversary-w26-sdk-recipes-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on the recipes: a replaced node loses the confirmation hook, a held call blocks close'
relations:
- reviews: story:sdk-recipes
revision: 1
---
unit: story:sdk-recipes at 8e8c87b (range b071766..8e8c87b), adversary tests uncommitted then committed with the fixes
verdict: NEEDS-CHANGE
cases: executed 27→31, red 4
origin: introduced 6 / pre-existing 1

1. Blocker, introduced: the authority's confirmation handle was handed out only together with the duplex node's call ids, so `native_voice` with its `speech` node replaced opened with its authority running and `authority()` returning nothing; with `authority` replaced, the executor answered to a second authority that never ran. Test `native::adversary_native_voice_with_its_speech_node_replaced_keeps_its_confirmation_hook`.
2. Warning, introduced: closing a `native_voice` session with a call held unanswered never returned, since the duplex node's drain waits for `tool_result` to end and the authority waits on the held call. Test `native::adversary_closing_with_a_held_call_unanswered_ends`.
3. Warning, introduced: `expose` of a port already exposed under another name exposed it twice, and opening failed with a model node handed two output edges. Test `adversary_exposing_an_exposed_port_under_its_port_name_keeps_one_reply`.
4. Warning, pre-existing and carried into this story: `StackBinding::generator()` handed out a hosted generator under `offline_strict`. Test `adversary_locality::adversary_an_offline_strict_stack_does_not_hand_out_a_hosted_generator`.
5. Note, introduced: a `declare_into` refused for an expose name or an unadded role-bound node left its pipeline, nodes and earlier bindings behind.
6. Note, introduced: `RecipeRegistered.name` was declared generated in `ess/domains/sdk.yaml`, though the recipe is keyed by the name sent.
7. Note, introduced: `with_node` accepted a name the recipe has no node for and never used it.

Fixed in e8ea264 (`AuthorityHandle::close`), a60e8df (findings 1, 2, 3, 5, 7), 18fe355 (finding 4) and 6a98498 (finding 6). Closing denies a held call with the reason `denied`; a reason that names the close would be a change to `ess/domains/tools.yaml`, and whether the model receives that denial while the duplex node drains is not asserted.
