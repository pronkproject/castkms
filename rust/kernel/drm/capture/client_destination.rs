// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Owned cleanup of one destination name, without ownership of capture revocation.

use super::Destination;
use crate::{
    error::to_result,
    fs::File,
    prelude::*,
    sync::aref::ARef, //
};
use core::num::NonZeroU64;

/// Retain a client file and request removal of one destination registration on drop.
///
/// The provider owns retained buffer references and accepted operation lifetimes. Removal
/// revokes neither exported storage nor the capture grant and does not wait for GPU work.
/// Every operation and drop requires sleepable context outside DRM, authority, provider
/// lifecycle and reservation locks. Explicit removal reports errors; a failing drop attempt
/// may leave provider resources retained until final client release.
#[must_use = "dropping the handle requests removal of its destination name"]
pub struct ClientDestination {
    client: ARef<File>,
    id: Option<NonZeroU64>,
}

impl ClientDestination {
    /// Register borrowed storage; native dispatch checks the endpoint and callback pair.
    ///
    /// The provider rechecks permission, complete layout and its resource limits. Success
    /// acquires provider-owned buffer references before the borrowed description expires.
    /// Failure leaves the name retryable and retains no new provider registration.
    pub fn register(client: &File, id: u64, destination: &Destination<'_>) -> Result<Self> {
        let id = NonZeroU64::new(id).ok_or(EINVAL)?;
        // SAFETY: The file, fully initialized metadata and every borrowed buffer remain live.
        // Native dispatch validates the endpoint and serializes the provider callback.
        to_result(unsafe {
            bindings::drm_capture_client_register_destination(
                client.as_ptr(),
                id.get(),
                destination.as_raw(),
            )
        })?;
        Ok(Self {
            client: client.into(),
            id: Some(id),
        })
    }

    /// The registered name, or None after acknowledged removal.
    pub fn id(&self) -> Option<u64> {
        self.id.map(NonZeroU64::get)
    }

    /// Remove once, preserving failed cleanup for retry even after revocation.
    ///
    /// ENOENT acknowledges an already absent entry. Non-reusable names prevent delayed
    /// cleanup from removing a replacement installed through another reference to the file.
    pub fn unregister(&mut self) -> Result {
        let Some(id) = self.id else {
            return Ok(());
        };
        // SAFETY: The owned file retains serialized provider state throughout cleanup.
        match to_result(unsafe {
            bindings::drm_capture_client_unregister_destination(self.client.as_ptr(), id.get())
        }) {
            Ok(()) | Err(ENOENT) => {
                self.id = None;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}

impl Drop for ClientDestination {
    fn drop(&mut self) {
        let _ = self.unregister();
    }
}
