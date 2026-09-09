// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Interruptible result observation without file transport or provider policy.

use super::Request;
use crate::{
    error::to_result,
    prelude::*, //
};

impl Request {
    /// Wait for a completed result without consuming the request or its image.
    ///
    /// The outer result describes waiting: interruption or removal of the request is an error.
    /// The inner result describes capture: provider failure is a completed, unsuccessful image.
    /// Interruption does not cancel the request. Revoking or canceling a claimed request does
    /// not finish the wait until its provider ends access. Hold no locks needed by that provider.
    pub fn wait(&self) -> Result<Result> {
        let mut result = bindings::drm_capture_result::default();
        // SAFETY: The request retains its stream and output storage remains writable until return.
        to_result(unsafe {
            bindings::drm_capture_wait_result(self.stream.0.get(), self.id, &mut result)
        })?;
        Ok(to_result(result.status))
    }
}
