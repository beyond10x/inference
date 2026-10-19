//! Adversary pass 2 against the one target-wide sequence: `Stores::sequence`, `Stores::token` and
//! `Stores::require` of `target/mod.rs`, driven through the shipped `inference.model` and
//! `inference.stream` modules.
//!
//! The runner reads a view at the token of the scenario's last command, whatever its domain, so a
//! token is a number of the whole target: every accepted command advances it by the events it
//! published, a refusal leaves it where it was, and a view of either domain read at any token the
//! target answered is answered. `ObservedEvent::at` places an occurrence "in the target's own
//! publication order", which is one order for the target as well.
//!
//! A run store does not exist yet, so a generation is arranged with the store's own
//! `start_generation`, as the module tests do. The two modules are loaded as modules of this file
//! because `target/mod.rs` keeps them private; their functions run unchanged.

#![allow(clippy::duplicate_mod)]

mod target;

#[allow(dead_code)]
#[path = "target/model.rs"]
mod model;

#[allow(dead_code)]
#[path = "target/stream.rs"]
mod stream;

use std::collections::BTreeMap;

use ess_conformance::scenario::{CommandRef, EventRef, ViewRef};
use ess_conformance::target::{
    Deadline, EventObservationRequest, SemanticCommandRequest, SemanticCommandResult,
    SemanticViewRequest, TargetError,
};
use ess_primitives::consistency::{ConsistencyToken, QueryConsistency};
use ess_primitives::facts::Number;
use ess_primitives::ids::CorrelationId;
use ess_primitives::node::Node;
use ess_primitives::time::Timestamp;
use inference_core::generation::StartGeneration;
use inference_core::id::{NodeId, RunId};
use uuid::Uuid;

use target::{Input, Stores, integer_node, named_list_node, named_node, text_node, unsupported};

const VIEWS: [&str; 4] = [
    "inference.model.Models",
    "inference.model.Adapters",
    "inference.stream.Generations",
    "inference.stream.Segments",
];

const UNKNOWN: &str = "00000000-0000-4000-8000-0000deadbeef";

fn correlation() -> CorrelationId {
    CorrelationId::new("adversary3-sequence").expect("a correlation id")
}

fn text(value: &str) -> Node {
    Node::Text(value.to_owned())
}

fn command(name: &str, input: Vec<(&str, Node)>) -> SemanticCommandRequest {
    SemanticCommandRequest {
        command: name.parse::<CommandRef>().expect("a command name"),
        actor: None,
        caller: None,
        input: input
            .into_iter()
            .map(|(field, value)| (field.to_owned(), value))
            .collect(),
        correlation: correlation(),
    }
}

fn declare_model() -> SemanticCommandRequest {
    let list = |values: &[&str]| Node::Seq(values.iter().map(|value| text(value)).collect());
    command(
        "inference.model.DeclareModel",
        vec![
            ("family", text("family")),
            ("checkpoint", text("checkpoint")),
            ("code_license", text("code_license")),
            ("weight_license", text("weight_license")),
            ("checksum", text("checksum")),
            ("input_modalities", list(&["Text"])),
            ("output_modalities", list(&["Audio"])),
            ("provenance", text("provenance")),
            ("source_revision", text("source_revision")),
        ],
    )
}

fn declare_adapter(model_id: &str) -> SemanticCommandRequest {
    command(
        "inference.model.DeclareAdapter",
        vec![
            ("model_id", text(model_id)),
            ("backend", text("backend")),
            ("contract_revision", text("contract_revision")),
            ("execution_form", text("in_process")),
            ("locality", text("same_process")),
            ("capabilities", Node::Seq(vec![text("tool_calling")])),
            ("max_concurrency", Node::Number(Number::from(1_i64))),
        ],
    )
}

fn emit(generation_id: &str, sequence: i64) -> SemanticCommandRequest {
    command(
        "inference.stream.EmitTextDelta",
        vec![
            ("generation_id", text(generation_id)),
            ("sequence", Node::Number(Number::from(sequence))),
            ("text", text("hello")),
        ],
    )
}

fn on_generation(name: &str, generation_id: &str) -> SemanticCommandRequest {
    command(
        &format!("inference.stream.{name}"),
        vec![("generation_id", text(generation_id))],
    )
}

fn on_segment(name: &str, segment_id: &str) -> SemanticCommandRequest {
    command(
        &format!("inference.stream.{name}"),
        vec![("segment_id", text(segment_id))],
    )
}

fn view(name: &str, consistency: QueryConsistency) -> SemanticViewRequest {
    SemanticViewRequest {
        view: name.parse::<ViewRef>().expect("a view name"),
        params: BTreeMap::new(),
        consistency,
        correlation: correlation(),
        deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
    }
}

fn execute(
    stores: &mut Stores,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    if request.command.to_string().starts_with("inference.model.") {
        model::execute_command(stores, request)
    } else {
        stream::execute_command(stores, request)
    }
}

fn read(
    stores: &mut Stores,
    name: &str,
    consistency: QueryConsistency,
) -> Result<usize, TargetError> {
    let request = view(name, consistency);
    let answer = if name.starts_with("inference.model.") {
        model::query_view(stores, &request)
    } else {
        stream::query_view(stores, &request)
    };
    answer.map(|answer| answer.rows.len())
}

fn arrange_generation(stores: &mut Stores, epoch: i64) -> String {
    stores
        .stream
        .start_generation(StartGeneration {
            run_id: RunId::from(Uuid::from_u128(0xaa)),
            node_id: NodeId::from(Uuid::from_u128(0xbb)),
            epoch,
        })
        .generation_id
        .to_string()
}

/// The token a command answered, as a number.
fn token_of(result: &SemanticCommandResult) -> u64 {
    let token = result
        .consistency
        .as_ref()
        .expect("a command answers a token");
    token
        .as_str()
        .strip_prefix("seq:")
        .and_then(|digits| digits.parse().ok())
        .unwrap_or_else(|| panic!("a token the target writes: {token:?}"))
}

fn field(result: &SemanticCommandResult, name: &str) -> String {
    match result
        .direct_events
        .first()
        .and_then(|event| event.payload.get(name))
    {
        Some(Node::Text(value)) => value.clone(),
        other => panic!("`{name}` of {result:?}: {other:?}"),
    }
}

fn at(sequence: u64) -> QueryConsistency {
    QueryConsistency::at_least(ConsistencyToken::new(format!("seq:{sequence}")).expect("a token"))
}

#[test]
fn every_accepted_command_of_either_domain_advances_the_one_token_and_a_refusal_does_not() {
    let mut stores = Stores::default();
    let mut tokens = Vec::new();
    let mut step =
        |stores: &mut Stores, label: &str, request: SemanticCommandRequest, want: u64| {
            let result =
                execute(stores, &request).unwrap_or_else(|error| panic!("{label}: {error}"));
            assert_eq!(token_of(&result), want, "{label}: {result:?}");
            tokens.push(want);
            result
        };

    let model = step(&mut stores, "DeclareModel", declare_model(), 1);
    let model_id = field(&model, "model_id");
    let refused = step(
        &mut stores,
        "DeclareAdapter, unknown model",
        declare_adapter(UNKNOWN),
        1,
    );
    assert!(
        refused.error.is_some() && refused.direct_events.is_empty(),
        "{refused:?}"
    );
    step(&mut stores, "DeclareAdapter", declare_adapter(&model_id), 2);

    let generation = arrange_generation(&mut stores, 1);
    let delta = step(&mut stores, "EmitTextDelta", emit(&generation, 1), 4);
    let segment = field(&delta, "segment_id");
    step(
        &mut stores,
        "CommitSegment, unknown",
        on_segment("CommitSegment", UNKNOWN),
        4,
    );
    step(
        &mut stores,
        "QueueForSynthesis, Tentative",
        on_segment("QueueForSynthesis", &segment),
        4,
    );
    step(&mut stores, "EmitTextDelta, unknown", emit(UNKNOWN, 2), 4);
    step(
        &mut stores,
        "CommitSegment",
        on_segment("CommitSegment", &segment),
        5,
    );
    step(
        &mut stores,
        "SupersedeGeneration, with a committed segment",
        on_generation("SupersedeGeneration", &generation),
        6,
    );
    step(
        &mut stores,
        "EmitTextDelta, superseded",
        emit(&generation, 2),
        6,
    );
    step(
        &mut stores,
        "SupersedeGeneration, again",
        on_generation("SupersedeGeneration", &generation),
        6,
    );

    let mut undeclared = declare_model();
    undeclared
        .input
        .insert("undeclared".to_owned(), text("undeclared"));
    assert!(execute(&mut stores, &undeclared).is_err());
    let mut malformed = emit(&generation, 3);
    malformed.input.remove("text");
    assert!(execute(&mut stores, &malformed).is_err());
    step(
        &mut stores,
        "DeclareModel, after two errors",
        declare_model(),
        7,
    );

    for token in tokens.iter().copied().chain([0, 7]) {
        for name in VIEWS {
            let answer = read(&mut stores, name, at(token));
            assert!(answer.is_ok(), "`{name}` at seq:{token}: {answer:?}");
        }
    }
    for name in VIEWS {
        for consistency in [
            at(8),
            QueryConsistency::at_least(ConsistencyToken::new("v:1").expect("a token")),
        ] {
            let answer = read(&mut stores, name, consistency.clone());
            assert!(
                matches!(answer, Err(TargetError::Unavailable { .. })),
                "`{name}` at {consistency:?}: {answer:?}"
            );
        }
    }
}

/// A small fixed-seed generator, so that a failing walk replays.
struct Walk(u64);

impl Walk {
    fn next(&mut self, below: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from((self.0 >> 33) % u64::try_from(below).expect("small")).expect("small")
    }
}

#[test]
fn a_fixed_seed_walk_across_both_domains_moves_the_token_by_exactly_the_events_published() {
    const SEGMENT_COMMANDS: [&str; 5] = [
        "CommitSegment",
        "QueueForSynthesis",
        "CompleteSynthesis",
        "AcknowledgePlayback",
        "DiscardSegment",
    ];
    const GENERATION_COMMANDS: [&str; 3] = [
        "CompleteGeneration",
        "SupersedeGeneration",
        "CancelGeneration",
    ];

    for seed in [1_u64, 7, 42, 1_000, 65_537] {
        let mut walk = Walk(seed);
        let mut stores = Stores::default();
        let mut expected = 0_u64;
        let mut models = vec![UNKNOWN.to_owned()];
        let mut generations: Vec<String> = vec![UNKNOWN.to_owned()];
        let mut segments = vec![UNKNOWN.to_owned()];
        let mut epoch = 0;
        for index in 0..400 {
            let choice = walk.next(6);
            let request = match choice {
                0 => declare_model(),
                1 => declare_adapter(&models[walk.next(models.len())]),
                2 => {
                    epoch += 1;
                    generations.push(arrange_generation(&mut stores, epoch));
                    expected += 1;
                    continue;
                }
                3 => emit(&generations[walk.next(generations.len())], index),
                4 => on_segment(
                    SEGMENT_COMMANDS[walk.next(SEGMENT_COMMANDS.len())],
                    &segments[walk.next(segments.len())],
                ),
                _ => on_generation(
                    GENERATION_COMMANDS[walk.next(GENERATION_COMMANDS.len())],
                    &generations[walk.next(generations.len())],
                ),
            };
            let context = format!("seed {seed}, step {index}, {}", request.command);
            let result =
                execute(&mut stores, &request).unwrap_or_else(|error| panic!("{context}: {error}"));
            let published = u64::try_from(result.direct_events.len()).expect("small");
            assert!(published <= 1, "{context}: {result:?}");
            assert_eq!(
                published == 1,
                result.error.is_none(),
                "{context}: an accepted command publishes one event, a refusal none: {result:?}"
            );
            expected += published;
            assert_eq!(token_of(&result), expected, "{context}: {result:?}");
            if published == 1 {
                match choice {
                    0 => models.push(field(&result, "model_id")),
                    3 => segments.push(field(&result, "segment_id")),
                    _ => {}
                }
            }
            let name = VIEWS[walk.next(VIEWS.len())];
            let earlier = u64::try_from(walk.next(usize::try_from(expected).expect("small") + 1))
                .expect("small");
            let answer = read(&mut stores, name, at(earlier));
            assert!(
                answer.is_ok(),
                "{context}: `{name}` at seq:{earlier}: {answer:?}"
            );
            let ahead = read(&mut stores, name, at(expected + 1));
            assert!(
                matches!(ahead, Err(TargetError::Unavailable { .. })),
                "{context}: `{name}` at seq:{}: {ahead:?}",
                expected + 1
            );
        }
        assert_eq!(stores.sequence(), expected, "seed {seed}");
        assert!(
            expected > 50,
            "seed {seed}: the walk accepted {expected} commands only"
        );
    }
}

#[test]
fn observed_event_positions_are_per_domain_until_the_envelope_sequences_them() {
    let mut stores = Stores::default();
    execute(&mut stores, &declare_model()).expect("DeclareModel answers");
    let generation = arrange_generation(&mut stores, 1);
    let delta = execute(&mut stores, &emit(&generation, 1)).expect("EmitTextDelta answers");
    assert_eq!(token_of(&delta), 3, "{delta:?}");

    let mut positions = Vec::new();
    for event in [
        "inference.model.ModelDeclared",
        "inference.stream.GenerationStarted",
        "inference.stream.TextDelta",
    ] {
        let request = EventObservationRequest {
            event: event.parse::<EventRef>().expect("an event name"),
            correlation: correlation(),
            deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
        };
        let observed = if event.starts_with("inference.model.") {
            model::observe_events(&mut stores, &request)
        } else {
            stream::observe_events(&mut stores, &request)
        }
        .expect("the event is observed");
        assert_eq!(observed.len(), 1, "{event}: {observed:?}");
        positions.push((event, observed[0].sequence));
    }
    assert_eq!(
        positions,
        vec![
            ("inference.model.ModelDeclared", Some(1)),
            ("inference.stream.GenerationStarted", Some(1)),
            ("inference.stream.TextDelta", Some(2)),
        ],
        "observed-event positions count per domain today, while tokens are target-wide; \
         story:inbound-text-spans-and-envelope (## Event order across domains) numbers them in \
         the target's one publication order. When it does, expect positions 1, 2 and 3 here"
    );
}
