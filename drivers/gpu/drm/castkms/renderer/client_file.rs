// SPDX-License-Identifier: GPL-2.0-only

//! Anonymous renderer transport over a kernel-callable immutable offer endpoint.

use super::{
    endpoint::{Endpoint, Phase},
    job::Completion,
};
use crate::CastKms;
use core::{ffi::c_void, mem::size_of, ptr::NonNull};
use kernel::{
    bindings,
    dma_fence::Fence,
    error::from_err_ptr,
    fs::{File, LocalFile},
    module::this_module,
    prelude::*,
    sync::{aref::ARef, poll::PollTable, Arc},
    transmute::{AsBytes, FromBytes},
    uaccess::{UserPtr, UserSlice},
    uapi,
};

#[repr(C)]
struct Query {
    version: u32,
    state: u32,
    constraints_id: u64,
    reserved: [u64; 2],
}
// SAFETY: Every byte belongs to an initialized integer, with no padding.
unsafe impl AsBytes for Query {}

#[repr(C)]
struct Prepare {
    constraints: u64,
    constraints_size: u32,
    flags: u32,
    width: u32,
    height: u32,
    reserved: [u64; 3],
}
// SAFETY: All fields are integers accepting every bit pattern.
unsafe impl FromBytes for Prepare {}

#[repr(C)]
struct Publish {
    result: u64,
    flags: u32,
    reserved: u32,
    padding: [u64; 2],
}
// SAFETY: All fields are integers accepting every bit pattern.
unsafe impl FromBytes for Publish {}

#[repr(C)]
struct Published {
    constraints_id: u64,
    reserved: [u64; 3],
}
// SAFETY: Every byte belongs to an initialized integer, with no padding.
unsafe impl AsBytes for Published {}

#[repr(C)]
struct Withdraw {
    flags: u32,
    reserved: [u32; 3],
}
// SAFETY: All fields are integers accepting every bit pattern.
unsafe impl FromBytes for Withdraw {}

#[repr(C)]
struct SubmitProbe {
    completion_fd: i32,
    flags: u32,
    reserved: [u64; 3],
}
// SAFETY: All fields are integers accepting every bit pattern.
unsafe impl FromBytes for SubmitProbe {}

#[repr(C)]
struct ReleaseSource {
    job_id: u64,
    completion_fd: i32,
    kind: u32,
    flags: u32,
    reserved: [u32; 3],
}

#[repr(C)]
struct ReleaseOutput {
    job_id: u64,
    completion_fd: i32,
    kind: u32,
    flags: u32,
    reserved: [u32; 3],
}
unsafe impl FromBytes for ReleaseOutput {}
// SAFETY: All fields are integers accepting every bit pattern.
unsafe impl FromBytes for ReleaseSource {}

const _: () = {
    assert!(size_of::<Query>() == size_of::<uapi::drm_castkms_renderer_query>());
    assert!(size_of::<Prepare>() == size_of::<uapi::drm_castkms_renderer_prepare_offer>());
    assert!(size_of::<Publish>() == size_of::<uapi::drm_castkms_renderer_publish_offer>());
    assert!(size_of::<Published>() == size_of::<uapi::drm_castkms_renderer_offer_result>());
    assert!(size_of::<Withdraw>() == size_of::<uapi::drm_castkms_renderer_withdraw_offer>());
    assert!(size_of::<SubmitProbe>() == size_of::<uapi::drm_castkms_renderer_submit_probe>());
    assert!(size_of::<ReleaseSource>() == size_of::<uapi::drm_castkms_renderer_release_source>());
    assert!(size_of::<ReleaseOutput>() == size_of::<uapi::drm_castkms_renderer_release_output>());
};

fn read<T: FromBytes>(arg: usize) -> Result<T> {
    UserSlice::new(UserPtr::from_addr(arg), size_of::<T>())
        .reader()
        .read::<T>()
}

fn fence(fd: i32) -> Result<ARef<Fence>> {
    let file = LocalFile::fget(fd.try_into().map_err(|_| EBADF)?).map_err(|_| EBADF)?;
    Fence::from_sync_file(&file)
}

struct ClientFile {
    endpoint: Arc<Endpoint>,
}

impl ClientFile {
    const OPS: bindings::file_operations = bindings::file_operations {
        owner: this_module::<CastKms>().as_ptr(),
        release: Some(Self::release),
        poll: Some(Self::poll),
        unlocked_ioctl: Some(Self::ioctl),
        #[cfg(CONFIG_COMPAT)]
        compat_ioctl: bindings::compat_ptr_ioctl,
        ..pin_init::zeroed()
    };

    fn new(endpoint: Arc<Endpoint>) -> Result<ARef<File>> {
        let holder = KBox::into_raw(KBox::new(Self { endpoint }, GFP_KERNEL)?);
        // SAFETY: The module-owned operations describe the exact private allocation.
        let file = from_err_ptr(unsafe {
            bindings::anon_inode_getfile(
                c"[castkms-renderer]".as_char_ptr(),
                &Self::OPS,
                holder.cast::<c_void>(),
                kernel::fs::file::flags::O_RDWR as i32,
            )
        });
        match file {
            // SAFETY: Successful creation transfers one initialized unpublished file reference.
            Ok(file) => Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(file.cast())) }),
            Err(error) => {
                // SAFETY: Failure did not consume the private allocation.
                drop(unsafe { KBox::from_raw(holder) });
                Err(error)
            }
        }
    }

    unsafe extern "C" fn release(_: *mut bindings::inode, file: *mut bindings::file) -> i32 {
        // SAFETY: Final VFS release returns the one allocation installed at creation.
        let holder = unsafe { KBox::from_raw((*file).private_data.cast::<Self>()) };
        holder.endpoint.close();
        drop(holder);
        0
    }

    unsafe extern "C" fn poll(
        file: *mut bindings::file,
        table: *mut bindings::poll_table_struct,
    ) -> bindings::__poll_t {
        // SAFETY: VFS retains the file and its private allocation throughout this callback.
        let holder = unsafe { &*(*file).private_data.cast::<Self>() };
        // SAFETY: Both pointers have their VFS callback lifetimes. Register before observing.
        unsafe {
            PollTable::from_raw(table)
                .register_wait(File::from_raw_file(file), holder.endpoint.changed())
        };
        match holder.endpoint.source_readable().and_then(|source| {
            if source { Ok(true) } else { holder.endpoint.output_readable() }
        }) {
            Ok(true) => (bindings::POLLIN | bindings::POLLRDNORM) as _,
            Ok(false) => 0,
            Err(_) => (bindings::POLLHUP | bindings::POLLERR) as _,
        }
    }

    unsafe extern "C" fn ioctl(file: *mut bindings::file, cmd: u32, arg: usize) -> isize {
        // SAFETY: VFS retains the initialized private allocation throughout dispatch.
        let holder = unsafe { &*(*file).private_data.cast::<Self>() };
        holder.dispatch(cmd, arg).map_or_else(
            |error| {
                // libdrm retries EAGAIN internally; readiness is a caller-controlled retry.
                let error = if error == EAGAIN { EBUSY } else { error };
                error.to_errno() as isize
            },
            |_| 0,
        )
    }

    fn dispatch(&self, cmd: u32, arg: usize) -> Result {
        match cmd {
            uapi::DRM_IOCTL_CASTKMS_RENDERER_QUERY => self.query(arg),
            uapi::DRM_IOCTL_CASTKMS_RENDERER_PREPARE_OFFER => self.prepare(arg),
            uapi::DRM_IOCTL_CASTKMS_RENDERER_PUBLISH_OFFER => self.publish(arg),
            uapi::DRM_IOCTL_CASTKMS_RENDERER_WITHDRAW_OFFER => {
                let request = read::<Withdraw>(arg)?;
                if request.flags != 0 || request.reserved != [0; 3] {
                    return Err(EINVAL);
                }
                self.endpoint.withdraw()
            }
            uapi::DRM_IOCTL_CASTKMS_RENDERER_SUBMIT_PROBE => {
                let request = read::<SubmitProbe>(arg)?;
                if request.flags != 0 || request.reserved != [0; 3] {
                    return Err(EINVAL);
                }
                let completion = if request.completion_fd == -1 {
                    None
                } else {
                    Some(fence(request.completion_fd)?)
                };
                self.endpoint.submit_probe(completion)
            }
            uapi::DRM_IOCTL_CASTKMS_RENDERER_REGISTER_IMAGE => {
                super::image_file::register(&self.endpoint, arg)
            }
            uapi::DRM_IOCTL_CASTKMS_RENDERER_UNREGISTER_IMAGE => {
                super::image_file::unregister(&self.endpoint, arg)
            }
            uapi::DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE => {
                super::scene_file::dequeue(&self.endpoint, arg)
            }
            uapi::DRM_IOCTL_CASTKMS_RENDERER_RELEASE_SOURCE => self.release_source(arg),
            uapi::DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_OUTPUT => {
                super::output_file::dequeue(&self.endpoint, arg)
            }
            uapi::DRM_IOCTL_CASTKMS_RENDERER_RELEASE_OUTPUT => self.release_output(arg),
            _ => Err(ENOTTY),
        }
    }

    fn query(&self, arg: usize) -> Result {
        let description = self.endpoint.describe()?;
        let query = Query {
            version: uapi::DRM_CASTKMS_RENDERER_VERSION,
            state: match description.phase {
                Phase::Empty => uapi::DRM_CASTKMS_RENDERER_STATE_EMPTY,
                Phase::Draft => uapi::DRM_CASTKMS_RENDERER_STATE_DRAFT,
                Phase::Publishing => uapi::DRM_CASTKMS_RENDERER_STATE_PUBLISHING,
                Phase::Published => uapi::DRM_CASTKMS_RENDERER_STATE_PUBLISHED,
                Phase::Withdrawn => uapi::DRM_CASTKMS_RENDERER_STATE_WITHDRAWN,
            },
            constraints_id: description.constraints_id,
            reserved: [0; 2],
        };
        UserSlice::new(UserPtr::from_addr(arg), size_of::<Query>())
            .writer()
            .write(&query)
    }

    fn prepare(&self, arg: usize) -> Result {
        let request = read::<Prepare>(arg)?;
        if request.constraints == 0 || request.flags != 0 || request.reserved != [0; 3] {
            return Err(EINVAL);
        }
        if request.constraints_size as usize > super::constraints_description::MAX_BYTES {
            return Err(E2BIG);
        }
        let pointer = usize::try_from(request.constraints).map_err(|_| EOVERFLOW)?;
        let mut bytes = KVec::with_capacity(request.constraints_size as usize, GFP_KERNEL)?;
        bytes.resize(request.constraints_size as usize, 0, GFP_KERNEL)?;
        UserSlice::new(UserPtr::from_addr(pointer), bytes.len())
            .reader()
            .read_slice(&mut bytes)?;
        let profile = super::constraints_description::decode(&bytes)?;
        self.endpoint
            .declare(profile, [request.width, request.height])
    }

    fn publish(&self, arg: usize) -> Result {
        let request = read::<Publish>(arg)?;
        if request.result == 0
            || request.flags != 0
            || request.reserved != 0
            || request.padding != [0; 2]
        {
            return Err(EINVAL);
        }
        let pointer = usize::try_from(request.result).map_err(|_| EOVERFLOW)?;
        self.endpoint.check_probe()?;
        self.endpoint.publish(|constraints_id| {
            let reply = Published {
                constraints_id,
                reserved: [0; 3],
            };
            UserSlice::new(UserPtr::from_addr(pointer), size_of::<Published>())
                .writer()
                .write(&reply)
        })
    }

    fn release_source(&self, arg: usize) -> Result {
        let request = read::<ReleaseSource>(arg)?;
        if request.job_id == 0 || request.flags != 0 || request.reserved != [0; 3] {
            return Err(EINVAL);
        }
        let completion = match request.kind {
            uapi::DRM_CASTKMS_RENDERER_RELEASE_NO_ACCESS if request.completion_fd == -1 => {
                Completion::WithoutAccess
            }
            uapi::DRM_CASTKMS_RENDERER_RELEASE_CPU_DONE if request.completion_fd == -1 => {
                Completion::Cpu
            }
            uapi::DRM_CASTKMS_RENDERER_RELEASE_SUBMITTED if request.completion_fd >= 0 => {
                Completion::Submitted(fence(request.completion_fd)?)
            }
            _ => return Err(EINVAL),
        };
        self.endpoint.release_source(request.job_id, completion)
    }

    fn release_output(&self, arg: usize) -> Result {
        let request = read::<ReleaseOutput>(arg)?;
        if request.job_id == 0 || request.flags != 0 || request.reserved != [0; 3] {
            return Err(EINVAL);
        }
        let completion = match request.kind {
            uapi::DRM_CASTKMS_RENDERER_RELEASE_NO_ACCESS if request.completion_fd == -1 => {
                Completion::WithoutAccess
            }
            uapi::DRM_CASTKMS_RENDERER_RELEASE_CPU_DONE if request.completion_fd == -1 => {
                Completion::Cpu
            }
            uapi::DRM_CASTKMS_RENDERER_RELEASE_SUBMITTED if request.completion_fd >= 0 => {
                Completion::Submitted(fence(request.completion_fd)?)
            }
            _ => return Err(EINVAL),
        };
        self.endpoint.release_output(request.job_id, completion)
    }
}

pub(super) fn create(endpoint: Arc<Endpoint>) -> Result<ARef<File>> {
    ClientFile::new(endpoint)
}
