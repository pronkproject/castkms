// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::{
    dma_fence::testing::ManualFence,
    sync::{Arc, Mutex},
    time::{delay::fsleep, Delta, Instant, Monotonic},
    workqueue::{self, impl_has_work, new_work, Work, WorkItem},
};
use core::sync::atomic::{AtomicUsize, Ordering};

struct Counts {
    retired: AtomicUsize,
    dropped: AtomicUsize,
}

impl Counts {
    fn new() -> Result<Arc<Self>> {
        Ok(Arc::new(
            Self {
                retired: AtomicUsize::new(0),
                dropped: AtomicUsize::new(0),
            },
            GFP_KERNEL,
        )?)
    }

    fn wait(&self, expected: usize) -> Result {
        let start = Instant::<Monotonic>::now();
        while self.dropped.load(Ordering::Acquire) < expected
            && start.elapsed() < Delta::from_millis(2000)
        {
            fsleep(Delta::from_millis(1));
        }
        assert_eq!(self.retired.load(Ordering::Acquire), expected);
        assert_eq!(self.dropped.load(Ordering::Acquire), expected);
        Ok(())
    }
}

struct Payload(Arc<Counts>);

impl Drop for Payload {
    fn drop(&mut self) {
        self.0.dropped.fetch_add(1, Ordering::Release);
    }
}

// SAFETY: Callback and payload destruction are built into LocalModule.
#[vtable]
unsafe impl Retire for Payload {
    fn retire(self) {
        self.0.retired.fetch_add(1, Ordering::Release);
    }
}

#[pin_data]
struct Signaler {
    #[pin]
    work: Work<Self>,
    #[pin]
    completion: Mutex<Option<ManualFence>>,
}

impl_has_work! {
    impl HasWork<Self> for Signaler { self.work }
}

impl WorkItem for Signaler {
    type Pointer = Arc<Self>;

    fn run(signaler: Arc<Self>) {
        let completion = signaler.completion.lock().take();
        if let Some(mut completion) = completion {
            let _ = completion.complete(Ok(()));
        }
    }
}

#[kunit_tests(rust_dma_fence_retirement)]
mod cases {
    use super::*;

    #[test]
    fn unused_owner_releases_synchronously() -> Result {
        let counts = Counts::new()?;
        let owner = Retirement::new(Payload(counts.clone()))?;
        assert_eq!(counts.dropped.load(Ordering::Acquire), 0);
        drop(owner);
        assert_eq!(counts.retired.load(Ordering::Acquire), 1);
        assert_eq!(counts.dropped.load(Ordering::Acquire), 1);
        Ok(())
    }

    #[test]
    fn pending_completion_retains_payload_without_a_caller() -> Result {
        for result in [Ok(()), Err(EIO), Err(EAGAIN)] {
            let counts = Counts::new()?;
            let mut completion = ManualFence::new()?;
            Retirement::new(Payload(counts.clone()))?.submit(&completion.fence());
            fsleep(Delta::from_millis(5));
            assert_eq!(counts.dropped.load(Ordering::Acquire), 0);
            completion.complete(result)?;
            counts.wait(1)?;
        }
        Ok(())
    }

    #[test]
    fn already_signaled_completion_releases_once() -> Result {
        for result in [Ok(()), Err(EIO)] {
            let counts = Counts::new()?;
            let mut completion = ManualFence::new()?;
            completion.complete(result)?;
            Retirement::new(Payload(counts.clone()))?.submit(&completion.fence());
            counts.wait(1)?;
        }
        Ok(())
    }

    #[test]
    fn one_pending_fence_does_not_delay_an_independent_cleanup() -> Result {
        let pending = Counts::new()?;
        let completed = Counts::new()?;
        let mut first = ManualFence::new()?;
        let mut second = ManualFence::new()?;
        Retirement::new(Payload(pending.clone()))?.submit(&first.fence());
        Retirement::new(Payload(completed.clone()))?.submit(&second.fence());
        second.complete(Ok(()))?;
        completed.wait(1)?;
        assert_eq!(pending.dropped.load(Ordering::Acquire), 0);
        first.complete(Err(EIO))?;
        pending.wait(1)
    }

    #[test]
    fn signaling_before_or_after_registration_releases_exactly_once() -> Result {
        let counts = Counts::new()?;
        for index in 0..32 {
            let mut completion = ManualFence::new()?;
            let owner = Retirement::new(Payload(counts.clone()))?;
            if index % 2 == 0 {
                completion.complete(Ok(()))?;
                owner.submit(&completion.fence());
            } else {
                owner.submit(&completion.fence());
                completion.complete(Ok(()))?;
            }
        }
        counts.wait(32)
    }

    #[test]
    fn concurrent_signaling_and_callback_registration_retire_once() -> Result {
        let counts = Counts::new()?;
        for _ in 0..256 {
            let completion = ManualFence::new()?;
            let fence = completion.fence();
            let owner = Retirement::new(Payload(counts.clone()))?;
            let signaler = Arc::pin_init(
                pin_init!(Signaler {
                    work <- new_work!("dma-fence-retirement-signal"),
                    completion <- crate::new_mutex!(Some(completion)),
                }),
                GFP_KERNEL,
            )?;
            assert!(workqueue::system_dfl().enqueue(signaler).is_ok());
            owner.submit(&fence);
        }
        counts.wait(256)
    }
}
