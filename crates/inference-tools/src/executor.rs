//! The executor contract: [`ToolExecutor`] executes one started call under a deadline and a
//! [`CancellationSignal`], and returns its result or a typed [`ToolFailure`].
//!
//! The trait is dyn-compatible and returns a boxed [`Future`] of the standard library, as the
//! endpoints of `inference-core` do, so that it commits a caller to no asynchronous executor.
//! Retrying the tool inside one call is the executor's; a refused result is not undone, since
//! stopping a model rolls back no completed write or external action.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::{Json, ToolCall, ToolFailure};

/// The future [`ToolExecutor::execute`] returns: boxed, `Send`, and borrowing the executor and
/// the call for `'a`.
pub type ExecutionFuture<'a> = Pin<Box<dyn Future<Output = Result<Json, ToolFailure>> + Send + 'a>>;

/// Executes tool calls. An implementation is an adapter crate's; this crate carries none.
pub trait ToolExecutor: Send + Sync {
    /// Executes `call`, which the caller has started (`inference.tools.StartToolCall`), with its
    /// arguments, before `deadline`, stopping when `cancellation` is cancelled.
    ///
    /// The result completes the call (`inference.tools.CompleteToolCall`); a failure fails it
    /// (`inference.tools.FailToolCall`).
    ///
    /// # Errors
    ///
    /// [`ToolFailure`] of kind [`DeadlineExceeded`](crate::ToolFailureKind::DeadlineExceeded) when
    /// `deadline` passed, [`Cancelled`](crate::ToolFailureKind::Cancelled) when `cancellation` was
    /// cancelled, and [`ExecutionError`](crate::ToolFailureKind::ExecutionError) when the tool
    /// failed.
    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        deadline: Instant,
        cancellation: &'a CancellationSignal,
    ) -> ExecutionFuture<'a>;
}

/// A cancellation signal shared between the caller that cancels a call and the executor that
/// observes it. Clones share one signal; once cancelled it stays cancelled.
#[derive(Debug, Clone, Default)]
pub struct CancellationSignal {
    cancelled: Arc<AtomicBool>,
}

impl CancellationSignal {
    /// A signal that is not cancelled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancels every clone of the signal.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Whether the signal was cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}
