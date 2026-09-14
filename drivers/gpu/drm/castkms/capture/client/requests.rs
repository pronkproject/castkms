// SPDX-License-Identifier: GPL-2.0-only

//! Bind client-local resource names to independently owned output attempts.

use super::Client;
use kernel::{
    dma_fence::Fence,
    prelude::*,
    sync::aref::ARef, //
};

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Client {
    /// Cancel one request through its named stream without resolving its destination again.
    ///
    /// Cancellation is independent of permission to admit more pixels. It preserves the
    /// terminal record for normal advancement and dequeue, and does not release the
    /// caller's storage-reuse obligations. A missing stream or request returns ENOENT.
    pub(crate) fn cancel(&mut self, stream: u64, use_id: u64) -> Result {
        self.streams
            .with_queue(stream, |queue| queue.cancel(use_id))
    }

    /// Queue one output for the exact registered destination and named stream.
    ///
    /// Destination removal after admission cannot replace the retained allocation. The
    /// request ID belongs to the stream, not the destination namespace. Rejected admission
    /// consumes neither that ID nor capacity. Current permission is checked by the stream.
    /// Call outside DRM, publication, worker-lifecycle and reservation locks. The recipient
    /// must exclude competing access to its destination until the output completes or the
    /// stream closes; a registered name or reuse fence alone does not establish exclusion.
    pub(crate) fn queue_to(
        &mut self,
        stream: u64,
        use_id: u64,
        destination: u64,
        reuse: Option<ARef<Fence>>,
    ) -> Result {
        let destination = self.destination(destination)?;
        self.streams
            .with_queue(stream, |queue| queue.queue_to(use_id, destination, reuse))
    }
}
