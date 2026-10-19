//! Adversary pass 1 against the `inference.stream` store of the testkit: a refused supersede or
//! cancel on a generation that holds segments, and a fixed-seed walk of every command over several
//! generations checked against the lifecycle of `ess/domains/stream.yaml` after each step.

use std::collections::BTreeMap;

use inference_core::generation::{GenerationState, StartGeneration};
use inference_core::id::{GenerationId, NodeId, RunId};
use inference_testkit::stream::StreamDomain;
use inference_text::{EmitTextDelta, SegmentId, SegmentState};
use uuid::Uuid;

fn start(domain: &mut StreamDomain, epoch: i64) -> GenerationId {
    domain
        .start_generation(StartGeneration {
            run_id: RunId::from(Uuid::from_u128(0x0a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d)),
            node_id: NodeId::from(Uuid::from_u128(0x1a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d)),
            epoch,
        })
        .generation_id
}

fn emit(domain: &mut StreamDomain, generation_id: GenerationId, sequence: i64) -> SegmentId {
    domain
        .emit_text_delta(EmitTextDelta {
            generation_id,
            sequence,
            text: format!("t{sequence}"),
        })
        .expect("an Active generation admits a delta")
        .segment_id
}

fn states(domain: &StreamDomain) -> BTreeMap<SegmentId, SegmentState> {
    domain
        .segments()
        .iter()
        .map(|segment| (segment.segment_id(), segment.state()))
        .collect()
}

/// One segment in each of the four unplayed states and one Played, all of `generation_id`.
fn unplayed_and_played(domain: &mut StreamDomain, generation_id: GenerationId) {
    let committed = emit(domain, generation_id, 2);
    let queued = emit(domain, generation_id, 3);
    let synthesized = emit(domain, generation_id, 4);
    let played = emit(domain, generation_id, 5);
    emit(domain, generation_id, 1);
    for segment in [committed, queued, synthesized, played] {
        domain.commit_segment(segment).expect("commits");
    }
    for segment in [queued, synthesized, played] {
        domain.queue_for_synthesis(segment).expect("queues");
    }
    for segment in [synthesized, played] {
        domain.complete_synthesis(segment).expect("synthesizes");
    }
    domain.acknowledge_playback(played).expect("plays");
}

#[test]
fn a_refused_supersede_or_cancel_discards_none_of_the_segments_the_generation_holds() {
    for terminal in [
        GenerationState::Completed,
        GenerationState::Superseded,
        GenerationState::Cancelled,
    ] {
        let mut domain = StreamDomain::new();
        let generation = start(&mut domain, 1);
        unplayed_and_played(&mut domain, generation);
        let before_terminal = states(&domain);
        match terminal {
            GenerationState::Completed => domain.complete_generation(generation).map(|_| ()),
            GenerationState::Superseded => domain.supersede_generation(generation).map(|_| ()),
            _ => domain.cancel_generation(generation).map(|_| ()),
        }
        .expect("Active moves");
        let held = states(&domain);
        if terminal == GenerationState::Completed {
            assert_eq!(held, before_terminal, "complete moves no segment");
        }
        let events = domain.events().len();

        assert!(domain.supersede_generation(generation).is_err());
        assert!(domain.cancel_generation(generation).is_err());
        assert!(domain.complete_generation(generation).is_err());

        assert_eq!(
            states(&domain),
            held,
            "a {terminal:?} generation refused the move, and its segments moved anyway"
        );
        assert_eq!(domain.events().len(), events);
    }
}

/// A fixed-seed linear congruential generator: the walk is the same on every run.
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

/// The pairs of before and after state the declared transitions allow, a segment's own command
/// or the cascade of its generation.
fn allowed(before: SegmentState, after: SegmentState) -> bool {
    use SegmentState::{Committed, Discarded, Played, Queued, Synthesized, Tentative};
    before == after
        || matches!(
            (before, after),
            (Tentative, Committed)
                | (Committed, Queued)
                | (Queued, Synthesized)
                | (Synthesized, Played)
                | (Tentative | Committed | Queued | Synthesized, Discarded)
        )
}

#[test]
fn a_fixed_seed_walk_of_every_command_keeps_the_declared_lifecycle() {
    for seed in [1_u64, 7, 42, 2026, 0xdead_beef] {
        let mut walk = Walk(seed);
        let mut domain = StreamDomain::new();
        let mut generations: Vec<GenerationId> = Vec::new();
        let mut segments: Vec<SegmentId> = Vec::new();
        for step in 0_i64..600 {
            let before = states(&domain);
            let events = domain.events().len();
            let segment_count = domain.segments().len();
            let choice = walk.next(10);
            let pick_generation = |walk: &mut Walk, generations: &[GenerationId]| {
                generations
                    .get(walk.next(generations.len().max(1)))
                    .copied()
                    .unwrap_or_else(|| GenerationId::from(Uuid::from_u128(0xfe)))
            };
            let pick_segment = |walk: &mut Walk, segments: &[SegmentId]| {
                segments
                    .get(walk.next(segments.len().max(1)))
                    .copied()
                    .unwrap_or_else(|| SegmentId::from(Uuid::from_u128(0xff)))
            };
            let mut cascaded: Option<GenerationId> = None;
            let mut emitted_into: Option<(GenerationId, GenerationState)> = None;
            let accepted = match choice {
                0 => {
                    generations.push(start(&mut domain, step));
                    true
                }
                1 => {
                    let generation = pick_generation(&mut walk, &generations);
                    domain.complete_generation(generation).is_ok()
                }
                2 => {
                    let generation = pick_generation(&mut walk, &generations);
                    let ok = domain.supersede_generation(generation).is_ok();
                    cascaded = ok.then_some(generation);
                    ok
                }
                3 => {
                    let generation = pick_generation(&mut walk, &generations);
                    let ok = domain.cancel_generation(generation).is_ok();
                    cascaded = ok.then_some(generation);
                    ok
                }
                4 | 5 => {
                    let generation = pick_generation(&mut walk, &generations);
                    if let Some(held) = domain.generations().get(generation) {
                        emitted_into = Some((generation, held.state()));
                    }
                    match domain.emit_text_delta(EmitTextDelta {
                        generation_id: generation,
                        sequence: step,
                        text: format!("s{step}"),
                    }) {
                        Ok(event) => {
                            segments.push(event.segment_id);
                            true
                        }
                        Err(_) => false,
                    }
                }
                6 => {
                    let segment = pick_segment(&mut walk, &segments);
                    domain.commit_segment(segment).is_ok()
                }
                7 => {
                    let segment = pick_segment(&mut walk, &segments);
                    domain.queue_for_synthesis(segment).is_ok()
                }
                8 => {
                    let segment = pick_segment(&mut walk, &segments);
                    match walk.next(2) {
                        0 => domain.complete_synthesis(segment).is_ok(),
                        _ => domain.acknowledge_playback(segment).is_ok(),
                    }
                }
                _ => {
                    let segment = pick_segment(&mut walk, &segments);
                    domain.discard_segment(segment).is_ok()
                }
            };
            let context = format!("seed {seed}, step {step}, command {choice}");

            assert_eq!(
                domain.events().len(),
                events + usize::from(accepted),
                "{context}: one event per accepted command, none per refusal"
            );
            if let Some((_, held)) = emitted_into {
                assert_eq!(
                    domain.segments().len() > segment_count,
                    held == GenerationState::Active,
                    "{context}: a delta is a segment exactly when its generation is Active"
                );
            }
            let after = states(&domain);
            for (segment, state) in &before {
                let now = after[segment];
                assert!(
                    allowed(*state, now),
                    "{context}: {segment} moved {state:?} -> {now:?}"
                );
                let owner = domain
                    .segments()
                    .get(*segment)
                    .expect("stored")
                    .generation_id();
                if *state == SegmentState::Played {
                    assert_eq!(
                        now,
                        SegmentState::Played,
                        "{context}: played is never unsaid"
                    );
                }
                if cascaded == Some(owner) {
                    assert!(
                        matches!(now, SegmentState::Played | SegmentState::Discarded),
                        "{context}: {segment} of the moved generation is {now:?}"
                    );
                }
                let moved = usize::from(now != *state);
                let own_command = !accepted || cascaded.is_some() || choice >= 6;
                assert!(
                    moved == 0 || own_command,
                    "{context}: {segment} moved under a command that moves no segment"
                );
            }
            if !accepted {
                assert_eq!(after, before, "{context}: a refusal moves nothing");
            }
        }
        assert!(
            generations.len() > 5 && segments.len() > 20,
            "seed {seed}: the walk reached {} generations and {} segments",
            generations.len(),
            segments.len()
        );
    }
}
