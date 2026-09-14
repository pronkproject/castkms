// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::{
    sync::Arc,
    time::{
        delay::fsleep,
        Delta,
        Instant,
        Monotonic, //
    },
    types::ScopeGuard, //
};
use core::sync::atomic::{
    AtomicBool,
    AtomicUsize,
    Ordering, //
};

struct State {
    release: AtomicBool,
    started: AtomicUsize,
    finished: AtomicUsize,
    dropped: AtomicUsize,
}

impl State {
    fn new(release: bool) -> Result<Arc<Self>> {
        Ok(Arc::new(
            Self {
                release: AtomicBool::new(release),
                started: AtomicUsize::new(0),
                finished: AtomicUsize::new(0),
                dropped: AtomicUsize::new(0),
            },
            GFP_KERNEL,
        )?)
    }
}

struct Owned(Arc<State>);

impl Drop for Owned {
    fn drop(&mut self) {
        self.0.dropped.fetch_add(1, Ordering::Release);
    }
}

// SAFETY: The callback, payload destructor and dependencies are built into LocalModule.
#[vtable]
unsafe impl Delivery for Owned {
    fn run(self) {
        self.0.started.fetch_add(1, Ordering::Release);
        let start = Instant::<Monotonic>::now();
        while !self.0.release.load(Ordering::Acquire)
            && start.elapsed() < Delta::from_millis(5000)
        {
            fsleep(Delta::from_millis(1));
        }
        self.0.finished.fetch_add(1, Ordering::Release);
    }
}

fn wait_for(counter: &AtomicUsize, expected: usize) -> Result {
    let start = Instant::<Monotonic>::now();
    while counter.load(Ordering::Acquire) < expected
        && start.elapsed() < Delta::from_millis(2000)
    {
        fsleep(Delta::from_millis(1));
    }
    if counter.load(Ordering::Acquire) == expected {
        Ok(())
    } else {
        Err(ETIMEDOUT)
    }
}

#[kunit_tests(rust_drm_capture_delivery)]
mod cases {
    use super::*;

    #[test]
    fn native_rejection_does_not_invoke_an_invalid_callback() {
        // SAFETY: Native dispatch validates a missing table without retaining any payload.
        let result = unsafe {
            bindings::drm_capture_delivery_submit(core::ptr::null(), core::ptr::null_mut())
        };
        assert_eq!(result, EINVAL.to_errno());
    }

    #[test]
    fn detached_delivery_consumes_and_destroys_its_owned_payload() -> Result {
        let state = State::new(true)?;
        submit(Owned(state.clone()))?;
        wait_for(&state.dropped, 1)?;
        assert_eq!(state.started.load(Ordering::Acquire), 1);
        assert_eq!(state.finished.load(Ordering::Acquire), 1);
        Ok(())
    }

    #[test]
    fn a_blocked_delivery_does_not_stop_an_independent_one() -> Result {
        let blocked = State::new(false)?;
        let _release = ScopeGuard::new(|| blocked.release.store(true, Ordering::Release));
        submit(Owned(blocked.clone()))?;
        wait_for(&blocked.started, 1)?;
        let other = State::new(true)?;
        submit(Owned(other.clone()))?;
        wait_for(&other.dropped, 1)?;
        assert_eq!(blocked.finished.load(Ordering::Acquire), 0);
        blocked.release.store(true, Ordering::Release);
        wait_for(&blocked.dropped, 1)
    }

    #[test]
    fn bounded_admission_drops_rejected_payloads_without_running_them() -> Result {
        let state = State::new(false)?;
        let _release = ScopeGuard::new(|| state.release.store(true, Ordering::Release));
        let mut admitted = 0;
        let mut rejected = false;
        for _ in 0..128 {
            match submit(Owned(state.clone())) {
                Ok(()) => admitted += 1,
                Err(EAGAIN) => {
                    rejected = true;
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        assert!(rejected);
        assert!(admitted > 0);
        assert_eq!(state.dropped.load(Ordering::Acquire), 1);
        state.release.store(true, Ordering::Release);
        wait_for(&state.dropped, admitted + 1)?;
        assert_eq!(state.started.load(Ordering::Acquire), admitted);
        assert_eq!(state.finished.load(Ordering::Acquire), admitted);
        Ok(())
    }
}
