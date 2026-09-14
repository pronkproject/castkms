// SPDX-License-Identifier: GPL-2.0-only

//! Delayed callbacks retain normal execution ownership when their timers are expedited.

use super::*;

#[pin_data]
struct DelayedCounter {
    #[pin]
    work: DelayedWork<Self>,
    count: Atomic<u32>,
}

impl_has_delayed_work! {
    impl HasDelayedWork<Self> for DelayedCounter { self.work }
}

impl DelayedCounter {
    fn new() -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                work <- new_delayed_work!("rust-delayed-work-flush-test"),
                count: Atomic::new(0),
            }),
            GFP_KERNEL,
        )
    }
}

impl WorkItem for DelayedCounter {
    type Pointer = Arc<Self>;

    fn run(counter: Arc<Self>) {
        counter.count.fetch_add(1, Relaxed);
    }
}

#[kunit_tests(rust_workqueue_delayed_flush)]
mod cases {
    use super::*;

    #[test]
    fn unqueued_delayed_work_needs_no_wait() -> Result {
        let counter = DelayedCounter::new()?;
        assert!(!counter.work.flush());
        assert_eq!(counter.count.load(Relaxed), 0);
        Ok(())
    }

    #[test]
    fn flushing_expedites_the_timer_and_finishes_the_callback() -> Result {
        let counter = DelayedCounter::new()?;
        for expected in 1..=16 {
            let queued = system_dfl()
                .enqueue_delayed(counter.clone(), crate::time::msecs_to_jiffies(60_000));
            counter.work.flush();
            assert!(queued.is_ok());
            assert_eq!(counter.count.load(Relaxed), expected);
            assert!(!counter.work.flush());
        }
        Ok(())
    }
}
