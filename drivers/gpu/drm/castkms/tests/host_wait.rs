// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::host_compositor::{
    layout::Layout,
    pool::Pool,
    worker::{
        Handle,
        Outcome,
        Owner, //
    }, //
};
use core::sync::atomic::{
    AtomicBool,
    Ordering, //
};
use kernel::{
    sync::{
        Arc,
        Completion, //
    },
    workqueue::{
        self,
        impl_has_work,
        new_work,
        Work,
        WorkItem, //
    }, //
};

#[pin_data]
struct Waiter {
    #[pin]
    work: Work<Self>,
    #[pin]
    started: Completion,
    handle: Handle,
    closed: AtomicBool,
}

impl_has_work! {
    impl HasWork<Self> for Waiter { self.work }
}

impl Waiter {
    fn new(handle: Handle) -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                work <- new_work!("castkms-host-wait-test"),
                started <- Completion::new(),
                handle,
                closed: AtomicBool::new(false),
            }),
            GFP_KERNEL,
        )
    }
}

impl WorkItem for Waiter {
    type Pointer = Arc<Self>;

    fn run(waiter: Arc<Self>) {
        waiter.started.complete_all();
        waiter.closed.store(
            matches!(waiter.handle.wait_for_outcome(), Err(ENODEV)),
            Ordering::Release,
        );
    }
}

#[kunit_tests(rust_castkms_host_wait)]
mod cases {
    use super::*;

    #[test]
    fn a_consumer_waits_for_images_without_draining_the_worker() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let layout = Layout::new(640, 480)?;
        let pool = Pool::new(fixture.drm.device(), &fixture.host_budget, layout)?;
        let _remaining = fixture
            .host_budget
            .reserve(16 * 1024 * 1024 - 2 * layout.size())?;
        let owner = Owner::new(
            fixture.drm.device().output.clone(),
            fixture.drm.device().execution.clone(),
            pool,
        )?;
        let handle = owner.handle();
        let mut previous: Option<Arc<crate::host_compositor::compose::Completed>> = None;
        for _ in 0..64 {
            fixture.select(&fb, false, 0)?;
            handle.request()?;
            let Outcome::Image(image) = handle.wait_for_outcome()? else {
                return Err(EINVAL);
            };
            if let Some(ref old) = previous {
                check(old.content_serial() != image.content_serial())?;
            }
            check(matches!(fixture.host_budget.reserve(1), Err(EBUSY)))?;
            previous = Some(image);
        }
        Ok(())
    }

    #[test]
    fn consumer_waits_distinguish_failed_work_from_shutdown() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let handle = fixture
            .drm
            .device()
            .host
            .configure(fixture.drm.device(), Layout::new(3, 2)?)?;
        handle.request()?;
        check(matches!(
            handle.wait_for_outcome()?,
            Outcome::Failed(EINVAL)
        ))?;
        fixture.state.close();
        check(matches!(handle.wait_for_outcome(), Err(ENODEV)))?;
        Ok(())
    }

    #[test]
    fn an_outcome_published_before_waiting_is_not_lost() -> Result {
        let fixture = Fixture::new()?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(3, 2)?,
        )?;
        let owner = Owner::new(
            fixture.drm.device().output.clone(),
            fixture.drm.device().execution.clone(),
            pool,
        )?;
        let handle = owner.handle();
        handle.request()?;
        owner.flush();
        check(matches!(handle.wait_for_outcome()?, Outcome::NoScene))?;
        check(handle.take_outcome().is_none())?;
        Ok(())
    }

    #[test]
    fn shutdown_notifies_all_consumer_handles() -> Result {
        let fixture = Fixture::new()?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(3, 2)?,
        )?;
        let owner = Owner::new(
            fixture.drm.device().output.clone(),
            fixture.drm.device().execution.clone(),
            pool,
        )?;
        // Prepare every waiter before queueing work that needs teardown.
        let waiters = [
            Waiter::new(owner.handle())?,
            Waiter::new(owner.handle())?,
            Waiter::new(owner.handle())?,
        ];
        let mut all_queued = true;
        for waiter in &waiters {
            let queued = workqueue::system_dfl().enqueue(waiter.clone()).is_ok();
            all_queued &= queued;
            if queued {
                waiter.started.wait_for_completion();
            }
        }
        owner.close();
        for waiter in &waiters {
            waiter.work.flush();
        }
        check(all_queued)?;
        check(
            waiters
                .iter()
                .all(|waiter| waiter.closed.load(Ordering::Acquire)),
        )?;
        Ok(())
    }
}
