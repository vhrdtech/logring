//! Helpers for tests and the fuzz harness (`std`).

use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, Waker};

/// Nothing on a simulated medium ever waits: poll until ready.
pub fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = pin!(f);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}
