//! The show on the duplex mock and the fake audio host: turn order, the hand-over of each
//! host's transcript as text, playout one host at a time, the time limit and its closing turn.

mod support;

use std::time::Duration;

use inference_duplex::TurnDetection;
use inference_duplex::inference_audio::AudioFrame;
use inference_testkit::duplex::mock::{frame, pcm};
use repo_podcast::{Directive, Event, default_hosts, level};
use support::{BRIEF, Scripted};

/// The mock speaks 40 ms of audio a turn: two frames of 20 ms.
const TURN_AUDIO: Duration = Duration::from_millis(40);

/// A limit of five turns of the mock's audio.
const LIMIT: Duration = Duration::from_millis(200);

fn generating(events: &[Event]) -> Vec<(usize, usize, Directive)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Generating {
                host,
                turn,
                directive,
            } => Some((*host, *turn, *directive)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_hosts_take_turns_the_host_opening() {
    let (mut show, host) = support::open(Scripted::alternating(12), LIMIT);
    let events = support::run(&mut show, &host);
    let turns = generating(&events);
    assert!(turns.len() >= 4, "too few turns: {turns:?}");
    for (at, (speaker, turn, _)) in turns.iter().enumerate() {
        assert_eq!(*turn, at, "turns are numbered in order: {turns:?}");
        assert_eq!(*speaker, at % 2, "the hosts alternate: {turns:?}");
    }
    assert_eq!(turns[0].2, Directive::Open);
    let spoken: Vec<(usize, String)> = events
        .iter()
        .filter_map(|event| match event {
            Event::Generated {
                host, transcript, ..
            } => Some((*host, transcript.clone())),
            _ => None,
        })
        .collect();
    let hosts = default_hosts();
    for (at, (speaker, transcript)) in spoken.iter().enumerate() {
        assert_eq!(*transcript, format!("{} line {at}", hosts[*speaker].name));
    }
}

#[test]
fn each_host_receives_the_other_hosts_transcript_as_text() {
    let model = Scripted::alternating(12);
    let configs = model.configs.clone();
    let (mut show, host) = support::open(model, LIMIT);
    let events = support::run(&mut show, &host);
    let turns = generating(&events);
    let configs = configs.borrow();
    assert_eq!(configs.len(), turns.len(), "one session per turn");
    let hosts = default_hosts();
    for (at, config) in configs.iter().enumerate() {
        let speaker = &hosts[at % 2];
        assert_eq!(config.voice, speaker.voice, "turn {at} in its host's voice");
        assert!(
            config.instructions.contains(BRIEF),
            "turn {at} has the brief"
        );
        assert!(
            config.instructions.contains(&speaker.name),
            "turn {at} knows who speaks"
        );
        assert_eq!(config.turn_detection, TurnDetection::Disabled);
        assert!(!config.auto_response, "the show asks for each response");
        assert_eq!(
            config.idle_limit,
            Duration::from_secs(10),
            "a stalled turn is given up before it is heard as dead air"
        );
        if at > 0 {
            let other = &hosts[(at + 1) % 2];
            let said = format!("{}: {} line {}", other.name, other.name, at - 1);
            assert!(
                config.instructions.contains(&said),
                "turn {at} is given the other host's last turn as text, {said:?}:\n{}",
                config.instructions
            );
        }
    }
}

#[test]
fn audio_plays_one_host_at_a_time_never_overlapping() {
    let (mut show, host) = support::open(Scripted::alternating(12), LIMIT);
    let events = support::run(&mut show, &host);
    let mut playing: Option<usize> = None;
    let mut played = Vec::new();
    for event in &events {
        match event {
            Event::Playing {
                turn,
                queued,
                host: speaker,
            } => {
                assert_eq!(playing, None, "turn {turn} began while another played");
                assert_eq!(*queued, 0, "turn {turn} began with audio still queued");
                assert_eq!(*speaker, turn % 2);
                playing = Some(*turn);
            }
            Event::Played { turn, audio, .. } => {
                assert_eq!(playing, Some(*turn), "turn {turn} ended without playing");
                assert_eq!(*audio, TURN_AUDIO, "turn {turn} played all its audio");
                played.push(*turn);
                playing = None;
            }
            Event::Tick {
                speaking: Some(speaker),
                ..
            } => {
                let turn = playing.expect("a host speaks only while its turn plays");
                assert_eq!(*speaker, turn % 2);
            }
            _ => {}
        }
    }
    let expected: Vec<usize> = (0..generating(&events).len()).collect();
    assert_eq!(played, expected, "every turn played, in order");
    assert_eq!(show.speaker().queued(), 0);
}

#[test]
fn the_time_limit_ends_the_show_with_a_closing_turn_by_the_host() {
    let (mut show, host) = support::open(Scripted::alternating(12), LIMIT);
    let events = support::run(&mut show, &host);
    let turns = generating(&events);
    let directives: Vec<Directive> = turns.iter().map(|(_, _, directive)| *directive).collect();
    assert_eq!(
        directives,
        vec![
            Directive::Open,
            Directive::Reply,
            Directive::Reply,
            Directive::Reply,
            Directive::Reply,
            Directive::FinalWords,
            Directive::Close,
        ],
        "five turns fill 200 ms; then the guest's last words and the host's close"
    );
    assert_eq!(turns.last().map(|(speaker, ..)| *speaker), Some(0));
    let finished: Vec<Duration> = events
        .iter()
        .filter_map(|event| match event {
            Event::Finished { played } => Some(*played),
            _ => None,
        })
        .collect();
    assert_eq!(finished, vec![TURN_AUDIO * 7]);
    assert!(show.is_over());
    assert!(show.played() >= LIMIT);
}

#[test]
fn a_closing_turn_falls_to_the_host_when_the_guest_spoke_last() {
    let (mut show, host) = support::open(Scripted::alternating(12), Duration::from_millis(160));
    let events = support::run(&mut show, &host);
    let directives: Vec<(usize, Directive)> = generating(&events)
        .into_iter()
        .map(|(speaker, _, directive)| (speaker, directive))
        .collect();
    assert_eq!(
        directives,
        vec![
            (0, Directive::Open),
            (1, Directive::Reply),
            (0, Directive::Reply),
            (1, Directive::Reply),
            (0, Directive::Close),
        ]
    );
}

#[test]
fn the_closing_instructions_ask_the_host_to_close_the_show() {
    let model = Scripted::alternating(12);
    let configs = model.configs.clone();
    let (mut show, host) = support::open(model, LIMIT);
    let _ = support::run(&mut show, &host);
    let configs = configs.borrow();
    let last = configs.last().expect("a closing session");
    assert!(
        last.instructions.contains("close the show"),
        "{}",
        last.instructions
    );
}

#[test]
fn stopping_ends_the_show_at_once() {
    let (mut show, host) = support::open(Scripted::alternating(12), Duration::from_secs(60));
    for _ in 0..6 {
        let _ = show.step();
        let _ = host.pull(480);
    }
    let events = show.stop();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Finished { .. })),
        "{events:?}"
    );
    assert!(show.is_over());
    assert_eq!(show.speaker().queued(), 0);
    assert!(show.step().is_empty(), "nothing happens after the end");
}

#[test]
fn every_usage_the_sessions_report_is_handed_on_with_its_host() {
    let (mut show, host) = support::open(Scripted::alternating(12), LIMIT);
    let events = support::run(&mut show, &host);
    let mut usage = [0_usize; 2];
    for event in &events {
        if let Event::Usage {
            host: speaker,
            usage: repo_podcast::Usage::Response { .. },
        } = event
        {
            usage[*speaker] += 1;
        }
    }
    assert_eq!(usage, [4, 3], "one response usage per turn, per host");
}

#[test]
fn the_level_of_a_frame_is_its_loudness() {
    let format = pcm(24_000);
    let silence: AudioFrame = frame(format, 0, 0);
    assert!(level(&silence) < 0.001);
    let mut loud = frame(format, 0, 0);
    for pair in loud.data.chunks_mut(2) {
        pair.copy_from_slice(&i16::MAX.to_le_bytes());
    }
    assert!(level(&loud) > 0.9, "{}", level(&loud));
}
