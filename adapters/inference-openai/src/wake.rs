//! Waiting without the caller's runtime. The blocking forms park the calling thread on a future
//! and never enter a runtime, so they neither panic on an async worker nor need one; the async
//! forms are woken by what they poll, and an [`Alarm`] wakes them at a deadline from a clock
//! runtime of this crate's own.

/// Unparks the thread that waits on a future when the future is woken.
struct Unpark(std::thread::Thread);

impl std::task::Wake for Unpark {
    fn wake(self: std::sync::Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &std::sync::Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` on the calling thread until it is ready or `until` has passed, parking the
/// thread between polls: `None` when `until` passed first. No runtime is entered, and the
/// cooperative budget of a task the thread may be polling does not hold the future back.
pub(crate) fn block_on_until<F: std::future::Future>(
    future: F,
    until: Option<std::time::Instant>,
) -> Option<F::Output> {
    let mut future = std::pin::pin!(tokio::task::unconstrained(future));
    let waker = std::task::Waker::from(std::sync::Arc::new(Unpark(std::thread::current())));
    let mut cx = std::task::Context::from_waker(&waker);
    loop {
        if let std::task::Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return Some(output);
        }
        match until {
            None => std::thread::park(),
            Some(until) => {
                let now = std::time::Instant::now();
                if now >= until {
                    return None;
                }
                std::thread::park_timeout(until - now);
            }
        }
    }
}

/// Polls `future` on the calling thread until it is ready; see [`block_on_until`].
pub(crate) fn block_on<F: std::future::Future>(future: F) -> F::Output {
    match block_on_until(future, None) {
        Some(output) => output,
        None => unreachable!("a wait without a deadline ends only when the future is ready"),
    }
}

/// The clock every [`Alarm`] of the crate runs on: a runtime with one worker thread and time
/// only, built on first use; `None` when it could not be built.
#[cfg(feature = "duplex")]
fn clock() -> Option<&'static tokio::runtime::Handle> {
    static CLOCK: std::sync::OnceLock<Option<tokio::runtime::Runtime>> = std::sync::OnceLock::new();
    CLOCK
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("inference-openai-clock")
                .enable_time()
                .build()
                .ok()
        })
        .as_ref()
        .map(tokio::runtime::Runtime::handle)
}

/// Wakes a task at a deadline, on the crate's clock runtime, whatever polls it.
#[cfg(feature = "duplex")]
#[derive(Default)]
pub(crate) struct Alarm {
    sleep: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
}

#[cfg(feature = "duplex")]
impl std::fmt::Debug for Alarm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Alarm")
            .field("set", &self.sleep.is_some())
            .finish()
    }
}

#[cfg(feature = "duplex")]
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

/// When a session that waits for its provider next has something to check: the earlier of
/// `quiet`, when a finishing response completes, and the end of the liveness bound, `bound`
/// when it lies ahead, or a whole `limit` from `now` when it does not run.
#[cfg(feature = "duplex")]
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
