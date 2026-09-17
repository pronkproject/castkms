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

fn ready(fence: &Fence) -> Result<bool> {
    match fence.status() {
        FenceStatus::Pending => Ok(false),
        FenceStatus::Complete(result) => result.map(|()| true),
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
        if self.try_copy_to_destination(destination, reuse)? {
            Ok(())
        } else {
            Err(EAGAIN)
        }
    }

    /// Try delivery without confusing an unfinished dependency with a failed operation.
    ///
    /// `Ok(false)` means no destination write was attempted because capture or reuse is
    /// pending. `Ok(true)` includes completed copying and cache maintenance. Every error
    /// is terminal for the attempt, including `EAGAIN` reported by a completed fence or
    /// exporter; it may follow a partial write. Ownership and locking requirements match
    /// [`Self::copy_to_destination`].
    pub(crate) fn try_copy_to_destination(
        &self,
        destination: &Image,
        reuse: Option<&Fence>,
    ) -> Result<bool> {
        self.try_copy_to_destination_unless(destination, reuse, || false)
    }

    /// Observe currently acquired dependencies without entering an exporter callback.
    ///
    /// A true result is not exclusion: a racing submission may still make subsequent CPU
    /// access block. Callers must isolate that access from queue and teardown operations.
    pub(crate) fn destination_ready(&self, destination: &Image, reuse: Option<&Fence>) -> Result<bool> {
        let layout = self.storage.layout();
        let (width, height) = layout.dimensions();
        if destination.dimensions() != [width, height] {
            return Err(EINVAL);
        }
        match self.status()? {
            Status::Pending => return Ok(false),
            Status::Complete(result) => result?,
        }
        if let Some(reuse) = reuse {
            if !ready(reuse)? {
                return Ok(false);
            }
        }
        let dependencies = destination.buffer().reservation().snapshot(Usage::Read)?;
        for fence in dependencies.iter() {
            if !ready(fence)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Stop abandoned delivery before writing, including after a blocking exporter returns.
    ///
    /// Cancellation is observed between bounded copies, not an abort of exporter callbacks.
    /// A caller must retain storage until this operation returns, even after cancellation.
    pub(crate) fn try_copy_to_destination_unless(
        &self,
        destination: &Image,
        reuse: Option<&Fence>,
        cancelled: impl Fn() -> bool,
    ) -> Result<bool> {
        if cancelled() {
            return Err(ECANCELED);
        }
        if !self.destination_ready(destination, reuse)? {
            return Ok(false);
        }

        let layout = self.storage.layout();
        let mut row = KVVec::new();
        row.resize(layout.pitch(), 0, GFP_KERNEL)?;
        let mut write = Write::new(destination.buffer())?;
        let zeros = [0; 256];
        for y in 0..layout.dimensions().1 as usize {
            if cancelled() {
                return Err(ECANCELED);
            }
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
        write.finish()?;
        Ok(true)
    }
}
