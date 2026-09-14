// SPDX-License-Identifier: GPL-2.0-only

//! Host stream layout retained with its exact grant and accepted display configuration.

use super::{
    Capture,
    Stream, //
};
use crate::{
    capture::budget::CAPACITY_LIMIT,
    host_compositor::layout::Layout,
    scene::Configuration, //
};
use kernel::{
    drm::fourcc,
    prelude::*, //
};

/// A description for allocating a host-linear consumer before opening its stream.
///
/// The retained capture handle has no grantor ownership. No image budget, source claim
/// or compositor worker is retained or started. Metadata describes the result layout,
/// not the validity or host readability of any future source image.
#[derive(Clone)]
pub(crate) struct Description {
    capture: Capture,
    configuration: Configuration,
    layout: Layout,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Description {
    /// Historical mode and route identity, without preserving permission to open a stream.
    pub(crate) fn configuration(&self) -> &Configuration {
        &self.configuration
    }

    pub(crate) fn layout(&self) -> Layout {
        self.layout
    }

    pub(crate) fn format(&self) -> u32 {
        fourcc::XRGB8888
    }

    pub(crate) fn modifier(&self) -> u64 {
        fourcc::FORMAT_MOD_LINEAR
    }

    /// Maximum private request count, not a reservation of currently available capacity.
    pub(crate) fn max_requests(&self) -> u32 {
        CAPACITY_LIMIT
    }

    /// Open only for the retained grant and configuration, without silently changing layout.
    ///
    /// A same-size modeset still requires a fresh description. Ordinary content updates do
    /// not. Current permission and grant revocation are checked again before admission.
    pub(crate) fn create_stream(&self, capacity: u32) -> Result<Stream> {
        self.capture
            .create_stream(&self.configuration, self.layout, capacity)
    }
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Capture {
    /// Describe authorized current output without reserving storage or reading pixels.
    ///
    /// The description binds its own grant; callers cannot pair it with another capture
    /// handle. Query and stream creation are separate observations of current permission.
    pub(crate) fn describe_stream(&self) -> Result<Description> {
        if self.authority.is_revoked() {
            return Err(EKEYREVOKED);
        }
        self.policy.permission.with_current(|current| {
            let _admission = self.authority.begin()?;
            Ok(Description {
                capture: self.clone(),
                configuration: current.configuration().clone(),
                layout: current.layout(),
            })
        })
    }
}
