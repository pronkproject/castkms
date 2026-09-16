// SPDX-License-Identifier: GPL-2.0-only

//! Anonymous renderer endpoint retaining access without its revocation owner.

use super::session::Session;
use super::job::Completion;
use crate::{execution::Profile, CastKms};
use core::{ffi::c_void, ptr::NonNull};
use kernel::{
    bindings,
    dma_fence::Fence,
    error::from_err_ptr,
    drm::fourcc,
    fs::{
        file::FileDescriptorReservation,
        File,
        LocalFile, //
    },
    module::this_module,
    prelude::*,
    sync::{aref::ARef, Arc}, //
    transmute::{AsBytes, FromBytes},
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

#[repr(C)]
struct Begin {
    expected_generation: u64,
    result: u64,
    flags: u32,
    reserved: [u32; 3],
}

// SAFETY: Every bit pattern is valid for Begin's integer fields.
unsafe impl FromBytes for Begin {}

#[repr(C)]
struct BeginResult {
    candidate_id: u64,
    execution_generation: u64,
    profile: u32,
    width: u32,
    height: u32,
    refresh_millihz: u32,
    mode_flags: u32,
    reserved: u32,
}

// SAFETY: BeginResult contains only integers and has no padding.
unsafe impl AsBytes for BeginResult {}

#[repr(C)]
struct Abort {
    candidate_id: u64,
    flags: u32,
    reserved: u32,
}

// SAFETY: Every bit pattern is valid for Abort's integer fields.
unsafe impl FromBytes for Abort {}

#[repr(C)]
struct GetSnapshot {
    candidate_id: u64,
    result: u64,
    flags: u32,
    reserved: [u32; 3],
}

// SAFETY: Every bit pattern is valid for GetSnapshot's integer fields.
unsafe impl FromBytes for GetSnapshot {}

#[repr(C)]
struct SnapshotResult {
    dma_buf_fd: i32,
    format: u32,
    modifier: u64,
    width: u32,
    height: u32,
    pitch: u32,
    offset: u32,
    content_serial: u64,
    flags: u32,
    reserved: u32,
}

// SAFETY: SnapshotResult contains only integers and has no padding.
unsafe impl AsBytes for SnapshotResult {}

#[repr(C)]
struct SubmitProbe {
    candidate_id: u64,
    completion_fd: i32,
    source: u32,
    flags: u32,
    reserved: [u32; 3],
}

// SAFETY: Every bit pattern is valid for SubmitProbe's integer fields.
unsafe impl FromBytes for SubmitProbe {}

#[repr(C)]
struct CommitTakeover {
    candidate_id: u64,
    flags: u32,
    reserved: u32,
}

// SAFETY: Every bit pattern is valid for CommitTakeover's integer fields.
unsafe impl FromBytes for CommitTakeover {}

#[repr(C)]
struct ReleaseSource {
    job_id: u64,
    completion_fd: i32,
    kind: u32,
    flags: u32,
    reserved: [u32; 3],
}

// SAFETY: Every bit pattern is valid for ReleaseSource's integer fields.
unsafe impl FromBytes for ReleaseSource {}

struct ClientFile {
    session: Arc<Session>,
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

    fn new(session: Arc<Session>) -> Result<ARef<File>> {
        let holder = KBox::into_raw(KBox::new(Self { session }, GFP_KERNEL)?);
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
        let holder = unsafe { KBox::from_raw((*file).private_data.cast::<Self>()) };
        holder.session.close();
        drop(holder);
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
            uapi::DRM_IOCTL_CASTKMS_RENDERER_REGISTER_PROFILE => super::profile_file::register(&self.session, arg),
            uapi::DRM_IOCTL_CASTKMS_RENDERER_QUERY_CAPABILITIES => super::capability_file::query(&self.session, arg),
            uapi::DRM_IOCTL_CASTKMS_RENDERER_QUERY => self.query(arg),
            uapi::DRM_IOCTL_CASTKMS_RENDERER_BEGIN_TAKEOVER => self.begin(arg),
            uapi::DRM_IOCTL_CASTKMS_RENDERER_ABORT_TAKEOVER => self.abort(arg),
            uapi::DRM_IOCTL_CASTKMS_RENDERER_GET_SNAPSHOT => self.get_snapshot(arg),
            uapi::DRM_IOCTL_CASTKMS_RENDERER_SUBMIT_PROBE => self.submit_probe(arg),
            uapi::DRM_IOCTL_CASTKMS_RENDERER_COMMIT_TAKEOVER => self.commit_takeover(arg),
            uapi::DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE => {
                super::scene_file::dequeue(&self.session, arg)
            }
            uapi::DRM_IOCTL_CASTKMS_RENDERER_RELEASE_SOURCE => self.release_source(arg),
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
        let description = self.session.description()?;
        let query = Query {
            version: uapi::DRM_CASTKMS_RENDERER_VERSION,
            flags: 0,
            profile: profile_value(description.profile),
            reserved: 0,
            generation: description.generation,
        };
        UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of_val(&query))
            .writer()
            .write(&query)
    }

    fn begin(&self, arg: usize) -> Result {
        const {
            assert!(
                core::mem::size_of::<Begin>()
                    == core::mem::size_of::<uapi::drm_castkms_renderer_begin_takeover>()
            );
            assert!(
                core::mem::size_of::<BeginResult>()
                    == core::mem::size_of::<uapi::drm_castkms_renderer_takeover>()
            )
        };
        let mut reader =
            UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of::<Begin>()).reader();
        let request = reader.read::<Begin>()?;
        if request.expected_generation == 0
            || request.result == 0
            || request.flags != 0
            || request.reserved.iter().any(|field| *field != 0)
        {
            return Err(EINVAL);
        }
        let pending = self.session.begin(request.expected_generation)?;
        let configuration = pending.configuration()?;
        let execution = pending.execution()?;
        let result = BeginResult {
            candidate_id: pending.id(),
            execution_generation: execution.generation,
            profile: profile_value(execution.profile),
            width: configuration.dimensions()[0],
            height: configuration.dimensions()[1],
            refresh_millihz: configuration.refresh_millihz(),
            mode_flags: configuration.mode_flags(),
            reserved: 0,
        };
        let address = request.result.try_into().map_err(|_| EOVERFLOW)?;
        UserSlice::new(UserPtr::from_addr(address), core::mem::size_of_val(&result))
            .writer()
            .write(&result)?;
        pending.publish()
    }

    fn abort(&self, arg: usize) -> Result {
        const {
            assert!(
                core::mem::size_of::<Abort>()
                    == core::mem::size_of::<uapi::drm_castkms_renderer_abort_takeover>()
            )
        };
        let mut reader =
            UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of::<Abort>()).reader();
        let request = reader.read::<Abort>()?;
        if request.candidate_id == 0 || request.flags != 0 || request.reserved != 0 {
            return Err(EINVAL);
        }
        self.session.abort(request.candidate_id)
    }

    fn get_snapshot(&self, arg: usize) -> Result {
        const {
            assert!(
                core::mem::size_of::<GetSnapshot>()
                    == core::mem::size_of::<uapi::drm_castkms_renderer_get_snapshot>()
            );
            assert!(
                core::mem::size_of::<SnapshotResult>()
                    == core::mem::size_of::<uapi::drm_castkms_renderer_snapshot>()
            )
        };
        let mut reader =
            UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of::<GetSnapshot>()).reader();
        let request = reader.read::<GetSnapshot>()?;
        if request.candidate_id == 0
            || request.result == 0
            || request.flags != 0
            || request.reserved.iter().any(|field| *field != 0)
        {
            return Err(EINVAL);
        }
        let candidate = self.session.candidate(request.candidate_id)?;
        let snapshot = candidate.snapshot_current()?;
        let file = snapshot.export_file()?;
        let descriptor = FileDescriptorReservation::get_unused_fd_flags(
            kernel::fs::file::flags::O_CLOEXEC,
        )?;
        let layout = snapshot.layout();
        let (width, height) = layout.dimensions();
        let result = SnapshotResult {
            dma_buf_fd: descriptor
                .reserved_fd()
                .try_into()
                .map_err(|_| EOVERFLOW)?,
            format: fourcc::XRGB8888,
            modifier: fourcc::FORMAT_MOD_LINEAR,
            width,
            height,
            pitch: layout.pitch().try_into().map_err(|_| EOVERFLOW)?,
            offset: 0,
            content_serial: snapshot.content_serial_value(),
            flags: 0,
            reserved: 0,
        };
        let address = request.result.try_into().map_err(|_| EOVERFLOW)?;
        UserSlice::new(UserPtr::from_addr(address), core::mem::size_of_val(&result))
            .writer()
            .write(&result)?;
        candidate.publish_snapshot(&snapshot, || descriptor.fd_install(file))?;
        Ok(())
    }

    fn submit_probe(&self, arg: usize) -> Result {
        const {
            assert!(
                core::mem::size_of::<SubmitProbe>()
                    == core::mem::size_of::<uapi::drm_castkms_renderer_submit_probe>()
            )
        };
        let mut reader =
            UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of::<SubmitProbe>()).reader();
        let request = reader.read::<SubmitProbe>()?;
        if request.candidate_id == 0
            || request.completion_fd < -1
            || request.flags != 0
            || request.reserved.iter().any(|field| *field != 0)
        {
            return Err(EINVAL);
        }
        let startup_image = match request.source {
            uapi::DRM_CASTKMS_RENDERER_PROBE_PRIVATE => false,
            uapi::DRM_CASTKMS_RENDERER_PROBE_STARTUP_IMAGE => true,
            _ => return Err(EINVAL),
        };
        let candidate = self.session.candidate(request.candidate_id)?;
        let completion = if request.completion_fd == -1 {
            None
        } else {
            let file = LocalFile::fget(request.completion_fd.try_into().map_err(|_| EBADF)?)
                .map_err(|_| EBADF)?;
            Some(Fence::from_sync_file(&file)?)
        };
        if startup_image {
            candidate.submit_snapshot_probe(completion)
        } else {
            candidate.submit_private_probe(completion)
        }
    }

    fn commit_takeover(&self, arg: usize) -> Result {
        const {
            assert!(
                core::mem::size_of::<CommitTakeover>()
                    == core::mem::size_of::<uapi::drm_castkms_renderer_commit_takeover>()
            )
        };
        let mut reader = UserSlice::new(
            UserPtr::from_addr(arg),
            core::mem::size_of::<CommitTakeover>(),
        )
        .reader();
        let request = reader.read::<CommitTakeover>()?;
        if request.candidate_id == 0 || request.flags != 0 || request.reserved != 0 {
            return Err(EINVAL);
        }
        self.session.activate(request.candidate_id).map(|_| ())
    }

    fn release_source(&self, arg: usize) -> Result {
        const {
            assert!(core::mem::size_of::<ReleaseSource>()
                == core::mem::size_of::<uapi::drm_castkms_renderer_release_source>())
        };
        let mut reader =
            UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of::<ReleaseSource>()).reader();
        let request = reader.read::<ReleaseSource>()?;
        if request.job_id == 0
            || request.flags != 0
            || request.reserved.iter().any(|field| *field != 0)
        {
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
                let file = LocalFile::fget(request.completion_fd.try_into().map_err(|_| EBADF)?)
                    .map_err(|_| EBADF)?;
                Completion::Submitted(Fence::from_sync_file(&file)?)
            }
            _ => return Err(EINVAL),
        };
        self.session.release_source(request.job_id, completion)
    }
}

fn profile_value(profile: Profile) -> u32 {
    match profile {
        Profile::HostV1 => uapi::DRM_CASTKMS_EXECUTION_HOST_V1,
        Profile::GpuV1 => uapi::DRM_CASTKMS_EXECUTION_GPU_V1,
    }
}

pub(super) fn create(session: Arc<Session>) -> Result<ARef<File>> {
    ClientFile::new(session)
}
