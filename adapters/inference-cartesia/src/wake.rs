//! Waiting without the caller's runtime: the blocking forms park the calling thread on a future
//! and never enter a runtime, so they neither panic on an async worker nor need one.

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
