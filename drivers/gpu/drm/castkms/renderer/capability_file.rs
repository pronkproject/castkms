// SPDX-License-Identifier: GPL-2.0-only

//! Coherent capability query transport; policy stays in the session provider.

use super::{capability_description, session::Session};
use kernel::{
    prelude::*,
    transmute::{AsBytes, FromBytes},
    uaccess::{UserPtr, UserSlice},
    uapi,
};

#[repr(C)]
struct Query {
    result: u64,
    capacity: u32,
    flags: u32,
    reserved: u64,
}
// SAFETY: Integer fields accept every bit pattern.
unsafe impl FromBytes for Query {}

#[repr(C)]
struct Description {
    version: u32,
    size: u32,
    execution_profile: u32,
    flags: u32,
    execution_generation: u64,
    active_generation: u64,
    pending_generation: u64,
    transition: u64,
    validation_epoch: u64,
    active_offset: u32,
    active_size: u32,
    pending_offset: u32,
    pending_size: u32,
}
// SAFETY: All fields are initialized integers with no interior or tail padding.
unsafe impl AsBytes for Description {}

const _: () = {
    assert!(
        core::mem::size_of::<Query>()
            == core::mem::size_of::<uapi::drm_castkms_renderer_query_capabilities>()
    );
    assert!(core::mem::size_of::<Description>() == 72);
    assert!(uapi::DRM_CASTKMS_CAPABILITY_QUERY_MAX_BYTES as usize
        == core::mem::size_of::<Description>() + 2 * capability_description::MAX_BYTES);
    assert!(
        core::mem::size_of::<Description>()
            == core::mem::size_of::<uapi::drm_castkms_renderer_capabilities>()
    );
};

pub(super) fn query(session: &Session, arg: usize) -> Result {
    let request = UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of::<Query>())
        .reader()
        .read::<Query>()?;
    if request.result == 0
        || request.flags != 0
        || request.reserved != 0
        || (request.capacity as usize) < core::mem::size_of::<Description>()
    {
        return Err(EINVAL);
    }
    let result = usize::try_from(request.result).map_err(|_| EFAULT)?;
    let snapshot = session.capabilities()?;
    let active = capability_description::encode(&snapshot.validation.active)?;
    let pending = snapshot
        .pending
        .as_ref()
        .map(|pending| capability_description::encode(&pending.profile))
        .transpose()?;
    let active_offset = core::mem::size_of::<Description>();
    let pending_size = pending.as_ref().map_or(0, |bytes| bytes.len());
    let size = active_offset + active.len() + pending_size;
    let mut flags = 0;
    if snapshot.pending.is_some() {
        flags |= uapi::DRM_CASTKMS_CAPABILITY_STATE_PENDING;
        if snapshot.validation.pending.is_some_and(|(_, gated)| gated) {
            flags |= uapi::DRM_CASTKMS_CAPABILITY_STATE_GATED;
        }
    }
    let description = Description {
        version: uapi::DRM_CASTKMS_CAPABILITY_VERSION,
        size: size as u32,
        execution_profile: match snapshot.execution.profile {
            crate::execution::Profile::HostV1 => uapi::DRM_CASTKMS_EXECUTION_HOST_V1,
            crate::execution::Profile::GpuV1 => uapi::DRM_CASTKMS_EXECUTION_GPU_V1,
        },
        flags,
        execution_generation: snapshot.execution.generation,
        active_generation: snapshot.validation.generation,
        pending_generation: snapshot
            .pending
            .as_ref()
            .map_or(0, |pending| pending.generation),
        transition: snapshot
            .pending
            .as_ref()
            .map_or(0, |pending| pending.transition),
        validation_epoch: snapshot.validation.epoch,
        active_offset: active_offset as u32,
        active_size: active.len() as u32,
        pending_offset: if pending.is_some() {
            (active_offset + active.len()) as u32
        } else {
            0
        },
        pending_size: pending_size as u32,
    };
    let mut writer = UserSlice::new(UserPtr::from_addr(result), request.capacity as usize).writer();
    writer.write(&description)?;
    if (request.capacity as usize) < size {
        return Err(ENOSPC);
    }
    writer.write_slice(&active)?;
    if let Some(pending) = pending {
        writer.write_slice(&pending)?;
    }
    Ok(())
}
