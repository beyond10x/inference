//! Security review, resource release: every recognition, finished, cancelled, dropped or failed,
//! ends the thread that drove its connection. The count is read from the process's own task
//! list, so this file holds this one test and nothing runs beside it.

mod support;

use std::time::{Duration, Instant};

use inference_stt::{Recogniser, RecognitionHandle};
use support::{FixtureServer, Script, recognise, request, tone};

fn driving_threads() -> usize {
    std::fs::read_dir("/proc/self/task")
        .expect("the task list")
        .filter_map(Result::ok)
        .filter(|task| {
            std::fs::read_to_string(task.path().join("comm"))
                .is_ok_and(|name| name.trim_end() == "inference-deepg")
        })
        .count()
}

fn settled(bound: Duration) -> usize {
    let deadline = Instant::now() + bound;
    loop {
        let count = driving_threads();
        if count == 0 || Instant::now() >= deadline {
            return count;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[cfg(target_os = "linux")]
fn no_recognition_leaves_its_thread_behind() {
    assert_eq!(driving_threads(), 0);
    let server = FixtureServer::scripted(vec![
        Script::fixture("conversation.jsonl"),
        Script::fixture("conversation.jsonl"),
        Script::fixture("conversation.jsonl"),
        Script::closes_after_first(1011, "NET-0000"),
    ]);
    let mut recogniser = server.recogniser();
    let mut peak = 0;
    for round in 0..24 {
        match round % 4 {
            0 => {
                let _ = recognise(&mut recogniser, tone(4));
            }
            1 | 2 => {
                let mut handle = recogniser.start(request()).unwrap();
                handle.offer_frame(tone(1).remove(0)).unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                while handle.next().is_none() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                peak = peak.max(driving_threads());
                if round % 4 == 1 {
                    handle.cancel();
                }
                drop(handle);
            }
            _ => {
                let (handle, _) = recognise(&mut recogniser, tone(4));
                assert!(handle.failure().is_some(), "round {round}");
            }
        }
    }
    assert!(
        peak >= 1,
        "the measure sees a driving thread while one runs"
    );
    assert_eq!(
        settled(Duration::from_secs(3)),
        0,
        "threads left behind after 24 recognitions"
    );
}
