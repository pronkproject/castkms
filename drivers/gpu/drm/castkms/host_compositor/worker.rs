// SPDX-License-Identifier: GPL-2.0-only

//! One coalescing host worker, owned and drained independently of DRM registration.

mod progress;
mod request;

pub(crate) use request::Request;

use progress::Progress;

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
#[derive(Clone)]
pub(crate) enum Outcome {
    Image(Arc<Completed>),
    /// No active scene was available; an active blank is an ordinary image.
    NoScene,
    Failed(Error),
}

enum State {
    Open {
        outcome: Option<Outcome>,
        last_image: Option<Arc<Completed>>,
        progress: Progress,
    },
    Closed,
}

impl State {
    /// Return replaced storage for destruction outside the worker's result lock.
    fn record(
        &mut self,
        through: u64,
        next: Outcome,
    ) -> (Option<Outcome>, Option<Arc<Completed>>, Option<Outcome>) {
        let Self::Open {
            outcome,
            last_image,
            progress,
        } = self
        else {
            return (Some(next), None, None);
        };
        let retired_image = match &next {
            Outcome::Image(image) => last_image.replace(image.clone()),
            Outcome::NoScene => last_image.take(),
            Outcome::Failed(_) => None,
        };
        let retired_attempt = progress.finish(through, next.clone());
        (outcome.replace(next), retired_image, retired_attempt)
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
        let through = match &*worker.state.lock() {
            State::Open { progress, .. } => progress.starting(),
            State::Closed => return,
        };
        let outcome = match compose::current_checked(&worker.output, &worker.pool, || {
            let state = worker.state.lock();
            if matches!(*state, State::Closed) {
                Err(ENODEV)
            } else {
                Ok(state)
            }
        }) {
            Ok(Some(image)) => match Arc::new(image, GFP_KERNEL) {
                Ok(image) => Outcome::Image(image),
                Err(error) => Outcome::Failed(error.into()),
            },
            Ok(None) => Outcome::NoScene,
            Err(error) => Outcome::Failed(error),
        };
        let retired = worker.state.lock().record(through, outcome);
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

/// Detached cached results, released outside locks needed for buffer or device cleanup.
///
/// This owns no source claim and does not drain a running worker. Already admitted reads
/// remain owned by that worker until it finishes; the worker owner still drains on close.
#[must_use = "release detached results outside modeset, publication and reservation locks"]
pub(crate) struct RetiredResults {
    _state: State,
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
                    progress: Progress::new(),
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

    /// Stop new requests and source claims without waiting for an already admitted read.
    ///
    /// Source admission checks this same state after mapping preparation and retains its
    /// guard across the native claim. Cutoff therefore rejects a worker that has started
    /// but has not claimed yet. Detached results must be released outside outer locks;
    /// close or owner destruction still drains work and closes its private pool.
    pub(crate) fn stop_admission(&self) -> RetiredResults {
        let retired = core::mem::replace(&mut *self.worker.state.lock(), State::Closed);
        self.worker.changed.notify_all();
        RetiredResults { _state: retired }
    }

    /// Stop new requests and drain work outside locks needed for mapping or source retirement.
    pub(crate) fn close(&self) {
        drop(self.stop_admission());
        self.worker.work.flush();
        self.worker.pool.close();
    }
}

/// Shared access to private composition requests, without ownership of shutdown.
///
/// Untracked observation shares one consumable latest result; [`Self::request_outcome`] gives
/// each caller an independent minimum attempt to observe. Dropping a handle does not close the worker;
/// dropping its [`Owner`] closes all surviving handles and drains any accepted request.
/// A handle grants no permission to deliver the resulting pixels to a capture recipient.
#[derive(Clone)]
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
        self.request_outcome().map(|_| ())
    }

    /// Queue composition and independently observe an attempt covering this request.
    ///
    /// A running attempt covers only requests present when it started. A request arriving
    /// during that attempt queues another pass; already queued requests coalesce into it.
    pub(crate) fn request_outcome(&self) -> Result<Request> {
        let mut state = self.worker.state.lock();
        let State::Open { progress, .. } = &mut *state else {
            return Err(ENODEV);
        };
        let requested = progress.request()?;
        let _queued = workqueue::system_dfl().enqueue(self.worker.clone());
        Ok(Request::new(self.worker.clone(), requested))
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
    /// may consume an available outcome first. Use [`Self::request_outcome`] to associate
    /// independent observation with a new request. Call only from consumer context,
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
    /// A failure preserves this historical image; no active scene or shutdown clears
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
