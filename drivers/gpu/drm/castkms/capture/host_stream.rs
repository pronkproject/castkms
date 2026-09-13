// SPDX-License-Identifier: GPL-2.0-only

//! Synchronous kernel capture using the output's shared host compositor.

use super::provider::{
    self,
    Capture,
    Request, //
};
use crate::host_compositor::worker::{
    Handle,
    Outcome, //
};
use kernel::prelude::*;

/// Authorized delivery whose scheduling is private to the adapter.
///
/// Capture requires an exclusive borrow to keep deliveries ordered on one stream.
/// Different streams share composition without sharing consumable notifications.
/// Returned results own independent storage, not a private compositor image or source read.
pub(crate) struct Stream {
    delivery: provider::Stream,
    worker: Handle,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Stream {
    /// Reserve capture storage before configuring composition on the exact target device.
    ///
    /// Call from sleepable context without DRM, publication or buffer reservation locks.
    /// Configuration may allocate or drain a previous worker. Subsequent delivery checks
    /// current authorization again; configuring storage does not preserve that permission.
    pub(crate) fn new(capture: &Capture, capacity: u32) -> Result<Self> {
        let delivery = capture.stream(capacity)?;
        let device = delivery.device();
        let worker = device
            .host
            .configure_checked(device, delivery.layout(), || delivery.check_current())?;
        Ok(Self { delivery, worker })
    }

    /// Capture one attempt, with no automatic retries or assumed frame cadence.
    ///
    /// Wait outside DRM, publication, worker-lifecycle and buffer reservation locks.
    /// The wait is interruptible and owns no source claim. Failure abandons unreturned
    /// demand and releases its queue credit. A returned request has a terminal result;
    /// inspect that result for copy failure or revocation during authorized delivery.
    /// A blank scene returns EAGAIN until blank-image authorization is implemented.
    pub(crate) fn capture(&mut self) -> Result<Request> {
        let request = self.delivery.queue()?;
        match self.worker.request_outcome()?.wait()? {
            Outcome::Image(image) => self.delivery.deliver(&image)?,
            Outcome::Blank => return Err(EAGAIN),
            Outcome::Failed(error) => return Err(error),
        }
        Ok(request)
    }
}
