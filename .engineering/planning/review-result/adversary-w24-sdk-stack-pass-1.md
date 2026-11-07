---
format: aep.planning-md/3
id: review-result:adversary-w24-sdk-stack-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the SDK stack
relations:
- reviews: story:sdk-stack
revision: 1
---
unit: W24-sdk-stack story:sdk-stack, tree inference-0-24-0-sdk-stack, unit head 0ad2f5d, findings cover 24330a5
verdict: NEEDS-CHANGE
cases: executed 30→34, red 2
origin: introduced 9 / pre-existing 0 / undecided 0
wrote-outside-worktree: 4 paths (part 6)
needs-coordinator: whether to change stack.rs (or the story) so a builder binding can replace a binding that came from the environment or a file; amending the CapabilityRecord acceptance line

The run took about 0.5 cargo minutes and roughly 25 wall minutes, most of it waiting for the build slot. `target/` peaked at 426M and has been discarded back to 4K.

1. `git --no-pager diff --stat 0ad2f5d..HEAD`: `crates/inference/tests/adversary_w24_stack.rs | 208 +++` (1 file). It is a test file only. The commit is 24330a5, authored by `b10x-bot[bot]`, not pushed.

2. The cases are in `crates/inference/tests/adversary_w24_stack.rs`, each with a 20 s deadline. I ran them alone first: `cargo-run.sh test -p inference --test adversary_w24_stack` gave 2 passed, 2 failed (rc=101, secs=9).
   - **Red** `a_builder_binding_onto_a_stack_resolved_from_the_environment_wins`:
     - Setup: resolve with `INFERENCE_LLM_ADAPTER=A`, then `.bind(Binding::named(Llm, B))` and resolve again. The case expects `B` with source `Builder`.
     - Captured output: `resolving refused: the role llm is bound twice by the builder; bind each role once`
   - **Red** `a_relative_stack_file_is_not_read_from_the_process_directory`:
     - Setup: `Env::new().var("INFERENCE_STACK","Cargo.toml")`.
     - Captured output: `Ok(Stack { … })`. Resolve read the relative path against the process's current directory, though the `Env` has no working directory.
   - **Green** `expansion_is_one_pass_and_injects_no_key`: `${A}` where `A="${C}"` is not expanded again, `$B` stays literal, and a value containing `"\n[stt]` adds no key and binds no role.
   - **Green** `an_empty_adapter_variable_gives_way_to_the_file`.

3. The suite, run after the cases existed: `cargo-run.sh test -p inference --features testing --no-fail-fast --test stack --test adversary_w24_stack`
   - `adversary_w24_stack`: 2 passed; 2 failed.
   - `stack`: 30 passed; 0 failed.
   - `error: 1 target failed`, rc=101 (secs=1).
   - The before count of 30 is the `stack` target alone, with my file excluded. The implementor's 33 came from a wider feature set.

4. Findings that are judgement only (tree 24330a5):
   - **stack.rs:842**, NEEDS-CHANGE: this is the first red case.
     - What reaches it: design §3.2, where `main` passes `Stack::from_env()` into `run`. story:sdk-agent then calls `Agent::voice_with(stack)`, and its `.llm(..)` "binds through `Stack::bind`". With `INFERENCE_LLM_ADAPTER` set, that path errors instead of letting the builder win (§4.2), and the message wrongly blames the builder.
     - Fix: when a pending builder binding meets a binding whose source is not `Builder`, replace it, and refuse only builder against builder.
   - **stack.rs:930**, INFEASIBLE: this is the second red case.
     - What reaches it: only a hand-built `Env::new()` with a relative `INFERENCE_STACK`. `Env::process()` always sets a working directory.
     - Fix: refuse a relative path when the `Env` has no working directory.
   - **The locality-denied scenario**, warning: the conform target (`tests/target/sdk.rs`, `(Some("locality-denied"), Ok(stack))` arm) works out the refusal itself from `Catalog`. No SDK code refuses a denied locality.
     - The story defers that refusal to sdk-recipes planning. Until then, `StackBinding::generator()` (stack.rs:530, not in the story's plan) returns a usable hosted generator under the default `offline_strict` profile without any check.
     - `AdapterDescriptor.locality` is never compared with anything.
   - **registry/sherpa_onnx.rs:5 and registry/whispercpp.rs:5**, warning: both open their checkpoint during resolve. That contradicts stack.rs:12-14 ("opens nothing"), design §4.4 ("Resolving a stack does no I/O") and story R110. I did not build them (forbidden features).
   - **The CapabilityRecord acceptance line**, INFEASIBLE: no adapter produces a `CapabilityRecord` at runtime. The records are per-hardware JSON files that only tests read, and `adapter_id` changes on every constructor call. So "equal to the ones the registry declared" has nothing to compare against.
     - A small in-scope change (a `Binding::capability(..)` returning `Option`) would meet it only with `None == None`, which proves nothing. The coordinator should amend the story.
   - **sdk.yaml `RoleBoundTwice`**, note: the ESS text says "One source binds a role twice", but only the builder can. A stack file with `[llm]` twice is refused as `StackFile` "not TOML".
   - **stack.rs:986-995**, note: an unregistered adapter named in a stack file returns `StackFile` with no `AdapterNotRegistered` as its source. ESS `unknown-adapter` names that error, and the conform target only matches it by string.
   - **stack.rs:871**, note: every stack file is parsed even when the builder or the environment already settles every role and the profile. A broken `~/.config/inference/inference.toml` therefore fails `from_env` for every application.

5. What I attacked and could not break:
   - **Precedence:** the five sources in order for roles and for the profile, and a partial configuration (environment keys without an environment adapter are ignored, consistently).
   - **Expansion:** missing variable, recursion, a literal `$`, key injection.
   - **TOML:** wrong types, nested values, a missing `adapter`. Typos in role or top-level keys are silently ignored; I did not test that.
   - **Registry:** a duplicate name or a built-in name is refused.
   - **Constructors:** a binding of another role, or one returned by name, is refused.
   - **No credential or network at resolve:** `EnvironmentSource::openai()` and `OpenAiRealtime::new` read nothing, and `Deferred` loads the AWS configuration only at connect.
   - **Error messages:** each one names its fix, except the misleading RoleBoundTwice above.

6. Paths written outside the worktree, all in the scratch directory `<scratch>`:
   - `adv1-case.log`
   - `adv1-suite.log`
   - `msg.WlkgpL`
   - The harness also wrote background-task output under `<scratch>`.

7.
```findings
[
{"file":"crates/inference/src/stack.rs","line":842,"category":"acceptance","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"a builder binding onto a stack resolved from the environment or a file is refused as RoleBoundTwice 'by the builder' instead of winning, which breaks Agent::voice_with(Stack::from_env()).llm(..)"},
{"file":"crates/inference/src/stack.rs","line":930,"category":"boundary","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"with no working directory in the Env, a relative INFERENCE_STACK is read against the process's current directory"},
{"file":"crates/inference-testkit/tests/target/sdk.rs","category":"judgement","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"the locality-denied scenario is answered by the target's own computation over Catalog, no SDK code refuses a denied locality, and StackBinding::generator hands out a hosted generator under offline_strict"},
{"file":"crates/inference/src/registry/sherpa_onnx.rs","line":5,"category":"contract-drift","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"the sherpa-onnx and whispercpp registrations open their checkpoint at resolve, contradicting 'resolving does no I/O' in stack.rs:12, design section 4.4 and R110"},
{"file":"crates/inference/src/stack.rs","line":1067,"category":"acceptance","severity":"warning","verdict":"INFEASIBLE","origin":"introduced","message":"the acceptance line requiring Stack::adapter to return a CapabilityRecord equal to the registry's has no record to compare against, so the story line needs amending"},
{"file":"ess/domains/sdk.yaml","category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"RoleBoundTwice says one source binds a role twice, but only the builder can, and a duplicate role table in a file is refused as StackFile 'not TOML'"},
{"file":"crates/inference/src/stack.rs","line":989,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"an unregistered adapter named in a stack file returns StackFile without AdapterNotRegistered as its source, unlike the ESS unknown-adapter error"},
{"file":"crates/inference/src/stack.rs","line":871,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"every stack file is parsed even when higher sources settle every role and the profile, so a broken user config file fails every from_env"},
{"file":"crates/inference/src/registry/mod.rs","line":57,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"AdapterDescriptor's locality is never compared with the constructed adapter's locality or the profile"}
]
```
