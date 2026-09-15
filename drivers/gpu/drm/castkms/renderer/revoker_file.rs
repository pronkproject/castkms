// SPDX-License-Identifier: GPL-2.0-only

//! Anonymous revocation endpoint retaining the renderer permission owner.

use super::{permission::Owner, session::Session};
use crate::CastKms;
use core::{ffi::c_void, ptr::NonNull};
use kernel::{
    bindings,
    error::from_err_ptr,
    fs::File,
    module::this_module,
    prelude::*,
    sync::{aref::ARef, Arc}, //
};

struct RevokerFile {
    owner: Owner,
    session: Arc<Session>,
}

impl RevokerFile {
    const OPS: bindings::file_operations = bindings::file_operations {
        owner: this_module::<CastKms>().as_ptr(),
        release: Some(Self::release),
        ..pin_init::zeroed()
    };

    fn new(owner: Owner, session: Arc<Session>) -> Result<ARef<File>> {
        let holder = KBox::into_raw(KBox::new(Self { owner, session }, GFP_KERNEL)?);
        // SAFETY: The immutable operations table belongs to this module and describes
        // the exact allocation transferred as private data.
        let file = from_err_ptr(unsafe {
            bindings::anon_inode_getfile(
                c"[castkms-renderer-revoke]".as_char_ptr(),
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
        // SAFETY: Successful creation transfers one RevokerFile allocation and final
        // release returns its private data exactly once.
        let holder = unsafe { KBox::from_raw((*file).private_data.cast::<Self>()) };
        holder.owner.revoke();
        holder.session.close();
        drop(holder);
        0
    }
}

pub(super) fn create(owner: Owner, session: Arc<Session>) -> Result<ARef<File>> {
    RevokerFile::new(owner, session)
}
