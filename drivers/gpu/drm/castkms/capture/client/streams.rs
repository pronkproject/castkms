// SPDX-License-Identifier: GPL-2.0-only

//! Serialized queue access with construction and destruction outside the queue lock.

use crate::capture::{
    budget::STREAM_LIMIT,
    client_queue::Queue,
};
use kernel::{
    drm::capture::{
        Readiness,
        Resources, //
    },
    prelude::*,
    sync::{
        aref::ARef,
        Arc,
        Mutex, //
    },
    workqueue::{
        self,
        impl_has_delayed_work,
        new_delayed_work,
        DelayedWork,
        WorkItem, //
    }, //
};

#[pin_data]
struct Shared {
    #[pin]
    work: DelayedWork<Self>,
    #[pin]
    queues: Mutex<Option<Resources<Queue>>>,
    readiness: ARef<Readiness>,
}

impl_has_delayed_work! {
    impl HasDelayedWork<Self> for Shared { self.work }
}

impl Shared {
    /// Publish and schedule under the same exclusion that closes queue access.
    fn refresh(self: &Arc<Self>, queues: &mut Resources<Queue>) {
        let mut pending = false;
        let mut ready = false;
        queues.for_each_mut(|queue| {
            pending |= queue.has_pending();
            ready |= queue.has_results();
        });
        if self.readiness.has_results() != ready {
            self.readiness.update(ready);
        }
        if pending {
            // Retry unfinished dependencies, not a display or media cadence. An existing
            // queued callback already owns progress; a rejected extra Arc drops normally.
            let _ = workqueue::system_dfl().enqueue_delayed(self.clone(), 1);
        }
    }
}

impl WorkItem for Shared {
    type Pointer = Arc<Self>;

    fn run(shared: Arc<Self>) {
        let mut guard = shared.queues.lock();
        let Some(queues) = &mut *guard else {
            return;
        };
        queues.for_each_mut(|queue| {
            queue.advance();
        });
        shared.refresh(queues);
    }
}

/// Own the namespace; shared operations borrow payloads without transferring registrations.
///
/// Creating and removing names requires an exclusive owner borrow. Queue access is
/// serialized separately so completion work need not borrow the whole capture client.
/// A pending attempt schedules retries on an unbound worker, independently of composition.
/// The owner must be dropped outside locks needed by copying or stream destruction.
pub(super) struct Streams {
    shared: Arc<Shared>,
}

impl Streams {
    pub(super) fn new() -> Result<Self> {
        let queues = Resources::new(STREAM_LIMIT)?;
        let readiness = Readiness::new()?;
        Ok(Self {
            shared: Arc::pin_init(
                pin_init!(Shared {
                    work <- new_delayed_work!("castkms-client-completion"),
                    queues <- kernel::new_mutex!(Some(queues)),
                    readiness,
                }),
                GFP_KERNEL,
            )?,
        })
    }

    pub(super) fn readiness(&self) -> &Readiness {
        &self.shared.readiness
    }

    pub(super) fn insert(&mut self, id: u64, create: impl FnOnce() -> Result<Queue>) -> Result {
        self.shared
            .queues
            .lock()
            .as_ref()
            .ok_or(ESHUTDOWN)?
            .check(id)?;
        let mut created = Some(create()?);
        let result = {
            let mut queues = self.shared.queues.lock();
            match &mut *queues {
                Some(queues) => {
                    let result = queues.insert(id, || created.take().ok_or(EIO));
                    self.shared.refresh(queues);
                    result
                }
                None => Err(ESHUTDOWN),
            }
        };
        // Failed validation leaves the constructed stream to retire outside queue exclusion.
        drop(created);
        result
    }

    pub(super) fn with_queue<R>(
        &self,
        id: u64,
        operation: impl FnOnce(&mut Queue) -> Result<R>,
    ) -> Result<R> {
        let mut queues = self.shared.queues.lock();
        let queues = (*queues).as_mut().ok_or(ESHUTDOWN)?;
        let result = operation(queues.get_mut(id)?);
        self.shared.refresh(queues);
        result
    }

    pub(super) fn remove(&mut self, id: u64) -> Result {
        let removed = {
            let mut queues = self.shared.queues.lock();
            let queues = (*queues).as_mut().ok_or(ESHUTDOWN)?;
            queues.get_mut(id)?.try_close()?;
            let removed = queues.remove(id)?;
            self.shared.refresh(queues);
            removed
        };
        drop(removed);
        Ok(())
    }
}

impl Drop for Streams {
    fn drop(&mut self) {
        let retired = {
            let mut queues = self.shared.queues.lock();
            let retired = queues.take();
            self.shared.readiness.update(false);
            retired
        };
        drop(retired);
        // Closing under queue exclusion prevents every observer from requeueing. Detached
        // destination access owns no reference to this work item and is not flushed here.
        // Its payload and module remain retained until its native delivery callback returns.
        self.shared.work.flush();
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_capture_client_queue_ownership)]
mod tests {
    use super::*;

    #[test]
    fn closing_expedites_a_queued_retry_without_rearming_it() -> Result {
        let streams = Streams::new()?;
        let shared = streams.shared.clone();
        let queued = workqueue::system_dfl()
            .enqueue_delayed(shared.clone(), kernel::time::msecs_to_jiffies(60_000));
        drop(streams);
        assert!(queued.is_ok());
        assert!(!shared.work.flush());
        assert!(shared.queues.lock().is_none());
        assert!(!shared.readiness.has_results());
        Ok(())
    }

    #[test]
    fn constructing_a_stream_does_not_hold_queue_exclusion() -> Result {
        let mut streams = Streams::new()?;
        let shared = streams.shared.clone();
        let mut called = false;
        let result = streams.insert(1, || {
            called = true;
            let _queues = shared.queues.try_lock().ok_or(EBUSY)?;
            Err(EIO)
        });
        assert!(called);
        assert_eq!(result, Err(EIO));
        shared.queues.lock().as_ref().ok_or(ESHUTDOWN)?.check(1)?;
        drop(streams);
        assert!(shared.queues.lock().is_none());
        Ok(())
    }
}
