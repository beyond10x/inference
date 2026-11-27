---
format: aep.planning-md/3
id: approval-record:quality-check-records-ordered-2026-11-26
kind: approval-record
status: draft
title: Records written by each check run are ordered (2026-11-26)
relations:
- decides: story:quality-records-written-by-each-check-run
revision: 1
---
## Decision

On 2026-11-25 the owner wrote to the coordinator: "When that is merged, we also want to evaluate
it, right, and track over time. Maybe you can already start doing that technically."

The first story of five, story:quality-records-written-by-a-program, merged on 2026-11-26
(pull request 76). The coordinator took the second from the same scope read: each run of the
workflow `check` that runs the full gate writes the records of the assessments that read the
JUnit report of the workspace run, and a push to `main` keeps them as one forge artifact. The
coordinator ran the moves draft to proposed to active for
story:quality-records-written-by-each-check-run on the owner's words. The forge's job and step
times are left to a story of their own.

## What this does not authorise

No series of records: where it lives is a question put to the owner on 2026-11-25, and a forge
artifact is the first step of each of its options. No call to a hosted provider and no keyed
run. No secret, no schedule, no write to a branch or a ref by a job, no change of a committed
record, plan file or entry state. No tag and no release follows from this record.
