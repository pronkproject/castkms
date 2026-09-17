// SPDX-License-Identifier: GPL-2.0-only

//! Candidate-bound capability registration, independent of file transport.

use super::candidate::Candidate;
use crate::execution::proposal::{DescriptionSnapshot, Registration};
use kernel::{prelude::*, sync::Arc};

/// Retains the candidate independently of a file adapter's ownership arrangement.
/// Cancellation does not require live authority and never changes active execution.
pub(crate) struct Proposal {
    candidate: Arc<Candidate>,
    registration: Registration,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Proposal {
    pub(super) fn new(candidate: Arc<Candidate>, registration: Registration) -> Self {
        Self {
            candidate,
            registration,
        }
    }

    pub(crate) fn describe(&self) -> &DescriptionSnapshot {
        self.registration.description()
    }

    /// Recheck authority, configuration, worker reservation and pending generation.
    pub(crate) fn validate(&self) -> Result {
        self.candidate.check_proposal(&self.registration)
    }

    pub(crate) fn cancel(&self) {
        self.registration.cancel();
    }

    /// Register private target storage without selecting the proposed capabilities.
    pub(crate) fn register_image(
        &self,
        dimensions: [u32; 2],
        buffers: &[kernel::sync::aref::ARef<kernel::dma_buf::DmaBuf>],
    ) -> Result<Arc<super::private_image::Image>> {
        self.candidate
            .register_pending_image(&self.registration, dimensions, buffers)
    }

    pub(crate) fn activate(
        &self,
        device: &kernel::drm::Device<crate::Driver, kernel::drm::device::Registered>,
    ) -> Result<(
        crate::renderer_startup::Active,
        super::probe::Source,
        crate::execution::Description,
    )> {
        self.candidate.activate_proposal(device, &self.registration)
    }

    pub(crate) fn handback(
        &self,
        device: &kernel::drm::Device<crate::Driver, kernel::drm::device::Registered>,
    ) -> Result<crate::execution::Description> {
        self.candidate.handback(device, &self.registration)
    }
}
