// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use core::sync::atomic::{
    AtomicBool,
    Ordering, //
};
use kernel::{
    sync::Completion,
    workqueue::{
        self,
        impl_has_work,
        new_work,
        Work,
        WorkItem, //
    }, //
};

#[pin_data]
struct Closer {
    #[pin]
    work: Work<Self>,
    #[pin]
    entered: Completion,
    configuration: Arc<Configuration>,
    closed: AtomicBool,
}

impl_has_work! {
    impl HasWork<Self> for Closer { self.work }
}

impl Closer {
    fn new(configuration: Arc<Configuration>) -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                work <- new_work!("castkms-host-close-test"),
                entered <- Completion::new(),
                configuration,
                closed: AtomicBool::new(false),
            }),
            GFP_KERNEL,
        )
    }
}

impl WorkItem for Closer {
    type Pointer = Arc<Self>;

    fn run(closer: Arc<Self>) {
        closer.entered.complete_all();
        closer.configuration.close();
        closer.closed.store(true, Ordering::Release);
    }
}

#[kunit_tests(rust_castkms_host_configuration_locks)]
mod cases {
    use super::*;

    #[test]
    fn shutdown_waits_for_lifecycle_operations_without_locking_handle_access() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let owner = Owner::new(output)?;
        let configuration = owner.configuration();
        let closer = Closer::new(configuration.clone())?;

        let change = configuration.lifecycle.lock();
        let queued = workqueue::system_dfl().enqueue(closer.clone()).is_ok();
        if queued {
            closer.entered.wait_for_completion();
        }
        let closed_during_change = closer.closed.load(Ordering::Acquire);
        let unconfigured = matches!(configuration.current(), Err(EAGAIN));
        // Release the worker and join it before any assertion can abort the test.
        drop(change);
        closer.work.flush();
        assert!(queued);
        assert!(!closed_during_change);
        assert!(unconfigured);
        assert!(closer.closed.load(Ordering::Acquire));
        assert!(matches!(configuration.current(), Err(ENODEV)));
        owner.close();
        Ok(())
    }
}
