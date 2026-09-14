// SPDX-License-Identifier: GPL-2.0-only

//! One queued capture attempt, without ownership of stream shutdown or a source claim.

use crate::{
    capture::provider::{
        Request,
        Stream, //
    },
    host_compositor::worker::{
        self,
        Outcome, //
    }, //
};
use kernel::{
    drm::capture::Status,
    prelude::*,
    sync::Arc, //
};

/// Dropping uncompleted demand releases its credit without canceling shared composition.
///
/// The private worker outcome does not authorize delivery. Completion checks current image
/// permission and the exact request, even when other operations finish first. The stream
/// owner may close delivery while this object survives. No source claim is held while waiting.
#[must_use = "dropping a pending capture abandons its request"]
pub(crate) struct Pending {
    request: Option<Request>,
    worker: worker::Request,
    delivery: Arc<Stream>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Pending {
    pub(super) fn new(delivery: Arc<Stream>, request: Request, worker: worker::Request) -> Self {
        Self {
            request: Some(request),
            worker,
            delivery,
        }
    }

    /// Complete an available attempt without waiting for composition.
    ///
    /// `None` leaves demand pending. Any other result consumes the operation; a repeated
    /// call returns `EALREADY`. A returned request has a terminal status that still needs
    /// inspection for copy failure or revocation. This may sleep while authorizing/copying;
    /// call outside DRM, publication, destination and worker-lifecycle locks.
    pub(crate) fn try_complete(&mut self) -> Result<Option<Request>> {
        let request = self.request.as_ref().ok_or(EALREADY)?;
        let outcome = match request.status() {
            Ok(Status::Pending) => self.worker.try_outcome(),
            Ok(Status::Complete(result)) => Err(result.err().unwrap_or(EALREADY)),
            Err(error) => Err(error),
        };
        match outcome {
            Ok(None) => Ok(None),
            Ok(Some(outcome)) => self.complete(Ok(outcome)).map(Some),
            Err(error) => self.complete(Err(error)).map(Some),
        }
    }

    /// Wait interruptibly and complete exactly this request, outside locks needed by delivery.
    ///
    /// An interrupted wait abandons this operation. Callers that need to retain demand across
    /// scheduling decisions may keep the object and use [`Self::try_complete`] instead.
    pub(crate) fn wait(mut self) -> Result<Request> {
        let request = self.request.as_ref().ok_or(EALREADY)?;
        let outcome =
            request.wait_for_provider(self.worker.changed(), || self.worker.try_outcome());
        self.complete(outcome)
    }

    fn complete(&mut self, outcome: Result<Outcome>) -> Result<Request> {
        let request = self.request.take().ok_or(EALREADY)?;
        match outcome? {
            Outcome::Image(image) => request.deliver(&self.delivery, &image)?,
            Outcome::NoScene => return Err(EAGAIN),
            Outcome::Failed(error) => return Err(error),
        }
        Ok(request)
    }
}
