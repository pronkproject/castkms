// SPDX-License-Identifier: GPL-2.0-only

//! Capture file operations above provider permission and kernel negotiation.

mod resources;

use super::{
    negotiation::Negotiation,
    provider::Capture, //
};
use kernel::{
    drm::capture::{
        ClientOwner,
        Description, //
    },
    prelude::*, //
};

/// The shared file dispatcher exclusively borrows operation state for each callback.
/// No grantor or creating DRM file is retained by this client.
pub(crate) struct Client {
    negotiation: Negotiation,
}

impl Client {
    pub(crate) fn new(capture: Capture) -> Self {
        Self {
            negotiation: Negotiation::new(capture),
        }
    }
}

// SAFETY: The callback trampolines, client destructor and dependencies belong to CastKMS.
#[vtable]
unsafe impl ClientOwner for Client {
    fn describe(&mut self) -> Result<Description> {
        let offer = self.negotiation.describe()?;
        let image = offer.description();
        let (width, height) = image.layout().dimensions();
        Description::new(
            offer.id(),
            [width, height],
            image.format(),
            image.modifier(),
            image.max_requests(),
        )
    }
}
