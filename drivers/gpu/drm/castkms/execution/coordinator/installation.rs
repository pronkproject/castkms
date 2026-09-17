// SPDX-License-Identifier: GPL-2.0-only

//! Borrow the whole validation cohort until native installation succeeds.

use super::*;

const OUTPUTS: usize = crate::device::MAX_OUTPUTS as usize;

pub(crate) struct Update<'a> {
    pub(crate) scene: SceneView<'a>,
}

pub(crate) struct Retired;

#[must_use = "commit only in the native installation success continuation"]
pub(crate) struct Prepared;

impl Guard<'_> {
    pub(crate) fn prepare(&mut self, updates: &[Option<Update<'_>>; OUTPUTS]) -> Result<Prepared> {
        for (index, update) in updates.iter().enumerate() {
            if let Some(update) = update { self.check(index, update.scene)?; }
        }
        Ok(Prepared)
    }
}

impl Prepared {
    pub(crate) fn commit(self) -> Retired { Retired }
}
