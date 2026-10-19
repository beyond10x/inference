---
format: aep.planning-md/3
id: review-result:review-030-pipeline
kind: review-result
status: active
title: Read-only review of inference-pipeline and the graph target since 0.3.0 (haiku)
relations:
- reviews: story:pipeline-declaration
- reviews: story:pipeline-validation
prose_only: read-only general-purpose reviewer requested by the operator; findings are prose, each traced to a line
revision: 1
---
defects found: 6

Read-only review. I did not run `cargo test` because it writes to `target/`, so every finding below comes from reading the code path. Two are verified end to end by reading; the rest are marked by how sure I am.

1. **Forced `rejected` skips the store's state and existence checks** (verified)
   - `crates/inference-testkit/tests/target/graph.rs:304-306`: when `forced` is set, `validate_pipeline` returns `rejected(&domain.declaration().findings(...))` before the `Declaration::validate_pipeline` state check runs.
   - Concrete input: DeclarePipeline, then ValidatePipeline (answers `validated`, state becomes Validated), then force `ValidatePipeline/rejected`, then ValidatePipeline again. The target answers `rejected` with `findings: []` and no `wrong-state`. ESS says a non-Declared pipeline gets `wrong-state`, and `IncompatibleGraph` is described as naming each finding.
   - The test `graph.rs:1081-1142` codifies the empty-findings rejection on a valid Declared pipeline. The module doc (`graph.rs:11-16`) admits "none for a valid or unknown one". That is a fabricated outcome, not one decided by the store.

2. **Typed builder bypasses the catalog checks that the commands make** (verified)
   - `crates/inference-pipeline/src/builder.rs:12-14` says "What the commands refuse cannot be written here". That is false.
   - `build(&profile, ...)` at `builder.rs:359-371` takes any `&DeploymentProfile`. `DeploymentProfile::declare` in `inference-core/src/policy.rs:220` is public and needs no store, so a pipeline can reference a profile no catalog holds. `DeclarePipeline`'s `unknown-profile` never fires.
   - `bind(node, adapter: &Adapter)` at `builder.rs:267-271` takes a value, and `Adapter::declare` (`inference-core/src/model.rs:340`) is public. The bound adapter need not be declared, so `BindNode`'s `unknown-adapter` is bypassed.

3. **Identity counters collide across testkit domains** (verified path, impact needs a wrong-typed id; not run)
   - `crates/inference-testkit/src/policy.rs:49` issues profile `…0001` from its own counter. `crates/inference-testkit/src/graph.rs:70` uses `Declaration::counted()`, which also issues `…0001` first. `crates/inference-testkit/src/model.rs:193,205,211` does the same for model and adapter ids.
   - The target parses each id as a bare UUID (`tests/target/mod.rs:422-427`) and matches on the UUID alone, so an id of one type is accepted where another type is expected.
   - Concrete input: declare a profile (id `…0001`), declare a pipeline (id `…0001`), then `ValidatePipeline { pipeline_id: <profile id> }` returns `validated`. ESS would answer `wrong-state`/unknown for a profile id used as a pipeline id.

4. **A Validated pipeline can be made invalid** (verified by reading; the gap is documented, so it is a known hole, not a silent one)
   - `crates/inference-pipeline/src/declaration.rs:250-257`: `add_port` checks only that the node exists, with no pipeline-state guard. `graph.yaml:684-690` marks this as UNMAPPED.
   - Concrete input: source out (Essential, Text) connected to sink in, ValidatePipeline returns `validated`, then `AddPort` an unconnected Essential input on the sink. That succeeds, the pipeline stays Validated, and §10.2 check 6 is now violated. Nothing re-checks until planning, which is not implemented.

5. **Essential unconnected input ports are rejected as "unreachable mandatory output"** (suspected; spec reading, not a traced failure)
   - `crates/inference-pipeline/src/validation.rs:215-227` flags every Essential port with no edge, inputs included. §10.2 item 6 speaks of mandatory outputs only.
   - Concrete input: a Source node with an Essential input port that nothing feeds is rejected. `PortTraits::default()` (`builder.rs:56-66`) makes every port Essential by default, so this hits builder users who take the defaults. The code is consistent with its own doc (`lib.rs:~224`, `GraphDefect::UnreachableMandatoryOutput`), so the question is the spec, not a code/doc mismatch.

6. **Doc comment contradicts the variant set** (verified, minor)
   - `crates/inference-pipeline/src/lib.rs:266-267`: `PipelineState` is documented as "Declared until validated, then Planned". It has a `Validated` variant, which the doc skips.

Checked and found consistent with `ess/domains/graph.yaml` and `policy.yaml`: refusal order for `ConnectPorts`, `BindNode`, `AddNode`, `DeclareProfile` (`TableRule::broken_by` matches the ESS order), error field names, event and view field names, enum variant names (serde snake_case), `validation.rs` cycle detection (SCC naming), the `capacity` check (`bounded`), the absence of panics reachable from input, and scenario reset between `begin_scenario`/`end_scenario`. `ess/SKIPPED.md` could not be checked against the live suite without running it.
