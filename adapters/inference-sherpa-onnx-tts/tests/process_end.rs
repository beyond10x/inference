//! What a process does that ends while the worker of a dropped synthesis still generates. No drop
//! of this crate joins that worker, so a program can return from `main` with the thread inside
//! sherpa-onnx. The case runs that program as a child process, which is this test binary run on
//! this one case again with [`CHILD`] set, and reads how the child ended. Ignored: it needs the
//! checkpoint directory `INFERENCE_SHERPA_ONNX_TTS_DIR` and a process started on the sherpa-onnx
//! library built with text-to-speech.

use std::process::Command;
use std::task::{Context, Waker};
use std::time::{Duration, Instant};

use inference_sherpa_onnx_tts::inference_tts::inference_core::generation::{
    Generation, StartGeneration,
};
use inference_sherpa_onnx_tts::inference_tts::inference_core::id::{NodeId, RunId};
use inference_sherpa_onnx_tts::inference_tts::inference_text::OutputSegment;
use inference_sherpa_onnx_tts::inference_tts::{SynthesisHandle, Synthesiser};

use crate::support::open;

/// Set in the child process: it speaks, drops and returns instead of starting a child.
const CHILD: &str = "INFERENCE_SHERPA_ONNX_TTS_TEST_ENDS_MID_SENTENCE";

/// What the child writes once it dropped its handles with the sentence still being generated.
const DROPPED: &str = "dropped mid-sentence after";

/// The name of the case below as the test binary lists it.
const CASE: &str =
    "process_end::a_process_that_ends_while_a_dropped_synthesis_generates_exits_with_status_0";

/// One sentence, long enough that the engine needs more than a second for it on a CPU.
const SENTENCE: &str = "When the evening train had left the valley and the lamps along the river were lit one \
     after another, the keeper of the bridge walked home slowly through the quiet streets.";

/// The child: offers [`SENTENCE`], sees 300 ms later that nothing of it is handed over yet, drops
/// the synthesis and the synthesiser and returns, so the process ends from the end of `main`.
fn speak_drop_and_return() {
    let started = Instant::now();
    let mut synthesiser = open(1);
    let mut synthesis = synthesiser.start().expect("one synthesis runs");
    let (generation, _) = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    });
    let (tentative, _) = OutputSegment::emit(&generation, 1, SENTENCE.to_owned())
        .expect("a fresh generation is Active");
    let (segment, _) = tentative.commit().expect("a Tentative segment commits");
    synthesis
        .offer_segment(segment)
        .expect("a Committed segment is queued");
    let mut cx = Context::from_waker(Waker::noop());
    assert!(synthesis.poll_event(&mut cx).is_pending());
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        synthesis.poll_event(&mut cx).is_pending(),
        "the sentence is still being generated 300 ms after it was offered"
    );
    drop(synthesis);
    drop(synthesiser);
    eprintln!("{DROPPED} {} ms", started.elapsed().as_millis());
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn a_process_that_ends_while_a_dropped_synthesis_generates_exits_with_status_0() {
    if std::env::var_os(CHILD).is_some() {
        speak_drop_and_return();
        return;
    }
    let started = Instant::now();
    let output = Command::new(std::env::current_exe().expect("the test binary has a path"))
        .args(["--exact", CASE, "--ignored", "--nocapture"])
        .env(CHILD, "1")
        .output()
        .expect("the test binary starts again");
    let lasted = started.elapsed();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let dropped = stderr.lines().find(|line| line.contains(DROPPED));
    eprintln!(
        "the child ended with {:?} after {} ms; it wrote {dropped:?}",
        output.status,
        lasted.as_millis()
    );
    assert!(
        dropped.is_some(),
        "the child dropped its handles mid-sentence: {stderr}"
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "a process that returns from `main` while the worker of a dropped synthesis still \
         generates ends with status 0: {stderr}"
    );
}
