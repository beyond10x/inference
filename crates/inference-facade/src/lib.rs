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
//! as `UsageRecorded` and adds up per run and over the session ([`usage`]). The session's event
//! log and the bookkeeping an interrupt cuts by are bounded: at most 256 generations kept and
//! 256 cut generations remembered past their end, so that a refusal or any other event of a cut
//! generation arriving after its end stays cut; a late event of a cut generation older than
//! those is delivered, the one exception to the cut ([`live::LiveSession::interrupt`]). A session's history grows with its turns. The crate
//! carries no concrete model, backend or provider. Behind the feature `cost`, `cost` prices those records
//! with a price document the caller supplies, record by record and over the session, through
//! `llm-cost`; without it the crate depends on no pricing crate.

#[cfg(feature = "cost")]
pub mod cost;
pub mod describe;
pub mod events;
pub mod history;
pub mod live;
pub mod session;
pub mod switch;
pub mod usage;
