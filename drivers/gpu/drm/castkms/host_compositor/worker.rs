// SPDX-License-Identifier: GPL-2.0-only

//! One coalescing host worker, owned and drained independently of DRM registration.

use super::{
    compose::{
        self,
        Completed, //
    },
    pool::Pool, //
};
use crate::Output;
use kernel::{
    prelude::*,
    sync::{
        Arc,
        CondVar,
        Mutex, //
    },
    workqueue::{
        self,
        impl_has_work,
        new_work,
        Work,
        WorkItem, //
    }, //
};

/// A worker result, not a grant-authorized capture completion.
#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
pub(crate) enum Outcome {
    Image(Arc<Completed>),
    Blank,
    Failed(Error),
}

enum State {
    Open {
        outcome: Option<Outcome>,
        last_image: Option<Arc<Completed>>,
    },
    Closed,
}

impl State {
    /// Return replaced storage for destruction outside the worker's result lock.
    fn record(&mut self, next: Outcome) -> (Option<Outcome>, Option<Arc<Completed>>) {
        let Self::Open {
            outcome,
            last_image,
        } = self
        else {
            return (Some(next), None);
        };
        let retired_image = match &next {
            Outcome::Image(image) => last_image.replace(image.clone()),
            Outcome::Blank => last_image.take(),
            Outcome::Failed(_) => None,
        };
        (outcome.replace(next), retired_image)
    }
}

#[pin_data]
struct Worker {
    #[pin]
    work: Work<Self>,
    #[pin]
    state: Mutex<State>,
    #[pin]
    changed: CondVar,
    output: Arc<Output>,
    pool: Arc<Pool>,
}

impl_has_work! {
    impl HasWork<Self> for Worker { self.work }
}

impl WorkItem for Worker {
    type Pointer = Arc<Self>;

    fn run(worker: Arc<Self>) {
        if matches!(*worker.state.lock(), State::Closed) {
            return;
        }
        let outcome = match compose::current(&worker.output, &worker.pool) {
            Ok(Some(image)) => match Arc::new(image, GFP_KERNEL) {
                Ok(image) => Outcome::Image(image),
                Err(error) => Outcome::Failed(error.into()),
            },
            Ok(None) => Outcome::Blank,
            Err(error) => Outcome::Failed(error),
        };
        let retired = worker.state.lock().record(outcome);
        worker.changed.notify_all();
        drop(retired);
    }
}

/// Registration's unique owner of a worker and its bounded private storage.
///
/// Dropping the owner closes request admission, drains queued work and releases cached
/// images. Registration must close or drop the owner before tearing down the DRM device.
/// Worker callbacks never own this shutdown object.
/// No claim is taken by queueing; the callback chooses the then-current scene.
pub(crate) struct Owner {
    worker: Arc<Worker>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Owner {
    pub(crate) fn new(output: Arc<Output>, pool: Arc<Pool>) -> Result<Self> {
        let worker = Arc::pin_init(
            pin_init!(Worker {
                work <- new_work!("castkms-host-compose"),
                state <- kernel::new_mutex!(State::Open {
                    outcome: None,
                    last_image: None,
                }),
                changed <- kernel::sync::new_condvar!(),
                output,
                pool,
            }),
            GFP_KERNEL,
        )?;
        Ok(Self { worker })
    }

    /// Retain request access without sharing responsibility for shutdown.
    pub(crate) fn handle(&self) -> Handle {
        Handle {
            worker: self.worker.clone(),
        }
    }

    /// Drain the current request; callers must exclude concurrent requests if they need idle.
    pub(crate) fn flush(&self) {
        self.worker.work.flush();
    }

    /// Stop new requests and drain work outside locks needed for mapping or source retirement.
    pub(crate) fn close(&self) {
        let retired = core::mem::replace(&mut *self.worker.state.lock(), State::Closed);
        self.worker.changed.notify_all();
        drop(retired);
        self.worker.work.flush();
        self.worker.pool.close();
    }
}

/// Shared access to private composition requests, without ownership of shutdown.
///
/// Handles share one consumable latest result. Dropping a handle does not close the worker;
/// dropping its [`Owner`] closes all surviving handles and drains any accepted request.
/// A handle grants no permission to deliver the resulting pixels to a capture recipient.
#[derive(Clone)]
#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
pub(crate) struct Handle {
    worker: Arc<Worker>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Handle {
    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn flush_for_test(&self) {
        self.worker.work.flush();
    }

    /// Request a fresh private image, coalescing requests already queued on the same work.
    ///
    /// Call from sleepable context: request admission takes the worker's mutex.
    pub(crate) fn request(&self) -> Result {
        let state = self.worker.state.lock();
        if matches!(*state, State::Closed) {
            return Err(ENODEV);
        }
        let _queued = workqueue::system_dfl().enqueue(self.worker.clone());
        Ok(())
    }

    pub(crate) fn take_outcome(&self) -> Option<Outcome> {
        match &mut *self.worker.state.lock() {
            State::Open { outcome, .. } => outcome.take(),
            State::Closed => None,
        }
    }

    /// Wait interruptibly for one shared outcome, or return `ENODEV` on shutdown.
    ///
    /// This does not enqueue work or identify an individual request. Another handle
    /// may consume an available outcome first. Call only from consumer context,
    /// outside modeset, publication, reservation and worker lifecycle locks. Waiting
    /// holds no source claim and is not a source-retirement completion primitive.
    pub(crate) fn wait_for_outcome(&self) -> Result<Outcome> {
        let mut state = self.worker.state.lock();
        loop {
            match &mut *state {
                State::Open { outcome, .. } => {
                    if let Some(outcome) = outcome.take() {
                        return Ok(outcome);
                    }
                }
                State::Closed => return Err(ENODEV),
            }
            if self.worker.changed.wait_interruptible(&mut state) {
                return Err(ERESTARTSYS);
            }
        }
    }

    /// Retain the last complete private image without consuming the latest attempt.
    ///
    /// A failure preserves this historical image; a completed blank or shutdown clears
    /// it. The image may describe an older scene or owner. Callers must independently
    /// establish currentness and recipient authorization before delivering its pixels.
    pub(crate) fn last_image(&self) -> Option<Arc<Completed>> {
        match &*self.worker.state.lock() {
            State::Open { last_image, .. } => last_image.clone(),
            State::Closed => None,
        }
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.close();
    }
}
