// SPDX-License-Identifier: GPL-2.0-only

//! Pending capability metadata; neither renderer authority nor an activation gate.

use super::{publication::Publication, validation::Contract, Description};
use kernel::{prelude::*, sync::Arc};

/// An immutable observation, not continuing permission to act on its proposal.
#[derive(Clone)]
pub(crate) struct DescriptionSnapshot {
    /// Monotonic within one publication, including cancelled proposals.
    /// Separate from the active execution generation and any validation epoch.
    pub(crate) generation: u64,
    /// Device-scoped atomic transition identity, not a permission to render.
    pub(crate) transition: u64,
    pub(crate) expected: Description,
    pub(crate) profile: Contract,
}

pub(super) fn next_generation(previous: u64) -> Result<u64> {
    previous.checked_add(1).ok_or(EOVERFLOW)
}

pub(super) struct Entry {
    pub(super) description: DescriptionSnapshot,
    pub(super) worker: Arc<()>,
    pub(super) reservation: super::coordinator::Reservation,
}

/// Unique cancellation ownership. Drop outside display and admission locks.
/// Retained observations do not keep a cancelled proposal installed.
#[must_use = "dropping the registration cancels its pending profile"]
pub(crate) struct Registration {
    pub(super) publication: Arc<Publication>,
    pub(super) description: DescriptionSnapshot,
}

impl Registration {
    pub(crate) fn description(&self) -> &DescriptionSnapshot {
        &self.description
    }

    pub(crate) fn check(&self) -> Result {
        self.publication.check_proposal(self.description.generation)
    }

    pub(crate) fn cancel(&self) {
        self.publication
            .cancel_proposal(self.description.generation);
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_proposal_generation)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_generations_are_not_reused_or_wrapped() {
        assert_eq!(next_generation(1), Ok(2));
        assert_eq!(next_generation(2), Ok(3));
        assert_eq!(next_generation(u64::MAX), Err(EOVERFLOW));
    }
}
