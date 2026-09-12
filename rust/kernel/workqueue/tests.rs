// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::sync::atomic::{
    Atomic,
    Relaxed, //
};

#[pin_data]
struct Counter {
    #[pin]
    work: Work<Self>,
    count: Atomic<u32>,
}

impl_has_work! {
    impl HasWork<Self> for Counter { self.work }
}

impl Counter {
    fn new() -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                work <- new_work!("rust-work-flush-test"),
                count: Atomic::new(0),
            }),
            GFP_KERNEL,
        )
    }
}

impl WorkItem for Counter {
    type Pointer = Arc<Self>;

    fn run(counter: Arc<Self>) {
        counter.count.fetch_add(1, Relaxed);
    }
}

#[kunit_tests(rust_workqueue_flush)]
mod cases {
    use super::*;

    #[test]
    fn unqueued_work_needs_no_wait() -> Result {
        let counter = Counter::new()?;
        assert!(!counter.work.flush());
        assert_eq!(counter.count.load(Relaxed), 0);
        Ok(())
    }

    #[test]
    fn flushing_finishes_each_queued_instance() -> Result {
        let counter = Counter::new()?;
        for expected in 1..=16 {
            let queued = system_dfl().enqueue(counter.clone());
            counter.work.flush();
            assert!(queued.is_ok());
            assert_eq!(counter.count.load(Relaxed), expected);
            assert!(!counter.work.flush());
        }
        Ok(())
    }
}
