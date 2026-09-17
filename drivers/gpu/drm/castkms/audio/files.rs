// SPDX-License-Identifier: GPL-2.0-only

//! Thin anonymous-file transport for the kernel audio provider.

use super::{
    provider::{
        Access,
        Owner, //
    },
    BUFFER_FRAMES,
    FRAME_BYTES,
    RATE, //
};
use crate::{
    CastKms,
    Driver,
    File as DriverFile, //
};
use core::ptr::NonNull;
use kernel::{
    bindings,
    drm::{
        capture::ControlOwner,
        device::Registered,
        file::File as DrmFile,
        Device, //
    },
    error::from_err_ptr,
    fs::{
        file::{
            flags,
            FileDescriptorReservation, //
        },
        File, //
    },
    module::this_module,
    prelude::*,
    sync::{
        aref::ARef,
        poll::PollTable,
        Mutex, //
    },
    transmute::AsBytes,
    uaccess::{
        UserPtr,
        UserSlice, //
    },
    uapi, //
};

#[repr(C)]
struct Query {
    version: u32,
    format: u32,
    rate: u32,
    channels: u32,
    frame_bytes: u32,
    reserved: u32,
    buffer_frames: u64,
    dropped_frames: u64,
}
// SAFETY: All fields are integers with no implicit padding.
unsafe impl AsBytes for Query {}

#[repr(C)]
struct Descriptors {
    audio_fd: i32,
    revoke_fd: i32,
}
// SAFETY: All fields are integers with no implicit padding.
unsafe impl AsBytes for Descriptors {}

struct Client {
    access: Access,
    read: Pin<KBox<Mutex<KVec<u8>>>>,
}

impl Client {
    const OPS: bindings::file_operations = bindings::file_operations {
        owner: this_module::<CastKms>().as_ptr(),
        release: Some(Self::release),
        read: Some(Self::read),
        poll: Some(Self::poll),
        unlocked_ioctl: Some(Self::ioctl),
        #[cfg(CONFIG_COMPAT)]
        compat_ioctl: bindings::compat_ptr_ioctl,
        ..pin_init::zeroed()
    };

    fn new(access: Access, nonblock: bool) -> Result<ARef<File>> {
        let mut scratch = KVec::with_capacity(65_536, GFP_KERNEL)?;
        scratch.resize(65_536, 0, GFP_KERNEL)?;
        let read = KBox::pin_init(kernel::new_mutex!(scratch), GFP_KERNEL)?;
        let owner = KBox::into_raw(KBox::new(Self { access, read }, GFP_KERNEL)?);
        // SAFETY: The operation table retains the driver and describes exactly the
        // private allocation transferred on success. Native failure does not consume it.
        let result = from_err_ptr(unsafe {
            bindings::anon_inode_getfile(
                c"[castkms-audio]".as_char_ptr(),
                &Self::OPS,
                owner.cast(),
                (flags::O_RDONLY | if nonblock { flags::O_NONBLOCK } else { 0 }) as _,
            )
        });
        match result {
            Ok(file) => {
                // SAFETY: Native construction transfers one initialized unpublished file reference.
                Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(file.cast())) })
            }
            Err(error) => {
                // SAFETY: Native failure did not consume the private allocation.
                drop(unsafe { KBox::from_raw(owner) });
                Err(error)
            }
        }
    }

    unsafe extern "C" fn release(_: *mut bindings::inode, file: *mut bindings::file) -> c_int {
        // SAFETY: Final release transfers the allocation supplied to anon_inode_getfile once.
        drop(unsafe { KBox::from_raw((*file).private_data.cast::<Self>()) });
        0
    }

    unsafe extern "C" fn read(
        file: *mut bindings::file,
        data: *mut c_char,
        count: usize,
        _: *mut bindings::loff_t,
    ) -> isize {
        // SAFETY: The VFS retains the private allocation for the callback.
        let client = unsafe { &*(*file).private_data.cast::<Self>() };
        let result = (|| {
            if count == 0 {
                return Ok(0);
            }
            if count < FRAME_BYTES {
                return Err(EINVAL);
            }
            let size = count.min(65_536) / FRAME_BYTES * FRAME_BYTES;
            let mut scratch = client.read.lock();
            // SAFETY: VFS keeps this file live; flags can change with fcntl, hence a single read.
            let nonblock =
                unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*file).f_flags)) }
                    & flags::O_NONBLOCK
                    != 0;
            let read = client.access.read(&mut scratch[..size], nonblock)?;
            UserSlice::new(UserPtr::from_addr(data as usize), read)
                .writer()
                .write_slice(&scratch[..read])?;
            Ok(read as isize)
        })();
        result.unwrap_or_else(|error: Error| error.to_errno() as isize)
    }

    unsafe extern "C" fn poll(
        file: *mut bindings::file,
        table: *mut bindings::poll_table_struct,
    ) -> bindings::__poll_t {
        // SAFETY: VFS retains the file and its private allocation throughout poll.
        let client = unsafe { &*(*file).private_data.cast::<Self>() };
        if let Some(tap) = client.access.active_tap() {
            // SAFETY: Both pointers have their VFS poll callback lifetimes. The retained
            // tap keeps its wait queue alive through registration and readiness recheck.
            unsafe {
                PollTable::from_raw(table)
                    .register_wait(File::from_raw_file(file), &tap.changed)
            };
        }
        // SAFETY: Both pointers have their VFS poll callback lifetimes.
        unsafe {
            PollTable::from_raw(table)
                .register_wait(File::from_raw_file(file), client.access.authority_changed())
        };
        match client.access.readable() {
            Err(EAGAIN) => 0,
            Err(_) => (bindings::POLLHUP | bindings::POLLERR) as _,
            Ok(true) => (bindings::POLLIN | bindings::POLLRDNORM) as _,
            Ok(false) => 0,
        }
    }

    unsafe extern "C" fn ioctl(file: *mut bindings::file, cmd: c_uint, arg: c_ulong) -> c_long {
        // SAFETY: VFS retains our private allocation for the callback.
        let client = unsafe { &*(*file).private_data.cast::<Self>() };
        let result = (|| {
            if cmd != uapi::DRM_IOCTL_CASTKMS_AUDIO_QUERY {
                return Err(ENOTTY);
            }
            client.access.check()?;
            let query = Query {
                version: uapi::DRM_CASTKMS_AUDIO_VERSION,
                format: uapi::DRM_CASTKMS_AUDIO_S16_LE,
                rate: RATE as _,
                channels: 2,
                frame_bytes: FRAME_BYTES as _,
                reserved: 0,
                buffer_frames: BUFFER_FRAMES as _,
                dropped_frames: client.access.dropped_frames()?,
            };
            UserSlice::new(
                UserPtr::from_addr(arg as usize),
                core::mem::size_of_val(&query),
            )
            .writer()
            .write(&query)?;
            Ok(0)
        })();
        result.unwrap_or_else(|error: Error| error.to_errno() as c_long)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.access.close();
    }
}

/// Convert kernel ownership into files without installing descriptors.
pub(crate) fn into_files(owner: Owner, nonblock: bool) -> Result<(ARef<File>, ARef<File>)> {
    let client = Client::new(owner.access(), nonblock)?;
    let revoker = owner.authority().create_control_file_with_owner(owner)?;
    Ok((client, revoker))
}

// SAFETY: The vtable retains the driver throughout destruction of the provider owner.
#[vtable]
unsafe impl ControlOwner for Owner {}

pub(crate) fn create(
    dev: &Device<Driver, Registered>,
    _: &(),
    request: &mut uapi::drm_castkms_create_audio_capture,
    file: &DrmFile<DriverFile>,
) -> Result<u32> {
    const {
        assert!(
            core::mem::size_of::<Query>() == core::mem::size_of::<uapi::drm_castkms_audio_query>()
        );
        assert!(
            core::mem::size_of::<Descriptors>()
                == core::mem::size_of::<uapi::drm_castkms_audio_files>()
        );
    }
    if request.crtc_id == 0
        || request.connector_id == 0
        || request.flags & !uapi::DRM_CASTKMS_AUDIO_NONBLOCK != 0
        || request.reserved.iter().any(|value| *value != 0)
    {
        return Err(EINVAL);
    }
    let audio_fd = FileDescriptorReservation::get_unused_fd_flags(flags::O_CLOEXEC)?;
    let revoke_fd = FileDescriptorReservation::get_unused_fd_flags(flags::O_CLOEXEC)?;
    let owner = DriverFile::create_audio_owner(dev, file, request.crtc_id, request.connector_id)?;
    let (audio, revoke) = into_files(owner, request.flags & uapi::DRM_CASTKMS_AUDIO_NONBLOCK != 0)?;
    let descriptors = Descriptors {
        audio_fd: audio_fd.reserved_fd().try_into().map_err(|_| EOVERFLOW)?,
        revoke_fd: revoke_fd.reserved_fd().try_into().map_err(|_| EOVERFLOW)?,
    };
    let address = request.files.try_into().map_err(|_| EOVERFLOW)?;
    UserSlice::new(
        UserPtr::from_addr(address),
        core::mem::size_of_val(&descriptors),
    )
    .writer()
    .write(&descriptors)?;
    audio_fd.fd_install(audio);
    revoke_fd.fd_install(revoke);
    Ok(0)
}
