# Scenarios the testkit target does not answer yet

Every scenario of the synthesized suite that `crates/inference-testkit/tests/conform.rs` reports
`unsupported`, one line each, under the domain it belongs to, naming the story or epic that will
answer it. The test asserts that the `unsupported` ids of its report equal the ids listed here, so
a story that makes a scenario pass removes its line, and a change to the scenario set changes
this file with it.

## inference.graph

## inference.stream

## inference.tools


## inference.session

- inference.session.CloseSession/outcome/draining: answered by epic:session-facade
- inference.session.CloseSession/outcome/no-such-session: answered by epic:session-facade
- inference.session.CompleteSessionClose/outcome/closed: answered by epic:session-facade
- inference.session.CompleteSessionClose/outcome/no-such-session: answered by epic:session-facade
- inference.session.MarkSessionReady/outcome/no-such-session: answered by epic:session-facade
- inference.session.MarkSessionReady/outcome/ready: answered by epic:session-facade
- inference.session.OpenSession/outcome/mode-mismatch: answered by epic:session-facade
- inference.session.OpenSession/outcome/not-planned: answered by epic:session-facade
- inference.session.OpenSession/outcome/opened: answered by epic:session-facade
- inference.session.OpenSession/outcome/unknown-pipeline: answered by epic:session-facade
- inference.session.Session/state/Closed/refuses/inference.session.CloseSession: answered by epic:session-facade
- inference.session.Session/state/Closed/refuses/inference.session.CompleteSessionClose: answered by epic:session-facade
- inference.session.Session/state/Closed/refuses/inference.session.MarkSessionReady: answered by epic:session-facade
- inference.session.Session/state/Draining/refuses/inference.session.CloseSession: answered by epic:session-facade
- inference.session.Session/state/Draining/refuses/inference.session.MarkSessionReady: answered by epic:session-facade
- inference.session.Session/state/Opened/refuses/inference.session.CompleteSessionClose: answered by epic:session-facade
- inference.session.Session/state/Ready/refuses/inference.session.CompleteSessionClose: answered by epic:session-facade
- inference.session.Session/state/Ready/refuses/inference.session.MarkSessionReady: answered by epic:session-facade
- inference.session.Session/transition/close/by/inference.session.CompleteSessionClose/closed: answered by epic:session-facade
- inference.session.Session/transition/drain/by/inference.session.CloseSession/draining: answered by epic:session-facade
- inference.session.Session/transition/ready/by/inference.session.MarkSessionReady/ready: answered by epic:session-facade
