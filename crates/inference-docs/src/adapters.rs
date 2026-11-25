//! The adapter reference, `website/docs/reference/adapters.md`: which capabilities each model
//! binding claims, from the records the adapters commit, and what the local ones measured on the
//! reference machine.
//!
//! This crate links no adapter. A hosted binding's claims are a claims record under the adapter's
//! `records/`, held equal to the binding's code by a test in that adapter (`held_by`). A local
//! binding's claims are the capabilities its capability record gives evidence for, which the
//! adapter's record test loads and checks. Generation refuses a record whose `adapter_revision` is
//! not the workspace version, a claim that is not a capability, a feature the package does not
//! have, and a test the record names that its adapter does not define.
//!
//! A binding may carry one note ([`NOTES`]): what an open, a close or a drop gives a caller or
//! holds a thread for, with the release that brought it. Generation refuses:
//!
//! - a note on a binding the page does not list, and a second note on one binding;
//! - a constant listed in the note's `constants` that the file named beside it does not define
//!   exactly once as whole seconds or milliseconds ([`Note::constants`] names the forms read), or
//!   that the text does not give at least once as "`NAME` (value)" with that value;
//! - a code span that is a path of identifiers, `a` or `a::b`, one of whose parts is no whole
//!   word of the package's sources, comments and strings included;
//! - a code span `a::b` whose last part no source of the package or of a crate under `crates/`
//!   defines as a function, a constant, a type, a module or an enum variant.
//!
//! That is all it holds. Other numbers of the text, other mentions of a listed constant, and the
//! prose are not checked. Of `a::b` only the last part is held to a definition: that `b` belongs
//! to `a`, is public, or is about that binding rather than another of the same package is not
//! checked. A code span that is no path of identifiers, such as `session.closed` or a feature
//! name, is skipped.

use std::fs;
use std::path::Path;

use inference_core::model::Capability;
use serde::Deserialize;
use serde_json::Value;

use crate::crates::{Package, packages, workspace_version};
use crate::errors::names;
use crate::{Result, SOURCE, TREE, cell, front_matter, test_defined};

/// Where the adapter reference lands, relative to the repository root.
pub const PAGE: &str = "website/docs/reference/adapters.md";

/// Where a binding's claims come from.
#[derive(Clone, Copy)]
enum Record {
    /// A claims record of a hosted binding, under the adapter's directory.
    Claims(&'static str),
    /// A capability record of a local binding, under the adapter's directory, and the model it
    /// was measured with.
    Capability(&'static str, &'static str),
}

/// When a binding shipped, by its `CHANGELOG.md` entry.
#[derive(Clone, Copy, Debug)]
pub enum Release {
    /// In the release `0.x.0` whose section introduced it, and in every one after it.
    Since(&'static str),
    /// On `main`, its entry under `[Unreleased]`, and in no release yet. Unused from a release
    /// until the next binding or note lands on `main`, as `status::next` is.
    #[allow(dead_code)]
    Next,
}

impl Release {
    /// The status the page gives it.
    pub fn status(self) -> String {
        match self {
            Self::Since(version) => format!("Since {version}"),
            Self::Next => crate::status::NEXT_HEADING.to_owned(),
        }
    }
}

/// One model binding the page lists.
struct Binding {
    /// What the reader calls it.
    name: &'static str,
    /// The workspace package that holds it.
    package: &'static str,
    /// The contract it implements and how it runs.
    runs: &'static str,
    /// The features a local binding needs; a hosted binding's come from its record.
    features: &'static [&'static str],
    record: Record,
    /// The variables it reads.
    environment: &'static str,
    /// The licences of its code and weights, or where its data goes.
    licence: &'static str,
    /// When it shipped, by its `CHANGELOG.md` entry.
    release: Release,
}

/// Every model binding, in the order the page lists them.
const BINDINGS: &[Binding] = &[
    Binding {
        name: "OpenAI Realtime",
        package: "inference-openai",
        runs: "native speech (duplex); hosted, WebSocket",
        features: &[],
        record: Record::Claims("records/claims-realtime.json"),
        environment: "`OPENAI_API_KEY`, or the keychain secret `openai-api-key` with feature `keychain`; `INFERENCE_OPENAI_REALTIME_MODEL`",
        licence: "hosted service: audio leaves the machine",
        release: Release::Since("0.5.0"),
    },
    Binding {
        name: "OpenAI GPT-Live",
        package: "inference-openai",
        runs: "native speech (duplex); hosted, WebSocket (`v1/live/sessions`)",
        features: &[],
        record: Record::Claims("records/claims-gpt-live.json"),
        environment: "`OPENAI_API_KEY`, or the keychain secret `openai-api-key` with feature `keychain`",
        licence: "hosted service: audio leaves the machine; billed per session second",
        release: Release::Since("0.6.0"),
    },
    Binding {
        name: "Amazon Nova Sonic on Bedrock",
        package: "inference-aws",
        runs: "native speech (duplex); hosted, bidirectional stream",
        features: &[],
        record: Record::Claims("records/claims-nova-sonic.json"),
        environment: "the AWS default credential chain; `AWS_REGION`; `INFERENCE_NOVA_SONIC_MODEL`",
        licence: "hosted service: audio leaves the machine",
        release: Release::Since("0.5.0"),
    },
    Binding {
        name: "ElevenLabs streaming TTS",
        package: "inference-elevenlabs",
        runs: "TTS, streaming; hosted, WebSocket (`multi-stream-input`), one connection per utterance, one context per segment",
        features: &[],
        record: Record::Claims("records/claims-tts.json"),
        environment: "`ELEVENLABS_API_KEY`, or the keychain secret `elevenlabs-api-key` with feature `keychain`",
        licence: "hosted service: text leaves the machine",
        release: Release::Since("0.16.0"),
    },
    Binding {
        name: "OpenAI streamed speech TTS",
        package: "inference-openai",
        runs: "TTS, streaming; hosted, HTTP (`POST /v1/audio/speech`, chunked PCM), one request per segment",
        features: &[],
        record: Record::Claims("records/claims-speech.json"),
        environment: "`OPENAI_API_KEY`, or the keychain secret `openai-api-key` with feature `keychain`",
        licence: "hosted service: text leaves the machine",
        release: Release::Since("0.22.0"),
    },
    Binding {
        name: "ElevenLabs agent conversation",
        package: "inference-elevenlabs",
        runs: "native speech (duplex); hosted, WebSocket (`v1/convai/conversation`), the agent's own model, voice and formats",
        features: &[],
        record: Record::Claims("records/claims-agent.json"),
        environment: "`ELEVENLABS_AGENT_ID` for a public agent, or `ELEVENLABS_AGENT_SIGNED_URL` for a private one (the live test and `compare`)",
        licence: "hosted service: audio leaves the machine",
        release: Release::Since("0.17.0"),
    },
    Binding {
        name: "Cartesia Sonic streaming TTS",
        package: "inference-cartesia",
        runs: "TTS, streaming; hosted, WebSocket (`tts/websocket`), one connection per synthesis, one context per segment",
        features: &[],
        record: Record::Claims("records/claims-tts.json"),
        environment: "`CARTESIA_API_KEY`, or the keychain secret `cartesia-api-key` with feature `keychain`",
        licence: "hosted service: text leaves the machine",
        release: Release::Since("0.22.0"),
    },
    Binding {
        name: "Deepgram streaming STT",
        package: "inference-deepgram",
        runs: "STT, streaming; hosted, WebSocket (`v1/listen`), one connection per recognition",
        features: &[],
        record: Record::Claims("records/claims-stt.json"),
        environment: "`DEEPGRAM_API_KEY`, or the keychain secret `deepgram-api-key` with feature `keychain`",
        licence: "hosted service: audio leaves the machine",
        release: Release::Since("0.17.0"),
    },
    Binding {
        name: "ElevenLabs realtime STT",
        package: "inference-elevenlabs",
        runs: "STT, streaming; hosted, WebSocket (`v1/speech-to-text/realtime`), one connection per recognition, μ-law 8 kHz or PCM16 in, committed spans with word timings",
        features: &[],
        record: Record::Claims("records/claims-stt.json"),
        environment: "`ELEVENLABS_API_KEY`, or the keychain secret `elevenlabs-api-key` with feature `keychain`",
        licence: "hosted service: audio leaves the machine",
        release: Release::Since("0.28.0"),
    },
    Binding {
        name: "Deepgram Voice Agent",
        package: "inference-deepgram",
        runs: "native speech (duplex); hosted, WebSocket (`v1/agent/converse`), the listen, think and speak providers its `Settings` name",
        features: &[],
        record: Record::Claims("records/claims-voice-agent.json"),
        environment: "`DEEPGRAM_API_KEY` with Voice Agent access, or the keychain secret `deepgram-api-key` with feature `keychain`",
        licence: "hosted service: audio leaves the machine",
        release: Release::Since("0.17.0"),
    },
    Binding {
        name: "Phonic speech-to-speech",
        package: "inference-phonic",
        runs: "native speech (duplex); hosted, WebSocket (`v1/sts/ws`), `phonic-1` unless configured otherwise",
        features: &[],
        record: Record::Claims("records/claims-duplex.json"),
        environment: "`PHONIC_API_KEY`, or the keychain secret `phonic-api-key` with feature `keychain`",
        licence: "hosted service: audio leaves the machine",
        release: Release::Since("0.22.0"),
    },
    Binding {
        name: "Deepslate Opal",
        package: "inference-deepslate",
        runs: "native speech (duplex); hosted, WebSocket (`api/v1/vendors/<vendor>/organizations/<organization>/realtime`), protocol buffers messages",
        features: &[],
        record: Record::Claims("records/claims-opal.json"),
        environment: "`DEEPSLATE_API_KEY`, or the keychain secret `deepslate-api-key` with feature `keychain`; `DEEPSLATE_VENDOR_ID`, `DEEPSLATE_ORGANIZATION_ID`, and `DEEPSLATE_VOICE_ID` for spoken output",
        licence: "hosted service: audio leaves the machine",
        release: Release::Since("0.22.0"),
    },
    Binding {
        name: "sherpa-onnx streaming STT",
        package: "inference-sherpa-onnx",
        runs: "STT, streaming; in process, native library",
        features: &["stt"],
        record: Record::Capability(
            "records/stt-i9-10900k-cpu.json",
            "streaming Zipformer, English",
        ),
        environment: "`INFERENCE_SHERPA_ONNX_STT_DIR`",
        licence: "code Apache-2.0 (sherpa-onnx 1.13.8), MIT (onnxruntime); weights Apache-2.0, trained on LibriSpeech (CC BY 4.0)",
        release: Release::Since("0.6.0"),
    },
    Binding {
        name: "whisper.cpp batch STT",
        package: "inference-whispercpp",
        runs: "STT, batch; in process, native library, CPU",
        features: &[],
        record: Record::Capability("records/stt-i9-10900k-cpu.json", "Whisper base.en"),
        environment: "`INFERENCE_WHISPERCPP_STT_MODEL`",
        licence: "code MIT (whisper.cpp), Unlicense (whisper-rs); weights MIT",
        release: Release::Since("0.6.0"),
    },
    Binding {
        name: "llama-server LLM",
        package: "inference-llama-server",
        runs: "LLM; local worker, loopback HTTP",
        features: &[],
        record: Record::Capability(
            "capability/reference-machine-cpu.json",
            "Qwen2.5-1.5B-Instruct q4_k_m",
        ),
        environment: "`INFERENCE_LLAMA_SERVER_URL`, `INFERENCE_LLAMA_SERVER_MODEL`",
        licence: "code MIT (llama.cpp); weights Apache-2.0",
        release: Release::Since("0.6.0"),
    },
    Binding {
        name: "llama-server LLM, tools on",
        package: "inference-llama-server",
        runs: "LLM; local worker, loopback HTTP, server started with `--jinja`",
        features: &["tools"],
        record: Record::Capability(
            "capability/reference-machine-cpu-tools.json",
            "Qwen2.5-1.5B-Instruct q4_k_m",
        ),
        environment: "`INFERENCE_LLAMA_SERVER_URL`, `INFERENCE_LLAMA_SERVER_MODEL`",
        licence: "code MIT (llama.cpp); weights Apache-2.0",
        release: Release::Since("0.6.0"),
    },
];

/// What an open, a close or a drop of a binding gives a caller or holds a thread for, beyond its
/// claims, with the release that brought it.
struct Note {
    /// The binding, by its name in [`BINDINGS`].
    binding: &'static str,
    /// When it shipped, by its `CHANGELOG.md` entry.
    release: Release,
    /// The constants the text gives with their value, each as its name and the file that defines
    /// it, a path under the package's `src/`. Generation reads the value from that file only,
    /// which must define it exactly once as `pub const NAME: <type> = <…>from_secs(n);` or
    /// `from_millis(n)`, on one line or wrapped after `=`, `n` in digits with or without `_`; the
    /// text gives it at least once as "`NAME` (n s)" or "`NAME` (n ms)".
    constants: &'static [(&'static str, &'static str)],
    text: &'static str,
}

/// The binding whose note the error reference links to.
pub const AGENT_CONVERSATION: &str = "ElevenLabs agent conversation";

/// Every note, at most one per binding.
const NOTES: &[Note] = &[
    Note {
        binding: "OpenAI GPT-Live",
        release: Release::Next,
        constants: &[
            ("CLOSE_TIMEOUT", "live.rs"),
            ("DEFAULT_READ_TIMEOUT", "config.rs"),
        ],
        text: "`LiveClient::with_close_timeout` sets the close timeout and \
               `LiveClient::close_timeout` reads it; it stays with the session across \
               `LiveClient::into_split` and `LiveClient::reunite`. \
               `OpenAiLive::with_close_timeout` and `OpenAiLive::close_timeout` do the same for \
               the sessions the model opens. Unset, it is `CLOSE_TIMEOUT` (15 s), as before. A \
               graceful close waits for `session.closed` at most the longer of the close timeout \
               and the endpoint's read timeout, `DEFAULT_READ_TIMEOUT` (30 s) unless \
               `Endpoint::with_read_timeout` set another. With that default, a close timeout of \
               30 s or less changes nothing; a shorter close needs a shorter read timeout, which \
               also bounds every read of the session. Without `session.closed` by then, \
               `LiveClient::close` returns `RealtimeError::Stalled`, and an `OpenAiLive` session \
               closes with the usage seen so far, unconfirmed.",
    },
    Note {
        binding: "ElevenLabs streaming TTS",
        release: Release::Next,
        constants: &[
            ("CLOSE_BOUND", "connection.rs"),
            ("CLOSE_GRACE", "synthesiser.rs"),
        ],
        text: "Dropping an `ElevenLabsSynthesis` closes its connection with `close_socket` and a \
               close frame, within `CLOSE_BOUND` (1 s). Inside a Tokio runtime the drop holds the \
               dropping thread at most `CLOSE_GRACE` (5 ms). A close that ended by then has been \
               sent when the drop returns; one that did not goes on, on the handle's own thread, \
               within `CLOSE_BOUND`. When the process ends before that close is written, the \
               socket ends without the close frame, and the server may have received \
               `close_socket` alone, or neither. Outside a Tokio runtime, and inside another \
               executor, the drop waits for the close, at most `CLOSE_BOUND`. A handle dropped \
               while its async form waits for a message to be sent or to arrive ends the socket \
               without a close frame.",
    },
    Note {
        binding: AGENT_CONVERSATION,
        release: Release::Next,
        constants: &[],
        text: "A model built with `ElevenLabsAgent::signed` opens at most one session. Its first \
               open that the binding does not refuse for its configuration, and whose runtime \
               was built, takes the signed URL for its handshake, whatever happens next: also \
               when nothing connects, the server refuses the upgrade, the handshake times out or \
               is cancelled, or the agent's formats are not the session's. Every later open that \
               the binding does not refuse for its configuration, and whose runtime was built, \
               fails with `SessionFailureKind::Rejected` before anything connects, and \
               `ElevenLabsAgent::open_error` is `agent::AgentError::SignedUrlSpent`. For the next \
               open, also the one with the formats the agent reported, fetch a new signed URL \
               and build a new model with it. An open refused for its configuration, or whose \
               runtime could not be built, leaves the URL with the model. A model built with \
               `ElevenLabsAgent::public` opens any number of sessions.",
    },
    Note {
        binding: "Cartesia Sonic streaming TTS",
        release: Release::Next,
        constants: &[
            ("CLOSE_BOUND", "connection.rs"),
            ("CLOSE_GRACE", "synthesiser.rs"),
        ],
        text: "Dropping a `CartesiaSynthesis` closes its connection with a close frame, within \
               `CLOSE_BOUND` (1 s). Inside a Tokio runtime the drop holds the dropping thread at \
               most `CLOSE_GRACE` (5 ms). A close that ended by then has been sent when the drop \
               returns; one that did not goes on, on the handle's own thread, within \
               `CLOSE_BOUND`. When the process ends before that close is written, the socket ends \
               without the close frame. Outside a Tokio runtime, and inside another executor, the \
               drop waits for the close, at most `CLOSE_BOUND`. A handle dropped while its async \
               form waits for a message to be sent or to arrive ends the socket without a close \
               frame.",
    },
];

/// The text of the note on `binding`, if it has one.
pub fn note(binding: &str) -> Option<&'static str> {
    NOTES
        .iter()
        .find(|note| note.binding == binding)
        .map(|note| note.text)
}

/// The value of the constant `name` as `text` defines it, in the forms [`Note::constants`]
/// names, written `n s` or `n ms`. `Err` says what is wrong.
fn duration_constant(text: &str, name: &str) -> std::result::Result<String, &'static str> {
    let start = format!("pub const {name}:");
    let definitions: Vec<&str> = text
        .match_indices(start.as_str())
        .filter(|(at, _)| {
            text[..*at]
                .rsplit('\n')
                .next()
                .unwrap_or_default()
                .trim()
                .is_empty()
        })
        .map(|(at, _)| &text[at + start.len()..])
        .collect();
    let rest = match definitions[..] {
        [] => return Err("does not define"),
        [rest] => rest,
        _ => return Err("defines more than once"),
    };
    let expression = rest
        .split_once('=')
        .and_then(|(_, after)| after.split_once(';'))
        .map(|(expression, _)| expression)
        .ok_or("has no value `= …;` for")?;
    let number = |call: &str| -> Option<u64> {
        let (digits, tail) = expression.split_once(call)?.1.split_once(')')?;
        let digits = digits.trim();
        if digits.is_empty()
            || !tail.trim().is_empty()
            || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
        {
            return None;
        }
        digits.replace('_', "").parse().ok()
    };
    number("from_secs(")
        .map(|n| format!("{n} s"))
        .or_else(|| number("from_millis(").map(|n| format!("{n} ms")))
        .ok_or("gives no whole seconds or milliseconds in digits for")
}

/// Whether `span` is a path of identifiers, `a` or `a::b`.
fn is_item(span: &str) -> bool {
    span.split("::").all(|part| {
        part.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Whether a line of `text` outside a `//` comment defines `item`: `fn`, `const`, `struct`,
/// `enum`, `trait`, `type` or `mod` and the name, or a line that starts with the name alone or
/// followed by `,`, `(` or `{`, as an enum variant does (and so does a call on a line of its
/// own, which this takes for a definition too).
fn defines(text: &str, item: &str) -> bool {
    let identifier = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.lines()
        .map(str::trim_start)
        .filter(|line| !line.starts_with("//"))
        .any(|line| {
            let keyword = [
                "fn ", "const ", "struct ", "enum ", "trait ", "type ", "mod ",
            ]
            .iter()
            .any(|keyword| {
                let needle = format!("{keyword}{item}");
                line.match_indices(needle.as_str()).any(|(at, _)| {
                    !line[..at].chars().next_back().is_some_and(identifier)
                        && line[at + needle.len()..]
                            .chars()
                            .next()
                            .is_some_and(|c| "(<:{; ".contains(c))
                })
            });
            let variant = line.strip_prefix(item).is_some_and(|rest| {
                let rest = rest.trim_start();
                rest.is_empty() || rest.starts_with([',', '(', '{'])
            });
            keyword || variant
        })
}

/// The section of the notes, each binding given as its name and its package's directory, the
/// sources as `errors::sources` reads them; empty without a note.
fn notes_section(
    bindings: &[(&str, &str)],
    notes: &[Note],
    sources: &[(String, String)],
) -> Result<String> {
    for note in notes {
        if !bindings.iter().any(|(name, _)| *name == note.binding) {
            return Err(format!("a note is on no binding {}", note.binding));
        }
    }
    if notes.is_empty() {
        return Ok(String::new());
    }
    let shared: Vec<&str> = sources
        .iter()
        .filter(|(file, _)| file.starts_with("crates/"))
        .map(|(_, text)| text.as_str())
        .collect();
    let mut out = String::from(
        "\n## Opens, closes and drops\n\nWhat an open, a close or a drop of a binding gives a \
         caller or holds a thread for, where a release changed it. The status is that of the \
         release that brought it, as in the bindings table. A constant is the adapter's, with \
         the value its source defines.\n\n",
    );
    for (name, directory) in bindings {
        let mine: Vec<&Note> = notes.iter().filter(|note| note.binding == *name).collect();
        let note = match mine[..] {
            [] => continue,
            [note] => note,
            _ => return Err(format!("{name} has two notes; a binding has one at most")),
        };
        let prefix = format!("{directory}/src/");
        let own: Vec<&str> = sources
            .iter()
            .filter(|(file, _)| file.starts_with(&prefix))
            .map(|(_, text)| text.as_str())
            .collect();
        for (constant, file) in note.constants {
            let path = format!("{prefix}{file}");
            let text = sources
                .iter()
                .find(|(source, _)| *source == path)
                .map(|(_, text)| text.as_str())
                .ok_or_else(|| format!("{name}: {constant} is in {path}, which does not exist"))?;
            let value = duration_constant(text, constant)
                .map_err(|problem| format!("{name}: {path} {problem} {constant}"))?;
            let written = format!("`{constant}` ({value})");
            if !note.text.contains(&written) {
                return Err(format!("{name}: the note does not say {written}"));
            }
        }
        let items: Vec<&str> = note
            .text
            .split('`')
            .skip(1)
            .step_by(2)
            .filter(|span| is_item(span))
            .collect();
        let unnamed: Vec<String> = items
            .iter()
            .filter(|span| {
                span.split("::")
                    .any(|part| !own.iter().any(|text| names(text, part)))
            })
            .map(|span| format!("`{span}`"))
            .collect();
        if !unnamed.is_empty() {
            return Err(format!(
                "{name}: no source of {directory} names {}",
                unnamed.join(", ")
            ));
        }
        let undefined: Vec<String> = items
            .iter()
            .filter_map(|span| Some((span, span.rsplit_once("::")?.1)))
            .filter(|(_, last)| {
                !own.iter()
                    .chain(shared.iter())
                    .any(|text| defines(text, last))
            })
            .map(|(span, _)| format!("`{span}`"))
            .collect();
        if !undefined.is_empty() {
            return Err(format!(
                "{name}: no source of {directory} or of crates/ defines the last part of {}",
                undefined.join(", ")
            ));
        }
        out.push_str(&format!(
            "- **{name}** ({}). {}\n",
            note.release.status(),
            cell(note.text)
        ));
    }
    Ok(out)
}

/// A hosted binding's claims record.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimsRecord {
    model: String,
    binding: String,
    features: Vec<String>,
    claims: Vec<String>,
    held_by: String,
    adapter_revision: String,
}

/// The parts of a local capability record the page reads.
#[derive(Deserialize)]
struct CapabilityRecord {
    hardware: String,
    evidence: Vec<Evidence>,
    resource_profile: ResourceProfile,
    adapter_revision: String,
}

#[derive(Deserialize)]
struct Evidence {
    capability: String,
    tests: Vec<String>,
}

#[derive(Deserialize)]
struct ResourceProfile {
    peak_resident_memory_mib: u64,
    load_time_ms: u64,
    #[serde(default)]
    cpu_real_time_factor_milli: Option<u64>,
    gpu_memory_mib: Option<u64>,
}

/// One binding with what its record says.
struct Read<'a> {
    binding: &'a Binding,
    model: String,
    features: Vec<String>,
    claims: Vec<String>,
    /// The record's path relative to the repository root.
    path: String,
    /// Local only: the hardware and the resource profile.
    measured: Option<(String, ResourceProfile)>,
}

/// The serialized name of every capability, in declaration order.
fn capability_names() -> Result<Vec<String>> {
    Capability::ALL
        .iter()
        .map(|capability| match serde_json::to_value(capability) {
            Ok(Value::String(name)) => Ok(name),
            _ => Err(format!("{capability:?} does not serialize as a name")),
        })
        .collect()
}

/// Refuse `test` (`binary::function`) unless `tests/<binary>.rs` of the package defines it as a
/// test. An ignored test is admitted: a local record names tests that need a checkpoint or a
/// running server, which the adapter's record test runs when they are present.
fn check_test(root: &Path, package: &Package, test: &str, what: &str) -> Result<()> {
    let (binary, name) = test
        .split_once("::")
        .ok_or_else(|| format!("{what}: {test} is not `binary::function`"))?;
    let file = format!("{}/tests/{binary}.rs", package.directory);
    let source = fs::read_to_string(root.join(&file))
        .map_err(|error| format!("{what}: reading {file}: {error}"))?;
    match test_defined(&source, name) {
        Ok(()) | Err("an ignored test") => Ok(()),
        Err(problem) => Err(format!("{what}: {file} has no test {name} ({problem})")),
    }
}

fn read<'a>(
    root: &Path,
    binding: &'a Binding,
    package: &Package,
    version: &str,
    names: &[String],
) -> Result<Read<'a>> {
    let relative = match binding.record {
        Record::Claims(path) | Record::Capability(path, _) => {
            format!("{}/{path}", package.directory)
        }
    };
    let text = fs::read_to_string(root.join(&relative))
        .map_err(|error| format!("{}: reading {relative}: {error}", binding.name))?;
    let parse_error = |error: serde_json::Error| format!("{}: {relative}: {error}", binding.name);
    let (model, features, claims, revision, measured) = match binding.record {
        Record::Claims(_) => {
            let record: ClaimsRecord = serde_json::from_str(&text).map_err(parse_error)?;
            if !record.binding.starts_with(&package.name.replace('-', "_")) {
                return Err(format!(
                    "{}: {relative} names binding {} outside {}",
                    binding.name, record.binding, package.name
                ));
            }
            check_test(root, package, &record.held_by, binding.name)?;
            (
                record.model,
                record.features,
                record.claims,
                record.adapter_revision,
                None,
            )
        }
        Record::Capability(_, model) => {
            let record: CapabilityRecord = serde_json::from_str(&text).map_err(parse_error)?;
            for evidence in &record.evidence {
                if evidence.tests.is_empty() {
                    return Err(format!(
                        "{}: {relative} claims {} with no test",
                        binding.name, evidence.capability
                    ));
                }
                for test in &evidence.tests {
                    check_test(root, package, test, binding.name)?;
                }
            }
            (
                model.to_owned(),
                binding.features.iter().map(|f| (*f).to_owned()).collect(),
                record
                    .evidence
                    .iter()
                    .map(|evidence| evidence.capability.clone())
                    .collect(),
                record.adapter_revision,
                Some((record.hardware, record.resource_profile)),
            )
        }
    };
    if revision != version {
        return Err(format!(
            "{}: {relative} is of adapter revision {revision}, the workspace is at {version}",
            binding.name
        ));
    }
    for claim in &claims {
        if !names.contains(claim) {
            return Err(format!(
                "{}: {relative} claims {claim}, which is not a capability",
                binding.name
            ));
        }
    }
    for feature in &features {
        if !package.features.contains(feature) {
            return Err(format!(
                "{}: {} has no feature {feature}",
                binding.name, package.name
            ));
        }
    }
    Ok(Read {
        binding,
        model,
        features,
        claims,
        path: relative,
        measured,
    })
}

fn code_list(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_owned()
    } else {
        items
            .iter()
            .map(|item| format!("`{item}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Thousands separated by commas.
fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn page_for(root: &Path, metadata: &Value, bindings: &[Binding], notes: &[Note]) -> Result<String> {
    let all = packages(metadata)?;
    let version = workspace_version(metadata)?;
    let names = capability_names()?;
    let mut reads = Vec::new();
    for binding in bindings {
        let package = all
            .iter()
            .find(|package| package.name == binding.package)
            .ok_or_else(|| format!("{}: no package {}", binding.name, binding.package))?;
        reads.push(read(root, binding, package, &version, &names)?);
    }
    let mut out = front_matter(
        "Adapters",
        "Adapters",
        "Every model binding of the inference adapters: what it claims, the features it needs and what it measured.",
        2,
    );
    out.push_str(&format!(
        "# Adapters\n\nGenerated by `inference-docs` from the records the adapters commit. A \
         hosted binding's claims are a claims record that a test in its adapter holds equal to the \
         code; a local binding's claims are the capabilities its capability record gives evidence \
         for, each evidence a named test of the adapter. Every adapter is off until you depend on \
         it and turn on the features below. [Contracts](../concepts/contracts.md) says what each \
         capability means.\n\n## Bindings\n\n\
         Status names the release whose [changelog]({SOURCE}/CHANGELOG.md) section introduced \
         the binding: \"Since 0.x.0\" is in that release and every one after it, and \
         \"{next}\" is built and tested on `main`, its entry under Unreleased, and in no \
         release yet.\n\n\
         | Binding | Package | Runs | Model | Features | Status |\n\
         |---|---|---|---|---|---|\n",
        next = crate::status::NEXT_HEADING,
    ));
    for read in &reads {
        out.push_str(&format!(
            "| {} | [`{}`]({TREE}/{}) | {} | `{}` | {} | {} |\n",
            read.binding.name,
            read.binding.package,
            all.iter()
                .find(|package| package.name == read.binding.package)
                .map(|package| package.directory.as_str())
                .unwrap_or_default(),
            cell(read.binding.runs),
            cell(&read.model),
            code_list(&read.features),
            read.binding.release.status(),
        ));
    }
    out.push_str(
        "\n## Environment, licences and records\n\nWhat each binding reads from the environment, \
         the licences of its code and weights or where its data goes, and the record its claims \
         come from.\n\n",
    );
    for read in &reads {
        out.push_str(&format!(
            "- **{}**. Environment: {}. Licences or data: {}. Record: [{}]({SOURCE}/{}).\n",
            read.binding.name,
            cell(read.binding.environment),
            cell(read.binding.licence),
            read.path.rsplit('/').next().unwrap_or_default(),
            read.path,
        ));
    }
    let directories: Vec<(&str, &str)> = reads
        .iter()
        .map(|read| {
            (
                read.binding.name,
                all.iter()
                    .find(|package| package.name == read.binding.package)
                    .map(|package| package.directory.as_str())
                    .unwrap_or_default(),
            )
        })
        .collect();
    out.push_str(&notes_section(
        &directories,
        notes,
        &crate::errors::sources(root)?,
    )?);
    out.push_str("\n## Claims\n\nEvery capability of the contract and the bindings that claim it, separated by semicolons. A capability nobody claims is still part of the contract: a pipeline that requires it is refused at planning, naming the node.\n\n| Capability | Claimed by |\n|---|---|\n");
    for name in &names {
        let claimed: Vec<_> = reads
            .iter()
            .filter(|read| read.claims.contains(name))
            .map(|read| read.binding.name)
            .collect();
        let claimed = if claimed.is_empty() {
            "none".to_owned()
        } else {
            claimed.join("; ")
        };
        out.push_str(&format!("| `{name}` | {claimed} |\n"));
    }
    out.push_str(
        "\n## Measured on the reference machine\n\nFrom each local binding's capability record. \
         The CPU real-time factor is compute time over audio time; below 1 keeps up.\n\n\
         | Binding | Peak resident memory | Load time | CPU real-time factor | GPU memory |\n\
         |---|---|---|---|---|\n",
    );
    let mut hardware = Vec::new();
    for read in &reads {
        let Some((machine, profile)) = &read.measured else {
            continue;
        };
        if !hardware.contains(machine) {
            hardware.push(machine.clone());
        }
        out.push_str(&format!(
            "| {} | {} MiB | {} ms | {} | {} |\n",
            read.binding.name,
            grouped(profile.peak_resident_memory_mib),
            grouped(profile.load_time_ms),
            profile
                .cpu_real_time_factor_milli
                .map_or("n/a".to_owned(), |milli| format!(
                    "{}.{:03}",
                    milli / 1000,
                    milli % 1000
                )),
            profile
                .gpu_memory_mib
                .map_or("none".to_owned(), |mib| format!("{} MiB", grouped(mib))),
        ));
    }
    out.push_str("\nThe machines, as the records name them:\n\n");
    for machine in hardware {
        out.push_str(&format!("- {}\n", cell(&machine)));
    }
    Ok(out)
}

/// The adapter reference for the repository at `root`.
pub fn page(root: &Path, metadata: &Value) -> Result<String> {
    page_for(root, metadata, BINDINGS, NOTES)
}

/// Each binding's name, the release it shipped in, and whether it is hosted.
#[cfg(test)]
pub fn releases() -> impl Iterator<Item = (&'static str, Release, bool)> {
    BINDINGS.iter().map(|binding| {
        (
            binding.name,
            binding.release,
            matches!(binding.record, Record::Claims(_)),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crates;

    fn metadata() -> Value {
        crates::metadata(&crate::repository_root()).unwrap()
    }

    #[test]
    fn the_page_lists_every_claim_of_every_record() {
        let page = page(&crate::repository_root(), &metadata()).unwrap();
        assert!(
            page.contains("| Capability | Claimed by |\n|---|---|\n")
                && page.contains("| `native_duplex` | OpenAI GPT-Live |\n")
                && page.contains("| `revisable_input` | none |\n"),
            "{page}"
        );
        assert!(page.contains("| sherpa-onnx streaming STT | 363 MiB | 4,937 ms | 0.131 | none |"));
    }

    #[test]
    fn the_bindings_table_keeps_the_long_cells_out_in_a_list() {
        let page = page(&crate::repository_root(), &metadata()).unwrap();
        assert!(
            page.contains(
                "| Binding | Package | Runs | Model | Features | Status |\n|---|---|---|---|---|---|\n"
            ),
            "{page}"
        );
        let list = page
            .split_once("\n## Environment, licences and records\n")
            .unwrap()
            .1;
        assert!(
            list.contains("- **whisper.cpp batch STT**. Environment: `INFERENCE_WHISPERCPP_STT_MODEL`. Licences or data: code MIT (whisper.cpp), Unlicense (whisper-rs); weights MIT. Record: [stt-i9-10900k-cpu.json]("),
            "{list}"
        );
    }

    #[test]
    fn a_record_of_another_revision_or_naming_a_missing_test_is_refused() {
        let root = crate::scratch("adapters");
        let metadata = metadata();
        let openai = packages(&metadata)
            .unwrap()
            .into_iter()
            .find(|package| package.name == "inference-openai")
            .unwrap();
        let version = workspace_version(&metadata).unwrap();
        let dir = root.join(&openai.directory);
        fs::create_dir_all(dir.join("records")).unwrap();
        fs::create_dir_all(dir.join("tests")).unwrap();
        fs::write(dir.join("tests/claims.rs"), "#[test]\nfn holds() {}\n").unwrap();
        let binding = Binding {
            name: "B",
            package: "inference-openai",
            runs: "r",
            features: &[],
            record: Record::Claims("records/c.json"),
            environment: "e",
            licence: "l",
            release: Release::Since("0.1.0"),
        };
        let write = |revision: &str, held_by: &str, claim: &str| {
            fs::write(
                dir.join("records/c.json"),
                serde_json::json!({
                    "model": "m",
                    "binding": "inference_openai::x",
                    "features": ["duplex"],
                    "claims": [claim],
                    "held_by": held_by,
                    "adapter_revision": revision,
                })
                .to_string(),
            )
            .unwrap();
        };
        let names = capability_names().unwrap();
        write(&version, "claims::holds", "output_streaming");
        assert!(read(&root, &binding, &openai, &version, &names).is_ok());
        write("0.0.1", "claims::holds", "output_streaming");
        let error = read(&root, &binding, &openai, &version, &names)
            .err()
            .unwrap();
        assert!(error.contains("adapter revision 0.0.1"), "{error}");
        write(&version, "claims::gone", "output_streaming");
        let error = read(&root, &binding, &openai, &version, &names)
            .err()
            .unwrap();
        assert!(error.contains("has no test gone"), "{error}");
        write(&version, "claims::holds", "telepathy");
        let error = read(&root, &binding, &openai, &version, &names)
            .err()
            .unwrap();
        assert!(
            error.contains("telepathy, which is not a capability"),
            "{error}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Per binding, a phrase of the `CHANGELOG.md` entry that introduced it.
    const INTRODUCED: &[(&str, &str)] = &[
        (
            "OpenAI Realtime",
            "OpenAI Realtime as an `inference-duplex` native speech model",
        ),
        (
            "OpenAI GPT-Live",
            "OpenAI GPT-Live (`v1/live/sessions`) behind feature `gpt-live`",
        ),
        (
            "Amazon Nova Sonic on Bedrock",
            "Amazon Nova Sonic as an `inference-duplex` native speech model",
        ),
        (
            "ElevenLabs streaming TTS",
            "`inference-elevenlabs`, the first hosted TTS adapter",
        ),
        (
            "OpenAI streamed speech TTS",
            "`inference-openai`: feature `speech`, `speech::OpenAiSpeech`",
        ),
        (
            "ElevenLabs agent conversation",
            "`ElevenLabsAgent` in `inference-elevenlabs`, behind feature `duplex`",
        ),
        (
            "Cartesia Sonic streaming TTS",
            "`inference-cartesia`: a new adapter crate",
        ),
        (
            "Deepgram streaming STT",
            "`DeepgramRecogniser`, Deepgram streaming speech-to-text",
        ),
        (
            "ElevenLabs realtime STT",
            "`ElevenLabsRecogniser`, ElevenLabs realtime speech-to-text",
        ),
        (
            "Deepgram Voice Agent",
            "`DeepgramAgent` is a Deepgram Voice Agent session",
        ),
        (
            "Phonic speech-to-speech",
            "`inference-phonic`: a new adapter crate",
        ),
        (
            "Deepslate Opal",
            "`inference-deepslate`: a new adapter crate",
        ),
        (
            "sherpa-onnx streaming STT",
            "the sherpa-onnx streaming recogniser as a local STT adapter",
        ),
        (
            "whisper.cpp batch STT",
            "whisper.cpp as a local batch STT adapter",
        ),
        (
            "llama-server LLM",
            "a local llama.cpp `llama-server` as an output-streaming LLM",
        ),
        (
            "llama-server LLM, tools on",
            "tool proposals as structured events, and one capability record per configuration",
        ),
    ];

    /// One section of `CHANGELOG.md`, `[Unreleased]` or a release, its text and its `### Added`
    /// list with their whitespace collapsed, so a phrase matches across the file's line breaks.
    struct Section {
        version: String,
        text: String,
        added: String,
    }

    const UNRELEASED: &str = "Unreleased";

    fn collapsed(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The sections of `changelog`, oldest first, `[Unreleased]` last.
    fn sections(changelog: &str) -> Vec<Section> {
        let mut sections: Vec<_> = changelog
            .split("\n## [")
            .skip(1)
            .filter_map(|section| {
                let (version, text) = section.split_once(']')?;
                Some(Section {
                    version: version.to_owned(),
                    text: collapsed(text),
                    added: text
                        .split("\n### ")
                        .find(|list| list.starts_with("Added\n"))
                        .map(collapsed)
                        .unwrap_or_default(),
                })
            })
            .collect();
        sections.reverse();
        sections
    }

    /// What is wrong with each `(binding, release, introducing phrase)` against `changelog`: the
    /// oldest section that carries the phrase, in its Added list when `added_only`, must be the
    /// binding's release, `[Unreleased]` for one on main in no release yet.
    fn introduction_problems(
        changelog: &str,
        bindings: &[(&str, Release, &str)],
        added_only: bool,
    ) -> Vec<String> {
        let sections = sections(changelog);
        let mut wrong = Vec::new();
        for (name, release, phrase) in bindings {
            let expected = match release {
                Release::Since(version) if version.starts_with(|c: char| c.is_ascii_digit()) => {
                    *version
                }
                Release::Since(version) => {
                    wrong.push(format!("{name}: since {version:?}, which is no release"));
                    continue;
                }
                Release::Next => UNRELEASED,
            };
            match sections
                .iter()
                .find(|section| section.text.contains(phrase))
            {
                Some(section)
                    if section.version == expected
                        && (!added_only || section.added.contains(phrase)) => {}
                Some(section) => wrong.push(format!(
                    "{name}: {}, but {phrase:?} is first in the {} section{}",
                    release.status(),
                    section.version,
                    if !added_only || section.added.contains(phrase) {
                        ""
                    } else {
                        ", outside its Added list"
                    }
                )),
                None => wrong.push(format!(
                    "{name}: {}, but no section says {phrase:?}",
                    release.status()
                )),
            }
        }
        wrong
    }

    #[test]
    fn every_binding_is_since_the_release_whose_changelog_section_introduced_it() {
        let changelog = fs::read_to_string(crate::repository_root().join("CHANGELOG.md")).unwrap();
        assert!(
            sections(&changelog).len() > 20,
            "{} sections",
            sections(&changelog).len()
        );
        let mut bindings = Vec::new();
        let mut wrong = Vec::new();
        for binding in BINDINGS {
            match INTRODUCED.iter().find(|(name, _)| *name == binding.name) {
                Some((_, phrase)) => bindings.push((binding.name, binding.release, *phrase)),
                None => wrong.push(format!("{}: no introducing phrase", binding.name)),
            }
        }
        wrong.extend(introduction_problems(&changelog, &bindings, true));
        for (name, _) in INTRODUCED {
            if !BINDINGS.iter().any(|binding| binding.name == *name) {
                wrong.push(format!("{name}: no such binding"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn a_binding_is_on_main_only_while_its_entry_is_unreleased() {
        let changelog = "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- `fresh`: a new \
                         adapter\n  crate.\n\n## [0.2.0] — 2026-01-02\n\n### Added\n\n- `old`: a \
                         new adapter crate.\n\n## [0.1.0] — 2026-01-01\n\n### Added\n\n- The \
                         contracts.\n";
        let fresh = "`fresh`: a new adapter crate";
        let old = "`old`: a new adapter crate";
        assert_eq!(
            introduction_problems(
                changelog,
                &[
                    ("F", Release::Next, fresh),
                    ("O", Release::Since("0.2.0"), old)
                ],
                true
            ),
            Vec::<String>::new()
        );
        let refused =
            |release, phrase| introduction_problems(changelog, &[("B", release, phrase)], true);
        let wrong = refused(Release::Next, old);
        assert!(
            wrong.len() == 1 && wrong[0].contains("is first in the 0.2.0 section"),
            "{wrong:?}"
        );
        let wrong = refused(Release::Since("0.2.0"), fresh);
        assert!(
            wrong.len() == 1 && wrong[0].contains("is first in the Unreleased section"),
            "{wrong:?}"
        );
        let wrong = refused(Release::Since(UNRELEASED), fresh);
        assert!(
            wrong.len() == 1 && wrong[0].contains("which is no release"),
            "{wrong:?}"
        );
        let wrong = refused(Release::Next, "`gone`");
        assert!(
            wrong.len() == 1 && wrong[0].contains("no section says"),
            "{wrong:?}"
        );
    }

    #[test]
    fn a_binding_on_main_is_shown_as_in_the_next_release() {
        let next = [Binding {
            release: Release::Next,
            ..BINDINGS[0]
        }];
        let page = page_for(&crate::repository_root(), &metadata(), &next, &[]).unwrap();
        let row = page
            .lines()
            .find(|line| line.starts_with(&format!("| {} |", BINDINGS[0].name)))
            .unwrap();
        assert!(row.ends_with(" | On main, in the next release |"), "{row}");
        assert!(page.contains("\"On main, in the next release\""), "{page}");
    }

    #[test]
    fn every_row_of_the_bindings_table_names_the_release_it_shipped_in() {
        let page = page(&crate::repository_root(), &metadata()).unwrap();
        let table = page
            .split_once("\n## Bindings\n")
            .unwrap()
            .1
            .split_once("\n## ")
            .unwrap()
            .0;
        assert!(
            table.contains(
                "| Binding | Package | Runs | Model | Features | Status |\n|---|---|---|---|---|---|\n"
            ),
            "{table}"
        );
        let rows: Vec<_> = table
            .lines()
            .filter(|line| line.starts_with("| ") && !line.starts_with("| Binding |"))
            .collect();
        assert_eq!(rows.len(), BINDINGS.len(), "{table}");
        for (row, binding) in rows.iter().zip(BINDINGS) {
            assert!(row.starts_with(&format!("| {} |", binding.name)), "{row}");
            assert!(
                row.ends_with(&format!(" | {} |", binding.release.status())),
                "{row}"
            );
        }
        assert!(
            rows.iter()
                .any(|row| row.starts_with("| ElevenLabs realtime STT |")
                    && row.ends_with(" | Since 0.28.0 |")),
            "{table}"
        );
    }

    #[test]
    fn the_page_says_what_an_open_a_close_or_a_drop_gives_a_caller() {
        let page = page(&crate::repository_root(), &metadata()).unwrap();
        let section = page
            .split_once("\n## Opens, closes and drops\n")
            .unwrap_or_else(|| panic!("no section Opens, closes and drops in\n{page}"))
            .1
            .split_once("\n## ")
            .unwrap()
            .0;
        for expected in [
            "- **ElevenLabs agent conversation** (On main, in the next release). A model built \
             with `ElevenLabsAgent::signed` opens at most one session.",
            "`agent::AgentError::SignedUrlSpent`",
            "- **OpenAI GPT-Live** (On main, in the next release). `LiveClient::with_close_timeout`",
            "`CLOSE_TIMEOUT` (15 s)",
            "`DEFAULT_READ_TIMEOUT` (30 s)",
            "With that default, a close timeout of 30 s or less changes nothing",
            "- **ElevenLabs streaming TTS** (On main, in the next release). Dropping an \
             `ElevenLabsSynthesis`",
            "- **Cartesia Sonic streaming TTS** (On main, in the next release). Dropping a \
             `CartesiaSynthesis`",
            "`CLOSE_GRACE` (5 ms)",
            "`CLOSE_BOUND` (1 s)",
        ] {
            assert!(
                section.contains(expected),
                "{expected:?} is not in\n{section}"
            );
        }
        let at = |name: &str| {
            section
                .find(&format!("- **{name}** "))
                .unwrap_or_else(|| panic!("no note on {name}"))
        };
        assert!(
            at("OpenAI GPT-Live") < at("ElevenLabs streaming TTS")
                && at("ElevenLabs streaming TTS") < at("ElevenLabs agent conversation")
                && at("ElevenLabs agent conversation") < at("Cartesia Sonic streaming TTS"),
            "the notes are not in the order of the bindings:\n{section}"
        );
    }

    #[test]
    fn a_note_the_sources_of_its_package_do_not_hold_is_refused() {
        let bindings = [("B", "adapters/b")];
        let sources = vec![
            (
                "adapters/b/src/lib.rs".to_owned(),
                "pub const GRACE: Duration = Duration::from_millis(5);\n\
                 pub const BOUND: std::time::Duration = std::time::Duration::from_secs(1);\n\
                 pub struct Handle;\nimpl Handle {\n    pub fn close(&self) {}\n}\n\
                 // Kind::Rejected and Kind::Stalled; open comes later.\n"
                    .to_owned(),
            ),
            (
                "adapters/c/src/lib.rs".to_owned(),
                "pub struct Elsewhere;\npub const OTHER: Duration = Duration::from_secs(2);\n"
                    .to_owned(),
            ),
            (
                "crates/core/src/lib.rs".to_owned(),
                "pub enum Kind {\n    Rejected,\n}\n".to_owned(),
            ),
        ];
        let note = |binding: &'static str,
                    constants: &'static [(&'static str, &'static str)],
                    text: &'static str| Note {
            binding,
            release: Release::Next,
            constants,
            text,
        };
        let held = notes_section(
            &bindings,
            &[note(
                "B",
                &[("GRACE", "lib.rs"), ("BOUND", "lib.rs")],
                "`Handle::close` waits `GRACE` (5 ms), then `BOUND` (1 s), and fails with \
                 `Kind::Rejected`, for `x.close` with feature `some-feature`.",
            )],
            &sources,
        )
        .unwrap();
        assert!(
            held.contains(
                "- **B** (On main, in the next release). `Handle::close` waits `GRACE` (5 ms)"
            ),
            "{held}"
        );
        let refused = |notes: &[Note]| notes_section(&bindings, notes, &sources).err().unwrap();
        let error = refused(&[note("B", &[("GRACE", "lib.rs")], "`GRACE` (5 s)")]);
        assert!(error.contains("`GRACE` (5 ms)"), "{error}");
        let error = refused(&[note("B", &[("OTHER", "lib.rs")], "`OTHER` (2 s)")]);
        assert!(
            error.contains("adapters/b/src/lib.rs does not define OTHER"),
            "{error}"
        );
        let error = refused(&[note("B", &[("GRACE", "gone.rs")], "`GRACE` (5 ms)")]);
        assert!(
            error.contains("adapters/b/src/gone.rs, which does not exist"),
            "{error}"
        );
        let error = refused(&[note("B", &[], "`Handle::open` and `Elsewhere`")]);
        assert!(
            error.contains("no source of adapters/b names")
                && error.contains("`Elsewhere`")
                && !error.contains("`Handle::open`"),
            "{error}"
        );
        let error = refused(&[note("B", &[], "`Kind::Stalled` and `Handle::open`")]);
        assert!(
            error.contains("defines the last part of `Kind::Stalled`, `Handle::open`"),
            "{error}"
        );
        let error = refused(&[note("C", &[], "text")]);
        assert!(error.contains("no binding C"), "{error}");
        let error = refused(&[note("B", &[], "one"), note("B", &[], "two")]);
        assert!(error.contains("B has two notes"), "{error}");
    }

    #[test]
    fn a_constant_is_read_from_its_own_file_in_the_forms_rustfmt_leaves() {
        let bindings = [("B", "adapters/b")];
        let sources = vec![
            (
                "adapters/b/src/agent.rs".to_owned(),
                "pub const READ: Duration = Duration::from_secs(30);\n".to_owned(),
            ),
            (
                "adapters/b/src/config.rs".to_owned(),
                "pub const READ: Duration = Duration::from_secs(10);\n\
                 pub const WIDE: Duration = Duration::from_millis(1_000);\n\
                 pub const WRAPPED: std::time::Duration =\n    \
                 std::time::Duration::from_secs(15);\n\
                 pub const EXPR: Duration = Duration::from_secs(60 * 5);\n"
                    .to_owned(),
            ),
            (
                "adapters/b/src/twice.rs".to_owned(),
                "#[cfg(unix)]\npub const BOTH: Duration = Duration::from_secs(1);\n\
                 #[cfg(not(unix))]\npub const BOTH: Duration = Duration::from_secs(2);\n"
                    .to_owned(),
            ),
        ];
        let note = |constants: &'static [(&'static str, &'static str)], text: &'static str| Note {
            binding: "B",
            release: Release::Next,
            constants,
            text,
        };
        let section = |constants: &'static [(&'static str, &'static str)], text: &'static str| {
            notes_section(&bindings, &[note(constants, text)], &sources)
        };
        assert!(section(&[("READ", "config.rs")], "`READ` (10 s)").is_ok());
        let error = section(&[("READ", "agent.rs")], "`READ` (10 s)").unwrap_err();
        assert!(error.contains("does not say `READ` (30 s)"), "{error}");
        assert!(section(&[("WIDE", "config.rs")], "`WIDE` (1000 ms)").is_ok());
        assert!(section(&[("WRAPPED", "config.rs")], "`WRAPPED` (15 s)").is_ok());
        let error = section(&[("EXPR", "config.rs")], "`EXPR` (300 s)").unwrap_err();
        assert!(
            error.contains(
                "adapters/b/src/config.rs gives no whole seconds or milliseconds in digits for EXPR"
            ),
            "{error}"
        );
        let error = section(&[("BOTH", "twice.rs")], "`BOTH` (1 s)").unwrap_err();
        assert!(
            error.contains("adapters/b/src/twice.rs defines more than once BOTH"),
            "{error}"
        );
    }

    /// Per note, by its binding, a phrase of the `CHANGELOG.md` entry that brought it.
    const NOTED: &[(&str, &str)] = &[
        (
            "OpenAI GPT-Live",
            "`OpenAiLive::with_close_timeout` and `OpenAiLive::close_timeout`",
        ),
        (
            "ElevenLabs streaming TTS",
            "`inference-elevenlabs`, `inference-cartesia`: dropping a synthesis inside a Tokio runtime",
        ),
        (
            "ElevenLabs agent conversation",
            "`agent::AgentError::SignedUrlSpent` (`SessionFailureKind::Rejected`)",
        ),
        (
            "Cartesia Sonic streaming TTS",
            "`inference-elevenlabs`, `inference-cartesia`: dropping a synthesis inside a Tokio runtime",
        ),
    ];

    #[test]
    fn every_note_is_of_the_release_whose_changelog_section_brought_it() {
        let changelog = fs::read_to_string(crate::repository_root().join("CHANGELOG.md")).unwrap();
        let mut notes = Vec::new();
        let mut wrong = Vec::new();
        for note in NOTES {
            match NOTED.iter().find(|(name, _)| *name == note.binding) {
                Some((_, phrase)) => notes.push((note.binding, note.release, *phrase)),
                None => wrong.push(format!("{}: no phrase of its entry", note.binding)),
            }
        }
        wrong.extend(introduction_problems(&changelog, &notes, false));
        for (name, _) in NOTED {
            if !NOTES.iter().any(|note| note.binding == *name) {
                wrong.push(format!("{name}: no such note"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn a_note_may_be_brought_by_any_list_of_its_section() {
        let changelog = "# Changelog\n\n## [Unreleased]\n\n### Fixed\n\n- `fresh`: a drop\n  \
                         waits less.\n\n## [0.1.0] — 2026-01-01\n\n### Added\n\n- The \
                         contracts.\n";
        let phrase = "`fresh`: a drop waits less";
        assert_eq!(
            introduction_problems(changelog, &[("N", Release::Next, phrase)], false),
            Vec::<String>::new()
        );
        let wrong = introduction_problems(changelog, &[("B", Release::Next, phrase)], true);
        assert!(
            wrong.len() == 1 && wrong[0].contains("outside its Added list"),
            "{wrong:?}"
        );
    }

    #[test]
    fn numbers_are_grouped_by_thousands() {
        assert_eq!(grouped(7), "7");
        assert_eq!(grouped(4937), "4,937");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }
}
