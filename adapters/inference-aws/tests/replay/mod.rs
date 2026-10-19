//! An offline replay of Amazon Nova Sonic: a task that reads the input events a [`Session`] sends
//! and answers with the output events of "Handling output events with the bidirectional API",
//! <https://docs.aws.amazon.com/nova/latest/nova2-userguide/sonic-output-events.html>, over the
//! same two channels [`inference_aws::nova_sonic::NovaSonic::start`] binds to the service. No
//! network, no credential.
//!
//! It hears speech in any audio frame holding a non-zero byte and silence in any other, as the
//! testkit's mock does. Speech after silence starts a user turn; silence after speech ends it, as
//! Nova Sonic's own turn detection would. Its recogniser reads the turn's speech bytes as text when
//! they read as words, otherwise it hears the next [`Turn`]'s `heard`. Each turn is answered in the
//! order the reference shows: the user transcript (`USER`, `FINAL`), a usage event, a speculative
//! assistant text, the assistant audio, the final assistant text ending `END_TURN`, usage events
//! between. Speech while the assistant's audio plays is barge-in: the replay sends the
//! interruption notice, one late audio output of the interrupted block and its `INTERRUPTED` end.
//!
//! Output is paced: one event per `pace`, so a test can speak during a response. [`Faults`] stop
//! the output after a number of events, close it, or send a service error.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use inference_aws::duplex::Connector;
use inference_aws::nova_sonic::{
    DEFAULT_MODEL_ID, NovaSonicError, OutputFrame, ServiceError, Session, SessionConfig,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// What the replay answers a user turn with; `{heard}` in its text is what it heard.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// Speaks the text.
    Say(String),
    /// Asks for the tool `tool` with `arguments`; once the result comes, says `then` with
    /// `{result}` replaced by the result's content.
    Call {
        tool: String,
        arguments: Value,
        then: String,
    },
}

/// One user turn of the script.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub heard: String,
    pub reply: Reply,
}

/// A turn whose reply says `reply`.
pub fn turn(heard: &str, reply: &str) -> Turn {
    Turn {
        heard: heard.to_owned(),
        reply: Reply::Say(reply.to_owned()),
    }
}

/// Faults, counted in output events sent.
#[derive(Debug, Clone, Default)]
pub struct Faults {
    /// No output after this many events; the stream stays open.
    pub stall_after: Option<usize>,
    /// The output side closes after this many events.
    pub close_after: Option<usize>,
    /// The service reports this error after this many events.
    pub error_after: Option<(usize, ServiceError)>,
    /// The stream does not open: the connect error the client reports for a refusal.
    pub refuse: bool,
}

/// The replay as a [`Connector`].
#[derive(Debug, Clone)]
pub struct Replay {
    pub model_id: String,
    pub script: Vec<Turn>,
    pub audio_frames: usize,
    pub pace: Duration,
    pub faults: Faults,
    /// Every input event the replay received, in order.
    pub wire: Arc<Mutex<Vec<Value>>>,
}

impl Replay {
    /// A replay of the default model answering `script`, two audio frames per reply, one event
    /// per millisecond.
    pub fn new(script: Vec<Turn>) -> Self {
        Replay {
            model_id: DEFAULT_MODEL_ID.to_owned(),
            script,
            audio_frames: 2,
            pace: Duration::from_millis(1),
            faults: Faults::default(),
            wire: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The input events received so far.
    pub fn received(&self) -> Vec<Value> {
        self.wire.lock().expect("the wire log").clone()
    }

    /// The input events received so far named `name`, their bodies.
    pub fn received_named(&self, name: &str) -> Vec<Value> {
        self.received()
            .iter()
            .filter_map(|event| event["event"].get(name).cloned())
            .collect()
    }
}

impl Connector for Replay {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn connect(
        &self,
        config: SessionConfig,
    ) -> impl Future<Output = Result<Session, NovaSonicError>> + Send + 'static {
        let replay = self.clone();
        async move {
            if replay.faults.refuse {
                return Err(NovaSonicError::Connect(
                    "the service returned an error: AccessDeniedException".to_owned(),
                ));
            }
            let (input, received) = mpsc::channel(256);
            let (frames, output) = mpsc::channel(256);
            let session = Session::open(&config, input, output).await?;
            tokio::spawn(Provider::new(replay, frames).run(received));
            Ok(session)
        }
    }
}

struct Provider {
    script: VecDeque<Turn>,
    audio_frames: usize,
    pace: Duration,
    faults: Faults,
    wire: Arc<Mutex<Vec<Value>>>,
    frames: mpsc::Sender<OutputFrame>,
    plan: VecDeque<Value>,
    sent: usize,
    errored: bool,
    started: bool,
    output_rate: u64,
    ids: usize,
    speaking: bool,
    spoken: Vec<u8>,
    playing: Option<String>,
    after_result: Option<String>,
    totals: [u64; 4],
}

impl Provider {
    fn new(replay: Replay, frames: mpsc::Sender<OutputFrame>) -> Self {
        Provider {
            script: replay.script.into(),
            audio_frames: replay.audio_frames,
            pace: replay.pace,
            faults: replay.faults,
            wire: replay.wire,
            frames,
            plan: VecDeque::new(),
            sent: 0,
            errored: false,
            started: false,
            output_rate: 24_000,
            ids: 0,
            speaking: false,
            spoken: Vec::new(),
            playing: None,
            after_result: None,
            totals: [0; 4],
        }
    }

    async fn run(mut self, mut received: mpsc::Receiver<Vec<u8>>) {
        loop {
            if self
                .faults
                .close_after
                .is_some_and(|after| self.sent >= after)
            {
                return;
            }
            if let Some((after, error)) = self.faults.error_after.clone()
                && self.sent >= after
                && !self.errored
            {
                self.errored = true;
                let _ = self.frames.send(Err(error)).await;
            }
            let stalled = self
                .faults
                .stall_after
                .is_some_and(|after| self.sent >= after);
            let ready = !stalled && !self.plan.is_empty();
            tokio::select! {
                biased;
                input = received.recv() => match input {
                    None => return,
                    Some(bytes) => {
                        if !self.input(&bytes).await {
                            return;
                        }
                    }
                },
                () = tokio::time::sleep(self.pace), if ready => self.emit().await,
            }
        }
    }

    async fn emit(&mut self) {
        let Some(event) = self.plan.pop_front() else {
            return;
        };
        if let Some(start) = event["event"].get("contentStart")
            && start["type"] == "AUDIO"
        {
            self.playing = start["contentId"].as_str().map(str::to_owned);
        }
        if let Some(end) = event["event"].get("contentEnd")
            && end["type"] == "AUDIO"
        {
            self.playing = None;
        }
        self.sent += 1;
        let _ = self.frames.send(Ok(event.to_string().into_bytes())).await;
    }

    /// Takes one input event; `false` once the session ended.
    async fn input(&mut self, bytes: &[u8]) -> bool {
        let event: Value = serde_json::from_slice(bytes).expect("an input event is JSON");
        self.wire.lock().expect("the wire log").push(event.clone());
        let Some((name, body)) = event["event"]
            .as_object()
            .and_then(|object| object.iter().next())
        else {
            return true;
        };
        match name.as_str() {
            "promptStart" => {
                if let Some(rate) = body["audioOutputConfiguration"]["sampleRateHertz"].as_u64() {
                    self.output_rate = rate;
                }
            }
            "audioInput" => {
                let pcm = STANDARD
                    .decode(body["content"].as_str().unwrap_or_default())
                    .expect("audio input is base64");
                let speech: Vec<u8> = pcm.iter().copied().filter(|byte| *byte != 0).collect();
                if !speech.is_empty() && !self.speaking {
                    self.speaking = true;
                    if let Some(audio) = self.playing.take() {
                        self.barge_in(audio);
                    }
                }
                if !speech.is_empty() {
                    self.spoken.extend(speech);
                } else if self.speaking {
                    self.speaking = false;
                    self.end_turn();
                }
            }
            "toolResult" => {
                let content = body["content"].as_str().unwrap_or_default().to_owned();
                if let Some(then) = self.after_result.take() {
                    self.say(&then.replace("{result}", &content));
                }
            }
            "promptEnd" => {
                let end = self.completion_end();
                let _ = self.frames.send(Ok(end.to_string().into_bytes())).await;
            }
            "sessionEnd" => return false,
            _ => {}
        }
        true
    }

    fn next_id(&mut self, kind: &str) -> String {
        self.ids += 1;
        format!("{kind}-{}", self.ids)
    }

    fn with_ids(mut body: Value) -> Value {
        body["sessionId"] = json!("replay-session");
        body["promptName"] = json!("replay-prompt");
        body["completionId"] = json!("replay-completion");
        body
    }

    fn event(name: &str, body: Value) -> Value {
        json!({"event": {name: Self::with_ids(body)}})
    }

    fn text_block(&mut self, role: &str, stage: &str, text: &str, stop: &str) {
        let id = self.next_id("text");
        self.plan.push_back(Self::event(
            "contentStart",
            json!({
                "contentId": id,
                "type": "TEXT",
                "role": role,
                "additionalModelFields": format!("{{\"generationStage\":\"{stage}\"}}"),
                "textOutputConfiguration": {"mediaType": "text/plain"}
            }),
        ));
        self.plan.push_back(Self::event(
            "textOutput",
            json!({"contentId": id, "content": text}),
        ));
        self.plan.push_back(Self::event(
            "contentEnd",
            json!({"contentId": id, "type": "TEXT", "stopReason": stop}),
        ));
    }

    fn audio_output(&self, content_id: &str) -> Value {
        let samples = usize::try_from(self.output_rate / 50).unwrap_or(0);
        Self::event(
            "audioOutput",
            json!({"contentId": content_id, "content": STANDARD.encode(vec![0u8; samples * 2])}),
        )
    }

    /// A usage event: the change `[input speech, input text, output speech, output text]` and the
    /// running totals after it.
    fn usage(&mut self, delta: [u64; 4]) {
        for (total, change) in self.totals.iter_mut().zip(delta) {
            *total += change;
        }
        let [is, it, os, ot] = self.totals;
        self.plan.push_back(Self::event(
            "usageEvent",
            json!({
                "details": {
                    "delta": {
                        "input": {"speechTokens": delta[0], "textTokens": delta[1]},
                        "output": {"speechTokens": delta[2], "textTokens": delta[3]}
                    },
                    "total": {
                        "input": {"speechTokens": is, "textTokens": it},
                        "output": {"speechTokens": os, "textTokens": ot}
                    }
                },
                "totalInputTokens": is + it,
                "totalOutputTokens": os + ot,
                "totalTokens": is + it + os + ot
            }),
        ));
    }

    fn completion_end(&self) -> Value {
        Self::event("completionEnd", json!({"stopReason": "END_TURN"}))
    }

    /// The assistant speaks `text`: speculative text, audio, final text, usage between.
    fn say(&mut self, text: &str) {
        self.text_block("ASSISTANT", "SPECULATIVE", text, "PARTIAL_TURN");
        let audio = self.next_id("audio");
        self.plan.push_back(Self::event(
            "contentStart",
            json!({
                "contentId": audio,
                "type": "AUDIO",
                "role": "ASSISTANT",
                "audioOutputConfiguration": {
                    "mediaType": "audio/lpcm",
                    "sampleRateHertz": self.output_rate,
                    "sampleSizeBits": 16,
                    "channelCount": 1,
                    "encoding": "base64"
                }
            }),
        ));
        for _ in 0..self.audio_frames {
            let frame = self.audio_output(&audio);
            self.plan.push_back(frame);
        }
        self.plan.push_back(Self::event(
            "contentEnd",
            json!({"contentId": audio, "type": "AUDIO", "stopReason": "PARTIAL_TURN"}),
        ));
        self.usage([0, 0, 30, 0]);
        self.text_block("ASSISTANT", "FINAL", text, "END_TURN");
        self.usage([0, 0, 0, 8]);
    }

    /// The user turn ended: its transcript, then the scripted reply.
    fn end_turn(&mut self) {
        let recognised = recognised(std::mem::take(&mut self.spoken));
        let Some(turn) = self.script.pop_front() else {
            return;
        };
        let heard = recognised.unwrap_or(turn.heard);
        if !self.started {
            self.started = true;
            self.plan
                .push_back(Self::event("completionStart", json!({})));
        }
        self.text_block("USER", "FINAL", &heard, "END_TURN");
        self.usage([50, 0, 0, 0]);
        match turn.reply {
            Reply::Say(text) => self.say(&text.replace("{heard}", &heard)),
            Reply::Call {
                tool,
                arguments,
                then,
            } => {
                let id = self.next_id("tool");
                let tool_use_id = self.next_id("tooluse");
                self.plan.push_back(Self::event(
                    "contentStart",
                    json!({
                        "contentId": id,
                        "type": "TOOL",
                        "role": "TOOL",
                        "toolUseOutputConfiguration": {"mediaType": "application/json"}
                    }),
                ));
                self.plan.push_back(Self::event(
                    "toolUse",
                    json!({
                        "contentId": id,
                        "toolUseId": tool_use_id,
                        "toolName": tool,
                        "content": arguments.to_string()
                    }),
                ));
                self.plan.push_back(Self::event(
                    "contentEnd",
                    json!({"contentId": id, "type": "TOOL", "stopReason": "TOOL_USE"}),
                ));
                self.after_result = Some(then.replace("{heard}", &heard));
            }
        }
    }

    /// The user spoke over the audio `audio`: the rest of the turn is not sent; the notice, one
    /// late frame of the interrupted audio and its end are.
    fn barge_in(&mut self, audio: String) {
        self.plan.clear();
        self.text_block(
            "ASSISTANT",
            "SPECULATIVE",
            "{ \"interrupted\" : true }",
            "INTERRUPTED",
        );
        let late = self.audio_output(&audio);
        self.plan.push_back(late);
        self.plan.push_back(Self::event(
            "contentEnd",
            json!({"contentId": audio, "type": "AUDIO", "stopReason": "INTERRUPTED"}),
        ));
    }
}

/// What the recogniser heard in a turn's speech bytes, when they read as words.
fn recognised(bytes: Vec<u8>) -> Option<String> {
    let text = String::from_utf8(bytes).ok()?;
    let words = text
        .chars()
        .all(|character| character.is_ascii_graphic() || character == ' ')
        && text
            .chars()
            .any(|character| character.is_ascii_alphabetic());
    words.then(|| text.trim().to_owned())
}
