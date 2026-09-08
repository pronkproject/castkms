// SPDX-License-Identifier: GPL-2.0-only

//! Attribution of framebuffer creation, separate from current capture permission.

use kernel::drm::auth::{
    MasterRef,
    MasterSnapshot, //
};

enum Origin<I> {
    Unknown,
    Creator(I),
    Associated(I),
}

/// Immutable creation evidence. An association requires a matching current identity before
/// it identifies an owner; a master creator remains attributable after a master handoff.
pub(super) struct Provenance<I = MasterRef<super::Driver>> {
    origin: Origin<I>,
}

impl Provenance {
    pub(super) fn from_snapshot(snapshot: Option<MasterSnapshot<super::Driver>>) -> Self {
        let origin = match snapshot {
            Some(snapshot) if snapshot.was_current() => Origin::Creator(snapshot.master().clone()),
            Some(snapshot) => Origin::Associated(snapshot.master().clone()),
            None => Origin::Unknown,
        };
        Self { origin }
    }
}

impl<I: Eq> Provenance<I> {
    /// Resolve creation evidence without changing it. Unknown provenance never becomes an
    /// owner merely because a master is now active.
    pub(super) fn owner<'a>(&'a self, current: Option<&I>) -> Option<&'a I> {
        match &self.origin {
            Origin::Creator(owner) => Some(owner),
            Origin::Associated(owner) if current == Some(owner) => Some(owner),
            _ => None,
        }
    }

    /// An explicit selection of a different framebuffer permits adoption by the committing
    /// master. Geometry-only changes and same-framebuffer content updates do not.
    pub(super) fn committed_owner<'a>(
        &'a self,
        current: Option<&'a I>,
        selection: Selection,
    ) -> Option<&'a I> {
        match (selection, current) {
            (Selection::DifferentFramebuffer, Some(current)) => Some(current),
            _ => self.owner(current),
        }
    }
}

/// The caller must distinguish explicit framebuffer selection from inherited atomic state.
pub(super) enum Selection {
    RetainedFramebuffer,
    DifferentFramebuffer,
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[path = "provenance/tests.rs"]
mod tests;
