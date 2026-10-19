---
format: aep.planning-md/3
id: review-result:adversary-inbound-text-spans-and-envelope-pass-1
kind: review-result
status: active
title: Adversary, story:inbound-text-spans-and-envelope, pass 1
relations:
- reviews: story:inbound-text-spans-and-envelope
revision: 1
---
```
unit: story:inbound-text-spans-and-envelope, uncommitted working tree on base 75df5fb (impl/inbound-text-spans-and-envelope)
verdict: INFEASIBLE (one red case, nothing produces the state); the rest CONFIRMED
cases: executed 200→208, red 1
origin: introduced 4 / pre-existing 0 / undecided 0
wrote-outside-worktree: 26 paths, all under <unit scratch>/adversary-1/ (part 6)
needs-coordinator: none
```

The 200 is from the implementor's final logs (core 35, text 14, testkit 151). The 208 is my gate run: 37 + 16 + 155.

**1. Diff.** `git --no-pager diff --stat` reads `12 files changed, 1450 insertions(+), 75 deletions(-)`, the same as the unit's diff. None of those 12 files is mine. I touched no implementation file. My three files are untracked:
- `crates/inference-core/tests/adversary1_envelope_contract.rs`
- `crates/inference-text/tests/adversary1_inbound_contract.rs`
- `crates/inference-testkit/tests/adversary6_inbound_envelope_log.rs`

**2. Cases added.** Each file was run alone before the suite. The ESS-driven cases read `ess/domains/stream.yaml` as written (`include_str!`).

| file:line | asserts | now | mutant: unit suite stays green, my case goes red |
|---|---|---|---|
| testkit adv6:164 | every `StreamEvent` has a name declared in the YAML; it carries a generation exactly when its declaration has `generation_id`; the log refuses each such event under another generation | green | M1: `GenerationSuperseded` arm returns `None` (151 pass / red). M4: `TurnOpened` misnamed (151 / red) |
| testkit adv6:217 | an event refused for its sequence or its generation can be sent again under the same `event_id` | green | M2: `event_ids.insert` runs before the other checks (151 / red) |
| testkit adv6:273 | a fixed-seed walk of 3000 records matches a model of the three rules in their documented order (reaches all three refusals) | green | M2 (red). M3: sequence checked before duplicate (151 / red) |
| testkit adv6:349 | an envelope whose `kind` is not its payload's declared name is refused | **red** | – |
| core adv1:66 | the envelope writes the 14 declared fields in declared order; optional exactly where the type is `Optional<…>`; `Integer` fields refuse 1.5 | green | M6: `port` moved before `producer` (35 / red) |
| core adv1:109 | `Timebase::ALL` matches the declared `variants:` | green | – |
| text adv1:69 | the inputs of `OpenTurn`, `EmitHypothesis` and `ReviseSpan`, the 5 inbound events and the 5 inbound errors write their declared fields in order | green | M5: `#[serde(rename="seq")]` on `TranscriptHypothesis.sequence` (text 14 / testkit 151 / red) |
| text adv1:170 | a revision changes only the text | green | none tried |

Red output captured when the kind case was written, run alone (`cargo test -p inference-testkit --all-features --test adversary6_inbound_envelope_log`, exit 101):
```
thread 'an_envelope_whose_kind_is_not_its_payloads_declared_name_is_refused' panicked at crates/inference-testkit/tests/adversary6_inbound_envelope_log.rs:353:5:
a TurnCompleted (control) recorded under the kind TranscriptHypothesis (content) is read as content by every consumer that dispatches on kind; stream.yaml declares kind as the declared name of the payload's event and docs/design/inference.md:297 says control must not be mistaken for user content: Ok(())
test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```
(After formatting the file the assertion sits at line 358.) The core and text files run alone exited 0 with 2 passed each.

**3. Suite run** (after the cases existed). Each command ran on its own with its own exit status:

| command | exit | result |
|---|---|---|
| `cargo test -p inference-core --all-features --no-fail-fast` | 0 | passed 37, failed 0 |
| `cargo test -p inference-text --all-features --no-fail-fast` | 0 | passed 16, failed 0 |
| `cargo test -p inference-testkit --all-features --no-fail-fast` | 101 | passed 154, failed 1: `an_envelope_whose_kind_is_not_its_payloads_declared_name_is_refused ... FAILED` |
| `cargo clippy` on each of the three crates | 0 | – |
| `cargo fmt --check` on each of the three crates | 0 | first run exited 1 on my own files only; I applied `rustfmt --edition 2024` to those three files and re-ran |

`ess specify validate --path ess` exited 0. I re-synthesized both the base 75df5fb and the working tree: 229 scenarios each, the same ids, every scenario body identical, and the same 3 refusals. The implementor's claim that the suite is unchanged holds.

**4. Findings**
- **F1** `crates/inference-testkit/src/stream.rs:359`, `EventLog::record` never compares `kind` with `payload.name()`.
  - Measured: adv6:358 is red. A `TurnCompleted` filed under the kind `TranscriptHypothesis` is recorded (`Ok(())`).
  - What reaches it: nothing found. Only tests call `EventLog::record`, and they build every envelope by hand. INFEASIBLE, note.
  - Fix: a fourth refusal, `KindMismatch`.
- **F2** `crates/inference-testkit/src/stream.rs:649`, judgement. Story step 4 says the log "wraps every recorded event in an envelope". `StreamDomain` keeps bare `StreamEvent`s and never passes through `EventLog`.
  - Measured: no path from any command to `EventLog`.
  - Consequence: §23.1 test 6 is enforced only on envelopes a caller builds. `emit_hypothesis` (`:700`) accepts a repeated `(stream_id, sequence)`; the YAML marks this UNMAPPED at line 509.
  - What reaches it: every caller of `StreamDomain`. NEEDS-CHANGE, warning: either the story's wording or the design.
- **F3** `crates/inference-testkit/src/stream.rs:256`, mutant. The unit's suite stays green with M1, M2, M3 and M4 applied. The guards are adv6:164, :217 and :273. CONFIRMED, warning.
- **F4** `crates/inference-text/src/inbound.rs:104` and `crates/inference-core/src/envelope.rs:61`, contract drift. Field names and order on the wire are checked against literals, or not at all. M5 and M6 survive; the guards are text adv1:69 and core adv1:66. CONFIRMED, note.

**5. Attacked and could not break**
- Identifier types and optionality on both sides of the envelope (`EventEnvelope`).
- The `Timebase` variants and their order.
- `Turn` and `Span` transitions; `CommittedSpanImmutable` leaves the text as it was.
- The not-found and wrong-state answers in the target.
- The 15/214 conform split and the SKIPPED diff (3 ids).
- Positions in the one publication order.
- Event names and fields matched against the YAML.

**6. Paths written outside the worktree.** Everything is under `<unit scratch>/adversary-1/`:
- scripts and suites: `cmp.py`, `mutate.sh`, `replace.py`, `suite-base.json`, `suite-new.json`, `synth-base.log`, `synth-new.log`, `mutations.log`
- single-case runs: `case-core.log`, `case-text.log`, `case-testkit.log`
- gate logs: `gate-{test,clippy,fmt}-inference-{core,text,testkit}.log` (9 files)
- mutant logs: `mut-M*-*.log` (11 files)

I deleted the mutant copy `mut/` (1.9G including its `target/`) and the base export `base/` after logging their runs. Disk was at 11G free at the end. The lease is released.

```findings
- file: crates/inference-testkit/src/stream.rs
  line: 359
  category: property
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: EventLog::record accepts an envelope whose kind is not its payload's declared name, so a control event can be recorded as content; only hand-built test envelopes reach it
- file: crates/inference-testkit/src/stream.rs
  line: 649
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: StreamDomain records bare StreamEvents and never passes them through EventLog, so the story's "wraps every recorded event in an envelope" and the section 23.1 test 6 rules apply to no event a command emits
- file: crates/inference-testkit/src/stream.rs
  line: 256
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: four EventLog/StreamEvent mutants (superseded carries no generation, id taken before checks, sequence checked before duplicate, TurnOpened misnamed) leave the unit's 151 testkit cases green
- file: crates/inference-text/src/inbound.rs
  line: 104
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: renaming an inbound event field on the wire, and reordering the envelope's fields, leaves the unit's suite green because no test reads the wire names from stream.yaml
```
