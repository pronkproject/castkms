// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Runtime consumers of the shared KMS interfaces. Devices are never registered with userspace.

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

#[derive(Clone, Default)]
struct PlanePayload;
#[derive(Clone, Default)]
struct CrtcPayload;
#[derive(Clone, Default)]
struct ConnectorPayload;

impl plane::DriverPlaneState for PlanePayload {
    type Plane = TestPlane;
}
impl crtc::DriverCrtcState for CrtcPayload {
    type Crtc = TestCrtc;
}
impl connector::DriverConnectorState for ConnectorPayload {
    type Connector = TestConnector;
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
}

fn create(
    parent: &faux::Device<device::Bound>,
    counts: &Arc<Counts>,
) -> Result<UnregisteredDevice<TestDriver>> {
    let drm = UnregisteredDevice::new(
        parent,
        try_pin_init!(Data {
            counts: counts.clone(),
            plane: AtomicPtr::new(ptr::null_mut()),
            crtc: AtomicPtr::new(ptr::null_mut()),
            connector: AtomicPtr::new(ptr::null_mut()),
        }),
    )?;
    // SAFETY: The device was just allocated and remains unregistered. Exercise the same setup
    // hook as normal registration without publishing a DRM minor or enabling external access.
    unsafe { <TestDriver as private::KmsImpl>::setup_kms(&drm) }?;
    Ok(drm)
}

#[kunit_tests(rust_drm_kms)]
mod cases {
    use super::*;

    #[test]
    fn initial_state_has_parents() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-state", None)?;
        let drm = create(parent.as_ref(), &counts)?;
        assert_eq!(drm.num_crtcs(), 1);
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
        let drm = create(parent.as_ref(), &counts)?;
        assert_eq!(counts.objects.load(Ordering::Relaxed), 4);
        drop(drm);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
