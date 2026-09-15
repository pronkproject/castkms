// SPDX-License-Identifier: GPL-2.0-only

//! DRM file lifetime and checked issuance, above the kernel capture provider.

use crate::{
    capture::{
        permission::Permission,
        provider::{
            Creator,
            Grantor, //
        }, //
    },
    display,
    renderer::permission::{Owner as RendererOwner, Permission as RendererPermission},
    Driver, //
};
use kernel::{
    drm::{
        self,
        capture::{
            FilePair,
            Target, //
        },
        device::Registered,
        kms::{
            connector::Connector,
            crtc::Crtc, //
        }, //
    },
    prelude::*, //
};

pub(crate) struct File {
    grants: Creator,
    #[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
    audio_grants: drm::capture::Creator,
}

impl drm::file::DriverFile for File {
    type Driver = Driver;

    fn open(_: &drm::Device<Driver>) -> Result<Pin<KBox<Self>>> {
        Ok(KBox::new(
            Self {
                grants: Creator::new()?,
                #[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
                audio_grants: drm::capture::Creator::new(64)?,
            },
            GFP_KERNEL,
        )?
        .into())
    }
}

impl File {
    #[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
    pub(crate) fn create_audio_owner(
        dev: &drm::Device<Driver, Registered>,
        file: &drm::file::File<Self>,
        crtc_id: u32,
        connector_id: u32,
    ) -> Result<crate::audio::provider::Owner> {
        let crtc = dev.lookup_crtc(file, crtc_id)?;
        let connector = dev.lookup_connector(file, connector_id)?;
        Self::issue_audio_owner(file, crtc.crtc(), &connector)
    }

    #[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
    pub(crate) fn issue_audio_owner(
        file: &drm::file::File<Self>,
        crtc: &Crtc<display::Crtc>,
        connector: &Connector<display::Connector>,
    ) -> Result<crate::audio::provider::Owner> {
        let snapshot = file.master_snapshot().ok_or(EACCES)?;
        let target = {
            let guard = snapshot.master().lock_current().ok_or(EACCES)?;
            if !guard.is_master_file(file) {
                return Err(EACCES);
            }
            crate::display_control::Target::new(&guard, crtc, connector)?
        };
        let mut owner = crate::audio::provider::Owner::new(target)?;
        {
            let guard = snapshot.master().lock_current().ok_or(EACCES)?;
            if !guard.is_master_file(file)
                || !guard.holds_object(crtc)
                || !guard.holds_object(connector)
            {
                return Err(EACCES);
            }
            owner.track_creator(&file.inner().audio_grants)?;
        }
        owner.access().check()?;
        Ok(owner)
    }

    /// Issue renderer control from one current master interval.
    ///
    /// Construction runs outside native DRM locks. The second check prevents a file or
    /// display-object ownership change during allocation from authorizing the result.
    #[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
    pub(crate) fn issue_renderer_control(
        file: &drm::file::File<Self>,
        crtc: &Crtc<display::Crtc>,
        connector: &Connector<display::Connector>,
    ) -> Result<RendererOwner> {
        Self::issue_renderer_control_then(file, crtc, connector, || Ok(()))
    }

    /// Resolve file-visible IDs before issuing anonymous renderer endpoints.
    pub(crate) fn create_renderer_files(
        dev: &drm::Device<Driver, Registered>,
        file: &drm::file::File<Self>,
        crtc_id: u32,
        connector_id: u32,
    ) -> Result<crate::renderer::files::Files> {
        let crtc = dev.lookup_crtc(file, crtc_id)?;
        let connector = dev.lookup_connector(file, connector_id)?;
        Self::issue_renderer_control(file, crtc.crtc(), &connector)?.into_files(dev)
    }

    fn issue_renderer_control_then(
        file: &drm::file::File<Self>,
        crtc: &Crtc<display::Crtc>,
        connector: &Connector<display::Connector>,
        after_create: impl FnOnce() -> Result,
    ) -> Result<RendererOwner> {
        let snapshot = file.master_snapshot().ok_or(EACCES)?;
        let permission = {
            let guard = snapshot.master().lock_current().ok_or(EACCES)?;
            if !guard.is_master_file(file) {
                return Err(EACCES);
            }
            RendererPermission::new(&guard, crtc, connector)?
        };
        let owner = RendererOwner::new(permission)?;
        after_create()?;
        {
            let guard = snapshot.master().lock_current().ok_or(EACCES)?;
            if !guard.is_master_file(file)
                || !guard.holds_object(crtc)
                || !guard.holds_object(connector)
            {
                return Err(EACCES);
            }
        }
        owner.access().with_current(|_| Ok(()))?;
        Ok(owner)
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn issue_renderer_control_then_for_test(
        file: &drm::file::File<Self>,
        crtc: &Crtc<display::Crtc>,
        connector: &Connector<display::Connector>,
        after_create: impl FnOnce() -> Result,
    ) -> Result<RendererOwner> {
        Self::issue_renderer_control_then(file, crtc, connector, after_create)
    }

    /// Resolve the issuing file's IDs before entering the existing grant policy boundary.
    pub(crate) fn create_capture_files(
        dev: &drm::Device<Driver, Registered>,
        file: &drm::file::File<Self>,
        target: Target,
    ) -> Result<FilePair> {
        let crtc = dev.lookup_crtc(file, target.crtc_id())?;
        let connector = dev.lookup_connector(file, target.connector_id())?;
        Self::create_capture_grant(file, crtc.crtc(), &connector)?
            .into_files_with(crate::capture::client::Client::new)
    }

    /// Issue through a current master file without granting any public ioctl by implication.
    ///
    /// The file role and exact display objects are checked separately. Allocate the provider
    /// outside native locks, then recheck the same file role while attaching its close owner.
    /// The live file borrow excludes final close throughout issuance. No framebuffer pixels
    /// are accessed, and stream creation still needs current display and recipient checks.
    pub(crate) fn create_capture_grant(
        file: &drm::file::File<Self>,
        crtc: &Crtc<display::Crtc>,
        connector: &Connector<display::Connector>,
    ) -> Result<Grantor> {
        let snapshot = file.master_snapshot().ok_or(EACCES)?;
        let permission = {
            let guard = snapshot.master().lock_current().ok_or(EACCES)?;
            if !guard.is_master_file(file) {
                return Err(EACCES);
            }
            Permission::new(&guard, crtc, connector)?
        };
        let mut grantor = Grantor::new(permission)?;
        {
            let guard = snapshot.master().lock_current().ok_or(EACCES)?;
            if !guard.is_master_file(file)
                || !guard.holds_object(crtc)
                || !guard.holds_object(connector)
            {
                return Err(EACCES);
            }
            grantor.track_creator(&file.inner().grants)?;
        }
        Ok(grantor)
    }
}
