//! What exists and what does not: `website/data/status.json` (`b10x-status/1`), which the landing's
//! status section and the status page both read, and `website/docs/status.mdx`, from
//! [`CAPABILITIES`].
//!
//! A shipped item names the release it shipped in ("Since 0.x.0") and the test that holds it
//! (`path::function`), or, for a live acceptance, the live tests that call the service. An item on
//! `main` and in no release yet is [`NEXT`]: it names its test and no release, stays out of the
//! status document, whose `shipped` reads "runs today", and gets its own section on the status
//! page. Generation refuses an item whose test does not exist, is not a test or is ignored, a live
//! test that is not ignored, and a planned item that names a test.

use std::fs;
use std::path::Path;

use serde_json::json;

use crate::{Result, SOURCE, live_test_defined, test_defined};

/// Where the status document lands, relative to the repository root.
pub const DATA: &str = "website/data/status.json";
/// Where the status page lands, relative to the repository root.
pub const PAGE: &str = "website/docs/status.mdx";

/// The date the statuses below were last checked against `main`.
const AS_OF: &str = "2026-10-23";

/// The status of an item that is on `main` and in no release yet. `b10x-status/1` has no word for
/// it, so it never reaches the status document.
const NEXT: &str = "next";

/// The heading of the status page's section for [`NEXT`] items.
const NEXT_HEADING: &str = "On main, in the next release";

/// One line of the status table.
pub struct Capability {
    area: &'static str,
    label: &'static str,
    detail: &'static str,
    status: &'static str,
    /// The test that holds a shipped or next item, `path/to/file.rs::function`; empty for a
    /// planned one and for a live acceptance.
    evidence: &'static str,
    /// The live tests behind a live acceptance, each `path::function`: ignored by default because
    /// they call the service.
    live: &'static [&'static str],
    href: &'static str,
}

const fn shipped(
    area: &'static str,
    label: &'static str,
    detail: &'static str,
    evidence: &'static str,
    href: &'static str,
) -> Capability {
    Capability {
        area,
        label,
        detail,
        status: "shipped",
        evidence,
        live: &[],
        href,
    }
}

/// A shipped item held by live tests that call the service, not by a test of every check.
const fn live(
    area: &'static str,
    label: &'static str,
    detail: &'static str,
    tests: &'static [&'static str],
    href: &'static str,
) -> Capability {
    Capability {
        area,
        label,
        detail,
        status: "shipped",
        evidence: "",
        live: tests,
        href,
    }
}

/// An item on `main`, held by a test, and in no release yet. Unused from a release until the next
/// item lands on `main`.
#[allow(dead_code)]
const fn next(
    area: &'static str,
    label: &'static str,
    detail: &'static str,
    evidence: &'static str,
    href: &'static str,
) -> Capability {
    Capability {
        area,
        label,
        detail,
        status: NEXT,
        evidence,
        live: &[],
        href,
    }
}

const fn planned(
    area: &'static str,
    label: &'static str,
    detail: &'static str,
    href: &'static str,
) -> Capability {
    Capability {
        area,
        label,
        detail,
        status: "planned",
        evidence: "",
        live: &[],
        href,
    }
}

/// Every line of the status table, in the order the page shows them.
pub const CAPABILITIES: &[Capability] = &[
    shipped(
        "Contracts",
        "STT, LLM and TTS contracts",
        "Since 0.4.0: one contract per model kind, each with a scripted mock; a backend claims capabilities such as output streaming, and the backend-neutral checks hold every configuration to exactly what it was observed to do.",
        "crates/inference-testkit/tests/stt.rs::each_configuration_claims_exactly_what_it_was_observed_to_do",
        "/docs/concepts/contracts",
    ),
    shipped(
        "Contracts",
        "Tool calls",
        "Since 0.4.0: propose, authorize, reject, start, complete and fail, each from its source states only; the executor contract and the provisional-input barrier.",
        "crates/inference-tools/tests/lifecycle.rs::each_transition_moves_from_its_source_states_only",
        "/docs/concepts/contracts",
    ),
    shipped(
        "Contracts",
        "Native speech contract",
        "Since 0.5.0: one multiport node for a model that listens and speaks, its session ledger and a mock; a barge-in cancels the response, drops its late audio and truncates at the heard sample.",
        "crates/inference-duplex/tests/ledger.rs::barge_in_cancels_the_response_drops_its_late_audio_and_truncates_at_the_heard_sample",
        "/docs/concepts/contracts",
    ),
    shipped(
        "Contracts",
        "Conformance suite",
        "Since 0.5.0: the testkit target answers every scenario of the suite synthesized from the specification: 454 at 0.5.0, 460 at 0.6.0 and 0.7.0, and 462 on main.",
        "crates/inference-testkit/tests/conform.rs::the_suite_runs_against_the_testkit_target",
        "/docs/guides/run-the-checks",
    ),
    shipped(
        "Graph and runtime",
        "Pipelines checked before they run",
        "Since 0.4.0: a pipeline is declared through commands or the typed builder, validated, and planned from its nodes' capabilities; a node bound to a model without a capability it requires is refused, naming both.",
        "crates/inference-pipeline/tests/planning.rs::a_node_requiring_tool_calling_bound_to_an_adapter_without_it_is_refused_naming_both",
        "/docs/concepts/graphs",
    ),
    shipped(
        "Graph and runtime",
        "Declarative definitions and subgraphs",
        "Since 0.5.0: a graph definition is validated against the runtime's node descriptors; a subgraph runs as one node of its parent and keeps its own cancellation and tracing boundary.",
        "crates/inference-pipeline/tests/definition.rs::a_definition_declares_what_the_builder_declares_and_validates_and_plans",
        "/docs/concepts/graphs",
    ),
    shipped(
        "Graph and runtime",
        "Bounded edges and overload policies",
        "Since 0.4.0: every edge has a capacity and one of seven overload policies; pause holds the producer until the consumer takes an event.",
        "crates/inference-runtime/tests/bounded_edges.rs::pause_holds_the_producer_until_the_consumer_takes_one",
        "/docs/concepts/runtime",
    ),
    shipped(
        "Graph and runtime",
        "Run lifecycle and controller",
        "Since 0.4.0: a run moves through its states under the controller, which prepares, starts, drains and stops node instances.",
        "crates/inference-runtime/tests/run_lifecycle.rs::a_run_built_from_the_planned_form_reaches_completed_through_the_six_states",
        "/docs/concepts/runtime",
    ),
    shipped(
        "Graph and runtime",
        "Bounded tool feedback loop",
        "Since 0.5.0: a tool result goes back to the LLM node for the next turn of the same generation, up to an iteration bound; past it the proposal is rejected and the generation completes.",
        "crates/inference-runtime/tests/tool_feedback_loop.rs::the_proposal_past_the_iteration_bound_is_rejected_exhausted_and_the_generation_completes",
        "/docs/concepts/runtime",
    ),
    shipped(
        "Graph and runtime",
        "Record and replay",
        "Since 0.6.0: a recorder and a replay driver reproduce a run's control decisions, such as a barge-in and a tool call, in order.",
        "crates/inference-runtime/tests/control_event_replay.rs::replaying_a_recorded_barge_in_and_tool_call_reproduces_its_control_decisions_in_order",
        "/docs/concepts/runtime",
    ),
    shipped(
        "Offline and privacy",
        "offline_strict profile",
        "Since 0.6.0: planning refuses an adapter whose locality the deployment profile does not admit, inside subgraphs too, before anything opens.",
        "crates/inference-runtime/tests/offline_profile.rs::planning_refuses_an_external_provider_under_offline_strict_and_the_pipeline_stays_validated",
        "/docs/concepts/deployment-profiles",
    ),
    shipped(
        "Offline and privacy",
        "Egress watch",
        "Since 0.6.0: on Linux, an offline_strict run fails on any connection or unconnected datagram socket that is not loopback, naming the address. A tripwire, not a sandbox.",
        "crates/inference-runtime/tests/offline_profile.rs::a_node_connecting_to_test_net_1_fails_the_run_naming_the_address",
        "/docs/concepts/deployment-profiles",
    ),
    shipped(
        "Session",
        "Session facade",
        "Since 0.5.0: an application opens one session, sends, receives, commits input, interrupts and closes it, with one event vocabulary for text and voice.",
        "crates/inference-facade/tests/session.rs::open_session_opens_an_opened_session_with_no_run_over_a_planned_pipeline_of_the_mode",
        "/docs/concepts/session",
    ),
    shipped(
        "Session",
        "Topology switch by configuration",
        "Since 0.5.0: one session switches between the text-only pipeline and the voice cascade and keeps one history.",
        "crates/inference-facade/tests/topology_switch.rs::one_session_switches_text_to_voice_and_back_with_one_vocabulary_and_one_history",
        "/docs/concepts/session",
    ),
    shipped(
        "Adapters",
        "OpenAI Realtime",
        "Since 0.5.0: a native speech model behind feature duplex; SessionConfig::speed sets the output speed since 0.7.0.",
        "adapters/inference-openai/tests/duplex.rs::audio_in_gives_audio_and_transcript_out_on_one_response_id",
        "/docs/reference/adapters",
    ),
    shipped(
        "Adapters",
        "OpenAI GPT-Live",
        "Since 0.6.0: a native speech model behind features duplex and gpt-live, observed as native duplex by the conformance probe.",
        "adapters/inference-openai/tests/gpt_live_duplex.rs::the_declared_capabilities_are_what_gpt_live_takes",
        "/docs/reference/adapters",
    ),
    shipped(
        "Adapters",
        "Amazon Nova Sonic",
        "Since 0.5.0: a native speech model on Bedrock behind feature duplex; service errors are shown with ARNs and account ids redacted.",
        "adapters/inference-aws/tests/nova_sonic_duplex.rs::audio_in_gives_audio_and_transcript_out_on_one_response_id",
        "/docs/reference/adapters",
    ),
    live(
        "Adapters",
        "Nova Sonic live acceptance",
        "Since 0.7.0: on 2026-10-21 the live tests passed against Bedrock with the adapter as released in 0.7.0: one audio-in, audio-out exchange with the client, one session through the native speech contract, and the native duplex probe, which agrees with the adapter not claiming native_duplex. They need AWS access and are ignored by default.",
        &[
            "adapters/inference-aws/tests/nova_sonic_live.rs::one_audio_in_audio_out_exchange",
            "adapters/inference-aws/tests/nova_sonic_duplex_live.rs::one_audio_in_audio_out_session_through_the_contract",
            "adapters/inference-aws/tests/nova_sonic_duplex_live.rs::native_duplex_evidence_agrees_with_the_claim",
        ],
        "/docs/guides/talk-to-a-hosted-voice-model",
    ),
    shipped(
        "Adapters",
        "sherpa-onnx streaming STT",
        "Since 0.6.0: a local streaming recogniser behind feature stt, linked without a GPL phonemiser; its capability record names the tests that evidence each claim.",
        "adapters/inference-sherpa-onnx/tests/stt_record.rs::the_record_of_the_reference_machine_loads_and_evidences_exactly_the_claims",
        "/docs/reference/adapters",
    ),
    shipped(
        "Adapters",
        "whisper.cpp batch STT",
        "Since 0.6.0: a local batch recogniser on the CPU, with no streaming claims.",
        "adapters/inference-whispercpp/tests/stt_record.rs::the_record_of_the_reference_machine_loads_and_names_no_evidence_for_no_claim",
        "/docs/reference/adapters",
    ),
    shipped(
        "Adapters",
        "llama-server LLM",
        "Since 0.6.0: a local llama.cpp server as an output-streaming LLM, with cancellation, deadlines and tool proposals behind feature tools.",
        "adapters/inference-llama-server/tests/offline.rs::a_delta_reaches_the_caller_while_the_server_still_holds_the_rest_of_the_stream",
        "/docs/reference/adapters",
    ),
    shipped(
        "Adapters",
        "Microphone and speaker",
        "Since 0.5.0: capture and playout over cpal; since 0.6.0 the speaker acknowledges playback and stops at a cancel.",
        "adapters/inference-audio-device/tests/nodes.rs::the_sink_plays_the_frames_it_receives_and_a_cancel_event_stops_the_speaker_at_once",
        "/docs/examples",
    ),
    planned(
        "Adapters",
        "Local text-to-speech",
        "The backend-neutral TTS checks exist; no local TTS adapter does, so the offline cascade speaks through a mock.",
        "/docs/status/limitations",
    ),
    planned(
        "Adapters",
        "Self-hosted native speech model",
        "A native speech model that runs on your own machine.",
        "/docs/status/limitations",
    ),
    shipped(
        "Voice",
        "Local voice cascade, offline",
        "Since 0.6.0: STT, LLM and TTS through commit gates under offline_strict; only committed text is spoken, and the speaker confirms what played.",
        "examples/local-voice-cascade/tests/gates.rs::the_cascade_speaks_committed_answers_through_both_gates_and_plays_them_through_receipts",
        "/docs/guides/run-the-local-voice-cascade-offline",
    ),
    shipped(
        "Voice",
        "Barge-in",
        "Since 0.6.0: a second utterance supersedes the answer, stops the speaker and drops its late frames.",
        "examples/local-voice-cascade/tests/barge_in.rs::a_second_utterance_supersedes_the_first_answer_stops_the_speaker_and_drops_its_late_frames",
        "/docs/concepts/streams",
    ),
    shipped(
        "Voice",
        "Recogniser by configuration",
        "Since 0.7.0: INFERENCE_CASCADE_STT, or talk --stt, selects sherpa-onnx or whisper.cpp; the same cascade tests pass under both.",
        "examples/local-voice-cascade/tests/local_adapters.rs::the_stt_variable_selects_the_recogniser_and_the_variable_of_its_checkpoint",
        "/docs/guides/swap-the-recogniser-by-configuration",
    ),
    shipped(
        "Voice",
        "Hosted voice by provider",
        "Since 0.5.0: talk --provider selects openai or nova, and openai-live since 0.6.0, over the same conversation code.",
        "examples/realtime-voice/tests/wiring.rs::the_demo_sessions_are_what_each_provider_takes",
        "/docs/guides/talk-to-a-hosted-voice-model",
    ),
    shipped(
        "Voice",
        "Provider comparison",
        "Since 0.6.0: the compare example measures latency and usage across providers and prints one table.",
        "examples/realtime-voice/tests/compare.rs::two_scripted_providers_give_their_figures_their_order_and_the_table",
        "/docs/guides/compare-voice-providers",
    ),
    shipped(
        "Voice",
        "Repository podcast",
        "Since 0.7.0: two voice models discuss a repository over the speaker, with running token and cost counters.",
        "examples/repo-podcast/tests/show.rs::the_hosts_take_turns_the_host_opening",
        "/docs/examples/repo-podcast",
    ),
    shipped(
        "Usage and cost",
        "LLM usage per call",
        "Since 0.6.0: one usage record per LLM turn, after its events; a count the provider leaves out is unknown, never 0.",
        "crates/inference-llm/tests/model_turns.rs::each_turn_that_ended_yields_one_usage_record_after_its_events",
        "/docs/concepts/usage",
    ),
    shipped(
        "Usage and cost",
        "STT and TTS usage per call",
        "Since 0.7.0: a recognition ends with its input audio and one request, a synthesis with its characters and output audio, and local compute where the adapter measures it.",
        "crates/inference-stt/tests/usage.rs::a_recognition_of_one_and_a_half_seconds_records_its_audio_and_one_request",
        "/docs/concepts/usage",
    ),
    shipped(
        "Usage and cost",
        "Native speech usage per response",
        "Since 0.7.0: one usage record per response and per reported transcription; usage no response owns is kept as a session record.",
        "crates/inference-duplex/tests/ledger.rs::usage_per_response_and_of_transcription_is_recorded_for_a_meter",
        "/docs/concepts/usage",
    ),
    shipped(
        "Usage and cost",
        "Run and session usage totals",
        "Since 0.8.0: the facade delivers each usage record as an event and totals a run and a session kind by kind.",
        "crates/inference-facade/tests/usage.rs::a_spoken_turn_delivers_one_usage_record_per_model_call_and_totals_them_kind_by_kind",
        "/docs/concepts/usage",
    ),
    shipped(
        "Usage and cost",
        "Cost records from a price document",
        "Since 0.8.0: behind feature cost, the facade prices each usage record from a price document the caller supplies; no prices are built in.",
        "crates/inference-facade/tests/cost.rs::a_voice_cascade_priced_for_its_llm_only_has_one_cost_record_per_usage_record",
        "/docs/guides/estimate-cost-from-a-price-table",
    ),
    shipped(
        "Usage and cost",
        "Usage of a cancelled generation",
        "Since 0.8.0: a cancelled or superseded LLM turn still yields its usage record.",
        "crates/inference-testkit/tests/usage_on_cancel.rs::a_cancel_during_an_llm_turn_still_yields_the_turns_usage_record",
        "/docs/concepts/usage",
    ),
    shipped(
        "Usage and cost",
        "Subgraph usage roll-up",
        "Since 0.8.0: an LLM inside a subgraph is counted once in the parent run's and the session's totals.",
        "crates/inference-facade/tests/usage_subgraph.rs::an_llm_inside_a_subgraph_node_is_counted_once_in_the_parent_run_and_session_totals",
        "/docs/concepts/usage",
    ),
    planned(
        "Research and scale-out",
        "Orchestrated and native duplex text",
        "Duplex text, with orchestrated and native duplex reported as different capabilities.",
        "",
    ),
    planned(
        "Research and scale-out",
        "Trace export with consent",
        "Exporting traces of a run only with the user's consent.",
        "",
    ),
    planned(
        "Research and scale-out",
        "Worker processes, quotas and egress audit",
        "A worker process protocol for hosted providers, quotas and an egress audit.",
        "",
    ),
    planned(
        "SDK",
        "One inference crate",
        "One crate that re-exports the libraries by feature, with one error type.",
        "",
    ),
    planned(
        "SDK",
        "One-call transcribe, chat, speak and talk",
        "Transcribe a file, chat, speak or hold a conversation in one call.",
        "",
    ),
    planned(
        "SDK",
        "Stacks",
        "Bind each role (STT, LLM, TTS, native speech) to an adapter from code, the environment or a file.",
        "",
    ),
    planned(
        "SDK",
        "Named recipes",
        "Ready-made pipelines chosen by name.",
        "",
    ),
    planned(
        "SDK",
        "Tool attribute",
        "Declare a tool by putting an attribute on an async function.",
        "",
    ),
    planned(
        "SDK",
        "Native speech model as a graph node",
        "Place a native speech model in a graph like any other node.",
        "",
    ),
    planned(
        "SDK",
        "Scripted testing",
        "Test an application against scripted models without writing the mocks by hand.",
        "",
    ),
];

/// Refuse a capability list whose shipped items do not say when they shipped, or do not name a test
/// that exists, is a test and is not ignored, or live tests that exist and are ignored; whose next
/// items name a release or no such test; whose planned items name a test; or whose labels repeat.
fn checked(root: &Path, capabilities: &[Capability]) -> Result<()> {
    let mut labels = Vec::new();
    for capability in capabilities {
        let label = capability.label;
        if labels.contains(&label) {
            return Err(format!("status item {label:?} is listed twice"));
        }
        labels.push(label);
        match capability.status {
            "shipped" => {
                if !capability.detail.starts_with("Since 0.") {
                    return Err(format!(
                        "shipped status item {label:?} says no \"Since 0.x.0\"; an item in no release yet is next"
                    ));
                }
                if capability.live.is_empty() {
                    held(root, label, capability.evidence, test_defined)?;
                }
                for test in capability.live {
                    held(root, label, test, live_test_defined)?;
                }
            }
            NEXT => {
                if capability.detail.starts_with("Since ") {
                    return Err(format!(
                        "status item {label:?} is on main for the next release but names a release"
                    ));
                }
                held(root, label, capability.evidence, test_defined)?;
            }
            "planned" if capability.evidence.is_empty() && capability.live.is_empty() => {}
            "planned" => {
                return Err(format!("planned status item {label:?} names a test"));
            }
            other => return Err(format!("status item {label:?} has status {other}")),
        }
    }
    Ok(())
}

/// Refuse `evidence` unless it names, as `path::function`, a function under `root` that `defined`
/// accepts.
fn held(
    root: &Path,
    label: &str,
    evidence: &str,
    defined: fn(&str, &str) -> std::result::Result<(), &'static str>,
) -> Result<()> {
    let (file, function) = evidence
        .split_once("::")
        .ok_or_else(|| format!("status item {label:?} names no test as path::function"))?;
    let source = fs::read_to_string(root.join(file))
        .map_err(|error| format!("status item {label:?}: reading {file}: {error}"))?;
    defined(&source, function).map_err(|problem| {
        format!("status item {label:?}: {file}::{function} is {problem}")
            .replace("is no such function", "not defined")
    })
}

/// A Markdown link to the test `path::function` in the source tree.
fn test_link(evidence: &str) -> String {
    let (file, function) = evidence.split_once("::").unwrap_or_default();
    format!("[`{function}`]({SOURCE}/{file})")
}

/// The `b10x-status/1` document for the repository at `root`: the shipped and planned items, not
/// the ones on `main` for the next release.
pub fn data(root: &Path) -> Result<String> {
    checked(root, CAPABILITIES)?;
    let items: Vec<_> = CAPABILITIES
        .iter()
        .filter(|capability| capability.status != NEXT)
        .map(|capability| {
            let mut item = json!({
                "label": capability.label,
                "status": capability.status,
                "area": capability.area,
                "detail": capability.detail,
            });
            if !capability.href.is_empty() {
                item["href"] = capability.href.into();
            }
            item
        })
        .collect();
    let document = json!({
        "format": "b10x-status/1",
        "asOf": AS_OF,
        "source": "crates/inference-docs/src/status.rs; every shipped item names its release and the test that holds it",
        "items": items,
    });
    serde_json::to_string_pretty(&document)
        .map(|text| text + "\n")
        .map_err(|error| format!("serializing the status document: {error}"))
}

/// The status page, for the workspace at `version`.
pub fn page(root: &Path, version: &str) -> Result<String> {
    checked(root, CAPABILITIES)?;
    let count = |status: &str| {
        CAPABILITIES
            .iter()
            .filter(|capability| capability.status == status)
            .count()
    };
    let (on_main, planned) = (count(NEXT), count("planned"));
    let mut next = String::new();
    for capability in CAPABILITIES.iter().filter(|c| c.status == NEXT) {
        let label = if capability.href.is_empty() {
            capability.label.to_owned()
        } else {
            format!("[{}]({})", capability.label, capability.href)
        };
        next.push_str(&format!(
            "- **{label}** ({}). {} Test: {}.\n",
            capability.area,
            capability.detail,
            test_link(capability.evidence)
        ));
    }
    if next.is_empty() {
        next = format!("Nothing yet: everything on `main` is in `{version}`.\n");
    }
    let mut evidence = String::new();
    for capability in CAPABILITIES.iter().filter(|c| c.status != "planned") {
        let tests = if capability.live.is_empty() {
            test_link(capability.evidence)
        } else {
            let links: Vec<_> = capability.live.iter().map(|test| test_link(test)).collect();
            format!("live: {}", links.join(", "))
        };
        evidence.push_str(&format!("| {} | {tests} |\n", capability.label));
    }
    Ok(format!(
        "---\n{header}\ntitle: \"Status\"\nsidebar_position: 3\ndescription: \"What inference implements today, what is on main for the next release, and what is planned.\"\nlede: \"Every capability below carries its status; the landing page's status section reads the same file, so the two cannot disagree.\"\nsource: \"crates/inference-docs/src/status.rs, generated into website/data/status.json\"\nsource_url: \"{SOURCE}/website/data/status.json\"\ncustom_edit_url: null\n---\n\n\
         import status from '@site/data/status.json';\n\n\
         # Status\n\n\
         The current release is `{version}`, a source release at that Git tag; nothing is on a \
         registry. The [changelog]({SOURCE}/CHANGELOG.md) records every change a consumer can see. \
         **Shipped** means in a release, with a test on `main` that holds it: an item that says \
         \"Since\" is in that release and every one after it. **Planned** means not built. What is \
         on `main` and in no release yet is not counted as shipped; it is listed under \
         [{NEXT_HEADING}](#on-main-in-the-next-release). Checked on {AS_OF}: {shipped} shipped, \
         {planned} planned, and {on_main} on main for the next release.\n\n\
         <StatusTable data={{status}} />\n\n\
         ## {NEXT_HEADING}\n\n\
         Built and tested on `main`, and in no release yet: a dependency on the `{version}` tag \
         does not have these.\n\n\
         {next}\n\
         ## The test behind each item\n\n\
         The generator refuses an item whose test is missing or ignored. A live acceptance is held \
         by live tests instead: they call the provider, need its credentials, and are ignored by \
         default, so they run only when asked for.\n\n\
         | Item | Test |\n|---|---|\n{evidence}\n\
         What the tests do not establish is in [Limitations](./status/limitations.md).\n",
        header = crate::FRONT_MATTER_HEADER,
        shipped = count("shipped"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_capability_of_this_tree_passes_its_checks() {
        checked(&crate::repository_root(), CAPABILITIES).unwrap();
    }

    #[test]
    fn a_shipped_item_with_an_ignored_or_missing_test_or_no_release_is_refused() {
        let root = crate::scratch("status");
        fs::write(
            root.join("t.rs"),
            "#[test]\nfn held() {}\n\n#[test]\n#[ignore = \"needs a key\"]\nfn live() {}\n",
        )
        .unwrap();
        let item = |detail, evidence| [shipped("A", "L", detail, evidence, "/docs/")];
        assert!(checked(&root, &item("Since 0.1.0: x", "t.rs::held")).is_ok());
        let error = checked(
            &root,
            &item("On main, in the next release: x", "t.rs::held"),
        )
        .unwrap_err();
        assert!(error.contains("says no \"Since 0.x.0\""), "{error}");
        let error = checked(&root, &item("Since 0.1.0: x", "t.rs::live")).unwrap_err();
        assert!(error.contains("t.rs::live is an ignored test"), "{error}");
        let error = checked(&root, &item("Since 0.1.0: x", "t.rs::gone")).unwrap_err();
        assert!(error.contains("t.rs::gone not defined"), "{error}");
        let error = checked(&root, &item("Works: x", "t.rs::held")).unwrap_err();
        assert!(error.contains("says no \"Since 0.x.0\""), "{error}");
        let error = checked(
            &root,
            &[Capability {
                evidence: "t.rs::held",
                ..planned("A", "P", "x", "/docs/")
            }],
        )
        .unwrap_err();
        assert!(error.contains("names a test"), "{error}");
        let twice = [planned("A", "P", "x", "/"), planned("B", "P", "y", "/")];
        assert!(checked(&root, &twice).unwrap_err().contains("listed twice"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_item_on_main_names_a_held_test_and_no_release() {
        let root = crate::scratch("status-next");
        fs::write(
            root.join("t.rs"),
            "#[test]\nfn held() {}\n\n#[test]\n#[ignore = \"needs a key\"]\nfn live() {}\n",
        )
        .unwrap();
        let item = |detail, evidence| [next("A", "N", detail, evidence, "/docs/")];
        assert!(checked(&root, &item("Totals per run.", "t.rs::held")).is_ok());
        let error = checked(&root, &item("Since 0.1.0: x", "t.rs::held")).unwrap_err();
        assert!(error.contains("names a release"), "{error}");
        let error = checked(&root, &item("Totals per run.", "t.rs::live")).unwrap_err();
        assert!(error.contains("t.rs::live is an ignored test"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_live_item_names_only_ignored_tests_that_exist() {
        let root = crate::scratch("status-live");
        fs::write(
            root.join("t.rs"),
            "#[test]\nfn held() {}\n\n#[test]\n#[ignore = \"needs a key\"]\nfn live() {}\n",
        )
        .unwrap();
        let item = |tests| [live("A", "L", "Since 0.1.0: x", tests, "/docs/")];
        assert!(checked(&root, &item(&["t.rs::live"])).is_ok());
        let error = checked(&root, &item(&["t.rs::held"])).unwrap_err();
        assert!(error.contains("t.rs::held is not ignored"), "{error}");
        let error = checked(&root, &item(&["t.rs::gone"])).unwrap_err();
        assert!(error.contains("t.rs::gone not defined"), "{error}");
        let error = checked(&root, &item(&[])).unwrap_err();
        assert!(error.contains("names no test"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn items_on_main_stay_out_of_the_status_document_and_get_their_own_section() {
        let root = crate::repository_root();
        let document: serde_json::Value = serde_json::from_str(&data(&root).unwrap()).unwrap();
        let items = document["items"].as_array().unwrap();
        let on_main: Vec<_> = CAPABILITIES.iter().filter(|c| c.status == NEXT).collect();
        assert_eq!(items.len(), CAPABILITIES.len() - on_main.len());
        for item in items {
            let status = item["status"].as_str().unwrap();
            assert!(status == "shipped" || status == "planned", "{item}");
            let detail = item["detail"].as_str().unwrap();
            assert!(
                status != "shipped" || detail.starts_with("Since 0."),
                "{item}"
            );
        }
        let page = page(&root, "0.0.0").unwrap();
        let (before, section) = page
            .split_once("\n## On main, in the next release\n")
            .unwrap();
        for capability in on_main {
            assert!(!before.contains(capability.detail), "{}", capability.label);
            assert!(section.contains(capability.label), "{}", capability.label);
            assert!(section.contains(capability.detail), "{}", capability.label);
        }
    }
}
