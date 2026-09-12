// SPDX-License-Identifier: GPL-2.0-only

//! One coalescing host worker, owned and drained independently of DRM registration.

use super::{
    compose::{
        self,
        Completed, //
    },
    pool::Pool, //
};
use crate::{
    output::Output,
    scene::Scene, //
};
use kernel::{
    prelude::*,
    sync::{
        Arc,
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
#[expect(dead_code)]
pub(crate) enum Outcome {
    Image(Completed),
    Blank,
    Failed(Error),
}

struct State {
    closed: bool,
    outcome: Option<Outcome>,
}

#[pin_data]
struct Worker {
    #[pin]
    work: Work<Self>,
    #[pin]
    state: Mutex<State>,
    output: Arc<Output<Scene>>,
    pool: Arc<Pool>,
}

impl_has_work! {
    impl HasWork<Self> for Worker { self.work }
}

impl WorkItem for Worker {
    type Pointer = Arc<Self>;

    fn run(worker: Arc<Self>) {
        if worker.state.lock().closed {
            return;
        }
        let outcome = match compose::current(&worker.output, &worker.pool) {
            Ok(Some(image)) => Outcome::Image(image),
            Ok(None) => Outcome::Blank,
            Err(error) => Outcome::Failed(error),
        };
        let retired = {
            let mut state = worker.state.lock();
            if state.closed {
                Some(outcome)
            } else {
                state.outcome.replace(outcome)
            }
        };
        drop(retired);
    }
}

/// Registration's unique owner of a worker and its bounded private storage.
///
/// Dropping the owner closes request admission, drains queued work and releases cached
/// images. Registration must close or drop the owner before tearing down the DRM device.
/// Worker callbacks never own this shutdown object.
/// No claim is taken by queueing; the callback chooses the then-current scene.
#[expect(dead_code)]
pub(crate) struct Owner {
    worker: Arc<Worker>,
}

#[expect(dead_code)]
impl Owner {
    pub(crate) fn new(output: Arc<Output<Scene>>, pool: Arc<Pool>) -> Result<Self> {
        let worker = Arc::pin_init(
            pin_init!(Worker {
                work <- new_work!("castkms-host-compose"),
                state <- kernel::new_mutex!(State { closed: false, outcome: None }),
                output,
                pool,
            }),
            GFP_KERNEL,
        )?;
        Ok(Self { worker })
    }

    /// Request a fresh private image, coalescing requests already queued on the same work.
    ///
    /// Call from sleepable context: request admission takes the worker's mutex.
    pub(crate) fn request(&self) -> Result {
        let state = self.worker.state.lock();
        if state.closed {
            return Err(ENODEV);
        }
        let _queued = workqueue::system_dfl().enqueue(self.worker.clone());
        Ok(())
    }

    pub(crate) fn take_outcome(&self) -> Option<Outcome> {
        self.worker.state.lock().outcome.take()
    }

    /// Drain the current request; callers must exclude concurrent requests if they need idle.
    pub(crate) fn flush(&self) {
        self.worker.work.flush();
    }

    /// Stop new requests and drain work outside locks needed for mapping or source retirement.
    pub(crate) fn close(&self) {
        let retired = {
            let mut state = self.worker.state.lock();
            state.closed = true;
            state.outcome.take()
        };
        drop(retired);
        self.worker.work.flush();
        self.worker.pool.close();
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.close();
    }
}
