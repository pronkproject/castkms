// SPDX-License-Identifier: GPL-2.0-only

//! Capture file operations above provider permission and kernel negotiation.

mod destinations;
mod requests;
mod stream;
mod streams;

pub(crate) use stream::Stream;

use super::{
    destination::Image,
    negotiation::Negotiation,
    provider::Capture, //
};
use kernel::{
    dma_fence::Fence,
    drm::capture::{
        ClientOwner,
        Completion,
        Description,
        Destination,
        Readiness, //
    },
    prelude::*,
    sync::Arc, //
};

/// The shared file dispatcher exclusively borrows operation state for each callback.
/// No grantor or creating DRM file is retained by this client.
pub(crate) struct Client {
    streams: streams::Streams,
    destinations: destinations::Destinations,
    negotiation: Negotiation,
}

impl Client {
    pub(crate) fn new(capture: Capture) -> Result<Self> {
        Ok(Self {
            streams: streams::Streams::new()?,
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
        self.streams.with_queue(id, |_| Ok(()))?;
        Ok(Stream::new(&self.streams, id))
    }

    /// Close one stream without revoking the client or making its name reusable.
    ///
    /// Cleanup remains available after revocation or a modeset. Pending results are
    /// abandoned through normal queue destruction; shared composition is not canceled.
    /// Detached destination access returns EBUSY instead of blocking closure. Admission
    /// stays closed while cancellation and cleanup remain available for retry.
    /// Call outside DRM, publication, worker-lifecycle and reservation locks.
    pub(crate) fn close_stream(&mut self, id: u64) -> Result {
        self.streams.remove(id)
    }

    /// Retain caller-owned output storage without queuing capture or mapping pixels.
    ///
    /// The name belongs to this client, independently of stream names. Registration
    /// checks current capture permission and the retained buffer's export write access.
    /// It does not exclude another native or CPU user of the allocation.
    pub(crate) fn register_destination(&mut self, id: u64, image: Image) -> Result {
        self.negotiation.check_capture()?;
        let negotiation = &self.negotiation;
        self.destinations.insert(id, image, |image| negotiation.retain_destination_storage(image))
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
    fn cancel(&mut self, stream: u64, use_id: u64) -> Result {
        Client::cancel(self, stream, use_id)
    }

    fn queue_output(
        &mut self,
        stream: u64,
        use_id: u64,
        destination: u64,
        reuse: Option<&Fence>,
    ) -> Result {
        self.queue_to(stream, use_id, destination, reuse.map(Into::into))
    }

    fn dequeue(&mut self, stream: u64, publish: impl FnOnce(Completion) -> Result) -> Result {
        self.stream(stream)?.dequeue(|completion| {
            publish(Completion::new(
                completion.use_id,
                completion.result.map(|frame| frame.metadata().completed_at()),
            )?)
        })
    }

    fn readiness(&self) -> Option<&Readiness> {
        Some(self.streams.readiness())
    }

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
            image.configuration().refresh_millihz(),
            image.configuration().mode_flags(),
            image.format(),
            image.modifier(),
            image.max_requests(),
        )
    }
}
