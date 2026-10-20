//! The two hosts, what each turn asks of its host, and the session a turn is spoken in.
//!
//! Each turn is a session of its own: the host's voice, and instructions holding the host's
//! persona, the brief, the conversation so far as text and what the turn asks. The duplex
//! contract takes no text input, so this is how one host's transcript reaches the other.

use std::fmt::Write as _;
use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{SessionConfig, SpeechCapabilities, TurnDetection, TurnTuning};

/// Who a host is on the show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Runs the show: curious, asks, opens and closes it.
    Host,
    /// Knows the project: explains, with examples.
    Guest,
}

/// One voice of the show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub name: String,
    pub voice: String,
    pub role: Role,
}

/// The show's two voices: Maya, the curious host, and Theo, the expert guest.
#[must_use]
pub fn default_hosts() -> [Host; 2] {
    [
        Host {
            name: "Maya".to_owned(),
            voice: "marin".to_owned(),
            role: Role::Host,
        },
        Host {
            name: "Theo".to_owned(),
            voice: "cedar".to_owned(),
            role: Role::Guest,
        },
    ]
}

/// What a turn asks of its host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Directive {
    /// The first turn: welcome the listeners and ask the first question.
    Open,
    /// Answer the other host.
    Reply,
    /// Time is almost up: the guest's last thought.
    FinalWords,
    /// Time is up: the host closes the show.
    Close,
}

/// One finished turn of the conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub speaker: usize,
    pub text: String,
}

/// The persona of `host`, talking with `other`.
fn persona(host: &Host, other: &Host) -> String {
    match host.role {
        Role::Host => format!(
            "You are {}, the host of a technology podcast. You are curious and warm, you ask \
             one sharp question at a time, react to what {} says, and now and then sum up for \
             the listeners.",
            host.name, other.name
        ),
        Role::Guest => format!(
            "You are {}, the guest on a technology podcast hosted by {}. You know the project \
             in the brief inside out; you explain it with concrete examples and honest \
             opinions, and sometimes ask {} a question back.",
            host.name, other.name, other.name
        ),
    }
}

/// What `directive` asks of `host`.
fn ask(directive: Directive, host: &Host, other: &Host) -> String {
    match directive {
        Directive::Open => format!(
            "Open the show now: welcome the listeners, say what today's episode is about, \
             introduce yourself and your guest {}, and ask the first question.",
            other.name
        ),
        Directive::Reply => format!(
            "It is your turn: answer {}'s last words and carry the conversation on.",
            other.name
        ),
        Directive::FinalWords => format!(
            "Time is almost up: give your final thought on the project in one or two \
             sentences and thank {}.",
            other.name
        ),
        Directive::Close => format!(
            "Time is up: close the show now. Thank {}, sum the episode up in one sentence and \
             say goodbye to the listeners as {}.",
            other.name, host.name
        ),
    }
}

/// The instructions of a turn of `hosts[speaker]`: the persona, the brief, the conversation so
/// far as text, one line per turn, and what `directive` asks.
#[must_use]
pub fn instructions(
    hosts: &[Host; 2],
    speaker: usize,
    brief: &str,
    transcript: &[Line],
    directive: Directive,
) -> String {
    let host = &hosts[speaker % 2];
    let other = &hosts[(speaker + 1) % 2];
    let mut text = persona(host, other);
    text.push_str(
        " Speak English, naturally, as in a real conversation: two to four sentences a turn, \
         no lists, no stage directions, never read out file paths letter by letter, and do not \
         say your own name before you speak.\n\nThe brief of the project:\n",
    );
    text.push_str(brief.trim());
    text.push_str("\n\nThe conversation so far:\n");
    if transcript.is_empty() {
        text.push_str("(nothing yet)\n");
    }
    for line in transcript {
        let _ = writeln!(
            text,
            "{}: {}",
            hosts[line.speaker % 2].name,
            line.text.trim()
        );
    }
    text.push('\n');
    text.push_str(&ask(directive, host, other));
    text
}

/// PCM16 mono at 24 kHz, the format both hosts speak in where the model offers it.
fn pcm24() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(24_000).unwrap_or(NonZeroU32::MIN),
        channels: NonZeroU16::MIN,
    }
}

/// The session a turn is spoken in, by what the model takes: PCM16 at 24 kHz both ways (or the
/// model's first formats), no tools. A model that takes responses from the caller gets no
/// turn detection and no automatic response: the show asks for the one response of the turn.
/// A model that creates every response itself keeps its own turn-taking. The liveness bound is
/// ten seconds: a provider silent that long in the middle of a turn has failed it, and a longer
/// bound is heard as dead air. `None` when the model offers no format or turn detection at all.
#[must_use]
pub fn turn_config(
    capabilities: &SpeechCapabilities,
    voice: &str,
    instructions: String,
) -> Option<SessionConfig> {
    let wanted = pcm24();
    let output_format = if capabilities.output_formats.contains(&wanted) {
        wanted
    } else {
        *capabilities.output_formats.first()?
    };
    let input_format = if capabilities.input_formats.contains(&output_format) {
        output_format
    } else {
        *capabilities.input_formats.first()?
    };
    let manual = capabilities.manual_response;
    let turn_detection = if manual
        && capabilities
            .turn_detection
            .contains(&TurnDetection::Disabled)
    {
        TurnDetection::Disabled
    } else if capabilities
        .turn_detection
        .contains(&TurnDetection::SemanticVad)
    {
        TurnDetection::SemanticVad
    } else {
        *capabilities.turn_detection.first()?
    };
    Some(SessionConfig {
        voice: voice.to_owned(),
        instructions,
        input_format,
        output_format,
        turn_detection,
        turn_tuning: TurnTuning::default(),
        auto_response: !manual,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(10),
    })
}
