// SPDX-License-Identifier: GPL-2.0-only

//! Independent observation of a coalesced composition request.

use super::{
    Outcome,
    State,
    Worker, //
};
use kernel::{
    prelude::*,
    sync::{
        Arc,
        CondVar, //
    }, //
};

/// Observe an attempt that started after this request was queued, without claiming a source.
///
/// Observation does not consume the shared result. Several requests may be covered by one
/// attempt, and each observation returns the newest eligible completed attempt rather than
/// a permanent per-request snapshot. Dropping the handle does not cancel shared composition.
/// The result grants no capture permission.
pub(crate) struct Request {
    worker: Arc<Worker>,
    requested: u64,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Request {
    pub(super) fn new(worker: Arc<Worker>, requested: u64) -> Self {
        Self { worker, requested }
    }

    pub(crate) fn try_outcome(&self) -> Result<Option<Outcome>> {
        match &*self.worker.state.lock() {
            State::Open { progress, .. } => Ok(progress.observe(self.requested)),
            State::Closed => Err(ENODEV),
        }
    }

    /// Notifications for outcome changes and worker shutdown; register before observing.
    pub(crate) fn changed(&self) -> &CondVar {
        &self.worker.changed
    }

    /// Wait outside DRM, source, worker-lifecycle and buffer reservation locks.
    ///
    /// An interrupted wait leaves the request observable. Worker shutdown wakes every
    /// observer with ENODEV; a completed failed attempt is returned as Outcome::Failed.
    pub(crate) fn wait(&self) -> Result<Outcome> {
        let mut state = self.worker.state.lock();
        loop {
            match &*state {
                State::Open { progress, .. } => {
                    if let Some(outcome) = progress.observe(self.requested) {
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
}
