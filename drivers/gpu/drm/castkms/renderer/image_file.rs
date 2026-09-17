// SPDX-License-Identifier: GPL-2.0-only

//! Private-image descriptor import above renderer admission and storage policy.

use super::endpoint::Endpoint;
use kernel::{
    dma_buf::DmaBuf,
    prelude::*,
    transmute::FromBytes,
    uaccess::{UserPtr, UserSlice},
    uapi,
};

#[repr(C)]
struct Register {
    image_id: u64,
    buffers: u64,
    width: u32,
    height: u32,
    num_buffers: u32,
    flags: u32,
    reserved: [u64; 2],
}

// SAFETY: All fields are integers accepting every bit pattern.
unsafe impl FromBytes for Register {}

#[repr(C)]
struct Unregister {
    image_id: u64,
    flags: u32,
    reserved: u32,
}

// SAFETY: All fields are integers accepting every bit pattern.
unsafe impl FromBytes for Unregister {}

pub(super) fn register(endpoint: &Endpoint, arg: usize) -> Result {
    const {
        assert!(
            core::mem::size_of::<Register>()
                == core::mem::size_of::<uapi::drm_castkms_renderer_register_image>()
        );
    }
    let request = UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of::<Register>())
        .reader()
        .read::<Register>()?;
    if request.image_id == 0
        || request.buffers == 0
        || request.num_buffers == 0
        || request.num_buffers > 4
        || request.flags != 0
        || request.reserved != [0; 2]
    {
        return Err(EINVAL);
    }
    let address = usize::try_from(request.buffers).map_err(|_| EOVERFLOW)?;
    let mut reader = UserSlice::new(
        UserPtr::from_addr(address),
        request.num_buffers as usize * 4,
    )
    .reader();
    let mut buffers = KVec::with_capacity(request.num_buffers as usize, GFP_KERNEL)?;
    for _ in 0..request.num_buffers {
        buffers.push(DmaBuf::from_fd(reader.read::<i32>()?)?, GFP_KERNEL)?;
    }
    endpoint.register_image(request.image_id, [request.width, request.height], &buffers)
}

pub(super) fn unregister(endpoint: &Endpoint, arg: usize) -> Result {
    const {
        assert!(
            core::mem::size_of::<Unregister>()
                == core::mem::size_of::<uapi::drm_castkms_renderer_unregister_image>()
        );
    }
    let request = UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of::<Unregister>())
        .reader()
        .read::<Unregister>()?;
    if request.image_id == 0 || request.flags != 0 || request.reserved != 0 {
        return Err(EINVAL);
    }
    endpoint.unregister_image(request.image_id)
}
