// SPDX-License-Identifier: GPL-2.0-only

//! Transactional publication of one private-image-to-recipient output claim.

use super::endpoint::Endpoint;
use kernel::{
    fs::file::FileDescriptorReservation,
    prelude::*,
    transmute::{AsBytes, FromBytes},
    uaccess::{UserPtr, UserSlice},
    uapi,
};

#[repr(C)]
struct Request {
    result: u64,
    image_id: u64,
    flags: u32,
    reserved: u32,
    padding: u64,
}
unsafe impl FromBytes for Request {}

#[repr(C)]
struct ResultRecord {
    job_id: u64,
    image_id: u64,
    width: u32,
    height: u32,
    format: u32,
    memory_plane_count: u32,
    modifier: u64,
    dma_buf_fd: i32,
    pitch: u32,
    offset: u64,
    reserved: [u64; 2],
}
unsafe impl AsBytes for ResultRecord {}

const _: () = {
    assert!(core::mem::size_of::<Request>()
        == core::mem::size_of::<uapi::drm_castkms_renderer_acquire_output>());
    assert!(core::mem::size_of::<ResultRecord>()
        == core::mem::size_of::<uapi::drm_castkms_renderer_output>());
};

pub(super) fn acquire(endpoint: &Endpoint, arg: usize) -> Result {
    let request = UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of::<Request>())
        .reader().read::<Request>()?;
    if request.result == 0 || request.image_id == 0 || request.flags != 0
        || request.reserved != 0 || request.padding != 0
    {
        return Err(EINVAL);
    }
    let pending = endpoint.begin_output(request.image_id)?;
    let destination = pending.destination()?;
    let layout = destination.layout();
    let reservation = FileDescriptorReservation::get_unused_fd_flags(
        kernel::fs::file::flags::O_CLOEXEC,
    )?;
    let fd = i32::try_from(reservation.reserved_fd()).map_err(|_| EOVERFLOW)?;
    let file = destination.buffer().to_file();
    let result = ResultRecord {
        job_id: pending.id(),
        image_id: pending.image_id(),
        width: layout.dimensions[0],
        height: layout.dimensions[1],
        format: destination.format(),
        memory_plane_count: 1,
        modifier: destination.modifier(),
        dma_buf_fd: fd,
        pitch: layout.pitch.try_into().map_err(|_| EOVERFLOW)?,
        offset: layout.offset.try_into().map_err(|_| EOVERFLOW)?,
        reserved: [0; 2],
    };
    let address = usize::try_from(request.result).map_err(|_| EOVERFLOW)?;
    UserSlice::new(UserPtr::from_addr(address), core::mem::size_of::<ResultRecord>())
        .writer().write(&result)?;
    pending.publish(move || reservation.fd_install(file))
}
