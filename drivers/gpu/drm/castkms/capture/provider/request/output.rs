// SPDX-License-Identifier: GPL-2.0-only

//! Copy already authorized private results into a consumer's host-linear destination.

use super::Request;
use crate::capture::destination::Image;
use kernel::{
    dma_buf::cpu_access::Write,
    dma_fence::{
        Fence,
        Status as FenceStatus, //
    },
    dma_resv::Usage,
    drm::capture::Status,
    prelude::*, //
};

fn ready(fence: &Fence) -> Result {
    match fence.status() {
        FenceStatus::Pending => Err(EAGAIN),
        FenceStatus::Complete(result) => result,
    }
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Request {
    /// Copy a retained successful HOST result without keeping any compositor-source access.
    ///
    /// The recipient must own the destination and exclude conflicting use during copying.
    /// A supplied native reuse fence must cover explicitly synchronized prior users. Pending
    /// reuse returns EAGAIN without writing. Reservation snapshots include acquired implicit
    /// readers but are not history: already-signaled errors may no longer be discoverable.
    /// Neither snapshots nor CPU cache maintenance prevent a racing native submission.
    ///
    /// Destination acquisition and copying run without policy locks or source claims. A
    /// successful return includes exporter cache maintenance; it needs no completion fence.
    /// Failure may leave a partial destination and never reports those pixels as valid. The
    /// private request remains separately readable until it is dropped or its stream closes.
    /// Revocation preserves an already completed, authorized private result, as for copy_result.
    pub(crate) fn copy_to_destination(&self, destination: &Image, reuse: Option<&Fence>) -> Result {
        let layout = self.storage.layout();
        if destination.layout() != layout {
            return Err(EINVAL);
        }
        match self.status()? {
            Status::Pending => return Err(EAGAIN),
            Status::Complete(result) => result?,
        }
        if let Some(reuse) = reuse {
            ready(reuse)?;
        }
        let dependencies = destination.buffer().reservation().snapshot(Usage::Read)?;
        for fence in dependencies.iter() {
            ready(fence)?;
        }

        let mut row = KVVec::new();
        row.resize(layout.pitch(), 0, GFP_KERNEL)?;
        let mut write = Write::new(destination.buffer())?;
        let zeros = [0; 256];
        for y in 0..layout.dimensions().1 as usize {
            self.native
                .copy_result_range(y * layout.pitch(), &mut row)?;
            let offset = destination.offset() + y * destination.pitch();
            write.copy_from_slice(offset, &row)?;
            let mut padding = layout.pitch();
            while padding < destination.pitch() {
                let count = (destination.pitch() - padding).min(zeros.len());
                write.copy_from_slice(offset + padding, &zeros[..count])?;
                padding += count;
            }
        }
        write.finish()
    }
}
