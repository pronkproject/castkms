// SPDX-License-Identifier: GPL-2.0-only

//! Monotonic requests covered by independently observed worker attempts.

use super::Outcome;
use kernel::prelude::*;

pub(super) struct Progress {
    requested: u64,
    completed: Option<(u64, Outcome)>,
}

impl Progress {
    pub(super) fn new() -> Self {
        Self {
            requested: 0,
            completed: None,
        }
    }

    pub(super) fn request(&mut self) -> Result<u64> {
        self.requested = self.requested.checked_add(1).ok_or(EOVERFLOW)?;
        Ok(self.requested)
    }

    pub(super) fn starting(&self) -> u64 {
        self.requested
    }

    /// The single worker completes attempts in order; return replaced storage for unlocked drop.
    pub(super) fn finish(&mut self, through: u64, outcome: Outcome) -> Option<Outcome> {
        self.completed
            .replace((through, outcome))
            .map(|(_, outcome)| outcome)
    }

    pub(super) fn observe(&self, requested: u64) -> Option<Outcome> {
        self.completed
            .as_ref()
            .filter(|(through, _)| *through >= requested)
            .map(|(_, outcome)| outcome.clone())
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
