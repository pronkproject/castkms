// SPDX-License-Identifier: GPL-2.0-only

//! Serialized queue access with construction and destruction outside the queue lock.

use crate::capture::{
    budget::STREAM_LIMIT,
    host_queue::Queue, //
};
use kernel::{
    drm::capture::Resources,
    prelude::*,
    sync::{
        Arc,
        Mutex, //
    }, //
};

#[pin_data]
struct Shared {
    #[pin]
    queues: Mutex<Option<Resources<Queue>>>,
}

/// Own the namespace; shared operations borrow payloads without transferring registrations.
///
/// Creating and removing names requires an exclusive owner borrow. Queue access is
/// serialized separately so completion work need not borrow the whole capture client.
pub(super) struct Streams {
    shared: Arc<Shared>,
}

impl Streams {
    pub(super) fn new() -> Result<Self> {
        let queues = Resources::new(STREAM_LIMIT)?;
        Ok(Self {
            shared: Arc::pin_init(
                pin_init!(Shared {
                    queues <- kernel::new_mutex!(Some(queues)),
                }),
                GFP_KERNEL,
            )?,
        })
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
                Some(queues) => queues.insert(id, || created.take().ok_or(EIO)),
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
        operation((*queues).as_mut().ok_or(ESHUTDOWN)?.get_mut(id)?)
    }

    pub(super) fn remove(&mut self, id: u64) -> Result {
        let removed = {
            let mut queues = self.shared.queues.lock();
            (*queues).as_mut().ok_or(ESHUTDOWN)?.remove(id)?
        };
        drop(removed);
        Ok(())
    }
}

impl Drop for Streams {
    fn drop(&mut self) {
        let retired = self.shared.queues.lock().take();
        drop(retired);
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_capture_client_queue_ownership)]
mod tests {
    use super::*;

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
