// SPDX-License-Identifier: GPL-2.0-only

//! Authorized delivery into the exact retained request, independent of queue order.

use super::{
    super::{
        Frame,
        Stream, //
    },
    Request, //
};
use crate::{
    capture::host,
    host_compositor::compose::Completed, //
};
use kernel::{
    prelude::*,
    sync::Arc, //
};

impl Request {
    /// Retain the delivered image's description, without relabeling it after a later update.
    ///
    /// The request is abandoned on failure. Metadata is prepared before claiming delivery;
    /// success carries the request's terminal status, not an unconditional pixel-validity claim.
    pub(crate) fn deliver_frame(self, stream: &Stream, image: &Completed) -> Result<Frame> {
        let frame = Frame::new(self, image)?;
        frame.request().deliver(stream, image)?;
        Ok(frame)
    }

    /// Deliver only into this request after validating its stream and current image permission.
    ///
    /// A foreign stream is rejected even when it has the same layout, authority and numeric
    /// request ID. A canceled or completed request never redirects delivery to other demand.
    /// Copying runs outside policy locks and owns no compositor-source claim. Success means
    /// completion was recorded; inspect the result for copying or intervening revocation errors.
    pub(crate) fn deliver(&self, stream: &Stream, image: &Completed) -> Result {
        if !Arc::ptr_eq(&self.storage, &stream.storage) {
            return Err(EINVAL);
        }
        let job = stream.with_image(image, || {
            stream.capture.authority.claim_request(&self.native)
        })?;
        host::complete(image, job);
        Ok(())
    }
}
