//! Adversary cases for the envelope log of `story:inbound-text-spans-and-envelope`, driven from
//! `ess/domains/stream.yaml` read as written: every `inference.stream` event the log can record is
//! named and carries a generation exactly as its declaration says, a refused event leaves no trace
//! (its id stays free), the three rules hold together under a fixed-seed walk against a model,
//! and an envelope's `kind` is the declared name of its payload's event.

use std::collections::{BTreeMap, BTreeSet};

use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::generation::{
    GenerationCancelled, GenerationCompleted, GenerationStarted, GenerationSuperseded,
};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_testkit::stream::{EnvelopeRefusal, EventLog, StreamEvent};
use inference_text::{
    AudioPlayed, SegmentDiscarded, SegmentId, SegmentQueued, SpanId, SynthesisCompleted, TextDelta,
    TextSegmentCommitted, TranscriptCommitted, TranscriptHypothesis, TranscriptRevision,
    TurnCompleted, TurnId, TurnOpened,
};
use uuid::Uuid;

const STREAM_YAML: &str = include_str!("../../../ess/domains/stream.yaml");

/// The entries of the top-level `section` of `ess/domains/stream.yaml`, each with the names of
/// its `fields:`, read line by line as the file is written.
fn declared(section: &str) -> Vec<(String, Vec<String>)> {
    let mut entries: Vec<(String, Vec<String>)> = Vec::new();
    let mut inside = false;
    let mut key = String::new();
    for line in STREAM_YAML.lines() {
        let body = line.trim_start();
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        let indent = line.len() - body.len();
        if indent == 0 {
            inside = body == format!("{section}:");
            continue;
        }
        if !inside {
            continue;
        }
        match indent {
            2 => {
                if let Some(name) = body.strip_prefix("- name: ") {
                    entries.push((name.to_owned(), Vec::new()));
                    key.clear();
                }
            }
            4 => body.trim_end_matches(':').clone_into(&mut key),
            6 if key == "fields" => {
                if let (Some(name), Some(entry)) =
                    (body.strip_prefix("- name: "), entries.last_mut())
                {
                    entry.1.push(name.to_owned());
                }
            }
            _ => {}
        }
    }
    entries
}

fn id(low: u128) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 | low)
}

fn generation(low: u128) -> GenerationId {
    GenerationId::from(id(0x6000 | low))
}

fn stream(low: u128) -> StreamId {
    StreamId::from(id(0x5000 | low))
}

/// One event of every `StreamEvent` variant; the ones whose payload has a generation carry
/// `generation_id`.
fn one_of_each(generation_id: GenerationId) -> Vec<StreamEvent> {
    let segment_id = SegmentId::from(id(0x7001));
    let span_id = SpanId::from(id(0x8001));
    let turn_id = TurnId::from(id(0x9001));
    vec![
        StreamEvent::SpeechStarted(
            serde_json::from_value(serde_json::json!({"onset": 0})).expect("a sample position"),
        ),
        StreamEvent::SpeechEnded(
            serde_json::from_value(serde_json::json!({"offset": 160})).expect("a sample position"),
        ),
        StreamEvent::TurnOpened(TurnOpened {
            turn_id,
            run_id: RunId::from(id(0x2000)),
        }),
        StreamEvent::TurnCompleted(TurnCompleted { turn_id }),
        StreamEvent::TranscriptHypothesis(TranscriptHypothesis {
            span_id,
            turn_id,
            sequence: 1,
            text: "hello wor".to_owned(),
        }),
        StreamEvent::TranscriptRevision(TranscriptRevision {
            span_id,
            text: "hello world".to_owned(),
        }),
        StreamEvent::TranscriptCommitted(TranscriptCommitted {
            span_id,
            text: "hello world".to_owned(),
        }),
        StreamEvent::GenerationStarted(GenerationStarted {
            generation_id,
            run_id: RunId::from(id(0x2000)),
            epoch: 1,
        }),
        StreamEvent::GenerationCompleted(GenerationCompleted { generation_id }),
        StreamEvent::GenerationSuperseded(GenerationSuperseded { generation_id }),
        StreamEvent::GenerationCancelled(GenerationCancelled { generation_id }),
        StreamEvent::TextDelta(TextDelta {
            segment_id,
            generation_id,
            sequence: 1,
            text: "hi".to_owned(),
        }),
        StreamEvent::TextSegmentCommitted(TextSegmentCommitted {
            segment_id,
            text: "hi".to_owned(),
        }),
        StreamEvent::SegmentQueued(SegmentQueued { segment_id }),
        StreamEvent::SynthesisCompleted(SynthesisCompleted { segment_id }),
        StreamEvent::AudioPlayed(AudioPlayed { segment_id }),
        StreamEvent::SegmentDiscarded(SegmentDiscarded { segment_id }),
    ]
}

/// `payload` enveloped as event `event` at `sequence` on `stream_id`, its `kind` the payload's
/// declared name, under the envelope generation `generation_id`.
fn envelope(
    event: u128,
    stream_id: StreamId,
    sequence: i64,
    generation_id: Option<GenerationId>,
    payload: StreamEvent,
) -> EventEnvelope<StreamEvent> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::from(id(event)),
        run_id: RunId::from(id(0x2000)),
        session_id: None,
        stream_id,
        sequence,
        producer: NodeId::from(id(0x3000)),
        port: "out".to_owned(),
        kind: payload.name().to_owned(),
        generation_id,
        timestamp: sequence,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload,
    }
}

fn delta(generation_id: GenerationId) -> StreamEvent {
    StreamEvent::TextDelta(TextDelta {
        segment_id: SegmentId::from(id(0x7001)),
        generation_id,
        sequence: 1,
        text: "hi".to_owned(),
    })
}

#[test]
fn every_stream_event_is_named_and_carries_a_generation_exactly_as_stream_yaml_declares() {
    let events = declared("events");
    assert_eq!(
        events.len(),
        17,
        "stream.yaml declares 17 events: {events:?}"
    );
    let carried = generation(9);
    let all = one_of_each(carried);

    let names: BTreeSet<&str> = all.iter().map(StreamEvent::name).collect();
    let declared_names: BTreeSet<&str> = events.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names, declared_names,
        "one StreamEvent per declared event, each named as declared"
    );

    for event in &all {
        let fields = &events
            .iter()
            .find(|(name, _)| name == event.name())
            .expect("declared")
            .1;
        let declares = fields.iter().any(|field| field == "generation_id");
        assert_eq!(
            event.generation_id(),
            declares.then_some(carried),
            "{}: the payload carries a generation iff stream.yaml declares the field",
            event.name()
        );
        if declares {
            let mut log = EventLog::new();
            assert_eq!(
                log.record(envelope(
                    1,
                    stream(1),
                    1,
                    Some(generation(8)),
                    event.clone()
                )),
                Err(EnvelopeRefusal::GenerationMismatch {
                    payload: carried,
                    envelope: Some(generation(8)),
                }),
                "{}: an envelope naming another generation is refused",
                event.name()
            );
            assert!(log.is_empty());
        }
    }
}

#[test]
fn an_event_refused_for_its_sequence_or_generation_can_be_sent_again_under_the_same_id() {
    let mut log = EventLog::new();
    let g = generation(1);
    log.record(envelope(1, stream(1), 5, Some(g), delta(g)))
        .expect("a first event is recorded");

    assert_eq!(
        log.record(envelope(2, stream(1), 5, Some(g), delta(g))),
        Err(EnvelopeRefusal::SequenceNotIncreasing {
            stream_id: stream(1),
            last: 5,
            sequence: 5,
        })
    );
    log.record(envelope(2, stream(1), 6, Some(g), delta(g)))
        .expect("an event refused for its sequence was not recorded, so its id is free");

    assert_eq!(
        log.record(envelope(3, stream(1), 7, None, delta(g))),
        Err(EnvelopeRefusal::GenerationMismatch {
            payload: g,
            envelope: None,
        })
    );
    log.record(envelope(3, stream(1), 7, Some(g), delta(g)))
        .expect("an event refused for its generation was not recorded, so its id is free");

    let recorded: Vec<(EventId, i64)> = log
        .envelopes()
        .iter()
        .map(|envelope| (envelope.event_id, envelope.sequence))
        .collect();
    assert_eq!(
        recorded,
        vec![
            (EventId::from(id(1)), 5),
            (EventId::from(id(2)), 6),
            (EventId::from(id(3)), 7),
        ]
    );
}

/// A fixed-seed linear congruential generator, so the walk is the same on every machine.
struct Walk(u64);

impl Walk {
    fn next(&mut self, below: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % below
    }
}

#[test]
fn a_fixed_seed_walk_records_exactly_what_a_model_of_the_three_rules_records() {
    let mut walk = Walk(0x5eed_0006);
    let mut log = EventLog::new();
    let mut seen: BTreeSet<EventId> = BTreeSet::new();
    let mut last: BTreeMap<StreamId, i64> = BTreeMap::new();
    let mut recorded: Vec<(EventId, StreamId, i64)> = Vec::new();
    let mut refusals = [0_u32; 3];

    for step in 0..3_000 {
        let event_id = EventId::from(id(u128::from(walk.next(400)) + 1));
        let stream_id = stream(u128::from(walk.next(3)));
        let sequence = i64::try_from(walk.next(40)).expect("small") - 5;
        let envelope_generation = match walk.next(3) {
            0 => None,
            n => Some(generation(u128::from(n))),
        };
        let payload = match walk.next(4) {
            0 => delta(generation(1)),
            1 => delta(generation(2)),
            2 => StreamEvent::SegmentQueued(SegmentQueued {
                segment_id: SegmentId::from(id(0x7002)),
            }),
            _ => StreamEvent::TurnCompleted(TurnCompleted {
                turn_id: TurnId::from(id(0x9002)),
            }),
        };

        let expected = if seen.contains(&event_id) {
            Err(EnvelopeRefusal::DuplicateEventId { event_id })
        } else if let Some(&at) = last.get(&stream_id).filter(|&&at| sequence <= at) {
            Err(EnvelopeRefusal::SequenceNotIncreasing {
                stream_id,
                last: at,
                sequence,
            })
        } else if let Some(carried) = payload
            .generation_id()
            .filter(|&carried| envelope_generation != Some(carried))
        {
            Err(EnvelopeRefusal::GenerationMismatch {
                payload: carried,
                envelope: envelope_generation,
            })
        } else {
            Ok(())
        };
        match expected {
            Ok(()) => {
                seen.insert(event_id);
                last.insert(stream_id, sequence);
                recorded.push((event_id, stream_id, sequence));
            }
            Err(EnvelopeRefusal::DuplicateEventId { .. }) => refusals[0] += 1,
            Err(EnvelopeRefusal::SequenceNotIncreasing { .. }) => refusals[1] += 1,
            Err(EnvelopeRefusal::GenerationMismatch { .. }) => refusals[2] += 1,
            Err(EnvelopeRefusal::KindMismatch { .. }) => {
                unreachable!("the walk sends every envelope under its payload's own kind")
            }
        }

        let mut sent = envelope(0, stream_id, sequence, envelope_generation, payload);
        sent.event_id = event_id;
        assert_eq!(log.record(sent), expected, "step {step}");
    }

    assert!(
        refusals.iter().all(|&count| count > 0) && !recorded.is_empty(),
        "the walk reaches every rule and records something: {refusals:?}, {}",
        recorded.len()
    );
    let logged: Vec<(EventId, StreamId, i64)> = log
        .envelopes()
        .iter()
        .map(|envelope| (envelope.event_id, envelope.stream_id, envelope.sequence))
        .collect();
    assert_eq!(logged, recorded);
}

#[test]
fn an_envelope_whose_kind_is_not_its_payloads_declared_name_is_refused() {
    let mut log = EventLog::new();
    let control = StreamEvent::TurnCompleted(TurnCompleted {
        turn_id: TurnId::from(id(0x9001)),
    });
    let mut disguised = envelope(1, stream(1), 1, None, control);
    "inference.stream.TranscriptHypothesis".clone_into(&mut disguised.kind);

    let answer = log.record(disguised);
    assert!(
        answer.is_err(),
        "a TurnCompleted (control) recorded under the kind TranscriptHypothesis (content) is read \
         as content by every consumer that dispatches on kind; stream.yaml declares kind as the \
         declared name of the payload's event and docs/design/inference.md:297 says control must \
         not be mistaken for user content: {answer:?}"
    );
    assert!(log.is_empty(), "a refused event is not recorded");
}
