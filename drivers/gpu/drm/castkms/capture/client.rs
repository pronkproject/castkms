// SPDX-License-Identifier: GPL-2.0-only

//! Capture file operations above provider permission and kernel negotiation.

mod destinations;
mod stream;

pub(crate) use stream::Stream;

use super::{
    budget::STREAM_LIMIT,
    destination::Image,
    host_queue::Queue,
    negotiation::Negotiation,
    provider::Capture, //
};
use kernel::{
    drm::capture::{
        ClientOwner,
        Description,
        Destination,
        Resources, //
    },
    prelude::*,
    sync::Arc, //
};

/// The shared file dispatcher exclusively borrows operation state for each callback.
/// No grantor or creating DRM file is retained by this client.
pub(crate) struct Client {
    streams: Resources<Queue>,
    destinations: destinations::Destinations,
    negotiation: Negotiation,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Client {
    pub(crate) fn new(capture: Capture) -> Result<Self> {
        Ok(Self {
            streams: Resources::new(STREAM_LIMIT)?,
            destinations: destinations::Destinations::new()?,
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

    /// Close one stream without revoking the client or making its name reusable.
    ///
    /// Cleanup remains available after revocation or a modeset. Pending results are
    /// abandoned through normal queue destruction; shared composition is not canceled.
    /// Call outside DRM, publication, worker-lifecycle and reservation locks.
    pub(crate) fn close_stream(&mut self, id: u64) -> Result {
        drop(self.streams.remove(id)?);
        Ok(())
    }

    /// Retain caller-owned output storage without queuing capture or mapping pixels.
    ///
    /// The name belongs to this client, independently of stream names. Registration
    /// checks current capture permission and the retained buffer's export write access.
    /// It does not exclude another native or CPU user of the allocation.
    pub(crate) fn register_destination(&mut self, id: u64, image: Image) -> Result {
        self.negotiation.check_capture()?;
        self.destinations.insert(id, image)
    }

    /// Retain the exact registered image for an independently bounded operation.
    ///
    /// This transfers storage ownership only, not permission to capture future pixels.
    /// The operation must validate its stream and arrange destination reuse separately.
    pub(crate) fn destination(&self, id: u64) -> Result<Arc<Image>> {
        self.destinations.get(id)
    }

    /// Forget a name without revoking storage or canceling already accepted uses.
    ///
    /// Cleanup remains available after revocation. Surviving operation references keep
    /// the original image alive; its name is never reused during this client lifetime.
    pub(crate) fn unregister_destination(&mut self, id: u64) -> Result {
        self.destinations.remove(id)
    }
}

// SAFETY: The callback trampolines, client destructor and dependencies belong to CastKMS.
#[vtable]
unsafe impl ClientOwner for Client {
    fn register_destination(&mut self, id: u64, destination: &Destination<'_>) -> Result {
        if destination.num_planes() != 1 {
            return Err(EOPNOTSUPP);
        }
        let [width, height] = destination.dimensions();
        let layout = crate::host_compositor::layout::Layout::new(width, height)?;
        let plane = destination.plane(0).ok_or(EINVAL)?;
        let image = Image::new(
            plane.buffer().into(),
            layout,
            destination.format(),
            destination.modifier(),
            plane.stride() as usize,
            plane.offset().try_into().map_err(|_| EOVERFLOW)?,
        )?;
        Client::register_destination(self, id, image)
    }

    fn unregister_destination(&mut self, id: u64) -> Result {
        Client::unregister_destination(self, id)
    }

    fn open_stream(&mut self, id: u64, offer: u64, capacity: u32) -> Result {
        Client::open_stream(self, id, offer, capacity)
    }

    fn close_stream(&mut self, id: u64) -> Result {
        Client::close_stream(self, id)
    }

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
