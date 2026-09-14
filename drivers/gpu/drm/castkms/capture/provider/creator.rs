// SPDX-License-Identifier: GPL-2.0-only

//! Provider limits for grants bound to a creating owner.

use super::Policy;
use kernel::{
    drm::capture::{
        Authority,
        Creator as CaptureCreator, //
    },
    prelude::*, //
};

pub(super) use kernel::drm::capture::Registration;

const MAX_GRANTS: u32 = 64;

/// A creating owner with the driver's grant limit.
pub(crate) struct Creator(CaptureCreator);

impl Creator {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self(CaptureCreator::new(MAX_GRANTS)?))
    }

    pub(super) fn register(&self, authority: &Authority<Policy>) -> Result<Registration> {
        self.0.register(authority)
    }
}
