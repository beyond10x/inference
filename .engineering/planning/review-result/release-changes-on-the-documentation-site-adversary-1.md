---
format: aep.planning-md/3
id: review-result:release-changes-on-the-documentation-site-adversary-1
kind: review-result
status: active
title: Adversary pass 1, by reading, over the release pages of the documentation site (2026-11-25)
relations:
- reviews: story:release-changes-on-the-documentation-site
revision: 1
---
unit: story:release-changes-on-the-documentation-site (pass 1, by reading; worktree inference-release-docs, HEAD fe29cbad6 plus the uncommitted generator diff)
verdict: findings
wrote-outside-worktree: <scratch>/release-docs/adversary-1/ (created, left empty)

I found 3 warnings and 9 notes. Nothing was built or run. I made no edit, commit or push in the worktree, and called no hosted provider. The rules of brief item 6 hold: no person's name, private host or path is in the diff since 6ca6a2509; the implementor changed only `website/docs/{guides/run-the-checks.md,examples/repo-podcast.md}` and `crates/inference-docs/src/*`; `limitations.md` is untouched and no version number changed.

## Findings

1. **warning**: the GPT-Live note says the close timeout sets the wait, but the default read timeout overrides it.
   - Sentence: `crates/inference-docs/src/adapters.rs:274-275` says "`LiveClient::with_close_timeout` sets how long a graceful close waits for `session.closed`".
   - Source: `adapters/inference-openai/src/live.rs:1063` is `let bound = self.receiver.read_timeout.max(self.receiver.close_timeout);`. `live.rs:103-106` (CLOSE_TIMEOUT applies "when the endpoint's read timeout is shorter"), `live.rs:114` and `config.rs:22` (the default read timeout is 30 s), and `live_duplex.rs:403-405` agree.
   - What goes wrong: with the default endpoint, any close timeout up to 30 s changes nothing, including the 15 s default. A caller who sets 2 s to close faster still waits up to 30 s. A later clause says "the longer of", but the first sentence tells the caller the opposite.
   - Accepted: "`LiveClient::with_close_timeout` sets the close timeout and `LiveClient::close_timeout` reads it; it stays with the session across `LiveClient::into_split` and `LiveClient::reunite`. `OpenAiLive::with_close_timeout` and `OpenAiLive::close_timeout` do the same for the sessions the model opens. Unset, it is `CLOSE_TIMEOUT` (15 s), as before. A graceful close waits for `session.closed` at most the longer of the close timeout and the endpoint's read timeout, `DEFAULT_READ_TIMEOUT` (30 s) unless `Endpoint::with_read_timeout` set another. With that default, a close timeout of 30 s or less changes nothing, and a shorter close needs a shorter read timeout, which also bounds every read of the session. Without `session.closed` by then, …" (rest unchanged).

2. **warning**: the name check passes false notes.
   - Code: `adapters.rs:410-418` splits a code span `a::b` on `::` and only asks whether each part is a whole word anywhere in the package's `src/`, comments and strings included (`errors.rs` `names`). The module doc at `adapters.rs:120-123` says it refuses "a code span naming an item, `a::b`, that no source of the package names", which suggests the path itself is checked.
   - Mutants it does not catch (every word is present in the sources):
     - `ElevenLabsAgent::signed` swapped with `ElevenLabsAgent::public` (reverses the note);
     - `SessionFailureKind::Rejected` changed to `SessionFailureKind::Stalled` or `TransportClosed`;
     - `RealtimeError::Stalled` changed to `RealtimeError::Closed`;
     - `agent::AgentError::SignedUrlSpent` changed to `agent::AgentError::FormatMismatch`;
     - `OpenAiLive::reunite`, a method that does not exist;
     - a method renamed in code whose old name stays a word in comments (for example `signed`, see `agent_duplex.rs:28-43`);
     - an item of another binding of the same package, such as `CLOSE_GRACE` in the ElevenLabs agent note (both bindings are in `inference-elevenlabs`);
     - a span with `.` or a space, such as `session.closed`, which is skipped entirely.
   - Accepted (module doc), if the check stays as it is: "…and a code span `a::b` one of whose parts is no whole word of the package's sources; that the path exists, is public or belongs to that binding is not checked." Otherwise, hold the last part to a definition (`fn b`, `const B`, a variant) inside `impl A` or `enum A`.

3. **warning**: the constant check reads the first definition in the whole package, so it can refuse a true note and pass a false one.
   - Code: `adapters.rs:332-338` takes the first `pub const NAME:` line over all sources of the package, sorted by path (`errors.rs:94-138`).
   - Source: `inference-elevenlabs` defines `DEFAULT_READ_TIMEOUT` twice, `agent.rs:100` (30 s) and `config.rs:23` (10 s).
   - What goes wrong:
     - A TTS note giving "`DEFAULT_READ_TIMEOUT` (10 s)", which is true, is refused; "(30 s)", which is false for TTS, passes.
     - An honest, unrelated edit turns the gate red: adding a `pub const CLOSE_BOUND` with another value to `agent.rs`, which sorts before `connection.rs`, fails the true ElevenLabs TTS note.
   - Accepted: generation refuses a constant that the package's sources define more than once, or reads it from the module of the binding. The doc of `Note.constants` says which.

4. **note**: the constant check is brittle against honest edits.
   - Code: `adapters.rs:332-353` parses only a definition on one line, `pub const NAME: … = …from_secs(n)` or `from_millis(n)`, with `n` in plain digits.
   - Each of these makes generation fail with "no source … defines X as whole seconds or milliseconds":
     - `from_millis(1_000)`;
     - a definition rustfmt wraps after `=`;
     - `from_secs(60 * 5)`;
     - `pub(crate) const` re-exported through `pub use`.
   - Accepted: also parse `_` and a wrapped definition, or state in the doc of `Note.constants`: "each defined on one line as `pub const NAME: … = …from_secs(n)` or `from_millis(n)`, `n` in plain digits".

5. **note**: the value check covers only listed constants, and only one mention of each.
   - Code: `adapters.rs:397-407` checks only the names in `constants`, with `text.contains`.
   - Mutants it does not catch:
     - drop `CLOSE_GRACE` from `constants` and write "(50 ms)";
     - add a second, wrong "`CLOSE_BOUND` (2 s)" next to the right one;
     - change the prose ("at most one session" to "two", "the longer" to "the shorter").
   - Accepted (module doc): "…a constant listed in the note's `constants` that the text does not give at least once with its value; other numbers and other mentions in the text are not checked."

6. **note** (gap in the acceptance and the planned scope): the compare guide says nothing about the signed URL.
   - Page: `website/docs/guides/compare-voice-providers.md:37-55,83` does not name `ELEVENLABS_AGENT_ID` or `ELEVENLABS_AGENT_SIGNED_URL`, and not that only the first run is measured with a signed URL. Its Requirements box (lines 39-40) sends the reader to `talk-to-a-hosted-voice-model.md`, which has no ElevenLabs agent variable. Line 83 runs `elevenlabs` with `--runs 5`.
   - Source: `examples/realtime-voice/examples/compare.rs:22-27`; CHANGELOG `[Unreleased]` › Changed, the `compare` item.
   - The story asks for this "where it names the agent"; this page names it. `reference/environment.md:37` alone is not the page that covers the example.
   - Accepted (after line 55): "The `elevenlabs` provider talks to the agent `ELEVENLABS_AGENT_ID` names, or to a private agent through the signed URL in `ELEVENLABS_AGENT_SIGNED_URL` ([Environment](../reference/environment.md)). On main, in the next release, a signed URL opens one session: the arm measures its first run and records every later run with the refusal, `agent::AgentError::SignedUrlSpent`, as its note."

7. **note** (place): the close timeout is missing from the guide that lists the `inference_openai` API.
   - Page: `website/docs/guides/talk-to-a-hosted-voice-model.md:163-187` lists the `inference_openai` API by release, including `LiveClient::into_split`, `reunite` and `ping`. The new `LiveClient::with_close_timeout`, `LiveClient::close_timeout` and the `OpenAiLive::` pair are only on the generated adapters page.
   - Source: `live.rs:886-901`, `live_duplex.rs:403-415`.
   - Accepted (after line 187): "On main, in the next release: `LiveClient::with_close_timeout` and `OpenAiLive::with_close_timeout` set how long `close` waits for `session.closed` when the endpoint's read timeout is shorter; [Adapters](../reference/adapters.md#opens-closes-and-drops) gives the longest a close takes."

8. **note** (voice): the agent note uses a phrase the adapters page never defines.
   - Sentence: `adapters.rs:304` and `:307`, "gets past the configuration and the runtime".
   - Source: `agent_duplex.rs:28-30` and `:499-523` define it: the binding takes the configuration (`SessionLedger::open`) and the session's runtime was built.
   - Accepted: "Its first open that the binding does not refuse for its configuration, and whose runtime was built, takes the signed URL for its handshake, whatever happens next: … Every later open the binding does not refuse for its configuration fails with …"

9. **note**: the repo-podcast section reads as GPT-Live only.
   - Sentence: `website/docs/examples/repo-podcast.md:100` says "GPT-Live creates every response itself, so the show feeds it silence".
   - Source: `examples/repo-podcast/src/show.rs:387` (`listen = !capabilities.manual_response`) and `:491`. Lines 105-106 address a show your own code opens with `Show::open`, with any model.
   - Also, the front matter `source:` (line 6) still names only "CHANGELOG.md 0.7.0".
   - Accepted: "A model that creates every response itself, as GPT-Live does, is fed silence, and the show takes its first response as the turn. When it sends no response, transcript or audio for 10 seconds, the turn is given up."

10. **note** (voice): the quality section reads as a document disagreeing with itself, and Ratchet is never introduced.
    - Sentence: `website/docs/guides/run-the-checks.md:168-170` says the tests fail "when the documents stop agreeing with each other, … or with `quality/README.md` and `quality/adapters.md`". The table just above counts those two files among the documents.
    - Also, "Ratchet" (lines 162 and 164) is used without saying what it is.
    - Source: `crates/inference-docs/tests/quality/mod.rs:1-3`.
    - Accepted: "They fail when the YAML files and records stop agreeing with each other, with `Taskfile.yml`, with the tree, or with the pages that describe them, `quality/README.md` and `quality/adapters.md`: …". Name Ratchet once as "Ratchet (`beyond10x/ratchet`)", as `quality/README.md:8` does.

11. **note**: one table row is false on one reading.
    - Sentence: `run-the-checks.md:162` says "one record per assessment that has a baseline, read from a CI run".
    - Source: `conformance/passing-scenarios` has a baseline and no record (`quality/README.md:239-244`), and so do the six local STT values (`README.md:530-533`). `README.md:28` says "one baseline per assessment that has one, read from a CI run".
    - Accepted: "…and one record per assessment whose baseline was read from a CI run".

12. **note**: the link in the error reference is not tied to the agent note.
    - Sentence: `crates/inference-docs/src/errors.rs:181-183`, "[Adapters](adapters.md#opens-closes-and-drops) says what a later open of an ElevenLabs agent model built with a signed URL returns", is rendered unconditionally.
    - Source: `adapters.rs` `notes_section` returns "" when there are no notes. Docusaurus `onBrokenAnchors: 'throw'` (`website/docusaurus.config.ts:23`) catches only that case, not the removal of the agent note alone, which would leave the sentence false.
    - Accepted: render the sentence only while `NOTES` has a note on "ElevenLabs agent conversation", or add a test that holds both together.

13. **note** (follow-up; the implementor may not edit `quality/`): the release-status measure misses the new note test.
    - Source: `quality/README.md:650-672`, `release-statuses/held`, "holds for 4 of 4 tests", lists four tests.
    - The new test `every_note_is_of_the_release_whose_changelog_section_brought_it` (`adapters.rs` tests) holds a page status against the changelog, but the measure does not list it.
    - Accepted: a follow-up entry, "Note statuses: the test `every_note_is_of_the_release_whose_changelog_section_brought_it` of `crates/inference-docs/src/adapters.rs` holds that each note of the adapters page names the release whose section of `CHANGELOG.md` first has its phrase", with the count raised to 5.

## Checked and true

- **ElevenLabs agent note**: what spends the URL, what does not, the error, the remedy, and the public model. Checked against `agent_duplex.rs:28-43,309-316,408-416,466-553`.
- **Both TTS drop notes**, against `elevenlabs synthesiser.rs:33-38,774-815`, `cartesia synthesiser.rs:634-675`, and `connection.rs` (elevenlabs :21,:146-150; cartesia :20,:146). Each case is right:
  - inside a Tokio runtime, at most 5 ms, then the close goes on on the handle's own thread within 1 s;
  - outside a runtime, and inside another executor, it waits up to 1 s (`Handle::try_current`);
  - a process that ends first ends the socket without the close frame, and for ElevenLabs the server may have received `close_socket` alone or neither;
  - an async exchange in flight is aborted, so no close frame.
- **Rest of the GPT-Live note**: the `Stalled` return, the unconfirmed usage of an `OpenAiLive` close, and the split and reunite carry (`live.rs:1039-1087`, `live_duplex.rs:1231-1270`).
- **repo-podcast**:
  - keeping the audio versus ending the show (`show.rs:684-693`);
  - the 10 s default (`show.rs:42`, `:82`);
  - the struct literal in `podcast.rs:161-164`.
- **Check-scope paragraph** of run-the-checks, against `.github/workflows/check.yml:45-50`.
- **Test placement**: `tests/quality/` is a module of the `workflows` test binary (`tests/workflows.rs:8`), which `task test-workspace` runs.
- **Anchors**: `#opens-closes-and-drops` and `#the-quality-documents` match their headings.
- **Generated page**:
  - deterministic: sources sorted, notes in the order of the bindings;
  - reads only `crates/` and `adapters/` under the root;
  - does not depend on the order of a directory listing.
- **Changelog phrases**: every phrase in the test's `NOTED` table is a one-line substring of `[Unreleased]`.

```findings
- file: crates/inference-docs/src/adapters.rs
  line: 274
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the GPT-Live note says with_close_timeout sets the close wait; the bound is the longer of the read timeout and the close timeout, so with the default 30 s read timeout any close timeout up to 30 s, the 15 s default included, changes nothing
- file: crates/inference-docs/src/adapters.rs
  line: 410
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the name check of a note only looks up each part of a path as a word anywhere in the package, comments included; swapped, nonexistent or other-binding paths pass
- file: crates/inference-docs/src/adapters.rs
  line: 332
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the constant check takes the first definition of that name in the whole package; inference-elevenlabs defines DEFAULT_READ_TIMEOUT twice (30 s, 10 s), so it passes a false value and an unrelated constant can fail a true note
- file: crates/inference-docs/src/adapters.rs
  line: 332
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the constant parser rejects honest forms (an underscore in the digits, a wrapped definition, an expression, a re-export), so generation goes red on such an edit
- file: crates/inference-docs/src/adapters.rs
  line: 397
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: only the constants a note lists are checked, and only one mention each; a wrong value for an unlisted constant or a second wrong mention passes
- file: website/docs/guides/compare-voice-providers.md
  line: 37
  category: acceptance
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the page of the compare example does not say that a signed URL measures only the first run, nor name the agent variables
- file: website/docs/guides/talk-to-a-hosted-voice-model.md
  line: 163
  category: acceptance
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the guide lists the LiveClient API by release but not the new close-timeout methods
- file: crates/inference-docs/src/adapters.rs
  line: 304
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the agent note says an open gets past the configuration and the runtime without saying on the page what that is
- file: website/docs/examples/repo-podcast.md
  line: 100
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the silent-turn section names GPT-Live only, though the rule holds for any model that creates its own responses; the source line of the front matter is not updated
- file: website/docs/guides/run-the-checks.md
  line: 168
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the quality section says the documents stop agreeing with quality/README.md, which is one of them, and Ratchet is never introduced
- file: website/docs/guides/run-the-checks.md
  line: 162
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: a table row says one record per assessment that has a baseline; conformance and the local transcription values have baselines and no record
- file: crates/inference-docs/src/errors.rs
  line: 181
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the sentence of the error reference that points to the agent note is rendered unconditionally; with the agent note removed it is false and the anchor check does not catch that
- file: quality/README.md
  line: 650
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the measure release-statuses/held lists 4 tests; the new test of the note statuses is not among them (a follow-up outside the files of this unit)
```

## Not checked

- Whether the generator compiles and its tests pass. Nothing was run, and the regenerated `reference/adapters.md` and `reference/errors.md` do not exist yet. Read by eye: the slice match on `Vec<&Note>` and the `Release: Copy` use look sound.
- How Docusaurus renders the long list items.
- Every row of `quality/README.md` against the YAML and the records.
- The count of "54 tests" on `repo-podcast.md`.
- `status.mdx`.
- Whether `inference_openai::live` and `live_duplex` items need features the bindings table does not name.
- The `realtime-voice` `elevenlabs::resolves_within` entry. It changes nothing a caller of a published crate writes, so I did not hunt for a page; the implementor's report does not mention it either.

