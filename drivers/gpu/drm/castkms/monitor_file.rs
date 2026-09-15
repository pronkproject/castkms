// SPDX-License-Identifier: GPL-2.0-only

//! Anonymous file transport for exclusive virtual monitor control.

use crate::{monitor::Control, CastKms, Driver, File as DriverFile};
use core::{ffi::c_void, ptr::NonNull};
use kernel::{
    bindings,
    drm::{device::Registered, file::File as DrmFile, kms::connector::Edid, Device},
    error::from_err_ptr,
    fs::{file::FileDescriptorReservation, File},
    module::this_module,
    prelude::*,
    sync::{aref::ARef, Arc, Mutex},
    transmute::{AsBytes, FromBytes},
    uaccess::{UserPtr, UserSlice},
    uapi,
};

#[repr(C)]
struct Query {
    version: u32,
    flags: u32,
    max_edid_size: u32,
    reserved: u32,
}

// SAFETY: Query contains only integers and has no padding.
unsafe impl AsBytes for Query {}

#[repr(C)]
struct Attach {
    flags: u32,
    edid_size: u32,
    edid_ptr: u64,
}

// SAFETY: Every bit pattern is valid for Attach's integer fields.
unsafe impl FromBytes for Attach {}

#[repr(C)]
struct Detach {
    flags: u32,
    reserved: u32,
}

// SAFETY: Every bit pattern is valid for Detach's integer fields.
unsafe impl FromBytes for Detach {}

#[pin_data]
struct Lease {
    #[pin]
    control: Mutex<Option<Control>>,
}

impl Lease {
    fn new(control: Control) -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                control <- kernel::new_mutex!(Some(control)),
            }),
            GFP_KERNEL,
        )
    }

    fn attach(&self, edid: Option<Edid>) -> Result {
        self.control.lock().as_ref().ok_or(ECANCELED)?.attach(edid)
    }

    fn detach(&self) -> Result {
        self.control.lock().as_ref().ok_or(ECANCELED)?.detach()
    }

    fn revoke(&self) {
        let control = self.control.lock().take();
        drop(control);
    }
}

struct HolderFile {
    lease: Arc<Lease>,
}

impl HolderFile {
    const OPS: bindings::file_operations = bindings::file_operations {
        owner: this_module::<CastKms>().as_ptr(),
        release: Some(Self::release),
        unlocked_ioctl: Some(Self::ioctl),
        #[cfg(CONFIG_COMPAT)]
        compat_ioctl: bindings::compat_ptr_ioctl,
        ..pin_init::zeroed()
    };

    fn new(lease: Arc<Lease>) -> Result<ARef<File>> {
        let owner = KBox::into_raw(KBox::new(Self { lease }, GFP_KERNEL)?);
        // SAFETY: The immutable operations table belongs to this module and
        // describes the exact allocation transferred as private data.
        let file = from_err_ptr(unsafe {
            bindings::anon_inode_getfile(
                c"[castkms-monitor]".as_char_ptr(),
                &Self::OPS,
                owner.cast::<c_void>(),
                kernel::fs::file::flags::O_RDWR as i32,
            )
        });
        match file {
            Ok(file) => {
                // SAFETY: Creation transfers one initialized unpublished file
                // reference, with no concurrent file-position operation.
                Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(file.cast())) })
            }
            Err(error) => {
                // SAFETY: Native failure did not consume private data.
                drop(unsafe { KBox::from_raw(owner) });
                Err(error)
            }
        }
    }

    unsafe extern "C" fn release(_: *mut bindings::inode, file: *mut bindings::file) -> i32 {
        // SAFETY: Successful creation transfers one HolderFile allocation and
        // final release returns its private data exactly once.
        let owner = unsafe { KBox::from_raw((*file).private_data.cast::<Self>()) };
        owner.lease.revoke();
        drop(owner);
        0
    }

    unsafe extern "C" fn ioctl(file: *mut bindings::file, cmd: u32, arg: usize) -> isize {
        // SAFETY: Native dispatch retains the file throughout this callback,
        // and successful creation installed an initialized HolderFile.
        let owner = unsafe { &*(*file).private_data.cast::<Self>() };
        owner
            .dispatch(cmd, arg)
            .map_or_else(|error| error.to_errno() as isize, |_| 0)
    }

    fn dispatch(&self, cmd: u32, arg: usize) -> Result {
        match cmd {
            uapi::DRM_IOCTL_CASTKMS_MONITOR_QUERY => self.query(arg),
            uapi::DRM_IOCTL_CASTKMS_MONITOR_ATTACH => self.attach(arg),
            uapi::DRM_IOCTL_CASTKMS_MONITOR_DETACH => self.detach(arg),
            _ => Err(ENOTTY),
        }
    }

    fn query(&self, arg: usize) -> Result {
        const {
            assert!(
                core::mem::size_of::<Query>()
                    == core::mem::size_of::<uapi::drm_castkms_monitor_query>()
            )
        };
        let query = Query {
            version: uapi::DRM_CASTKMS_MONITOR_CONTROL_VERSION,
            flags: 0,
            max_edid_size: uapi::DRM_CASTKMS_MONITOR_MAX_EDID_SIZE,
            reserved: 0,
        };
        UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of_val(&query))
            .writer()
            .write(&query)
    }

    fn attach(&self, arg: usize) -> Result {
        const {
            assert!(
                core::mem::size_of::<Attach>()
                    == core::mem::size_of::<uapi::drm_castkms_monitor_attach>()
            )
        };
        let mut reader =
            UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of::<Attach>()).reader();
        let request = reader.read::<Attach>()?;
        if request.flags != 0
            || request.edid_size > uapi::DRM_CASTKMS_MONITOR_MAX_EDID_SIZE
            || (request.edid_size == 0) != (request.edid_ptr == 0)
        {
            return Err(EINVAL);
        }
        let edid = if request.edid_size == 0 {
            None
        } else {
            let address = request.edid_ptr.try_into().map_err(|_| EOVERFLOW)?;
            let mut bytes = KVec::new();
            UserSlice::new(UserPtr::from_addr(address), request.edid_size as usize)
                .read_all(&mut bytes, GFP_KERNEL)?;
            Some(Edid::new(&bytes)?)
        };
        self.lease.attach(edid)
    }

    fn detach(&self, arg: usize) -> Result {
        const {
            assert!(
                core::mem::size_of::<Detach>()
                    == core::mem::size_of::<uapi::drm_castkms_monitor_detach>()
            )
        };
        let mut reader =
            UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of::<Detach>()).reader();
        let request = reader.read::<Detach>()?;
        if request.flags != 0 || request.reserved != 0 {
            return Err(EINVAL);
        }
        self.lease.detach()
    }
}

struct RevokerFile {
    lease: Arc<Lease>,
}

impl RevokerFile {
    const OPS: bindings::file_operations = bindings::file_operations {
        owner: this_module::<CastKms>().as_ptr(),
        release: Some(Self::release),
        ..pin_init::zeroed()
    };

    fn new(lease: Arc<Lease>) -> Result<ARef<File>> {
        let owner = KBox::into_raw(KBox::new(Self { lease }, GFP_KERNEL)?);
        // SAFETY: The immutable operations table belongs to this module and
        // describes the exact allocation transferred as private data.
        let file = from_err_ptr(unsafe {
            bindings::anon_inode_getfile(
                c"[castkms-monitor-revoke]".as_char_ptr(),
                &Self::OPS,
                owner.cast::<c_void>(),
                kernel::fs::file::flags::O_RDWR as i32,
            )
        });
        match file {
            Ok(file) => {
                // SAFETY: Creation transfers one initialized unpublished file
                // reference, with no concurrent file-position operation.
                Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(file.cast())) })
            }
            Err(error) => {
                // SAFETY: Native failure did not consume private data.
                drop(unsafe { KBox::from_raw(owner) });
                Err(error)
            }
        }
    }

    unsafe extern "C" fn release(_: *mut bindings::inode, file: *mut bindings::file) -> i32 {
        // SAFETY: Successful creation transfers one RevokerFile allocation and
        // final release returns its private data exactly once.
        let owner = unsafe { KBox::from_raw((*file).private_data.cast::<Self>()) };
        owner.lease.revoke();
        drop(owner);
        0
    }
}

pub(crate) fn create(
    dev: &Device<Driver, Registered>,
    _: &(),
    request: &mut uapi::drm_castkms_create_monitor_control,
    file: &DrmFile<DriverFile>,
) -> Result<u32> {
    if request.flags != 0 || request.reserved != 0 {
        return Err(EINVAL);
    }
    let connector = dev.lookup_connector(file, request.connector_id)?;
    let control_reservation =
        FileDescriptorReservation::get_unused_fd_flags(kernel::fs::file::flags::O_CLOEXEC)?;
    let revoke_reservation =
        FileDescriptorReservation::get_unused_fd_flags(kernel::fs::file::flags::O_CLOEXEC)?;
    let snapshot = file.master_snapshot().ok_or(EACCES)?;
    {
        let guard = snapshot.master().lock_current().ok_or(EACCES)?;
        if !guard.is_master_file(file) || !guard.holds_object(&*connector) {
            return Err(EACCES);
        }
    }
    let lease = Lease::new(dev.monitor.acquire(dev)?)?;
    {
        let guard = snapshot.master().lock_current().ok_or(EACCES)?;
        if !guard.is_master_file(file) || !guard.holds_object(&*connector) {
            return Err(EACCES);
        }
    }
    let revoke_file = RevokerFile::new(lease.clone())?;
    let control_file = HolderFile::new(lease)?;
    request.control_fd = control_reservation
        .reserved_fd()
        .try_into()
        .map_err(|_| EOVERFLOW)?;
    request.revoke_fd = revoke_reservation
        .reserved_fd()
        .try_into()
        .map_err(|_| EOVERFLOW)?;
    control_reservation.fd_install(control_file);
    revoke_reservation.fd_install(revoke_file);
    Ok(0)
}
