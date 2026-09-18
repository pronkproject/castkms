// SPDX-License-Identifier: GPL-2.0-only

//! DRM file lifetime and checked issuance, above the kernel capture provider.

use crate::{
    authority::grants,
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
    sync::Arc,
};

pub(crate) struct File {
    grants: Creator,
    renderer_grants: drm::capture::Creator,
    #[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
    audio_grants: drm::capture::Creator,
}

#[derive(Clone, Copy)]
enum IssuanceOrigin<'a> {
    Master,
    Administrative(&'a drm::Device<Driver>),
}

impl drm::file::DriverFile for File {
    type Driver = Driver;

    fn open(_: &drm::Device<Driver>) -> Result<Pin<KBox<Self>>> {
        Ok(KBox::new(
            Self {
                grants: Creator::new()?,
                renderer_grants: drm::capture::Creator::new(64)?,
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

    /// Issue renderer control for the current master identity.
    ///
    /// Construction runs outside native DRM locks. The second check prevents a file or
    /// display-object ownership change during allocation from authorizing the result.
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
        administrative: bool,
    ) -> Result<crate::renderer::files::Files> {
        let (crtc, connector) = if administrative {
            (
                dev.lookup_crtc_unfiltered(crtc_id)?,
                dev.lookup_connector_unfiltered(connector_id)?,
            )
        } else {
            (
                dev.lookup_crtc(file, crtc_id)?,
                dev.lookup_connector(file, connector_id)?,
            )
        };
        let owner = if administrative {
            Self::issue_administrative_renderer_control(dev, file, crtc.crtc(), &connector)?
        } else {
            Self::issue_renderer_control(file, crtc.crtc(), &connector)?
        };
        crate::renderer::files::Files::new(owner, dev)
    }

    fn issue_administrative_renderer_control(
        dev: &drm::Device<Driver>,
        file: &drm::file::File<Self>,
        crtc: &Crtc<display::Crtc>,
        connector: &Connector<display::Connector>,
    ) -> Result<RendererOwner> {
        Self::issue_renderer_control_from(
            file,
            crtc,
            connector,
            IssuanceOrigin::Administrative(dev),
            || Ok(()),
        )
    }

    fn issue_renderer_control_then(
        file: &drm::file::File<Self>,
        crtc: &Crtc<display::Crtc>,
        connector: &Connector<display::Connector>,
        after_create: impl FnOnce() -> Result,
    ) -> Result<RendererOwner> {
        Self::issue_renderer_control_from(
            file,
            crtc,
            connector,
            IssuanceOrigin::Master,
            after_create,
        )
    }

    fn issue_renderer_control_from(
        file: &drm::file::File<Self>,
        crtc: &Crtc<display::Crtc>,
        connector: &Connector<display::Connector>,
        origin: IssuanceOrigin<'_>,
        after_create: impl FnOnce() -> Result,
    ) -> Result<RendererOwner> {
        let (master, interval) = match origin {
            IssuanceOrigin::Master => {
                (file.master_snapshot().ok_or(EACCES)?.master().clone(), None)
            }
            IssuanceOrigin::Administrative(dev) => {
                let master = dev.authority.snapshot().ok_or(EBUSY)?;
                let interval = dev.authority.interval().map_err(|error| {
                    if error == EACCES {
                        EBUSY
                    } else {
                        error
                    }
                })?;
                (master, Some(interval))
            }
        };
        let permission = {
            let guard =
                master
                    .lock_current()
                    .ok_or(if interval.is_some() { EBUSY } else { EACCES })?;
            if interval.is_some() {
                if guard.is_master_file(file) {
                    return Err(EBUSY);
                }
                if !guard.exclusively_holds_object(crtc)
                    || !guard.exclusively_holds_object(connector)
                {
                    return Err(EBUSY);
                }
                RendererPermission::new(&guard, crtc, connector)?
            } else {
                if !guard.is_master_file(file) {
                    return Err(EACCES);
                }
                RendererPermission::new(&guard, crtc, connector)?
            }
        };
        let mut owner = RendererOwner::new(permission)?;
        after_create()?;
        {
            let guard =
                master
                    .lock_current()
                    .ok_or(if interval.is_some() { ESTALE } else { EACCES })?;
            if interval.is_some()
                && (!guard.exclusively_holds_object(crtc)
                    || !guard.exclusively_holds_object(connector))
            {
                return Err(EBUSY);
            }
            if (interval.is_none() && !guard.is_master_file(file))
                || (interval.is_some() && guard.is_master_file(file))
                || !guard.holds_object(crtc)
                || !guard.holds_object(connector)
            {
                return Err(EACCES);
            }
            if let (IssuanceOrigin::Administrative(dev), Some(expected)) = (origin, interval) {
                if dev.authority.interval() != Ok(expected) {
                    return Err(ESTALE);
                }
            }
            owner.track_creator(&file.inner().renderer_grants)?;
        }
        owner.access().with_output(|| Ok(()))?;
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

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn issue_administrative_renderer_control_then_for_test(
        dev: &drm::Device<Driver>,
        file: &drm::file::File<Self>,
        crtc: &Crtc<display::Crtc>,
        connector: &Connector<display::Connector>,
        after_create: impl FnOnce() -> Result,
    ) -> Result<RendererOwner> {
        Self::issue_renderer_control_from(
            file,
            crtc,
            connector,
            IssuanceOrigin::Administrative(dev),
            after_create,
        )
    }

    /// Resolve the issuing file's IDs before entering the existing grant policy boundary.
    pub(crate) fn create_capture_files(
        dev: &drm::Device<Driver, Registered>,
        file: &drm::file::File<Self>,
        target: Target,
        origin: drm::capture::Origin,
    ) -> Result<FilePair> {
        let administrative = origin == drm::capture::Origin::Administrative;
        let (crtc, connector) = if administrative {
            (
                dev.lookup_crtc_unfiltered(target.crtc_id())?,
                dev.lookup_connector_unfiltered(target.connector_id())?,
            )
        } else {
            (
                dev.lookup_crtc(file, target.crtc_id())?,
                dev.lookup_connector(file, target.connector_id())?,
            )
        };
        let origin = if administrative {
            IssuanceOrigin::Administrative(dev)
        } else {
            IssuanceOrigin::Master
        };
        Self::issue_capture_grant_from(file, crtc.crtc(), &connector, origin, || Ok(()))?
            .into_files_with(crate::capture::client::Client::new)
    }

    /// Issue one independently scoped final-image grant for every group member.
    pub(crate) fn create_group_capture_files(
        dev: &drm::Device<Driver, Registered>,
        file: &drm::file::File<Self>,
        targets: &[(u32, u32)],
        administrative: bool,
        group: &Arc<grants::Registry>,
    ) -> Result<KVec<FilePair>> {
        let mut objects = KVec::with_capacity(targets.len(), GFP_KERNEL)?;
        for &(crtc_id, connector_id) in targets {
            let object = if administrative {
                (
                    dev.lookup_crtc_unfiltered(crtc_id)?,
                    dev.lookup_connector_unfiltered(connector_id)?,
                )
            } else {
                (
                    dev.lookup_crtc(file, crtc_id)?,
                    dev.lookup_connector(file, connector_id)?,
                )
            };
            objects.push(object, GFP_KERNEL)?;
        }

        let (master, interval) =
            if administrative {
                let master = dev.authority.snapshot().ok_or(EBUSY)?;
                let interval = dev.authority.interval().map_err(|error| {
                    if error == EACCES {
                        EBUSY
                    } else {
                        error
                    }
                })?;
                (master, Some(interval))
            } else {
                (file.master_snapshot().ok_or(EACCES)?.master().clone(), None)
            };
        let mut permissions = KVec::with_capacity(objects.len(), GFP_KERNEL)?;
        {
            let guard = master
                .lock_current()
                .ok_or(if administrative { EBUSY } else { EACCES })?;
            if administrative == guard.is_master_file(file) {
                return Err(if administrative { EBUSY } else { EACCES });
            }
            for (crtc, connector) in &objects {
                if administrative
                    && (!guard.exclusively_holds_object(crtc.crtc())
                        || !guard.exclusively_holds_object(&**connector))
                {
                    return Err(EBUSY);
                }
                let permission = if let Some(interval) = interval {
                    Permission::administrative(&guard, crtc.crtc(), connector, interval)?
                } else {
                    Permission::new(&guard, crtc.crtc(), connector)?
                };
                permissions.push(permission, GFP_KERNEL)?;
            }
        }

        let mut grantors = KVec::with_capacity(permissions.len(), GFP_KERNEL)?;
        for permission in permissions {
            grantors.push(Grantor::new(permission)?, GFP_KERNEL)?;
        }
        {
            let guard =
                master
                    .lock_current()
                    .ok_or(if administrative { ESTALE } else { EACCES })?;
            if administrative == guard.is_master_file(file) {
                return Err(if administrative { EBUSY } else { EACCES });
            }
            if interval.is_some_and(|expected| dev.authority.interval() != Ok(expected)) {
                return Err(ESTALE);
            }
            for (crtc, connector) in &objects {
                if !guard.holds_object(crtc.crtc()) || !guard.holds_object(&**connector) {
                    return Err(EACCES);
                }
                if administrative
                    && (!guard.exclusively_holds_object(crtc.crtc())
                        || !guard.exclusively_holds_object(&**connector))
                {
                    return Err(EBUSY);
                }
            }
            for grantor in &mut grantors {
                grantor.track_creator(&file.inner().grants)?;
                grantor.track_group(group)?;
            }
        }

        let mut files = KVec::with_capacity(grantors.len(), GFP_KERNEL)?;
        for grantor in grantors {
            files.push(
                grantor.into_files_with(crate::capture::client::Client::new)?,
                GFP_KERNEL,
            )?;
        }
        Ok(files)
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
        Self::issue_capture_grant_from(
            file,
            crtc,
            connector,
            IssuanceOrigin::Master,
            || Ok(()),
        )
    }

    fn issue_capture_grant_from(
        file: &drm::file::File<Self>,
        crtc: &Crtc<display::Crtc>,
        connector: &Connector<display::Connector>,
        origin: IssuanceOrigin<'_>,
        after_create: impl FnOnce() -> Result,
    ) -> Result<Grantor> {
        let (master, interval) = match origin {
            IssuanceOrigin::Master => {
                (file.master_snapshot().ok_or(EACCES)?.master().clone(), None)
            }
            IssuanceOrigin::Administrative(dev) => {
                let master = dev.authority.snapshot().ok_or(EBUSY)?;
                let interval = dev.authority.interval().map_err(|error| {
                    if error == EACCES {
                        EBUSY
                    } else {
                        error
                    }
                })?;
                (master, Some(interval))
            }
        };
        let permission = {
            let guard = master
                .lock_current()
                .ok_or(if interval.is_some() { EBUSY } else { EACCES })?;
            if let Some(interval) = interval {
                if guard.is_master_file(file) {
                    return Err(EBUSY);
                }
                if !guard.exclusively_holds_object(crtc)
                    || !guard.exclusively_holds_object(connector)
                {
                    return Err(EBUSY);
                }
                Permission::administrative(&guard, crtc, connector, interval)?
            } else {
                if !guard.is_master_file(file) {
                    return Err(EACCES);
                }
                Permission::new(&guard, crtc, connector)?
            }
        };
        let mut grantor = Grantor::new(permission)?;
        after_create()?;
        {
            let guard = master
                .lock_current()
                .ok_or(if interval.is_some() { ESTALE } else { EACCES })?;
            if interval.is_some()
                && (!guard.exclusively_holds_object(crtc)
                    || !guard.exclusively_holds_object(connector))
            {
                return Err(EBUSY);
            }
            if (interval.is_none() && !guard.is_master_file(file))
                || (interval.is_some() && guard.is_master_file(file))
                || !guard.holds_object(crtc)
                || !guard.holds_object(connector)
            {
                return Err(EACCES);
            }
            if let (IssuanceOrigin::Administrative(dev), Some(expected)) = (origin, interval) {
                if dev.authority.interval() != Ok(expected) {
                    return Err(ESTALE);
                }
            }
            grantor.track_creator(&file.inner().grants)?;
        }
        Ok(grantor)
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn issue_administrative_capture_grant_then_for_test(
        dev: &drm::Device<Driver>,
        file: &drm::file::File<Self>,
        crtc: &Crtc<display::Crtc>,
        connector: &Connector<display::Connector>,
        after_create: impl FnOnce() -> Result,
    ) -> Result<Grantor> {
        Self::issue_capture_grant_from(
            file,
            crtc,
            connector,
            IssuanceOrigin::Administrative(dev),
            after_create,
        )
    }
}
