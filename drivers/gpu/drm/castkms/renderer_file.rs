// SPDX-License-Identifier: GPL-2.0-only

//! Primary-node adapter that publishes checked renderer capability files.

use crate::{Driver, File as DriverFile};
use kernel::{
    cred::{self, Capability},
    drm::{device::Registered, file::File as DrmFile, Device},
    fs::file::FileDescriptorReservation,
    prelude::*,
    transmute::AsBytes,
    uaccess::{UserPtr, UserSlice},
    uapi, //
};

#[repr(C)]
struct RendererFiles {
    renderer_fd: i32,
    revoke_fd: i32,
}

// SAFETY: RendererFiles contains only integers and has no padding.
unsafe impl AsBytes for RendererFiles {}

pub(crate) fn create(
    dev: &Device<Driver, Registered>,
    _: &(),
    request: &mut uapi::drm_castkms_create_renderer,
    file: &DrmFile<DriverFile>,
) -> Result<u32> {
    const {
        assert!(
            core::mem::size_of::<RendererFiles>()
                == core::mem::size_of::<uapi::drm_castkms_renderer_files>()
        )
    };
    let administrative = request.flags & uapi::DRM_CASTKMS_RENDERER_CREATE_ADMIN != 0;
    if request.crtc_id == 0
        || request.connector_id == 0
        || request.flags & !uapi::DRM_CASTKMS_RENDERER_CREATE_ADMIN != 0
        || request.reserved.iter().any(|field| *field != 0)
    {
        return Err(EINVAL);
    }
    if administrative && !cred::capable_in_initial_user_namespace(Capability::SysAdmin) {
        return Err(EACCES);
    }
    let renderer_reservation =
        FileDescriptorReservation::get_unused_fd_flags(kernel::fs::file::flags::O_CLOEXEC)?;
    let revoker_reservation =
        FileDescriptorReservation::get_unused_fd_flags(kernel::fs::file::flags::O_CLOEXEC)?;
    let files = DriverFile::create_renderer_files(
        dev,
        file,
        request.crtc_id,
        request.connector_id,
        administrative,
    )?;
    let (renderer, revoker) = files.into_files();
    let output = RendererFiles {
        renderer_fd: renderer_reservation
            .reserved_fd()
            .try_into()
            .map_err(|_| EOVERFLOW)?,
        revoke_fd: revoker_reservation
            .reserved_fd()
            .try_into()
            .map_err(|_| EOVERFLOW)?,
    };
    let address = request.files.try_into().map_err(|_| EOVERFLOW)?;
    UserSlice::new(UserPtr::from_addr(address), core::mem::size_of_val(&output))
        .writer()
        .write(&output)?;
    // No fallible operation remains after descriptor publication.
    renderer_reservation.fd_install(renderer);
    revoker_reservation.fd_install(revoker);
    Ok(0)
}
