//! Switching a live session to another topology without changing its event vocabulary
//! (`docs/design/inference.md` A-12 at 1148, the P4 exit at 1162).
//!
//! [`LiveSession::switch`] moves a Ready session over to another planned pipeline: it runs
//! `inference.session.SwitchPipeline`, which leaves the session Opened with no current run, so
//! that input sent meanwhile is refused by the Ready guard of `SendInput`; then it drains the run
//! below rather than cancelling it: it sends `stop` into every ingress of the old run, receives
//! what the old run's egresses still hold, delivered later in order among the session's own
//! events, and waits until the old run has ended; how it ended, `RunCompleted` or another, is
//! delivered after what its egresses held. A user turn the application did not commit ends with
//! the old run, unrecorded. Only once the old run Completed does the session create,
//! attach (`RunAttached` announces the new run) and activate the new pipeline's run and mark
//! itself ready, as [`crate::live::LiveSessions::open`] does. The session, its history and its
//! one [`SessionEvent`] vocabulary carry over; the new run's model is not given the history.
//!
//! The old run's driver goes on driving it to its end; the switch answers the new run's
//! [`SessionDriver`], which the application polls as it polled the first.

use std::error::Error;
use std::fmt;
use std::future;
use std::task::{Context, Poll};

use inference_core::envelope::ControlKind;
use inference_core::id::RunId;
use inference_pipeline::{
    Declaration, InteractionMode, Node, NodeImplementation, PipelineNotFound, PlannedPipeline,
};
use inference_runtime::run::RunState;

use crate::events::{SessionEvent, SessionPayload};
use crate::live::{LiveSession, OpenError, Planned, SessionDriver, lock, run_session};
use crate::session::{SwitchPipeline, SwitchPipelineError};

/// Why [`LiveSession::switch`] answered no new run.
#[derive(Debug)]
pub enum SwitchError {
    /// `SwitchPipeline` refused: nothing moved and the old run goes on.
    Refused(SwitchPipelineError),
    /// The old run ended in this state rather than Completed; the session is Opened with no
    /// current run.
    Drained(RunState),
    /// The new run did not start; the session was closed.
    Start(OpenError),
}

impl fmt::Display for SwitchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(error) => write!(f, "the session does not switch: {error}"),
            Self::Drained(state) => write!(f, "the old run ended {state:?}, not Completed"),
            Self::Start(error) => write!(f, "the new run did not start: {error}"),
        }
    }
}

impl Error for SwitchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Refused(error) => Some(error),
            Self::Drained(_) => None,
            Self::Start(error) => Some(error),
        }
    }
}

impl<P: SessionPayload> LiveSession<P> {
    /// Switches the session to the pipeline `planned`, planned in `declaration`, in `mode`; see
    /// the module documentation. Answers the new run's driver.
    ///
    /// # Errors
    ///
    /// [`SwitchError::Refused`] when `declaration` holds no such pipeline, checked before
    /// anything moves, or when `SwitchPipeline` refuses: a pipeline not
    /// Planned, another mode, a session that is not Ready. [`SwitchError::Drained`] when the old
    /// run did not complete. [`SwitchError::Start`] when the new run did not start.
    pub async fn switch(
        &mut self,
        declaration: &Declaration,
        planned: &PlannedPipeline,
        mode: InteractionMode,
        implement: impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<P>>,
    ) -> Result<SessionDriver<P>, SwitchError> {
        let pipeline_id = planned.pipeline_id();
        if declaration.pipeline(pipeline_id).is_none() {
            return Err(SwitchError::Refused(SwitchPipelineError::UnknownPipeline(
                PipelineNotFound { pipeline_id },
            )));
        }
        let switched = lock(&self.shared.sessions)
            .switch_pipeline(
                &Planned {
                    declaration,
                    run: None,
                },
                SwitchPipeline {
                    session_id: self.shared.session_id,
                    pipeline_id: planned.pipeline_id(),
                    mode,
                },
            )
            .map_err(SwitchError::Refused)?;
        self.end_turn(false);
        self.shared.leave();
        self.shared.record(SessionEvent::PipelineSwitched(switched));
        for ingress in &self.ingresses {
            let stop = self.control(&ingress.name, ControlKind::Stop);
            let _ = ingress.endpoint.send(stop).await;
        }
        self.ingresses.clear();
        let left = future::poll_fn(|cx| self.poll_drained(cx)).await;
        if left != RunState::Completed {
            return Err(SwitchError::Drained(left));
        }

        let (mut controller, run_id) = run_session(&self.shared, declaration, planned, implement)
            .await
            .map_err(SwitchError::Start)?;
        let (ingresses, egresses) = crate::live::outlets(controller.exposed());
        self.run_id = run_id;
        self.ingresses = ingresses;
        self.egresses = egresses;
        self.requests = controller.requests();
        self.queues = controller.queues();
        self.exposure_queues = controller.exposure_queues();
        Ok(SessionDriver::new(
            controller,
            std::sync::Arc::clone(&self.shared),
        ))
    }

    /// Receives what the old run's egresses still hold, queueing each event among the session's
    /// own, until every egress has ended; then how the old run ended, once it has.
    fn poll_drained(&mut self, cx: &mut Context<'_>) -> Poll<RunState> {
        loop {
            let mut progressed = false;
            let mut index = 0;
            while index < self.egresses.len() {
                let delivers = self.egresses[index].delivers;
                let polled = self.egresses[index].endpoint.recv().as_mut().poll(cx);
                match polled {
                    Poll::Ready(Some(envelope)) => {
                        progressed = true;
                        if let Some(event) = self.read(&envelope, delivers) {
                            self.shared.record(event);
                        }
                    }
                    Poll::Ready(None) => {
                        progressed = true;
                        drop(self.egresses.remove(index));
                    }
                    Poll::Pending => index += 1,
                }
            }
            if self.egresses.is_empty() {
                return self.shared.poll_left(cx);
            }
            if !progressed {
                return Poll::Pending;
            }
        }
    }
}
