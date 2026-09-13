// SPDX-License-Identifier: GPL-2.0-only

//! Retained identity of one display-mode and connector-route interval.

use kernel::{
    prelude::*,
    sync::Arc, //
};

struct Description {
    connectors: u32,
    dimensions: [u32; 2],
}

/// An immutable candidate description; only accepted publication makes it current.
///
/// Equality denotes the same interval, not merely equal dimensions or routing bits.
/// Clones retain that identity without retaining a device, framebuffer or pixel allocation.
#[derive(Clone)]
pub(crate) struct Configuration(Arc<Description>);

impl Configuration {
    pub(crate) fn new(connectors: u32, dimensions: [u32; 2]) -> Result<Self> {
        if connectors == 0 || dimensions.contains(&0) {
            return Err(EINVAL);
        }
        Ok(Self(Arc::new(
            Description {
                connectors,
                dimensions,
            },
            GFP_KERNEL,
        )?))
    }

    /// Connector-index bits, not registered object identity or capture authority.
    pub(crate) fn connector_mask(&self) -> u32 {
        self.0.connectors
    }

    pub(crate) fn dimensions(&self) -> [u32; 2] {
        self.0.dimensions
    }
}

impl PartialEq for Configuration {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Configuration {}
