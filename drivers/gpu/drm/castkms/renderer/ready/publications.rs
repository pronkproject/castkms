// SPDX-License-Identifier: GPL-2.0-only

//! Bounded native offer withdrawal, independent of retained KMS object lifetimes.

use kernel::{
    drm::{
        constraints::{List, OpaqueEntry},
        kms::constraints::Output,
    },
    prelude::*,
    sync::aref::ARef,
};

struct Publication {
    list: ARef<List>,
    id: u64,
}

/// Endpoint revocation drains these list references outside worker and native locks.
/// While live, a list may retain an entry for this same worker. Explicit endpoint/device
/// revocation must break that cycle; neither native entry retention nor list closure does so.
pub(super) struct Publications {
    entries: KVec<Publication>,
}

impl Publications {
    pub(super) fn new() -> Result<Self> {
        Ok(Self {
            entries: KVec::with_capacity(List::MAX_ENTRIES, GFP_KERNEL)?,
        })
    }

    /// The caller holds worker readiness through registration and native publication.
    /// Failure leaves neither a new listing nor a withdrawal record behind.
    pub(super) fn publish(
        &mut self,
        output: &Output<'_, crate::Driver>,
        entry: &OpaqueEntry,
    ) -> Result {
        if self.entries.len() == List::MAX_ENTRIES {
            return Err(ENOSPC);
        }
        self.entries.push(Publication {
            list: ARef::from(output.list()),
            id: entry.id(),
        }, GFP_KERNEL)?;
        if let Err(error) = output.add(entry) {
            // The borrowed output still retains the list, excluding final native destruction.
            drop(self.entries.pop());
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn withdraw(self) {
        for publication in self.entries {
            match publication.list.withdraw(publication.id) {
                Ok(()) | Err(ENOENT) | Err(ESTALE) => (),
                // Exhausted generations cannot advertise a worker as available after loss.
                Err(_) => publication.list.close(),
            }
        }
    }
}
