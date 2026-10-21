//! The optional session API of the inference framework.
//!
//! `inference-facade` holds the `inference.session` domain of `ess/domains/session.yaml`
//! ([`session`]): a session is an application-level interaction lifetime above graph runs, opened
//! over one planned pipeline in its interaction mode, holding its current run, zero or one, and
//! closed by draining. Sessions are optional: no other crate of the workspace needs one, and a
//! pipeline runs without one. What the state machine reads of the graph it references and does
//! not hold, a pipeline's existence, state and mode and a run's pipeline, it reads through the
//! [`session::Graph`] trait its host implements.
//!
//! A live session ([`live`]) binds a session to a run of its pipeline under the runtime's run
//! controller and reaches the run only through the pipeline's exposed ingresses and egresses, so
//! the application sees no topology: it opens, sends, commits its input, receives one event
//! vocabulary ([`events::SessionEvent`]), interrupts the active generation and closes by draining
//! or by cancelling, in-process and over no executor in particular. What the session commits and
//! delivers is its history ([`history`]), which outlives its runs. A Ready session switches to
//! another planned pipeline ([`switch`]), draining its run first and keeping its history and its
//! vocabulary, and [`describe`] reports the pipeline it runs now: nodes, bindings, effective
//! capabilities, downgrades and exposures. What the session's model calls consumed it delivers
//! as `UsageRecorded` and adds up per run and over the session ([`usage`]). The crate carries no
//! concrete model, backend or provider.

pub mod describe;
pub mod events;
pub mod history;
pub mod live;
pub mod session;
pub mod switch;
pub mod usage;
