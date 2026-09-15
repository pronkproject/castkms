// SPDX-License-Identifier: GPL-2.0-only

//! Anonymous renderer endpoint retaining access without its revocation owner.

use super::permission::Access;
use crate::{execution::Profile, CastKms};
use core::{ffi::c_void, ptr::NonNull};
use kernel::{
    bindings,
    error::from_err_ptr,
    fs::File,
    module::this_module,
    prelude::*,
    sync::aref::ARef, //
    transmute::AsBytes,
    uaccess::{UserPtr, UserSlice},
    uapi,
};

#[repr(C)]
struct Query {
    version: u32,
    flags: u32,
    profile: u32,
    reserved: u32,
    generation: u64,
}

// SAFETY: Query contains only integers and has no padding.
unsafe impl AsBytes for Query {}

struct ClientFile {
    access: Access,
}

impl ClientFile {
    const OPS: bindings::file_operations = bindings::file_operations {
        owner: this_module::<CastKms>().as_ptr(),
        release: Some(Self::release),
        unlocked_ioctl: Some(Self::ioctl),
        #[cfg(CONFIG_COMPAT)]
        compat_ioctl: bindings::compat_ptr_ioctl,
        ..pin_init::zeroed()
    };

    fn new(access: Access) -> Result<ARef<File>> {
        let holder = KBox::into_raw(KBox::new(Self { access }, GFP_KERNEL)?);
        // SAFETY: The immutable operations table belongs to this module and describes
        // the exact allocation transferred as private data.
        let file = from_err_ptr(unsafe {
            bindings::anon_inode_getfile(
                c"[castkms-renderer]".as_char_ptr(),
                &Self::OPS,
                holder.cast::<c_void>(),
                kernel::fs::file::flags::O_RDWR as i32,
            )
        });
        match file {
            Ok(file) => {
                // SAFETY: Creation transfers one initialized unpublished file reference,
                // with no concurrent file-position operation.
                Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(file.cast())) })
            }
            Err(error) => {
                // SAFETY: Native failure did not consume private data.
                drop(unsafe { KBox::from_raw(holder) });
                Err(error)
            }
        }
    }

    unsafe extern "C" fn release(_: *mut bindings::inode, file: *mut bindings::file) -> i32 {
        // SAFETY: Successful creation transfers one ClientFile allocation and final
        // release returns its private data exactly once.
        drop(unsafe { KBox::from_raw((*file).private_data.cast::<Self>()) });
        0
    }

    unsafe extern "C" fn ioctl(file: *mut bindings::file, cmd: u32, arg: usize) -> isize {
        // SAFETY: Native dispatch retains the file throughout this callback, and successful
        // creation installed an initialized ClientFile as its immutable private data.
        let holder = unsafe { &*(*file).private_data.cast::<Self>() };
        holder
            .dispatch(cmd, arg)
            .map_or_else(|error| error.to_errno() as isize, |_| 0)
    }

    fn dispatch(&self, cmd: u32, arg: usize) -> Result {
        match cmd {
            uapi::DRM_IOCTL_CASTKMS_RENDERER_QUERY => self.query(arg),
            _ => Err(ENOTTY),
        }
    }

    fn query(&self, arg: usize) -> Result {
        const {
            assert!(
                core::mem::size_of::<Query>()
                    == core::mem::size_of::<uapi::drm_castkms_renderer_query>()
            )
        };
        let description = self
            .access
            .with_current(|_| Ok(self.access.device().execution.describe()))?;
        let profile = match description.profile {
            Profile::HostV1 => uapi::DRM_CASTKMS_EXECUTION_HOST_V1,
        };
        let query = Query {
            version: uapi::DRM_CASTKMS_RENDERER_VERSION,
            flags: 0,
            profile,
            reserved: 0,
            generation: description.generation,
        };
        UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of_val(&query))
            .writer()
            .write(&query)
    }
}

pub(super) fn create(access: Access) -> Result<ARef<File>> {
    ClientFile::new(access)
}
