// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Setup of output-generation accounting for shared atomic preparation.

use super::{
    KmsDriver,
    UnregisteredKmsDevice, //
};
use crate::{
    error::to_result,
    prelude::*, //
};

impl<T: KmsDriver> UnregisteredKmsDevice<'_, T> {
    /// Enable preparation before creating CRTCs during single-threaded setup.
    ///
    /// `capacity` limits admitted reads per output generation, independently of media queues.
    /// The Rust KMS implementation uses the shared state lifetime and atomic commit helpers.
    /// Enabling accounting does not authorize capture or publish source buffers.
    pub fn enable_preparation(&self, capacity: u32) -> Result {
        // SAFETY: The setup view retains initialized mode configuration and excludes concurrent
        // registration or object creation. Native initialization rejects zero capacity, repeated
        // setup and setup after CRTC creation; mode configuration cleanup releases its resources.
        to_result(unsafe { bindings::drm_atomic_prepare_display_init(self.as_raw(), capacity) })
    }
}
