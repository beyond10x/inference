//! Adversary case on `story:cascade-turn-spans-in-a-burst`: the turn a session's close completes
//! is answered.
//!
//! The user speaks and the application closes the session before the endpoint silence ends the
//! turn, as a file fed faster than real time ends. The close drains the run: the detector ends the
//! speech at its end of input, the turn policy completes the turn, and the STT node sends that
//! turn's `TurnCompleted` and then its ends of input, one right behind the other. Every event
//! received before the close is delivered (`Agent::close`), so the model is asked one generation
//! over the turn's text. 16 agents run at once, 30 rounds each.

#![cfg(feature = "testing")]

use std::num::{NonZeroU16, NonZeroU32};

use inference::core::model::ModelId;
use inference::{Agent, Binding, Stack};
use inference_testkit::llm::{MockConfiguration, MockGenerator};
use inference_testkit::stt::ScriptStep;
use inference_testkit::stt::inference_stt::inference_audio::{AudioEncoding, AudioFormat};
use inference_testkit::stt::synthetic_audio::{Stretch, synthesize};
use inference_testkit::stt::{Cue, MockConfiguration as SttConfiguration, MockRecogniser};
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesiser};

const AGENTS: usize = 16;
const ROUNDS: usize = 30;
const TEXT: &str = "the last words";

/// 300 ms of tone and 50 ms of silence, shorter than the detector's hold.
const SPEECH: [Stretch; 2] = [
    Stretch::Tone {
        frames: 30,
        hertz: 440,
        amplitude: 0.5,
    },
    Stretch::Silence { frames: 5 },
];

async fn round() -> Result<(), String> {
    let generator = MockGenerator::new(
        MockConfiguration::StreamingOnly,
        ModelId::generate(),
        ["hello ", "there"],
    );
    let recogniser = MockRecogniser::new(
        SttConfiguration::Incremental,
        ModelId::generate(),
        vec![
            Cue {
                after_frames: 1,
                step: ScriptStep::Hypothesis(TEXT.to_owned()),
            },
            Cue {
                after_frames: 2,
                step: ScriptStep::Commit { span: 0 },
            },
        ],
    );
    let synthesiser = MockSynthesiser::new(
        TtsConfiguration::IncrementalStreaming,
        ModelId::generate(),
        AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(16_000).expect("16000"),
            channels: NonZeroU16::new(1).expect("1"),
        },
    );
    let stack = Stack::new()
        .bind(Binding::stt(recogniser))
        .bind(Binding::llm(generator.clone()))
        .bind(Binding::tts(synthesiser));
    let agent = Agent::from_recipe(inference::Recipe::voice_cascade())
        .stack(stack)
        .start()
        .await
        .map_err(|error| format!("the agent does not start: {error}"))?;
    for frame in synthesize(&SPEECH) {
        agent
            .send_audio(frame)
            .await
            .map_err(|error| format!("a frame is refused: {error}"))?;
    }
    agent
        .close()
        .await
        .map_err(|error| format!("the agent does not close: {error}"))?;
    let asked = generator.asked();
    if asked == [vec![TEXT.to_owned()]] {
        Ok(())
    } else {
        Err(format!(
            "the model was asked {asked:?}, not one generation over the turn the close completed"
        ))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_turn_a_close_completes_is_answered_in_every_round() {
    let agents: Vec<_> = (0..AGENTS)
        .map(|_| {
            tokio::spawn(async {
                let mut failures = Vec::new();
                for _ in 0..ROUNDS {
                    if let Err(failure) = round().await {
                        failures.push(failure);
                    }
                }
                failures
            })
        })
        .collect();
    let mut failures = Vec::new();
    for agent in agents {
        failures.extend(agent.await.expect("the agent's task ends"));
    }
    assert!(
        failures.is_empty(),
        "{} of {} rounds went wrong, the first: {:?}",
        failures.len(),
        AGENTS * ROUNDS,
        failures.first()
    );
}
