//! Adversary pass 1 of `story:segment-linear-transitions`, against the mock generator's maximum
//! concurrency of one: a generation holds its place until its handle is dropped, whatever state
//! it ended in, the place is shared by every clone of the generator, outlives the generator it
//! was started on, and clones starting at once on several threads admit exactly one generation.

#![cfg(feature = "llm")]

use std::sync::{Arc, Barrier};
use std::thread;

use inference_core::generation::GenerationState;
use inference_core::id::{NodeId, RunId};
use inference_core::model::ModelId;
use inference_llm::{ConcurrencyLimitReached, GenerationHandle, GenerationRequest, Generator};
use inference_testkit::llm::{MockConfiguration, MockGenerator};

const REFUSED: ConcurrencyLimitReached = ConcurrencyLimitReached { max_concurrency: 1 };

fn request() -> GenerationRequest {
    GenerationRequest {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
        prompt: "prompt".to_owned(),
    }
}

fn generator(configuration: MockConfiguration) -> MockGenerator {
    MockGenerator::new(configuration, ModelId::generate(), ["a", "b"])
}

/// A generation that ran to its end, was cancelled or was superseded still holds its place while
/// its handle is held, as the doc says ("a generation runs until its handle is dropped"), and
/// gives it back when dropped.
#[test]
fn a_generation_holds_its_place_until_its_handle_is_dropped_in_every_end_state() {
    for configuration in MockConfiguration::ALL {
        for end in [
            GenerationState::Completed,
            GenerationState::Cancelled,
            GenerationState::Superseded,
        ] {
            let mut generator = generator(configuration);
            let mut handle = generator.start(request()).expect("a fresh generator");
            match end {
                GenerationState::Completed => handle.by_ref().for_each(drop),
                GenerationState::Cancelled => {
                    handle.cancel().expect("Active cancels");
                }
                _ => {
                    handle.supersede().expect("Active supersedes");
                }
            }
            assert_eq!(
                handle.output().generation().state(),
                end,
                "{configuration:?}"
            );
            assert_eq!(
                generator.start(request()).map(|_| ()),
                Err(REFUSED),
                "{configuration:?}: a {end:?} generation whose handle is held keeps its place"
            );
            drop(handle);
            assert!(
                generator.start(request()).is_ok(),
                "{configuration:?}: dropping the {end:?} handle gives the place back"
            );
        }
    }
}

/// The place belongs to the generation, not to the generator value it was started on.
#[test]
fn the_place_outlives_the_generator_it_was_started_on() {
    let mut original = generator(MockConfiguration::ALL[0]);
    let mut clone = original.clone();
    let handle = original.start(request()).expect("a fresh generator");
    drop(original);
    assert_eq!(clone.start(request()).map(|_| ()), Err(REFUSED));
    drop(handle);
    let again = clone.start(request()).expect("the place was given back");
    assert_eq!(
        clone.start(request()).map(|_| ()),
        Err(REFUSED),
        "a refused start takes no place and leaves the running one counted"
    );
    drop(again);
    assert!(clone.start(request()).is_ok());
}

/// Clones starting at once on eight threads: exactly one generation runs.
#[test]
fn clones_starting_at_once_admit_exactly_one_generation() {
    for round in 0..50 {
        let generator = generator(MockConfiguration::ALL[round % MockConfiguration::ALL.len()]);
        let barrier = Arc::new(Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let mut generator = generator.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    let started = generator.start(request());
                    barrier.wait();
                    started.is_ok()
                })
            })
            .collect();
        let admitted = workers
            .into_iter()
            .map(|worker| worker.join().expect("the worker ran"))
            .filter(|admitted| *admitted)
            .count();
        assert_eq!(admitted, 1, "round {round}");
        let mut generator = generator;
        assert!(
            generator.start(request()).is_ok(),
            "round {round}: every place given back"
        );
    }
}
