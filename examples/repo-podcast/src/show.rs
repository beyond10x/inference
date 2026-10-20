//! The show: turns generated one session at a time, played one at a time.
//!
//! [`Show::step`] does three things each time: it starts the next turn when there is room for
//! it, handles what the turn's session reports, and feeds the speaker. The next turn is
//! generated while the current one plays, at most one ahead, so the hosts follow each other
//! without a gap; the speaker plays one turn's audio at a time and starts the next only when
//! nothing is queued.
//!
//! A turn of a model that takes responses from the caller (the OpenAI Realtime API) is one
//! response the show asks for. A model that creates every response itself (GPT-Live) is fed
//! silence, paced in real time, as such a model needs input to give output, and its first
//! response is the turn.
//!
//! The time limit is in audio: once the turns generated and the closing turns, at their
//! average length, would run past `limit`, the closing begins, so the show ends near the limit:
//! the guest, if it is the guest's turn, gives its final words, and the host closes the show. The
//! closing begins as well when the turns reach the most a show of `limit` may take (one per
//! [`SHORTEST_TURN`] of the limit, and never fewer than [`FEWEST_TURNS`]). The show is over when
//! the closing turn has played, or at once on [`Show::stop`].
//!
//! A turn whose session fails, stalls or is cancelled after its audio arrived keeps that audio
//! and counts as done, and the show goes on. One that fails before any audio ends the show once
//! what was generated has played, and so do [`SILENT_TURNS`] turns in a row without audio.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use inference_audio_device::convert::{decode, encode};
use inference_audio_device::{
    AudioHost, DeviceError, DeviceSelector, PlayoutConfig, PushError, Speaker,
};
use inference_duplex::inference_audio::{AudioFrame, SampleClockPosition};
use inference_duplex::{
    NativeSpeechModel, QuantityKind, ResponseId, SessionEvent, SpeechSessionHandle, UsageRecorded,
};
use realtime_voice::{Counts, Usage};

use crate::hosts::{Directive, Host, Line, Role, instructions, turn_config};

/// How long a model that creates its own responses may send no output before its turn is given
/// up: the turn's liveness bound, which the session's ledger does not run while no response is
/// awaited.
const SILENT_TURN: Duration = Duration::from_secs(10);

/// The turns in a row without audio that end the show.
pub const SILENT_TURNS: usize = 3;

/// The shortest average turn the most turns of a show are counted by.
pub const SHORTEST_TURN: Duration = Duration::from_secs(3);

/// The fewest turns a show may take, whatever its limit.
pub const FEWEST_TURNS: usize = 12;

/// How a show runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// The audio the show runs for before it closes.
    pub limit: Duration,
    /// How long one read of a session waits for the provider.
    pub wait: Duration,
    /// The most session events one step handles.
    pub max_events: usize,
    /// How much audio the speaker queues.
    pub playout: PlayoutConfig,
}

impl Default for Settings {
    /// Five minutes, reads of 10 ms, 64 events a step, four seconds of playout queue, enough
    /// to bridge the opening of the next turn's session; the meters read the played position.
    fn default() -> Self {
        Self {
            limit: Duration::from_secs(300),
            wait: Duration::from_millis(10),
            max_events: 64,
            playout: PlayoutConfig {
                buffer: Duration::from_secs(4),
            },
        }
    }
}

/// Something the show did, for the screen.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A turn's session opened and is generating.
    Generating {
        host: usize,
        turn: usize,
        directive: Directive,
    },
    /// A piece of a turn's transcript.
    Said {
        host: usize,
        turn: usize,
        text: String,
    },
    /// A turn's response ended; its whole transcript.
    Generated {
        host: usize,
        turn: usize,
        transcript: String,
    },
    /// A turn began to play; `queued` is what the speaker still held of earlier audio.
    Playing {
        host: usize,
        turn: usize,
        queued: usize,
    },
    /// A turn played to its end; `audio` is how long it was.
    Played {
        host: usize,
        turn: usize,
        audio: Duration,
    },
    /// Each step: who is heard, how loud, and the audio played so far.
    Tick {
        speaking: Option<usize>,
        level: f32,
        played: Duration,
    },
    /// A session reported usage.
    Usage { host: usize, usage: Usage },
    /// The session's ledger recorded a response's usage: each count as recorded, `None` where
    /// nobody counted it. It replaces what [`Event::Usage`] said of the response.
    Recorded {
        host: usize,
        response_id: ResponseId,
        counts: Counts,
    },
    /// A response of a host ended.
    Ended {
        host: usize,
        response_id: ResponseId,
    },
    /// A turn's session ended after `seconds`: what the binding billed, or else its wall time.
    Billed { host: usize, seconds: f64 },
    /// Something went wrong; the show ends once what was generated has played.
    Problem { host: Option<usize>, detail: String },
    /// The show is over.
    Finished { played: Duration },
}

/// The loudness of a frame: the root mean square of its samples, from 0 for silence to 1 for
/// a full-scale square wave; 0 for a frame that does not decode.
#[must_use]
pub fn level(frame: &AudioFrame) -> f32 {
    let Ok(samples) = decode(frame) else {
        return 0.0;
    };
    if samples.is_empty() {
        return 0.0;
    }
    let energy: f64 = samples
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum();
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    let rms = (energy / samples.len() as f64).sqrt() as f32;
    rms.min(1.0)
}

/// One turn: its audio not yet queued at the speaker, and how far it got.
#[derive(Debug)]
struct Turn {
    host: usize,
    index: usize,
    response: Option<ResponseId>,
    text: String,
    frames: VecDeque<AudioFrame>,
    /// The end of each queued frame, in samples of the turn, and its level.
    levels: Vec<(u64, f32)>,
    received: u64,
    pushed: u64,
    rate: u32,
    generated: bool,
    playing: bool,
    finished: bool,
}

impl Turn {
    fn audio(&self) -> Duration {
        samples(self.received, self.rate)
    }
}

fn samples(count: u64, rate: u32) -> Duration {
    if rate == 0 {
        return Duration::ZERO;
    }
    let nanos = u128::from(count) * 1_000_000_000 / u128::from(rate);
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

/// The session of the turn being generated.
struct Live<S> {
    session: S,
    host: usize,
    opened: Instant,
    /// When the session last sent output: a response, a transcript or audio.
    heard: Instant,
    /// Input samples sent, for a model fed silence.
    sent: u64,
    listen: bool,
}

/// A two-host show on one model and one speaker.
pub struct Show<M: NativeSpeechModel> {
    model: M,
    hosts: [Host; 2],
    brief: String,
    settings: Settings,
    speaker: Speaker,
    billed: fn(&M::Session) -> Option<f64>,
    transcript: Vec<Line>,
    queue: VecDeque<Turn>,
    live: Option<Live<M::Session>>,
    next: usize,
    closing: bool,
    closed: bool,
    /// The turns in a row generated without audio.
    silent_turns: usize,
    played: Duration,
    level: f32,
    over: bool,
}

impl<M: NativeSpeechModel> Show<M> {
    /// Opens the default output of `host`; no session opens before the first [`Show::step`].
    /// `billed` reads what a provider that bills session time billed for a turn's session.
    ///
    /// # Errors
    ///
    /// What the host answered when the speaker did not open.
    pub fn open(
        model: M,
        hosts: [Host; 2],
        brief: String,
        host: &dyn AudioHost,
        settings: Settings,
        billed: fn(&M::Session) -> Option<f64>,
    ) -> Result<Self, DeviceError> {
        let speaker = Speaker::open(host, &DeviceSelector::Default, settings.playout)?;
        Ok(Self {
            model,
            hosts,
            brief,
            settings,
            speaker,
            billed,
            transcript: Vec::new(),
            queue: VecDeque::new(),
            live: None,
            next: 0,
            closing: false,
            closed: false,
            silent_turns: 0,
            played: Duration::ZERO,
            level: 0.0,
            over: false,
        })
    }

    /// One step: the next turn started when there is room, the turn's session read, the
    /// speaker fed. Nothing once the show is over.
    pub fn step(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        if self.over {
            return events;
        }
        self.start_turn(&mut events);
        self.pump(&mut events);
        self.play(&mut events);
        let speaking = self
            .queue
            .front()
            .filter(|turn| turn.playing)
            .map(|turn| turn.host);
        events.push(Event::Tick {
            speaking,
            level: if speaking.is_some() { self.level } else { 0.0 },
            played: self.played(),
        });
        if self.closed && self.live.is_none() && self.queue.is_empty() {
            self.over = true;
            events.push(Event::Finished {
                played: self.played,
            });
        }
        events
    }

    /// Ends the show at once: the session in progress closed, the speaker stopped.
    pub fn stop(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        if self.over {
            return events;
        }
        if let Some(live) = self.live.take() {
            self.end_session(live, &mut events);
        }
        let played = self.played();
        let _ = self.speaker.stop();
        self.queue.clear();
        self.closed = true;
        self.over = true;
        events.push(Event::Finished { played });
        events
    }

    /// Whether the show is over.
    pub const fn is_over(&self) -> bool {
        self.over
    }

    /// The audio played so far: every turn played and the part of the one playing.
    pub fn played(&self) -> Duration {
        let current = self
            .queue
            .front()
            .filter(|turn| turn.playing)
            .and_then(|turn| {
                self.speaker
                    .played()
                    .map(|position| samples(position.get().min(turn.pushed), turn.rate))
            })
            .unwrap_or_default();
        self.played + current
    }

    /// The speaker.
    pub const fn speaker(&self) -> &Speaker {
        &self.speaker
    }

    /// The model.
    pub const fn model(&self) -> &M {
        &self.model
    }

    /// Opens the next turn's session when none is generating, nothing waits behind the turn
    /// playing, and the show is not closed.
    fn start_turn(&mut self, events: &mut Vec<Event>) {
        if self.live.is_some() || self.closed || self.queue.len() > 1 {
            return;
        }
        let index = self.next;
        let host = index % 2;
        let directive = if index == 0 {
            Directive::Open
        } else if self.closing || self.time_to_close(index) {
            self.closing = true;
            match self.hosts[host].role {
                Role::Guest => Directive::FinalWords,
                Role::Host => Directive::Close,
            }
        } else {
            Directive::Reply
        };
        let text = instructions(&self.hosts, host, &self.brief, &self.transcript, directive);
        let capabilities = self.model.capabilities().clone();
        let Some(config) = turn_config(&capabilities, &self.hosts[host].voice, text) else {
            self.fail(
                Some(host),
                "the model offers no audio format or turn detection".to_owned(),
                events,
            );
            return;
        };
        let rate = config.output_format.sample_rate.get();
        let mut session = match self.model.open(config) {
            Ok(session) => session,
            Err(error) => {
                self.fail(
                    Some(host),
                    format!("the session did not open: {error}"),
                    events,
                );
                return;
            }
        };
        let listen = !capabilities.manual_response;
        if !listen && let Err(error) = session.create_response() {
            self.fail(Some(host), format!("no response: {error}"), events);
            return;
        }
        if directive == Directive::Close {
            self.closed = true;
        }
        self.next += 1;
        events.push(Event::Generating {
            host,
            turn: index,
            directive,
        });
        self.queue.push_back(Turn {
            host,
            index,
            response: None,
            text: String::new(),
            frames: VecDeque::new(),
            levels: Vec::new(),
            received: 0,
            pushed: 0,
            rate,
            generated: false,
            playing: false,
            finished: false,
        });
        let opened = Instant::now();
        self.live = Some(Live {
            session,
            host,
            opened,
            heard: opened,
            sent: 0,
            listen,
        });
    }

    /// Whether turn `index`, every turn before it generated, should begin the closing: when
    /// the audio generated and the closing it would begin, at the average turn length so far
    /// (the guest's final words and the host's close on the guest's turn, the close alone on
    /// the host's), would run past the limit, or when the closing would otherwise not fit in
    /// the most turns of the show. Until the most turns are reached the show does not end
    /// before the limit, and it ends at most two average turns after it, where it began the
    /// closing only once the limit was generated.
    fn time_to_close(&self, index: usize) -> bool {
        let generated: Duration = self.played + self.queue.iter().map(Turn::audio).sum();
        let average = generated / u32::try_from(index.max(1)).unwrap_or(u32::MAX);
        let closing = match self.hosts[index % 2].role {
            Role::Guest => average * 2,
            Role::Host => average,
        };
        generated + closing > self.settings.limit || index + 2 >= self.most_turns()
    }

    /// The most turns the show takes: one per [`SHORTEST_TURN`] of the limit and the two
    /// closing turns, never fewer than [`FEWEST_TURNS`].
    fn most_turns(&self) -> usize {
        let per_turn = SHORTEST_TURN.as_millis().max(1);
        let turns = self.settings.limit.as_millis().div_ceil(per_turn);
        usize::try_from(turns)
            .unwrap_or(usize::MAX)
            .saturating_add(2)
            .max(FEWEST_TURNS)
    }

    /// Reads the generating turn's session.
    fn pump(&mut self, events: &mut Vec<Event>) {
        let Some(mut live) = self.live.take() else {
            return;
        };
        if live.listen {
            self.feed_silence(&mut live, events);
        }
        let mut done = false;
        for _ in 0..self.settings.max_events {
            let Some(event) = live.session.next_event(self.settings.wait) else {
                break;
            };
            if matches!(
                event,
                SessionEvent::ResponseCreated(_)
                    | SessionEvent::OutputTranscriptDelta(_)
                    | SessionEvent::OutputAudio(_)
            ) {
                live.heard = Instant::now();
            }
            done |= self.handle(live.host, event, events);
            if done {
                break;
            }
        }
        if !done && live.listen && live.heard.elapsed() >= SILENT_TURN {
            self.broken(
                live.host,
                format!("nothing from the provider for {} s", SILENT_TURN.as_secs()),
                events,
            );
            done = true;
        }
        if done {
            self.end_session(live, events);
            self.finish_turn(events);
        } else {
            self.live = Some(live);
        }
    }

    /// Sends silence to a model that creates its own responses, as much as the time since the
    /// session opened.
    fn feed_silence(&self, live: &mut Live<M::Session>, events: &mut Vec<Event>) {
        let format = live.session.ledger().session().config().input_format;
        let rate = u64::from(format.sample_rate.get());
        let due =
            u64::try_from(live.opened.elapsed().as_millis()).unwrap_or(u64::MAX) * rate / 1_000;
        let frame_samples = format.sample_rate.get() / 50;
        while live.sent < due {
            let silence = vec![0.0; frame_samples as usize * usize::from(format.channels.get())];
            let frame = AudioFrame {
                format,
                position: SampleClockPosition::new(live.sent).unwrap_or(SampleClockPosition::MAX),
                sample_count: frame_samples,
                data: encode(&silence, format.encoding),
            };
            if let Err(error) = live.session.append_audio(frame) {
                events.push(Event::Problem {
                    host: Some(live.host),
                    detail: format!("input audio: {error}"),
                });
                return;
            }
            live.sent += u64::from(frame_samples);
        }
    }

    /// Handles one event of the generating turn's session; whether the turn is done.
    fn handle(&mut self, host: usize, event: SessionEvent, events: &mut Vec<Event>) -> bool {
        let Some(turn) = self.queue.back_mut() else {
            return true;
        };
        let ours = |turn: &Turn, response_id: ResponseId| turn.response == Some(response_id);
        match event {
            SessionEvent::ResponseCreated(created) => {
                if turn.response.is_none() {
                    turn.response = Some(created.response_id);
                }
            }
            SessionEvent::OutputTranscriptDelta(delta) if ours(turn, delta.response_id) => {
                turn.text.push_str(&delta.text);
                events.push(Event::Said {
                    host,
                    turn: turn.index,
                    text: delta.text,
                });
            }
            SessionEvent::OutputAudio(audio) if ours(turn, audio.response_id) => {
                turn.received += u64::from(audio.frame.sample_count);
                turn.frames.push_back(audio.frame);
            }
            SessionEvent::ResponseCompleted(done) if ours(turn, done.response_id) => {
                events.push(Event::Ended {
                    host,
                    response_id: done.response_id,
                });
                return true;
            }
            SessionEvent::ResponseCancelled(cancelled) if ours(turn, cancelled.response_id) => {
                events.push(Event::Ended {
                    host,
                    response_id: cancelled.response_id,
                });
                self.broken(host, "the response was cancelled".to_owned(), events);
                return true;
            }
            SessionEvent::ResponseFailed(failed) if ours(turn, failed.response_id) => {
                events.push(Event::Ended {
                    host,
                    response_id: failed.response_id,
                });
                self.broken(
                    host,
                    format!("the response failed: {}", failed.failure),
                    events,
                );
                return true;
            }
            SessionEvent::SpeechSessionFailed(failed) => {
                self.broken(
                    host,
                    format!("the session failed: {}", failed.failure),
                    events,
                );
                return true;
            }
            SessionEvent::UsageRecorded(record) => {
                if let Some(event) = recorded(host, &record) {
                    events.push(event);
                }
            }
            SessionEvent::ResponseUsageRecorded(recorded) => events.push(Event::Usage {
                host,
                usage: Usage::Response {
                    response_id: recorded.response_id,
                    usage: recorded.usage,
                },
            }),
            SessionEvent::TranscriptionUsageRecorded(recorded) => events.push(Event::Usage {
                host,
                usage: Usage::Transcription(recorded.usage),
            }),
            _ => {}
        }
        false
    }

    /// Closes a turn's session, handing on the usage it reports last and the time it billed.
    fn end_session(&mut self, mut live: Live<M::Session>, events: &mut Vec<Event>) {
        if live.session.ledger().session().require_open().is_ok() {
            let _ = live.session.close();
        }
        while let Some(event) = live.session.next_event(Duration::ZERO) {
            match event {
                SessionEvent::ResponseUsageRecorded(recorded) => events.push(Event::Usage {
                    host: live.host,
                    usage: Usage::Response {
                        response_id: recorded.response_id,
                        usage: recorded.usage,
                    },
                }),
                SessionEvent::TranscriptionUsageRecorded(recorded) => events.push(Event::Usage {
                    host: live.host,
                    usage: Usage::Transcription(recorded.usage),
                }),
                SessionEvent::UsageRecorded(record) => {
                    if let Some(event) = recorded(live.host, &record) {
                        events.push(event);
                    }
                }
                _ => {}
            }
        }
        let seconds =
            (self.billed)(&live.session).unwrap_or_else(|| live.opened.elapsed().as_secs_f64());
        events.push(Event::Billed {
            host: live.host,
            seconds,
        });
    }

    /// The generating turn has all its audio: its transcript joins the conversation.
    fn finish_turn(&mut self, events: &mut Vec<Event>) {
        let Some(turn) = self.queue.back_mut() else {
            return;
        };
        turn.generated = true;
        let host = turn.host;
        let silent = turn.received == 0;
        let text = turn.text.trim().to_owned();
        events.push(Event::Generated {
            host,
            turn: turn.index,
            transcript: text.clone(),
        });
        if !text.is_empty() {
            self.transcript.push(Line {
                speaker: host,
                text,
            });
        }
        self.silent_turns = if silent { self.silent_turns + 1 } else { 0 };
        if self.silent_turns == SILENT_TURNS && !self.closed {
            self.fail(
                Some(host),
                format!("{SILENT_TURNS} turns in a row carried no audio"),
                events,
            );
        }
    }

    /// The generating turn of `host` broke off for `detail`: with audio it keeps what arrived,
    /// counts as done and the show goes on; without, the show ends once what was generated has
    /// played.
    fn broken(&mut self, host: usize, detail: String, events: &mut Vec<Event>) {
        if self.queue.back().is_some_and(|turn| turn.received > 0) {
            events.push(Event::Problem {
                host: Some(host),
                detail: format!("{detail}; the turn's audio is kept and the show goes on"),
            });
        } else {
            self.fail(Some(host), detail, events);
        }
    }

    /// Feeds the speaker the turn at the front of the queue, and ends it once it has played.
    fn play(&mut self, events: &mut Vec<Event>) {
        let Some(turn) = self.queue.front_mut() else {
            self.level = 0.0;
            return;
        };
        if !turn.playing {
            if turn.frames.is_empty() && !turn.generated {
                return;
            }
            let queued = self.speaker.queued();
            if queued > 0 {
                return;
            }
            let _ = self.speaker.stop();
            turn.playing = true;
            events.push(Event::Playing {
                host: turn.host,
                turn: turn.index,
                queued,
            });
        }
        while let Some(frame) = turn.frames.front() {
            match self.speaker.try_push(frame) {
                Ok(()) => {
                    turn.pushed += u64::from(frame.sample_count);
                    turn.levels.push((turn.pushed, level(frame)));
                }
                Err(PushError::Full) => break,
                Err(error) => events.push(Event::Problem {
                    host: Some(turn.host),
                    detail: format!("output audio: {error}"),
                }),
            }
            turn.frames.pop_front();
        }
        if turn.generated && turn.frames.is_empty() {
            if !turn.finished {
                self.speaker.finish();
                turn.finished = true;
            }
            if self.speaker.queued() == 0 {
                let audio = samples(turn.pushed, turn.rate);
                self.played += audio;
                events.push(Event::Played {
                    host: turn.host,
                    turn: turn.index,
                    audio,
                });
                self.queue.pop_front();
                self.level = 0.0;
                return;
            }
        }
        let heard = self.speaker.played().map_or(0, SampleClockPosition::get);
        self.level = turn
            .levels
            .iter()
            .find(|(end, _)| *end > heard)
            .map_or(0.0, |(_, level)| *level);
    }

    /// Ends the show once what was generated has played.
    fn fail(&mut self, host: Option<usize>, detail: String, events: &mut Vec<Event>) {
        events.push(Event::Problem { host, detail });
        self.closed = true;
    }
}

/// The event of a response's usage record of `host`; `None` for a record of no response.
fn recorded(host: usize, record: &UsageRecorded) -> Option<Event> {
    let count = |kind: QuantityKind| {
        record
            .quantities
            .iter()
            .find(|quantity| quantity.kind() == kind)
            .and_then(|quantity| quantity.value())
            .and_then(|value| u64::try_from(value).ok())
    };
    Some(Event::Recorded {
        host,
        response_id: record.response_id?,
        counts: Counts {
            input_text: count(QuantityKind::InputTextTokens),
            input_audio: count(QuantityKind::InputAudioTokens),
            input_cached: count(QuantityKind::CachedInputTokens),
            output_text: count(QuantityKind::OutputTextTokens),
            output_audio: count(QuantityKind::OutputAudioTokens),
        },
    })
}
