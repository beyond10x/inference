//! The async form of a Nova Sonic session is woken by the binding, not by itself: awaiting one
//! event while the provider sends nothing polls it a bounded handful of times, and an event the
//! provider sends later wakes it. Offline: a scripted provider stream over in-process channels,
//! no network, no credential.
#![cfg(feature = "duplex")]

use std::future::Future;
use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use inference_aws::duplex::{Connector, NovaSonicSpeech};
use inference_aws::nova_sonic::{DEFAULT_MODEL_ID, NovaSonicError, Session, SessionConfig};
use inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{
    NativeSpeechModel, SessionConfig as SpeechConfig, SpeechSessionHandle, TurnDetection,
    TurnTuning,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// How long the provider sends nothing before its one event.
const SILENT: Duration = Duration::from_millis(400);

/// How long the first await waits, well inside [`SILENT`].
const QUIET: Duration = Duration::from_millis(200);

/// The most polls a woken form takes over [`QUIET`] with nothing arriving; a form that wakes
/// itself on every poll takes thousands.
const MOST_POLLS: u32 = 10;

/// A provider that keeps its input open, sends nothing for [`SILENT`], then `events`, and stays
/// open.
#[derive(Debug, Clone)]
struct Late {
    events: Vec<Value>,
}

impl Connector for Late {
    fn model_id(&self) -> &str {
        DEFAULT_MODEL_ID
    }

    fn connect(
        &self,
        config: SessionConfig,
    ) -> impl Future<Output = Result<Session, NovaSonicError>> + Send + 'static {
        let events = self.events.clone();
        async move {
            let (input, mut received) = mpsc::channel::<Vec<u8>>(256);
            let (frames, output) = mpsc::channel(256);
            let session = Session::open(&config, input, output).await?;
            tokio::spawn(async move {
                let drained = tokio::spawn(async move { while received.recv().await.is_some() {} });
                tokio::time::sleep(SILENT).await;
                for event in events {
                    if frames
                        .send(Ok(event.to_string().into_bytes()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                frames.closed().await;
                drained.abort();
            });
            Ok(session)
        }
    }
}

fn event(name: &str, mut body: Value) -> Value {
    body["sessionId"] = json!("scripted-session");
    body["promptName"] = json!("scripted-prompt");
    body["completionId"] = json!("scripted-completion");
    json!({"event": {name: body}})
}

/// The user's final transcript "hi", which the session delivers as a committed input.
fn user_said_hi() -> Vec<Value> {
    vec![
        event("completionStart", json!({})),
        event(
            "contentStart",
            json!({
                "contentId": "user-1",
                "type": "TEXT",
                "role": "USER",
                "additionalModelFields": "{\"generationStage\":\"FINAL\"}",
                "textOutputConfiguration": {"mediaType": "text/plain"}
            }),
        ),
        event(
            "textOutput",
            json!({"contentId": "user-1", "content": "hi"}),
        ),
        event(
            "contentEnd",
            json!({"contentId": "user-1", "type": "TEXT", "stopReason": "END_TURN"}),
        ),
    ]
}

fn pcm() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("a positive rate"),
        channels: NonZeroU16::MIN,
    }
}

fn config() -> SpeechConfig {
    SpeechConfig {
        voice: "tiffany".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm(),
        output_format: pcm(),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(5),
    }
}

#[test]
fn the_async_form_waits_without_polling_itself_and_an_arrival_wakes_it() {
    let mut session = NovaSonicSpeech::with_connector(Late {
        events: user_said_hi(),
    })
    .expect("the binding's runtime starts")
    .open(config())
    .expect("opens");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a runtime");
    let mut polls = 0_u32;
    let quiet = runtime.block_on(async {
        tokio::time::timeout(
            QUIET,
            std::future::poll_fn(|cx| {
                polls += 1;
                session.poll_event(cx)
            }),
        )
        .await
    });
    assert!(
        quiet.is_err(),
        "an event arrived while the provider sent nothing: {quiet:?}"
    );
    assert!(
        polls <= MOST_POLLS,
        "one await polled the session {polls} times over {QUIET:?} with nothing arriving"
    );
    let woken = runtime
        .block_on(async { tokio::time::timeout(Duration::from_secs(2), session.event()).await });
    assert!(
        matches!(woken, Ok(Some(_))),
        "the provider's event did not wake the async form: {woken:?}"
    );
    session.close().expect("open");
}
