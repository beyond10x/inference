//! The executor contract: [`ToolExecutor`] executes one started call under a deadline and a
//! [`CancellationSignal`], and returns its result or a typed [`ToolFailure`].
//!
//! The trait is dyn-compatible and returns a boxed [`Future`] of the standard library, as the
//! endpoints of `inference-core` do, so that it commits a caller to no asynchronous executor.
//! Attempting a started call again is its caller's, by the [`RetryPolicy`] of the call's
//! side-effect class, each attempt one [`ToolExecutor::execute`] of the same call; attempts are
//! not lifecycle transitions. A refused result is not undone, since stopping a model rolls back no
//! completed write or external action.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::{Json, SideEffectClass, ToolCall, ToolFailure, ToolFailureKind};

/// The future [`ToolExecutor::execute`] returns: boxed, `Send`, and borrowing the executor and
/// the call for `'a`.
pub type ExecutionFuture<'a> = Pin<Box<dyn Future<Output = Result<Json, ToolFailure>> + Send + 'a>>;

/// Executes tool calls. An implementation is an adapter crate's; this crate carries none.
pub trait ToolExecutor: Send + Sync {
    /// Executes `call`, which the caller has started (`inference.tools.StartToolCall`), with its
    /// arguments, before `deadline`, stopping when `cancellation` is cancelled.
    ///
    /// The result completes the call (`inference.tools.CompleteToolCall`); a failure fails it
    /// (`inference.tools.FailToolCall`), unless the caller's [`RetryPolicy`] attempts it again.
    /// Every attempt of one call presents the same call, so an implementation deduplicates on
    /// its `call_id`.
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

/// How often the caller of a [`ToolExecutor`] attempts one started call, by the side-effect class
/// the call carries (`docs/design/inference.md` 851, 1007).
///
/// A `read_only` or `idempotent_write` call that failed transiently is attempted again, up to the
/// policy's limit; a `non_idempotent_write` is attempted once, whatever the limit, and a failure
/// leaves it Failed for reconciliation (852). Every attempt presents the same call, so its
/// `call_id` is the key a tool deduplicates on (849, 1215); none is an exactly-once claim. A
/// failure is transient when the executor answered `execution_error`: `deadline_exceeded` spent
/// the call's one deadline and `cancelled` was asked for, so neither is attempted again. The
/// default policy, like a limit of 0, attempts every call once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RetryPolicy {
    limit: u32,
}

impl RetryPolicy {
    /// A policy of at most `attempts` attempts of a call that may be attempted again.
    #[must_use]
    pub const fn up_to(attempts: u32) -> Self {
        Self { limit: attempts }
    }

    /// How many attempts, at least one, a call of `class` gets.
    #[must_use]
    pub const fn attempts(self, class: SideEffectClass) -> u32 {
        match class {
            SideEffectClass::NonIdempotentWrite => 1,
            SideEffectClass::ReadOnly | SideEffectClass::IdempotentWrite => {
                if self.limit == 0 {
                    1
                } else {
                    self.limit
                }
            }
        }
    }

    /// Whether `failure` is transient, so that a call with attempts left is attempted again.
    #[must_use]
    pub const fn retries(self, failure: &ToolFailure) -> bool {
        matches!(failure.kind, ToolFailureKind::ExecutionError)
    }
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
