// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Runtime consumers of the shared KMS interfaces for disposable test kernels.
//!
//! Most cases keep their devices unregistered. Registration cases publish temporary virtual
//! DRM devices, with no physical hardware or capture inputs.

#[cfg(CONFIG_FAILSLAB)]
mod allocation;
mod events;
mod inspection;
mod properties;

use super::*;
use crate::{
    drm::{self, fourcc, gem, UnregisteredDevice},
    faux,
    sync::Arc,
};
use core::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

#[derive(Default)]
struct Counts {
    gem_objects: AtomicU32,
    gem_creations: AtomicU32,
    fail_gem_open: AtomicU32,
    fail_prime_import: AtomicU32,
    objects: AtomicU32,
    setup_failures: AtomicU32,
    plane_updates: AtomicU32,
    enables: AtomicU32,
    disables: AtomicU32,
    crtc_states: AtomicU32,
    fail_crtc_state_alloc: AtomicU32,
    plane_states: AtomicU32,
    fail_plane_state_alloc: AtomicU32,
    connector_states: AtomicU32,
    fail_connector_state_alloc: AtomicU32,
    #[cfg(CONFIG_FAILSLAB)]
    fail_connector_heap_alloc: AtomicU32,
    #[cfg(CONFIG_FAILSLAB)]
    connector_heap_failures: AtomicU32,
}

// No device reference: keeping a mode object alive must not create a device ownership cycle.
struct Lifetime(Arc<Counts>);

impl Lifetime {
    fn new(counts: &Arc<Counts>) -> Self {
        counts.objects.fetch_add(1, Ordering::Relaxed);
        Self(counts.clone())
    }
}

impl Drop for Lifetime {
    fn drop(&mut self) {
        self.0.objects.fetch_sub(1, Ordering::Relaxed);
    }
}

#[pin_data]
struct Data {
    counts: Arc<Counts>,
    fail_after_plane: bool,
    // Non-owning observations, filled before setup returns. Tests borrow the owning DRM device
    // before using them and exclude object creation and teardown. Atomic state access on other
    // tasks goes through transactions, which acquire the native per-object locks.
    plane: AtomicPtr<bindings::drm_plane>,
    crtc: AtomicPtr<bindings::drm_crtc>,
    connector: AtomicPtr<bindings::drm_connector>,
}

struct TestDriver;
struct TestFile;
#[pin_data(PinnedDrop)]
struct TestObject {
    #[cfg_attr(not(CONFIG_DRM_CLIENT), expect(dead_code))]
    allocated_size: usize,
    counts: Arc<Counts>,
}

#[pinned_drop]
impl PinnedDrop for TestObject {
    fn drop(self: Pin<&mut Self>) {
        self.counts.gem_objects.fetch_sub(1, Ordering::Relaxed);
    }
}
#[pin_data]
struct TestPlane {
    life: Lifetime,
}
#[pin_data]
struct TestCrtc {
    life: Lifetime,
}
#[pin_data]
struct TestEncoder {
    life: Lifetime,
}
#[pin_data]
struct TestConnector {
    life: Lifetime,
}

struct PlanePayload {
    counts: Arc<Counts>,
    value: KBox<u64>,
}
struct CrtcPayload {
    counts: Arc<Counts>,
    value: KBox<u64>,
}
struct ConnectorPayload {
    counts: Arc<Counts>,
    value: KBox<u64>,
}

impl plane::DriverPlaneState for PlanePayload {
    type Plane = TestPlane;

    fn new(plane: &plane::Plane<Self::Plane>) -> Result<Self> {
        Self::allocate(&plane.life.0, 0)
    }

    fn duplicate(&self) -> Result<Self> {
        Self::allocate(&self.counts, *self.value)
    }
}

impl PlanePayload {
    fn allocate(counts: &Arc<Counts>, value: u64) -> Result<Self> {
        // Reject at the payload boundary without injecting a slab allocation failure.
        if counts.fail_plane_state_alloc.load(Ordering::Relaxed) != 0 {
            return Err(ENOMEM);
        }
        let value = KBox::new(value, GFP_KERNEL)?;
        counts.plane_states.fetch_add(1, Ordering::Relaxed);
        Ok(Self {
            counts: counts.clone(),
            value,
        })
    }
}

impl Drop for PlanePayload {
    fn drop(&mut self) {
        self.counts.plane_states.fetch_sub(1, Ordering::Relaxed);
    }
}

impl crtc::DriverCrtcState for CrtcPayload {
    type Crtc = TestCrtc;

    fn new(crtc: &crtc::Crtc<Self::Crtc>) -> Result<Self> {
        Self::allocate(&crtc.life.0, 0)
    }

    fn duplicate(&self) -> Result<Self> {
        Self::allocate(&self.counts, *self.value)
    }
}

impl CrtcPayload {
    fn allocate(counts: &Arc<Counts>, value: u64) -> Result<Self> {
        // Deterministic failure at the driver's payload boundary, not slab fault injection.
        if counts.fail_crtc_state_alloc.load(Ordering::Relaxed) != 0 {
            return Err(ENOMEM);
        }
        let value = KBox::new(value, GFP_KERNEL)?;
        counts.crtc_states.fetch_add(1, Ordering::Relaxed);
        Ok(Self {
            counts: counts.clone(),
            value,
        })
    }
}

impl Drop for CrtcPayload {
    fn drop(&mut self) {
        self.counts.crtc_states.fetch_sub(1, Ordering::Relaxed);
    }
}
impl connector::DriverConnectorState for ConnectorPayload {
    type Connector = TestConnector;

    fn new(connector: &connector::Connector<Self::Connector>) -> Result<Self> {
        Self::allocate(&connector.life.0, 0)
    }

    fn duplicate(&self) -> Result<Self> {
        Self::allocate(&self.counts, *self.value)
    }
}

impl ConnectorPayload {
    fn allocate(counts: &Arc<Counts>, value: u64) -> Result<Self> {
        // Reject at the payload boundary without injecting a slab allocation failure.
        if counts.fail_connector_state_alloc.load(Ordering::Relaxed) != 0 {
            return Err(ENOMEM);
        }
        #[cfg(CONFIG_FAILSLAB)]
        let value = if counts.fail_connector_heap_alloc.load(Ordering::Relaxed) != 0 {
            let (result, consumed) = allocation::fail_value_allocation(value)?;
            counts
                .connector_heap_failures
                .fetch_add(u32::from(consumed), Ordering::Relaxed);
            result?
        } else {
            KBox::new(value, GFP_KERNEL)?
        };
        #[cfg(not(CONFIG_FAILSLAB))]
        let value = KBox::new(value, GFP_KERNEL)?;
        counts.connector_states.fetch_add(1, Ordering::Relaxed);
        Ok(Self {
            counts: counts.clone(),
            value,
        })
    }
}

impl Drop for ConnectorPayload {
    fn drop(&mut self) {
        self.counts.connector_states.fetch_sub(1, Ordering::Relaxed);
    }
}

impl drm::file::DriverFile for TestFile {
    type Driver = TestDriver;

    fn open(_: &Device<TestDriver>) -> Result<Pin<KBox<Self>>> {
        Ok(KBox::new(Self, GFP_KERNEL)?.into())
    }
}

#[vtable]
impl gem::DriverObject for TestObject {
    type Driver = TestDriver;
    type Args = ();

    fn new(dev: &Device<TestDriver>, size: usize, _: ()) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            allocated_size: size,
            counts: {
                dev.counts.gem_objects.fetch_add(1, Ordering::Relaxed);
                dev.counts.gem_creations.fetch_add(1, Ordering::Relaxed);
                dev.counts.clone()
            },
        })
    }

    fn dumb_create_args(_: &Device<TestDriver>, _: usize) -> Result<()> {
        Ok(())
    }

    fn prime_import_args(dev: &Device<TestDriver>, _: usize) -> Result<()> {
        if dev.counts.fail_prime_import.load(Ordering::Relaxed) != 0 {
            Err(EACCES)
        } else {
            Ok(())
        }
    }

    fn open(obj: &gem::DriverAllocImpl<Self>, _: &gem::DriverFile<Self>) -> Result {
        if obj.counts.fail_gem_open.load(Ordering::Relaxed) != 0 {
            Err(EACCES)
        } else {
            Ok(())
        }
    }
}

#[vtable]
impl drm::Driver for TestDriver {
    type Data = Data;
    type RegistrationData<'a> = ();
    type File = TestFile;
    type Object = gem::shmem::Object<TestObject>;
    type ParentDevice<Ctx: device::DeviceContext> = faux::Device<Ctx>;
    type Kms = Self;

    const INFO: drm::DriverInfo = drm::DriverInfo {
        major: 0,
        minor: 0,
        patchlevel: 0,
        name: c"rust_kms_test",
        desc: c"Rust KMS runtime tests",
    };
    const IOCTLS: &'static [drm::ioctl::DrmIoctlDescriptor] = &[];
}

#[vtable]
impl plane::DriverPlane for TestPlane {
    type Args = ();
    type Driver = TestDriver;
    type State = PlanePayload;

    fn new(dev: &Device<TestDriver>, _: ()) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            life: Lifetime::new(&dev.counts)
        })
    }

    fn atomic_check(check: plane::PlaneAtomicCheck<'_, Self>) -> Result {
        use plane::RawPlaneState;
        let (state, mut new) = check.take_state_new_state();
        if let Some(crtc) = new.crtc() {
            let crtc_state = state.add_crtc_state(crtc)?;
            new.atomic_helper_check(&crtc_state, false, false)?;
        }
        Ok(())
    }

    fn atomic_update(commit: plane::PlaneAtomicCommit<'_, Self>) {
        commit
            .plane()
            .life
            .0
            .plane_updates
            .fetch_add(1, Ordering::Relaxed);
    }
}

#[vtable]
impl crtc::DriverCrtc for TestCrtc {
    type Args = ();
    type Driver = TestDriver;
    type State = CrtcPayload;
    type VblankImpl = PhantomData<Self>;

    fn new(dev: &Device<TestDriver>, _: &()) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            life: Lifetime::new(&dev.counts)
        })
    }

    fn atomic_enable(commit: crtc::CrtcAtomicCommit<'_, Self>) {
        commit.crtc().life.0.enables.fetch_add(1, Ordering::Relaxed);
    }

    fn atomic_disable(commit: crtc::CrtcAtomicCommit<'_, Self>) {
        commit
            .crtc()
            .life
            .0
            .disables
            .fetch_add(1, Ordering::Relaxed);
    }
}

#[vtable]
impl encoder::DriverEncoder for TestEncoder {
    type Args = ();
    type Driver = TestDriver;

    fn new(dev: &Device<TestDriver>, _: ()) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            life: Lifetime::new(&dev.counts)
        })
    }
}

#[vtable]
impl connector::DriverConnector for TestConnector {
    type Args = ();
    type Driver = TestDriver;
    type State = ConnectorPayload;

    fn new(dev: &Device<TestDriver>, _: ()) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            life: Lifetime::new(&dev.counts)
        })
    }

    fn get_modes<'a>(
        connector: connector::ConnectorGuard<'a, Self>,
        _: &ModeConfigGuard<'a, TestDriver>,
    ) -> i32 {
        connector.add_modes_noedid((640, 480))
    }
}

#[vtable]
impl KmsDriver for TestDriver {
    type Connector = TestConnector;
    type Plane = TestPlane;
    type Crtc = TestCrtc;
    type Encoder = TestEncoder;

    fn mode_config_info(_: &device::Device, _: &Data) -> Result<ModeConfigInfo> {
        Ok(ModeConfigInfo {
            min_resolution: (1, 1),
            max_resolution: (640, 480),
            max_cursor: (64, 64),
            preferred_depth: 24,
            preferred_fourcc: Some(fourcc::XRGB8888),
            enable_default_client: false,
        })
    }

    fn create_objects(dev: &UnregisteredKmsDevice<'_, Self>) -> Result {
        use connector::AsRawConnector;
        use crtc::{AsRawCrtc, RawCrtc};
        use plane::AsRawPlane;

        let plane = plane::UnregisteredPlane::<TestPlane>::new(
            dev,
            0,
            &[fourcc::XRGB8888],
            Some(&[fourcc::FORMAT_MOD_LINEAR]),
            plane::Type::Primary,
            None,
            (),
        )?;
        dev.plane.store(plane.as_raw(), Ordering::Relaxed);
        if dev.fail_after_plane {
            dev.counts.setup_failures.fetch_add(1, Ordering::Relaxed);
            return Err(EINVAL);
        }
        let crtc = crtc::UnregisteredCrtc::<TestCrtc>::new(
            dev,
            plane,
            None::<&plane::UnregisteredPlane<TestPlane>>,
            None,
            (),
        )?;
        dev.crtc.store(crtc.as_raw(), Ordering::Relaxed);
        let encoder = encoder::UnregisteredEncoder::<TestEncoder>::new(
            dev,
            encoder::Type::Virtual,
            crtc.mask(),
            0,
            None,
            (),
        )?;
        let connector = connector::UnregisteredConnector::<TestConnector>::new(
            dev,
            connector::Type::Virtual,
            (),
        )?;
        connector.attach_encoder(encoder)?;
        dev.connector.store(connector.as_raw(), Ordering::Relaxed);
        Ok(())
    }

    fn atomic_commit_tail<'a>(
        mut tail: atomic::AtomicCommitTail<'a, Self>,
        modesets: atomic::ModesetsReadyToken<'a, Self>,
        planes: atomic::PlaneUpdatesReadyToken<'a, Self>,
    ) -> atomic::CommittedAtomicState<'a, Self> {
        let disabled = tail.commit_modeset_disables(modesets);
        let planes = tail.commit_planes(planes, atomic::PlaneCommitFlags::default());
        let enabled = tail.commit_modeset_enables(disabled);
        tail.fake_vblank();
        tail.commit_hw_done(enabled, planes)
    }
}

fn allocate(
    parent: &faux::Device<device::Bound>,
    counts: &Arc<Counts>,
    fail_after_plane: bool,
) -> Result<UnregisteredDevice<TestDriver>> {
    UnregisteredDevice::new(
        parent,
        try_pin_init!(Data {
            counts: counts.clone(),
            fail_after_plane,
            plane: AtomicPtr::new(ptr::null_mut()),
            crtc: AtomicPtr::new(ptr::null_mut()),
            connector: AtomicPtr::new(ptr::null_mut()),
        }),
    )
}

fn create(
    parent: &faux::Device<device::Bound>,
    counts: &Arc<Counts>,
    fail_after_plane: bool,
) -> Result<UnregisteredDevice<TestDriver>> {
    let drm = allocate(parent, counts, fail_after_plane)?;
    // SAFETY: The device was just allocated and remains unregistered. Exercise the same setup
    // hook as normal registration without publishing a DRM minor or enabling external access.
    unsafe { <TestDriver as private::KmsImpl>::setup_kms(&drm) }?;
    Ok(drm)
}

// Use a real internal DRM client for handle tests. Unlike a framebuffer fixture, a handle
// operation needs the native file's GEM tables and the driver's initialized file payload.
// The client is never registered for callbacks; this owner unwinds its initialized resources.
#[cfg(CONFIG_DRM_CLIENT)]
struct HandleClient {
    raw: KBox<crate::types::Opaque<bindings::drm_client_dev>>,
}

#[cfg(CONFIG_DRM_CLIENT)]
impl HandleClient {
    fn new(dev: &Device<TestDriver>) -> Result<Self> {
        let raw = KBox::new(crate::types::Opaque::new(Default::default()), GFP_KERNEL)?;
        // SAFETY: The zeroed client has stable heap storage and no previous initialization.
        // The borrowed device has completed KMS setup. Success retains a native device reference.
        crate::error::to_result(unsafe {
            bindings::drm_client_init(
                dev.as_raw(),
                raw.get(),
                c"rust-kms-handles".as_ptr().cast(),
                ptr::null(),
            )
        })?;
        Ok(Self { raw })
    }

    fn file(&self) -> &drm::File<TestFile> {
        // SAFETY: Successful client initialization opens a native file on TestDriver and calls
        // TestFile::open. The client retains it until drop; the result borrows that ownership.
        unsafe { drm::File::from_raw((*self.raw.get()).file) }
    }

    fn export_dumb(&self) -> Result<ARef<crate::dma_buf::DmaBuf>> {
        let dev = self.file().device_raw();
        let mut args = bindings::drm_mode_create_dumb {
            width: 64,
            height: 64,
            bpp: 32,
            ..Default::default()
        };
        // SAFETY: The native client owns a matching live file and device; arguments are private.
        crate::error::to_result(unsafe {
            (*(*dev).driver).dumb_create.unwrap()(self.file().as_raw(), dev, &mut args)
        })?;
        // SAFETY: Export the newly created handle through native PRIME's file/cache protocol.
        // This returns a DMA-BUF reference without installing a descriptor in any task.
        let raw = crate::error::from_err_ptr(unsafe {
            bindings::drm_gem_prime_handle_to_dmabuf(
                dev,
                self.file().as_raw(),
                args.handle,
                bindings::O_RDWR,
            )
        })?;
        let raw = NonNull::new(raw).ok_or(ENOMEM)?;
        // SAFETY: Native PRIME returned one owned reference to an initialized DMA-BUF.
        Ok(unsafe { crate::dma_buf::DmaBuf::from_owned_raw(raw) })
    }
}

#[cfg(CONFIG_DRM_CLIENT)]
impl Drop for HandleClient {
    fn drop(&mut self) {
        // SAFETY: Release the initialized, unregistered client's file and modeset resources
        // exactly once, before freeing its stable storage. No callback registration escaped.
        unsafe { bindings::drm_client_release(self.raw.get()) };
    }
}

#[cfg(CONFIG_DRM_CLIENT)]
fn failed_foreign_import(native_setup: bool) -> Result {
    let source_counts = Arc::new(Counts::default(), GFP_KERNEL)?;
    let target_counts = Arc::new(Counts::default(), GFP_KERNEL)?;
    let parent = faux::Registration::new(c"rust-kms-prime-failure", None)?;
    let raw_parent = parent.as_ref().as_ref().as_raw();
    // SAFETY: The private faux device has no existing DMA users. Its embedded mask storage
    // remains stable until all test mappings and both DRM devices have been released.
    unsafe { (*raw_parent).dma_mask = &raw mut (*raw_parent).coherent_dma_mask };
    // SAFETY: No earlier allocation or mapping constrains this private test device's DMA mask.
    crate::error::to_result(unsafe { bindings::dma_set_mask_and_coherent(raw_parent, u64::MAX) })?;
    let source = create(parent.as_ref(), &source_counts, false)?;
    let client = HandleClient::new(&source)?;
    let buffer = client.export_dumb()?;
    // SAFETY: Every return path releases registration before the owning faux parent.
    let registration = unsafe {
        drm::Registration::new_static(
            parent.as_ref().as_ref(),
            allocate(parent.as_ref(), &target_counts, false)?,
            Ok::<(), Error>(()),
            0,
        )?
    };
    let expected = if native_setup {
        // SAFETY: This private target has no GEM objects, mappings, or concurrent clients.
        // Replace its empty offset manager with a one-page window: native initialization of
        // the four-page import must fail after the Rust payload has been constructed.
        unsafe {
            let manager = (*registration.device().as_raw()).vma_offset_manager;
            bindings::drm_vma_offset_manager_destroy(manager);
            bindings::drm_vma_offset_manager_init(manager, 1 << 20, 1);
        }
        ENOSPC
    } else {
        target_counts.fail_prime_import.store(1, Ordering::Relaxed);
        EACCES
    };
    let error = {
        let guard = registration.registration_guard().ok_or(ENODEV)?;
        gem::shmem::Object::<TestObject>::import(&guard, &buffer).err()
    };
    assert_eq!(error, Some(expected));
    assert_eq!(target_counts.gem_objects.load(Ordering::Relaxed), 0);
    assert_eq!(
        target_counts.gem_creations.load(Ordering::Relaxed),
        u32::from(native_setup)
    );
    drop(client);
    drop(source);
    drop(buffer);
    drop(registration);
    // SAFETY: No locks are held; KUnit's kernel thread drains delayed DMA-BUF file release.
    unsafe { bindings::flush_delayed_fput() };
    assert_eq!(source_counts.gem_objects.load(Ordering::Relaxed), 0);
    assert_eq!(target_counts.gem_objects.load(Ordering::Relaxed), 0);
    assert_eq!(source_counts.objects.load(Ordering::Relaxed), 0);
    assert_eq!(target_counts.objects.load(Ordering::Relaxed), 0);
    Ok(())
}

// A fixed valid GEM framebuffer fixture, built with kernel helpers rather than a fake DRM file.
// Keep the raw setup here; display transactions below use the shared typed configuration API.
fn framebuffer<D, O>(dev: &Device<D>) -> Result<framebuffer::FramebufferRef<D>>
where
    D: KmsDriver<Object = gem::shmem::Object<O>>,
    O: gem::DriverObject<Driver = D, Args = ()>,
{
    use gem::IntoGEMObject;
    const FUNCS: bindings::drm_framebuffer_funcs = bindings::drm_framebuffer_funcs {
        destroy: Some(bindings::drm_gem_fb_destroy),
        create_handle: Some(bindings::drm_gem_fb_create_handle),
        dirty: None,
    };
    let object = gem::shmem::Object::<O>::new(dev, 640 * 480 * 4, Default::default(), ())?;
    let mut fb = KBox::new(bindings::drm_framebuffer::default(), GFP_KERNEL)?;
    fb.dev = dev.as_raw();
    fb.width = 640;
    fb.height = 480;
    fb.pitches[0] = 640 * 4;
    fb.modifier = fourcc::FORMAT_MOD_LINEAR;
    // SAFETY: The known packed format has one four-byte plane, matching the allocation above.
    fb.format = unsafe { bindings::drm_format_info(fourcc::XRGB8888) };
    fb.obj[0] = object.as_raw();
    // SAFETY: All metadata and the owned GEM reference are initialized before publication.
    // Failure leaves both Rust allocations owned locally for normal unwind.
    crate::error::to_result(unsafe {
        bindings::drm_framebuffer_init(dev.as_raw(), &mut *fb, &FUNCS)
    })?;
    // Transfer the GEM reference to drm_gem_fb_destroy.
    // SAFETY: The framebuffer's native users retain `dev`; Rust handles pair its lifetime too.
    let _ = unsafe { object.into_native() };
    let raw = KBox::into_raw(fb);
    // SAFETY: The initializer gave us a live framebuffer, with `dev` borrowed throughout.
    let owned = unsafe { framebuffer::Framebuffer::<D>::from_raw(raw) }.to_owned_ref();
    // SAFETY: Drop the initial native reference; `owned` now retains the framebuffer and device.
    unsafe { bindings::drm_framebuffer_put(raw) };
    Ok(owned)
}

fn mode() -> Result<modes::DisplayMode> {
    modes::DisplayMode::from_timings(modes::ModeTimings {
        clock_khz: 25175,
        hdisplay: 640,
        hsync_start: 656,
        hsync_end: 752,
        htotal: 800,
        vdisplay: 480,
        vsync_start: 490,
        vsync_end: 492,
        vtotal: 525,
        flags: modes::ModeFlags::NHSYNC | modes::ModeFlags::NVSYNC,
    })
}

fn device_references<D: KmsDriver>(dev: &Device<D>) -> i32 {
    // SAFETY: The device borrow keeps its aligned native kref counter alive. Read it through
    // the kernel atomic API, matching the native refcount access discipline.
    unsafe {
        crate::sync::atomic::Atomic::<i32>::from_ptr(
            &raw mut (*dev.as_raw()).ref_.refcount.refs.counter,
        )
        .load(crate::sync::atomic::Relaxed)
    }
}

struct ContentionResult {
    result: Result,
    older_errno: i32,
    attempts: u32,
    deadlocks: u32,
    fresh: bool,
    published: u64,
    objects: u32,
    plane_states: u32,
    crtc_states: u32,
}

// Force opposing lock order with two real acquire contexts. No lock-error injection is used.
fn contended_update(check_only: bool, consume_deadlock: bool) -> Result<ContentionResult> {
    use crate::{sync::Completion, workqueue};
    use core::sync::atomic::AtomicI32;
    use crtc::AsRawCrtc;
    use plane::AsRawPlane;

    let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
    let parent = faux::Registration::new(c"rust-kms-contention", None)?;
    let dev = create(parent.as_ref(), &counts, false)?;
    let older_has_crtc = Arc::pin_init(Completion::new(), GFP_KERNEL)?;
    let younger_has_plane = Arc::pin_init(Completion::new(), GFP_KERNEL)?;
    let done = Arc::pin_init(Completion::new(), GFP_KERNEL)?;
    let older_errno = Arc::new(AtomicI32::new(0), GFP_KERNEL)?;
    let worker_dev: ARef<Device<TestDriver>> = (&*dev).into();
    let worker_crtc = older_has_crtc.clone();
    let worker_plane = younger_has_plane.clone();
    let worker_done = done.clone();
    let worker_errno = older_errno.clone();
    workqueue::system_dfl().try_spawn(GFP_KERNEL, move || {
        // SAFETY: Setup completed before spawn. The test owns the device and excludes object
        // creation, registration and teardown until both transactions have returned.
        let result = unsafe {
            let crtc = crtc::Crtc::<TestCrtc>::from_raw(worker_dev.crtc.load(Ordering::Relaxed));
            let plane =
                plane::Plane::<TestPlane>::from_raw(worker_dev.plane.load(Ordering::Relaxed));
            atomic::run_check(&worker_dev, |state| {
                let _crtc = state.add_crtc_state(crtc)?;
                worker_crtc.complete_all();
                worker_plane.wait_for_completion();
                let _plane = state.add_plane_state(plane)?;
                Ok(())
            })
        };
        worker_errno.store(result.err().map_or(0, Error::to_errno), Ordering::Release);
        // Wake the main task even if the older transaction failed before its callback ran.
        worker_crtc.complete_all();
        drop(worker_dev);
        worker_done.complete_all();
    })?;
    older_has_crtc.wait_for_completion();
    // SAFETY: The initialized device owns these objects until after both tasks finish. Each
    // transaction acquires its own object locks before accessing mutable state.
    let crtc = unsafe { crtc::Crtc::<TestCrtc>::from_raw(dev.crtc.load(Ordering::Relaxed)) };
    let plane = unsafe { plane::Plane::<TestPlane>::from_raw(dev.plane.load(Ordering::Relaxed)) };
    let mut attempts = 0;
    let mut deadlocks = 0;
    let mut fresh = true;
    let update = |state: Pin<&mut atomic::AtomicStateComposer<TestDriver>>| {
        attempts += 1;
        let mut plane_state = state.add_plane_state(plane)?;
        fresh &= *plane_state.value == 0;
        *plane_state.value = 17;
        younger_has_plane.complete_all();
        match state.add_crtc_state(crtc) {
            Ok(_) => Ok(()),
            Err(error) => {
                if error == EDEADLK {
                    deadlocks += 1;
                    if consume_deadlock {
                        return Ok(());
                    }
                }
                Err(error)
            }
        }
    };
    // SAFETY: Setup completed, and neither task registers or tears down the device. Starting
    // only after the older task acquired its CRTC establishes the acquire-context age order.
    let result = unsafe {
        if check_only {
            atomic::run_check(&dev, update)
        } else {
            atomic::run_update(&dev, update)
        }
    };
    // No early return after spawning: unblock the worker even on allocation/callback failure.
    younger_has_plane.complete_all();
    done.wait_for_completion();
    let mut published = 0;
    // SAFETY: Both concurrent attempts ended, and the same initialized-device exclusion holds.
    unsafe {
        atomic::run_check(&dev, |state| {
            published = *state.add_plane_state(plane)?.value;
            Ok(())
        })
    }?;
    drop(dev);
    Ok(ContentionResult {
        result,
        older_errno: older_errno.load(Ordering::Acquire),
        attempts,
        deadlocks,
        fresh,
        published,
        objects: counts.objects.load(Ordering::Relaxed),
        plane_states: counts.plane_states.load(Ordering::Relaxed),
        crtc_states: counts.crtc_states.load(Ordering::Relaxed),
    })
}

#[kunit_tests(rust_drm_kms)]
mod cases {
    use super::*;
    use crtc::AsRawCrtc;
    use encoder::AsRawEncoder;
    use plane::AsRawPlane;

    #[cfg(CONFIG_DRM_CLIENT)]
    #[test]
    fn foreign_import_rejection_releases_attachment() -> Result {
        failed_foreign_import(false)
    }

    #[cfg(CONFIG_DRM_CLIENT)]
    #[test]
    fn foreign_import_native_failure_drops_payload() -> Result {
        failed_foreign_import(true)
    }

    #[cfg(CONFIG_DRM_CLIENT)]
    #[test]
    fn foreign_import_retains_typed_storage_without_vmap() -> Result {
        use gem::{BaseObject, IntoGEMObject};

        let source_counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let target_counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-prime-import", None)?;
        let raw_parent = parent.as_ref().as_ref().as_raw();
        // SAFETY: This private faux device has no DMA users yet. Provide stable mask storage
        // for native direct-DMA mappings in the test VM; no physical device is accessed.
        unsafe { (*raw_parent).dma_mask = &raw mut (*raw_parent).coherent_dma_mask };
        // SAFETY: The private test device has no allocations or mappings with an older mask.
        crate::error::to_result(unsafe {
            bindings::dma_set_mask_and_coherent(raw_parent, u64::MAX)
        })?;
        let source = create(parent.as_ref(), &source_counts, false)?;
        let client = HandleClient::new(&source)?;
        let original = client.export_dumb()?;
        let buffer = original.clone();
        drop(original);
        assert_eq!(buffer.size(), 16384);
        // SAFETY: Registration is dropped before the owning faux parent on every return path.
        let registration = unsafe {
            drm::Registration::new_static(
                parent.as_ref().as_ref(),
                allocate(parent.as_ref(), &target_counts, false)?,
                Ok::<(), Error>(()),
                0,
            )?
        };
        let imported = {
            let guard = registration.registration_guard().ok_or(ENODEV)?;
            gem::shmem::Object::<TestObject>::import(&guard, &buffer)?
        };
        assert_eq!(imported.size(), 16384);
        assert_eq!(imported.allocated_size, 16384);
        assert_eq!(target_counts.gem_objects.load(Ordering::Relaxed), 1);
        // SAFETY: Import completed before publication. These backing fields remain immutable.
        let (attachment, filp, reservation, source_reservation) = unsafe {
            (
                (*imported.as_raw()).import_attach,
                (*imported.as_raw()).filp,
                (*imported.as_raw()).resv,
                (*buffer.as_raw()).resv,
            )
        };
        assert!(!attachment.is_null());
        assert!(filp.is_null());
        assert_eq!(reservation, source_reservation);
        // Reading an imported table must not transfer its destruction to local shmem cleanup.
        let table = imported.sg_table(parent.as_ref().as_ref())? as *const _;
        assert_eq!(imported.sg_table(parent.as_ref().as_ref())? as *const _, table);
        drop(client);
        drop(source);
        drop(buffer);
        drop(registration);
        assert_eq!(source_counts.gem_objects.load(Ordering::Relaxed), 1);
        assert_eq!(target_counts.gem_objects.load(Ordering::Relaxed), 1);
        drop(imported);
        // SAFETY: KUnit runs in a kernel thread; drain deferred file release before inspecting
        // exporter lifetime. No object or reservation locks are held across the flush.
        unsafe { bindings::flush_delayed_fput() };
        assert_eq!(source_counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(target_counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(source_counts.objects.load(Ordering::Relaxed), 0);
        assert_eq!(target_counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[cfg(CONFIG_DRM_CLIENT)]
    #[test]
    fn native_dumb_handle_retains_typed_object() -> Result {
        use gem::BaseObject;

        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-dumb-handle", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        let client = HandleClient::new(&drm)?;
        let mut args = bindings::drm_mode_create_dumb {
            width: 64,
            height: 64,
            bpp: 32,
            ..Default::default()
        };
        // SAFETY: The installed callback belongs to the live, privately owned device. The
        // native client file belongs to that device and the arguments are exclusively owned.
        let result = unsafe {
            (*(*drm.as_raw()).driver).dumb_create.unwrap()(
                client.file().as_raw(),
                drm.as_raw(),
                &mut args,
            )
        };
        crate::error::to_result(result)?;
        assert_ne!(args.handle, 0);
        assert_eq!(args.pitch, 256);
        assert_eq!(args.size, 16384);
        let object = gem::shmem::Object::<TestObject>::lookup_handle(client.file(), args.handle)?;
        assert_eq!(object.size(), 16384);
        assert_eq!(object.allocated_size, 16384);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 1);
        drop(client);
        drop(drm);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 4);
        assert_eq!(object.allocated_size, 16384);
        drop(object);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[cfg(CONFIG_DRM_CLIENT)]
    #[test]
    fn native_dumb_handle_rejection_unwinds_payload() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-dumb-reject", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        let client = HandleClient::new(&drm)?;
        let mut args = bindings::drm_mode_create_dumb {
            width: 64,
            height: 64,
            bpp: 32,
            ..Default::default()
        };
        counts.fail_gem_open.store(1, Ordering::Relaxed);
        // SAFETY: The real internal client's file belongs to this initialized device. Its
        // installed callback receives exclusive valid arguments, as in the success case.
        let result = unsafe {
            (*(*drm.as_raw()).driver).dumb_create.unwrap()(
                client.file().as_raw(),
                drm.as_raw(),
                &mut args,
            )
        };
        assert_eq!(result, EACCES.to_errno());
        assert_eq!(args.handle, 0);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        drop(client);
        drop(drm);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn constructed_mode_has_crtc_timings() -> Result {
        let mode = mode()?;
        assert_eq!(mode.crtc_clock(), 25175);
        assert_eq!(mode.crtc_vtotal(), 525);
        assert_eq!(mode.crtc_vblank_start(), 480);
        assert_eq!(mode.crtc_vblank_end(), 525);
        Ok(())
    }

    #[test]
    fn plane_name_is_literal() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-plane-name", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        // SAFETY: The device remains unregistered and owns the additional plane until teardown.
        let dev = unsafe { UnregisteredKmsDevice::new(&drm) };
        let name = c"plane-%%";
        let plane = plane::UnregisteredPlane::<TestPlane>::new(
            &dev,
            1,
            &[fourcc::XRGB8888],
            None,
            plane::Type::Overlay,
            Some(name),
            (),
        )?;
        // SAFETY: Successful initialization owns a NUL-terminated name for the device lifetime.
        let stored = unsafe { CStr::from_char_ptr((*plane.as_raw()).name) };
        assert_eq!(stored, name);
        Ok(())
    }

    #[test]
    fn crtc_name_is_literal() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-crtc-name", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        // SAFETY: The device remains unregistered and owns the additional objects until teardown.
        let dev = unsafe { UnregisteredKmsDevice::new(&drm) };
        let primary = plane::UnregisteredPlane::<TestPlane>::new(
            &dev,
            0,
            &[fourcc::XRGB8888],
            None,
            plane::Type::Primary,
            None,
            (),
        )?;
        let name = c"crtc-%%";
        let crtc = crtc::UnregisteredCrtc::<TestCrtc>::new(
            &dev,
            primary,
            None::<&plane::UnregisteredPlane<TestPlane>>,
            Some(name),
            (),
        )?;
        // SAFETY: Successful initialization owns a NUL-terminated name for the device lifetime.
        let stored = unsafe { CStr::from_char_ptr((*crtc.as_raw()).name) };
        assert_eq!(stored, name);
        Ok(())
    }

    #[test]
    fn encoder_name_is_literal() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-encoder-name", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        // SAFETY: The device remains unregistered and owns the additional encoder until teardown.
        let dev = unsafe { UnregisteredKmsDevice::new(&drm) };
        let name = c"encoder-%%";
        let encoder = encoder::UnregisteredEncoder::<TestEncoder>::new(
            &dev,
            encoder::Type::Virtual,
            1,
            0,
            Some(name),
            (),
        )?;
        // SAFETY: Successful initialization owns a NUL-terminated name for the device lifetime.
        let stored = unsafe { CStr::from_char_ptr((*encoder.as_raw()).name) };
        assert_eq!(stored, name);
        Ok(())
    }

    #[test]
    fn initial_state_has_parents() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-state", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        // SAFETY: Successful setup initialized the mode configuration; the device is still
        // unregistered and no other thread accesses its objects.
        let dev = unsafe { UnregisteredKmsDevice::new(&drm) };
        assert_eq!(dev.num_crtcs(), 1);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 4);
        let plane = drm.plane.load(Ordering::Relaxed);
        let crtc = drm.crtc.load(Ordering::Relaxed);
        let connector = drm.connector.load(Ordering::Relaxed);
        // SAFETY: Successful setup initialized these objects and their states. The unregistered
        // device owns all of them, remains borrowed here, and has no concurrent state updates.
        let (plane_state, crtc_state, connector_state) =
            unsafe { ((*plane).state, (*crtc).state, (*connector).state) };
        assert!(!plane_state.is_null());
        assert!(!crtc_state.is_null());
        assert!(!connector_state.is_null());
        // SAFETY: The initialized states remain owned by the borrowed device, as above.
        let parents = unsafe {
            (
                (*plane_state).plane,
                (*crtc_state).crtc,
                (*connector_state).connector,
            )
        };
        assert_eq!(parents, (plane, crtc, connector));
        Ok(())
    }

    #[test]
    fn mode_objects_are_destroyed() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-destroy", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        assert_eq!(counts.objects.load(Ordering::Relaxed), 4);
        drop(drm);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn initialized_mode_config_lock() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-config-lock", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        // SAFETY: Successful setup initialized mode configuration, and only this thread has
        // access to the unregistered device's objects.
        let dev = unsafe { UnregisteredKmsDevice::new(&drm) };
        let guard = dev.mode_config_lock();
        let first_matches = ptr::eq(guard.drm_dev(), &*drm);
        drop(guard);
        // Reacquiring also exercises the first guard's unlock path.
        let guard = dev.mode_config_lock();
        let second_matches = ptr::eq(guard.drm_dev(), &*drm);
        drop(guard);
        drop(drm);
        assert!(first_matches);
        assert!(second_matches);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn rejected_atomic_update_preserves_state() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-reject-update", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        // SAFETY: Setup completed and this task exclusively owns the unregistered device.
        let crtc = unsafe { crtc::Crtc::<TestCrtc>::from_raw(drm.crtc.load(Ordering::Relaxed)) };
        // SAFETY: No concurrent update accesses the initial published state.
        let initial = unsafe { (*crtc.as_raw()).state };
        let mut attempts = 0;
        let mut duplicated = false;
        // SAFETY: Initial state exists, with no registration, object creation or teardown during
        // either call. Reentering after rejection checks that the prior call released its lock.
        let rejected = unsafe {
            atomic::run_update(&drm, |state| {
                attempts += 1;
                let new = state.add_crtc_state(crtc)?;
                duplicated = state.get_old_crtc_state(crtc).is_some();
                drop(new);
                Err(EINVAL)
            })
        };
        let retried = unsafe {
            atomic::run_update(&drm, |state| {
                attempts += 1;
                let _new = state.add_crtc_state(crtc)?;
                Err(ECANCELED)
            })
        };
        // SAFETY: Both blocking calls finished; there is no concurrent update.
        let final_state = unsafe { (*crtc.as_raw()).state };
        drop(drm);
        assert_eq!(rejected, Err(EINVAL));
        assert_eq!(retried, Err(ECANCELED));
        assert!(duplicated);
        assert_eq!(attempts, 2);
        assert_eq!(initial, final_state);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn atomic_primary_modeset() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-modeset", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        let fb = framebuffer(&drm)?;
        let mode = mode()?;
        // SAFETY: The fully initialized, unregistered device owns these objects exclusively.
        let crtc = unsafe { crtc::Crtc::<TestCrtc>::from_raw(drm.crtc.load(Ordering::Relaxed)) };
        let connector = unsafe {
            <connector::Connector<TestConnector> as connector::AsRawConnector>::from_raw(
                drm.connector.load(Ordering::Relaxed),
            )
        };
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[connector],
            position: (0, 0),
        };
        // SAFETY: Initial states exist; setup, registration and teardown are excluded.
        unsafe { atomic::run_update(&drm, |state| state.set_crtc_config(crtc, Some(&scanout))) }?;
        // SAFETY: The blocking update completed, and no other task modifies this device.
        let (active, selected_fb) = unsafe {
            (
                (*(*crtc.as_raw()).state).active,
                (*(*drm.plane.load(Ordering::Relaxed)).state).fb,
            )
        };
        let matches = selected_fb == fb.as_raw();
        let plane_updates = counts.plane_updates.load(Ordering::Relaxed);
        // SAFETY: Same exclusive initialized-device lifetime as the modeset above.
        unsafe { atomic::run_update(&drm, |state| state.set_crtc_config(crtc, None)) }?;
        drop(fb);
        drop(drm);
        assert!(active);
        assert!(matches);
        assert_eq!(plane_updates, 1);
        assert_eq!(counts.enables.load(Ordering::Relaxed), 1);
        assert_eq!(counts.disables.load(Ordering::Relaxed), 1);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn shared_reservation_does_not_retain_own_device() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-reservation-owner", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        let baseline = device_references(&drm);
        let owner = gem::shmem::Object::<TestObject>::new(&drm, 4096, Default::default(), ())?;
        let child = gem::shmem::Object::<TestObject>::new(
            &drm,
            4096,
            gem::shmem::ObjectConfig {
                parent_resv_obj: Some(&owner),
                ..Default::default()
            },
            (),
        )?;
        drop(owner);
        // Only the child Rust handle should retain the device. Its embedded reservation-owner
        // reference must not create a device cycle if native KMS state later retains the child.
        let references = device_references(&drm);
        drop(child);
        let after_drop = device_references(&drm);
        drop(drm);
        assert_eq!(references, baseline + 1);
        assert_eq!(after_drop, baseline);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn shared_reservation_retains_foreign_device() -> Result {
        let first_counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let second_counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-foreign-reservation", None)?;
        let first = create(parent.as_ref(), &first_counts, false)?;
        let second = create(parent.as_ref(), &second_counts, false)?;
        let owner = gem::shmem::Object::<TestObject>::new(&first, 4096, Default::default(), ())?;
        let child = gem::shmem::Object::<TestObject>::new(
            &second,
            4096,
            gem::shmem::ObjectConfig {
                parent_resv_obj: Some(&owner),
                ..Default::default()
            },
            (),
        )?;
        drop(owner);
        drop(first);
        drop(second);
        let first_live = first_counts.objects.load(Ordering::Relaxed);
        let second_live = second_counts.objects.load(Ordering::Relaxed);
        drop(child);
        assert_eq!(first_live, 4);
        assert_eq!(second_live, 4);
        assert_eq!(first_counts.objects.load(Ordering::Relaxed), 0);
        assert_eq!(second_counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn owned_shmem_retains_device() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-shmem-owner", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        let object = gem::shmem::Object::<TestObject>::new(&drm, 4096, Default::default(), ())?;
        let copy = object.clone();
        drop(object);
        drop(drm);
        let objects_while_owned = counts.objects.load(Ordering::Relaxed);
        drop(copy);
        assert_eq!(objects_while_owned, 4);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn owned_shmem_mapping_retains_device() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-shmem-map-owner", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        let object = gem::shmem::Object::<TestObject>::new(&drm, 4096, Default::default(), ())?;
        let mapping = object.owned_vmap::<4096>()?;
        drop(object);
        drop(drm);
        let objects_while_mapped = counts.objects.load(Ordering::Relaxed);
        drop(mapping);
        assert_eq!(objects_while_mapped, 4);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn owned_framebuffer_retains_device() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-framebuffer-owner", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        let fb = framebuffer(&drm)?;
        let copy = fb.clone();
        drop(fb);
        drop(drm);
        // No KMS state references the framebuffer. The remaining owned copy must retain both
        // allocations, including the device's mode objects, until its destructor has run.
        let objects_while_owned = counts.objects.load(Ordering::Relaxed);
        let dimensions = (copy.width(), copy.height());
        drop(copy);
        assert_eq!(objects_while_owned, 4);
        assert_eq!(dimensions, (640, 480));
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.connector_states.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn atomic_primary_flip_retires_framebuffer() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-flip", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        let first = framebuffer(&drm)?;
        let second = framebuffer(&drm)?;
        let mode = mode()?;
        // SAFETY: Setup completed and this task exclusively owns the unregistered device.
        let crtc = unsafe { crtc::Crtc::<TestCrtc>::from_raw(drm.crtc.load(Ordering::Relaxed)) };
        let connector = unsafe {
            <connector::Connector<TestConnector> as connector::AsRawConnector>::from_raw(
                drm.connector.load(Ordering::Relaxed),
            )
        };
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &first,
            connectors: &[connector],
            position: (0, 0),
        };
        // SAFETY: Initial state exists with no concurrent registration, setup or teardown.
        unsafe { atomic::run_update(&drm, |state| state.set_crtc_config(crtc, Some(&scanout))) }?;
        // Leave the first framebuffer owned only by the published scanout state.
        drop(first);
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &second,
            connectors: &[connector],
            position: (0, 0),
        };
        // SAFETY: Same exclusively owned, initialized device as the first update.
        unsafe { atomic::run_update(&drm, |state| state.set_crtc_config(crtc, Some(&scanout))) }?;
        // SAFETY: Blocking commit completed. This private device has no concurrent framebuffer
        // allocations or commits, so its count and published state are stable for inspection.
        let (remaining, selected) = unsafe {
            (
                (*drm.as_raw()).mode_config.num_fb,
                (*(*drm.plane.load(Ordering::Relaxed)).state).fb,
            )
        };
        let matches = selected == second.as_raw();
        let updates = counts.plane_updates.load(Ordering::Relaxed);
        let enables = counts.enables.load(Ordering::Relaxed);
        let disables = counts.disables.load(Ordering::Relaxed);
        // SAFETY: Same exclusive initialized-device lifetime as the two updates above.
        unsafe { atomic::run_update(&drm, |state| state.set_crtc_config(crtc, None)) }?;
        drop(second);
        // SAFETY: No other task owns or allocates framebuffers on this test device.
        let after_disable = unsafe { (*drm.as_raw()).mode_config.num_fb };
        drop(drm);
        assert_eq!(remaining, 1);
        assert!(matches);
        assert_eq!(updates, 2);
        assert_eq!(enables, 1);
        assert_eq!(disables, 0);
        assert_eq!(after_disable, 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn atomic_check_does_not_publish() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-check-only", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        let fb = framebuffer(&drm)?;
        let mode = mode()?;
        // SAFETY: The initialized device owns these objects and has no concurrent updates.
        let crtc = unsafe { crtc::Crtc::<TestCrtc>::from_raw(drm.crtc.load(Ordering::Relaxed)) };
        let connector = unsafe {
            <connector::Connector<TestConnector> as connector::AsRawConnector>::from_raw(
                drm.connector.load(Ordering::Relaxed),
            )
        };
        let initial = unsafe { (*crtc.as_raw()).state };
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[connector],
            position: (0, 0),
        };
        // SAFETY: Full setup completed; registration, object creation and teardown are excluded.
        unsafe { atomic::run_check(&drm, |state| state.set_crtc_config(crtc, Some(&scanout))) }?;
        // SAFETY: Validation returned and no other task accesses the device's published state.
        let (unchanged, active, selected) = unsafe {
            (
                (*crtc.as_raw()).state == initial,
                (*(*crtc.as_raw()).state).active,
                (*(*drm.plane.load(Ordering::Relaxed)).state).fb,
            )
        };
        let callbacks = (
            counts.plane_updates.load(Ordering::Relaxed),
            counts.enables.load(Ordering::Relaxed),
            counts.disables.load(Ordering::Relaxed),
        );
        // SAFETY: A fresh transaction owns the same exclusively accessed initialized device.
        // Successful submission also checks that validation released its lock and temporary state.
        unsafe { atomic::run_update(&drm, |state| state.set_crtc_config(crtc, Some(&scanout))) }?;
        unsafe { atomic::run_update(&drm, |state| state.set_crtc_config(crtc, None)) }?;
        drop(fb);
        drop(drm);
        assert!(unchanged);
        assert!(!active);
        assert!(selected.is_null());
        assert_eq!(callbacks, (0, 0, 0));
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn crtc_payload_initialization_failure_unwinds() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-crtc-state-init", None)?;
        counts.fail_crtc_state_alloc.store(1, Ordering::Relaxed);
        let failed = create(parent.as_ref(), &counts, false).err();
        let remaining = counts.objects.load(Ordering::Relaxed);
        counts.fail_crtc_state_alloc.store(0, Ordering::Relaxed);
        let drm = create(parent.as_ref(), &counts, false)?;
        let initialized = counts.crtc_states.load(Ordering::Relaxed);
        drop(drm);
        assert_eq!(failed, Some(ENOMEM));
        assert_eq!(remaining, 0);
        assert_eq!(initialized, 1);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn crtc_payload_duplication_failure_preserves_state() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-crtc-state-dup", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        // SAFETY: Full setup completed; the private device has no concurrent users.
        let crtc = unsafe { crtc::Crtc::<TestCrtc>::from_raw(drm.crtc.load(Ordering::Relaxed)) };
        let initial = unsafe { (*crtc.as_raw()).state };
        counts.fail_crtc_state_alloc.store(1, Ordering::Relaxed);
        // SAFETY: Initial states exist with no concurrent setup, registration or teardown.
        let failed = unsafe {
            atomic::run_check(&drm, |state| {
                let _new = state.add_crtc_state(crtc)?;
                Ok(())
            })
        };
        // SAFETY: Validation returned; no other task modifies the published state.
        let unchanged = unsafe { (*crtc.as_raw()).state == initial };
        let remaining = counts.crtc_states.load(Ordering::Relaxed);
        counts.fail_crtc_state_alloc.store(0, Ordering::Relaxed);
        // SAFETY: Same exclusive initialized-device lifetime as the failed check.
        unsafe {
            atomic::run_check(&drm, |state| {
                let _new = state.add_crtc_state(crtc)?;
                Ok(())
            })
        }?;
        drop(drm);
        assert_eq!(failed, Err(ENOMEM));
        assert!(unchanged);
        assert_eq!(remaining, 1);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn crtc_payload_duplication_is_independent() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-crtc-state-copy", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        // SAFETY: Setup completed and this task exclusively owns the unregistered device.
        let crtc = unsafe { crtc::Crtc::<TestCrtc>::from_raw(drm.crtc.load(Ordering::Relaxed)) };
        let mut observed = (0, 0);
        // SAFETY: Initial states exist and setup, registration and teardown are excluded.
        unsafe {
            atomic::run_update(&drm, |state| {
                *state.add_crtc_state(crtc)?.value = 7;
                Ok(())
            })
        }?;
        // SAFETY: Same initialized-device exclusion as the preceding update. Mutate only the
        // unpublished copy and observe the old payload through the transaction's read accessor.
        unsafe {
            atomic::run_check(&drm, |state| {
                let mut new = state.add_crtc_state(crtc)?;
                observed.0 = *new.value;
                *new.value = 9;
                observed.1 = *state.get_old_crtc_state(crtc).ok_or(EINVAL)?.value;
                Ok(())
            })
        }?;
        drop(drm);
        assert_eq!(observed, (7, 7));
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn plane_payload_initialization_failure_unwinds() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-plane-state-init", None)?;
        counts.fail_plane_state_alloc.store(1, Ordering::Relaxed);
        let failed = create(parent.as_ref(), &counts, false).err();
        let remaining = counts.objects.load(Ordering::Relaxed);
        counts.fail_plane_state_alloc.store(0, Ordering::Relaxed);
        let drm = create(parent.as_ref(), &counts, false)?;
        let initialized = counts.plane_states.load(Ordering::Relaxed);
        drop(drm);
        assert_eq!(failed, Some(ENOMEM));
        assert_eq!(remaining, 0);
        assert_eq!(initialized, 1);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn plane_payload_duplication_failure_preserves_state() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-plane-state-dup", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        // SAFETY: Full setup completed; the private device has no concurrent users.
        let plane =
            unsafe { plane::Plane::<TestPlane>::from_raw(drm.plane.load(Ordering::Relaxed)) };
        let initial = unsafe { (*plane.as_raw()).state };
        counts.fail_plane_state_alloc.store(1, Ordering::Relaxed);
        // SAFETY: Initial states exist with no concurrent setup, registration or teardown.
        let failed = unsafe {
            atomic::run_check(&drm, |state| {
                let _new = state.add_plane_state(plane)?;
                Ok(())
            })
        };
        // SAFETY: Validation returned; no other task modifies the published state.
        let unchanged = unsafe { (*plane.as_raw()).state == initial };
        let remaining = counts.plane_states.load(Ordering::Relaxed);
        counts.fail_plane_state_alloc.store(0, Ordering::Relaxed);
        // SAFETY: Same exclusive initialized-device lifetime as the failed check.
        unsafe {
            atomic::run_check(&drm, |state| {
                let _new = state.add_plane_state(plane)?;
                Ok(())
            })
        }?;
        drop(drm);
        assert_eq!(failed, Err(ENOMEM));
        assert!(unchanged);
        assert_eq!(remaining, 1);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn plane_payload_duplication_is_independent() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-plane-state-copy", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        // SAFETY: Setup completed and this task exclusively owns the unregistered device.
        let plane =
            unsafe { plane::Plane::<TestPlane>::from_raw(drm.plane.load(Ordering::Relaxed)) };
        let mut observed = (0, 0);
        // SAFETY: Initial states exist and setup, registration and teardown are excluded.
        unsafe {
            atomic::run_update(&drm, |state| {
                *state.add_plane_state(plane)?.value = 7;
                Ok(())
            })
        }?;
        // SAFETY: Same initialized-device exclusion as the preceding update. Only the
        // unpublished copy is mutated; the old payload remains shared read-only.
        unsafe {
            atomic::run_check(&drm, |state| {
                let mut new = state.add_plane_state(plane)?;
                observed.0 = *new.value;
                *new.value = 9;
                observed.1 = *state.get_old_plane_state(plane).ok_or(EINVAL)?.value;
                Ok(())
            })
        }?;
        drop(drm);
        assert_eq!(observed, (7, 7));
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn connector_payload_initialization_failure_unwinds() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-conn-state-init", None)?;
        counts
            .fail_connector_state_alloc
            .store(1, Ordering::Relaxed);
        let failed = create(parent.as_ref(), &counts, false).err();
        let remaining = counts.objects.load(Ordering::Relaxed);
        counts
            .fail_connector_state_alloc
            .store(0, Ordering::Relaxed);
        let drm = create(parent.as_ref(), &counts, false)?;
        let initialized = counts.connector_states.load(Ordering::Relaxed);
        drop(drm);
        assert_eq!(failed, Some(ENOMEM));
        assert_eq!(remaining, 0);
        assert_eq!(initialized, 1);
        assert_eq!(counts.connector_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn connector_payload_duplication_failure_preserves_state() -> Result {
        use connector::AsRawConnector;

        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-conn-state-dup", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        // SAFETY: Full setup completed; the private device has no concurrent users.
        let connector = unsafe {
            connector::Connector::<TestConnector>::from_raw(drm.connector.load(Ordering::Relaxed))
        };
        let initial = unsafe { (*connector.as_raw()).state };
        counts
            .fail_connector_state_alloc
            .store(1, Ordering::Relaxed);
        // SAFETY: Initial states exist with no concurrent setup, registration or teardown.
        let failed = unsafe {
            atomic::run_check(&drm, |state| {
                let _new = state.add_connector_state(connector)?;
                Ok(())
            })
        };
        // SAFETY: Validation returned; no other task modifies the published state.
        let unchanged = unsafe { (*connector.as_raw()).state == initial };
        let remaining = counts.connector_states.load(Ordering::Relaxed);
        counts
            .fail_connector_state_alloc
            .store(0, Ordering::Relaxed);
        // SAFETY: Same exclusive initialized-device lifetime as the failed check.
        unsafe {
            atomic::run_check(&drm, |state| {
                let _new = state.add_connector_state(connector)?;
                Ok(())
            })
        }?;
        drop(drm);
        assert_eq!(failed, Err(ENOMEM));
        assert!(unchanged);
        assert_eq!(remaining, 1);
        assert_eq!(counts.connector_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn connector_payload_duplication_is_independent() -> Result {
        use connector::AsRawConnector;

        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-conn-state-copy", None)?;
        let drm = create(parent.as_ref(), &counts, false)?;
        // SAFETY: Setup completed and this task exclusively owns the unregistered device.
        let connector = unsafe {
            connector::Connector::<TestConnector>::from_raw(drm.connector.load(Ordering::Relaxed))
        };
        let mut observed = (0, 0);
        // SAFETY: Initial states exist and setup, registration and teardown are excluded.
        unsafe {
            atomic::run_update(&drm, |state| {
                *state.add_connector_state(connector)?.value = 7;
                Ok(())
            })
        }?;
        // SAFETY: Same initialized-device exclusion as the preceding update. Only the
        // unpublished copy is mutated; the old payload remains shared read-only.
        unsafe {
            atomic::run_check(&drm, |state| {
                let mut new = state.add_connector_state(connector)?;
                observed.0 = *new.value;
                *new.value = 9;
                observed.1 = *state
                    .get_old_connector_state(connector)
                    .ok_or(EINVAL)?
                    .value;
                Ok(())
            })
        }?;
        drop(drm);
        assert_eq!(observed, (7, 7));
        assert_eq!(counts.connector_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn registered_device_lifecycle() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-registration", None)?;
        // SAFETY: Every return path drops registration before the owning faux parent.
        let registration = unsafe {
            drm::Registration::new_static(
                parent.as_ref().as_ref(),
                allocate(parent.as_ref(), &counts, false)?,
                Ok::<(), Error>(()),
                0,
            )?
        };
        let retained: ARef<Device<TestDriver>> = registration.device().into();
        let crtc_count = {
            let registered = registration.registration_guard().ok_or(ENODEV)?;
            registered.check_atomic_update(|_| Ok(()))?;
            registered.num_crtcs()
        };
        drop(registration);
        // SAFETY: Registration succeeded and retained owns the device after unplug. Ioctl
        // context requires past registration, not current registration or a bound parent.
        let unplugged = unsafe { retained.assume_ctx::<drm::Ioctl>() };
        let rejected = unplugged.registration_guard().is_none();
        let remaining = counts.objects.load(Ordering::Relaxed);
        drop(retained);
        assert_eq!(crtc_count, 1);
        assert!(rejected);
        assert_eq!(remaining, 4);
        assert_eq!(counts.connector_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn registered_unplug_disables_scanout() -> Result {
        use connector::AsRawConnector;

        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-active-unplug", None)?;
        // SAFETY: Every return path drops registration before the owning faux parent.
        let registration = unsafe {
            drm::Registration::new_static(
                parent.as_ref().as_ref(),
                allocate(parent.as_ref(), &counts, false)?,
                Ok::<(), Error>(()),
                0,
            )?
        };
        let retained: ARef<Device<TestDriver>> = registration.device().into();
        {
            let registered = registration.registration_guard().ok_or(ENODEV)?;
            // SAFETY: Registration completed setup. The device reference owns these immutable
            // mode objects, and the guard excludes unplug while the transaction uses them.
            let crtc = unsafe {
                crtc::Crtc::<TestCrtc>::from_raw(registered.crtc.load(Ordering::Relaxed))
            };
            let connector = unsafe {
                connector::Connector::<TestConnector>::from_raw(
                    registered.connector.load(Ordering::Relaxed),
                )
            };
            let fb = framebuffer(&registered)?;
            let mode = mode()?;
            let scanout = atomic::CrtcScanout {
                mode: &mode,
                framebuffer: &fb,
                connectors: &[connector],
                position: (0, 0),
            };
            registered.check_atomic_update(|state| state.set_crtc_config(crtc, Some(&scanout)))?;
            registered.atomic_update(|state| state.set_crtc_config(crtc, Some(&scanout)))?;
            // The published plane retains the framebuffer when this client reference is dropped.
        }
        let before = counts.disables.load(Ordering::Relaxed);
        drop(registration);
        // SAFETY: Unplug completed its shutdown transaction and no clients remain. retained
        // owns the device, and no further publication or framebuffer creation takes place.
        let (active, selected, framebuffers) = unsafe {
            (
                (*(*retained.crtc.load(Ordering::Relaxed)).state).active,
                (*(*retained.plane.load(Ordering::Relaxed)).state).fb,
                (*retained.as_raw()).mode_config.num_fb,
            )
        };
        // SAFETY: The retained device completed registration earlier in this test.
        let rejected = unsafe { retained.assume_ctx::<drm::Ioctl>() }
            .registration_guard()
            .is_none();
        drop(retained);
        assert_eq!(before, 0);
        assert_eq!(counts.enables.load(Ordering::Relaxed), 1);
        assert_eq!(counts.disables.load(Ordering::Relaxed), 1);
        assert!(!active);
        assert!(selected.is_null());
        assert_eq!(framebuffers, 0);
        assert!(rejected);
        assert_eq!(counts.connector_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn scoped_registration_retires_before_return() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-scoped", None)?;
        let retained: ARef<Device<TestDriver>> = drm::Registration::with_static(
            parent.as_ref().as_ref(),
            allocate(parent.as_ref(), &counts, false)?,
            Ok::<(), Error>(()),
            0,
            |registration| {
                let guard = registration.registration_guard().ok_or(ENODEV)?;
                guard.check_atomic_update(|_| Ok(()))?;
                Ok(registration.device().into())
            },
        )?;
        // SAFETY: The scoped operation completed registration and then synchronously unplugged.
        let rejected = unsafe { retained.assume_ctx::<drm::Ioctl>() }
            .registration_guard()
            .is_none();
        drop(parent);
        // The escaped allocation is still valid after removal, but no longer proves binding.
        let remaining = counts.objects.load(Ordering::Relaxed);
        drop(retained);
        assert!(rejected);
        assert_eq!(remaining, 4);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.connector_states.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn scoped_registration_failure_unplugs() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-scoped-failure", None)?;
        let mut retained: Option<ARef<Device<TestDriver>>> = None;
        let result: Result = drm::Registration::with_static(
            parent.as_ref().as_ref(),
            allocate(parent.as_ref(), &counts, false)?,
            Ok::<(), Error>(()),
            0,
            |registration| {
                retained = Some(registration.device().into());
                registration
                    .registration_guard()
                    .ok_or(ENODEV)?
                    .check_atomic_update(|_| Err(ECANCELED))
            },
        );
        let retained = retained.ok_or(ENODEV)?;
        // SAFETY: The callback ran after successful registration; failure still unplugs it.
        let rejected = unsafe { retained.assume_ctx::<drm::Ioctl>() }
            .registration_guard()
            .is_none();
        drop(retained);
        assert_eq!(result, Err(ECANCELED));
        assert!(rejected);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.connector_states.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn unplug_waits_for_registration_guard() -> Result {
        use crate::{sync::Completion, workqueue};

        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-concurrent-unplug", None)?;
        let start = Arc::pin_init(Completion::new(), GFP_KERNEL)?;
        let done = Arc::pin_init(Completion::new(), GFP_KERNEL)?;
        let finished = Arc::new(AtomicU32::new(0), GFP_KERNEL)?;
        // SAFETY: The registration is always destroyed before parent, including spawn failure.
        let registration = unsafe {
            drm::Registration::new_static(
                parent.as_ref().as_ref(),
                allocate(parent.as_ref(), &counts, false)?,
                Ok::<(), Error>(()),
                0,
            )?
        };
        let retained: ARef<Device<TestDriver>> = registration.device().into();
        // SAFETY: The device completed registration. Its independent reference lets the worker
        // own teardown while this task holds a guard through the already-registered view.
        let device = unsafe { retained.assume_ctx::<drm::Ioctl>() };
        // A tuple drops its fields in order even if allocating the work item fails. Capturing
        // the whole tuple with drop(owners) preserves registration-before-parent teardown.
        let owners = (registration, parent);
        let worker_start = start.clone();
        let worker_done = done.clone();
        let worker_finished = finished.clone();
        // Spawn before taking a guard: failed allocation drops the closure synchronously and
        // would otherwise wait for a guard held by this very task.
        workqueue::system_dfl().try_spawn(GFP_KERNEL, move || {
            worker_start.wait_for_completion();
            drop(owners);
            worker_finished.store(1, Ordering::Release);
            worker_done.complete_all();
        })?;
        let guard = device.registration_guard();
        // From here, every path releases the worker and joins it before assertions or return.
        start.complete_all();
        let mut closed = false;
        for _ in 0..1000 {
            if device.registration_guard().is_none() {
                closed = true;
                break;
            }
            // SAFETY: The test runs in sleepable task context, including the SRCU guard.
            unsafe { bindings::msleep(1) };
        }
        let early = finished.load(Ordering::Acquire);
        let check = guard
            .as_ref()
            .ok_or(ENODEV)
            .and_then(|guard| guard.check_atomic_update(|_| Ok(())));
        drop(guard);
        done.wait_for_completion();
        let rejected = device.registration_guard().is_none();
        drop(retained);
        assert!(closed);
        assert_eq!(early, 0);
        assert_eq!(check, Ok(()));
        assert!(rejected);
        assert_eq!(finished.load(Ordering::Acquire), 1);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn contended_atomic_check_retries_fresh_state() -> Result {
        let observed = contended_update(true, false)?;
        assert_eq!(observed.result, Ok(()));
        assert_eq!(observed.older_errno, 0);
        assert!(observed.attempts >= 2);
        assert!(observed.deadlocks >= 1);
        assert!(observed.fresh);
        assert_eq!(observed.published, 0);
        assert_eq!(observed.objects, 0);
        assert_eq!(observed.plane_states, 0);
        assert_eq!(observed.crtc_states, 0);
        Ok(())
    }

    #[test]
    fn contended_atomic_commit_retries_fresh_state() -> Result {
        let observed = contended_update(false, false)?;
        assert_eq!(observed.result, Ok(()));
        assert_eq!(observed.older_errno, 0);
        assert!(observed.attempts >= 2);
        assert!(observed.deadlocks >= 1);
        assert!(observed.fresh);
        assert_eq!(observed.published, 17);
        assert_eq!(observed.objects, 0);
        assert_eq!(observed.plane_states, 0);
        assert_eq!(observed.crtc_states, 0);
        Ok(())
    }

    #[test]
    fn consumed_deadlock_still_retries_before_commit() -> Result {
        let observed = contended_update(false, true)?;
        assert_eq!(observed.result, Ok(()));
        assert_eq!(observed.older_errno, 0);
        assert!(observed.attempts >= 2);
        assert!(observed.deadlocks >= 1);
        assert!(observed.fresh);
        assert_eq!(observed.published, 17);
        assert_eq!(observed.objects, 0);
        assert_eq!(observed.plane_states, 0);
        assert_eq!(observed.crtc_states, 0);
        Ok(())
    }

    #[test]
    fn unbacked_deadlock_error_is_not_retried() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-unbacked-deadlock", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let mut attempts = 0;
        // SAFETY: The device completed setup and remains unregistered on this task.
        let result = unsafe {
            atomic::run_update(&dev, |_| {
                attempts += 1;
                Err(EDEADLK)
            })
        };
        // SAFETY: The same setup and teardown exclusion holds for the subsequent operation.
        let retry = unsafe { atomic::run_check(&dev, |_| Ok(())) };
        drop(dev);
        assert_eq!(result, Err(EDEADLK));
        assert_eq!(attempts, 1);
        assert_eq!(retry, Ok(()));
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn plane_check_rejects_another_crtc() -> Result {
        use connector::AsRawConnector;
        use plane::{AsRawPlaneStatePrivate, RawPlaneState};

        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-plane-check-crtc", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        // SAFETY: The device remains unregistered and this task exclusively extends its setup.
        let setup = unsafe { UnregisteredKmsDevice::new(&dev) };
        let extra_plane = plane::UnregisteredPlane::<TestPlane>::new(
            &setup,
            0,
            &[fourcc::XRGB8888],
            Some(&[fourcc::FORMAT_MOD_LINEAR]),
            plane::Type::Primary,
            None,
            (),
        )?;
        let extra_crtc = crtc::UnregisteredCrtc::<TestCrtc>::new(
            &setup,
            extra_plane,
            None::<&plane::UnregisteredPlane<TestPlane>>,
            None,
            (),
        )?;
        // SAFETY: Complete only the newly added objects' initial states before any transaction.
        // The native helper skips the existing objects whose initial states already exist.
        crate::error::to_result(unsafe {
            bindings::drm_mode_config_create_initial_state(dev.as_raw())
        })?;
        // SAFETY: Setup is complete and all object references remain within the device lifetime.
        let extra_crtc = unsafe { crtc::Crtc::<TestCrtc>::from_raw(extra_crtc.as_raw()) };
        let crtc = unsafe { crtc::Crtc::<TestCrtc>::from_raw(dev.crtc.load(Ordering::Relaxed)) };
        let plane =
            unsafe { plane::Plane::<TestPlane>::from_raw(dev.plane.load(Ordering::Relaxed)) };
        let connector = unsafe {
            connector::Connector::<TestConnector>::from_raw(dev.connector.load(Ordering::Relaxed))
        };
        let fb = framebuffer(&dev)?;
        let mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[connector],
            position: (0, 0),
        };
        let mut derived_unchanged = true;
        // SAFETY: No further setup, registration or teardown occurs during either transaction.
        let rejected = unsafe {
            atomic::run_check(&dev, |mut state| {
                state.as_mut().set_crtc_config(crtc, Some(&scanout))?;
                let mut plane = state.add_plane_state(plane)?;
                let wrong = state.add_crtc_state(extra_crtc)?;
                let before = (plane.as_raw().src.x2, plane.as_raw().dst.x2);
                let result = plane.atomic_helper_check(&wrong, false, false);
                derived_unchanged &= before == (plane.as_raw().src.x2, plane.as_raw().dst.x2);
                result
            })
        };
        let accepted = unsafe {
            atomic::run_check(&dev, |mut state| {
                state.as_mut().set_crtc_config(crtc, Some(&scanout))?;
                let mut plane = state.add_plane_state(plane)?;
                let matching = state.add_crtc_state(crtc)?;
                plane.atomic_helper_check(&matching, false, false)
            })
        };
        drop(fb);
        drop(dev);
        assert_eq!(rejected, Err(EINVAL));
        assert_eq!(accepted, Ok(()));
        assert!(derived_unchanged);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn plane_check_rejects_another_transaction() -> Result {
        use plane::RawPlaneState;

        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-plane-check-state", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        // SAFETY: Setup completed and the device owns these objects until both checks finish.
        let crtc = unsafe { crtc::Crtc::<TestCrtc>::from_raw(dev.crtc.load(Ordering::Relaxed)) };
        let plane =
            unsafe { plane::Plane::<TestPlane>::from_raw(dev.plane.load(Ordering::Relaxed)) };
        // SAFETY: The device remains unregistered; no setup or teardown overlaps these checks.
        let rejected = unsafe {
            atomic::run_check(&dev, |state| {
                let mut plane = state.add_plane_state(plane)?;
                let raw =
                    NonNull::new(bindings::drm_atomic_commit_alloc(dev.as_raw())).ok_or(ENOMEM)?;
                // Use the same task's acquire context, not a nested lock-acquisition context.
                // The second transaction has separately allocated private state and is never
                // committed. It is dropped locally before the outer callback or context ends.
                (*raw.as_ptr()).acquire_ctx = (*state.as_raw()).acquire_ctx;
                let other = atomic::AtomicStateComposer::<TestDriver>::new(raw);
                let result = {
                    let other_crtc = other.add_crtc_state(crtc)?;
                    plane.atomic_helper_check(&other_crtc, false, false)
                };
                (*raw.as_ptr()).acquire_ctx = ptr::null_mut();
                drop(other);
                result
            })
        };
        // SAFETY: The same setup exclusion holds; the disabled plane and CRTC now share state.
        let accepted = unsafe {
            atomic::run_check(&dev, |state| {
                let mut plane = state.add_plane_state(plane)?;
                let matching = state.add_crtc_state(crtc)?;
                plane.atomic_helper_check(&matching, false, false)
            })
        };
        drop(dev);
        assert_eq!(rejected, Err(EINVAL));
        assert_eq!(accepted, Ok(()));
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn partial_object_setup_unwinds() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-unwind", None)?;
        assert_eq!(create(parent.as_ref(), &counts, true).err(), Some(EINVAL));
        assert_eq!(counts.setup_failures.load(Ordering::Relaxed), 1);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn foreign_primary_is_rejected() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent_a = faux::Registration::new(c"rust-kms-primary-a", None)?;
        let parent_b = faux::Registration::new(c"rust-kms-primary-b", None)?;
        let a = create(parent_a.as_ref(), &counts, false)?;
        let b = create(parent_b.as_ref(), &counts, false)?;
        // SAFETY: Both devices remain unregistered; the observed plane is owned by b and its
        // reference is confined to these device borrows. No transaction modifies either device.
        let dev_a = unsafe { UnregisteredKmsDevice::new(&a) };
        let foreign = unsafe {
            plane::UnregisteredPlane::<TestPlane>::from_raw(b.plane.load(Ordering::Relaxed))
        };
        assert_eq!(
            crtc::UnregisteredCrtc::<TestCrtc>::new(
                &dev_a,
                foreign,
                None::<&plane::UnregisteredPlane<TestPlane>>,
                None,
                (),
            )
            .err(),
            Some(EINVAL)
        );
        // SAFETY: b has completed setup, remains unregistered and is only accessed here.
        let dev_b = unsafe { UnregisteredKmsDevice::new(&b) };
        assert_eq!(dev_a.num_crtcs(), 1);
        assert_eq!(dev_b.num_crtcs(), 1);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 8);
        drop(a);
        drop(b);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn foreign_cursor_is_rejected() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent_a = faux::Registration::new(c"rust-kms-cursor-a", None)?;
        let parent_b = faux::Registration::new(c"rust-kms-cursor-b", None)?;
        let a = create(parent_a.as_ref(), &counts, false)?;
        let b = create(parent_b.as_ref(), &counts, false)?;
        // SAFETY: The devices are unregistered and own the observed objects for this scope.
        let dev_a = unsafe { UnregisteredKmsDevice::new(&a) };
        // SAFETY: b also remains unregistered; the extra cursor is owned by it until teardown.
        let dev_b = unsafe { UnregisteredKmsDevice::new(&b) };
        let primary = unsafe {
            plane::UnregisteredPlane::<TestPlane>::from_raw(a.plane.load(Ordering::Relaxed))
        };
        let foreign = plane::UnregisteredPlane::<TestPlane>::new(
            &dev_b,
            1,
            &[fourcc::ARGB8888],
            Some(&[fourcc::FORMAT_MOD_LINEAR]),
            plane::Type::Cursor,
            None,
            (),
        )?;
        assert_eq!(
            crtc::UnregisteredCrtc::<TestCrtc>::new(&dev_a, primary, Some(foreign), None, (),)
                .err(),
            Some(EINVAL)
        );
        assert_eq!(dev_a.num_crtcs(), 1);
        assert_eq!(dev_b.num_crtcs(), 1);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 9);
        drop(a);
        drop(b);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
