// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Connect KMS setup and accepted output state to shared preparation accounting.

use super::{
    crtc::{
        AsRawCrtcStatePrivate,
        CrtcAtomicCommit,
        DriverCrtc, //
    },
    KmsDriver,
    UnregisteredKmsDevice, //
};
use crate::{
    drm::preparation::Source,
    error::to_result,
    prelude::*,
    sync::aref::ARef, //
};
use core::ptr::NonNull;

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

impl<T: DriverCrtc> CrtcAtomicCommit<'_, T> {
    /// Retain read accounting for the output generation accepted by this commit.
    ///
    /// Returns `None` when the device has not enabled preparation. Accounting also exists for
    /// disabled outputs; the reference does not imply that the output has a visible image.
    /// It grants neither capture authority nor pixel access. A provider must authorize a read,
    /// retain its storage, and acquire a claim separately before accessing source pixels.
    pub fn preparation_source(&self) -> Option<ARef<Source>> {
        let (_, new) = self.old_new_state();
        // SAFETY: The callback retains the accepted state. Preparation initializes its source
        // before publication and does not replace the pointer while the state remains live.
        let source = NonNull::new(unsafe { (*new.as_raw()).prepare_source })?;
        // SAFETY: The accepted state retains the initialized source while we acquire a reference.
        unsafe { bindings::drm_prepare_source_get(source.as_ptr()) };
        // SAFETY: Source transparently represents the native object. Transfer the acquired
        // reference to its Rust owner, independently of the callback and accepted state.
        Some(unsafe { ARef::from_raw(source.cast()) })
    }
}
