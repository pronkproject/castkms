// SPDX-License-Identifier: GPL-2.0-only

//! Kernel capture using the output's shared host compositor.

pub(crate) mod output;
mod pending;

pub(crate) use pending::Pending;

use super::destination::Image;
use super::provider::{
    self,
    Capture,
    Description,
    Request, //
};
use crate::host_compositor::worker::Handle;
use kernel::{
    dma_fence::Fence,
    prelude::*,
    sync::{
        aref::ARef,
        Arc, //
    }, //
};
use output::Output;

/// Authorized delivery whose scheduling is private to the adapter.
///
/// Queued operations retain independent observations and identify their exact requests.
/// Different streams share composition without sharing consumable notifications.
/// Returned results own independent storage, not a private compositor image or source read.
pub(crate) struct Stream {
    delivery: Arc<provider::Stream>,
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
        Self::from_description(&capture.describe_stream()?, capacity)
    }

    /// Open the described layout or fail if permission or its configuration has changed.
    ///
    /// Stale descriptions do not configure a worker or start composition. The returned
    /// stream still checks current authorization at each delivery.
    pub(crate) fn from_description(description: &Description, capacity: u32) -> Result<Self> {
        let delivery = Arc::new(description.create_stream(capacity)?, GFP_KERNEL)?;
        let device = delivery.device();
        let worker = device
            .host
            .configure_checked(device, delivery.layout(), || delivery.check_current())?;
        Ok(Self { delivery, worker })
    }

    /// Queue bounded demand without waiting or retaining a compositor-source claim.
    ///
    /// The returned operation is independently owned and may complete out of queue order.
    /// Dropping it abandons only its request, not shared composition. Closing this stream
    /// stops delivery through surviving operations; their references do not preserve access.
    pub(crate) fn queue(&self) -> Result<Pending> {
        let request = self.delivery.queue()?;
        let worker = self.worker.request_outcome()?;
        Ok(Pending::new(self.delivery.clone(), request, worker))
    }

    /// Queue an attempt for a retained destination matching this stream's fixed layout.
    ///
    /// Reject incompatible storage and known current-source aliases before reserving capture
    /// credit or scheduling rendering. This is not exclusion against later source selection.
    /// The caller must exclude conflicting destination access until the returned output
    /// completes. Dropping output requests cancellation but does not drain detached access.
    /// An unfinished reuse fence retains no source claim, and the
    /// fence's completion does not replace the caller's duty to exclude subsequent users.
    pub(crate) fn queue_to(
        &self,
        destination: Arc<Image>,
        reuse: Option<ARef<Fence>>,
    ) -> Result<Output> {
        if destination.layout() != self.delivery.layout() {
            return Err(EINVAL);
        }
        if !destination.buffer().is_writable() {
            return Err(EACCES);
        }
        self.delivery.check_destination(destination.buffer())?;
        Ok(Output::new(self.queue()?, destination, reuse))
    }

    /// Capture one attempt, with no automatic retries or assumed frame cadence.
    ///
    /// Wait outside DRM, publication, worker-lifecycle and buffer reservation locks.
    /// The wait is interruptible and owns no source claim. Failure abandons unreturned
    /// demand and releases its queue credit. A returned request has a terminal result;
    /// inspect that result for copy failure or revocation during authorized delivery.
    /// An inactive or unpublished output returns EAGAIN. Active blank output produces
    /// an ordinary authorized image, with no framebuffer content revision.
    pub(crate) fn capture(&mut self) -> Result<Request> {
        self.queue()?.wait()
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.delivery.close();
    }
}
