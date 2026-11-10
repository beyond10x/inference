//! Two turns of the cascade fixture fed at once over `voice_cascade` with the mocks: each turn is
//! answered by a generation of its own, over its own turn's text, whatever the timing of the
//! nodes. 16 agents run at once, 60 rounds each, 960 rounds in all.
//!
//! The recogniser is scripted per recognition ([`MockRecogniser::recognising`]) so that the two
//! turns carry distinct texts. The agent has no speaker, so the cascade keeps its replies
//! instead of cutting them at a turn's opening, and the replies cannot be told apart by the
//! interruptions they suffer; the case counts what the model was asked instead, one request per
//! generation: the model's requests are what each turn's reply was made of.

#![cfg(feature = "testing")]

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use inference::core::model::ModelId;
use inference::{Agent, Binding, Stack};
use inference_testkit::llm::{MockConfiguration, MockGenerator};
use inference_testkit::stt::ScriptStep;
use inference_testkit::stt::inference_stt::inference_audio::{AudioEncoding, AudioFormat};
use inference_testkit::stt::synthetic_audio::{FIXTURE, synthesize};
use inference_testkit::stt::{Cue, MockConfiguration as SttConfiguration, MockRecogniser};
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesiser};

const AGENTS: usize = 16;
const ROUNDS: usize = 60;
const LIMIT: Duration = Duration::from_secs(10);
const FIRST: &str = "first turn text";
const SECOND: &str = "second turn text";

fn script(text: &str) -> Vec<Cue> {
    vec![
        Cue {
            after_frames: 1,
            step: ScriptStep::Hypothesis(text.to_owned()),
        },
        Cue {
            after_frames: 2,
            step: ScriptStep::Commit { span: 0 },
        },
    ]
}

/// One round: a fresh agent hears the fixture's two utterances at once and answers; whether its
/// model was asked one generation over each turn's text.
async fn round() -> Result<(), String> {
    let generator = MockGenerator::new(
        MockConfiguration::StreamingOnly,
        ModelId::generate(),
        ["hello ", "there"],
    );
    let recogniser = MockRecogniser::new(
        SttConfiguration::Incremental,
        ModelId::generate(),
        script(FIRST),
    )
    .recognising(vec![script(FIRST), script(SECOND)]);
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
    for frame in synthesize(&FIXTURE) {
        agent
            .send_audio(frame)
            .await
            .map_err(|error| format!("a frame is refused: {error}"))?;
    }
    let started = Instant::now();
    let expected = vec![vec![FIRST.to_owned()], vec![SECOND.to_owned()]];
    while generator.asked() != expected && started.elapsed() < LIMIT {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let asked = generator.asked();
    let _ = agent.close().await;
    if asked == expected {
        Ok(())
    } else {
        Err(format!(
            "the model was asked {asked:?}, not one generation over each turn's text"
        ))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn two_turns_fed_at_once_are_each_answered_over_their_own_text_in_every_round() {
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
