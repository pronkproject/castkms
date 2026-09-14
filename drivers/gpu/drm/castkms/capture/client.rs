// SPDX-License-Identifier: GPL-2.0-only

//! Capture file operations above provider permission and kernel negotiation.

mod resources;
mod stream;

pub(crate) use stream::Stream;

use super::{
    budget::STREAM_LIMIT,
    host_queue::Queue,
    negotiation::Negotiation,
    provider::Capture, //
};
use kernel::{
    drm::capture::{
        ClientOwner,
        Description, //
    },
    prelude::*, //
};
use resources::Resources;

/// The shared file dispatcher exclusively borrows operation state for each callback.
/// No grantor or creating DRM file is retained by this client.
pub(crate) struct Client {
    streams: Resources<Queue>,
    negotiation: Negotiation,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Client {
    pub(crate) fn new(capture: Capture) -> Result<Self> {
        Ok(Self {
            streams: Resources::new(STREAM_LIMIT)?,
            negotiation: Negotiation::new(capture),
        })
    }

    /// Open the named offer under a new caller-supplied stream ID.
    ///
    /// Validate the stream ID and table capacity before allocating provider resources.
    /// Failure does not consume the ID. No output publication is needed after success.
    /// Each stream retains its own queue; later description queries do not replace it.
    /// Call outside DRM, publication, worker-lifecycle and reservation locks.
    pub(crate) fn open_stream(&mut self, id: u64, offer: u64, capacity: u32) -> Result {
        let negotiation = &self.negotiation;
        self.streams
            .insert(id, || negotiation.open(offer, capacity))
    }

    /// Borrow one client's retained queue without substituting a newer stream.
    ///
    /// Queue operations keep their own current-permission and delivery checks. Merely
    /// finding an entry neither preserves permission nor authorizes source access.
    pub(crate) fn stream(&mut self, id: u64) -> Result<Stream<'_>> {
        Ok(Stream::new(self.streams.get_mut(id)?))
    }
}

// SAFETY: The callback trampolines, client destructor and dependencies belong to CastKMS.
#[vtable]
unsafe impl ClientOwner for Client {
    fn describe(&mut self) -> Result<Description> {
        let offer = self.negotiation.describe()?;
        let image = offer.description();
        let (width, height) = image.layout().dimensions();
        Description::new(
            offer.id(),
            [width, height],
            image.format(),
            image.modifier(),
            image.max_requests(),
        )
    }
}
