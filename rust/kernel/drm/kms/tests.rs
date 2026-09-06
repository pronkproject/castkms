// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Runtime consumers of the shared KMS interfaces for disposable test kernels.
//!
//! Most cases keep their devices unregistered. Registration cases publish temporary virtual
//! DRM devices, with no physical hardware or capture inputs.

mod inspection;

use super::*;
use crate::{
    drm::{self, fourcc, gem, UnregisteredDevice},
    faux,
    sync::Arc,
};
use core::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

#[derive(Default)]
struct Counts {
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
    // before using them; they never survive its teardown or expose objects to another thread.
    plane: AtomicPtr<bindings::drm_plane>,
    crtc: AtomicPtr<bindings::drm_crtc>,
    connector: AtomicPtr<bindings::drm_connector>,
}

struct TestDriver;
struct TestFile;
#[pin_data]
struct TestObject {}
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

impl gem::DriverObject for TestObject {
    type Driver = TestDriver;
    type Args = ();

    fn new(_: &Device<TestDriver>, _: usize, _: ()) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {})
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

// A fixed valid GEM framebuffer fixture, built with kernel helpers rather than a fake DRM file.
// Keep the raw setup here; display transactions below use the shared typed configuration API.
fn framebuffer(dev: &Device<TestDriver>) -> Result<ARef<framebuffer::Framebuffer<TestDriver>>> {
    use gem::IntoGEMObject;
    const FUNCS: bindings::drm_framebuffer_funcs = bindings::drm_framebuffer_funcs {
        destroy: Some(bindings::drm_gem_fb_destroy),
        create_handle: Some(bindings::drm_gem_fb_create_handle),
        dirty: None,
    };
    let object = gem::shmem::Object::<TestObject>::new(dev, 640 * 480 * 4, Default::default(), ())?;
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
    // Transfer the GEM reference to drm_gem_fb_destroy, and the framebuffer reference to ARef.
    let _ = ARef::into_raw(object);
    // SAFETY: The successful initializer gave us one owned framebuffer reference. Its C destroy
    // function releases the GEM reference and the kmalloc-compatible KBox allocation.
    Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(KBox::into_raw(fb).cast())) })
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

#[kunit_tests(rust_drm_kms)]
mod cases {
    use super::*;
    use crtc::AsRawCrtc;
    use encoder::AsRawEncoder;
    use plane::AsRawPlane;

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
