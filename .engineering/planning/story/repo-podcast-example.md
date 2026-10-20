---
format: aep.planning-md/3
id: story:repo-podcast-example
kind: story
status: implemented
title: 'A podcast example: Claude distils a repository, two voice models discuss it over the speaker'
relations:
- decomposes: epic:native-voice
- serves: vision:inference
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T16:23:02Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-20T16:23:02Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-20T19:15:54Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1}}}
---
## Context

The owner, 2026-10-20: "create a podcast example where first we use Claude to point it to a certain
directory, like a repository. Then it will distill details about it, and then you feed this as
context into basically two models which are talking to each other, like in a podcast situation.
And I just want to listen to that, and it shall be played out over the speaker. [...] a 5 minute,
10 minute talk over this repo." A fun example, first pointed at this repository.

## Planned scope

- `examples/repo-podcast` (new example package, no binary target; an example `podcast` with clap
  derive): `--dir <path>` (default: the repository root), `--minutes <n>` (default 5),
  `--context-file <path>` to skip the distilling step, `--save-context <path>`, `--provider
  openai|openai-live` (default `openai`).
- Distil: run the Claude Code CLI as an external process in `--dir`, non-interactive
  (`claude -p` with a fixed prompt asking for a podcast brief: what the project is, why it exists,
  its parts, three surprising details, open questions; read-only tools only), capture its stdout
  as the brief. It is an external process the example talks to; nothing of it is committed here.
  A missing `claude` is a clear error naming `--context-file` as the alternative.
- Two hosts: two `inference-duplex` speech sessions on the chosen provider with different voices
  and personas in their instructions (a curious host and an expert guest), both given the brief.
  They take turns: each host's finished output transcript is sent to the other as text input; the
  host's audio is played on the default speaker through `inference-audio-device`, one host at a
  time, never overlapping. The conversation opens with an intro, runs until `--minutes` of audio
  have played, and the host is asked to close the show at the end. Enter stops early.
- Prints the transcript as it plays and, at the end, the usage per host and in total (the usage
  display of `realtime-voice`).
- Tests on the duplex mock and the fake audio host: the turn order alternates, each host receives
  the other's transcript as text, audio never overlaps, the time limit ends the show with a
  closing turn, a missing `claude` binary gives the named error, `--context-file` skips it.

## Acceptance

`cargo test -p repo-podcast` passes on the mocks; `cargo run -p repo-podcast --example podcast`
on a machine with the OpenAI key in the keychain and `claude` on the path distils this repository
and plays a two-host conversation about it over the speaker for the given minutes.

## Terminal UI

Added on the owner's words, 2026-10-20: "some sort of running counter for the costs and the tokens
and so on [...] some fancy TUI which animates both sides speaking and you see the text"; "this TUI
app to be recorded [...] maybe 2 minutes"; "while you record it [...] play it out at the same time".

- The `podcast` example is a terminal UI (ratatui over crossterm, both MIT): two panels, one per
  host, each with its name, voice and an animated level meter driven by the audio it is playing
  (idle when the other host speaks); the live transcript of the current turn under the speaker;
  a footer with running counters per host and in total: input and output text and audio tokens,
  turns, elapsed and remaining time, and the estimated cost so far.
- Cost: from a price table file (`--prices`, the `realtime-voice` format). The example ships one
  table per supported model under `examples/repo-podcast/prices/`, each naming its source URL and
  the date it was read; the TUI shows the table's name and date beside the cost. GPT-Live is
  priced per session second.
- Plain mode (`--plain`) prints the same events as lines, for logs and tests.
- The TUI renders against ratatui's test backend in the tests: both panels, the meter of the
  speaking host active and the other idle, counters equal to the sums of the usage events.
