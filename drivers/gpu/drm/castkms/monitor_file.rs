// SPDX-License-Identifier: GPL-2.0-only

//! Anonymous file transport for exclusive virtual monitor control.

use crate::{monitor::Control, CastKms, Driver, File as DriverFile};
use core::{ffi::c_void, ptr::NonNull};
use kernel::{
    bindings,
    cred::{self, Capability},
    drm::{device::Registered, file::File as DrmFile, kms::connector::Edid, Device},
    error::from_err_ptr,
    fs::{file::FileDescriptorReservation, File},
    module::this_module,
    prelude::*,
    sync::{aref::ARef, poll::PollTable, Arc, Mutex},
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

#[repr(C)]
struct CecSetTransport {
    flags: u32,
    reserved: u32,
}
// SAFETY: Every bit pattern is valid for the integer fields.
unsafe impl FromBytes for CecSetTransport {}

#[repr(C)]
struct CecTransaction {
    cookie: u64,
    state_generation: u64,
    signal_free_time: u32,
    attempts: u8,
    length: u8,
    message: [u8; 16],
    reserved: [u8; 2],
}
// SAFETY: Every byte belongs to an initialized integer or byte array.
unsafe impl AsBytes for CecTransaction {}

#[repr(C)]
struct CecComplete {
    cookie: u64,
    status: u8,
    arbitration_lost: u8,
    nack: u8,
    low_drive: u8,
    error: u8,
    reserved: [u8; 3],
}
// SAFETY: Every bit pattern is valid for the integer and byte fields.
unsafe impl FromBytes for CecComplete {}

#[repr(C)]
struct CecReceive {
    flags: u32,
    length: u8,
    message: [u8; 16],
    reserved: [u8; 11],
}
// SAFETY: Every bit pattern is valid for the integer and byte fields.
unsafe impl FromBytes for CecReceive {}

#[repr(C)]
struct CecState {
    state_generation: u64,
    pending_cookie: u64,
    submitted: u64,
    completed: u64,
    nack: u64,
    error: u64,
    timeout: u64,
    received: u64,
    invalid: u64,
    flags: u32,
    physical_address: u16,
    logical_address_mask: u16,
    reserved: [u64; 2],
}
// SAFETY: Every byte belongs to an initialized integer or array, without padding.
unsafe impl AsBytes for CecState {}

const _: () = {
    assert!(core::mem::size_of::<CecSetTransport>()
        == core::mem::size_of::<uapi::drm_castkms_cec_set_transport>());
    assert!(core::mem::size_of::<CecTransaction>()
        == core::mem::size_of::<uapi::drm_castkms_cec_transaction>());
    assert!(core::mem::size_of::<CecComplete>()
        == core::mem::size_of::<uapi::drm_castkms_cec_complete>());
    assert!(core::mem::size_of::<CecReceive>()
        == core::mem::size_of::<uapi::drm_castkms_cec_receive>());
    assert!(core::mem::size_of::<CecState>()
        == core::mem::size_of::<uapi::drm_castkms_cec_state>());
};

fn read<T: FromBytes>(arg: usize) -> Result<T> {
    UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of::<T>())
        .reader()
        .read::<T>()
}

#[pin_data]
struct Lease {
    #[pin]
    control: Mutex<Option<Control>>,
}

impl Lease {
    fn new() -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                control <- kernel::new_mutex!(None),
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

    fn set_cec_online(&self, online: bool) -> Result {
        self.control
            .lock()
            .as_ref()
            .ok_or(ECANCELED)?
            .cec()
            .set_online(online)
    }

    fn peek_cec(&self) -> Result<crate::cec::Transaction> {
        self.control
            .lock()
            .as_ref()
            .ok_or(ECANCELED)?
            .cec()
            .peek()
    }

    fn acquire_cec(&self, cookie: u64) -> Result {
        self.control
            .lock()
            .as_ref()
            .ok_or(ECANCELED)?
            .cec()
            .acquire(cookie)
    }

    fn complete_cec(
        &self,
        cookie: u64,
        result: kernel::drm::kms::connector::cec::TransmitResult,
    ) -> Result {
        self.control
            .lock()
            .as_ref()
            .ok_or(ECANCELED)?
            .cec()
            .complete(cookie, result)
    }

    fn receive_cec(&self, message: kernel::drm::kms::connector::cec::Message) -> Result {
        self.control
            .lock()
            .as_ref()
            .ok_or(ECANCELED)?
            .cec()
            .receive(message)
    }

    fn cec_snapshot(&self) -> Result<crate::cec::Snapshot> {
        self.control
            .lock()
            .as_ref()
            .ok_or(ECANCELED)?
            .cec()
            .snapshot()
    }
}

struct HolderFile {
    lease: Arc<Lease>,
}

impl HolderFile {
    const OPS: bindings::file_operations = bindings::file_operations {
        owner: this_module::<CastKms>().as_ptr(),
        release: Some(Self::release),
        poll: Some(Self::poll),
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

    unsafe extern "C" fn poll(
        file: *mut bindings::file,
        table: *mut bindings::poll_table_struct,
    ) -> bindings::__poll_t {
        // SAFETY: VFS retains this file and its private allocation throughout poll.
        let owner = unsafe { &*(*file).private_data.cast::<Self>() };
        let control = owner.lease.control.lock();
        let Some(control) = control.as_ref() else {
            return (bindings::POLLHUP | bindings::POLLERR) as _;
        };
        // SAFETY: Both VFS pointers remain valid throughout this callback. The
        // monitor owns the wait queue beyond the control interval.
        unsafe {
            PollTable::from_raw(table)
                .register_wait(File::from_raw_file(file), control.cec().changed())
        };
        match control.cec().readable() {
            Ok(true) => (bindings::POLLIN | bindings::POLLRDNORM) as _,
            Ok(false) => 0,
            Err(_) => (bindings::POLLHUP | bindings::POLLERR) as _,
        }
    }

    fn dispatch(&self, cmd: u32, arg: usize) -> Result {
        match cmd {
            uapi::DRM_IOCTL_CASTKMS_MONITOR_QUERY => self.query(arg),
            uapi::DRM_IOCTL_CASTKMS_MONITOR_ATTACH => self.attach(arg),
            uapi::DRM_IOCTL_CASTKMS_MONITOR_DETACH => self.detach(arg),
            uapi::DRM_IOCTL_CASTKMS_MONITOR_CEC_SET_TRANSPORT => self.cec_set_transport(arg),
            uapi::DRM_IOCTL_CASTKMS_MONITOR_CEC_ACQUIRE_TX => self.cec_acquire(arg),
            uapi::DRM_IOCTL_CASTKMS_MONITOR_CEC_COMPLETE_TX => self.cec_complete(arg),
            uapi::DRM_IOCTL_CASTKMS_MONITOR_CEC_RECEIVE => self.cec_receive(arg),
            uapi::DRM_IOCTL_CASTKMS_MONITOR_CEC_GET_STATE => self.cec_state(arg),
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
            flags: uapi::DRM_CASTKMS_MONITOR_CAP_CEC,
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

    fn cec_set_transport(&self, arg: usize) -> Result {
        let request = read::<CecSetTransport>(arg)?;
        if request.flags & !uapi::DRM_CASTKMS_CEC_TRANSPORT_ONLINE != 0
            || request.reserved != 0
        {
            return Err(EINVAL);
        }
        self.lease
            .set_cec_online(request.flags & uapi::DRM_CASTKMS_CEC_TRANSPORT_ONLINE != 0)
    }

    fn cec_acquire(&self, arg: usize) -> Result {
        let transaction = self.lease.peek_cec()?;
        let bytes = transaction.message.as_bytes();
        let mut message = [0; 16];
        message[..bytes.len()].copy_from_slice(bytes);
        let result = CecTransaction {
            cookie: transaction.cookie,
            state_generation: transaction.state_generation,
            signal_free_time: transaction.signal_free_time,
            attempts: transaction.attempts,
            length: bytes.len() as u8,
            message,
            reserved: [0; 2],
        };
        UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of_val(&result))
            .writer()
            .write(&result)?;
        self.lease.acquire_cec(transaction.cookie)
    }

    fn cec_complete(&self, arg: usize) -> Result {
        let request = read::<CecComplete>(arg)?;
        if request.reserved != [0; 3] {
            return Err(EINVAL);
        }
        self.lease.complete_cec(
            request.cookie,
            kernel::drm::kms::connector::cec::TransmitResult {
                status: request.status,
                arbitration_lost: request.arbitration_lost,
                nack: request.nack,
                low_drive: request.low_drive,
                error: request.error,
            },
        )
    }

    fn cec_receive(&self, arg: usize) -> Result {
        let request = read::<CecReceive>(arg)?;
        if request.flags != 0
            || request.reserved != [0; 11]
            || request.length == 0
            || request.length as usize > request.message.len()
        {
            return Err(EINVAL);
        }
        self.lease.receive_cec(
            kernel::drm::kms::connector::cec::Message::new(
                &request.message[..request.length as usize],
            )?,
        )
    }

    fn cec_state(&self, arg: usize) -> Result {
        let snapshot = self.lease.cec_snapshot()?;
        let state = CecState {
            state_generation: snapshot.generation,
            pending_cookie: snapshot.pending_cookie,
            submitted: snapshot.submitted,
            completed: snapshot.completed,
            nack: snapshot.nack,
            error: snapshot.error,
            timeout: snapshot.timeout,
            received: snapshot.received,
            invalid: snapshot.invalid,
            flags: snapshot.flags,
            physical_address: snapshot.physical_address,
            logical_address_mask: snapshot.logical_address_mask,
            reserved: [0; 2],
        };
        UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of_val(&state))
            .writer()
            .write(&state)
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
    let administrative = request.flags & uapi::DRM_CASTKMS_MONITOR_CREATE_ADMIN != 0;
    if request.flags & !uapi::DRM_CASTKMS_MONITOR_CREATE_ADMIN != 0
        || request.reserved != [0; 2]
        || request.files == 0
    {
        return Err(EINVAL);
    }
    if administrative && !cred::capable_in_initial_user_namespace(Capability::SysAdmin) {
        return Err(EACCES);
    }
    let result = usize::try_from(request.files).map_err(|_| EFAULT)?;
    let connector = if administrative {
        dev.lookup_connector_unfiltered(request.connector_id)?
    } else {
        dev.lookup_connector(file, request.connector_id)?
    };
    let control_reservation =
        FileDescriptorReservation::get_unused_fd_flags(kernel::fs::file::flags::O_CLOEXEC)?;
    let revoke_reservation =
        FileDescriptorReservation::get_unused_fd_flags(kernel::fs::file::flags::O_CLOEXEC)?;
    let snapshot = if administrative {
        None
    } else {
        Some(file.master_snapshot().ok_or(EACCES)?)
    };
    let check_authority = || -> Result {
        if let Some(snapshot) = &snapshot {
            let guard = snapshot.master().lock_current().ok_or(EACCES)?;
            if !guard.is_master_file(file) || !guard.holds_object(&*connector) {
                return Err(EACCES);
            }
        }
        Ok(())
    };
    check_authority()?;
    let pending = connector.monitor.reserve(dev)?;
    let lease = Lease::new()?;
    check_authority()?;
    let revoke_file = RevokerFile::new(lease.clone())?;
    let control_file = HolderFile::new(lease.clone())?;
    let control_fd = control_reservation
        .reserved_fd()
        .try_into()
        .map_err(|_| EOVERFLOW)?;
    let revoke_fd = revoke_reservation
        .reserved_fd()
        .try_into()
        .map_err(|_| EOVERFLOW)?;
    #[repr(C)]
    struct Files {
        control_fd: i32,
        revoke_fd: i32,
    }
    // SAFETY: Two initialized integers without padding.
    unsafe impl AsBytes for Files {}
    const {
        assert!(
            core::mem::size_of::<Files>()
                == core::mem::size_of::<uapi::drm_castkms_monitor_files>()
        );
    }
    let files = Files {
        control_fd,
        revoke_fd,
    };
    UserSlice::new(UserPtr::from_addr(result), core::mem::size_of_val(&files))
        .writer()
        .write(&files)?;
    let current = if let Some(snapshot) = &snapshot {
        let guard = snapshot.master().lock_current().ok_or(EACCES)?;
        if !guard.is_master_file(file) || !guard.holds_object(&*connector) {
            return Err(EACCES);
        }
        Some(guard)
    } else {
        None
    };
    let control = pending.publish_unnotified()?;
    drop(current);
    *lease.control.lock() = Some(control);
    control_reservation.fd_install(control_file);
    revoke_reservation.fd_install(revoke_file);
    dev.changed.notify_all();
    dev.hotplug_event();
    Ok(0)
}
