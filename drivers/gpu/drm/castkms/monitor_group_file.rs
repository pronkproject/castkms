// SPDX-License-Identifier: GPL-2.0-only

//! Anonymous lifetime capability for one attached tiled monitor.

use crate::{authority::grants, display, monitor, CastKms, Driver, File as DriverFile};
use core::{
    ffi::c_void,
    ptr::NonNull,
    sync::atomic::{AtomicU64, Ordering},
};
use kernel::{
    alloc::kvec::KVec,
    bindings,
    cred::{self, Capability},
    drm::{device::Registered, file::File as DrmFile, kms::connector::Edid, Device},
    error::from_err_ptr,
    fs::{file::FileDescriptorReservation, File, LocalFile},
    module::this_module,
    prelude::*,
    sync::{aref::ARef, Arc, Mutex},
    transmute::{AsBytes, FromBytes},
    uaccess::{UserPtr, UserSlice},
    uapi,
};

#[repr(C)]
struct Member {
    connector_id: u32,
    edid_size: u32,
    edid_ptr: u64,
}

// SAFETY: Every bit pattern is valid for the integer fields.
unsafe impl FromBytes for Member {}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Mapping {
    group_id: u64,
    connector_id: u32,
    horizontal_location: u16,
    vertical_location: u16,
    reserved: u64,
}

// SAFETY: Mapping contains only initialized integers without padding.
unsafe impl AsBytes for Mapping {}

#[repr(C)]
struct Query {
    version: u32,
    flags: u32,
    member_count: u32,
    reserved: u32,
    horizontal_tiles: u32,
    vertical_tiles: u32,
    tile_width: u32,
    tile_height: u32,
    topology_id: [u8; 9],
    reserved2: [u8; 7],
    group_id: u64,
    mappings: u64,
    mapping_capacity: u32,
    reserved3: u32,
}

#[repr(C)]
struct CaptureMember {
    group_id: u64,
    crtc_id: u32,
    connector_id: u32,
    horizontal_location: u16,
    vertical_location: u16,
    reserved: u32,
    capture_fd: i32,
    control_fd: i32,
    reserved2: u64,
}

// SAFETY: Query contains only initialized integers and byte arrays without padding.
unsafe impl AsBytes for Query {}
// SAFETY: Every bit pattern is valid for the integer and byte-array fields.
unsafe impl FromBytes for Query {}
// SAFETY: CaptureMember contains only initialized integers without padding.
unsafe impl AsBytes for CaptureMember {}

const _: () = {
    assert!(
        core::mem::size_of::<Member>()
            == core::mem::size_of::<uapi::drm_castkms_monitor_group_member>()
    );
    assert!(
        core::mem::size_of::<Mapping>()
            == core::mem::size_of::<uapi::drm_castkms_monitor_group_mapping>()
    );
    assert!(
        core::mem::size_of::<Query>()
            == core::mem::size_of::<uapi::drm_castkms_monitor_group_query>()
    );
    assert!(
        core::mem::size_of::<CaptureMember>()
            == core::mem::size_of::<uapi::drm_castkms_monitor_group_capture_member>()
    );
};

struct Candidate {
    connector_id: u32,
    output_index: usize,
    input_index: usize,
    edid: Option<Edid>,
}

#[pin_data]
struct Lease {
    group_id: u64,
    topology: monitor::group::Topology,
    mappings: [Mapping; uapi::DRM_CASTKMS_MONITOR_GROUP_MAX_MEMBERS as usize],
    crtc_ids: [u32; uapi::DRM_CASTKMS_MONITOR_GROUP_MAX_MEMBERS as usize],
    device_groups: Arc<monitor::group::Registry>,
    grants: Arc<grants::Registry>,
    #[pin]
    control: Mutex<Option<Managed>>,
}

struct Managed {
    _control: monitor::group::Control,
    _claim: monitor::group::Claim,
}

impl Lease {
    fn new(
        group_id: u64,
        topology: monitor::group::Topology,
        mappings: [Mapping; uapi::DRM_CASTKMS_MONITOR_GROUP_MAX_MEMBERS as usize],
        crtc_ids: [u32; uapi::DRM_CASTKMS_MONITOR_GROUP_MAX_MEMBERS as usize],
        device_groups: Arc<monitor::group::Registry>,
    ) -> Result<Arc<Self>> {
        let grants = grants::Registry::new_monitor_group()?;
        Arc::pin_init(
            pin_init!(Self {
                group_id,
                topology,
                mappings,
                crtc_ids,
                device_groups,
                grants,
                control <- kernel::new_mutex!(None),
            }),
            GFP_KERNEL,
        )
    }

    fn revoke(&self) {
        let control = self.control.lock().take();
        drop(control);
        self.grants.close();
    }
}

struct HolderFile {
    lease: Arc<Lease>,
}

struct HolderOperations(bindings::file_operations);

// SAFETY: The operations table is immutable after static initialization.
unsafe impl Sync for HolderOperations {}

static HOLDER_OPS: HolderOperations = HolderOperations(bindings::file_operations {
    owner: this_module::<CastKms>().as_ptr(),
    release: Some(HolderFile::release),
    unlocked_ioctl: Some(HolderFile::ioctl),
    #[cfg(CONFIG_COMPAT)]
    compat_ioctl: bindings::compat_ptr_ioctl,
    ..pin_init::zeroed()
});

impl HolderFile {
    fn new(lease: Arc<Lease>) -> Result<ARef<File>> {
        let owner = KBox::into_raw(KBox::new(Self { lease }, GFP_KERNEL)?);
        // SAFETY: The immutable operations table describes the exact private allocation.
        let file = from_err_ptr(unsafe {
            bindings::anon_inode_getfile(
                c"[castkms-monitor-group]".as_char_ptr(),
                &HOLDER_OPS.0,
                owner.cast::<c_void>(),
                kernel::fs::file::flags::O_RDWR as i32,
            )
        });
        match file {
            Ok(file) => {
                // SAFETY: Creation transfers one unpublished initialized file reference.
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
        // SAFETY: Successful creation transfers one allocation and final release returns it.
        let owner = unsafe { KBox::from_raw((*file).private_data.cast::<Self>()) };
        owner.lease.revoke();
        drop(owner);
        0
    }

    unsafe extern "C" fn ioctl(file: *mut bindings::file, cmd: u32, arg: usize) -> isize {
        // SAFETY: VFS retains this file and its initialized private allocation.
        let owner = unsafe { &*(*file).private_data.cast::<Self>() };
        owner
            .dispatch(cmd, arg)
            .map_or_else(|error| error.to_errno() as isize, |_| 0)
    }

    fn dispatch(&self, cmd: u32, arg: usize) -> Result {
        match cmd {
            uapi::DRM_IOCTL_CASTKMS_MONITOR_GROUP_QUERY => self.query(arg),
            _ => Err(ENOTTY),
        }
    }

    fn query(&self, arg: usize) -> Result {
        let address = UserPtr::from_addr(arg);
        let request = UserSlice::new(address, core::mem::size_of::<Query>())
            .reader()
            .read::<Query>()?;
        if request.version != uapi::DRM_CASTKMS_MONITOR_GROUP_VERSION
            || request.flags != 0
            || request.reserved != 0
            || request.reserved2 != [0; 7]
            || request.reserved3 != 0
            || (request.mappings == 0) != (request.mapping_capacity == 0)
        {
            return Err(EINVAL);
        }
        if self.lease.control.lock().is_none() {
            return Err(ECANCELED);
        }
        let dimensions = self.lease.topology.dimensions();
        let tile_size = self.lease.topology.tile_size();
        let query = Query {
            version: uapi::DRM_CASTKMS_MONITOR_GROUP_VERSION,
            flags: 0,
            member_count: self
                .lease
                .topology
                .member_count()
                .try_into()
                .map_err(|_| EOVERFLOW)?,
            reserved: 0,
            horizontal_tiles: dimensions[0].into(),
            vertical_tiles: dimensions[1].into(),
            tile_width: tile_size[0].into(),
            tile_height: tile_size[1].into(),
            topology_id: *self.lease.topology.identity(),
            reserved2: [0; 7],
            group_id: self.lease.group_id,
            mappings: request.mappings,
            mapping_capacity: request.mapping_capacity,
            reserved3: 0,
        };
        if request.mappings != 0 {
            if request.mapping_capacity < query.member_count {
                return Err(ENOSPC);
            }
            let mappings_address = usize::try_from(request.mappings).map_err(|_| EFAULT)?;
            let mappings_bytes = (query.member_count as usize)
                .checked_mul(core::mem::size_of::<Mapping>())
                .ok_or(EOVERFLOW)?;
            let mut writer =
                UserSlice::new(UserPtr::from_addr(mappings_address), mappings_bytes).writer();
            for mapping in &self.lease.mappings[..query.member_count as usize] {
                writer.write(mapping)?;
            }
        }
        UserSlice::new(address, core::mem::size_of_val(&query))
            .writer()
            .write(&query)
    }

    fn lease_from_fd(dev: &Device<Driver, Registered>, fd: i32) -> Result<Arc<Lease>> {
        let file = LocalFile::fget(fd.try_into().map_err(|_| EBADF)?).map_err(|_| EBADF)?;
        // SAFETY: fget retains the file and its immutable operations table through inspection.
        if !core::ptr::eq(unsafe { (*file.as_ptr()).f_op }, &HOLDER_OPS.0) {
            return Err(EBADF);
        }
        // SAFETY: HOLDER_OPS uniquely identifies the initialized HolderFile private allocation.
        let owner = unsafe { &*(*file.as_ptr()).private_data.cast::<Self>() };
        if !Arc::ptr_eq(&owner.lease.device_groups, &dev.monitor_groups) {
            return Err(EXDEV);
        }
        if owner.lease.control.lock().is_none() {
            return Err(ECANCELED);
        }
        Ok(owner.lease.clone())
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
        // SAFETY: The immutable operations table describes the exact private allocation.
        let file = from_err_ptr(unsafe {
            bindings::anon_inode_getfile(
                c"[castkms-monitor-group-revoke]".as_char_ptr(),
                &Self::OPS,
                owner.cast::<c_void>(),
                kernel::fs::file::flags::O_RDWR as i32,
            )
        });
        match file {
            Ok(file) => {
                // SAFETY: Creation transfers one unpublished initialized file reference.
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
        // SAFETY: Successful creation transfers one allocation and final release returns it.
        let owner = unsafe { KBox::from_raw((*file).private_data.cast::<Self>()) };
        owner.lease.revoke();
        drop(owner);
        0
    }
}

fn output_index(
    dev: &Device<Driver, Registered>,
    connector: &kernel::drm::kms::connector::Connector<display::Connector>,
) -> Result<usize> {
    dev.displays
        .iter()
        .position(|display| Arc::ptr_eq(&display.monitor, &connector.monitor))
        .ok_or(EINVAL)
}

static NEXT_GROUP_ID: AtomicU64 = AtomicU64::new(1);

fn next_group_id() -> Result<u64> {
    NEXT_GROUP_ID
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .map_err(|_| EOVERFLOW)
}

pub(crate) fn create(
    dev: &Device<Driver, Registered>,
    _: &(),
    request: &mut uapi::drm_castkms_create_monitor_group,
    file: &DrmFile<DriverFile>,
) -> Result<u32> {
    let administrative = request.flags & uapi::DRM_CASTKMS_MONITOR_CREATE_ADMIN != 0;
    let member_count = request.member_count as usize;
    if request.version != uapi::DRM_CASTKMS_MONITOR_GROUP_VERSION
        || request.flags & !uapi::DRM_CASTKMS_MONITOR_CREATE_ADMIN != 0
        || member_count < 2
        || member_count > uapi::DRM_CASTKMS_MONITOR_GROUP_MAX_MEMBERS as usize
        || request.reserved != 0
        || request.reserved2 != [0; 2]
        || request.members == 0
        || request.mappings == 0
        || request.files == 0
    {
        return Err(EINVAL);
    }
    if administrative && !cred::capable_in_initial_user_namespace(Capability::SysAdmin) {
        return Err(EACCES);
    }

    let members_address = usize::try_from(request.members).map_err(|_| EFAULT)?;
    let mappings_address = usize::try_from(request.mappings).map_err(|_| EFAULT)?;
    let files_address = usize::try_from(request.files).map_err(|_| EFAULT)?;
    let members_bytes = member_count
        .checked_mul(core::mem::size_of::<Member>())
        .ok_or(EOVERFLOW)?;
    let mut reader = UserSlice::new(UserPtr::from_addr(members_address), members_bytes).reader();
    let mut candidates = KVec::with_capacity(member_count, GFP_KERNEL)?;
    let mut authority_connectors = KVec::with_capacity(member_count, GFP_KERNEL)?;
    for input_index in 0..member_count {
        let member = reader.read::<Member>()?;
        if member.edid_size == 0
            || member.edid_size > uapi::DRM_CASTKMS_MONITOR_MAX_EDID_SIZE
            || member.edid_ptr == 0
        {
            return Err(EINVAL);
        }
        let connector = if administrative {
            dev.lookup_connector_unfiltered(member.connector_id)?
        } else {
            dev.lookup_connector(file, member.connector_id)?
        };
        let edid_address = usize::try_from(member.edid_ptr).map_err(|_| EFAULT)?;
        let mut bytes = KVec::new();
        UserSlice::new(UserPtr::from_addr(edid_address), member.edid_size as usize)
            .read_all(&mut bytes, GFP_KERNEL)?;
        let output_index = output_index(dev, &connector)?;
        authority_connectors.push(connector, GFP_KERNEL)?;
        candidates.push(
            Candidate {
                connector_id: member.connector_id,
                output_index,
                input_index,
                edid: Some(Edid::new(&bytes)?),
            },
            GFP_KERNEL,
        )?;
    }
    candidates.sort_unstable_by_key(|candidate| candidate.output_index);
    if candidates
        .windows(2)
        .any(|pair| pair[0].output_index == pair[1].output_index)
    {
        return Err(EINVAL);
    }

    let snapshot = if administrative {
        None
    } else {
        Some(file.master_snapshot().ok_or(EACCES)?)
    };
    let check_authority = || -> Result {
        let Some(snapshot) = &snapshot else {
            return Ok(());
        };
        let guard = snapshot.master().lock_current().ok_or(EACCES)?;
        if !guard.is_master_file(file) {
            return Err(EACCES);
        }
        for connector in &authority_connectors {
            if !guard.holds_object(&**connector) {
                return Err(EACCES);
            }
        }
        Ok(())
    };
    check_authority()?;

    let mut output_indices = KVec::with_capacity(member_count, GFP_KERNEL)?;
    let mut edids = KVec::with_capacity(member_count, GFP_KERNEL)?;
    for candidate in &candidates {
        output_indices.push(candidate.output_index, GFP_KERNEL)?;
    }
    for candidate in candidates.iter_mut() {
        edids.push(candidate.edid.take().ok_or(EINVAL)?, GFP_KERNEL)?;
    }

    let topology = monitor::group::Topology::from_edids(&edids)?;
    let claim = dev.monitor_groups.claim(*topology.identity())?;
    let pending = monitor::Monitor::reserve_group(dev, &output_indices)?;
    let group_id = next_group_id()?;
    let mut mappings = [Mapping::default(); uapi::DRM_CASTKMS_MONITOR_GROUP_MAX_MEMBERS as usize];
    let mut crtc_ids = [0; uapi::DRM_CASTKMS_MONITOR_GROUP_MAX_MEMBERS as usize];
    for (sorted_index, candidate) in candidates.iter().enumerate() {
        let location = topology.member_location(sorted_index).ok_or(EINVAL)?;
        mappings[candidate.input_index] = Mapping {
            group_id,
            connector_id: candidate.connector_id,
            horizontal_location: location[0].into(),
            vertical_location: location[1].into(),
            reserved: 0,
        };
        crtc_ids[candidate.input_index] = dev.displays[candidate.output_index]
            .crtc_id
            .copy()
            .ok_or(ENODEV)?;
    }
    let lease = Lease::new(
        group_id,
        topology,
        mappings,
        crtc_ids,
        dev.monitor_groups.clone(),
    )?;
    let control_reservation =
        FileDescriptorReservation::get_unused_fd_flags(kernel::fs::file::flags::O_CLOEXEC)?;
    let revoke_reservation =
        FileDescriptorReservation::get_unused_fd_flags(kernel::fs::file::flags::O_CLOEXEC)?;
    let control_file = HolderFile::new(lease.clone())?;
    let revoke_file = RevokerFile::new(lease.clone())?;

    let mappings_bytes = member_count
        .checked_mul(core::mem::size_of::<Mapping>())
        .ok_or(EOVERFLOW)?;
    let mut writer = UserSlice::new(UserPtr::from_addr(mappings_address), mappings_bytes).writer();
    for mapping in &lease.mappings[..member_count] {
        writer.write(mapping)?;
    }

    #[repr(C)]
    struct Files {
        control_fd: i32,
        revoke_fd: i32,
    }
    // SAFETY: Files contains two initialized integers without padding.
    unsafe impl AsBytes for Files {}
    const {
        assert!(
            core::mem::size_of::<Files>()
                == core::mem::size_of::<uapi::drm_castkms_monitor_files>()
        );
    }
    let files = Files {
        control_fd: control_reservation
            .reserved_fd()
            .try_into()
            .map_err(|_| EOVERFLOW)?,
        revoke_fd: revoke_reservation
            .reserved_fd()
            .try_into()
            .map_err(|_| EOVERFLOW)?,
    };
    UserSlice::new(
        UserPtr::from_addr(files_address),
        core::mem::size_of_val(&files),
    )
    .writer()
    .write(&files)?;

    check_authority()?;
    let (control, _) = pending.attach(edids)?;
    *lease.control.lock() = Some(Managed {
        _control: control,
        _claim: claim,
    });
    control_reservation.fd_install(control_file);
    revoke_reservation.fd_install(revoke_file);
    Ok(0)
}

struct CapturePublication {
    capture_reservation: FileDescriptorReservation,
    control_reservation: FileDescriptorReservation,
    capture: ARef<File>,
    control: ARef<File>,
}

pub(crate) fn create_capture(
    dev: &Device<Driver, Registered>,
    _: &(),
    request: &mut uapi::drm_castkms_create_monitor_group_capture,
    file: &DrmFile<DriverFile>,
) -> Result<u32> {
    let administrative = request.flags & uapi::DRM_CASTKMS_MONITOR_GROUP_CAPTURE_ADMIN != 0;
    if request.version != uapi::DRM_CASTKMS_MONITOR_GROUP_VERSION
        || request.flags & !uapi::DRM_CASTKMS_MONITOR_GROUP_CAPTURE_ADMIN != 0
        || request.group_fd < 0
        || request.members == 0
        || request.reserved != [0; 3]
    {
        return Err(EINVAL);
    }
    if administrative && !cred::capable_in_initial_user_namespace(Capability::SysAdmin) {
        return Err(EACCES);
    }

    let lease = HolderFile::lease_from_fd(dev, request.group_fd)?;
    let member_count = lease.topology.member_count();
    if request.member_capacity < member_count.try_into().map_err(|_| EOVERFLOW)? {
        return Err(ENOSPC);
    }
    let mut targets = KVec::with_capacity(member_count, GFP_KERNEL)?;
    for index in 0..member_count {
        targets.push(
            (lease.crtc_ids[index], lease.mappings[index].connector_id),
            GFP_KERNEL,
        )?;
    }
    let pairs =
        DriverFile::create_group_capture_files(dev, file, &targets, administrative, &lease.grants)?;

    let mut publications = KVec::with_capacity(member_count, GFP_KERNEL)?;
    let members_address = usize::try_from(request.members).map_err(|_| EFAULT)?;
    let members_bytes = member_count
        .checked_mul(core::mem::size_of::<CaptureMember>())
        .ok_or(EOVERFLOW)?;
    let mut writer = UserSlice::new(UserPtr::from_addr(members_address), members_bytes).writer();
    for (index, pair) in pairs.into_iter().enumerate() {
        let capture_reservation =
            FileDescriptorReservation::get_unused_fd_flags(kernel::fs::file::flags::O_CLOEXEC)?;
        let control_reservation =
            FileDescriptorReservation::get_unused_fd_flags(kernel::fs::file::flags::O_CLOEXEC)?;
        let member = &lease.mappings[index];
        let output = CaptureMember {
            group_id: lease.group_id,
            crtc_id: lease.crtc_ids[index],
            connector_id: member.connector_id,
            horizontal_location: member.horizontal_location,
            vertical_location: member.vertical_location,
            reserved: 0,
            capture_fd: capture_reservation
                .reserved_fd()
                .try_into()
                .map_err(|_| EOVERFLOW)?,
            control_fd: control_reservation
                .reserved_fd()
                .try_into()
                .map_err(|_| EOVERFLOW)?,
            reserved2: 0,
        };
        writer.write(&output)?;
        let (capture, control) = pair.into_files();
        publications.push(
            CapturePublication {
                capture_reservation,
                control_reservation,
                capture,
                control,
            },
            GFP_KERNEL,
        )?;
    }

    for publication in publications {
        publication
            .capture_reservation
            .fd_install(publication.capture);
        publication
            .control_reservation
            .fd_install(publication.control);
    }
    Ok(0)
}
