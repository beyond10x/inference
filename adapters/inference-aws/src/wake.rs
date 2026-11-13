//! Waking the async form of a session: an [`Alarm`] wakes it at a deadline from a clock runtime
//! of this crate's own, and a [`WakerSlot`] when the session's tasks queue an arrival.

/// The clock every [`Alarm`] of the crate runs on: a runtime with one worker thread and time
/// only, built on first use; `None` when it could not be built.
fn clock() -> Option<&'static tokio::runtime::Handle> {
    static CLOCK: std::sync::OnceLock<Option<tokio::runtime::Runtime>> = std::sync::OnceLock::new();
    CLOCK
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("inference-aws-clock")
                .enable_time()
                .build()
                .ok()
        })
        .as_ref()
        .map(tokio::runtime::Runtime::handle)
}

/// Wakes a task at a deadline, on the crate's clock runtime, whatever polls it.
#[derive(Default)]
pub(crate) struct Alarm {
    sleep: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
}

impl std::fmt::Debug for Alarm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Alarm")
            .field("set", &self.sleep.is_some())
            .finish()
    }
}

impl Alarm {
    /// Ready once `at` has passed; until then `Pending`, with `cx` woken at `at`. Without a
    /// clock it wakes `cx` at once, so the caller polls again rather than sleeping past `at`.
    pub(crate) fn poll_at(
        &mut self,
        at: std::time::Instant,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        use std::future::Future;

        if std::time::Instant::now() >= at {
            return std::task::Poll::Ready(());
        }
        let deadline = tokio::time::Instant::from_std(at);
        if let Some(sleep) = self.sleep.as_mut() {
            if sleep.deadline() != deadline {
                sleep.as_mut().reset(deadline);
            }
        } else if let Some(clock) = clock() {
            let _entered = clock.enter();
            self.sleep = Some(Box::pin(tokio::time::sleep_until(deadline)));
        }
        match self.sleep.as_mut() {
            Some(sleep) => sleep.as_mut().poll(cx),
            None => {
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
        }
    }
}

/// The waker of the task that waits on a queue a thread fills; the thread wakes it after each
/// message it queued and when it lets go of the queue.
#[derive(Debug, Default)]
pub(crate) struct WakerSlot(std::sync::Mutex<Option<std::task::Waker>>);

impl WakerSlot {
    /// Keeps `waker` to wake, in place of the one kept before.
    pub(crate) fn register(&self, waker: &std::task::Waker) {
        let mut kept = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !kept.as_ref().is_some_and(|kept| kept.will_wake(waker)) {
            *kept = Some(waker.clone());
        }
    }

    /// Wakes the waker kept, if there is one.
    pub(crate) fn wake(&self) {
        let kept = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(waker) = kept {
            waker.wake();
        }
    }
}

/// When a session that waits for its provider next has something to check: the earlier of
/// `quiet`, when a finishing response completes, and the end of the liveness bound, `bound`
/// when it lies ahead, or a whole `limit` from `now` when it does not run.
pub(crate) fn due(
    quiet: Option<std::time::Instant>,
    bound: Option<std::time::Instant>,
    limit: std::time::Duration,
    now: std::time::Instant,
) -> Option<std::time::Instant> {
    let bound = bound
        .filter(|bound| *bound > now)
        .or_else(|| now.checked_add(limit));
    match (quiet, bound) {
        (Some(quiet), Some(bound)) => Some(quiet.min(bound)),
        (quiet, bound) => quiet.or(bound),
    }
}
