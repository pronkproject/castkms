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
}

impl drm::file::DriverFile for File {
    type Driver = Driver;

    fn open(_: &drm::Device<Driver>) -> Result<Pin<KBox<Self>>> {
        Ok(KBox::new(
            Self {
                grants: Creator::new()?,
            },
            GFP_KERNEL,
        )?
        .into())
    }
}

impl File {
    /// Resolve the issuing file's IDs before entering the existing grant policy boundary.
    pub(crate) fn create_capture_files(
        dev: &drm::Device<Driver, Registered>,
        file: &drm::file::File<Self>,
        target: Target,
    ) -> Result<FilePair> {
        let crtc = dev.lookup_crtc(file, target.crtc_id())?;
        let connector = dev.lookup_connector(file, target.connector_id())?;
        Self::create_capture_grant(file, crtc.crtc(), &connector)?
            .into_files_with(|capture| Ok(crate::capture::client::Client::new(capture)))
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
