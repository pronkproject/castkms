// SPDX-License-Identifier: GPL-2.0-only

//! Profile registration transport; policy and committed lifetime stay in the session.

use super::{capability_description, session::Session};
use kernel::{
    prelude::*,
    transmute::{AsBytes, FromBytes},
    uaccess::{UserPtr, UserSlice},
    uapi,
};

#[repr(C)]
struct Register {
    candidate_id: u64,
    profile: u64,
    result: u64,
    profile_size: u32,
    flags: u32,
    reserved: [u64; 2],
}
// SAFETY: Integer fields accept every bit pattern.
unsafe impl FromBytes for Register {}

#[repr(C)]
struct Registered {
    transition: u64,
    capability_generation: u64,
    execution_generation: u64,
    reserved: u64,
}
// SAFETY: Every byte belongs to an initialized integer; there is no padding.
unsafe impl AsBytes for Registered {}

const _: () = {
    assert!(
        core::mem::size_of::<Register>()
            == core::mem::size_of::<uapi::drm_castkms_renderer_register_profile>()
    );
    assert!(
        core::mem::size_of::<Registered>()
            == core::mem::size_of::<uapi::drm_castkms_renderer_profile_result>()
    );
};

pub(super) fn register(session: &Session, arg: usize) -> Result {
    let request = UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of::<Register>())
        .reader()
        .read::<Register>()?;
    if request.candidate_id == 0
        || request.profile == 0
        || request.result == 0
        || request.flags != 0
        || request.reserved != [0; 2]
        || request.profile_size < 128
    {
        return Err(EINVAL);
    }
    if request.profile_size as usize > capability_description::MAX_BYTES {
        return Err(E2BIG);
    }
    let input = usize::try_from(request.profile).map_err(|_| EFAULT)?;
    let result = usize::try_from(request.result).map_err(|_| EFAULT)?;
    let mut bytes = KVec::new();
    UserSlice::new(UserPtr::from_addr(input), request.profile_size as usize)
        .read_all(&mut bytes, GFP_KERNEL)?;
    let description = match capability_description::decode(&bytes)? {
        Some(profile) => session.propose_profile(request.candidate_id, profile)?,
        None => session.propose_host(request.candidate_id)?,
    };
    let reply = Registered {
        transition: description.transition,
        capability_generation: description.generation,
        execution_generation: description.expected.generation,
        reserved: 0,
    };
    // Registration remains queryable on reply failure; do not invent rollback after visibility.
    UserSlice::new(UserPtr::from_addr(result), core::mem::size_of_val(&reply))
        .writer()
        .write(&reply)
}
