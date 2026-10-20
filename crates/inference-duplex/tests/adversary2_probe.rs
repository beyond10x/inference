//! Adversary pass 2: the honesty of `conformance::native_duplex_evidence`.
//!
//! The probe's dependency check reads `before` as "the output before the input sent during it".
//! It takes `before` from the single event that ended its first loop: an `OutputAudio` leaves it
//! empty, and every transcript the response had already recorded is counted as `after`. A model
//! that answers the opening and never reads another frame then gives the evidence whenever the
//! opening says the marker, as the Nova Sonic live probe's does (opening and `during` are the same
//! clip, "What is the capital of France?", marker "france").

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use inference_duplex::conformance::{NativeDuplexProbe, native_duplex_evidence};
use inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_duplex::inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::inference_tools::Json;
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, SessionConfig, SessionError, SessionEvent,
    SessionLedger, SpeechCallId, SpeechCapabilities, SpeechSessionHandle, SpeechSessionId,
    TuningSupport, TurnDetection, TurnTuning,
};

fn pcm(sample_rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(sample_rate).expect("a positive rate"),
        channels: NonZeroU16::MIN,
    }
}

fn frame(from: u64, byte: u8) -> AudioFrame {
    let format = pcm(16_000);
    AudioFrame {
        format,
        position: SampleClockPosition::new(from).expect("a position"),
        sample_count: 320,
        data: vec![byte; usize::try_from(format.data_len(320)).expect("a length")],
    }
}

fn capabilities() -> SpeechCapabilities {
    SpeechCapabilities {
        input_formats: vec![pcm(16_000)],
        output_formats: vec![pcm(16_000)],
        turn_detection: TurnDetection::ALL.to_vec(),
        turn_tuning: TuningSupport::ALL,
        manual_commit: true,
        manual_response: true,
        manual_interrupt: true,
        client_cancel: true,
        truncation: true,
        tool_calling: true,
    }
}

/// A model that answers the first user turn and never reads another frame: half duplex.
struct AnswersTheOpeningOnly {
    adapter: Adapter,
    capabilities: SpeechCapabilities,
}

struct Session {
    ledger: SessionLedger,
    spoke: bool,
    answered: bool,
}

impl NativeSpeechModel for AnswersTheOpeningOnly {
    type Session = Session;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        &self.capabilities
    }

    fn open(&mut self, config: SessionConfig) -> Result<Session, OpenError> {
        let (ledger, _) = SessionLedger::open(
            SpeechSessionId::generate(),
            &self.adapter,
            &self.capabilities,
            config,
            Instant::now(),
        )?;
        Ok(Session {
            ledger,
            spoke: false,
            answered: false,
        })
    }
}

impl SpeechSessionHandle for Session {
    fn ledger(&self) -> &SessionLedger {
        &self.ledger
    }

    /// Speech then silence ends the first turn, which is answered as a provider answers it:
    /// audio first, its transcript after. Every later frame is accepted and never read.
    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        let speech = frame.data.iter().any(|byte| *byte != 0);
        self.ledger.accept_input(frame)?;
        if self.answered {
            return Ok(());
        }
        if speech {
            self.spoke = true;
            return Ok(());
        }
        if self.spoke {
            self.answered = true;
            let now = Instant::now();
            self.ledger.input_committed(now)?;
            self.ledger
                .input_transcribed("What is the capital of France?".to_owned(), now)?;
            let response = ResponseId::generate();
            self.ledger.response_created(response, now)?;
            self.ledger.output_audio(response, frame_out(), now)?;
            self.ledger.output_transcript(
                response,
                "The capital of France is Paris.".to_owned(),
                now,
            )?;
            self.ledger.response_completed(response, now)?;
        }
        Ok(())
    }

    fn commit_input(&mut self) -> Result<(), SessionError> {
        self.ledger.commit_input(Instant::now())
    }

    fn create_response(&mut self) -> Result<(), SessionError> {
        self.ledger.request_response(Instant::now())
    }

    fn cancel_response(&mut self, response_id: ResponseId) -> Result<(), SessionError> {
        self.ledger.cancel(response_id)
    }

    fn truncate_output(
        &mut self,
        response_id: ResponseId,
        heard: SampleClockPosition,
    ) -> Result<(), SessionError> {
        self.ledger.truncate(response_id, heard)
    }

    fn submit_tool_result(
        &mut self,
        call_id: &SpeechCallId,
        result: Json,
    ) -> Result<(), SessionError> {
        self.ledger.answer(call_id, result)
    }

    fn close(&mut self) -> Result<(), SessionError> {
        self.ledger.close()
    }

    fn next_event(&mut self, _wait: Duration) -> Option<SessionEvent> {
        self.ledger.next_event()
    }
}

fn frame_out() -> AudioFrame {
    frame(0, 1)
}

#[test]
fn a_model_that_never_reads_the_input_during_output_gives_no_native_duplex_evidence() {
    let mut model = AnswersTheOpeningOnly {
        adapter: Adapter::declare_as(
            AdapterId::generate(),
            DeclareAdapter {
                model_id: ModelId::generate(),
                backend: "half-duplex".to_owned(),
                contract_revision: "1".to_owned(),
                execution_form: ExecutionForm::HostedService,
                locality: Locality::ExternalProvider,
                capabilities: vec![Capability::ConcurrentInputOutput],
                max_concurrency: 1,
            },
        )
        .expect("a positive concurrency")
        .0,
        capabilities: capabilities(),
    };
    let probe = NativeDuplexProbe {
        config: SessionConfig {
            voice: "calm".to_owned(),
            instructions: "answer briefly".to_owned(),
            input_format: pcm(16_000),
            output_format: pcm(16_000),
            turn_detection: TurnDetection::ServerVad,
            turn_tuning: TurnTuning::default(),
            auto_response: true,
            auto_interrupt: false,
            tools: Vec::new(),
            idle_limit: Duration::from_secs(5),
        },
        opening: vec![frame(0, 9), frame(320, 0)],
        during: vec![frame(640, 9), frame(960, 0)],
        marker: "france".to_owned(),
        wait: Duration::from_millis(1),
        patience: 3,
    };

    let outcome = native_duplex_evidence(&mut model, &probe);
    assert!(
        outcome.is_err(),
        "the model recorded its whole answer before the probe sent a frame of `during`, and \
         never read `during`; the probe called that native duplex: {outcome:?}"
    );
}
