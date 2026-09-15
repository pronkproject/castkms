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
}
