// SPDX-License-Identifier: GPL-2.0-only

//! Current access to one output, independent of grant transport and pixel storage.

use crate::{
    display,
    display_control,
    host_compositor::{
        compose::Completed,
        layout::Layout, //
    },
    host_snapshot::Snapshot,
    output::Identity,
    scene::Configuration,
    Driver, //
};
use kernel::{
    drm::{
        auth::{
            CurrentMasterGuard,
            MasterRef, //
        },
        kms::{
            connector::Connector,
            crtc::Crtc, //
        },
        Device, //
    },
    prelude::*, //
};

/// Retained recipient and exact display objects, not a continuing authorization token.
///
/// Drop outside native master, object-ID and modeset locks: the retained references may
/// perform final DRM cleanup. Device-owned state must not retain permissions indefinitely.
pub(crate) struct Permission {
    target: display_control::Target,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Permission {
    /// Borrow the exact target's device for resource operations, not pixel authorization.
    pub(super) fn device(&self) -> &Device<Driver> {
        self.target.device()
    }

    /// Establish a kernel-issued target from stabilized top-level display control.
    ///
    /// File issuance must additionally verify the issuing file's master role and retain
    /// its revocation ownership. A retained master snapshot alone is not accepted here.
    pub(crate) fn new(
        guard: &CurrentMasterGuard<'_, Driver>,
        crtc: &Crtc<display::Crtc>,
        connector: &Connector<display::Connector>,
    ) -> Result<Self> {
        Ok(Self {
            target: display_control::Target::new(guard, crtc, connector)?,
        })
    }

    /// Check access and accepted ownership while excluding changes through the callback.
    ///
    /// Lock order is native master, object IDs, then accepted output. The callback may
    /// register or claim a native capture job, but must not read pixels, acquire modeset
    /// locks, wait for rendering, or release final DRM object references. Returned data
    /// is historical metadata, not permission to make a later unchecked claim.
    pub(crate) fn with_current<R>(&self, f: impl FnOnce(Current<'_>) -> Result<R>) -> Result<R> {
        self.target.with_current(|control| {
            control.check_scene_owner()?;
            let [width, height] = control.configuration().dimensions();
            f(Current {
                control,
                layout: Layout::new(width, height)?,
            })
        })
    }
}

/// Callback-local evidence of current access; neither sendable nor independently constructible.
pub(crate) struct Current<'a> {
    control: display_control::Current<'a>,
    layout: Layout,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Current<'_> {
    pub(crate) fn configuration(&self) -> &Configuration {
        self.control.configuration()
    }

    pub(crate) fn layout(&self) -> Layout {
        self.layout
    }

    /// Check independently completed pixels without confusing retention with permission.
    ///
    /// A completed image may precede the latest content update within the same display
    /// interval. Its original content serial remains unchanged; it is not relabeled current.
    pub(crate) fn check_image(&self, image: &Completed) -> Result {
        self.check_origin(
            image.output_identity(),
            image.configuration(),
            image.layout(),
            image.owner(),
        )
    }

    /// Validate an independent copy against current access without relabeling its content.
    ///
    /// Like an ordinary completed image, a snapshot may contain earlier content within the
    /// same authorized interval. Success applies only while this callback's guards are held.
    pub(crate) fn check_snapshot(&self, snapshot: &Snapshot) -> Result {
        self.check_origin(
            snapshot.output_identity(),
            snapshot.configuration(),
            snapshot.layout(),
            snapshot.owner(),
        )
    }

    fn check_origin(
        &self,
        output: &Identity,
        configuration: Option<&Configuration>,
        layout: Layout,
        owner: Option<&MasterRef<Driver>>,
    ) -> Result {
        if output != self.control.output_identity()
            || configuration != Some(self.control.configuration())
            || layout != self.layout
            || owner != Some(self.control.master())
        {
            return Err(EACCES);
        }
        Ok(())
    }
}
