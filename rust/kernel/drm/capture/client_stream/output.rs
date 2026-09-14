// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Output admission using retained kernel objects instead of file descriptor lookup.

use super::ClientStream;
use crate::{
    dma_fence::Fence,
    error::to_result,
    prelude::*, //
};

impl ClientStream {
    /// Queue one output attempt against a destination registered on this client.
    ///
    /// Success consumes the increasing, nonzero attempt name and retains the exact
    /// destination until its access ends. Rejection leaves the name retryable. The
    /// optional fence describes previously submitted destination use; the caller must
    /// also exclude competing access until terminal completion or successful stream close.
    /// Removing the destination name does not cancel accepted writes.
    pub fn queue_output(&self, use_id: u64, destination: u64, reuse: Option<&Fence>) -> Result {
        let id = self.id.ok_or(ENOENT)?;
        // SAFETY: The owned client and optional borrowed fence stay live throughout native
        // dispatch. Providers retain their own references before accepting the attempt.
        to_result(unsafe {
            bindings::drm_capture_client_queue_output(
                self.client.as_ptr(),
                id.get(),
                use_id,
                destination,
                reuse.map_or(core::ptr::null_mut(), Fence::as_raw),
            )
        })
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
