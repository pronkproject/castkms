// SPDX-License-Identifier: GPL-2.0-only

//! Current access to one output, independent of grant transport and pixel storage.

use crate::{
    display,
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
            connector::{
                Connector,
                RawConnector, //
            },
            crtc::{
                Crtc,
                CrtcRef, //
            }, //
        },
        Device, //
    },
    prelude::*,
    sync::aref::ARef,
    types::NotThreadSafe, //
};

/// Retained recipient and exact display objects, not a continuing authorization token.
///
/// Drop outside native master, object-ID and modeset locks: the retained references may
/// perform final DRM cleanup. Device-owned state must not retain permissions indefinitely.
pub(crate) struct Permission {
    master: MasterRef<Driver>,
    crtc: CrtcRef<display::Crtc>,
    connector: ARef<Connector<display::Connector>>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Permission {
    /// Borrow the exact target's device for resource operations, not pixel authorization.
    pub(super) fn device(&self) -> &Device<Driver> {
        self.crtc.drm_dev()
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
        if guard.master().is_lessee() || !guard.holds_object(crtc) || !guard.holds_object(connector)
        {
            return Err(EACCES);
        }
        Ok(Self {
            master: guard.master().clone(),
            crtc: crtc.to_owned_ref(),
            connector: connector.into(),
        })
    }

    /// Check access and accepted ownership while excluding changes through the callback.
    ///
    /// Lock order is native master, object IDs, then accepted output. The callback may
    /// register or claim a native capture job, but must not read pixels, acquire modeset
    /// locks, wait for rendering, or release final DRM object references. Returned data
    /// is historical metadata, not permission to make a later unchecked claim.
    pub(crate) fn with_current<R>(&self, f: impl FnOnce(Current<'_>) -> Result<R>) -> Result<R> {
        let guard = self.master.lock_current().ok_or(EACCES)?;
        if !guard.holds_object(self.crtc.crtc()) || !guard.holds_object(&*self.connector) {
            return Err(EACCES);
        }
        let output = &self.crtc.drm_dev().output;
        output.with_accepted(|accepted| {
            let accepted = accepted.ok_or(ENODEV)?;
            let configuration = accepted.configuration.as_ref().ok_or(ENODEV)?;
            if configuration.connector_mask() & self.connector.mask() == 0 {
                return Err(EACCES);
            }
            let scene = accepted.scene.ok_or(EAGAIN)?;
            if scene.owner() != Some(&self.master) {
                return Err(EACCES);
            }
            let [width, height] = configuration.dimensions();
            f(Current {
                configuration,
                layout: Layout::new(width, height)?,
                output: output.identity(),
                master: &self.master,
                _task: NotThreadSafe,
            })
        })
    }
}

/// Callback-local evidence of current access; neither sendable nor independently constructible.
pub(crate) struct Current<'a> {
    configuration: &'a Configuration,
    layout: Layout,
    output: &'a Identity,
    master: &'a MasterRef<Driver>,
    _task: NotThreadSafe,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Current<'_> {
    pub(crate) fn configuration(&self) -> &Configuration {
        self.configuration
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
        if output != self.output
            || configuration != Some(self.configuration)
            || layout != self.layout
            || owner != Some(self.master)
        {
            return Err(EACCES);
        }
        Ok(())
    }
}
