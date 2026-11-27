---
format: aep.planning-md/3
id: approval-record:wave-2026-11-27-evaluate-ordered
kind: approval-record
status: draft
title: The series on a ref and the local keyed lane are built; the download is admitted by a gates change first (2026-11-27)
relations:
- decides: story:quality-records-series-on-a-ref
- decides: story:quality-live-lane-on-the-workstation
- decides: story:quality-records-adopted-from-a-main-run
revision: 1
---
## Decision

On 2026-11-27 the owner and the operator decided the open questions of the evaluation plan:

- D43 A (the owner): keyed runs execute on this workstation. Keys come from the local keychain;
  they are never printed and never reach a commit or a log.
- D48 A (the operator): the series of quality records is written by CI with `GITHUB_TOKEN`, one
  job alone.
- D65 A (the operator): because the ruleset lets only the bot app create or update a branch, the
  series lives on the ref `refs/ratchet/records`, outside `refs/heads`, as Ratchet's design has
  it; no ruleset change. If the forge refuses the push, the job fails and the refusal is
  reported; it is not worked around.
- D62 B (the owner): the download a sub-agent routed around is admitted by a reviewed change of
  `b10x-gates` first; the records of story:quality-records-adopted-from-a-main-run are then
  downloaded again through it, and the files of the routed-around download are deleted.

The coordinator drafted story:quality-records-series-on-a-ref and
story:quality-live-lane-on-the-workstation from the evaluation plan's stories 4 and 5 and moves
them to active on these words.

## What this does not authorise

- No live run: the first run of `task quality-live` waits for the owner's yes to its count of
  hosted calls, and each later run too.
- No ruleset, branch protection or policy change; no forge secret.
- No tag and no release.
