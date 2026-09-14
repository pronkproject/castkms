// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Request cancellation independently of permission, publication and storage reuse.

use super::ClientStream;
use crate::{
    error::to_result,
    prelude::*, //
};

impl ClientStream {
    /// Request cancellation of one admitted attempt, including after revocation.
    ///
    /// Success does not acknowledge its result or permit destination reuse. Observe the
    /// terminal result through dequeue, or close the stream successfully, before reusing
    /// its storage. Missing requests return ENOENT. Requests already cancelled or terminal
    /// return EALREADY. A closed handle makes no provider call.
    pub fn cancel(&self, use_id: u64) -> Result {
        let id = self.id.ok_or(ENOENT)?;
        // SAFETY: The owned file retains client state throughout synchronous native
        // dispatch. Scalar names transfer no pointer or descriptor ownership.
        to_result(unsafe {
            bindings::drm_capture_client_cancel(self.client.as_ptr(), id.get(), use_id)
        })
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
