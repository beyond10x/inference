# ElevenLabs realtime speech-to-text: the protocol the fixtures follow

Read on 2026-11-12, without a key, from
<https://elevenlabs.io/docs/api-reference/speech-to-text/v-1-speech-to-text-realtime> (the
AsyncAPI 2.6.0 description, fetched as Markdown). The page has no publication or revision date of
its own, so the date it was read is the only date. This is a summary of that page; the binding's
reading of it, and what it decided where the page is silent, are in `src/stt_protocol.rs`.

## Connection

- `GET /v1/speech-to-text/realtime` upgraded to a WebSocket. Servers: `wss://api.elevenlabs.io/`
  (default), `wss://api.us.elevenlabs.io/`, `wss://api.eu.residency.elevenlabs.io/`,
  `wss://api.in.residency.elevenlabs.io/`, `wss://api.sg.residency.elevenlabs.io/`.
- Authentication: header `xi-api-key`, or a single-use `token` query parameter.
- Query: `model_id` (`scribe_v2_realtime`, the default, or `scribe_v3_live`), `token`,
  `audio_format` (described only as "Any type"; the session's `AudioFormatEnum` is `pcm_8000`,
  `pcm_16000` (default), `pcm_22050`, `pcm_24000`, `pcm_44100`, `pcm_48000`, `ulaw_8000`),
  `language_code`, `secondary_languages` (array), `commit_strategy` (`manual` default, `vad`,
  `turn_prediction` for models that predict turns), `vad_threshold` (double),
  `vad_silence_threshold_secs` (double), `min_speech_duration_ms` (integer),
  `min_silence_duration_ms` (integer), `include_timestamps` (default false),
  `include_language_detection` (default false), `keyterms` (array), `no_verbatim`,
  `multilang_mode`, `diarize`, `entity_detection`, `entity_redaction`, `entity_redaction_mode`,
  `transcript_edit`, `filter_background_audio`, `keepalive_interval_ms` (500 to 10000),
  `enable_logging` (default true).

## Client to server

One message type, `input_audio_chunk`: `message_type`, `audio_base_64` (base64, required),
`commit` (boolean, required: commit the transcription after this chunk), `sample_rate`
(integer, required), `previous_text` (only with the first chunk; an error otherwise).

## Server to client

- `session_started`: `session_id`, `config` (`sample_rate`, `audio_format`, `language_code`,
  `commit_strategy` `manual` or `vad`, the VAD settings, `model_id`, `enable_logging`,
  `include_timestamps`, `include_language_detection`, `keyterms`, `no_verbatim`).
- `partial_transcript`: `text`, interim and may change.
- `committed_transcript`: `text`, stable for its segment.
- `committed_transcript_with_timestamps`: `text`, `language_code` (string or null), `words`
  (array or null) of `text`, `start` and `end` in seconds, `type` (`word` or `spacing`),
  `speaker_id`, `logprob`, `characters`.
- With options the binding does not set: `diarized_transcript`, `speakers_merged`,
  `committed_transcript_entities`, `redacted_transcript`, `edited_transcript`.
- `warning`: `warning`.
- Fourteen error types, each with `message_type` and `error` (string): `error`, `auth_error`,
  `quota_exceeded`, `commit_throttled`, `unaccepted_terms`, `rate_limited`, `queue_overflow`,
  `resource_exhausted`, `session_time_limit_exceeded`, `input_error`, `invalid_request`,
  `chunk_size_exceeded`, `insufficient_audio_activity`, `transcriber_error`.

## Not stated

No usage or billing message, no close codes, no end-of-stream message, no link between a commit
and the committed transcript that answers it, whether both committed messages are sent when
timestamps are on, and how a commit with nothing to commit is answered.
