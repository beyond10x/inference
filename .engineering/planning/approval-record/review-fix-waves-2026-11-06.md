---
format: aep.planning-md/3
id: approval-record:review-fix-waves-2026-11-06
kind: approval-record
status: draft
title: Review-fix stories sequenced into 0.18.0, 0.19.0 and 0.20.0, after the 0.17.0 voice-provider wave
relations:
- decides: story:live-session-close-and-bounded-log
- decides: story:pipeline-tool-and-node-cancel-findings
- decides: story:openai-transport-deadlines-and-cleartext
- decides: story:nova-sonic-stall-and-bounds
- decides: story:acknowledging-sink-drain-ends
- decides: story:llama-server-loopback-only
- decides: story:runtime-subgraph-and-recorder-bounds
- decides: story:contract-usage-and-naming-drift
- decides: story:llm-wrapper-outcomes-and-debug
- decides: story:example-and-testkit-findings
revision: 1
---
## Decision

On 2026-11-06 the owner asked for every finding of review-result:full-codebase-2026-11-06 to be
fixed. The coordinator sequenced the ten stories filed from it as follows:

- The 0.17.0 wave closes with its four voice-provider units only and releases when the last of
  them merges. None of the ten stories rides in it.
- 0.18.0 takes four stories, the crates with warnings first: story:live-session-close-and-bounded-log,
  story:pipeline-tool-and-node-cancel-findings, story:openai-transport-deadlines-and-cleartext,
  story:nova-sonic-stall-and-bounds.
- 0.19.0 takes the next four: story:acknowledging-sink-drain-ends, story:llama-server-loopback-only,
  story:runtime-subgraph-and-recorder-bounds, story:contract-usage-and-naming-drift.
- 0.20.0 takes the last two, notes only: story:llm-wrapper-outcomes-and-debug,
  story:example-and-testkit-findings.
- The provider stories accepted earlier for 0.18.0 move behind these waves.
- Implementation of the first two started before this sequencing, in their own trees forked from
  the 0.17.0 wave branch; they merge into the 0.18.0 integration branch.
- Builds run one cargo call at a time across both waves.
