// SPDX-License-Identifier: GPL-2.0-only

//! Retained identity of one display-mode and connector-route interval.

use kernel::{
    prelude::*,
    sync::Arc, //
};

struct Description {
    connectors: u32,
    dimensions: [u32; 2],
    refresh_millihz: u32,
    mode_flags: u32,
}

/// An immutable candidate description; only accepted publication makes it current.
///
/// Equality denotes the same interval, not merely equal dimensions or routing bits.
/// Clones retain that identity without retaining a device, framebuffer or pixel allocation.
#[derive(Clone)]
pub(crate) struct Configuration(Arc<Description>);

impl Configuration {
    pub(crate) fn new(
        connectors: u32,
        dimensions: [u32; 2],
        refresh_millihz: u32,
        mode_flags: u32,
    ) -> Result<Self> {
        if connectors == 0 || dimensions.contains(&0) || refresh_millihz == 0 {
            return Err(EINVAL);
        }
        Ok(Self(Arc::new(
            Description {
                connectors,
                dimensions,
                refresh_millihz,
                mode_flags,
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

    pub(crate) fn refresh_millihz(&self) -> u32 {
        self.0.refresh_millihz
    }

    pub(crate) fn mode_flags(&self) -> u32 {
        self.0.mode_flags
    }
}

impl PartialEq for Configuration {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Configuration {}
