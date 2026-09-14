// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Owned cleanup for a stream named within an anonymous capture client.

mod completion;

use crate::{
    error::to_result,
    fs::File,
    prelude::*,
    sync::aref::ARef, //
};
use core::num::NonZeroU64;

/// Retain the client file and request closure of one admitted stream on drop.
///
/// The stream name is never reused within that file, so delayed cleanup cannot select
/// a replacement. Successful explicit closure ends that stream's destination writes,
/// without revoking the grant or waiting for shared rendering and downstream consumers.
/// Every operation and drop requires sleepable context outside DRM, authority, provider
/// lifecycle and reservation locks. Use explicit close to observe provider errors; drop
/// attempts cleanup, but a failing provider may retain resources until client destruction.
/// Dropping a file reference alone is not acknowledgment that destination access has ended.
#[must_use = "dropping the handle requests closure of its client stream"]
pub struct ClientStream {
    client: ARef<File>,
    id: Option<NonZeroU64>,
}

impl ClientStream {
    /// Open the offered configuration under a new caller-supplied name.
    ///
    /// Native dispatch validates the client role and requires both lifetime callbacks.
    /// The provider checks current permission, the offer and its own bounds. Success
    /// retains the file, not a revocation owner; failure does not consume the name.
    pub fn open(client: &File, id: u64, offer: u64, capacity: u32) -> Result<Self> {
        let id = NonZeroU64::new(id).ok_or(EINVAL)?;
        // SAFETY: The borrowed file remains live. Native dispatch validates its role and
        // serializes provider access. Scalar arguments transfer no pointer or fd ownership.
        to_result(unsafe {
            bindings::drm_capture_client_open_stream(client.as_ptr(), id.get(), offer, capacity)
        })?;
        Ok(Self {
            client: client.into(),
            id: Some(id),
        })
    }

    /// The admitted name, or None after acknowledged closure.
    pub fn id(&self) -> Option<u64> {
        self.id.map(NonZeroU64::get)
    }

    /// Close once, including after revocation, and retain failed cleanup for retry.
    ///
    /// An absent entry is already closed. Providers must never reuse names, including
    /// when another holder closes the stream directly. Other failures leave the handle
    /// open for an explicit retry or the final drop's cleanup attempt.
    pub fn close(&mut self) -> Result {
        let Some(id) = self.id else {
            return Ok(());
        };
        // SAFETY: The owned file keeps its client state alive. Native dispatch validates
        // the endpoint and serializes cleanup without requiring current pixel permission.
        match to_result(unsafe {
            bindings::drm_capture_client_close_stream(self.client.as_ptr(), id.get())
        }) {
            Ok(()) | Err(ENOENT) => {
                self.id = None;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}

impl Drop for ClientStream {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
