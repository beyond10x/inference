//! G.711 mu-law at 8 kHz in 20 ms frames reaches a model that takes it as it is, and a model that
//! does not is refused at open.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_duplex::inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::{
    OpenError, ResponseId, SessionConfig, SessionEvent, SessionLedger, SpeechCapabilities,
    SpeechSessionId, TuningSupport, TurnDetection, TurnTuning, Unsupported,
};

fn format(encoding: AudioEncoding, sample_rate: u32) -> AudioFormat {
    AudioFormat {
        encoding,
        sample_rate: NonZeroU32::new(sample_rate).expect("a positive rate"),
        channels: NonZeroU16::MIN,
    }
}

fn adapter() -> Adapter {
    Adapter::declare_as(
        AdapterId::generate(),
        DeclareAdapter {
            model_id: ModelId::generate(),
            backend: "scripted".to_owned(),
            contract_revision: "1".to_owned(),
            execution_form: ExecutionForm::HostedService,
            locality: Locality::ExternalProvider,
            capabilities: vec![Capability::ConcurrentInputOutput],
            max_concurrency: 1,
        },
    )
    .expect("a positive concurrency")
    .0
}

fn capabilities(formats: Vec<AudioFormat>) -> SpeechCapabilities {
    SpeechCapabilities {
        input_formats: formats.clone(),
        output_formats: formats,
        turn_detection: vec![TurnDetection::ServerVad],
        turn_tuning: TuningSupport::ALL,
        manual_commit: true,
        manual_response: true,
        manual_interrupt: true,
        client_cancel: true,
        truncation: true,
        tool_calling: false,
    }
}

fn config(format: AudioFormat) -> SessionConfig {
    SessionConfig {
        voice: "calm".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: format,
        output_format: format,
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(10),
    }
}

#[test]
fn mulaw_at_8_khz_in_20_ms_frames_passes_both_ways_byte_for_byte() {
    let mulaw = format(AudioEncoding::PcmMulaw, 8_000);
    let now = Instant::now();
    let (mut ledger, _) = SessionLedger::open(
        SpeechSessionId::generate(),
        &adapter(),
        &capabilities(vec![mulaw, format(AudioEncoding::PcmS16le, 24_000)]),
        config(mulaw),
        now,
    )
    .expect("the model takes mu-law");
    let twenty_ms = AudioFrame {
        format: mulaw,
        position: SampleClockPosition::new(0).expect("a position"),
        sample_count: 160,
        data: (0..=159_u8).collect(),
    };
    assert_eq!(mulaw.data_len(160), 160, "one byte per sample");
    ledger
        .accept_input(twenty_ms.clone())
        .expect("in the session's format");
    let response = ResponseId::generate();
    ledger.response_created(response, now).expect("created");
    ledger
        .output_audio(response, twenty_ms.clone(), now)
        .expect("in the session's format");
    let events: Vec<SessionEvent> = std::iter::from_fn(|| ledger.next_event()).collect();
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::InputAccepted(accepted) if accepted.frame == twenty_ms
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::OutputAudio(audio) if audio.frame == twenty_ms
    )));
}

#[test]
fn a_model_without_mulaw_refuses_it_at_open_rather_than_resampling() {
    let mulaw = format(AudioEncoding::PcmMulaw, 8_000);
    let refused = SessionLedger::open(
        SpeechSessionId::generate(),
        &adapter(),
        &capabilities(vec![format(AudioEncoding::PcmS16le, 16_000)]),
        config(mulaw),
        Instant::now(),
    )
    .err();
    assert_eq!(
        refused,
        Some(OpenError::Unsupported(Unsupported::InputFormat(mulaw)))
    );
}
