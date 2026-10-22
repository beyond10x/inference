//! Adversary case, pass 4, story `duplex-late-usage-counted` at d2feb27: the close rule over the
//! events the ledger builds from provider traffic.
//!
//! The testkit's conformance target keeps its own responses and calls stores, so the close
//! scenarios receive their held events through `ReceiveSessionEvent` alone: no scenario reaches
//! the `push_received` calls of `output_audio`, `output_transcript`, `response_completed` and
//! `tool_call`. The ledger's own tests assert the received kind only of an input transcript and
//! of usage. This case holds one event of each of the other four kinds at a close and asserts
//! that each is a received event of its kind, delivered before `SpeechSessionClosed`.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_duplex::inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::inference_tools::{DeclareTool, SideEffectClass, Tool};
use inference_duplex::{
    ReceivedEventKind, ReceivedSessionEventState, ResponseId, SessionConfig, SessionEnd,
    SessionEvent, SessionLedger, SpeechCallId, SpeechCapabilities, SpeechSessionId, TuningSupport,
    TurnDetection, TurnTuning,
};
use serde_json::json;

fn pcm16k() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("a positive rate"),
        channels: NonZeroU16::MIN,
    }
}

fn ledger(now: Instant) -> SessionLedger {
    let adapter = Adapter::declare_as(
        AdapterId::generate(),
        DeclareAdapter {
            model_id: ModelId::generate(),
            backend: "scripted".to_owned(),
            contract_revision: "1".to_owned(),
            execution_form: ExecutionForm::HostedService,
            locality: Locality::ExternalProvider,
            capabilities: vec![Capability::ConcurrentInputOutput, Capability::ToolCalling],
            max_concurrency: 1,
        },
    )
    .expect("a positive concurrency")
    .0;
    let weather = Tool::declare(DeclareTool {
        name: "weather".to_owned(),
        schema: json!({"type": "object"}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0;
    let capabilities = SpeechCapabilities {
        input_formats: vec![pcm16k()],
        output_formats: vec![pcm16k()],
        turn_detection: vec![TurnDetection::ServerVad],
        turn_tuning: TuningSupport::ALL,
        manual_commit: false,
        manual_response: false,
        manual_interrupt: false,
        client_cancel: false,
        truncation: false,
        tool_calling: true,
    };
    let config = SessionConfig {
        voice: "calm".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm16k(),
        output_format: pcm16k(),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: vec![weather],
        idle_limit: Duration::from_secs(10),
    };
    SessionLedger::open(
        SpeechSessionId::generate(),
        &adapter,
        &capabilities,
        config,
        now,
    )
    .expect("the session opens")
    .0
}

/// Output audio, an output transcript, a tool call and a response end that the caller has not
/// taken when it closes are each a received event of their kind, delivered before
/// `SpeechSessionClosed`.
#[test]
fn every_kind_held_at_the_close_is_received_and_delivered_before_closed() {
    let now = Instant::now();
    let mut ledger = ledger(now);
    let response = ResponseId::generate();
    ledger.response_created(response, now).expect("created");
    while ledger.next_event().is_some() {}
    let format = pcm16k();
    let samples = 320;
    ledger
        .output_audio(
            response,
            AudioFrame {
                format,
                position: SampleClockPosition::new(0).expect("a position"),
                sample_count: samples,
                data: vec![0; usize::try_from(format.data_len(samples)).expect("a length")],
            },
            now,
        )
        .expect("in progress");
    ledger
        .output_transcript(response, "Hello".to_owned(), now)
        .expect("in progress");
    ledger
        .tool_call(
            SpeechCallId::new("call-1"),
            response,
            "weather",
            json!({}),
            now,
        )
        .expect("proposed");
    ledger.response_completed(response, now).expect("completed");
    ledger.close().expect("open");
    let events: Vec<SessionEvent> = std::iter::from_fn(|| ledger.next_event()).collect();
    assert!(
        matches!(events.last(), Some(SessionEvent::SpeechSessionClosed(_))),
        "{events:#?}"
    );
    let delivered: Vec<(
        ReceivedEventKind,
        ReceivedSessionEventState,
        Option<SessionEnd>,
    )> = ledger
        .received_events()
        .iter()
        .filter(|received| received.kind() != ReceivedEventKind::Usage)
        .map(|received| {
            (
                received.kind(),
                received.state(),
                received.delivered_before(),
            )
        })
        .collect();
    let closed = (
        ReceivedSessionEventState::Delivered,
        Some(SessionEnd::Closed),
    );
    assert_eq!(
        delivered,
        [
            (ReceivedEventKind::OutputAudio, closed.0, closed.1),
            (ReceivedEventKind::OutputTranscript, closed.0, closed.1),
            (ReceivedEventKind::ToolCall, closed.0, closed.1),
            (ReceivedEventKind::ResponseEnd, closed.0, closed.1),
        ],
        "each event the provider sent and the caller had not taken is received and delivered \
         before the close"
    );
}
