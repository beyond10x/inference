//! Adversary pass 1 on the show: usage the ledger records with unknown counts, a turn that
//! stalls before its response ends, a model that creates its own responses and stays silent, a
//! show whose responses carry no audio, and the speaker's samples turn by turn.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use inference_audio_device::fake::FakeHost;
use inference_duplex::inference_audio::{AudioEncoding, AudioFrame, SampleClockPosition};
use inference_duplex::inference_core::model::Adapter;
use inference_duplex::inference_tools::Json;
use inference_duplex::{
    NativeSpeechModel, OpenError, QuantityKind, QuantitySource, ResponseId, SessionConfig,
    SessionError, SessionEvent, SessionLedger, SpeechCallId, SpeechCapabilities,
    SpeechSessionHandle,
};
use inference_testkit::duplex::mock::{
    Faults, MockSpeechModel, MockSpeechSession, Reply, ScriptedTurn,
};
use realtime_voice::Reported;
use repo_podcast::{Board, Directive, Event, Settings, Show, default_hosts};

/// What a session of [`Tee`] does besides speaking its line.
#[derive(Clone, Copy, Default)]
struct Twist {
    /// The provider stalls after this many of its events.
    stall_after: Option<usize>,
    /// Output audio is dropped.
    mute: bool,
}

/// The mock with one line per session, a [`Twist`] for chosen sessions, every event each
/// session delivers kept with the session's index, and each session's audio filled with a
/// level of its own, so the speaker's samples say which turn they belong to.
struct Tee {
    template: MockSpeechModel,
    lines: VecDeque<String>,
    twists: Vec<(usize, Twist)>,
    every: Twist,
    opened: usize,
    seen: Rc<RefCell<Vec<(usize, SessionEvent)>>>,
}

impl Tee {
    fn new(turns: usize) -> Self {
        let hosts = default_hosts();
        Self {
            template: MockSpeechModel::native(Vec::new()),
            lines: (0..turns)
                .map(|turn| format!("{} line {turn}", hosts[turn % 2].name))
                .collect(),
            twists: Vec::new(),
            every: Twist::default(),
            opened: 0,
            seen: Rc::default(),
        }
    }

    fn twist(mut self, session: usize, twist: Twist) -> Self {
        self.twists.push((session, twist));
        self
    }
}

struct TeeSession {
    inner: MockSpeechSession,
    index: usize,
    mute: bool,
    seen: Rc<RefCell<Vec<(usize, SessionEvent)>>>,
}

/// The sample value of session `index`'s audio.
fn tag(index: usize) -> i16 {
    i16::try_from(1_000 * (index + 1)).unwrap_or(i16::MAX)
}

impl NativeSpeechModel for Tee {
    type Session = TeeSession;

    fn adapter(&self) -> &Adapter {
        self.template.adapter()
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        self.template.capabilities()
    }

    fn open(&mut self, config: SessionConfig) -> Result<TeeSession, OpenError> {
        let index = self.opened;
        self.opened += 1;
        let twist = self
            .twists
            .iter()
            .find(|(at, _)| *at == index)
            .map_or(self.every, |(_, twist)| *twist);
        let line = self
            .lines
            .pop_front()
            .unwrap_or_else(|| "nothing more".to_owned());
        let mut mock = MockSpeechModel::native(vec![ScriptedTurn {
            heard: "your turn".to_owned(),
            reply: Reply::Say(line),
        }]);
        if let Some(after) = twist.stall_after {
            mock = mock.with_faults(Faults {
                stall_after: Some(after),
                close_after: None,
            });
        }
        mock.open(config).map(|inner| TeeSession {
            inner,
            index,
            mute: twist.mute,
            seen: Rc::clone(&self.seen),
        })
    }
}

impl SpeechSessionHandle for TeeSession {
    fn ledger(&self) -> &SessionLedger {
        self.inner.ledger()
    }

    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        self.inner.append_audio(frame)
    }

    fn commit_input(&mut self) -> Result<(), SessionError> {
        self.inner.commit_input()
    }

    fn create_response(&mut self) -> Result<(), SessionError> {
        self.inner.commit_input()?;
        self.inner.create_response()
    }

    fn cancel_response(&mut self, response_id: ResponseId) -> Result<(), SessionError> {
        self.inner.cancel_response(response_id)
    }

    fn truncate_output(
        &mut self,
        response_id: ResponseId,
        heard: SampleClockPosition,
    ) -> Result<(), SessionError> {
        self.inner.truncate_output(response_id, heard)
    }

    fn submit_tool_result(
        &mut self,
        call_id: &SpeechCallId,
        result: Json,
    ) -> Result<(), SessionError> {
        self.inner.submit_tool_result(call_id, result)
    }

    fn close(&mut self) -> Result<(), SessionError> {
        self.inner.close()
    }

    fn next_event(&mut self, wait: Duration) -> Option<SessionEvent> {
        loop {
            let mut event = self.inner.next_event(wait)?;
            self.seen.borrow_mut().push((self.index, event.clone()));
            if let SessionEvent::OutputAudio(audio) = &mut event {
                if self.mute {
                    continue;
                }
                if audio.frame.format.encoding == AudioEncoding::PcmS16le {
                    let bytes = tag(self.index).to_le_bytes();
                    for pair in audio.frame.data.chunks_mut(2) {
                        pair.copy_from_slice(&bytes);
                    }
                }
            }
            return Some(event);
        }
    }
}

/// Settings whose empty reads advance the mock's clock by a second, so its ten-second liveness
/// bound is reached in ten steps.
fn settings(limit: Duration) -> Settings {
    Settings {
        limit,
        wait: Duration::from_secs(1),
        ..Settings::default()
    }
}

fn open<M: NativeSpeechModel>(model: M, settings: Settings) -> (Show<M>, FakeHost) {
    let host = FakeHost::laptop();
    let show = Show::open(
        model,
        default_hosts(),
        "a brief".to_owned(),
        &host,
        settings,
        |_| None,
    )
    .expect("the fake speaker opens");
    (show, host)
}

/// Steps the show up to `steps` times, the fake device playing 10 ms after each; the events and
/// the samples the device played.
fn run<M: NativeSpeechModel>(
    show: &mut Show<M>,
    host: &FakeHost,
    steps: usize,
) -> (Vec<Event>, Vec<f32>) {
    let mut events = Vec::new();
    let mut heard = Vec::new();
    for _ in 0..steps {
        events.extend(show.step());
        if show.is_over() {
            break;
        }
        heard.extend(host.pull(480).unwrap_or_default());
    }
    (events, heard)
}

fn directives(events: &[Event]) -> Vec<Directive> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Generating { directive, .. } => Some(*directive),
            _ => None,
        })
        .collect()
}

fn board_of(events: &[Event]) -> Board {
    let table = repo_podcast::PriceTable::parse(repo_podcast::REALTIME_PRICES).expect("shipped");
    let mut board = Board::new(
        default_hosts(),
        Duration::from_millis(200),
        Some(table),
        Reported::OPENAI,
    );
    for event in events {
        board.apply(event);
    }
    board
}

/// The responses of session `index` the ledger recorded usage of with a token count unknown.
fn unknown_records(seen: &[(usize, SessionEvent)], index: usize) -> usize {
    seen.iter()
        .filter(|(at, _)| *at == index)
        .filter(|(_, event)| match event {
            SessionEvent::UsageRecorded(record) => {
                record.response_id.is_some()
                    && record.quantities.iter().any(|quantity| {
                        quantity.kind() == QuantityKind::InputTextTokens
                            && quantity.source() == QuantitySource::Unknown
                    })
            }
            _ => false,
        })
        .count()
}

/// Session 3 (Theo's second turn) stalls after its transcript and audio, before its usage and
/// its end: the mock's provider events are transcribed, transcription usage, created,
/// transcript, audio, audio.
fn stalling_third_reply() -> Tee {
    Tee::new(12).twist(
        3,
        Twist {
            stall_after: Some(6),
            mute: false,
        },
    )
}

#[test]
fn a_response_the_ledger_records_with_unknown_counts_is_not_shown_as_a_known_total() {
    let model = stalling_third_reply();
    let seen = Rc::clone(&model.seen);
    let (mut show, host) = open(model, settings(Duration::from_millis(200)));
    let (events, _) = run(&mut show, &host, 2_000);
    assert!(
        show.is_over(),
        "the stalled turn ends the show: {events:#?}"
    );
    assert_eq!(
        unknown_records(&seen.borrow(), 3),
        1,
        "the ledger records the stalled response with its token counts unknown"
    );
    let board = board_of(&events);
    let theo = board.counter_line(Some(1));
    let total = board.counter_line(None);
    assert!(
        theo.contains('?') || theo.contains("without usage"),
        "Theo's counters hide a response whose counts the ledger records as unknown: {theo}"
    );
    assert!(
        total.contains('?') || total.contains("without usage"),
        "the total hides it as well: {total}"
    );
}

#[test]
fn stopping_mid_turn_does_not_show_the_cancelled_response_as_a_known_total() {
    let model = Tee::new(12);
    let seen = Rc::clone(&model.seen);
    let (mut show, host) = open(
        model,
        Settings {
            max_events: 2,
            ..settings(Duration::from_secs(60))
        },
    );
    let mut events = Vec::new();
    for _ in 0..2_000 {
        let step = show.step();
        let theo_again = step.iter().any(|event| {
            matches!(
                event,
                Event::Generating {
                    host: 1,
                    turn: 3,
                    ..
                }
            )
        });
        events.extend(step);
        if theo_again {
            break;
        }
        let _ = host.pull(480);
    }
    events.extend(show.stop());
    assert_eq!(
        unknown_records(&seen.borrow(), 3),
        1,
        "closing the session cancels its response, recorded with its counts unknown"
    );
    let board = board_of(&events);
    let theo = board.counter_line(Some(1));
    assert!(
        theo.contains('?') || theo.contains("without usage"),
        "Enter mid-turn: Theo's counters show a known total that leaves out a response the \
         provider bills: {theo}"
    );
}

#[test]
fn the_counters_equal_the_sum_of_the_ledgers_usage_records_on_a_clean_show() {
    let model = Tee::new(12);
    let seen = Rc::clone(&model.seen);
    let (mut show, host) = open(model, settings(Duration::from_millis(200)));
    let (events, _) = run(&mut show, &host, 20_000);
    assert!(show.is_over());
    let board = board_of(&events);
    let seen = seen.borrow();
    for speaker in 0..2 {
        let mut sums = [0_i64; 4];
        let kinds = [
            QuantityKind::InputTextTokens,
            QuantityKind::InputAudioTokens,
            QuantityKind::OutputTextTokens,
            QuantityKind::OutputAudioTokens,
        ];
        for (index, event) in seen.iter() {
            let SessionEvent::UsageRecorded(record) = event else {
                continue;
            };
            if index % 2 != speaker || record.response_id.is_none() {
                continue;
            }
            for quantity in &record.quantities {
                if let Some(at) = kinds.iter().position(|kind| *kind == quantity.kind()) {
                    sums[at] += quantity.value().expect("every count known on the mock");
                }
            }
        }
        let counts = board.counts(speaker);
        let shown = [
            counts.input_text,
            counts.input_audio,
            counts.output_text,
            counts.output_audio,
        ]
        .map(|count| count.and_then(|count| i64::try_from(count).ok()));
        assert_eq!(shown, sums.map(Some), "host {speaker}");
    }
}

#[test]
fn a_turn_that_stalls_before_its_end_still_leaves_the_show_a_closing_turn() {
    let (mut show, host) = open(stalling_third_reply(), settings(Duration::from_millis(200)));
    let (events, _) = run(&mut show, &host, 2_000);
    assert!(show.is_over());
    let said = events
        .iter()
        .any(|event| matches!(event, Event::Generated { turn: 3, transcript, .. } if !transcript.is_empty()));
    assert!(
        said,
        "the stalled turn had its transcript and audio: {events:#?}"
    );
    assert_eq!(
        directives(&events).last(),
        Some(&Directive::Close),
        "a stall after the turn's audio (the first live run's closing turn) ends a show at \
         160 ms of 200 ms with no goodbye: {:?}",
        directives(&events)
    );
}

#[test]
fn a_model_that_creates_its_own_responses_and_stays_silent_is_given_up_within_ten_seconds() {
    let (mut show, host) = open(
        MockSpeechModel::server_turns(Vec::new()),
        Settings {
            limit: Duration::from_secs(60),
            wait: Duration::from_millis(10),
            ..Settings::default()
        },
    );
    let started = Instant::now();
    let mut problem = None;
    while started.elapsed() < Duration::from_millis(11_000) {
        for event in show.step() {
            if let Event::Problem { detail, .. } = event {
                problem.get_or_insert((started.elapsed(), detail));
            }
        }
        if problem.is_some() {
            break;
        }
        let _ = host.pull(480);
        std::thread::sleep(Duration::from_millis(20));
    }
    let (after, detail) = problem.expect(
        "the turn's liveness bound is ten seconds (turn_config, the commit message); after 11 s \
         of dead air the silent turn is still not given up",
    );
    assert!(
        after <= Duration::from_millis(10_500),
        "{after:?}: {detail}"
    );
}

#[test]
fn a_show_whose_responses_carry_no_audio_still_ends() {
    let mut model = Tee::new(400);
    model.every = Twist {
        stall_after: None,
        mute: true,
    };
    let (mut show, host) = open(
        model,
        Settings {
            limit: Duration::from_millis(200),
            wait: Duration::ZERO,
            ..Settings::default()
        },
    );
    let (events, _) = run(&mut show, &host, 3_000);
    let turns = directives(&events).len();
    assert!(
        show.is_over(),
        "the limit is audio played and nothing else bounds the show: {turns} sessions opened \
         and billed, none ended it"
    );
}

#[test]
fn the_speaker_plays_each_turn_whole_and_never_two_at_once_and_nothing_after_stop() {
    let (mut show, host) = open(
        Tee::new(12),
        Settings {
            limit: Duration::from_millis(200),
            wait: Duration::ZERO,
            ..Settings::default()
        },
    );
    let (_, heard) = run(&mut show, &host, 20_000);
    let mut order = Vec::new();
    let mut counts = std::collections::BTreeMap::new();
    for sample in heard.iter().step_by(2) {
        if *sample == 0.0 {
            continue;
        }
        #[allow(clippy::cast_possible_truncation)]
        let value = (sample * 32_768.0).round() as i32;
        let turn = usize::try_from((value + 500) / 1_000 - 1).expect("a tagged sample");
        assert_eq!(
            i32::from(tag(turn)),
            value,
            "a sample between two turns' levels: two turns mixed"
        );
        *counts.entry(turn).or_insert(0_usize) += 1;
        if order.last() != Some(&turn) {
            assert!(
                !order.contains(&turn),
                "turn {turn} heard again after another: {order:?}"
            );
            order.push(turn);
        }
    }
    assert_eq!(order, (0..7).collect::<Vec<_>>());
    for (turn, count) in counts {
        assert_eq!(count, 1_920, "turn {turn}: 40 ms at 48 kHz");
    }

    let (mut show, host) = open(
        Tee::new(12),
        Settings {
            limit: Duration::from_secs(60),
            wait: Duration::ZERO,
            ..Settings::default()
        },
    );
    for _ in 0..3 {
        let _ = show.step();
    }
    let _ = show.stop();
    let after = host.pull(4_800).unwrap_or_default();
    assert!(
        after.iter().all(|sample| *sample == 0.0),
        "audio after stop"
    );
}
